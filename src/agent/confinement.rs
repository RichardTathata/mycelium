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
    /// **The fleet CA's private key is not on this node** (closure plan C5). A node holding it can
    /// mint itself a new identity after being removed, so removal would only remove a name. `Set`
    /// when TLS is configured and no CA key is in this node's certificate directory; `Unset` when
    /// one is (the `auto_cert_dir` development default) or TLS is off.
    pub ca_key_off_node: Setting,
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
            ("ca_key_off_node", self.ca_key_off_node),
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
        // A view over the guarantee registry (plan I2): each setting is one guarantee, resolved
        // **ignoring the node's role** — this profile is about a node that fronts agents, so a
        // gateway guarantee that would read `NotApplicable` on a node with no `http_port` is read
        // here as the setting it is. `Enforced` is `Set`; `NotInBuild` is `NotInBuild`; anything
        // else is `Unset`.
        let setting = |id: &str| -> Setting {
            match super::guarantee::resolve_ignoring_role(&self.task_ctx, id) {
                Some(super::guarantee::Resolution::Enforced) => Setting::Set,
                Some(super::guarantee::Resolution::NotInBuild { .. }) => Setting::NotInBuild,
                _ => Setting::Unset,
            }
        };
        ConfinementReport {
            egress_allow_list: setting("egress.allow_list"),
            identity_proofs_required: setting("id.proofs_required"),
            audit_sink: setting("audit.sink"),
            action_evaluator: setting("ae.authorised_at_seam"),
            evidence_journal: setting("ae.recorded_before_dispatch"),
            ca_key_off_node: setting("id.ca_key_off_node"),
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
        #[cfg(feature = "tls")]
        assert_eq!(r.identity_proofs_required, Setting::Unset);
        #[cfg(not(feature = "tls"))]
        assert_eq!(r.identity_proofs_required, Setting::NotInBuild, "inert without `tls`, and the report says so");
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
        // Proofs ride on the TLS identity: the flag is inert without `[tls]`, and since the guarantee
        // registry (plan I2) the report says so — `Unset` until TLS is configured too.
        assert_eq!(agent(cfg.clone()).confinement_report().identity_proofs_required, Setting::Unset, "inert without [tls]");
        #[cfg(feature = "tls")]
        {
            cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: std::env::temp_dir().join(format!("confine-p-{}", crate::test_util::alloc_port())), ..Default::default() });
        }
        let r = agent(cfg).confinement_report();
        assert_eq!(r.egress_allow_list, Setting::Set);
        #[cfg(feature = "tls")]
        assert_eq!(r.identity_proofs_required, Setting::Set);
        assert!(!r.unmet().contains(&"egress_allow_list"));
        assert!(!r.unmet().contains(&"identity_proofs_required"));
        assert_eq!(r.network_confinement, NetworkConfinement::Unverified);
    }

    /// **Closure plan C5:** a node that holds the fleet CA's private key could mint itself a new
    /// identity after being removed, so it is reported unmet; without it the setting holds.
    #[test]
    fn a_ca_key_on_the_node_is_an_unmet_setting() {
        let dir = std::env::temp_dir().join(format!("confine-ca-{}", crate::test_util::alloc_port()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = GossipConfig::default();
        cfg.tls = Some(crate::config::TlsConfig { auto_cert_dir: dir.clone(), ..Default::default() });
        // An empty directory is not "the key is off the node": start() mints a CA here, key included,
        // and the report says so before it happens (the guarantee registry, 2026-10-03).
        assert_eq!(agent(cfg.clone()).confinement_report().ca_key_off_node, Setting::Unset, "a CA will be minted here");
        std::fs::write(dir.join("ca-cert.pem"), b"-----BEGIN CERTIFICATE-----").unwrap();
        assert_eq!(agent(cfg.clone()).confinement_report().ca_key_off_node, Setting::Unset,
            "a CA cert alone is not enough: without a pre-issued node cert, start() needs the CA key to sign one");
        cfg.tls.as_mut().unwrap().cert_pem = Some(dir.join("node.cert.pem"));
        cfg.tls.as_mut().unwrap().key_pem = Some(dir.join("node.key.pem"));
        let r = agent(cfg.clone()).confinement_report();
        assert_eq!(r.ca_key_off_node, Setting::Set, "a provisioned CA cert, a pre-issued node cert, and no key here");
        std::fs::write(dir.join("ca-key.pem"), b"-----BEGIN PRIVATE KEY-----").unwrap();
        let r = agent(cfg).confinement_report();
        assert_eq!(r.ca_key_off_node, Setting::Unset);
        assert!(r.unmet().contains(&"ca_key_off_node"));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(agent(GossipConfig::default()).confinement_report().ca_key_off_node, Setting::Unset, "no TLS, no removal");
    }

    /// A setting the build cannot provide is reported as such, never as a choice not made.
    #[cfg(not(feature = "compliance"))]
    #[test]
    fn a_setting_outside_the_build_is_reported_as_not_in_build() {
        let r = agent(GossipConfig::default()).confinement_report();
        assert_eq!(r.audit_sink, Setting::NotInBuild);
    }
}
