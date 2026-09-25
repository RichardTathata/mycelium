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
}

/// The resource as a grant names it: everything before the resolved provider's `@`.
pub fn resource_key(resource: &str) -> &str {
    resource.split('@').next().unwrap_or(resource)
}

/// The operation a grant must enumerate for this call: `{operation}:{resource_key}`.
pub fn mandate_operation(operation: &str, resource: &str) -> String {
    format!("{operation}:{}", resource_key(resource))
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

struct AuthorityState {
    gate: ExecutionGate,
    verifier: GrantVerifier,
    revocations: RevocationView,
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
            state: Mutex::new(AuthorityState { gate, verifier, revocations: RevocationView::new() }),
            external,
            durable: std::sync::OnceLock::new(),
            running: Mutex::new(std::collections::HashMap::new()),
            next_work: std::sync::atomic::AtomicU64::new(1),
            stops: Mutex::new(std::collections::VecDeque::new()),
        }
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
            snapshot.into_iter().filter(|(_, w)| st.gate.check(w, &st.revocations, now_ms).is_err()).map(|(id, _)| id).collect()
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
        let unknown = |why: String| Assessment {
            binding: Some(MandateBinding::unknown(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, why)),
            valid_until_ms: Some(m.valid_until_ms),
        };
        let refused = |r: MandateRefusal| Assessment {
            binding: Some(MandateBinding::refused(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch, r)),
            valid_until_ms: Some(m.valid_until_ms),
        };

        if m.holder.as_str() != caller {
            return unknown(format!("the grant's holder {} is not the caller {caller}", m.holder.as_str()));
        }
        let Ok(proof) = base64::engine::general_purpose::STANDARD.decode(p.possession.as_bytes()) else {
            return unknown("the possession proof is not base64".into());
        };
        let request = possession_request(operation, resource, arguments_digest);

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

        let work = AuthorizedWork::new(mandate_operation(operation, resource), Some(m.clone()), u64::MAX);
        match st.gate.check(&work, &st.revocations, now_ms) {
            Ok(()) => Assessment {
                binding: Some(MandateBinding::established(m.holder.clone(), m.term.clone(), m.scope.clone(), m.epoch)),
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
}
