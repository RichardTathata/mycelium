//! **Authority at execution for the wiki store** (Boundary H item A1,
//! [`docs/design/authority-at-execution.md`](../../../docs/design/authority-at-execution.md) §7).
//!
//! # The gap this closes
//!
//! The mandate fence checks one thing inside the git transaction: that the appointment ref still
//! holds what this curator was configured with. That stops a **superseded** curator. It does not
//! stop a curator whose mandate has **expired**, or has been **revoked** by a checkpoint, or who
//! has **heard nothing** from its authority for longer than the freshness bound and so cannot know
//! whether it has been revoked. Those curators kept writing: the queue drained, retries retried,
//! and publishes pushed commits made earlier.
//!
//! [`ExecutionGateAuthority`] closes it. It implements the store's
//! [`WriteAuthority`](crate::mandate_fence::WriteAuthority) over Mycelium's own
//! [`ExecutionGate`], so every commit attempt and every push attempt re-runs A1's check at that
//! moment:
//! - the resource's epoch, scope, window and operation ([`ExecutionGate::check`]);
//! - the work's own window, which can never outlast the mandate's;
//! - revocation standing, where **silence is a denial**: no fresh, authority-signed checkpoint
//!   means the write does not happen.
//!
//! # Time is passed in
//!
//! The authority reads no clock. The deployment supplies `now_ms`, as the gateway's
//! `ExecutionAuthority` receives it, so the check replays deterministically and both clock
//! extremes are testable.

use std::sync::{Arc, Mutex};

use mycelium::knowledge::issuer::{MemberKeySource, TrustedExternalIssuers};
use mycelium::mandate::authority::{
    AuthorizedWork, CheckpointOffer, ExecutionDenial, ExecutionGate, RevocationView, SignedRevocationCheckpoint,
};
use mycelium::mandate::Mandate;

use crate::mandate_fence::WriteAuthority;

/// The operation a curator's mandate must enumerate to write to, and publish, the wiki store.
pub const WIKI_WRITE: &str = "wiki.write";

struct State {
    gate: ExecutionGate,
    revocations: RevocationView,
}

/// A1's execution gate as the wiki store's [`WriteAuthority`].
///
/// One per curator term: it carries the mandate the curator was appointed under. A re-appointment
/// is a new term and a new `ExecutionGateAuthority`.
pub struct ExecutionGateAuthority {
    /// Lock-order row 44: a leaf, taken under `GitStore::write_lock` (row 32) when the store asks,
    /// and on its own when a checkpoint or epoch is offered. Never held across I/O.
    state: Mutex<State>,
    work: AuthorizedWork,
    external: TrustedExternalIssuers,
    now_ms: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl ExecutionGateAuthority {
    /// An authority for a curator holding `mandate`, checked by `gate`.
    ///
    /// The mandate must enumerate [`WIKI_WRITE`]. `external` names any configured external issuers
    /// that may sign revocation checkpoints; member authorities verify through the caller's live
    /// member-key view at [`offer_checkpoint`](Self::offer_checkpoint). `now_ms` is the deployment's
    /// clock.
    pub fn new(
        gate: ExecutionGate,
        mandate: Mandate,
        external: TrustedExternalIssuers,
        now_ms: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self {
        let not_after = mandate.valid_until_ms;
        Self {
            state: Mutex::new(State { gate, revocations: RevocationView::new() }),
            work: AuthorizedWork::new(WIKI_WRITE, Some(mandate), not_after),
            external,
            now_ms: Arc::new(now_ms),
        }
    }

    /// Offer a signed revocation checkpoint. Until one is accepted and while it stays fresh, every
    /// write is refused as `RevocationUnknown`.
    pub fn offer_checkpoint(&self, signed: &SignedRevocationCheckpoint, members: &impl MemberKeySource) -> CheckpointOffer {
        let now = (self.now_ms)();
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let st = &mut *st;
        let (policy, clock) = (st.gate.freshness(), st.gate.clock());
        st.revocations.offer(signed, &policy, clock, now, members, &self.external)
    }

    /// Install a later epoch at the store. Never goes backwards. A curator whose mandate carries an
    /// earlier epoch is refused from its next write on.
    pub fn install_epoch(&self, epoch: u64) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).gate.install_epoch(epoch)
    }

    /// May the curator write **now**? The typed form of
    /// [`authorize_write`](WriteAuthority::authorize_write).
    pub fn check(&self) -> Result<(), ExecutionDenial> {
        let now = (self.now_ms)();
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.gate.check(&self.work, &st.revocations, now)
    }
}

impl WriteAuthority for ExecutionGateAuthority {
    fn authorize_write(&self) -> Result<(), String> {
        self.check().map_err(|d| format!("{d:?}"))
    }
}
