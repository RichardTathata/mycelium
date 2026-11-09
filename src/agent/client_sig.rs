//! A client's own signature on a gateway call — `cs` and `cn` (identity-and-lifecycle I1, decision D1′).
//!
//! ## What it is
//!
//! A service holding a client certificate (`mycelium tls issue --client`, `client_cert.rs`) signs each call it makes
//! through a gateway door with the certificate's key, and sends the signature in `params._meta`:
//!
//! - `cs` = `{ "k": base64(key, 32 bytes), "sig": base64(Ed25519 signature, 64 bytes) }`;
//! - `cn` = `{ "t": issued_at_ms, "i": instance (16 lowercase hex), "c": counter }`.
//!
//! The signature is over [`client_signature_message`]: the principal the call is made as, the operation, the
//! resource before `@`, the arguments' digest over their canonical form (`serde_json`'s compact, key-sorted
//! encoding — the digest the possession proof already uses), and `cn`. **Published format**: the SDKs compute the
//! same bytes, and one vector under `tests/fixtures/cs/` pins them in Rust, jest and pytest.
//!
//! ## Where it is checked
//!
//! The gateway carries `cs` and `cn` into the caller envelope as it carries a mandate, and the provider checks them
//! at its receive boundary (`gateway_caller::verify`), before any cache: the signature by `k`; `k` the **current**
//! key of the envelope's principal (`client_cert::client_key_standing`: a superseded or unknown key is refused);
//! the binding — the operation, resource and arguments the provider is about to act on are the ones signed; the
//! issue time inside the caller envelope's freshness window. Then `verify_received` admits `cn` once
//! ([`ClientReplayWindows`]).
//!
//! `instance` is a random 64-bit id a process draws once, so two processes under one certificate share the key and
//! no counter: without it their `(t, 5)` and `(t, 5)` would be one key and the second refused as a replay.
//!
//! The gateway's own signature does not cover `cs`/`cn`: they authenticate themselves. A relay that strips them
//! makes the call unsigned, which a gateway counts (`gateway_actions_unsigned{kind}`) and, at the refusal release,
//! refuses.

#[cfg(feature = "tls")]
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Domain tag of the signed message.
pub const CLIENT_SIGNATURE_TAG: &[u8] = b"mycelium.gateway/client-signature/1";
#[cfg(feature = "tls")]
/// The most `(t, c)` entries one instance keeps inside the freshness window; past it the oldest is evicted and
/// becomes that instance's floor (refused at or below), as the caller envelope's window does.
pub(crate) const CLIENT_WINDOW_PER_INSTANCE: usize = 65_536;
#[cfg(feature = "tls")]
/// The most instances one `(principal, key)` keeps inside the freshness window. A new instance past it is refused
/// (counted): only the key's holder can mint instances, so this bounds the memory a stolen key can take.
pub(crate) const CLIENT_INSTANCES_PER_KEY: usize = 1_024;

/// `cs`: who signed, and the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientSignature {
    /// Base64 of the client certificate's Ed25519 key (32 bytes).
    pub k: String,
    /// Base64 of the Ed25519 signature (64 bytes) over [`client_signature_message`].
    pub sig: String,
}

/// `cn`: when, from which process, and its count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientNonce {
    /// The client's wall clock (ms) when it signed.
    pub t: u64,
    /// The process's instance id: 16 lowercase hex digits (a `u64`, as hex so a JSON reader keeps all 64 bits).
    pub i: String,
    /// Monotonic within the process.
    pub c: u64,
}

impl ClientNonce {
    /// The instance id, if `i` is 16 lowercase hex digits.
    pub fn instance(&self) -> Option<u64> {
        (self.i.len() == 16 && self.i.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            .then(|| u64::from_str_radix(&self.i, 16).ok())
            .flatten()
    }
}

/// The bytes a client signs: [`CLIENT_SIGNATURE_TAG`], then the principal, the operation and the resource — the bare
/// resource, `tool:{name}` or `skill:{id}`, **whole**, so a name holding `@` is bound in full and no provider is named
/// (#745's confirm round) — each as a `u32` little-endian length and its bytes, then the arguments' 32-byte digest, then `issued_at_ms`,
/// `instance` and `counter` as `u64` little-endian.
pub fn client_signature_message(
    principal: &str,
    operation: &str,
    resource: &str,
    arguments_digest: &[u8; 32],
    issued_at_ms: u64,
    instance: u64,
    counter: u64,
) -> Vec<u8> {
    let mut out = CLIENT_SIGNATURE_TAG.to_vec();
    for part in [principal.as_bytes(), operation.as_bytes(), resource.as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(arguments_digest);
    out.extend_from_slice(&issued_at_ms.to_le_bytes());
    out.extend_from_slice(&instance.to_le_bytes());
    out.extend_from_slice(&counter.to_le_bytes());
    out
}

/// SHA-256 of the arguments' canonical form: [`canonical_json`], RFC 8785 (JCS), which the three languages reproduce
/// byte for byte — `serde_json`'s own encoding writes numbers as ryu does (`1e-5`, `1e16`), Python's `json.dumps` as
/// `1e-05` and `1e+16`, JavaScript's as `1e+21` (#742's review, finding 1).
pub fn client_arguments_digest(arguments: &serde_json::Value) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(canonical_json(arguments).as_bytes()).into()
}

/// A JSON value in the JSON Canonicalization Scheme (RFC 8785): no whitespace; object members sorted by their names'
/// UTF-16 code units; strings escaped as ECMAScript's `JSON.stringify` escapes them (`"`, `\\`, the short forms
/// `\b \f \n \r \t`, other controls as `\u00xx`, everything else literal); numbers as ECMAScript's
/// `Number.prototype.toString` writes the IEEE-754 double. An integer beyond ±2^53 is written as the double it rounds
/// to, as a JavaScript client would have parsed it.
pub fn canonical_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_jcs(value, &mut out);
    out
}

fn write_jcs(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&jcs_number(n)),
        Value::String(s) => jcs_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, v) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_jcs(v, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                jcs_string(k, out);
                out.push(':');
                write_jcs(&map[k], out);
            }
            out.push('}');
        }
    }
}

fn jcs_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

fn jcs_number(n: &serde_json::Number) -> String {
    if let Some(u) = n.as_u64()
        && u <= MAX_SAFE_INTEGER
    {
        return u.to_string();
    }
    if let Some(i) = n.as_i64()
        && i.unsigned_abs() <= MAX_SAFE_INTEGER
    {
        return i.to_string();
    }
    es_number(n.as_f64().unwrap_or(f64::NAN))
}

/// ECMAScript's `Number::toString` for a finite double, from the shortest round-trip digits.
fn es_number(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        // JSON has no NaN or infinity; -0 is written "0".
        return "0".into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    // `{:e}` writes the shortest digits that round-trip: `d[.ddd]e[-]x`.
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let k = digits.len() as i64;
    let n = exp.parse::<i64>().unwrap_or(0) + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e < 0 { format!("-{}", -e) } else { format!("+{e}") };
        if k == 1 { format!("{digits}e{e}") } else { format!("{}.{}e{e}", &digits[..1], &digits[1..]) }
    };
    format!("{sign}{body}")
}

/// What a door signs as for one call: the operation, the resource and the arguments the provider will act on. A
/// `tools/call` is `("tools/call", "tool:{name}", arguments)`; an A2A send is `("skill.invoke", "skill:{id}",
/// {"text": message})`.
///
/// Refuses ([`UnsafeInteger`]) arguments holding an integer outside ±(2^53 − 1): RFC 8785 writes it as the nearest
/// double, so `9007199254740993` and `9007199254740992` would digest the same while the tool receives the exact
/// integer, and a relay could swap one for the other under a signature that still verifies (#742's confirm round).
/// Send large ids as strings.
pub fn door_binding_mcp(name: &str, arguments: &serde_json::Value) -> Result<(String, String, [u8; 32]), UnsafeInteger> {
    if holds_unsafe_integer(arguments) {
        return Err(UnsafeInteger);
    }
    Ok(("tools/call".into(), format!("tool:{name}"), client_arguments_digest(arguments)))
}

/// A signed call's arguments held an integral number outside ±(2^53 − 1), which the canonical form cannot bind
/// exactly. Send it as a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnsafeInteger;

impl std::fmt::Display for UnsafeInteger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an integer outside ±(2^53 - 1) cannot be bound exactly by a client signature: send it as a string")
    }
}

impl std::error::Error for UnsafeInteger {}

/// Does `value` hold, at any depth, a number whose magnitude is 2^53 or more, float or not (`1e16`, `6.022e23`)? An
/// integer literal is judged exactly; a float by its value — every double of that magnitude is an integer, and
/// JavaScript cannot tell `1e16` from `10000000000000000`, so the three languages draw the line in the same place.
pub fn holds_unsafe_integer(value: &serde_json::Value) -> bool {
    use serde_json::Value;
    match value {
        Value::Number(n) => {
            if let Some(u) = n.as_u64() {
                u > MAX_SAFE_INTEGER
            } else if let Some(i) = n.as_i64() {
                i.unsigned_abs() > MAX_SAFE_INTEGER
            } else {
                n.as_f64().is_some_and(|f| f.fract() == 0.0 && f.abs() > MAX_SAFE_INTEGER as f64)
            }
        }
        Value::Array(items) => items.iter().any(holds_unsafe_integer),
        Value::Object(map) => map.values().any(holds_unsafe_integer),
        _ => false,
    }
}

/// [`door_binding_mcp`] for an A2A send of `text` to `skill_id`.
pub fn door_binding_a2a(skill_id: &str, text: &str) -> (String, String, [u8; 32]) {
    ("skill.invoke".into(), format!("skill:{skill_id}"), client_arguments_digest(&serde_json::json!({ "text": text })))
}

#[cfg(feature = "tls")]
/// A service's signer: its certificate's key and principal, one instance id drawn at construction, and a counter.
/// Sign each call with [`ClientSigner::sign`] and send the result as `params._meta.cs` / `cn`
/// ([`ClientSigner::meta`]).
pub struct ClientSigner {
    key: ed25519_dalek::SigningKey,
    principal: String,
    instance: u64,
    counter: AtomicU64,
}

#[cfg(feature = "tls")]
impl std::fmt::Debug for ClientSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientSigner").field("principal", &self.principal).field("instance", &self.instance).finish()
    }
}

#[cfg(feature = "tls")]
impl ClientSigner {
    /// A signer for `principal` (`token:{issuer}/{name}`, as the certificate names it), drawing its instance id
    /// through the replay seam.
    pub fn new(key: ed25519_dalek::SigningKey, principal: impl Into<String>) -> Self {
        let instance = mycelium_core::sim_seam::rng_u64_from("client-sig/instance", 0);
        Self::with_instance(key, principal, instance)
    }

    /// A signer with a given instance id (a test, or a replay taking the recorded one).
    pub fn with_instance(key: ed25519_dalek::SigningKey, principal: impl Into<String>, instance: u64) -> Self {
        Self { key, principal: principal.into(), instance, counter: AtomicU64::new(0) }
    }

    /// Sign one call at `issued_at_ms`. Each call takes the next counter.
    pub fn sign_at(&self, operation: &str, resource: &str, arguments_digest: &[u8; 32], issued_at_ms: u64) -> (ClientSignature, ClientNonce) {
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        let msg = client_signature_message(&self.principal, operation, resource, arguments_digest, issued_at_ms, self.instance, counter);
        let sig = self.key.sign(&msg).to_bytes();
        let b64 = base64::engine::general_purpose::STANDARD;
        (
            ClientSignature { k: b64.encode(self.key.verifying_key().to_bytes()), sig: b64.encode(sig) },
            ClientNonce { t: issued_at_ms, i: format!("{:016x}", self.instance), c: counter },
        )
    }

    /// [`Self::sign_at`] on the wall clock.
    pub fn sign(&self, operation: &str, resource: &str, arguments_digest: &[u8; 32]) -> (ClientSignature, ClientNonce) {
        self.sign_at(operation, resource, arguments_digest, mycelium_core::sim_seam::wall_now_ms())
    }

    /// Sign a `tools/call` of `name` with `arguments` ([`door_binding_mcp`]); refuses arguments holding an integer
    /// outside ±(2^53 − 1) — send large ids as strings.
    pub fn sign_tools_call(&self, name: &str, arguments: &serde_json::Value) -> Result<(ClientSignature, ClientNonce), UnsafeInteger> {
        let (op, res, d) = door_binding_mcp(name, arguments)?;
        Ok(self.sign(&op, &res, &d))
    }

    /// `{"cs": …, "cn": …}`, to merge into `params._meta`.
    pub fn meta(signed: &(ClientSignature, ClientNonce)) -> serde_json::Value {
        serde_json::json!({ "cs": signed.0, "cn": signed.1 })
    }
}

#[cfg(feature = "gateway")]
/// `cs` and `cn` from a request's `params._meta`: both or neither. `Err` names what is malformed.
pub(crate) fn from_params(params: &serde_json::Value) -> Result<Option<(ClientSignature, ClientNonce)>, String> {
    let meta = params.get("_meta");
    let cs = meta.and_then(|m| m.get("cs")).filter(|v| !v.is_null());
    let cn = meta.and_then(|m| m.get("cn")).filter(|v| !v.is_null());
    match (cs, cn) {
        (None, None) => Ok(None),
        (Some(cs), Some(cn)) => {
            let cs: ClientSignature = serde_json::from_value(cs.clone()).map_err(|e| format!("_meta.cs: {e}"))?;
            let cn: ClientNonce = serde_json::from_value(cn.clone()).map_err(|e| format!("_meta.cn: {e}"))?;
            if cn.instance().is_none() {
                return Err("_meta.cn.i: 16 lowercase hex digits".into());
            }
            Ok(Some((cs, cn)))
        }
        _ => Err("_meta.cs and _meta.cn travel together".into()),
    }
}

/// A verified client signature, as the provider read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientSigned {
    /// The client certificate's key that signed (the principal's current key when verified).
    pub key: [u8; 32],
    /// The client's issue time (ms).
    pub issued_at_ms: u64,
    /// The process's instance id.
    pub instance: u64,
    /// The process's counter.
    pub counter: u64,
}

#[cfg(feature = "tls")]
/// Why a client signature was refused (each denies; `verify` maps it to a `CallerError`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientSigRefusal {
    /// `k` or `sig` is not base64 of the right length, or `i` is not an instance id.
    Malformed,
    /// The provider cannot read the operation, resource or arguments of this kind, so it cannot check the binding.
    Unbound,
    /// The arguments hold an integer outside ±(2^53 − 1), which the canonical form cannot bind exactly.
    UnsafeInteger,
    /// The signature does not verify over this call's operation, resource and arguments.
    BadSignature,
    /// `k` has no verified certificate under the principal here: never issued, another fleet's, or issued so recently
    /// that its record has not reached this node yet — **retriable** while the record travels.
    KeyUnknown,
    /// `k` is a superseded key: a newer certificate for the principal has been read here. Final.
    KeySuperseded,
    /// The issue time is outside the caller envelope's window: a clock skewed past the drift bound, or a held call.
    /// Built by the gateway's door only (the provider answers `CallerError::Stale`).
    #[cfg_attr(not(feature = "gateway"), allow(dead_code))]
    Stale,
    /// This node has no identity (`[tls]` unset in a build with `tls`), so it holds no CA anchor and judges no client
    /// certificate: the signature cannot be checked here, rather than its key being unknown (#745's review).
    Unverifiable,
}

#[cfg(feature = "tls")]
impl ClientSigRefusal {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Malformed => "client_signature_malformed",
            Self::Unbound => "client_signature_unbound",
            Self::UnsafeInteger => "client_signature_unsafe_integer",
            Self::BadSignature => "client_signature_invalid",
            Self::KeyUnknown => "client_key_unknown",
            Self::KeySuperseded => "client_key_superseded",
            Self::Stale => "stale",
            Self::Unverifiable => "client_signature_unverifiable",
        }
    }
}

#[cfg(feature = "tls")]
/// The operation, resource and arguments digest a provider acts on for a request of `kind`, read from its payload
/// (and, for a skill, the envelope's resource claim, which names the skill): `None` for a kind whose binding this
/// tree does not read.
pub(crate) fn binding_of(kind: &str, payload: &[u8], resource_claim: Option<&str>) -> Result<(String, String, [u8; 32]), ClientSigRefusal> {
    let unbound = ClientSigRefusal::Unbound;
    if kind == crate::signal::signal_kind::MCP_INVOKE {
        let v: serde_json::Value = serde_json::from_slice(payload).map_err(|_| unbound)?;
        let name = v["params"]["name"].as_str().ok_or(unbound)?;
        let (op, res, d) = door_binding_mcp(name, &v["params"]["arguments"]).map_err(|_| ClientSigRefusal::UnsafeInteger)?;
        // The envelope's claim, where present, must name the same tool.
        // The claim is `{resource}@{provider}`: the provider is after the **last** `@`, so a name holding one still binds
        // (#745's review).
        if resource_claim.is_some_and(|r| r.rsplit_once('@').map_or(r, |(head, _)| head) != res.as_str()) {
            return Err(unbound);
        }
        return Ok((op, res, d));
    }
    if kind == "skill.invoke" {
        let skill = resource_claim.map(|r| r.rsplit_once('@').map_or(r, |(head, _)| head)).and_then(|r| r.strip_prefix("skill:")).ok_or(unbound)?;
        let text = std::str::from_utf8(payload).map_err(|_| unbound)?;
        return Ok(door_binding_a2a(skill, text));
    }
    Err(unbound)
}

/// Check `cs`/`cn` for a call made as `principal`: the binding read from the request, the signature, and the key's
/// standing. The issue time's window and the replay window are checked by the caller.
#[cfg(feature = "tls")]
pub(crate) fn check(
    ctx: &super::TaskCtx,
    principal: &str,
    kind: &str,
    payload: &[u8],
    resource_claim: Option<&str>,
    cs: &ClientSignature,
    cn: &ClientNonce,
) -> Result<ClientSigned, ClientSigRefusal> {
    let binding = binding_of(kind, payload, resource_claim)?;
    check_bound(ctx, principal, &binding, cs, cn)
}

/// [`check`] over a binding already read: the signature over it, then the key's standing.
#[cfg(feature = "tls")]
fn check_bound(ctx: &super::TaskCtx, principal: &str, binding: &(String, String, [u8; 32]), cs: &ClientSignature, cn: &ClientNonce) -> Result<ClientSigned, ClientSigRefusal> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64.decode(&cs.k).ok().and_then(|v| v.try_into().ok()).ok_or(ClientSigRefusal::Malformed)?;
    let sig: [u8; 64] = b64.decode(&cs.sig).ok().and_then(|v| v.try_into().ok()).ok_or(ClientSigRefusal::Malformed)?;
    let instance = cn.instance().ok_or(ClientSigRefusal::Malformed)?;
    if ctx.tls.get().is_none() {
        return Err(ClientSigRefusal::Unverifiable);
    }
    let (op, res, d) = binding;
    let msg = client_signature_message(principal, op, res, d, cn.t, instance, cn.c);
    if !crate::tls::verify_bytes(&key, &msg, &sig) {
        return Err(ClientSigRefusal::BadSignature);
    }
    // After the signature, so a forgery answers as one.
    match super::client_cert::client_key_standing(ctx, principal, &key) {
        super::client_cert::ClientKeyStanding::Current => {}
        super::client_cert::ClientKeyStanding::Superseded => return Err(ClientSigRefusal::KeySuperseded),
        _ => return Err(ClientSigRefusal::KeyUnknown),
    }
    Ok(ClientSigned { key, issued_at_ms: cn.t, instance, counter: cn.c })
}

/// **The door's own check of `cs`, before it claims a grant's use** (identity-and-lifecycle I1, #744's review): the
/// signature over the door's binding — [`door_binding_mcp`] or [`door_binding_a2a`], what the provider will bind — and
/// the key's standing ([`client_key_standing`](super::client_cert::client_key_standing)). Not `cn`: the replay window is
/// the provider's, which checks everything again at its receive boundary. A call this refuses never reaches the
/// preflight, so a grant it presents is not spent. `None` principal (an unauthenticated caller): nothing to bind the
/// signature to here, and the provider decides.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub(crate) fn check_at_door(
    ctx: &super::TaskCtx,
    principal: Option<&str>,
    binding: Result<(String, String, [u8; 32]), UnsafeInteger>,
    signed: &(ClientSignature, ClientNonce),
) -> Result<(), ClientSigRefusal> {
    let Some(principal) = principal else { return Ok(()) };
    let binding = binding.map_err(|_| ClientSigRefusal::UnsafeInteger)?;
    let checked = check_bound(ctx, principal, &binding, &signed.0, &signed.1)?;
    // The issue time on the provider's window: a skewed clock is refused here, before a grant is spent.
    if !super::gateway_caller::client_issue_time_fresh(ctx, checked.issued_at_ms, ctx.hlc.decision_now_ms()) {
        return Err(ClientSigRefusal::Stale);
    }
    Ok(())
}

#[cfg(feature = "tls")]
/// One instance's `(t, c)` entries inside the freshness window, and the floor an eviction left.
#[derive(Default)]
struct InstanceWindow {
    seen: std::collections::BTreeSet<(u64, u64)>,
    floor: Option<(u64, u64)>,
}

#[cfg(feature = "tls")]
/// One principal's client keys, each with its instances and their windows, behind one leaf lock (lock-order row 76).
type PrincipalWindow = std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<[u8; 32], std::collections::HashMap<u64, InstanceWindow>>>>;

#[cfg(feature = "tls")]
/// Every client principal's recent `cn`s, as one provider has received them: keyed by principal, then by key, then by
/// instance, each instance with **its own** eviction-only floor — a floor over `(t, instance, counter)` would order
/// honest processes by their random id. The window's rule is the caller envelope's: a delayed `cn` inside the window
/// is admitted, so a gateway reordering two honest calls trips nothing; one is refused only when seen before, at or
/// below its instance's floor (raised past [`CLIENT_WINDOW_PER_INSTANCE`] later ones), or past the freshness horizon
/// (refused before this, by `verify`). **In memory only**: a provider that restarts forgets every `cn`, so a call
/// captured before the restart replays once after it, inside the freshness window (stated).
///
/// Bounded: a principal enters only through a signature by its current key, and each admission drops the principal's
/// keys that are neither the signing key nor its current one — so a principal holds its current key's windows, and
/// briefly a racing predecessor's (#742's review, item 7).
#[derive(Default)]
pub(crate) struct ClientReplayWindows {
    by_principal: papaya::HashMap<String, PrincipalWindow>,
    /// `cn`s dropped as replays. `GET /stats` `client_signatures_replayed`.
    pub(crate) replayed: AtomicU64,
    /// New instances refused because the key already had [`CLIENT_INSTANCES_PER_KEY`] inside the window.
    /// `GET /stats` `client_instances_refused`.
    pub(crate) instances_refused: AtomicU64,
}

#[cfg(feature = "tls")]
/// Why a `cn` was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientReplay {
    /// Seen before, or at or below its instance's floor: a replay, dropped unanswered.
    Replayed,
    /// The key already has [`CLIENT_INSTANCES_PER_KEY`] instances inside the window: not a replay, refused with an
    /// answer (`client_instances_exhausted`) so a signer that draws an instance per invocation is told, not left to
    /// time out.
    TooManyInstances,
}

#[cfg(feature = "tls")]
impl ClientReplayWindows {
    /// Admit `signed` for `principal` once, at `horizon` (entries issued before it are forgotten: `verify` refuses
    /// them as stale). `current` is the principal's current key as this node reads it now.
    pub(crate) fn admit(&self, principal: &str, signed: &ClientSigned, current: Option<[u8; 32]>, horizon: u64) -> Result<(), ClientReplay> {
        let window = std::sync::Arc::clone(self.by_principal.pin().get_or_insert_with(principal.to_string(), Default::default));
        let mut keys = window.lock();
        keys.retain(|k, instances| {
            instances.retain(|_, iw| {
                while iw.seen.first().is_some_and(|&(t, _)| t < horizon) {
                    iw.seen.pop_first();
                }
                if iw.floor.is_some_and(|(t, _)| t < horizon) {
                    iw.floor = None;
                }
                !iw.seen.is_empty() || iw.floor.is_some()
            });
            *k == signed.key || (Some(*k) == current && !instances.is_empty())
        });
        let w = keys.entry(signed.key).or_default();
        if !w.contains_key(&signed.instance) && w.len() >= CLIENT_INSTANCES_PER_KEY {
            self.instances_refused.fetch_add(1, Ordering::Relaxed);
            return Err(ClientReplay::TooManyInstances);
        }
        let iw = w.entry(signed.instance).or_default();
        let key = (signed.issued_at_ms, signed.counter);
        if iw.floor.is_some_and(|f| key <= f) || !iw.seen.insert(key) {
            self.replayed.fetch_add(1, Ordering::Relaxed);
            return Err(ClientReplay::Replayed);
        }
        if iw.seen.len() > CLIENT_WINDOW_PER_INSTANCE
            && let Some(oldest) = iw.seen.pop_first()
        {
            iw.floor = Some(iw.floor.map_or(oldest, |f| f.max(oldest)));
        }
        Ok(())
    }

    /// How many keys `principal` holds windows for (tests).
    #[cfg(test)]
    pub(crate) fn keys_held(&self, principal: &str) -> usize {
        self.by_principal.pin().get(principal).map_or(0, |w| w.lock().len())
    }
}

#[cfg(feature = "tls")]
/// Calls made through a gateway door by a principal with a client certificate on record, carrying no `cs`, per door
/// kind (`tools/call`, `skill.invoke`). Counted now; refused at the refusal release (`deprecations.md` §87).
#[derive(Default)]
pub(crate) struct UnsignedActions {
    /// At the gateway's doors — they exist only with `gateway` (a `tls` build without it, such as `mycelium-wiki`'s
    /// `execution-authority`, counts only at the provider).
    #[cfg(feature = "gateway")]
    pub(crate) tools_call: AtomicU64,
    #[cfg(feature = "gateway")]
    pub(crate) skill_invoke: AtomicU64,
    /// The same, counted where the call runs: a provider receiving an unsigned `mcp.invoke` or `skill.invoke` for a
    /// principal whose certificate it holds — what a gateway that strips `cs` leaves behind (#742's review, item 4).
    pub(crate) at_provider: AtomicU64,
}

#[cfg(feature = "tls")]
impl UnsignedActions {
    /// Count one, for `kind`.
    #[cfg(feature = "gateway")]
    pub(crate) fn note(&self, kind: &'static str) {
        match kind {
            "tools/call" => self.tools_call.fetch_add(1, Ordering::Relaxed),
            _ => self.skill_invoke.fetch_add(1, Ordering::Relaxed),
        };
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_actions_unsigned_total", "kind" => kind).increment(1);
        tracing::warn!(kind, "a call from a principal with a client certificate on record carried no client signature (cs)");
    }

    /// Count one received at this provider.
    pub(crate) fn note_at_provider(&self, kind: &str) {
        self.at_provider.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_provider_actions_unsigned_total", "kind" => kind.to_string()).increment(1);
        let _ = kind;
    }
}

/// What a gateway door decided about a call's `cs`/`cn` before dispatching it.
#[cfg(feature = "gateway")]
pub(crate) enum AtDoor {
    /// Dispatch, carrying the client's signature when it sent one.
    Proceed(Option<(ClientSignature, ClientNonce)>),
    /// Refuse: a JSON-RPC `-32602` with this reason and message.
    Refuse { reason: &'static str, message: String },
}

/// The door's part (identity-and-lifecycle I1): read `cs`/`cn` from `params._meta` (both or neither); refuse a signed
/// call at a gateway without `gateway_identity_issuer` — its principals are then qualified by the node id, which no
/// client certificate can name, so the signature could never be the principal's — and count an unsigned call from a
/// principal with a client certificate on record, as `kind`.
#[cfg(feature = "gateway")]
pub(crate) fn at_door(ctx: &super::TaskCtx, principal: Option<&str>, params: &serde_json::Value, kind: &'static str) -> AtDoor {
    match from_params(params) {
        Err(why) => AtDoor::Refuse { reason: "request_malformed", message: format!("request malformed: {why}") },
        Ok(Some(_)) if ctx.config.gateway_identity_issuer.is_none() => AtDoor::Refuse {
            reason: "gateway_identity_issuer_unset",
            message: "a client-signed call needs this gateway's gateway_identity_issuer set: without it principals are \
                      qualified by the node id, which no client certificate names"
                .into(),
        },
        Ok(Some(signed)) => AtDoor::Proceed(Some(signed)),
        Ok(None) => {
            #[cfg(feature = "tls")]
            if let Some(p) = principal
                && ctx.client_certs.mark(p).is_some()
            {
                ctx.unsigned_actions.note(kind);
            }
            #[cfg(not(feature = "tls"))]
            let _ = (principal, kind);
            AtDoor::Proceed(None)
        }
    }
}

/// Does `params._meta` carry `cs` or `cn`? A door that binds no client signature (the LLM doors) refuses such a call
/// rather than drop the signature silently (#742's review, item 4).
#[cfg(feature = "llm")]
pub(crate) fn carries_client_signature(params: &serde_json::Value) -> bool {
    params.get("_meta").is_some_and(|m| m.get("cs").is_some_and(|v| !v.is_null()) || m.get("cn").is_some_and(|v| !v.is_null()))
}
