//! The two causal fields a request carries into every record its work produces, and that no record mints
//! (adopter-surface X4: *one id, and one parent, through the records*).
//!
//! - `correlation` groups the records of one run. It is the caller's, one key segment — the same rule a reason
//!   trace's `run_id` obeys (2.33.0, `mycelium-reason::validate_run_id`): non-empty, at most
//!   [`MAX_CORRELATION_BYTES`], no `/`, no whitespace or control character.
//! - `parent` is the content hash of the record this work depends on: 64 lowercase hex characters, the SHA-256 an
//!   evidence record's `journal_sha256` cites and a trace line's hash under `mycelium explain --closure`.
//!
//! **Placement: the application payload, and only there.** For a request with JSON params, the two keys sit under
//! `params._meta` beside `operation_id` and `mandate`: `/mcp`'s `params._meta`; `/a2a`'s `params._meta` on the
//! legacy `tasks/send` and `params.metadata._meta` on the current generation's `message/send` (the same place the
//! mandate is read from, `a2a.rs`); the LLM doors' body `_meta`; the federation call's body `_meta`; `POST
//! /gateway/kv`'s top-level `correlation`/`parent`. Never a header, never the caller envelope: the gateway caller's signature covers a digest of the application payload
//! (`gateway_caller::signed_fields`), so a field placed outside it could be re-pointed under a signature that still
//! verifies — v2.12.0's lesson. A field carried as a header or in the envelope is never read, and it is counted once per
//! request against the door that saw it (`GET /stats` `causal_fields_misplaced`, a map by door: `mcp`, `a2a`, `llm`,
//! `federation`, `kv`, `caller_envelope`).
//!
//! A value that is present and malformed is refused at the door (`PreflightRefusal::Malformed`, `request_malformed`),
//! never corrected or dropped: a record with a silently emptied parent would read as a root.

use serde_json::Value;

/// The most bytes a `correlation` may hold — `mycelium-reason`'s `MAX_RUN_ID_BYTES`, the same door rule.
pub const MAX_CORRELATION_BYTES: usize = 128;

/// The header names the plan forbids, counted and never read.
pub const MISPLACED_HEADERS: [&str; 2] = ["x-mycelium-correlation", "x-mycelium-parent"];

/// What a request carried under `_meta`, validated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CausalFields {
    pub correlation: Option<String>,
    pub parent: Option<String>,
}

impl CausalFields {
    /// Neither field present.
    pub fn is_empty(&self) -> bool {
        self.correlation.is_none() && self.parent.is_none()
    }

    /// The `_meta` object that carries exactly these fields, for forwarding to a provider inside the application
    /// payload (never the envelope); `None` when there is nothing to carry.
    pub fn meta(&self) -> Option<serde_json::Map<String, Value>> {
        if self.is_empty() {
            return None;
        }
        let mut m = serde_json::Map::new();
        if let Some(c) = &self.correlation {
            m.insert("correlation".into(), Value::String(c.clone()));
        }
        if let Some(p) = &self.parent {
            m.insert("parent".into(), Value::String(p.clone()));
        }
        Some(m)
    }
}

/// Why a field was refused — one sentence a caller can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CausalFieldError(pub String);

impl std::fmt::Display for CausalFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CausalFieldError {}

/// Is `id` one key segment, as a `correlation` must be?
pub fn validate_correlation(id: &str) -> Result<(), CausalFieldError> {
    if id.is_empty() {
        return Err(CausalFieldError("_meta.correlation is empty".into()));
    }
    if id.len() > MAX_CORRELATION_BYTES {
        return Err(CausalFieldError(format!("_meta.correlation is {} bytes; at most {MAX_CORRELATION_BYTES}", id.len())));
    }
    if id.contains('/') {
        return Err(CausalFieldError("_meta.correlation holds a '/'; it is one key segment".into()));
    }
    if id.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(CausalFieldError("_meta.correlation holds whitespace or a control character".into()));
    }
    Ok(())
}

/// Is `hex` a record's content hash, as a `parent` must be: 64 lowercase hex characters?
pub fn validate_parent(hex: &str) -> Result<(), CausalFieldError> {
    if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(CausalFieldError("_meta.parent is not a content hash (64 lowercase hex characters)".into()));
    }
    Ok(())
}

/// The causal fields under `params._meta`, each absent, or present and valid. A present field that is not a string,
/// or a string that fails its rule, is an error naming the field — the door refuses, never corrects.
pub fn causal_fields(params: &Value) -> Result<CausalFields, CausalFieldError> {
    let meta = &params["_meta"];
    let correlation = match meta.get("correlation") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            validate_correlation(s)?;
            Some(s.clone())
        }
        Some(_) => return Err(CausalFieldError("_meta.correlation must be a string".into())),
    };
    let parent = match meta.get("parent") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            validate_parent(s)?;
            Some(s.clone())
        }
        Some(_) => return Err(CausalFieldError("_meta.parent must be a string".into())),
    };
    Ok(CausalFields { correlation, parent })
}

/// The content hash the records cite, over the exact bytes of a record as written: a journal frame's body, or one
/// trace line without its newline. Lowercase hex.
pub fn content_hash(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    let d: [u8; 32] = sha2::Sha256::digest(bytes).into();
    d.iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn absent_fields_are_absent_and_present_ones_are_carried() {
        assert_eq!(causal_fields(&json!({})).unwrap(), CausalFields::default());
        assert_eq!(causal_fields(&json!({"_meta": {"mandate": {}}})).unwrap(), CausalFields::default());
        let h = "a".repeat(64);
        let f = causal_fields(&json!({"_meta": {"correlation": "run-7", "parent": h}})).unwrap();
        assert_eq!(f, CausalFields { correlation: Some("run-7".into()), parent: Some(h.clone()) });
        assert_eq!(f.meta().unwrap(), json!({"correlation": "run-7", "parent": h}).as_object().unwrap().clone());
        assert!(CausalFields::default().meta().is_none());
    }

    #[test]
    fn a_correlation_is_one_key_segment() {
        for bad in ["", "a/b", "a b", "a\nb", "\u{7}", &"x".repeat(MAX_CORRELATION_BYTES + 1)] {
            assert!(causal_fields(&json!({"_meta": {"correlation": bad}})).is_err(), "{bad:?} admitted");
        }
        assert!(causal_fields(&json!({"_meta": {"correlation": 7}})).is_err(), "a number is not an id");
        assert!(causal_fields(&json!({"_meta": {"correlation": "x".repeat(MAX_CORRELATION_BYTES)}})).is_ok());
        assert!(causal_fields(&json!({"_meta": {"correlation": "run-7.é"}})).is_ok(), "unicode is a segment");
    }

    #[test]
    fn a_parent_is_a_content_hash() {
        for bad in ["", "abc", &"A".repeat(64), &"g".repeat(64), &"a".repeat(63), &"a".repeat(65)] {
            assert!(causal_fields(&json!({"_meta": {"parent": bad}})).is_err(), "{bad:?} admitted");
        }
        assert_eq!(content_hash(b"first").len(), 64);
        assert!(causal_fields(&json!({"_meta": {"parent": content_hash(b"first")}})).is_ok());
    }
}
