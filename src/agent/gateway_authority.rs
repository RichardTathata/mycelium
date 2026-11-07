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
use crate::knowledge::issuer::{MemberKeySource, TrustedExternalIssuers};
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
    /// SPIKE S-cancel: the mandate's first revocation checkpoint, carried beside it so a provider can establish a
    /// `call:` mandate before the checkpoint gossips. Absent on every mandate today.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<SignedRevocationCheckpoint>,
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
pub fn arguments_digest(arguments: &serde_json::Value) -> [u8; 32] {
    super::action_evaluator::arguments_digest(&serde_json::to_vec(arguments).unwrap_or_default())
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
    /// SPIKE S-cancel: each `call:` scope the view holds, with its mandate's `valid_until_ms` — what the sweep
    /// reclaims once the window has passed.
    call_windows: std::collections::HashMap<(crate::mandate::PrincipalId, String), u64>,
}

/// A gateway's execution authority: attached with `GossipAgent::with_execution_authority`.
pub struct ExecutionAuthority {
    state: Mutex<AuthorityState>,
    external: TrustedExternalIssuers,
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
}

/// The words `assess` gives a revoked grant's standing — read by the agent's wait, which treats this one unknown
/// standing as final (adopter-surface X3, second PR).
pub(crate) const GRANT_REVOKED: &str = "the grant is revoked";

/// The key a grant's checkpoints are written under by the issuing gateway: `grant/{agent}/{request}/checkpoint`.
pub(crate) fn grant_checkpoint_key(grant: &crate::mandate::Mandate) -> String {
    format!("grant/{}/{}/checkpoint", grant.holder.as_str(), grant.term.as_str())
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
    pub fn new(gate: ExecutionGate, verifier: GrantVerifier, external: TrustedExternalIssuers) -> Self {
        Self {
            state: Mutex::new(AuthorityState { gate, verifier, revocations: RevocationView::new(), call_windows: std::collections::HashMap::new() }),
            external,
            durable: std::sync::OnceLock::new(),
            running: Mutex::new(std::collections::HashMap::new()),
            next_work: std::sync::atomic::AtomicU64::new(1),
            stops: Mutex::new(std::collections::VecDeque::new()),
            used_grants: Mutex::new(std::collections::HashMap::new()),
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

    /// SPIKE S-cancel: **the memory-reclamation hook** — forget every `call:` scope whose mandate's window has passed:
    /// its checkpoints and its revoked set. Kept until the window ends, not until the work ends, so a revoked `call:`
    /// mandate presented again with its first checkpoint still reads revoked (a replay inside the window). Returns how
    /// many scopes were reclaimed.
    pub fn reclaim_calls(&self, now_ms: u64) -> usize {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        let expired: Vec<_> = st.call_windows.iter().filter(|(_, until)| **until < now_ms).map(|(k, _)| k.clone()).collect();
        for k in &expired {
            st.revocations.forget(&k.0, &k.1);
            st.call_windows.remove(k);
        }
        expired.len()
    }

    /// SPIKE S-cancel: the view's sizes — `(newest checkpoints, revoked sets, call windows)`.
    pub fn view_sizes(&self) -> (usize, usize, usize) {
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (a, b) = st.revocations.sizes();
        (a, b, st.call_windows.len())
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
        self.reclaim_calls(now_ms);
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

    /// Offer a revocation checkpoint, at `now_ms`.
    pub fn offer_checkpoint(
        &self,
        signed: &SignedRevocationCheckpoint,
        now_ms: u64,
        members: &impl MemberKeySource,
    ) -> CheckpointOffer {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        let (policy, clock) = (st.gate.freshness(), st.gate.clock());
        st.revocations.offer(signed, &policy, clock, now_ms, members, &self.external)
    }

    /// Install a later epoch at the protected resource. Never goes backwards.
    pub fn install_epoch(&self, epoch: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).gate.install_epoch(epoch)
    }

    /// **Assess one call.** `presented` is `params._meta.mandate`, if any; `caller` is the principal
    /// the auth layer resolved.
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
        let Some(p) = presented else { return Assessment { binding: None, valid_until_ms: None } };
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
            return unknown(format!("the grant's holder {} is not the caller {caller}", m.holder.as_str()));
        }
        let Ok(proof) = base64::engine::general_purpose::STANDARD.decode(p.possession.as_bytes()) else {
            return unknown("the possession proof is not base64".into());
        };
        let request = possession_request(operation, resource, arguments_digest);
        // Adopter-surface X3: a grant's arguments binding is checked first, so a grant presented for other arguments
        // is refused **by name** rather than as a possession mismatch (the proof covers this call's digest, which the
        // bound one is not). Refusing on an as-yet-unverified binding is fail-closed: nothing is granted by it.
        let grant_binding = m.grant_binding();
        if let Some(b) = &grant_binding {
            let presented = hex32(arguments_digest);
            if b.arguments_digest != presented {
                return refused(MandateRefusal::BoundToOtherArguments { bound: b.arguments_digest.clone(), presented });
            }
        }

        // SPIKE S-cancel: a `call:` mandate's first checkpoint, carried beside it, offered before the standing is read —
        // only for the mandate's own authority and scope, so a carried checkpoint can speak for nothing else.
        if m.is_call()
            && let Some(cp) = &p.checkpoint
            && cp.checkpoint.authority == m.established_by
            && cp.checkpoint.scope == m.scope
            && !self.holds_checkpoint(&cp.checkpoint.authority, &cp.checkpoint.scope, cp.checkpoint.seq)
        {
            let _ = self.offer_checkpoint(cp, now_ms, members);
        }

        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        match st.verifier.check(&p.grant, &request, Some(&proof), now_ms, members, &self.external) {
            GrantVerdict::Valid => {}
            GrantVerdict::Superseded { held } => {
                return refused(MandateRefusal::Superseded { installed: held, presented: m.epoch });
            }
            GrantVerdict::OutOfWindow => {
                return refused(MandateRefusal::OutOfWindow { now_ms, valid_until_ms: m.valid_until_ms });
            }
            other => return unknown(format!("grant not established: {other:?}")),
        }

        // Adopter-surface X3: a grant is one appointment for one call. Its scope is its own, so the resource gate's
        // epoch rule does not apply to it; what does: the binding its issuer signed (the arguments digest, the policy
        // revision — the latter checked by the preflight once the decision's revision is known), the window, and the
        // revocation standing of its term under the gate's freshness rule.
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
                            .with_facts(Some(digest_hex), binding.policy_revision),
                    ),
                    valid_until_ms: Some(m.valid_until_ms),
                },
                RevocationStanding::Revoked => unknown(GRANT_REVOKED.into()),
                RevocationStanding::Unknown => unknown("the grant's issuer's checkpoint has not reached this verifier".into()),
            };
        }

        // SPIKE S-cancel: a `call:` mandate is judged as a grant is — its window and its issuer's checkpoint for its own
        // scope — and a revoked term is `Refused`, not `Unknown` (D2).
        if m.is_call() {
            if !m.permits(&mandate_operation(operation, resource)) {
                return refused(MandateRefusal::NotEnumerated { operation: mandate_operation(operation, resource) });
            }
            return match st.revocations.grant_standing(&m.established_by, &m.scope, &m.term) {
                RevocationStanding::NotRevoked => {
                    st.call_windows.insert((m.established_by.clone(), m.scope.clone()), m.valid_until_ms);
                    Assessment {
                        binding: Some(
                            MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)
                                .with_facts(Some(digest_hex), None),
                        ),
                        valid_until_ms: Some(m.valid_until_ms),
                    }
                }
                RevocationStanding::Revoked => {
                    st.call_windows.insert((m.established_by.clone(), m.scope.clone()), m.valid_until_ms);
                    refused(MandateRefusal::Revoked { term: m.term.as_str().to_string() })
                }
                RevocationStanding::Unknown => unknown("the call's checkpoint has not reached this verifier".into()),
            };
        }

        let work = AuthorizedWork::new(mandate_operation(operation, resource), Some(m.clone()), u64::MAX);
        match st.gate.check(&work, &st.revocations, now_ms) {
            Ok(()) => Assessment {
                binding: Some(
                    MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)
                        .with_facts(Some(digest_hex), None),
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
        PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof), checkpoint: None }
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
            PresentedMandate { grant: SignedMandateGrant { mandate: m, signature }, possession: base64::engine::general_purpose::STANDARD.encode(possession), checkpoint: None }
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
        let g1_other = PresentedMandate { grant: g1.grant.clone(), possession: base64::engine::general_purpose::STANDARD.encode(possession), checkpoint: None };
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

/// SPIKE S-cancel (identity-and-lifecycle wave 1b): throwaway measurements, never merged. Run with
/// `cargo test --lib --features tls,compliance,a2a spike_s_cancel -- --nocapture --test-threads=1`.
#[cfg(test)]
mod spike_s_cancel {
    use super::*;
    use super::tests::{external, keys, CALLER, OP, RES};
    use crate::agent::action_evaluator::MandateState;
    use crate::knowledge::issuer::MemberKeys;
    use crate::mandate::authority::{ClockModel, Continuation, DrainBound, FreshnessPolicy, ResourceTier, RevocationCheckpoint, StopContract};
    use crate::mandate::grant::{possession_message, EntitlementTable, CALL_SCOPE_PREFIX};
    use crate::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Instant;
    use base64::Engine as _;
    use crate::mandate::grant::GRANT_SCOPE_PREFIX;

    const ISSUER: &str = "operator:acme";
    const DIGEST: [u8; 32] = [7u8; 32];

    fn none() -> HashMap<NodeId, MemberKeys> { HashMap::new() }

    /// A provider's authority entitling `call:` scopes from the issuer — an empty revocation view.
    fn provider(ak: &SigningKey, hk: &SigningKey) -> Arc<ExecutionAuthority> {
        let mut t = EntitlementTable::new();
        t.entitle(CALL_SCOPE_PREFIX, PrincipalId::new(ISSUER).unwrap());
        t.entitle_grants(PrincipalId::new(ISSUER).unwrap());
        let gate = ExecutionGate::strict(
            ResourceAuthority::new("depot", 1), ResourceTier::Serialised, ClockModel { skew_ms: 100 },
            FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
        ).unwrap();
        Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(t), external(ak, hk)))
    }

    fn checkpoint(ak: &SigningKey, scope: &str, seq: u64, at: u64, revoked: &[&str]) -> SignedRevocationCheckpoint {
        let c = RevocationCheckpoint {
            authority: PrincipalId::new(ISSUER).unwrap(), scope: scope.into(), seq, issued_at_ms: at,
            revoked: revoked.iter().map(|t| TermId::new(t).unwrap()).collect(),
        };
        SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(ak, &c.canonical_bytes()).to_vec(), checkpoint: c }
    }

    /// What the admitting gateway mints for one call: the `call:` mandate, its possession proof and — when `carry` —
    /// its first checkpoint (seq 1, nothing revoked) beside it.
    fn mint(ak: &SigningKey, hk: &SigningKey, id: u64, at: u64, until: u64, carry: bool) -> PresentedMandate {
        let scope = format!("{CALL_SCOPE_PREFIX}{id}");
        let m = Mandate {
            holder: PrincipalId::new(CALLER).unwrap(), established_by: PrincipalId::new(ISSUER).unwrap(),
            purpose: format!("call {id} admitted at the gateway"), scope: scope.clone(),
            operations: vec![mandate_operation(OP, RES)], epoch: 1, term: TermId::new(&format!("c{id}")).unwrap(),
            valid_from_ms: at, valid_until_ms: until,
        };
        let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(ak, &m.canonical_bytes()).to_vec(), mandate: m };
        let proof = mycelium_core::tls::sign_bytes(hk, &possession_message(&grant.mandate, &possession_request(OP, RES, &DIGEST)));
        PresentedMandate {
            grant, possession: base64::engine::general_purpose::STANDARD.encode(proof),
            checkpoint: carry.then(|| checkpoint(ak, &scope, 1, at, &[])),
        }
    }

    fn state(a: &Assessment) -> Option<MandateState> { a.binding.as_ref().map(|b| b.state.clone()) }

    fn pct(mut v: Vec<f64>) -> (f64, f64, f64) {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        (mean, v[v.len() / 2], v[v.len() * 99 / 100])
    }

    fn rss_kib() -> u64 {
        let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
    }

    /// **Q1: a `call:` mandate is established at a provider that has never seen its checkpoint, and its work is
    /// registered with `begin`, with no KV read.** Without the carried checkpoint the same mandate is `Unknown`.
    #[test]
    fn s1_established_from_the_carried_checkpoint_and_registered() {
        let (ak, hk) = keys();
        let p = provider(&ak, &hk);
        let bare = mint(&ak, &hk, 1, 1_000, 60_000, false);
        let a = p.assess(CALLER, OP, RES, &DIGEST, Some(&bare), 2_000, &none());
        assert!(matches!(state(&a), Some(MandateState::Unknown(ref why)) if why.contains("checkpoint")), "{a:?}");
        let carried = mint(&ak, &hk, 2, 1_000, 60_000, true);
        let a = p.assess(CALLER, OP, RES, &DIGEST, Some(&carried), 2_000, &none());
        assert_eq!(state(&a), Some(MandateState::Established), "{a:?}");
        let guard = p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(carried.grant.mandate.clone()), 60_000), Arc::new(|| 2_000));
        assert_eq!(p.running_count(), 1);
        // A checkpoint carried for another scope speaks for nothing: the bare mandate is still unknown.
        let mut forged = mint(&ak, &hk, 3, 1_000, 60_000, false);
        forged.checkpoint = Some(checkpoint(&ak, "call:999", 1, 1_000, &[]));
        assert!(matches!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&forged), 2_000, &none())), Some(MandateState::Unknown(_))));
        drop(guard);
        assert_eq!(p.running_count(), 0);
        println!("S1: bare=Unknown, carried=Established, begin registered 1, cross-scope checkpoint ignored");
    }

    /// **Q2: the bytes the checkpoint adds, and the admission latency against today's grant path.**
    #[test]
    fn s2_bytes_and_latency() {
        let (ak, hk) = keys();
        let bare = mint(&ak, &hk, 10, 1_000, 60_000, false);
        let carried = mint(&ak, &hk, 10, 1_000, 60_000, true);
        let b0 = serde_json::to_vec(&bare).unwrap().len();
        let b1 = serde_json::to_vec(&carried).unwrap().len();
        let cp_alone = serde_json::to_vec(carried.checkpoint.as_ref().unwrap()).unwrap().len();
        println!("S2 bytes: presented mandate {b0} B bare, {b1} B carried (+{} B; checkpoint JSON alone {cp_alone} B); envelope cap {} B", b1 - b0, super::super::gateway_caller::MAX_ENVELOPE_BYTES);

        const N: u64 = 20_000;
        // Carried: a fresh call each time — the provider verifies the grant, the possession proof and the checkpoint.
        let p = provider(&ak, &hk);
        let calls: Vec<_> = (0..N).map(|i| mint(&ak, &hk, 100 + i, 1_000, 60_000, true)).collect();
        let mut carried_us = Vec::with_capacity(N as usize);
        for c in &calls {
            let t = Instant::now();
            let a = p.assess(CALLER, OP, RES, &DIGEST, Some(c), 2_000, &none());
            carried_us.push(t.elapsed().as_secs_f64() * 1e6);
            assert_eq!(state(&a), Some(MandateState::Established));
        }
        // Today's grant path, at the provider: the checkpoint read from the KV (here: its bytes, already local) and
        // parsed, offered, then the grant assessed — the same three signatures, plus the parse.
        let q = provider(&ak, &hk);
        let digest_hex = super::super::action_evaluator::hex32(&DIGEST);
        let grants: Vec<(PresentedMandate, Vec<u8>)> = (0..N).map(|i| {
            let request = format!("r{i}");
            let m = Mandate {
                holder: PrincipalId::new(CALLER).unwrap(), established_by: PrincipalId::new(ISSUER).unwrap(),
                purpose: format!("grant issued for request {request}"), scope: format!("{GRANT_SCOPE_PREFIX}{request}"),
                operations: Mandate::grant_operations(mandate_operation(OP, RES), &digest_hex, None), epoch: 1,
                term: TermId::new(&request).unwrap(), valid_from_ms: 1_000, valid_until_ms: 60_000,
            };
            let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&ak, &m.canonical_bytes()).to_vec(), mandate: m };
            let proof = mycelium_core::tls::sign_bytes(&hk, &possession_message(&grant.mandate, &possession_request(OP, RES, &DIGEST)));
            let cp = serde_json::to_vec(&checkpoint(&ak, &grant.mandate.scope, 1, 1_000, &[])).unwrap();
            (PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof), checkpoint: None }, cp)
        }).collect();
        let mut grant_us = Vec::with_capacity(N as usize);
        for (g, cp) in &grants {
            let t = Instant::now();
            let signed: SignedRevocationCheckpoint = serde_json::from_slice(cp).unwrap();
            let _ = q.offer_checkpoint(&signed, 2_000, &none());
            let a = q.assess(CALLER, OP, RES, &DIGEST, Some(g), 2_000, &none());
            grant_us.push(t.elapsed().as_secs_f64() * 1e6);
            assert_eq!(state(&a), Some(MandateState::Established));
        }
        // And the carried path once the checkpoint is held (a second presentation): the verification is skipped.
        let mut held_us = Vec::with_capacity(N as usize);
        for c in &calls {
            let t = Instant::now();
            let _ = p.assess(CALLER, OP, RES, &DIGEST, Some(c), 2_000, &none());
            held_us.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let (cm, c50, c99) = pct(carried_us);
        let (gm, g50, g99) = pct(grant_us);
        let (hm, h50, h99) = pct(held_us);
        println!("S2 latency over {N} calls (µs, mean/p50/p99): carried {cm:.1}/{c50:.1}/{c99:.1}; today's grant path (KV bytes parsed + offered + assessed) {gm:.1}/{g50:.1}/{g99:.1}; carried, checkpoint already held {hm:.1}/{h50:.1}/{h99:.1}");
    }

    /// **Q3: a revoked term reads `Refused`; and the first bound — a term revoked between the gateway's framing and the
    /// provider's admission carries a checkpoint that predates the revocation, so the work is registered and stopped
    /// only when the revoking checkpoint arrives.**
    #[test]
    fn s3_revoked_is_refused_and_the_race_is_stopped_by_the_sweep() {
        let (ak, hk) = keys();
        let p = provider(&ak, &hk);
        // Revoked before admission, the revoking checkpoint already here: Refused by name.
        let c = mint(&ak, &hk, 20, 1_000, 60_000, true);
        let _ = p.offer_checkpoint(&checkpoint(&ak, "call:20", 2, 1_500, &["c20"]), 1_500, &none());
        assert!(matches!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&c), 2_000, &none())),
            Some(MandateState::Refused(MandateRefusal::Revoked { ref term })) if term == "c20"));
        // The race: framed with seq 1, revoked (seq 2) after framing, admitted before seq 2 arrives.
        let r = mint(&ak, &hk, 21, 1_000, 60_000, true);
        assert_eq!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&r), 2_000, &none())), Some(MandateState::Established));
        let g = p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(r.grant.mandate.clone()), 60_000), Arc::new(|| 3_000));
        assert_eq!(p.sweep(2_100), 0, "registered, and nothing has revoked it here yet");
        let _ = p.offer_checkpoint(&checkpoint(&ak, "call:21", 2, 1_800, &["c21"]), 2_200, &none());
        assert_eq!(p.sweep(2_200), 1, "the revoking checkpoint's arrival stops it at the next sweep");
        assert!(g.is_cancelled());
        // The stale carried checkpoint cannot un-revoke: presented again, the call is refused.
        assert!(matches!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&r), 2_300, &none())), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
        drop(g);
        println!("S3: revoked term = Refused(Revoked); a stale carried checkpoint admits, the revoking one stops at the next sweep (interval {} ms), and cannot be un-revoked by re-presenting seq 1", p.sweep_interval_ms());
    }

    /// **The second bound: `drain_bound` is `Unbounded` for the unenforced and unconfirmable classes** — a cancel writes
    /// `Refused` and stops nothing the resource can confirm.
    #[test]
    fn s4_unbounded_classes() {
        let clock = ClockModel { skew_ms: 100 };
        let unconfirmable = StopContract { continuation: Continuation::ReauthorizeAt { interval_ms: 500 }, cancellation_latency_ms: Some(50), confirmation_latency_ms: None };
        let unenforced = StopContract { continuation: Continuation::RunToCompletion { max_duration_ms: 5_000, enforced: false }, cancellation_latency_ms: Some(50), confirmation_latency_ms: Some(10) };
        let bounded = StopContract { continuation: Continuation::ReauthorizeAt { interval_ms: 500 }, cancellation_latency_ms: Some(50), confirmation_latency_ms: Some(10) };
        println!("S4: unconfirmable {:?}; unenforced {:?}; reauthorize-every-500 {:?}",
            unconfirmable.drain_bound(clock, 1_000), unenforced.drain_bound(clock, 1_000), bounded.drain_bound(clock, 1_000));
        assert!(matches!(unconfirmable.drain_bound(clock, 1_000), DrainBound::Unbounded(_)));
        assert!(matches!(unenforced.drain_bound(clock, 1_000), DrainBound::Unbounded(_)));
        assert_eq!(bounded.drain_bound(clock, 1_000), DrainBound::Bounded(660));
    }

    /// **The memory-reclamation hook on 10⁵ calls.** Each call holds a checkpoint and a window entry until its window
    /// passes; the sweep's `reclaim_calls` forgets them. Half are revoked, so the revoked sets are counted too.
    #[test]
    fn s5_reclamation_on_100k_calls() {
        let (ak, hk) = keys();
        let p = provider(&ak, &hk);
        const N: u64 = 100_000;
        let calls: Vec<_> = (0..N).map(|i| mint(&ak, &hk, 1_000_000 + i, 1_000, 10_000, true)).collect();
        let rss0 = rss_kib();
        let t = Instant::now();
        for (i, c) in calls.iter().enumerate() {
            assert_eq!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(c), 2_000, &none())), Some(MandateState::Established));
            let g = p.begin(AuthorizedWork::new(mandate_operation(OP, RES), Some(c.grant.mandate.clone()), 10_000), Arc::new(|| 2_000));
            if i % 2 == 0 {
                let scope = &c.grant.mandate.scope;
                let _ = p.offer_checkpoint(&checkpoint(&ak, scope, 2, 1_500, &[c.grant.mandate.term.as_str()]), 2_000, &none());
            }
            drop(g);
        }
        let admit = t.elapsed();
        let held = p.view_sizes();
        let rss1 = rss_kib();
        assert_eq!(p.running_count(), 0);
        // Every offered checkpoint opens a revoked-set entry for its scope, empty or not: one per call, not one per
        // revoked call.
        assert_eq!(held, (N as usize, N as usize, N as usize));
        // Inside the window nothing is reclaimed: a revoked call re-presented with its first checkpoint stays revoked.
        assert_eq!(p.reclaim_calls(9_999), 0);
        assert!(matches!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&calls[0]), 9_999, &none())), Some(MandateState::Refused(MandateRefusal::Revoked { .. }))));
        let t = Instant::now();
        let reclaimed = p.sweep(10_001);
        let _ = reclaimed;
        let sweep = t.elapsed();
        let after = p.view_sizes();
        let rss2 = rss_kib();
        assert_eq!(after, (0, 0, 0));
        // After the window: re-presented, refused as out of window — reclaiming lost nothing a caller could exploit.
        assert!(matches!(state(&p.assess(CALLER, OP, RES, &DIGEST, Some(&calls[0]), 10_001, &none())), Some(MandateState::Refused(MandateRefusal::OutOfWindow { .. }))));
        println!("S5: {N} calls admitted+registered+dropped in {admit:?} ({:.1} µs/call); held {held:?}; reclaimed by one sweep in {sweep:?}, after {after:?}; RSS KiB before {rss0}, held {rss1} (+{}), after reclaim {rss2}", admit.as_secs_f64() * 1e6 / N as f64, rss1.saturating_sub(rss0));
    }
}
