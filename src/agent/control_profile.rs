//! The node's control profile (v3 item 4 PR 4a) — which promises the governors enforce, §7 of
//! `docs/design/adaptive-stability.md`.
//!
//! One profile per node, stored as an atomic so a change needs no lock and no config-struct change
//! (that shape break is scheduled in §6.6, not smuggled in here). The default is `Legacy`: today's
//! governors, untouched, and no governor consults the confidence predicate. **Shadow first** —
//! `Observe` records what an enforcing profile would have held and holds nothing; the count is the
//! number an operator watches before turning enforcement on.

use crate::agent::GossipAgent;
use crate::control::Profile;
use std::sync::atomic::Ordering;

impl GossipAgent {
    /// Set which promises the governors enforce. Takes effect on each governor's next pass.
    pub fn set_control_profile(&self, profile: Profile) {
        self.task_ctx.control_profile.store(profile.as_u8(), Ordering::Relaxed);
    }

    /// The profile in force. `Legacy` unless an operator opted in.
    pub fn control_profile(&self) -> Profile {
        Profile::from_u8(self.task_ctx.control_profile.load(Ordering::Relaxed))
    }

    /// How many governor actions an enforcing profile *would* have held, counted under `Observe`.
    /// Never reset; an operator compares two readings.
    pub fn control_would_hold_count(&self) -> u64 {
        self.task_ctx.control_would_hold.load(Ordering::Relaxed)
    }
}
