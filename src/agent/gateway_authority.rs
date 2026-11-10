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
    stops: Mutex<std::collections::VecDeque<crate::mandate::authority::StopRecord>>,
    /// Adopter-surface X3 (second PR), lock-order row 70: the grants this authority has admitted once, by scope, term
    /// and door — the single-use claim's atomic half (the marker under the request in the KV is its durable half).
    /// Leaf, µs.
    used_grants: Mutex<std::collections::HashMap<(String, String, String, String), u64>>,
    /// Mandates established on the pre-I3 digest form (identity-and-lifecycle I3, §9 O1), at this node's gateway doors
    /// and at it as a provider — two counters, so a call a node assesses at both is not one number counted twice.
    legacy_digest_matches: std::sync::atomic::AtomicU64,
    legacy_digest_matches_at_provider: std::sync::atomic::AtomicU64,
    legacy_digest_matches_at_agent: std::sync::atomic::AtomicU64,
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
        self.authority.running.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.id);
        if self.is_cancelled() {
            let at = |v: &std::sync::atomic::AtomicU64| match v.load(std::sync::atomic::Ordering::Acquire) {
                0 => None,
                t => Some(t),
            };
            let now = (self.now_ms)();
            let record = crate::mandate::authority::StopRecord {
                requested_ms: at(&self.requested_ms),
                acknowledged_ms: at(&self.acknowledged_ms).or(Some(now)),
                confirmed_ms: Some(now),
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
            state: Mutex::new(AuthorityState { gate, verifier, revocations: RevocationView::new() }),
            external,
            durable: std::sync::OnceLock::new(),
            running: Mutex::new(std::collections::HashMap::new()),
            next_work: std::sync::atomic::AtomicU64::new(1),
            stops: Mutex::new(std::collections::VecDeque::new()),
            used_grants: Mutex::new(std::collections::HashMap::new()),
            legacy_digest_matches: std::sync::atomic::AtomicU64::new(0),
            legacy_digest_matches_at_provider: std::sync::atomic::AtomicU64::new(0),
            legacy_digest_matches_at_agent: std::sync::atomic::AtomicU64::new(0),
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
            let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            snapshot
                .into_iter()
                .filter(|(_, w)| match &w.mandate {
                    // Adopter-surface X3 (second PR): work under a **grant** is judged as `assess` judges a grant — its
                    // own window and its issuer's checkpoint for the grant's scope — not by the resource gate, whose
                    // scope and epoch rules a grant's own scope would fail at the first sweep (a defect of the first PR,
                    // unobserved because no granted handler ran longer than one sweep interval).
                    Some(m) if m.is_grant() => {
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
        self.stops.lock().unwrap_or_else(|e| e.into_inner()).iter().copied().collect()
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
        let (policy, clock) = (st.gate.freshness(), st.gate.clock());
        st.revocations.offer(signed, &policy, clock, now_ms, members, &external)
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
