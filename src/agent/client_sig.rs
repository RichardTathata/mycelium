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

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Domain tag of the signed message.
pub const CLIENT_SIGNATURE_TAG: &[u8] = b"mycelium.gateway/client-signature/1";
/// The most `(t, c)` entries one instance keeps inside the freshness window; past it the oldest is evicted and
/// becomes that instance's floor (refused at or below), as the caller envelope's window does.
pub(crate) const CLIENT_WINDOW_PER_INSTANCE: usize = 65_536;
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

/// The bytes a client signs: [`CLIENT_SIGNATURE_TAG`], then the principal, the operation and the resource before `@`,
/// each as a `u32` little-endian length and its bytes, then the arguments' 32-byte digest, then `issued_at_ms`,
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
    let resource = resource.split('@').next().unwrap_or(resource);
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

/// SHA-256 of the arguments' canonical form (`serde_json`'s compact, key-sorted encoding).
pub fn client_arguments_digest(arguments: &serde_json::Value) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(serde_json::to_vec(arguments).unwrap_or_default()).into()
}

/// What a door signs as for one call: the operation, the resource and the arguments the provider will act on. A
/// `tools/call` is `("tools/call", "tool:{name}", arguments)`; an A2A send is `("skill.invoke", "skill:{id}",
/// {"text": message})`.
pub fn door_binding_mcp(name: &str, arguments: &serde_json::Value) -> (String, String, [u8; 32]) {
    ("tools/call".into(), format!("tool:{name}"), client_arguments_digest(arguments))
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
    /// The signature does not verify over this call's operation, resource and arguments.
    BadSignature,
    /// `k` is not the principal's current client key: superseded, never issued, or not a certificate of this fleet.
    KeyNotCurrent,
}

#[cfg(feature = "tls")]
impl ClientSigRefusal {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Malformed => "client_signature_malformed",
            Self::Unbound => "client_signature_unbound",
            Self::BadSignature => "client_signature_invalid",
            Self::KeyNotCurrent => "client_key_not_current",
        }
    }
}

#[cfg(feature = "tls")]
/// The operation, resource and arguments digest a provider acts on for a request of `kind`, read from its payload
/// (and, for a skill, the envelope's resource claim, which names the skill): `None` for a kind whose binding this
/// tree does not read.
pub(crate) fn binding_of(kind: &str, payload: &[u8], resource_claim: Option<&str>) -> Option<(String, String, [u8; 32])> {
    if kind == crate::signal::signal_kind::MCP_INVOKE {
        let v: serde_json::Value = serde_json::from_slice(payload).ok()?;
        let name = v["params"]["name"].as_str()?;
        let (op, res, d) = door_binding_mcp(name, &v["params"]["arguments"]);
        // The envelope's claim, where present, must name the same tool.
        if resource_claim.is_some_and(|r| r.split('@').next() != Some(res.as_str())) {
            return None;
        }
        return Some((op, res, d));
    }
    if kind == "skill.invoke" {
        let skill = resource_claim?.split('@').next()?.strip_prefix("skill:")?;
        let text = std::str::from_utf8(payload).ok()?;
        return Some(door_binding_a2a(skill, text));
    }
    None
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
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let key: [u8; 32] = b64.decode(&cs.k).ok().and_then(|v| v.try_into().ok()).ok_or(ClientSigRefusal::Malformed)?;
    let sig: [u8; 64] = b64.decode(&cs.sig).ok().and_then(|v| v.try_into().ok()).ok_or(ClientSigRefusal::Malformed)?;
    let instance = cn.instance().ok_or(ClientSigRefusal::Malformed)?;
    let (op, res, d) = binding_of(kind, payload, resource_claim).ok_or(ClientSigRefusal::Unbound)?;
    let msg = client_signature_message(principal, &op, &res, &d, cn.t, instance, cn.c);
    if !crate::tls::verify_bytes(&key, &msg, &sig) {
        return Err(ClientSigRefusal::BadSignature);
    }
    // After the signature, so a forgery answers as one.
    if super::client_cert::client_key_standing(ctx, principal, &key) != super::client_cert::ClientKeyStanding::Current {
        return Err(ClientSigRefusal::KeyNotCurrent);
    }
    Ok(ClientSigned { key, issued_at_ms: cn.t, instance, counter: cn.c })
}

/// One instance's `(t, c)` entries inside the freshness window, and the floor an eviction left.
#[derive(Default)]
struct InstanceWindow {
    seen: std::collections::BTreeSet<(u64, u64)>,
    floor: Option<(u64, u64)>,
}

/// One client key's instances and their windows, behind one leaf lock (lock-order row 76).
type KeyWindow = std::sync::Arc<parking_lot::Mutex<std::collections::HashMap<u64, InstanceWindow>>>;

/// Every client key's recent `cn`s, as one provider has received them: keyed by `(principal, key)`, then by
/// instance, each instance with **its own** eviction-only floor — a floor over `(t, instance, counter)` would order
/// honest processes by their random id. The window's rule is the caller envelope's: a delayed `cn` inside the window
/// is admitted, so a gateway reordering two honest calls trips nothing; one is refused only when seen before, at or
/// below its instance's floor (raised past [`CLIENT_WINDOW_PER_INSTANCE`] later ones), or past the freshness horizon
/// (refused before this, by `verify`).
#[derive(Default)]
pub(crate) struct ClientReplayWindows {
    by_key: papaya::HashMap<(String, [u8; 32]), KeyWindow>,
    /// `cn`s dropped as replays.
    pub(crate) replayed: AtomicU64,
    /// New instances refused because the key already had [`CLIENT_INSTANCES_PER_KEY`] inside the window.
    pub(crate) instances_refused: AtomicU64,
}

/// Why a `cn` was not admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClientReplay {
    /// Seen before, or at or below its instance's floor.
    Replayed,
    /// The key already has [`CLIENT_INSTANCES_PER_KEY`] instances inside the window.
    TooManyInstances,
}

impl ClientReplayWindows {
    /// Admit `signed` for `principal` once, at `horizon` (entries issued before it are forgotten: `verify` refuses
    /// them as stale).
    pub(crate) fn admit(&self, principal: &str, signed: &ClientSigned, horizon: u64) -> Result<(), ClientReplay> {
        let id = (principal.to_string(), signed.key);
        let window = std::sync::Arc::clone(self.by_key.pin().get_or_insert_with(id, Default::default));
        let mut w = window.lock();
        w.retain(|_, iw| {
            while iw.seen.first().is_some_and(|&(t, _)| t < horizon) {
                iw.seen.pop_first();
            }
            if iw.floor.is_some_and(|(t, _)| t < horizon) {
                iw.floor = None;
            }
            !iw.seen.is_empty() || iw.floor.is_some()
        });
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
}

#[cfg(feature = "tls")]
/// Calls made through a gateway door by a principal with a client certificate on record, carrying no `cs`, per door
/// kind (`tools/call`, `skill.invoke`). Counted now; refused at the refusal release (`deprecations.md` §87).
#[derive(Default)]
pub(crate) struct UnsignedActions {
    pub(crate) tools_call: AtomicU64,
    pub(crate) skill_invoke: AtomicU64,
}

#[cfg(feature = "tls")]
impl UnsignedActions {
    /// Count one, for `kind`.
    pub(crate) fn note(&self, kind: &'static str) {
        match kind {
            "tools/call" => self.tools_call.fetch_add(1, Ordering::Relaxed),
            _ => self.skill_invoke.fetch_add(1, Ordering::Relaxed),
        };
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_actions_unsigned_total", "kind" => kind).increment(1);
        tracing::warn!(kind, "a call from a principal with a client certificate on record carried no client signature (cs)");
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
