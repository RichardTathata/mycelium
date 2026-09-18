//! **The AgentFacts adapter** (v3 item 3 PR 7) — a self-signed AgentFacts document as a
//! knowledge-layer **claim**.
//!
//! # Why a claim, and why that is the whole point
//!
//! An AgentFacts document is **what a node says about itself**: its capabilities, its endpoints,
//! its locality. It is self-certified — the node signs it with its own identity, and the crate
//! docs are explicit that *trust is the fetcher's to decide*. In `docs/design/knowledge-layer.md`
//! §1's terms that is a **claim**, and the layer's resolution rules never count a claim as
//! evidence: however well-signed, a node cannot vouch for itself.
//!
//! So this adapter does not make AgentFacts *more* trusted. It makes them **legible to the layer
//! in the right category** — a fetcher can file the document beside the assessments others have
//! made of that node, link an assessment to it with `supports` or `challenges`, and let a reader
//! resolve with its own policy. The signature is verified so that a forged document yields no
//! record at all; the *kind* is what stops a genuine one being mistaken for evidence.
//!
//! # The issuer is the key, not the name
//!
//! `node_id` is a string anyone can type. The Ed25519 public key is what actually signed the
//! document. The record's issuer is the key (base64, as `SignedFacts` carries it), because an
//! issuer is *who can be held to the statement*, and only the key-holder can.

use mycelium::knowledge::{IssuerId, KnowledgeRecord, RecordError, RecordKind};

use crate::SignedFacts;

/// Why a signed document could not become a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// The self-signature does not verify. No record is produced from a document whose author
    /// cannot be established.
    BadSignature,
    /// The public key field is empty.
    EmptyKey,
    /// The record itself was malformed.
    Record(RecordError),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => f.write_str("AgentFacts self-signature does not verify"),
            Self::EmptyKey => f.write_str("AgentFacts document carries no public key"),
            Self::Record(e) => write!(f, "record: {e:?}"),
        }
    }
}

impl From<RecordError> for ClaimError {
    fn from(e: RecordError) -> Self {
        Self::Record(e)
    }
}

/// The subject a node's self-description is filed under: the node's own key.
pub fn subject_for(public_key_b64: &str) -> String {
    format!("agentfacts/{public_key_b64}")
}

/// **A verified AgentFacts document as a claim.**
///
/// The signature is checked first; a document that does not verify produces no record. The body is
/// the document's canonical bytes — the same bytes that were signed — so the record digests exactly
/// what the node put its name to.
///
/// `at_ms` is the caller's: pass the document's issuance time if you have the [`AgentFacts`] it was
/// built from, or the time you fetched it if you do not — and know which you passed. The adapter
/// does not read it out of the JSON-LD, because the NANDA field mapping lives in one place
/// (`to_nanda_jsonld`) and a second reader of those names would be a second place.
///
/// [`AgentFacts`]: crate::AgentFacts
pub fn claim(signed: &SignedFacts, at_ms: u64) -> Result<KnowledgeRecord, ClaimError> {
    if !signed.verify() {
        return Err(ClaimError::BadSignature);
    }
    let issuer = IssuerId::new(&signed.public_key_b64).ok_or(ClaimError::EmptyKey)?;
    let body = serde_json::to_vec(&signed.document).unwrap_or_default();
    Ok(KnowledgeRecord::new(
        issuer,
        RecordKind::Claim,
        at_ms,
        subject_for(&signed.public_key_b64),
        body,
        Vec::new(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer, SigningKey};
    use mycelium::knowledge::resolution::{classify, ReaderPolicy, ReleaseId, Verdict};
    use mycelium::knowledge::store::KnowledgeStore;
    use mycelium::knowledge::{Link, LinkKind, RecordId};
    use serde_json::json;

    fn b64() -> base64::engine::GeneralPurpose {
        base64::engine::general_purpose::STANDARD
    }

    /// A genuinely self-signed document from a fixed seed — no RNG, so the test is deterministic.
    fn signed(seed: u8, document: serde_json::Value) -> SignedFacts {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let bytes = serde_json::to_vec(&document).unwrap();
        let sig = sk.sign(&bytes);
        SignedFacts {
            document,
            alg: "ed25519",
            public_key_b64: b64().encode(sk.verifying_key().to_bytes()),
            signature_b64: b64().encode(sig.to_bytes()),
        }
    }

    #[test]
    fn a_verified_document_becomes_a_claim_issued_by_its_key() {
        let s = signed(1, json!({"name": "greenfield-coop", "capabilities": []}));
        let r = claim(&s, 1_000).expect("verifies");

        assert_eq!(r.kind(), RecordKind::Claim);
        assert_eq!(r.issuer().as_str(), s.public_key_b64, "the key, not the name");
        assert_eq!(r.subject(), subject_for(&s.public_key_b64));
        assert_eq!(r.body(), serde_json::to_vec(&s.document).unwrap(), "the signed bytes exactly");
        assert!(r.links().is_empty());
    }

    /// A tampered document yields **no record**, not a record with a bad flag.
    #[test]
    fn a_tampered_document_produces_no_record() {
        let mut s = signed(1, json!({"name": "greenfield-coop"}));
        s.document = json!({"name": "greenfield-coop", "capabilities": ["everything"]});
        assert_eq!(claim(&s, 1_000).unwrap_err(), ClaimError::BadSignature);
    }

    /// A document signed by one key but presented with another's public key does not verify: the
    /// issuer cannot be swapped underneath the signature.
    #[test]
    fn the_issuer_cannot_be_swapped_under_the_signature() {
        let a = signed(1, json!({"name": "a"}));
        let b = signed(2, json!({"name": "a"}));
        let mut forged = a.clone();
        forged.public_key_b64 = b.public_key_b64;
        assert_eq!(claim(&forged, 1_000).unwrap_err(), ClaimError::BadSignature);
    }

    /// **The point of the adapter.** A perfectly valid, perfectly signed AgentFacts document filed
    /// under a release's subject, **with a `supports` link and from an issuer other than the
    /// provider**, is still not evidence — so the *kind* is the only thing excluding it, and the
    /// kind is what this adapter chose. A node cannot vouch for itself, and the signature does not
    /// change that.
    #[test]
    fn a_signed_claim_is_still_not_evidence() {
        let release = ReleaseId::new("summarizer", "1.2.0");
        let provider = IssuerId::new("someone-else").unwrap();
        let s = signed(1, json!({"claims": "94% agreement"}));
        let r = claim(&s, 1_000).expect("verifies");
        // File it under the release with a supporting link, as a naive integration might — every
        // other condition for counting is deliberately met.
        let r = KnowledgeRecord::new(
            r.issuer().clone(),
            r.kind(),
            r.at_ms(),
            release.subject(),
            r.body().to_vec(),
            vec![Link {
                kind: LinkKind::Supports,
                target: RecordId { issuer: provider.clone(), digest: [1u8; 32] },
            }],
        )
        .unwrap();

        let mut store = KnowledgeStore::new();
        store.put(r);
        let v = classify(&store, &release, &provider, &ReaderPolicy::default(), 1_000);
        assert!(
            matches!(v, Verdict::InsufficientEvidence { have: 0, .. }),
            "a claim is what a node says about itself; it counts for nothing: {v:?}"
        );
    }
}
