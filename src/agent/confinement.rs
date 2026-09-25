//! The node's **confinement self-report** (Boundary H item H7) —
//! [`docs/design/confined-fleet.md`](../../../docs/design/confined-fleet.md).
//!
//! # What a node can say about its own confinement, and what it cannot
//!
//! The confined-fleet profile rests on two kinds of control. Some are **node settings** a node can
//! read about itself: an egress allow-list, required identity proofs, an attached audit sink, an
//! action evaluator with an evidence journal. The decisive one is **not**: whether agent pods can
//! reach anything but their gateway is a property of the *network* (separate pods, an enforcing CNI,
//! a NetworkPolicy), and a node cannot observe its own network policy.
//!
//! So [`ConfinementReport`] states the node settings as facts, and states network confinement as
//! [`NetworkConfinement::Unverified`] — always. Deployment evidence for network confinement comes
//! from the reference deployment's test (`scripts/test-confined-fleet.sh`), never from this report.
//! A report that said "confined" would be a node vouching for something it cannot see.
//!
//! A setting this build cannot have (the audit sink without `compliance`; the evaluator and journal
//! without `gateway` + `tls`) is reported as [`Setting::NotInBuild`], never as absent-by-choice.

use serde::Serialize;

use super::GossipAgent;

/// One node-level setting the profile requires.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Setting {
    /// Configured as the profile requires.
    Set,
    /// Not configured.
    Unset,
    /// This build cannot have it: the feature that provides it is not compiled in.
    NotInBuild,
}

impl Setting {
    fn from_bool(b: bool) -> Self {
        if b { Setting::Set } else { Setting::Unset }
    }
}

/// Whether agent pods can reach anything but their gateway. A node cannot observe this.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum NetworkConfinement {
    /// Not verifiable from inside the node. Evidence for it is the reference deployment's test, or
    /// the network's own telemetry — never this report.
    Unverified,
}

/// Whether this node's wall clock is within the bound *s* the authority checks assume (closure plan
/// C11). A node cannot verify its own clock against real time.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum ClockSync {
    /// Not verifiable from inside the node. Evidence for it is the deployment's time-sync monitoring
    /// (NTP or equivalent), never this report.
    Unverified,
}

/// What this node can state about the confined-fleet profile's node-level requirements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfinementReport {
    /// `GossipConfig::egress.allow_hosts` is non-empty, so the substrate's own outbound paths fail
    /// closed (Boundary C). Empty means allow-all.
    pub egress_allow_list: Setting,
    /// `GossipConfig::require_identity_proofs`: an identity entry this node cannot authenticate is
    /// rejected, which the member path of issuer binding (P1) rests on.
    pub identity_proofs_required: Setting,
    /// An external audit sink is attached (WS-C), keeping original bytes of every sealed record.
    pub audit_sink: Setting,
    /// An action evaluator is attached, so gateway dispatches are authorised at the seam.
    pub action_evaluator: Setting,
    /// An evidence journal is attached, so every decision is recorded, fsynced, before dispatch.
    pub evidence_journal: Setting,
    /// Always [`NetworkConfinement::Unverified`].
    pub network_confinement: NetworkConfinement,
    /// Always [`ClockSync::Unverified`] (closure plan C11): every expiry and freshness check assumes
    /// the wall clock is within *s* of real time, and a node cannot vouch for its own clock.
    pub clock_sync: ClockSync,
}

impl ConfinementReport {
    /// The node-level requirements that are not [`Setting::Set`], by name. Empty means every
    /// node-level requirement holds — which is **not** the same as the fleet being confined.
    pub fn unmet(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (name, s) in [
            ("egress_allow_list", self.egress_allow_list),
            ("identity_proofs_required", self.identity_proofs_required),
            ("audit_sink", self.audit_sink),
            ("action_evaluator", self.action_evaluator),
            ("evidence_journal", self.evidence_journal),
        ] {
            if s != Setting::Set {
                out.push(name);
            }
        }
        out
    }
}

impl GossipAgent {
    /// **What this node can state about its own confinement** (Boundary H item H7).
    ///
    /// The node-level settings the confined-fleet profile requires, as facts, and network
    /// confinement as `Unverified` — always, because a node cannot see its own network policy.
    pub fn confinement_report(&self) -> ConfinementReport {
        #[cfg(feature = "compliance")]
        let audit_sink = Setting::from_bool(self.task_ctx.audit_sink.get().is_some());
        #[cfg(not(feature = "compliance"))]
        let audit_sink = Setting::NotInBuild;

        #[cfg(all(feature = "gateway", feature = "tls"))]
        let (action_evaluator, evidence_journal) = (
            Setting::from_bool(self.task_ctx.action_evaluator.get().is_some()),
            Setting::from_bool(self.task_ctx.evidence_journal.get().is_some()),
        );
        #[cfg(not(all(feature = "gateway", feature = "tls")))]
        let (action_evaluator, evidence_journal) = (Setting::NotInBuild, Setting::NotInBuild);

        ConfinementReport {
            egress_allow_list: Setting::from_bool(!self.config.egress.allow_hosts.is_empty()),
            identity_proofs_required: Setting::from_bool(self.config.require_identity_proofs),
            audit_sink,
            action_evaluator,
            evidence_journal,
            network_confinement: NetworkConfinement::Unverified,
            clock_sync: ClockSync::Unverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GossipConfig;
    use crate::node_id::NodeId;

    fn agent(cfg: GossipConfig) -> GossipAgent {
        GossipAgent::new(NodeId::new("127.0.0.1", crate::test_util::alloc_port()).unwrap(), cfg)
    }

    /// A default node meets none of the profile's node settings, and never claims network
    /// confinement.
    #[test]
    fn a_default_node_reports_every_setting_unmet_and_the_network_unverified() {
        let r = agent(GossipConfig::default()).confinement_report();
        assert_eq!(r.egress_allow_list, Setting::Unset);
        assert_eq!(r.identity_proofs_required, Setting::Unset);
        assert_eq!(r.network_confinement, NetworkConfinement::Unverified);
        assert_eq!(r.clock_sync, ClockSync::Unverified, "a node cannot vouch for its own clock");
        assert!(r.unmet().contains(&"egress_allow_list"));
    }

    /// Configured settings are reported as set — and network confinement is **still** unverified:
    /// no node setting can make a node vouch for its network.
    #[test]
    fn configured_settings_are_reported_and_the_network_is_still_unverified() {
        let mut cfg = GossipConfig::default();
        cfg.egress.allow_hosts = vec!["gateway.internal".into()];
        cfg.require_identity_proofs = true;
        let r = agent(cfg).confinement_report();
        assert_eq!(r.egress_allow_list, Setting::Set);
        assert_eq!(r.identity_proofs_required, Setting::Set);
        assert!(!r.unmet().contains(&"egress_allow_list"));
        assert!(!r.unmet().contains(&"identity_proofs_required"));
        assert_eq!(r.network_confinement, NetworkConfinement::Unverified);
    }

    /// A setting the build cannot provide is reported as such, never as a choice not made.
    #[cfg(not(feature = "compliance"))]
    #[test]
    fn a_setting_outside_the_build_is_reported_as_not_in_build() {
        let r = agent(GossipConfig::default()).confinement_report();
        assert_eq!(r.audit_sink, Setting::NotInBuild);
    }
}
