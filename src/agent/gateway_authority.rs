//! **Mandates established at the gateway** (Boundary H item A1, gateway wiring) —
//! [`docs/design/authority-at-execution.md`](../../../docs/design/authority-at-execution.md) §6.
//!
//! # The gap this closes
//!
//! AE1 gave the action envelope a slot for the enforcement point's finding about a mandate, and the
//! gateway always filled it with `None`: it held nothing it could verify. So a policy rule that
//! **requires** a mandate could never be satisfied at the gateway — it always answered
//! `Indeterminate`. With P2's portable grants and A1's execution gate, the gateway can now
//! **establish** a mandate for the call in front of it.
//!
//! # What a caller presents, and what the gateway checks
//!
//! In `params._meta.mandate`: a [`PresentedMandate`] — a `SignedMandateGrant` and a base64 possession
//! proof. The gateway, when an operator has attached an [`ExecutionAuthority`]:
//!
//! 1. requires the grant's **holder to be the authenticated caller** — the principal the auth layer
//!    resolved, never anything the request asserts;
//! 2. verifies the grant through P2 (issued, entitled by configuration, current, possessed), with
//!    the possession proof bound to **this call** ([`possession_request`]: operation, the resource
//!    before `@`, and the arguments digest);
//! 3. runs A1's [`ExecutionGate`] at dispatch: the resource's own epoch, scope, window and operation
//!    check, and fresh revocation standing;
//! 4. binds the result into the envelope as `Established`, `Refused` or `Unknown`, and clamps the
//!    envelope's `not_after_ms` to the mandate's window.
//!
//! No authority attached ⇒ exactly the behaviour before: `mandate: None`.
//!
//! # The operation a grant must enumerate
//!
//! `{operation}:{resource before '@'}` — e.g. `skill.invoke:skill:depot/dispatch`. The part after
//! `@` is the provider the gateway resolved, which the caller cannot know in advance, so neither the
//! grant nor the proof binds it.
//!
//! # One lock
//!
//! [`ExecutionAuthority::state`] (lock-order row 43): the gate, the grant verifier's retained epochs
//! and the revocation view, together. A leaf, held for a synchronous check, never across the
//! evaluator, the evidence write or the dispatch.

use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::action_evaluator::MandateBinding;
use crate::knowledge::issuer::{MemberKeySource, SharedExternalIssuers};
#[cfg(test)]
use crate::knowledge::issuer::TrustedExternalIssuers;
use crate::mandate::authority::{
    AuthorizedWork, CheckpointOffer, ExecutionDenial, ExecutionGate, RevocationView, SignedRevocationCheckpoint,
};
use crate::mandate::grant::{GrantVerdict, GrantVerifier, SignedMandateGrant};
use crate::mandate::MandateRefusal;

/// What a caller presents in `params._meta.mandate`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedMandate {
    /// The appointment, signed by its establishing authority.
    pub grant: SignedMandateGrant,
    /// Base64 of the holder's signature over
    /// `possession_message(grant, possession_request(operation, resource, arguments_digest))`.
    pub possession: String,
}

/// What a gateway presents in a caller envelope's **second mandate slot**, `cm` (identity-and-lifecycle I4): the
/// `call:` mandate it issued to itself for this one call — a cancellation handle, not authority a rule can require
/// (D-1) — its possession proof, and the mandate's **first revocation checkpoint**, carried beside it so a provider
/// that has never seen the checkpoint establishes the mandate without waiting for gossip. Assessed by
/// [`ExecutionAuthority::assess_call`], never by [`ExecutionAuthority::assess`].
///
/// Self-authenticating, as a [`PresentedMandate`] is: the mandate and the checkpoint are signed by the gateway's node
/// key, and the proof binds this call's operation, resource and RFC 8785 arguments digest. On the wire the
/// checkpoint's signature is base64.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedCallMandate {
    /// The `call:` mandate, signed by the gateway that holds it.
    pub grant: SignedMandateGrant,
    /// Base64 of the gateway's signature over
    /// `possession_message(grant, possession_request(operation, resource, arguments_digest))`.
    pub possession: String,
    /// The mandate's first checkpoint for its own scope. **Required**: a `call:` mandate without one cannot be
    /// established where its checkpoint has not arrived.
    #[serde(with = "carried_checkpoint")]
    pub checkpoint: SignedRevocationCheckpoint,
}

impl PresentedCallMandate {
    /// A `call:` mandate with its possession proof (raw signature bytes) and its first checkpoint.
    pub fn new(grant: SignedMandateGrant, possession: &[u8], checkpoint: SignedRevocationCheckpoint) -> Self {
        Self { grant, possession: base64::engine::general_purpose::STANDARD.encode(possession), checkpoint }
    }
}

/// A carried checkpoint's wire form: `{ "checkpoint": {…}, "signature": "<base64>" }` — about half the bytes of the
/// signature as the JSON array [`SignedRevocationCheckpoint`] derives.
mod carried_checkpoint {
    use super::*;
    use crate::mandate::authority::RevocationCheckpoint;

    #[derive(Serialize)]
    struct Out<'a> {
        checkpoint: &'a RevocationCheckpoint,
        signature: String,
    }
    #[derive(Deserialize)]
    struct In {
        checkpoint: RevocationCheckpoint,
        signature: String,
    }

    pub(super) fn serialize<S: serde::Serializer>(c: &SignedRevocationCheckpoint, s: S) -> Result<S::Ok, S::Error> {
        Out { checkpoint: &c.checkpoint, signature: base64::engine::general_purpose::STANDARD.encode(&c.signature) }.serialize(s)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SignedRevocationCheckpoint, D::Error> {
        let In { checkpoint, signature } = In::deserialize(d)?;
        let signature = base64::engine::general_purpose::STANDARD
            .decode(signature.as_bytes())
            .map_err(|_| serde::de::Error::custom("the carried checkpoint's signature is not base64"))?;
        Ok(SignedRevocationCheckpoint { checkpoint, signature })
    }
}

/// The resource as a grant names it: everything before the resolved provider's `@`.
pub fn resource_key(resource: &str) -> &str {
    resource.split('@').next().unwrap_or(resource)
}

/// The operation a grant must enumerate for this call: `{operation}:{resource_key}`.
pub fn mandate_operation(operation: &str, resource: &str) -> String {
    format!("{operation}:{}", resource_key(resource))
}

/// **The canonical digest of a call's arguments**, as the gateway and a provider compute it: SHA-256
/// of their `serde_json` encoding (keys in sorted order). A holder signs its possession proof over
/// `possession_request(operation, resource, &arguments_digest(&arguments))`. The SDKs export the same
/// function (`arguments_digest` / `argumentsDigest`) with shared golden vectors; this is the Rust one,
/// for a member acting under its own grant (closure plan C12 found it was crate-private).
///
/// **RFC 8785 (JCS)** since identity-and-lifecycle I3 (§9 O1) — the client signature's digest
/// ([`client_arguments_digest`](super::client_sig::client_arguments_digest)), which Rust, Python and TypeScript
/// reproduce byte for byte. It was `serde_json`'s own encoding, which writes `1e-5` where Python writes `1e-05` and
/// JavaScript `0.00001`, so a Python or TypeScript holder with such a float was refused. A grant issued before the
/// change binds that older form; for one release a verifier accepts either ([`legacy_arguments_digest`],
/// [`ExecutionAuthority::assess_arguments`]). An integer outside ±(2^53 − 1) is not bound exactly by this digest:
/// [`ExecutionAuthority::assess_arguments`] refuses it, as the grant door does.
pub fn arguments_digest(arguments: &serde_json::Value) -> [u8; 32] {
    super::client_sig::client_arguments_digest(arguments)
}

/// The arguments' digest **before** identity-and-lifecycle I3: SHA-256 of `serde_json`'s encoding (members by UTF-8
/// bytes, numbers as ryu writes them). Accepted beside [`arguments_digest`] for one release, so a grant issued and a
/// possession proof made under the older SDKs is honoured — each match counted
/// ([`ExecutionAuthority::legacy_digest_matches`]) and named on the record — and refused after it.
pub fn legacy_arguments_digest(arguments: &serde_json::Value) -> [u8; 32] {
    super::action_evaluator::arguments_digest(&serde_json::to_vec(arguments).unwrap_or_default())
}

/// Which form of the arguments' digest a presented mandate matched (identity-and-lifecycle I3, §9 O1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestForm {
    /// RFC 8785 (JCS): [`arguments_digest`].
    Jcs,
    /// The pre-I3 `serde_json` form: [`legacy_arguments_digest`], accepted for one release.
    Legacy,
}

/// Whether a recorded `arguments_digest` (hex) is one of these arguments' digest forms — a standing grant request
/// asked before identity-and-lifecycle I3 carries the legacy form, and is the same request for the window (#750's
/// review, item 4). Read by the request door's reuse and the agent's wait.
pub(crate) fn recorded_digest_matches(recorded: &serde_json::Value, arguments: &serde_json::Value) -> bool {
    digest_forms(arguments).iter().any(|(_, d)| recorded == hex32(d).as_str())
}

/// Where a legacy-digest match is counted (identity-and-lifecycle I3's window, `deprecations.md` §93): a gateway door,
/// this node as a provider, or this node's agent verifying the grant its own wait read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CountAt {
    Gateway,
    Provider,
    Agent,
}

/// The digests a verifier tries, JCS first and the legacy form only where it differs.
fn digest_forms(arguments: &serde_json::Value) -> Vec<(DigestForm, [u8; 32])> {
    let jcs = arguments_digest(arguments);
    let legacy = legacy_arguments_digest(arguments);
    if legacy == jcs { vec![(DigestForm::Jcs, jcs)] } else { vec![(DigestForm::Jcs, jcs), (DigestForm::Legacy, legacy)] }
}

/// The request bytes a possession proof binds to: this operation, this resource (before `@`), these
/// arguments. Tagged and length-prefixed. **Published format** — the SDKs compute it too, and a
/// golden vector pins all three.
pub fn possession_request(operation: &str, resource: &str, arguments_digest: &[u8; 32]) -> Vec<u8> {
    let mut out = b"mycelium.gateway/mandate-request/1".to_vec();
    for part in [operation.as_bytes(), resource_key(resource).as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    out.extend_from_slice(arguments_digest);
    out
}

use crate::mandate::authority::RevocationStanding;
use super::action_evaluator::hex32;

struct AuthorityState {
    gate: ExecutionGate,
    verifier: GrantVerifier,
    revocations: RevocationView,
    /// Identity-and-lifecycle I4: each `call:` scope this node admitted a call in, with the end of its mandate's
    /// window. A `call:` checkpoint is kept in `revocations` only for a scope held here, and until that end — not
    /// until the work ends — so a cancelled call presented again inside its window still reads revoked.
    call_windows: std::collections::HashMap<(crate::mandate::PrincipalId, String), u64>,
    /// The same windows ordered by their end, so a sweep reclaims only what has expired, at most
    /// [`RECLAIM_PER_SWEEP`] at a time, without scanning what it holds.
    call_expiry: std::collections::BTreeSet<(u64, crate::mandate::PrincipalId, String)>,
}

impl AuthorityState {
    /// Hold `scope` until `until`, or until the later end if it is held already: a second mandate on one scope with
    /// a later window moves the scope's expiry, so it is never reclaimed while that mandate can still be presented.
    fn keep_call_window(&mut self, authority: &crate::mandate::PrincipalId, scope: &str, until: u64) {
        let k = (authority.clone(), scope.to_string());
        match self.call_windows.get(&k).copied() {
            Some(held) if held >= until => {}
            Some(held) => {
                self.call_expiry.remove(&(held, k.0.clone(), k.1.clone()));
                self.call_expiry.insert((until, k.0.clone(), k.1.clone()));
                self.call_windows.insert(k, until);
            }
            None => {
                self.call_expiry.insert((until, k.0.clone(), k.1.clone()));
                self.call_windows.insert(k, until);
            }
        }
    }

    /// Forget up to [`RECLAIM_PER_SWEEP`] `call:` scopes whose window ended before `now_ms`: the window, the scope's
    /// newest checkpoint and its revoked set. Past its window a `call:` mandate is refused out of window whatever
    /// its checkpoint says, so nothing a forgotten scope held still decides anything.
    fn reclaim_call_windows(&mut self, now_ms: u64) -> usize {
        let mut n = 0;
        while n < RECLAIM_PER_SWEEP {
            let Some(first) = self.call_expiry.first().cloned() else { break };
            if first.0 >= now_ms {
                break;
            }
            self.call_expiry.remove(&first);
            self.revocations.forget(&first.1, &first.2);
            self.call_windows.remove(&(first.1, first.2));
            n += 1;
        }
        n
    }
}

/// The most `call:` scopes one [`ExecutionAuthority::sweep`] reclaims (identity-and-lifecycle I4), bounding how
/// long it holds lock-order row 43 for them; the rest wait for the next sweep.
pub const RECLAIM_PER_SWEEP: usize = 1_024;

/// A stop record with the mandate whose lapse stopped the work (identity-and-lifecycle I4; the maintainer's decision
/// of 2026-11-12, Q1), from [`ExecutionAuthority::named_stop_records`]. [`StopRecord`] itself is unchanged. A call
/// registered under a client's grant and its gateway's `call:` mandate has two registrations, and only a cancelled
/// one writes a record — so the record names which of the two stopped it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedStop {
    /// The scope of the mandate the work was registered under. `None`: it was registered under none.
    pub scope: Option<String>,
    /// That mandate's term.
    pub term: Option<String>,
    /// The three moments of the stop.
    pub record: crate::mandate::authority::StopRecord,
}

/// A gateway's execution authority: attached with `GossipAgent::with_execution_authority`.
pub struct ExecutionAuthority {
    state: Mutex<AuthorityState>,
    /// Shared (identity-and-lifecycle I1): the node's client-certificate fold feeds client holders' current keys here.
    external: SharedExternalIssuers,
    /// Closure plan C8: installed epochs that survive a restart, if attached.
    durable: std::sync::OnceLock<std::sync::Arc<crate::mandate::authority::DurableEpochs>>,
    /// Closure plan C10, lock-order row 48: work admitted under a mandate and still running. Leaf:
    /// never held while row 43 (`state`) is, and never across a handler.
    running: Mutex<std::collections::HashMap<u64, Running>>,
    next_work: std::sync::atomic::AtomicU64,
    /// Closure plan C10, lock-order row 49: stop records of cancelled work, newest last, bounded. Leaf.
    stops: Mutex<std::collections::VecDeque<NamedStop>>,
    /// Adopter-surface X3 (second PR), lock-order row 70: the grants this authority has admitted once, by scope, term
    /// and door — the single-use claim's atomic half (the marker under the request in the KV is its durable half).
    /// Leaf, µs.
    used_grants: Mutex<std::collections::HashMap<(String, String, String, String), u64>>,
    /// Mandates established on the pre-I3 digest form (identity-and-lifecycle I3, §9 O1), at this node's gateway doors
    /// and at it as a provider — two counters, so a call a node assesses at both is not one number counted twice.
    legacy_digest_matches: std::sync::atomic::AtomicU64,
    legacy_digest_matches_at_provider: std::sync::atomic::AtomicU64,
    legacy_digest_matches_at_agent: std::sync::atomic::AtomicU64,
    /// Identity-and-lifecycle I4: copies of a `call:` checkpoint read from the KV and not used — see
    /// [`call_checkpoints_refused`](Self::call_checkpoints_refused).
    call_checkpoints_refused: std::sync::atomic::AtomicU64,
}

/// The words `assess` gives a revoked grant's standing — read by the agent's wait, which treats this one unknown
/// standing as final (adopter-surface X3, second PR).
pub(crate) const GRANT_REVOKED: &str = "the grant is revoked";

/// The key a grant's checkpoints are written under by the issuing gateway: `grant/{holder segment}/{request}/checkpoint`.
pub(crate) fn grant_checkpoint_key(grant: &crate::mandate::Mandate) -> String {
    format!("grant/{}/{}/checkpoint", grant_holder_segment(grant.holder.as_str()), grant.term.as_str())
}

/// A grant holder as one `grant/` key segment (identity-and-lifecycle I1): a node principal (`node:{ip}:{port}`) is
/// written as it is, as every grant before I1 was; any other principal — a client's `token:{issuer}/{name}`, which
/// holds a `/` — as the lowercase hex of its bytes. The two never collide: hex has no `:`.
pub(crate) fn grant_holder_segment(holder: &str) -> String {
    if holder.starts_with("node:") {
        holder.to_string()
    } else {
        holder.bytes().map(|b| format!("{b:02x}")).collect()
    }
}

/// The holder a `grant/` key segment names ([`grant_holder_segment`]'s inverse): `None` for a segment that is
/// neither a node principal nor the canonical hex of a UTF-8 principal.
pub(crate) fn grant_holder_from_segment(segment: &str) -> Option<String> {
    if segment.starts_with("node:") {
        return (!segment.contains('/')).then(|| segment.to_string());
    }
    if segment.is_empty() || !segment.len().is_multiple_of(2) || !segment.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..segment.len()).step_by(2).map(|i| u8::from_str_radix(&segment[i..i + 2], 16).ok()).collect();
    let holder = String::from_utf8(bytes?).ok()?;
    // A node principal hex-encoded is not canonical: it is written literally.
    (!holder.starts_with("node:") && !holder.is_empty()).then_some(holder)
}

/// The holder a supervisor's door names in its `{agent}` path segment, as `(key segment, principal)`: the key
/// segment as [`grant_holder_segment`] writes it, **or the principal itself** — the request record's `agent`, which
/// the SDKs' `grant_decide` / `grant_revoke` put in the path (percent-encoded, so a client principal's `/` stays in
/// one segment; #743's review, item 2). A client principal holds a `:`, which no hex segment does, so the two
/// readings never meet. `None` for neither.
pub(crate) fn grant_holder_from_path(agent: &str) -> Option<(String, String)> {
    if agent.starts_with("node:") {
        return grant_holder_from_segment(agent).map(|p| (agent.to_string(), p));
    }
    if agent.contains(':') {
        let segment = grant_holder_segment(agent);
        return (grant_holder_from_segment(&segment).as_deref() == Some(agent)).then(|| (segment, agent.to_string()));
    }
    grant_holder_from_segment(agent).map(|p| (agent.to_string(), p))
}

/// **Feed one grant's checkpoint from the KV into `authority`** (adopter-surface X3, second PR): the issuing node
/// writes the grant's revocation checkpoint beside the grant — the first at issue (nothing revoked), the next to
/// revoke — and it travels by gossip; a verifier that has not seen it holds the grant `Unknown`. Offering is
/// monotonic (a lower sequence is ignored), so feeding before every assessment is safe and cheap. Nothing is fed
/// for a mandate that is not a grant.
pub(crate) fn feed_grant_checkpoint(ctx: &super::TaskCtx, authority: &ExecutionAuthority, grant: &crate::mandate::Mandate, now_ms: u64) {
    if !grant.is_grant() {
        return;
    }
    if let Some(bytes) = mycelium_core::ops::kv_get(ctx, &grant_checkpoint_key(grant))
        && let Ok(signed) = serde_json::from_slice::<crate::mandate::authority::SignedRevocationCheckpoint>(&bytes)
    {
        let _ = authority.offer_checkpoint(&signed, now_ms, &super::gateway_member_keys(ctx));
    }
}

/// Every grant checkpoint under `grant/` offered to `authority` — run when the prefix changes, so a grant issued or
/// revoked at another node is known to this node's verifier and to its sweep without a call to prompt it.
pub(crate) fn feed_grant_checkpoints(ctx: &super::TaskCtx, authority: &ExecutionAuthority) {
    let now_ms = ctx.hlc.decision_now_ms();
    let members = super::gateway_member_keys(ctx);
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, "grant/") {
        if key.ends_with("/checkpoint")
            && let Ok(signed) = serde_json::from_slice::<crate::mandate::authority::SignedRevocationCheckpoint>(&bytes)
            // A checkpoint the view holds already is not verified again (#678's review, finding 4).
            && !authority.holds_checkpoint(&signed.checkpoint.authority, &signed.checkpoint.scope, signed.checkpoint.seq)
        {
            let _ = authority.offer_checkpoint(&signed, now_ms, &members);
        }
    }
}

/// The owned key family a `call:` mandate's records live under (identity-and-lifecycle I4):
/// `call/{gateway segment}/{term}/…`, written by the issuing gateway.
pub(crate) const CALL_KEY_PREFIX: &str = "call/";

/// The key of a `call:` mandate's checkpoint — the first, then the revoking one:
/// `call/{gateway segment}/{term}/checkpoint`. `gateway` is the issuing gateway's principal (`node:{id}`), written
/// as [`grant_holder_segment`] writes a holder; `term` is the mandate's, which its scope `call:{term}` repeats.
pub(crate) fn call_checkpoint_key(gateway: &str, term: &str) -> String {
    format!("{CALL_KEY_PREFIX}{}/{term}/checkpoint", grant_holder_segment(gateway))
}

/// This node's own KV copy of a `call:` scope's newest checkpoint: what [`ExecutionAuthority::assess_call`] reads
/// under its hold (`read_local`), and what the feeder offers. One key read (papaya, no lock) and a parse.
///
/// **The KV is not trusted for it** (the rule of 2026-11-12: a security-bearing record in the gossip KV is signed by
/// its owner and verified at read). Any member can write this key, so a copy is used only when (a) its body names the
/// gateway and the scope its key path names — checked here — and (b) its signature verifies under that gateway's node
/// key — checked by the view's `offer`, which every caller passes it through. A copy that fails either, or does not
/// parse, is **treated as absent** and counted ([`ExecutionAuthority::call_checkpoints_refused`]). Absent is the safe
/// fallback the row states: the call stands on its carried checkpoint, and a revocation this node has already seen
/// stays seen, so a forged copy can neither cancel a call nor un-cancel one. What a member that overwrites or
/// tombstones the key *can* do is keep a cancel this node has not yet read from reaching it by this path, until the
/// gateway writes again or the mandate's window ends — stated, and bounded by the window.
pub(crate) fn local_call_checkpoint(
    ctx: &super::TaskCtx,
    refused: &ExecutionAuthority,
    authority: &crate::mandate::PrincipalId,
    scope: &str,
) -> Option<SignedRevocationCheckpoint> {
    let term = scope.strip_prefix(crate::mandate::grant::CALL_SCOPE_PREFIX)?;
    let bytes = mycelium_core::ops::kv_get(ctx, &call_checkpoint_key(authority.as_str(), term))?;
    match serde_json::from_slice::<SignedRevocationCheckpoint>(&bytes) {
        Ok(signed) if signed.checkpoint.authority == *authority && signed.checkpoint.scope == scope => Some(signed),
        _ => {
            refused.note_call_checkpoint_refused();
            None
        }
    }
}

/// **Offer `authority` the newest checkpoint of each `call:` scope it holds** (identity-and-lifecycle I4, exit 16) —
/// run when a key under `call/` changes. It reads **one key per held window** and never scans the key family, so its
/// cost follows the calls this node admitted, not the `call:` checkpoints the fleet's KV holds; a checkpoint the view
/// holds already is not verified again. The wake is a coalescing watch that does not carry the written key, which is
/// why every held window's key is read. Returns how many keys it read.
pub(crate) fn feed_call_checkpoints(ctx: &super::TaskCtx, authority: &ExecutionAuthority) -> usize {
    let held = authority.held_call_scopes();
    if held.is_empty() {
        return 0;
    }
    let now_ms = ctx.hlc.decision_now_ms();
    let members = super::gateway_member_keys(ctx);
    for (by, scope) in &held {
        if let Some(signed) = local_call_checkpoint(ctx, authority, by, scope)
            && !authority.holds_checkpoint(by, scope, signed.checkpoint.seq)
        {
            // `offer_checkpoint` keeps it only while the window is still held (row 43), so a scope reclaimed since
            // the snapshot above plants nothing. A copy whose signature is not the gateway's is not kept, and counted.
            if matches!(authority.offer_checkpoint(&signed, now_ms, &members), CheckpointOffer::Unverifiable(_)) {
                authority.note_call_checkpoint_refused();
            }
        }
    }
    held.len()
}

/// One piece of running work (closure plan C10).
struct Running {
    work: AuthorizedWork,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requested_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

/// How many stop records an authority keeps for measurement.
const STOP_RECORDS_KEPT: usize = 1024;

/// **Work running under a mandate** (Boundary H closure plan C10), from
/// [`ExecutionAuthority::begin`]. While it lives, the authority's sweep re-checks the work's mandate
/// (A1: epoch, scope, window, operation, revocation standing); the first failed check cancels it.
/// Dropping the guard ends the work: if it was cancelled, that drop is the **confirmed** stop and is
/// recorded for [`drain_report`](crate::mandate::authority::drain_report).
pub struct WorkGuard {
    authority: std::sync::Arc<ExecutionAuthority>,
    id: u64,
    cancel: std::sync::Arc<tokio::sync::Notify>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    requested_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    acknowledged_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    now_ms: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl std::fmt::Debug for WorkGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkGuard").field("id", &self.id).field("cancelled", &self.is_cancelled()).finish()
    }
}

impl WorkGuard {
    /// Has the authority cancelled this work?
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Resolves when the authority cancels this work (at once if it already has), and records that
    /// the work **acknowledged** the cancellation.
    pub async fn cancelled(&self) {
        let notified = self.cancel.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.is_cancelled() {
            notified.await;
        }
        let _ = self.acknowledged_ms.compare_exchange(
            0,
            (self.now_ms)().max(1),
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        );
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        let was = self.authority.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
        if self.is_cancelled() {
            let at = |v: &std::sync::atomic::AtomicU64| match v.load(std::sync::atomic::Ordering::Acquire) {
                0 => None,
                t => Some(t),
            };
            let now = (self.now_ms)();
            let mandate = was.and_then(|r| r.work.mandate);
            let record = NamedStop {
                scope: mandate.as_ref().map(|m| m.scope.clone()),
                term: mandate.as_ref().map(|m| m.term.as_str().to_string()),
                record: crate::mandate::authority::StopRecord {
                    requested_ms: at(&self.requested_ms),
                    acknowledged_ms: at(&self.acknowledged_ms).or(Some(now)),
                    confirmed_ms: Some(now),
                },
            };
            let mut stops = self.authority.stops.lock().unwrap_or_else(|e| e.into_inner());
            if stops.len() == STOP_RECORDS_KEPT {
                stops.pop_front();
            }
            stops.push_back(record);
        }
    }
}

impl std::fmt::Debug for ExecutionAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionAuthority").finish_non_exhaustive()
    }
}

/// The finding for one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    /// What goes into the envelope's `mandate`.
    pub binding: Option<MandateBinding>,
    /// The mandate's `valid_until_ms`, to clamp the envelope's window to.
    pub valid_until_ms: Option<u64>,
}

impl ExecutionAuthority {
    /// An authority over `gate` (A1) and `verifier` (P2), trusting `external` for authorities and
    /// holders on the configured-external path.
    ///
    /// `external` is a [`TrustedExternalIssuers`] or a [`SharedExternalIssuers`]; attached to a node, the authority's set
    /// is fed each client principal's current key (identity-and-lifecycle I1), so a grant held by a client verifies.
    pub fn new(gate: ExecutionGate, verifier: GrantVerifier, external: impl Into<SharedExternalIssuers>) -> Self {
        let external = external.into();
        Self {
            state: Mutex::new(AuthorityState {
                gate,
                verifier,
                revocations: RevocationView::new(),
                call_windows: std::collections::HashMap::new(),
                call_expiry: std::collections::BTreeSet::new(),
            }),
            external,
            durable: std::sync::OnceLock::new(),
            running: Mutex::new(std::collections::HashMap::new()),
            next_work: std::sync::atomic::AtomicU64::new(1),
            stops: Mutex::new(std::collections::VecDeque::new()),
            used_grants: Mutex::new(std::collections::HashMap::new()),
            legacy_digest_matches: std::sync::atomic::AtomicU64::new(0),
            legacy_digest_matches_at_provider: std::sync::atomic::AtomicU64::new(0),
            legacy_digest_matches_at_agent: std::sync::atomic::AtomicU64::new(0),
            call_checkpoints_refused: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// **Claim one use of a grant at `door`** (adopter-surface X3, second PR): `true` the first time this authority
    /// sees the grant — its holder, scope and term — at that door, `false` afterwards. A grant admits one call at
    /// each enforcement point; the gateway and the provider each claim their own use of the same call, and a second
    /// presentation at either is refused `GrantUsed`. The in-memory map is the atomic half (two concurrent
    /// presentations meet here) and is pruned by the sweep once `valid_until_ms` has passed; the door writes a marker
    /// under the request so a restart — or the pruning — does not forget the use.
    pub fn claim_grant_use(&self, holder: &str, scope: &str, term: &str, door: &str, valid_until_ms: u64) -> bool {
        let mut used = self.used_grants.lock().unwrap_or_else(|e| e.into_inner());
        let key = (holder.to_string(), scope.to_string(), term.to_string(), door.to_string());
        if used.contains_key(&key) {
            return false;
        }
        used.insert(key, valid_until_ms);
        true
    }

    /// Release a claim made by [`claim_grant_use`](Self::claim_grant_use) whose work never ran because the use could
    /// not be recorded durably (external review 2026-10-31, R3): nothing ran, so the use is not spent, and a retry
    /// after the disk is repaired runs once. A restart gives the same answer, since no marker was written.
    pub fn release_grant_use(&self, holder: &str, scope: &str, term: &str, door: &str) {
        let mut used = self.used_grants.lock().unwrap_or_else(|e| e.into_inner());
        used.remove(&(holder.to_string(), scope.to_string(), term.to_string(), door.to_string()));
    }

    /// Does this authority already hold a checkpoint of `authority` for `scope` at or past `seq`?
    pub fn holds_checkpoint(&self, authority: &crate::mandate::PrincipalId, scope: &str, seq: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).revocations.holds(authority, scope, seq)
    }

    /// **Admit running work under this authority** (closure plan C10). Until the returned guard is
    /// dropped, [`sweep`](Self::sweep) re-checks `work`'s mandate, and cancels it at the first check
    /// that fails. `now_ms` is the caller's clock, read when the guard records a stop.
    pub fn begin(
        self: &std::sync::Arc<Self>,
        work: AuthorizedWork,
        now_ms: std::sync::Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> WorkGuard {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        let id = self.next_work.fetch_add(1, Ordering::Relaxed);
        let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let requested_ms = std::sync::Arc::new(AtomicU64::new(0));
        let acknowledged_ms = std::sync::Arc::new(AtomicU64::new(0));
        self.running.lock().unwrap_or_else(|e| e.into_inner()).insert(
            id,
            Running {
                work,
                cancel: std::sync::Arc::clone(&cancel),
                cancelled: std::sync::Arc::clone(&cancelled),
                requested_ms: std::sync::Arc::clone(&requested_ms),
            },
        );
        WorkGuard { authority: std::sync::Arc::clone(self), id, cancel, cancelled, requested_ms, acknowledged_ms, now_ms }
    }

    /// **Re-check every piece of running work now** (closure plan C10): the same A1 check as at
    /// admission, so expiry, revocation, a stale revocation view and a superseded epoch all stop work
    /// that is already running. Returns how many were cancelled by this sweep. The agent runs it
    /// every [`sweep_interval_ms`](Self::sweep_interval_ms).
    pub fn sweep(&self, now_ms: u64) -> usize {
        use std::sync::atomic::Ordering;
        // The single-use claims of grants whose window has passed (the marker on the WAL keeps the durable half).
        self.used_grants.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, until| *until > now_ms);
        // Snapshot under row 48, check under row 43, cancel under row 48: never both at once.
        let snapshot: Vec<(u64, AuthorizedWork)> = self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|(_, r)| !r.cancelled.load(Ordering::Acquire))
            .map(|(id, r)| (*id, r.work.clone()))
            .collect();
        let denied: Vec<u64> = {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            // Identity-and-lifecycle I4: the `call:` scopes whose window has passed are forgotten under the same hold,
            // at most `RECLAIM_PER_SWEEP` of them. Work under one is past its `not_after_ms` and is denied below
            // by its window, whatever the view still holds.
            st.reclaim_call_windows(now_ms);
            let st = &*st;
            snapshot
                .into_iter()
                .filter(|(_, w)| match &w.mandate {
                    // Adopter-surface X3 (second PR): work under a **grant** is judged as `assess` judges a grant — its
                    // own window and its issuer's checkpoint for the grant's scope — not by the resource gate, whose
                    // scope and epoch rules a grant's own scope would fail at the first sweep (a defect of the first PR,
                    // unobserved because no granted handler ran longer than one sweep interval).
                    // A `call:` mandate's work the same way (identity-and-lifecycle I4): a cancel is its term revoked.
                    Some(m) if m.is_grant() || m.is_call() => {
                        now_ms > w.not_after_ms
                            || matches!(st.revocations.grant_standing(&m.established_by, &m.scope, &m.term), RevocationStanding::Revoked)
                    }
                    _ => st.gate.check(w, &st.revocations, now_ms).is_err(),
                })
                .map(|(id, _)| id)
                .collect()
        };
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let mut n = 0;
        for id in denied {
            if let Some(r) = running.get(&id)
                && !r.cancelled.swap(true, Ordering::AcqRel)
            {
                r.requested_ms.store(now_ms.max(1), Ordering::Release);
                r.cancel.notify_waiters();
                n += 1;
            }
        }
        n
    }

    /// How often the agent sweeps running work: a twentieth of the freshness window *F*, within
    /// 100 ms and 5 s. The drain bound for a cancellable handler is this, plus *s*, plus the
    /// handler's own time to stop.
    pub fn sweep_interval_ms(&self) -> u64 {
        (self.state.lock().unwrap_or_else(|e| e.into_inner()).gate.freshness().freshness_ms / 20).clamp(100, 5_000)
    }

    /// Stop records of cancelled work (the newest `1024`), for measuring T_drain with
    /// [`drain_report`](crate::mandate::authority::drain_report).
    pub fn stop_records(&self) -> Vec<crate::mandate::authority::StopRecord> {
        self.stops.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|s| s.record).collect()
    }

    /// [`stop_records`](Self::stop_records), each with the scope and term of the mandate the stopped work was
    /// registered under (identity-and-lifecycle I4): which mandate's lapse stopped it.
    pub fn named_stop_records(&self) -> Vec<NamedStop> {
        self.stops.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }

    /// Work currently running under this authority.
    pub fn running_count(&self) -> usize {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// **Record when this reader started** (closure plan C8). From here on, only revocation
    /// checkpoints issued since `now_ms` (less 2*s*) may refresh freshness, so an old checkpoint
    /// replayed after a restart cannot make a revoked appointment read as current.
    /// `GossipAgent::with_execution_authority` calls it at attach time. It resets the revocation view,
    /// which at attach time holds nothing anyway.
    pub fn mark_started(&self, now_ms: u64) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).revocations = RevocationView::started_at(now_ms);
    }

    /// **Keep installed epochs across restarts** (closure plan C8). The journal's floor for this
    /// gate's scope is installed now, if it is higher than what is configured; later epochs go through
    /// [`install_epoch_durably`](Self::install_epoch_durably). Set once.
    /// Whether installed epochs are journalled before they take effect (`with_durable_epochs`).
    pub fn has_durable_epochs(&self) -> bool {
        self.durable.get().is_some()
    }

    pub fn with_durable_epochs(&self, durable: std::sync::Arc<crate::mandate::authority::DurableEpochs>) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(floor) = durable.floor(st.gate.scope()) {
            st.gate.install_epoch(floor);
        }
        drop(st);
        let _ = self.durable.set(durable);
    }

    /// Install a later epoch, **journalled first** when durable epochs are attached, so a restart
    /// cannot fall back below it. Without them, the same as [`install_epoch`](Self::install_epoch).
    pub async fn install_epoch_durably(&self, epoch: u64) -> Result<bool, crate::agent::journal::JournalError> {
        if let Some(d) = self.durable.get() {
            let scope = self.state.lock().unwrap_or_else(|e| e.into_inner()).gate.scope().to_string();
            d.record(&scope, epoch).await?;
        }
        Ok(self.install_epoch(epoch))
    }

    /// The authority's external issuers — the handle the node's client-certificate fold feeds (identity-and-lifecycle
    /// I1). A clone is the same set.
    pub fn external_issuers(&self) -> SharedExternalIssuers {
        self.external.clone()
    }

    /// Offer a revocation checkpoint, at `now_ms`.
    pub fn offer_checkpoint(
        &self,
        signed: &SignedRevocationCheckpoint,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> CheckpointOffer {
        // A snapshot, taken before the state lock: the shared set's lock is never held with another (lock-order row 77).
        let external = self.external.read().clone();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        // Identity-and-lifecycle I4: a `call:` checkpoint is kept only for a scope this node admitted a call in and
        // still holds the window of — checked under the same hold as the offer, so nothing is planted for a scope the
        // sweep has reclaimed or another node's call.
        let c = &signed.checkpoint;
        if c.scope.starts_with(crate::mandate::grant::CALL_SCOPE_PREFIX) && !st.call_windows.contains_key(&(c.authority.clone(), c.scope.clone())) {
            return CheckpointOffer::NotHeld;
        }
        let (policy, clock) = (st.gate.freshness(), st.gate.clock());
        st.revocations.offer(signed, &policy, clock, now_ms, members, &external)
    }

    /// **Copies of a `call:` checkpoint read from this node's KV and not used** (identity-and-lifecycle I4), since
    /// this authority was built: one that did not parse, named another gateway or scope than its key, or was not
    /// signed by that gateway's node key. Each was treated as absent. Any fleet member can write the key (Layer I
    /// detects, it does not prevent), so a rising count is a member writing under another gateway's `call/` keys, or
    /// a gateway whose key this node does not hold. Counted per read: the feeder reads a held window's key on every
    /// wake, so one bad copy counts until it is overwritten. `GET /stats` `call_checkpoints_refused`;
    /// `mycelium_call_checkpoints_refused_total`.
    pub fn call_checkpoints_refused(&self) -> u64 {
        self.call_checkpoints_refused.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn note_call_checkpoint_refused(&self) {
        self.call_checkpoints_refused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_call_checkpoints_refused_total").increment(1);
    }

    /// The `call:` scopes this authority holds a window for (identity-and-lifecycle I4), as `(issuing gateway,
    /// scope)`: the calls this node admitted whose mandate's window has not been reclaimed.
    pub(crate) fn held_call_scopes(&self) -> Vec<(crate::mandate::PrincipalId, String)> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).call_windows.keys().cloned().collect()
    }

    /// What the revocation view holds: `(newest checkpoints, revoked sets, call: windows)`. Each admitted `call:`
    /// scope counts one of each until its window is reclaimed (identity-and-lifecycle I4, exit 9).
    pub fn view_sizes(&self) -> (usize, usize, usize) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (newest, revoked) = st.revocations.sizes();
        (newest, revoked, st.call_windows.len())
    }

    /// **Assess a call's `call:` mandate** (identity-and-lifecycle I4) — the envelope's second slot, `cm`. `gateway`
    /// is the principal of the gateway whose envelope carried it, `node:{via}`, which the caller established from
    /// the frame's verified sender. `read_local` returns this node's own KV copy of the scope's newest checkpoint
    /// (`local_call_checkpoint`); it is called **inside** the hold, see below, so it must take no lock.
    ///
    /// Lock-order row 43 is taken **once**, and under that one hold, in this order:
    ///
    /// 1. the mandate is verified: the scope is `call:{term}`; its `established_by` and `holder` are both `gateway`
    ///    (D-6 — no entitlement table is read); its signature; its window, no longer than `GRANT_TTL_CAP_SECS`; the
    ///    possession proof over this call's operation, resource and **RFC 8785** arguments digest (the kind is new,
    ///    so no pre-I3 form is accepted); the operation it enumerates;
    /// 2. the scope's **window is inserted**;
    /// 3. `read_local` is called;
    /// 4. the carried checkpoint and the local copy are offered, each only if it is for this mandate's own
    ///    authority and scope;
    /// 5. the term is judged: `Established`, [`MandateRefusal::Revoked`], or `Unknown`.
    ///
    /// The order closes a race (N2): a revoking checkpoint applied to the KV before step 3 is read there; one
    /// applied after it wakes the feeder, which offers it only for a held window and takes row 43 to ask — so it
    /// waits for this hold and then finds the window. A mandate that does not verify plants nothing.
    ///
    /// Nothing here is retained across a restart (D-4): the next admission's `read_local` re-reads the KV.
    #[allow(clippy::too_many_arguments)]
    pub fn assess_call(
        &self,
        gateway: &str,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        presented: &PresentedCallMandate,
        now_ms: u64,
        members: &impl MemberKeySource,
        read_local: impl FnOnce(&crate::mandate::PrincipalId, &str) -> Option<SignedRevocationCheckpoint>,
    ) -> Assessment {
        let m = &presented.grant.mandate;
        let digest_hex = hex32(&m.digest());
        let unknown = |why: String| Assessment {
            binding: Some(
                MandateBinding::unknown(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, why)
                    .with_facts(Some(digest_hex.clone()), None),
            ),
            valid_until_ms: Some(m.valid_until_ms),
        };
        let refused = |r: MandateRefusal| Assessment {
            binding: Some(
                MandateBinding::refused(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, r)
                    .with_facts(Some(digest_hex.clone()), None),
            ),
            valid_until_ms: Some(m.valid_until_ms),
        };
        if !m.is_call() {
            return unknown("the second mandate slot holds a mandate that is not a call: mandate".into());
        }
        let Ok(proof) = base64::engine::general_purpose::STANDARD.decode(presented.possession.as_bytes()) else {
            return unknown("the possession proof is not base64".into());
        };
        if super::client_sig::holds_unsafe_integer(arguments) {
            return refused(MandateRefusal::UnsafeInteger);
        }
        let carried = &presented.checkpoint;
        if carried.checkpoint.authority != m.established_by || carried.checkpoint.scope != m.scope {
            return unknown("the carried checkpoint is not for this mandate's authority and scope".into());
        }
        let request = possession_request(operation, resource, &arguments_digest(arguments));
        // A snapshot, taken before the state lock: the shared set's lock is never held with another (lock-order row 77).
        let external = self.external.read().clone();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        // 1. The mandate verifies, or nothing is planted.
        match crate::mandate::grant::check_call_mandate(&presented.grant, gateway, &request, &proof, now_ms, members, &external) {
            GrantVerdict::Valid => {}
            GrantVerdict::OutOfWindow => return refused(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: m.valid_until_ms }),
            GrantVerdict::NotEntitled => {
                return unknown(format!(
                    "a call: mandate counts only when its scope is call:{{term}} and its issuer and holder are the gateway that sent it ({gateway}); this one is issued by {} and held by {}",
                    m.established_by.as_str(),
                    m.holder.as_str()
                ));
            }
            other => return unknown(format!("call mandate not established: {other:?}")),
        }
        if !m.permits(&mandate_operation(operation, resource)) {
            return refused(MandateRefusal::NotEnumerated { operation: mandate_operation(operation, resource) });
        }
        // 2. The window, before the read: from here the feeder offers this scope's checkpoints.
        st.keep_call_window(&m.established_by, &m.scope, m.valid_until_ms);
        #[cfg(test)]
        call_hooks::fire(self, call_hooks::Stage::BeforeLocalRead);
        // 3. This node's own copy, read under the hold.
        let local = read_local(&m.established_by, &m.scope);
        #[cfg(test)]
        call_hooks::fire(self, call_hooks::Stage::AfterLocalRead);
        // 4. Both offered — only for this mandate's own authority and scope, so neither speaks for anything else.
        let (policy, clock) = (st.gate.freshness(), st.gate.clock());
        for (from_kv, cp) in [(false, Some(carried)), (true, local.as_ref())] {
            let Some(cp) = cp else { continue };
            let c = &cp.checkpoint;
            if c.authority != m.established_by || c.scope != m.scope {
                if from_kv {
                    self.note_call_checkpoint_refused();
                }
                continue;
            }
            if !st.revocations.holds(&c.authority, &c.scope, c.seq)
                && matches!(st.revocations.offer(cp, &policy, clock, now_ms, members, &external), CheckpointOffer::Unverifiable(_))
                && from_kv
            {
                // The KV's copy is not the gateway's: treated as absent.
                self.note_call_checkpoint_refused();
            }
        }
        // 5. Judged as a grant's term is: no freshness, the window bounds its life. A revoked term is refused by name.
        match st.revocations.grant_standing(&m.established_by, &m.scope, &m.term) {
            RevocationStanding::NotRevoked => Assessment {
                binding: Some(
                    MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)
                        .with_facts(Some(digest_hex.clone()), None),
                ),
                valid_until_ms: Some(m.valid_until_ms),
            },
            RevocationStanding::Revoked => refused(MandateRefusal::Revoked { term: m.term.as_str().to_string() }),
            _ => unknown("the call's checkpoint did not verify at this node".into()),
        }
    }

    /// Install a later epoch at the protected resource. Never goes backwards.
    pub fn install_epoch(&self, epoch: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).gate.install_epoch(epoch)
    }

    /// **Assess one call.** `presented` is `params._meta.mandate`, if any; `caller` is the principal
    /// the auth layer resolved. `arguments_digest` is the one form the caller computed; a door that holds the
    /// arguments calls [`assess_arguments`](Self::assess_arguments), which also accepts a grant issued before
    /// identity-and-lifecycle I3 and refuses an integer no digest binds.
    #[allow(clippy::too_many_arguments)]
    pub fn assess(
        &self,
        caller: &str,
        operation: &str,
        resource: &str,
        arguments_digest: &[u8; 32],
        presented: Option<&PresentedMandate>,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> Assessment {
        self.assess_digests(caller, operation, resource, &[(DigestForm::Jcs, *arguments_digest)], false, presented, now_ms, members).0
    }

    /// **Assess one call from its arguments** (identity-and-lifecycle I3, §9 O1): the binding and the possession proof
    /// are tried against the RFC 8785 digest ([`arguments_digest`]) and, for one release, the pre-I3 `serde_json` digest
    /// ([`legacy_arguments_digest`]) — the form that matched is returned, and a legacy match counted as a gateway door's
    /// ([`legacy_digest_matches`](Self::legacy_digest_matches)). Arguments holding an integer outside ±(2^53 − 1) are
    /// refused by name ([`MandateRefusal::UnsafeInteger`]) when a mandate is presented, since no digest binds them
    /// exactly.
    #[allow(clippy::too_many_arguments)]
    pub fn assess_arguments(
        &self,
        caller: &str,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        presented: Option<&PresentedMandate>,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> (Assessment, Option<DigestForm>) {
        self.assess_arguments_at(CountAt::Gateway, caller, operation, resource, arguments, presented, now_ms, members)
    }

    /// [`assess_arguments`](Self::assess_arguments) at a point, which counts a legacy match as its own.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn assess_arguments_at(
        &self,
        at: CountAt,
        caller: &str,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        presented: Option<&PresentedMandate>,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> (Assessment, Option<DigestForm>) {
        let unsafe_integer = super::client_sig::holds_unsafe_integer(arguments);
        let (assessment, form) = self.assess_digests(caller, operation, resource, &digest_forms(arguments), unsafe_integer, presented, now_ms, members);
        if form == Some(DigestForm::Legacy) {
            let (counter, point) = match at {
                CountAt::Gateway => (&self.legacy_digest_matches, "gateway"),
                CountAt::Provider => (&self.legacy_digest_matches_at_provider, "provider"),
                CountAt::Agent => (&self.legacy_digest_matches_at_agent, "agent"),
            };
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            #[cfg(feature = "metrics")]
            metrics::counter!("mycelium_legacy_digest_matches_total", "point" => point).increment(1);
            #[cfg(not(feature = "metrics"))]
            let _ = point;
            let mut assessment = assessment;
            assessment.binding = assessment.binding.map(MandateBinding::with_legacy_digest);
            return (assessment, form);
        }
        (assessment, form)
    }

    /// [`assess_arguments`](Self::assess_arguments) without counting a legacy match: the provider's re-assessment of a
    /// call its own preflight already assessed (and counted).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reassess_arguments(
        &self,
        caller: &str,
        operation: &str,
        resource: &str,
        arguments: &serde_json::Value,
        presented: Option<&PresentedMandate>,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> Assessment {
        let unsafe_integer = super::client_sig::holds_unsafe_integer(arguments);
        self.assess_digests(caller, operation, resource, &digest_forms(arguments), unsafe_integer, presented, now_ms, members).0
    }

    /// Mandates established on the pre-I3 digest form ([`legacy_arguments_digest`]) at this node's gateway doors since
    /// this authority was built — what an operator watches fall to zero before the release that refuses that form.
    /// In memory: it starts at zero when the node restarts. The durable record of each match is its evidence record's
    /// `checked` field ([`LEGACY_DIGEST_CHECKED`](super::action_evaluator::LEGACY_DIGEST_CHECKED)). `GET /stats`
    /// `legacy_digest_matches`; `mycelium_legacy_digest_matches_total{point="gateway"}`.
    pub fn legacy_digest_matches(&self) -> u64 {
        self.legacy_digest_matches.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// [`legacy_digest_matches`](Self::legacy_digest_matches) at this node as a provider (`with_provider_enforcement`):
    /// `GET /stats` `provider_legacy_digest_matches`; `mycelium_legacy_digest_matches_total{point="provider"}`.
    pub fn legacy_digest_matches_at_provider(&self) -> u64 {
        self.legacy_digest_matches_at_provider.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// [`legacy_digest_matches`](Self::legacy_digest_matches) at this node's agent, verifying a grant its own wait read
    /// (`AgentStateMachine::transition_invoking`) — counted there because a provider without `with_provider_enforcement`
    /// never assesses the call, and the agent's wait writes no evidence record: `GET /stats`
    /// `agent_legacy_digest_matches`; `mycelium_legacy_digest_matches_total{point="agent"}`.
    pub fn legacy_digest_matches_at_agent(&self) -> u64 {
        self.legacy_digest_matches_at_agent.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[allow(clippy::too_many_arguments)]
    fn assess_digests(
        &self,
        caller: &str,
        operation: &str,
        resource: &str,
        digests: &[(DigestForm, [u8; 32])],
        unsafe_integer: bool,
        presented: Option<&PresentedMandate>,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> (Assessment, Option<DigestForm>) {
        let Some(p) = presented else { return (Assessment { binding: None, valid_until_ms: None }, None) };
        let m = &p.grant.mandate;
        let digest_hex = hex32(&m.digest());
        // An unknown standing names the mandate it could not establish, as a refusal does (adopter-surface X3, second
        // PR): the record of a revoked grant's call then carries the grant's hash, not only the rule it failed.
        let unknown = |why: String| Assessment {
            binding: Some(
                MandateBinding::unknown(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, why)
                    .with_facts(Some(digest_hex.clone()), m.grant_binding().and_then(|b| b.policy_revision)),
            ),
            valid_until_ms: Some(m.valid_until_ms),
        };
        let refused = |r: MandateRefusal| Assessment {
            binding: Some(
                MandateBinding::refused(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, r)
                    .with_facts(Some(digest_hex.clone()), m.grant_binding().and_then(|b| b.policy_revision)),
            ),
            valid_until_ms: Some(m.valid_until_ms),
        };

        // Identity-and-lifecycle I4 (D-1, N3): a `call:` mandate is a gateway's cancellation handle, assessed only in
        // the envelope's second slot (`assess_call`). In this one it establishes nothing, whoever presents it — a
        // client replaying a gateway's `call:` mandate as its own gains nothing, and no rule's `requires_mandate`
        // is met by one.
        if m.is_call() {
            return (unknown("a call: mandate is not a caller's".into()), None);
        }
        if m.holder.as_str() != caller {
            return (unknown(format!("the grant's holder {} is not the caller {caller}", m.holder.as_str())), None);
        }
        let Ok(proof) = base64::engine::general_purpose::STANDARD.decode(p.possession.as_bytes()) else {
            return (unknown("the possession proof is not base64".into()), None);
        };
        // Identity-and-lifecycle I3 (§9 O1): no digest binds an integer past ±(2^53 − 1) exactly.
        if unsafe_integer {
            return (refused(MandateRefusal::UnsafeInteger), None);
        }
        // Adopter-surface X3: a grant's arguments binding is checked first, so a grant presented for other arguments
        // is refused **by name** rather than as a possession mismatch (the proof covers this call's digest, which the
        // bound one is not). Refusing on an as-yet-unverified binding is fail-closed: nothing is granted by it. In I3's
        // window the binding may name either form, and the proof may be over either, independently: each digest is
        // computed here from this call's own arguments, so either pairing binds exactly these arguments — and an
        // un-upgraded binder (proving over the legacy form) can present a grant an upgraded door asked for (bound to
        // RFC 8785), and an upgraded one a grant from a request asked for before the upgrade (#750's review, item 4).
        let grant_binding = m.grant_binding();
        let bound_form = match &grant_binding {
            Some(b) => match digests.iter().find(|(_, d)| b.arguments_digest == hex32(d)) {
                Some((form, _)) => Some(*form),
                None => {
                    let (bound, presented) = (b.arguments_digest.clone(), hex32(&digests[0].1));
                    return (refused(MandateRefusal::BoundToOtherArguments { bound, presented }), None);
                }
            },
            None => None,
        };

        let external = self.external.read().clone();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        // The possession proof over each digest form in turn: the first that verifies. The form matched is the legacy
        // one if either the binding or the proof used it.
        let mut matched: Option<DigestForm> = None;
        let mut first: Option<GrantVerdict> = None;
        for (form, digest) in digests {
            match st.verifier.check(&p.grant, &possession_request(operation, resource, digest), Some(&proof), now_ms, members, &external) {
                GrantVerdict::Valid => {
                    matched = Some(if bound_form == Some(DigestForm::Legacy) { DigestForm::Legacy } else { *form });
                    break;
                }
                other => {
                    first.get_or_insert(other);
                }
            }
        }
        if matched.is_none() {
            return match first {
                Some(GrantVerdict::Superseded { held }) => (refused(MandateRefusal::Superseded { installed: held, presented: m.epoch }), None),
                Some(GrantVerdict::OutOfWindow) => (refused(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: m.valid_until_ms }), None),
                other => (unknown(format!("grant not established: {:?}", other)), None),
            };
        }

        // Adopter-surface X3: a grant is one appointment for one call. Its scope is its own, so the resource gate's
        // epoch rule does not apply to it; what does: the binding its issuer signed (the arguments digest, the policy
        // revision — the latter checked by the preflight once the decision's revision is known), the window, and the
        // revocation standing of its term under the gate's freshness rule.
        // The rest decides on the matched form's proof; the form is reported only for an established mandate.
        let assessment = (|| -> Assessment {
            if let Some(binding) = grant_binding {
                if !m.permits(&mandate_operation(operation, resource)) {
                    return refused(MandateRefusal::NotEnumerated { operation: mandate_operation(operation, resource) });
                }
                // A grant's standing is its issuer's checkpoint for the grant's scope, not the gate's freshness: the
                // grant's own window bounds its life (#673's review, finding 1).
                return match st.revocations.grant_standing(&m.established_by, &m.scope, &m.term) {
                    RevocationStanding::NotRevoked => Assessment {
                        binding: Some(
                            MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)
                                .with_facts(Some(digest_hex.clone()), binding.policy_revision),
                        ),
                        valid_until_ms: Some(m.valid_until_ms),
                    },
                    RevocationStanding::Revoked => unknown(GRANT_REVOKED.into()),
                    RevocationStanding::Unknown => unknown("the grant's issuer's checkpoint has not reached this verifier".into()),
                };
            }

            let work = AuthorizedWork::new(mandate_operation(operation, resource), Some(m.clone()), u64::MAX);
            match st.gate.check(&work, &st.revocations, now_ms) {
                Ok(()) => Assessment {
                    binding: Some(
                        MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)
                            .with_facts(Some(digest_hex.clone()), None),
                    ),
                    valid_until_ms: Some(m.valid_until_ms),
                },
                Err(ExecutionDenial::Refused(r)) => refused(r),
                Err(ExecutionDenial::WorkExpired) => {
                    refused(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: m.valid_until_ms })
                }
                Err(ExecutionDenial::Revoked) => unknown("the appointment is revoked".into()),
                Err(ExecutionDenial::RevocationUnknown) => {
                    unknown("revocation standing unknown: no fresh checkpoint".into())
                }
                Err(ExecutionDenial::NoMandate) => Assessment { binding: None, valid_until_ms: None },
            }
        })();
        let established = assessment.binding.as_ref().is_some_and(|b| b.state == super::action_evaluator::MandateState::Established);
        (assessment, if established { matched } else { None })
    }
}

/// Test hooks inside [`ExecutionAuthority::assess_call`]'s hold (identity-and-lifecycle I4, N2), so a test can land
/// a revoking checkpoint between the window's insert and the local read, or just after the read.
#[cfg(test)]
pub(crate) mod call_hooks {
    use std::sync::Arc;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Stage {
        /// The window is inserted; the local copy is not yet read.
        BeforeLocalRead,
        /// The local copy has been read; nothing is offered yet.
        AfterLocalRead,
    }

    pub(crate) type Hook = Arc<dyn Fn(Stage) + Send + Sync>;

    /// Per authority (by address), until removed.
    pub(crate) static HOOKS: std::sync::LazyLock<papaya::HashMap<usize, Hook>> = std::sync::LazyLock::new(papaya::HashMap::new);

    pub(crate) fn key(authority: &super::ExecutionAuthority) -> usize {
        authority as *const super::ExecutionAuthority as usize
    }

    pub(super) fn fire(authority: &super::ExecutionAuthority, stage: Stage) {
        let hook = HOOKS.pin().get(&key(authority)).cloned();
        if let Some(hook) = hook {
            hook(stage);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::knowledge::issuer::MemberKeys;
    use crate::knowledge::IssuerId;
    use crate::mandate::authority::{
        ClockModel, FreshnessPolicy, ResourceTier, RevocationCheckpoint,
    };
    use crate::mandate::grant::{possession_message, EntitlementTable};
    use crate::agent::action_evaluator::MandateState;
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;

    pub(crate) const CALLER: &str = "token:gw/agent-1";
    pub(crate) const OP: &str = "skill.invoke";
    pub(crate) const RES: &str = "skill:depot/dispatch@127.0.0.1:9";

    pub(crate) fn keys() -> (SigningKey, SigningKey) {
        (SigningKey::from_bytes(&[41u8; 32]), SigningKey::from_bytes(&[42u8; 32]))
    }

    pub(crate) fn external(authority: &SigningKey, holder: &SigningKey) -> TrustedExternalIssuers {
        let mut e = TrustedExternalIssuers::new();
        e.trust(IssuerId::new("operator:acme").unwrap(), authority.verifying_key().to_bytes()).unwrap();
        e.trust(IssuerId::new(CALLER).unwrap(), holder.verifying_key().to_bytes()).unwrap();
        e
    }

    pub(crate) fn authority(authority_key: &SigningKey, holder: &SigningKey) -> ExecutionAuthority {
        let mut t = EntitlementTable::new();
        t.entitle("depot", PrincipalId::new("operator:acme").unwrap());
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("depot", 1),
            ResourceTier::Serialised,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        )
        .unwrap();
        ExecutionAuthority::new(gate, GrantVerifier::new(t), external(authority_key, holder))
    }

    pub(crate) fn mandate(epoch: u64, until: u64) -> Mandate {
        Mandate {
            holder: PrincipalId::new(CALLER).unwrap(),
            established_by: PrincipalId::new("operator:acme").unwrap(),
            purpose: "dispatch".into(),
            scope: "depot".into(),
            operations: vec![mandate_operation(OP, RES)],
            epoch,
            term: TermId::new("t1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: until,
        }
    }

    pub(crate) fn present(authority: &SigningKey, holder: &SigningKey, m: Mandate, digest: &[u8; 32]) -> PresentedMandate {
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(authority, &m.canonical_bytes()).to_vec(), mandate: m };
        let proof = mycelium_core::tls::sign_bytes(holder, &possession_message(&grant.mandate, &possession_request(OP, RES, digest)));
        PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof) }
    }

    pub(crate) fn checkpoint(authority: &SigningKey, seq: u64, at: u64, revoked: &[&str]) -> SignedRevocationCheckpoint {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new("operator:acme").unwrap(),
            scope: "depot".into(),
            seq,
            issued_at_ms: at,
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(authority, &c.canonical_bytes()).to_vec(), checkpoint: c }
    }

    fn none() -> HashMap<NodeId, MemberKeys> {
        HashMap::new()
    }

    fn state(a: &Assessment) -> Option<MandateState> {
        a.binding.as_ref().map(|b| b.state.clone())
    }

    const DIGEST: [u8; 32] = [7u8; 32];

    #[test]
    fn no_presentation_binds_nothing() {
        let (ak, hk) = keys();
        let a = authority(&ak, &hk).assess(CALLER, OP, RES, &DIGEST, None, 1_000, &none());
        assert_eq!(a, Assessment { binding: None, valid_until_ms: None });
    }

    /// A valid grant, possessed for this call, with a fresh revocation view: **established**.
    #[test]
    fn a_valid_presented_grant_is_established() {
        let (ak, hk) = keys();
        let auth = authority(&ak, &hk);
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &[]), 1_000, &none());
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &hk, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert_eq!(state(&a), Some(MandateState::Established));
        assert_eq!(a.valid_until_ms, Some(50_000));
    }

    /// An authority whose shared set trusts nothing by configuration — the holder's key arrives as a client key.
    fn client_authority(ak: &SigningKey) -> ExecutionAuthority {
        let auth = authority(ak, &SigningKey::from_bytes(&[99u8; 32]));
        let set = auth.external_issuers();
        set.untrust(&IssuerId::new(CALLER).unwrap());
        auth
    }

    /// **A client key proves possession, and nothing else** (identity-and-lifecycle I1; #743's review, items 3 and 4).
    /// The holder's key fed as a client key establishes its grant; a second issuance supersedes it, and a proof by the
    /// old key is refused at the authority — **whatever order the feeds arrive in** (an older issuance fed after the
    /// newer is not written back). The same key fed as a client key for the grant's *authority* signs no checkpoint
    /// and establishes no grant: a client is never an issuer. Seen failing first, one build per plant: the set's
    /// replacement ignoring issuance (*SUPERSEDED*: the old key, fed last, verified), and `verify_signed_by` reading
    /// client keys (*ISSUER*: the client-signed checkpoint was accepted).
    #[test]
    fn a_client_key_proves_possession_only_and_a_superseded_one_never_returns() {
        let (ak, old) = keys();
        let new = SigningKey::from_bytes(&[77u8; 32]);
        let auth = client_authority(&ak);
        let set = auth.external_issuers();
        let holder = IssuerId::new(CALLER).unwrap();
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &[]), 1_000, &none());
        set.set_client_key(holder.clone(), 10, old.verifying_key().to_bytes()).unwrap();
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &old, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert_eq!(state(&a), Some(MandateState::Established), "the client key proves possession");
        // The newer issuance, then the older fed late (a feed that read the marks before the fold raised them).
        set.set_client_key(holder.clone(), 20, new.verifying_key().to_bytes()).unwrap();
        set.set_client_key(holder, 10, old.verifying_key().to_bytes()).unwrap();
        let by_old = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &old, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        let by_new = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &new, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&by_old), Some(MandateState::Unknown(_))), "SUPERSEDED: a proof by the superseded key: {by_old:?}");
        assert_eq!(state(&by_new), Some(MandateState::Established));

        // A client key for the authority's own id: no checkpoint, no grant.
        let rogue = SigningKey::from_bytes(&[78u8; 32]);
        let auth = client_authority(&ak);
        let set = auth.external_issuers();
        set.untrust(&IssuerId::new("operator:acme").unwrap());
        set.set_client_key(IssuerId::new("operator:acme").unwrap(), 1, rogue.verifying_key().to_bytes()).unwrap();
        set.set_client_key(IssuerId::new(CALLER).unwrap(), 1, old.verifying_key().to_bytes()).unwrap();
        let offered = auth.offer_checkpoint(&checkpoint(&rogue, 1, 1_000, &[]), 1_000, &none());
        assert!(!matches!(offered, CheckpointOffer::Accepted), "ISSUER: a client key signed a mandate authority's checkpoint: {offered:?}");
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&rogue, &old, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))), "ISSUER: a client key established a grant as its authority: {a:?}");
    }

    /// **The holder must be the caller.** Someone else's grant is not established for you.
    #[test]
    fn a_grant_held_by_someone_else_is_not_established() {
        let (ak, hk) = keys();
        let auth = authority(&ak, &hk);
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &[]), 1_000, &none());
        let a = auth.assess("token:gw/impostor", OP, RES, &DIGEST, Some(&present(&ak, &hk, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))));
    }

    /// **Possession is bound to this call.** A proof made for different arguments does not carry.
    #[test]
    fn a_proof_for_other_arguments_does_not_carry() {
        let (ak, hk) = keys();
        let auth = authority(&ak, &hk);
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &[]), 1_000, &none());
        let p = present(&ak, &hk, mandate(1, 50_000), &[9u8; 32]);
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&p), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))));
    }

    /// Without a fresh revocation checkpoint, authority is not established — silence is not "not revoked".
    #[test]
    fn no_fresh_revocation_view_is_not_established() {
        let (ak, hk) = keys();
        let a = authority(&ak, &hk).assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &hk, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))));
    }

    /// A superseded appointment is **refused**, not merely unknown — and names the epochs.
    #[test]
    fn a_superseded_appointment_is_refused() {
        let (ak, hk) = keys();
        let auth = authority(&ak, &hk);
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &[]), 1_000, &none());
        assert!(auth.install_epoch(2));
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &hk, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Refused(MandateRefusal::Superseded { .. }))), "{a:?}");
    }

    #[test]
    fn a_revoked_appointment_is_not_established() {
        let (ak, hk) = keys();
        let auth = authority(&ak, &hk);
        auth.offer_checkpoint(&checkpoint(&ak, 1, 1_000, &["t1"]), 1_000, &none());
        let a = auth.assess(CALLER, OP, RES, &DIGEST, Some(&present(&ak, &hk, mandate(1, 50_000), &DIGEST)), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(ref w)) if w.contains("revoked")), "{a:?}");
    }

    /// **The canonical-arguments vector** the SDKs pin: the gateway's digest of `{"text":"dispatch"}` —
    /// sorted keys, no whitespace, UTF-8 — which a caller must reproduce to sign a possession proof.
    #[test]
    fn arguments_digest_golden_vector() {
        let canonical = serde_json::to_vec(&serde_json::json!({ "text": "dispatch" })).unwrap();
        assert_eq!(canonical, br#"{"text":"dispatch"}"#);
        let hex: String = crate::agent::action_evaluator::arguments_digest(&canonical).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "719121f66b67e12629032511ad5cff8f9b541591eed0fffbe645b9f5e14a7a23");
        // The public function computes the same bytes (closure plan C12).
        let public: String = arguments_digest(&serde_json::json!({ "text": "dispatch" })).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(public, hex);
    }

    /// **The golden vector** the SDKs pin: operation, resource before `@`, arguments digest.
    #[test]
    fn possession_request_golden_vector() {
        let digest = [0xabu8; 32];
        let bytes = possession_request("skill.invoke", "skill:depot/dispatch@10.0.0.1:57000", &digest);
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            concat!(
                "6d7963656c69756d2e676174657761792f6d616e646174652d726571756573742f31",
                "0c000000", "736b696c6c2e696e766f6b65",
                "14000000", "736b696c6c3a6465706f742f6469737061746368",
                "abababababababababababababababababababababababababababababababab"
            )
        );
    }

    /// **The arguments' digest is RFC 8785, and for one release the pre-I3 form is accepted beside it**
    /// (identity-and-lifecycle I3, §9 O1). Arguments whose keys the two forms order differently:
    /// a grant bound to the RFC 8785 digest, proved over it, is established on the new form; a grant issued before
    /// the change — bound to and proved over `serde_json`'s form — is established on the legacy form, counted, and the
    /// binding marked so the record says it; a grant bound to one form presented with arguments that match neither
    /// is refused by name; an integer outside ±(2^53 − 1) is refused by name before any digest is compared. Seen
    /// failing first, one build per plant: the legacy form not tried (the pre-I3 grant refused), and the unsafe-integer
    /// check removed (the call assessed on a digest that cannot bind it).
    #[test]
    fn a_grant_binds_the_jcs_digest_and_a_pre_i3_grant_is_honoured_for_one_release() {
        use crate::mandate::grant::GRANT_SCOPE_PREFIX;
        let (authority_key, holder) = keys();
        let mut t = EntitlementTable::new();
        t.entitle_grants(PrincipalId::new("operator:acme").unwrap());
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("depot", 1),
            ResourceTier::Serialised,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        )
        .unwrap();
        let a = ExecutionAuthority::new(gate, GrantVerifier::new(t), external(&authority_key, &holder));
        let members: HashMap<NodeId, MemberKeys> = HashMap::new();
        // Keys where UTF-16 order (RFC 8785) and UTF-8 order (`serde_json`) disagree.
        let args = serde_json::json!({"\u{ff01}": 1, "\u{1F600}": 2});
        let (jcs, legacy) = (arguments_digest(&args), legacy_arguments_digest(&args));
        assert_ne!(jcs, legacy, "the case is one where the two forms differ");
        let grant = |request: &str, digest: &[u8; 32]| {
            let m = Mandate {
                holder: PrincipalId::new(CALLER).unwrap(),
                established_by: PrincipalId::new("operator:acme").unwrap(),
                purpose: format!("grant for {request}"),
                scope: format!("{GRANT_SCOPE_PREFIX}{request}"),
                operations: Mandate::grant_operations(mandate_operation(OP, RES), &hex32(digest), None),
                epoch: 1,
                term: TermId::new(request).unwrap(),
                valid_from_ms: 1_000,
                valid_until_ms: 20_000,
            };
            let signature = crate::tls::sign_bytes(&authority_key, &m.canonical_bytes()).to_vec();
            let possession = crate::tls::sign_bytes(&holder, &possession_message(&m, &possession_request(OP, RES, digest)));
            PresentedMandate { grant: SignedMandateGrant { mandate: m, signature }, possession: base64::engine::general_purpose::STANDARD.encode(possession) }
        };
        for scope in ["grant:new", "grant:old", "grant:other"] {
            let c = RevocationCheckpoint { authority: PrincipalId::new("operator:acme").unwrap(), scope: scope.into(), seq: 1, issued_at_ms: 5_000, revoked: Default::default() };
            let signed = crate::mandate::authority::SignedRevocationCheckpoint { signature: crate::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c };
            let _ = a.offer_checkpoint(&signed, 5_000, &members);
        }
        let (new, new_form) = a.assess_arguments(CALLER, OP, RES, &args, Some(&grant("new", &jcs)), 6_000, &members);
        assert_eq!((state(&new), new_form), (Some(MandateState::Established), Some(DigestForm::Jcs)), "{new:?}");
        assert!(!new.binding.as_ref().unwrap().legacy_digest);
        let (old, old_form) = a.assess_arguments(CALLER, OP, RES, &args, Some(&grant("old", &legacy)), 6_000, &members);
        assert_eq!((state(&old), old_form), (Some(MandateState::Established), Some(DigestForm::Legacy)), "WINDOW: the pre-I3 grant: {old:?}");
        assert!(old.binding.as_ref().unwrap().legacy_digest, "the binding says it matched the legacy form");
        assert_eq!(a.legacy_digest_matches(), 1, "counted");
        // The same call assessed again at this node as a provider is counted there, not at the doors a second time.
        let (at_provider, form) = a.assess_arguments_at(CountAt::Provider, CALLER, OP, RES, &args, Some(&grant("old", &legacy)), 6_000, &members);
        assert_eq!((state(&at_provider), form), (Some(MandateState::Established), Some(DigestForm::Legacy)));
        assert_eq!((a.legacy_digest_matches(), a.legacy_digest_matches_at_provider()), (1, 1), "COUNT: one per point");
        let other = serde_json::json!({"tolerance": 0.5});
        let (refused, _) = a.assess_arguments(CALLER, OP, RES, &other, Some(&grant("other", &jcs)), 6_000, &members);
        assert!(matches!(state(&refused), Some(MandateState::Refused(MandateRefusal::BoundToOtherArguments { .. }))), "{refused:?}");
        let big = serde_json::json!({"id": 9_007_199_254_740_993u64});
        let (unsafe_int, _) = a.assess_arguments(CALLER, OP, RES, &big, Some(&grant("new", &arguments_digest(&big))), 6_000, &members);
        assert_eq!(state(&unsafe_int), Some(MandateState::Refused(MandateRefusal::UnsafeInteger)), "UNSAFE: {unsafe_int:?}");
    }

    /// X3: a grant is one appointment for one call. Its binding is read from its signed operations: the matching
    /// digest is established (with the grant's digest and bound revision on the finding), another digest is refused
    /// by name, a revoked term is not established, and the window is read on the clock the caller passes — a grant
    /// past its `valid_until_ms` is out of window however still the HLC stayed. The resource gate's epoch rule does
    /// not apply: two grants at epoch 1 are both established.
    #[test]
    fn a_grant_is_bound_to_its_arguments_and_expires_on_the_wall_clock() {
        use crate::mandate::grant::GRANT_SCOPE_PREFIX;
        let (authority_key, holder) = keys();
        let mut t = EntitlementTable::new();
        t.entitle("depot", PrincipalId::new("operator:acme").unwrap());
        t.entitle_grants(PrincipalId::new("operator:acme").unwrap());
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("depot", 1),
            ResourceTier::Serialised,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        )
        .unwrap();
        let a = ExecutionAuthority::new(gate, GrantVerifier::new(t), external(&authority_key, &holder));
        let members: HashMap<NodeId, MemberKeys> = HashMap::new();
        let digest = [9u8; 32];
        let grant = |request: &str| {
            let m = Mandate {
                holder: PrincipalId::new(CALLER).unwrap(),
                established_by: PrincipalId::new("operator:acme").unwrap(),
                purpose: format!("grant issued by oidc:idp/alice for request {request}"),
                scope: format!("{GRANT_SCOPE_PREFIX}{request}"),
                operations: Mandate::grant_operations(mandate_operation(OP, RES), &super::super::action_evaluator::hex32(&digest), Some("rev-7")),
                epoch: 1,
                term: TermId::new(request).unwrap(),
                valid_from_ms: 1_000,
                valid_until_ms: 20_000,
            };
            let signature = crate::tls::sign_bytes(&authority_key, &m.canonical_bytes()).to_vec();
            let possession = crate::tls::sign_bytes(&holder, &possession_message(&m, &possession_request(OP, RES, &digest)));
            PresentedMandate { grant: SignedMandateGrant { mandate: m, signature }, possession: base64::engine::general_purpose::STANDARD.encode(possession) }
        };
        // A fresh checkpoint from the issuing authority, so revocation standing is known.
        let checkpoint = |seq: u64, revoked: &[&str], at: u64, scope: &str| {
            let c = RevocationCheckpoint {
                authority: PrincipalId::new("operator:acme").unwrap(),
                scope: scope.into(),
                seq,
                issued_at_ms: at,
                revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
            };
            crate::mandate::authority::SignedRevocationCheckpoint { signature: crate::tls::sign_bytes(&authority_key, &c.canonical_bytes()).to_vec(), checkpoint: c }
        };
        let _ = a.offer_checkpoint(&checkpoint(1, &[], 5_000, "grant:r1"), 5_000, &members);
        let _ = a.offer_checkpoint(&checkpoint(1, &[], 5_000, "grant:r2"), 5_000, &members);

        let g1 = grant("r1");
        let found = a.assess(CALLER, OP, RES, &digest, Some(&g1), 5_000, &members);
        let b = found.binding.expect("a finding");
        assert_eq!(b.state, MandateState::Established, "{b:?}");
        assert_eq!(b.digest.as_deref(), Some(super::super::action_evaluator::hex32(&g1.grant.mandate.digest()).as_str()));
        assert_eq!(b.bound_policy_revision.as_deref(), Some("rev-7"));
        // A second grant at the same epoch, its own appointment.
        let g2 = grant("r2");
        assert_eq!(a.assess(CALLER, OP, RES, &digest, Some(&g2), 5_000, &members).binding.unwrap().state, MandateState::Established);
        // Other arguments: refused by name, before anything else.
        let other = [10u8; 32];
        let possession = crate::tls::sign_bytes(&holder, &possession_message(&g1.grant.mandate, &possession_request(OP, RES, &other)));
        let g1_other = PresentedMandate { grant: g1.grant.clone(), possession: base64::engine::general_purpose::STANDARD.encode(possession) };
        assert!(matches!(a.assess(CALLER, OP, RES, &other, Some(&g1_other), 5_000, &members).binding.unwrap().state,
            MandateState::Refused(MandateRefusal::BoundToOtherArguments { .. })));
        // Past the window on the clock passed in (the HLC is not consulted here at all).
        assert!(matches!(a.assess(CALLER, OP, RES, &digest, Some(&g1), 20_001, &members).binding.unwrap().state,
            MandateState::Refused(MandateRefusal::OutOfWindow { .. })));
        // Revoked: not established, by name.
        let _ = a.offer_checkpoint(&checkpoint(2, &["r1"], 6_000, "grant:r1"), 6_000, &members);
        assert!(matches!(a.assess(CALLER, OP, RES, &digest, Some(&g1), 6_000, &members).binding.unwrap().state,
            MandateState::Unknown(ref why) if why.contains("revoked")));
        assert_eq!(a.assess(CALLER, OP, RES, &digest, Some(&g2), 6_000, &members).binding.unwrap().state, MandateState::Established, "the other grant stands");
    }
}

/// **Test-only: what a gateway mints for one call** (identity-and-lifecycle I4). The gateway's own mint is the next
/// PR's (`gateway_mint_call_mandates`, inside framing); until it lands, the tests mint here with a node key, in the
/// shape `assess_call` verifies: scope `call:{term}`, issuer and holder the gateway, the one operation of the call, a
/// possession proof over the RFC 8785 arguments digest, and the first checkpoint (sequence 1, nothing revoked).
#[cfg(test)]
pub(crate) mod call_mint {
    use super::*;
    use crate::mandate::authority::RevocationCheckpoint;
    use crate::mandate::grant::{possession_message, CALL_SCOPE_PREFIX};
    use crate::mandate::{Mandate, PrincipalId, TermId};
    use ed25519_dalek::SigningKey;

    /// One call's mandate, before it is signed.
    pub(crate) fn mandate(gateway: &str, term: &str, operation: &str, resource: &str, from_ms: u64, until_ms: u64) -> Mandate {
        Mandate {
            holder: PrincipalId::new(gateway).unwrap(),
            established_by: PrincipalId::new(gateway).unwrap(),
            purpose: format!("call {term} dispatched by {gateway}"),
            scope: format!("{CALL_SCOPE_PREFIX}{term}"),
            operations: vec![mandate_operation(operation, resource)],
            epoch: 1,
            term: TermId::new(term).unwrap(),
            valid_from_ms: from_ms,
            valid_until_ms: until_ms,
        }
    }

    /// A checkpoint of `gateway` for `scope`, signed by `key`.
    pub(crate) fn checkpoint(key: &SigningKey, gateway: &str, scope: &str, seq: u64, at_ms: u64, revoked: &[&str]) -> SignedRevocationCheckpoint {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new(gateway).unwrap(),
            scope: scope.into(),
            seq,
            issued_at_ms: at_ms,
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(key, &c.canonical_bytes()).to_vec(), checkpoint: c }
    }

    /// `m` signed by `key`, its possession proof for this call, and its first checkpoint.
    pub(crate) fn present(key: &SigningKey, m: Mandate, operation: &str, resource: &str, arguments: &serde_json::Value) -> PresentedCallMandate {
        let first = checkpoint(key, m.established_by.as_str(), &m.scope, 1, m.valid_from_ms, &[]);
        let proof = mycelium_core::tls::sign_bytes(key, &possession_message(&m, &possession_request(operation, resource, &arguments_digest(arguments))));
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(key, &m.canonical_bytes()).to_vec(), mandate: m };
        PresentedCallMandate::new(grant, &proof, first)
    }

    /// The whole mint for one call.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mint(key: &SigningKey, gateway: &str, term: &str, operation: &str, resource: &str, arguments: &serde_json::Value, from_ms: u64, until_ms: u64) -> PresentedCallMandate {
        present(key, mandate(gateway, term, operation, resource, from_ms, until_ms), operation, resource, arguments)
    }
}

/// Identity-and-lifecycle I4 at the authority: `assess_call`, the windows, the sweep and the stop records, with no
/// node. The node-level halves (the KV read, the feeder) are in `call_mandate_tests`.
#[cfg(test)]
mod call_tests {
    use super::call_mint::{checkpoint, mandate, mint, present};
    use super::tests::{external, keys, OP, RES};
    use super::*;
    use crate::agent::action_evaluator::MandateState;
    use crate::knowledge::issuer::MemberKeys;
    use crate::mandate::authority::{ClockModel, FreshnessPolicy, ResourceTier};
    use crate::mandate::grant::EntitlementTable;
    use crate::mandate::{PrincipalId, ResourceAuthority};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;
    use std::sync::Arc;

    const GW: &str = "node:127.0.0.1:7601";
    const OTHER_GW: &str = "node:127.0.0.1:7602";

    fn gw_key() -> SigningKey {
        SigningKey::from_bytes(&[51u8; 32])
    }
    fn other_key() -> SigningKey {
        SigningKey::from_bytes(&[52u8; 32])
    }
    /// Both gateways are members whose keys this provider holds.
    fn members() -> HashMap<NodeId, MemberKeys> {
        HashMap::from([
            (NodeId::new("127.0.0.1", 7601).unwrap(), MemberKeys { retained: vec![gw_key().verifying_key().to_bytes()], ..Default::default() }),
            (NodeId::new("127.0.0.1", 7602).unwrap(), MemberKeys { retained: vec![other_key().verifying_key().to_bytes()], ..Default::default() }),
        ])
    }
    fn args() -> serde_json::Value {
        serde_json::json!({ "text": "restock aisle 4" })
    }
    /// A provider's authority. `fleet`: grants from every member are entitled — which must not reach `call:`.
    fn provider(fleet: bool) -> Arc<ExecutionAuthority> {
        let (ak, hk) = keys();
        let mut t = EntitlementTable::new();
        t.entitle("depot", PrincipalId::new("operator:acme").unwrap());
        t.entitle_grants(PrincipalId::new("operator:acme").unwrap());
        if fleet {
            t.entitle_grants_from_fleet();
        }
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("depot", 1),
            ResourceTier::Serialised,
            ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        )
        .unwrap();
        Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(t), external(&ak, &hk)))
    }
    fn state(a: &Assessment) -> Option<MandateState> {
        a.binding.as_ref().map(|b| b.state.clone())
    }
    fn no_local(_: &PrincipalId, _: &str) -> Option<SignedRevocationCheckpoint> {
        None
    }
    fn assess(p: &ExecutionAuthority, gateway: &str, c: &PresentedCallMandate, now: u64) -> Assessment {
        p.assess_call(gateway, OP, RES, &args(), c, now, &members(), no_local)
    }
    fn begin(p: &Arc<ExecutionAuthority>, c: &PresentedCallMandate, now: u64) -> WorkGuard {
        let m = &c.grant.mandate;
        p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(m.clone()), m.valid_until_ms), Arc::new(move || now))
    }

    /// **Exit 1, at the authority.** A provider that has never seen a call's checkpoint establishes its `call:`
    /// mandate from the carried one, and the work is registered. Seen failing first: with the carried checkpoint not
    /// offered — the stripped form, the only one before the row — the mandate is `Unknown`.
    #[test]
    fn a_call_mandate_is_established_from_its_carried_checkpoint_and_registered() {
        let p = provider(false);
        assert_eq!(p.view_sizes(), (0, 0, 0));
        let c = mint(&gw_key(), GW, "c1", OP, RES, &args(), 1_000, 60_000);
        let a = assess(&p, GW, &c, 2_000);
        assert_eq!(state(&a), Some(MandateState::Established), "{a:?}");
        assert_eq!(a.valid_until_ms, Some(60_000));
        assert_eq!(p.view_sizes(), (1, 1, 1), "one checkpoint, one revoked-set entry and one window per admitted call");
        let g = begin(&p, &c, 2_000);
        assert_eq!(p.running_count(), 1);
        drop(g);
        assert_eq!(p.running_count(), 0);
    }

    /// **Exit 2 and N3 (c), D-1.** The same mandate in the first slot — `assess`, where a caller's mandate goes —
    /// establishes nothing, presented by the gateway itself or by a client: a `call:` mandate is never a caller's.
    /// Seen failing first: without the `is_call` arm in `assess_digests`, the reason is the entitlement table's
    /// (`NotEntitled`), which an operator entitling the scope would lift.
    #[test]
    fn a_call_mandate_in_the_first_slot_is_not_a_callers() {
        let (ak, hk) = keys();
        let mut t = EntitlementTable::new();
        // Even entitled by name and from the fleet, the first slot does not take it.
        t.entitle("call:c2", PrincipalId::new(GW).unwrap());
        t.entitle_grants_from_fleet();
        let gate = ExecutionGate::strict(ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 }, FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 }).unwrap();
        let p = ExecutionAuthority::new(gate, GrantVerifier::new(t), external(&ak, &hk));
        let c = mint(&gw_key(), GW, "c2", OP, RES, &args(), 1_000, 60_000);
        let slot1 = PresentedMandate { grant: c.grant.clone(), possession: c.possession.clone() };
        for caller in [GW, "token:gw/agent-1"] {
            let (a, form) = p.assess_arguments(caller, OP, RES, &args(), Some(&slot1), 2_000, &members());
            assert!(matches!(state(&a), Some(MandateState::Unknown(ref why)) if why == "a call: mandate is not a caller's"), "{caller}: {a:?}");
            assert_eq!(form, None);
        }
        assert_eq!(p.view_sizes(), (0, 0, 0), "and nothing is held for it");
    }

    /// **Exit 3 and N3 (b), (d), D-6.** A `call:` mandate counts only when its issuer and its holder are the gateway
    /// whose envelope carries it. From another gateway's envelope it is `Unknown`; issued by one member for another
    /// is `Unknown`; signed by another member node in the gateway's name is `Unknown` — all with grants entitled from
    /// the whole fleet, which does not reach `call:`. Nothing is held for any of them. Seen failing first: with
    /// `check_call_mandate` verifying under the mandate's own `established_by` — any member an issuer, the spike's
    /// rule — the first is `Established`.
    #[test]
    fn a_call_mandate_counts_only_from_the_gateway_that_issued_and_holds_it() {
        let p = provider(true);
        // (b) The gateway's own mandate, carried in another gateway's envelope.
        let c = mint(&gw_key(), GW, "c3", OP, RES, &args(), 1_000, 60_000);
        let a = assess(&p, OTHER_GW, &c, 2_000);
        assert!(matches!(state(&a), Some(MandateState::Unknown(ref why)) if why.contains("the gateway that sent it")), "{a:?}");
        // (d) Issued by the other member, held by the sending gateway: `established_by` is not the holder.
        let mut m = mandate(GW, "c4", OP, RES, 1_000, 60_000);
        m.established_by = PrincipalId::new(OTHER_GW).unwrap();
        let first = checkpoint(&other_key(), OTHER_GW, &m.scope, 1, 1_000, &[]);
        let proof = mycelium_core::tls::sign_bytes(&gw_key(), &crate::mandate::grant::possession_message(&m, &possession_request(OP, RES, &arguments_digest(&args()))));
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&other_key(), &m.canonical_bytes()).to_vec(), mandate: m };
        let a = assess(&p, GW, &PresentedCallMandate::new(grant, &proof, first), 2_000);
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))), "{a:?}");
        // Signed by another member's key in the gateway's name: the signature is not the gateway's.
        let forged = mint(&other_key(), GW, "c5", OP, RES, &args(), 1_000, 60_000);
        let a = assess(&p, GW, &forged, 2_000);
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))), "{a:?}");
        // A principal that is not a node's.
        let token = mint(&gw_key(), "token:gw/agent-1", "c6", OP, RES, &args(), 1_000, 60_000);
        assert!(matches!(state(&assess(&p, "token:gw/agent-1", &token, 2_000)), Some(MandateState::Unknown(_))));
        // A scope that is not `call:{term}`.
        let mut m = mandate(GW, "c7", OP, RES, 1_000, 60_000);
        m.scope = "call:c8".into();
        assert!(matches!(state(&assess(&p, GW, &present(&gw_key(), m, OP, RES, &args()), 2_000)), Some(MandateState::Unknown(_))));
        assert_eq!(p.view_sizes(), (0, 0, 0), "a mandate that does not verify plants nothing");
        // And the honest one, last, is established at the same provider.
        assert_eq!(state(&assess(&p, GW, &c, 2_000)), Some(MandateState::Established));
    }

    /// **What else `assess_call` refuses**, each by name: another call's arguments or operation, a window past its
    /// end or longer than the cap, an integer no digest binds, a carried checkpoint for another scope.
    #[test]
    fn a_call_mandate_is_bound_to_its_call_and_its_window() {
        let p = provider(false);
        let c = mint(&gw_key(), GW, "c10", OP, RES, &args(), 1_000, 60_000);
        // Other arguments: the possession proof is for this call's digest.
        let a = p.assess_call(GW, OP, RES, &serde_json::json!({ "text": "something else" }), &c, 2_000, &members(), no_local);
        assert!(matches!(state(&a), Some(MandateState::Unknown(ref why)) if why.contains("WrongHolder")), "{a:?}");
        // Another resource: the proof and the enumerated operation are for this one.
        let a = p.assess_call(GW, OP, "skill:depot/other@127.0.0.1:9", &args(), &c, 2_000, &members(), no_local);
        assert!(matches!(state(&a), Some(MandateState::Unknown(_))), "{a:?}");
        // A mandate enumerating another operation than the one its proof is for.
        let mut m = mandate(GW, "c11", OP, RES, 1_000, 60_000);
        m.operations = vec!["tools/call:tool:other".into()];
        let a = assess(&p, GW, &present(&gw_key(), m, OP, RES, &args()), 2_000);
        assert!(matches!(state(&a), Some(MandateState::Refused(MandateRefusal::NotEnumerated { .. }))), "{a:?}");
        // Past its window.
        assert!(matches!(state(&assess(&p, GW, &c, 60_001)), Some(MandateState::Refused(MandateRefusal::OutOfWindow { .. }))));
        // A window longer than the cap: a receiver would hold it that long.
        let long = mint(&gw_key(), GW, "c12", OP, RES, &args(), 1_000, 1_000 + crate::mandate::grant::GRANT_TTL_CAP_SECS * 1_000 + 1);
        assert!(matches!(state(&assess(&p, GW, &long, 2_000)), Some(MandateState::Refused(MandateRefusal::OutOfWindow { .. }))));
        let at_cap = mint(&gw_key(), GW, "c13", OP, RES, &args(), 1_000, 1_000 + crate::mandate::grant::GRANT_TTL_CAP_SECS * 1_000);
        assert_eq!(state(&assess(&p, GW, &at_cap, 2_000)), Some(MandateState::Established));
        // An integer outside ±(2^53 − 1).
        let big = serde_json::json!({ "id": 9_007_199_254_740_993u64 });
        let c14 = mint(&gw_key(), GW, "c14", OP, RES, &big, 1_000, 60_000);
        let a = p.assess_call(GW, OP, RES, &big, &c14, 2_000, &members(), no_local);
        assert!(matches!(state(&a), Some(MandateState::Refused(MandateRefusal::UnsafeInteger))), "{a:?}");
        // A checkpoint carried for another scope speaks for nothing, and is not offered.
        let mut c15 = mint(&gw_key(), GW, "c15", OP, RES, &args(), 1_000, 60_000);
        c15.checkpoint = checkpoint(&gw_key(), GW, "call:c999", 1, 1_000, &[]);
        assert!(matches!(state(&assess(&p, GW, &c15, 2_000)), Some(MandateState::Unknown(ref why)) if why.contains("carried checkpoint")));
        assert_eq!(p.view_sizes().2, 1, "and no window is held for it: only c13's, the one mandate established so far");
        // A carried checkpoint the gateway did not sign: the window is held (the mandate verified), nothing else.
        let before = p.view_sizes();
        let mut c16 = mint(&gw_key(), GW, "c16", OP, RES, &args(), 1_000, 60_000);
        c16.checkpoint = checkpoint(&other_key(), GW, "call:c16", 1, 1_000, &[]);
        assert!(matches!(state(&assess(&p, GW, &c16, 2_000)), Some(MandateState::Unknown(_))));
        assert_eq!(p.view_sizes(), (before.0, before.1, before.2 + 1));
    }

    /// **Exits 5 and 6, at the authority (bound 1).** A term whose revocation is in the provider's own KV when the
    /// call is admitted is `Refused(Revoked)` there, by name. A call admitted before its revocation arrives is
    /// registered, and stopped by the sweep once the revoking checkpoint is offered; its stop record names the
    /// `call:` scope and term. Re-presented with its stale first checkpoint, it stays refused.
    #[test]
    fn a_revoked_term_is_refused_and_running_work_is_stopped_when_the_revocation_arrives() {
        let p = provider(false);
        // Already revoked in this node's KV: read under the hold, refused before anything is registered.
        let c = mint(&gw_key(), GW, "c20", OP, RES, &args(), 1_000, 60_000);
        let revoking = checkpoint(&gw_key(), GW, "call:c20", 2, 1_500, &["c20"]);
        let a = p.assess_call(GW, OP, RES, &args(), &c, 2_000, &members(), |by, scope| {
            assert_eq!((by.as_str(), scope), (GW, "call:c20"));
            Some(revoking.clone())
        });
        assert!(matches!(state(&a), Some(MandateState::Refused(MandateRefusal::Revoked { ref term })) if term == "c20"), "{a:?}");
        // The race: framed with sequence 1, admitted before the revocation reaches this node.
        let r = mint(&gw_key(), GW, "c21", OP, RES, &args(), 1_000, 60_000);
        assert_eq!(state(&assess(&p, GW, &r, 2_000)), Some(MandateState::Established));
        let g = begin(&p, &r, 3_000);
        assert_eq!(p.sweep(2_100), 0, "registered, and nothing has revoked it here yet");
        assert!(p.named_stop_records().is_empty());
        let offered = p.offer_checkpoint(&checkpoint(&gw_key(), GW, "call:c21", 2, 1_800, &["c21"]), 2_200, &members());
        assert_eq!(offered, CheckpointOffer::Accepted);
        assert_eq!(p.sweep(2_200), 1, "the revoking checkpoint's arrival stops it at the next sweep");
        assert!(g.is_cancelled());
        drop(g);
        let stops = p.named_stop_records();
        assert_eq!(stops.len(), 1);
        assert_eq!((stops[0].scope.as_deref(), stops[0].term.as_deref()), (Some("call:c21"), Some("c21")), "the record names the mandate whose lapse stopped the work");
        assert_eq!(p.stop_records(), vec![stops[0].record], "and `stop_records` is the same records, unchanged in shape");
        // The stale carried checkpoint cannot un-revoke.
        assert!(matches!(state(&assess(&p, GW, &r, 2_300)), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
    }

    /// **N4, at the authority: a call registered under both slots.** Two `begin`s, so `running_count` is 2 for one
    /// call; revoking the `call:` term cancels that registration alone and its record names the term; revoking the
    /// grant cancels the other and names the grant. Seen failing first: with the sweep's arm reading `is_grant()`
    /// only, `call:` work is judged by the resource gate and cancelled at the first sweep for its scope, revoked or
    /// not.
    #[test]
    fn a_call_under_both_slots_is_registered_twice_and_each_record_names_its_mandate() {
        let (ak, _) = keys();
        let p = provider(false);
        let c = mint(&gw_key(), GW, "c30", OP, RES, &args(), 1_000, 60_000);
        assert_eq!(state(&assess(&p, GW, &c, 2_000)), Some(MandateState::Established));
        // A client's grant, its issuer's first checkpoint held.
        let mut grant = crate::agent::gateway_authority::tests::mandate(1, 60_000);
        grant.scope = "grant:r30".into();
        grant.term = crate::mandate::TermId::new("r30").unwrap();
        let grant_cp = |seq, revoked: &[&str]| {
            let c = crate::mandate::authority::RevocationCheckpoint {
                authority: PrincipalId::new("operator:acme").unwrap(),
                scope: "grant:r30".into(),
                seq,
                issued_at_ms: 1_000,
                revoked: revoked.iter().map(|t| crate::mandate::TermId::new(t).unwrap()).collect(),
            };
            SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&ak, &c.canonical_bytes()).to_vec(), checkpoint: c }
        };
        assert_eq!(p.offer_checkpoint(&grant_cp(1, &[]), 2_000, &members()), CheckpointOffer::Accepted);
        let register = |p: &Arc<ExecutionAuthority>| {
            let under_grant = p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(grant.clone()), 60_000), Arc::new(|| 2_000));
            (under_grant, begin(p, &c, 2_000))
        };
        let (under_grant, under_call) = register(&p);
        assert_eq!(p.running_count(), 2, "one call, two registrations");
        assert_eq!(p.sweep(2_100), 0, "neither is revoked: `call:` work is not judged by the resource gate");
        // The cancel: the `call:` term revoked.
        let _ = p.offer_checkpoint(&checkpoint(&gw_key(), GW, "call:c30", 2, 2_150, &["c30"]), 2_200, &members());
        assert_eq!(p.sweep(2_200), 1);
        assert!(under_call.is_cancelled() && !under_grant.is_cancelled());
        // The handler's future drops both guards; only the cancelled one writes a record.
        drop((under_grant, under_call));
        let stops = p.named_stop_records();
        assert_eq!(stops.iter().map(|s| (s.scope.as_deref(), s.term.as_deref())).collect::<Vec<_>>(), vec![(Some("call:c30"), Some("c30"))]);
        assert_eq!(p.running_count(), 0);
        // The other way: a second call under the same grant, the grant revoked.
        let c2 = mint(&gw_key(), GW, "c31", OP, RES, &args(), 1_000, 60_000);
        assert_eq!(state(&assess(&p, GW, &c2, 2_300)), Some(MandateState::Established));
        let under_grant = p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(grant.clone()), 60_000), Arc::new(|| 2_300));
        let under_call = begin(&p, &c2, 2_300);
        let _ = p.offer_checkpoint(&grant_cp(2, &["r30"]), 2_400, &members());
        assert_eq!(p.sweep(2_400), 1);
        assert!(under_grant.is_cancelled() && !under_call.is_cancelled());
        drop((under_grant, under_call));
        assert_eq!(p.named_stop_records().last().map(|s| (s.scope.clone(), s.term.clone())), Some((Some("grant:r30".into()), Some("r30".into()))));
    }

    /// **Exit 9, at the authority: what is held, and until when.** A `call:` checkpoint for a scope this node
    /// admitted no call in is not kept (`NotHeld`). An admitted scope is kept until its mandate's window ends — not
    /// until its work ends — so a cancelled call presented again inside the window stays refused; one sweep past
    /// the window nothing is held, and a late checkpoint for the forgotten scope is not kept either. A second
    /// mandate on the scope with a later window moves the scope's expiry.
    #[test]
    fn a_call_scope_is_held_for_its_window_and_only_where_it_was_admitted() {
        let p = provider(false);
        // Not admitted here: nothing is kept, whatever arrives.
        for seq in 1..=2 {
            assert_eq!(p.offer_checkpoint(&checkpoint(&gw_key(), GW, "call:c40", seq, 1_000, &["c40"]), 2_000, &members()), CheckpointOffer::NotHeld);
        }
        assert_eq!(p.view_sizes(), (0, 0, 0));
        // Admitted, run, finished, then cancelled late: still held, still refused inside the window.
        let c = mint(&gw_key(), GW, "c41", OP, RES, &args(), 1_000, 10_000);
        assert_eq!(state(&assess(&p, GW, &c, 2_000)), Some(MandateState::Established));
        drop(begin(&p, &c, 2_000));
        let _ = p.offer_checkpoint(&checkpoint(&gw_key(), GW, "call:c41", 2, 2_500, &["c41"]), 3_000, &members());
        p.sweep(9_999);
        assert_eq!(p.view_sizes(), (1, 1, 1), "kept until the window ends, not until the work ends");
        assert!(matches!(state(&assess(&p, GW, &c, 9_999)), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
        p.sweep(10_000);
        assert_eq!(p.view_sizes(), (1, 1, 1), "the window's last millisecond is inside it");
        p.sweep(10_001);
        assert_eq!(p.view_sizes(), (0, 0, 0), "one sweep past the window, nothing is held");
        assert_eq!(p.offer_checkpoint(&checkpoint(&gw_key(), GW, "call:c41", 3, 9_000, &["c41"]), 10_002, &members()), CheckpointOffer::NotHeld);
        assert_eq!(p.view_sizes(), (0, 0, 0), "and a checkpoint arriving late for the forgotten scope is not kept");
        assert!(matches!(state(&assess(&p, GW, &c, 10_002)), Some(MandateState::Refused(MandateRefusal::OutOfWindow { .. }))), "past the window the mandate is refused whatever was forgotten");
        // A second mandate on one scope with a later end moves the scope's expiry.
        let early = mint(&gw_key(), GW, "c42", OP, RES, &args(), 1_000, 5_000);
        let late = mint(&gw_key(), GW, "c42", OP, RES, &args(), 1_000, 8_000);
        assert_eq!(state(&assess(&p, GW, &early, 2_000)), Some(MandateState::Established));
        assert_eq!(state(&assess(&p, GW, &late, 2_000)), Some(MandateState::Established));
        assert_eq!(state(&assess(&p, GW, &early, 2_000)), Some(MandateState::Established), "and the earlier end, seen again, does not move it back");
        p.sweep(5_001);
        assert_eq!(p.view_sizes().2, 1, "the scope is held while the later mandate can still be presented");
        p.sweep(8_001);
        assert_eq!(p.view_sizes(), (0, 0, 0));
    }

    /// **Exit 10: no sweep reclaims more than its cap.** `RECLAIM_PER_SWEEP` + 10 windows, all expired: the first
    /// sweep forgets exactly the cap, the second the rest.
    #[test]
    fn a_sweep_reclaims_at_most_its_cap() {
        let p = provider(false);
        let by = PrincipalId::new(GW).unwrap();
        {
            let mut st = p.state.lock().unwrap();
            for i in 0..RECLAIM_PER_SWEEP + 10 {
                st.keep_call_window(&by, &format!("call:w{i}"), 1_000 + (i as u64 % 7));
            }
        }
        assert_eq!(p.view_sizes().2, RECLAIM_PER_SWEEP + 10);
        p.sweep(2_000);
        assert_eq!(p.view_sizes().2, 10);
        p.sweep(2_000);
        assert_eq!(p.view_sizes().2, 0);
    }

    /// **Exit 11 and N5 (b): the start rule.** A reader that started after a call was minted — a restarted
    /// provider, where no window table is persisted (D-4) — still establishes it: a `call:` checkpoint is exempt from
    /// the start rule as a grant's is. And N5 (a) at the view: the same reader, finding the revoking checkpoint in
    /// its KV, refuses the call. Seen failing first: under the `grant:`-only exemption the first is `Unknown`.
    #[test]
    fn a_call_minted_before_a_restart_is_established_after_it_and_a_revoked_one_refused() {
        let p = provider(false);
        p.mark_started(50_000);
        let c = mint(&gw_key(), GW, "c50", OP, RES, &args(), 1_000, 90_000);
        let a = assess(&p, GW, &c, 51_000);
        assert_eq!(state(&a), Some(MandateState::Established), "{a:?}");
        // A fresh reader again; this time its KV holds the revocation written before it restarted.
        let q = provider(false);
        q.mark_started(50_000);
        let revoking = checkpoint(&gw_key(), GW, "call:c50", 2, 2_000, &["c50"]);
        let a = q.assess_call(GW, OP, RES, &args(), &c, 51_000, &members(), |_, _| Some(revoking.clone()));
        assert!(matches!(state(&a), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))), "{a:?}");
        // A standing scope is still under the rule.
        let (ak, _) = keys();
        let old = crate::agent::gateway_authority::tests::checkpoint(&ak, 1, 1_000, &[]);
        assert!(matches!(p.offer_checkpoint(&old, 51_000, &members()), CheckpointOffer::IssuedBeforeStart { .. }));
    }

    /// **The wire shape of `cm`.** A `PresentedCallMandate` round-trips, its checkpoint's signature is base64 on
    /// the wire, and a mandate without a checkpoint does not parse — the checkpoint is required.
    #[test]
    fn a_presented_call_mandate_carries_its_checkpoint_with_a_base64_signature() {
        let c = mint(&gw_key(), GW, "c60", OP, RES, &args(), 1_000, 60_000);
        let v = serde_json::to_value(&c).unwrap();
        let sig = v["checkpoint"]["signature"].as_str().expect("base64, not an array");
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(sig).unwrap(), c.checkpoint.signature);
        assert_eq!(v["checkpoint"]["checkpoint"]["scope"], "call:c60");
        assert_eq!(serde_json::from_value::<PresentedCallMandate>(v.clone()).unwrap(), c);
        let mut bare = v.clone();
        bare.as_object_mut().unwrap().remove("checkpoint");
        assert!(serde_json::from_value::<PresentedCallMandate>(bare).is_err());
        let mut array = v;
        array["checkpoint"]["signature"] = serde_json::json!(c.checkpoint.signature);
        assert!(serde_json::from_value::<PresentedCallMandate>(array).is_err(), "the array form is not accepted");
        println!("cm: {} B as JSON (its checkpoint {} B)", serde_json::to_vec(&c).unwrap().len(), serde_json::to_vec(&serde_json::to_value(&c).unwrap()["checkpoint"]).unwrap().len());
    }

    /// **Exit 10's measurement: how long a sweep holds row 43 for the `call:` windows.** 10⁵ windows, all expired:
    /// each sweep reclaims its cap. Run in release, `--ignored --nocapture`; the figures are on the lock-order row.
    #[test]
    #[ignore = "a measurement: cargo test --release --lib --features compliance,a2a a_sweeps_hold_is_measured -- --ignored --nocapture"]
    fn a_sweeps_hold_is_measured_on_100k_windows() {
        let p = provider(false);
        let by = PrincipalId::new(GW).unwrap();
        const N: usize = 100_000;
        {
            let mut st = p.state.lock().unwrap();
            for i in 0..N {
                st.keep_call_window(&by, &format!("call:w{i}"), 10_000);
            }
        }
        let t = std::time::Instant::now();
        p.sweep(9_000);
        let idle = t.elapsed();
        let (mut sweeps, mut worst, total) = (0, std::time::Duration::ZERO, std::time::Instant::now());
        while p.view_sizes().2 > 0 {
            let t = std::time::Instant::now();
            p.sweep(10_001);
            worst = worst.max(t.elapsed());
            sweeps += 1;
        }
        println!("sweep hold, {N} call: windows: idle sweep {idle:?}; {sweeps} reclaiming sweeps, worst {worst:?}, total {:?}", total.elapsed());
        assert_eq!(sweeps, N.div_ceil(RECLAIM_PER_SWEEP));
    }
}
