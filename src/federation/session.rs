//! Partition and reconnect (item 2 PR 6).
//!
//! §10 of `docs/design/federated-domains.md` states the cycle exactly, and this is that sentence as
//! a state machine:
//!
//! > **On disconnect:** discovery **expires**, calls fail **explicitly**, and authority already
//! > issued lasts only to its stated expiry. Nothing is silently extended.
//! >
//! > **On reconnect:** discovery refreshes **before** new work is admitted, and **the meshes never
//! > merge**.
//!
//! # Why reconnect has a state of its own
//!
//! The tempting shape is two states — down and up — with discovery refreshed "soon after"
//! reconnecting. That shape has a window in which the link is up and the catalogue is stale, and
//! work admitted in that window is dispatched against a view of the partner that predates the
//! partition. Whatever changed while the link was down — a withdrawn export, a revoked grant,
//! a rotated key — is exactly what the caller would be acting on.
//!
//! So [`LinkState::Refreshing`] exists, and work is **refused** in it. Not queued: a queue would
//! deliver the same stale-view calls a moment later, having also hidden the reason.
//!
//! # What this module is and is not
//!
//! A state machine, not a transport. It decides *whether work may be admitted*; connecting,
//! fetching a catalogue and carrying a call are the transport's job, and arrive with it.
//!
//! **The "never merge" half of §10 is not enforced here and cannot be** — a link state cannot
//! prevent two meshes joining. That invariant lives where it can be observed: the two-mesh harness
//! (`lib_tests::two_meshes_never_learn_each_other`), which asserts it from membership tables and the
//! native namespaces, and which re-runs unchanged once the transport exists.

use super::DomainId;

/// Where a link to one partner stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// Not connected. Discovery is gone, not merely old.
    Down,
    /// Connected, and the catalogue has **not** been refreshed since. No new work.
    Refreshing,
    /// Connected with current discovery. Work admitted.
    Ready,
}

/// Why work was not admitted.
///
/// `Down` and `Refreshing` stay distinct because they are different operator situations: *"the
/// partner is unreachable"* and *"the partner is reachable and we are not ready to trust what we
/// know about them yet"*. Collapsing them would hide a reconnect that never completes — which looks
/// exactly like a healthy link that happens to be refusing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The link is down.
    Down {
        /// Which partner.
        partner: DomainId,
    },
    /// Connected, but discovery has not refreshed since the link came back.
    Refreshing {
        /// Which partner.
        partner: DomainId,
    },
    /// Trust in this partner has been revoked. Terminal until an operator says otherwise.
    Revoked {
        /// Which partner.
        partner: DomainId,
    },
}

impl std::fmt::Display for LinkRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Down { partner } => write!(f, "link to {partner} is down"),
            Self::Refreshing { partner } => write!(f, "link to {partner} is reconnecting; discovery has not refreshed"),
            Self::Revoked { partner } => write!(f, "trust in {partner} is revoked"),
        }
    }
}

impl std::error::Error for LinkRefusal {}

/// One partner's link.
#[derive(Clone, Debug)]
pub struct PartnerLink {
    partner: DomainId,
    state: LinkState,
    revoked: bool,
}

impl PartnerLink {
    /// A link that has not yet connected.
    pub fn new(partner: DomainId) -> Self {
        Self { partner, state: LinkState::Down, revoked: false }
    }

    /// Which partner.
    pub fn partner(&self) -> &DomainId {
        &self.partner
    }

    /// Current state.
    pub fn state(&self) -> LinkState {
        self.state
    }

    /// The link dropped.
    ///
    /// Returns `true` if the caller must now **discard its discovery** for this partner. §10 says
    /// discovery *expires* on disconnect; the resolver's own freshness window would get there
    /// eventually, but "eventually" is not what the record says, and the gap is precisely the
    /// interval in which a stale catalogue is most likely to be wrong.
    pub fn disconnected(&mut self) -> bool {
        self.state = LinkState::Down;
        true
    }

    /// The transport reconnected. Work stays refused until discovery refreshes.
    pub fn connected(&mut self) {
        if !self.revoked {
            self.state = LinkState::Refreshing;
        }
    }

    /// Discovery has been refreshed against the reconnected partner.
    ///
    /// Only meaningful from [`LinkState::Refreshing`] — a refresh while `Down` is a refresh against
    /// nothing, and quietly promoting to `Ready` would be the stale-view window arriving by another
    /// route.
    pub fn discovery_refreshed(&mut self) {
        if self.state == LinkState::Refreshing && !self.revoked {
            self.state = LinkState::Ready;
        }
    }

    /// Revoke trust. Terminal: no transition reopens it.
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.state = LinkState::Down;
    }

    /// May new work be admitted to this partner right now?
    pub fn admit(&self) -> Result<(), LinkRefusal> {
        if self.revoked {
            return Err(LinkRefusal::Revoked { partner: self.partner.clone() });
        }
        match self.state {
            LinkState::Ready => Ok(()),
            LinkState::Refreshing => Err(LinkRefusal::Refreshing { partner: self.partner.clone() }),
            LinkState::Down => Err(LinkRefusal::Down { partner: self.partner.clone() }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> PartnerLink {
        PartnerLink::new(DomainId::new("beta.example").unwrap())
    }

    #[test]
    fn a_new_link_admits_nothing() {
        let l = link();
        assert_eq!(l.state(), LinkState::Down);
        assert!(matches!(l.admit(), Err(LinkRefusal::Down { .. })));
    }

    /// **The whole point of `Refreshing`.** Reconnecting is not being ready: work admitted between
    /// the link coming back and discovery refreshing would be dispatched against a view of the
    /// partner that predates the partition.
    #[test]
    fn reconnecting_does_not_admit_work_until_discovery_refreshes() {
        let mut l = link();
        l.connected();
        assert_eq!(l.state(), LinkState::Refreshing);
        assert!(
            matches!(l.admit(), Err(LinkRefusal::Refreshing { .. })),
            "a reconnected link with a stale catalogue must refuse, not proceed"
        );

        l.discovery_refreshed();
        assert_eq!(l.state(), LinkState::Ready);
        assert!(l.admit().is_ok());
    }

    /// Refused, not queued — a queue delivers the same stale-view calls a moment later, having also
    /// hidden the reason.
    #[test]
    fn the_two_not_ready_states_are_told_apart() {
        let mut down = link();
        let mut refreshing = link();
        refreshing.connected();
        assert_ne!(
            down.admit().unwrap_err(),
            refreshing.admit().unwrap_err(),
            "'unreachable' and 'reachable but not yet current' are different operator situations"
        );
        let _ = down.disconnected();
    }

    /// §10: discovery **expires** on disconnect rather than ageing out eventually.
    #[test]
    fn disconnecting_tells_the_caller_to_discard_discovery() {
        let mut l = link();
        l.connected();
        l.discovery_refreshed();
        assert!(l.admit().is_ok());

        assert!(l.disconnected(), "the caller is told to drop what it knew");
        assert!(matches!(l.admit(), Err(LinkRefusal::Down { .. })));
    }

    /// A reconnect must go through `Refreshing` **every time** — not just the first.
    #[test]
    fn every_reconnect_re_enters_refreshing() {
        let mut l = link();
        l.connected();
        l.discovery_refreshed();
        l.disconnected();
        l.connected();
        assert_eq!(l.state(), LinkState::Refreshing, "a second partition is not cheaper than the first");
        assert!(matches!(l.admit(), Err(LinkRefusal::Refreshing { .. })));
    }

    /// A refresh while down is a refresh against nothing, and must not promote the link.
    #[test]
    fn a_refresh_while_down_does_not_open_the_link() {
        let mut l = link();
        l.discovery_refreshed();
        assert_eq!(l.state(), LinkState::Down, "refreshing against a dead link proves nothing");
        assert!(matches!(l.admit(), Err(LinkRefusal::Down { .. })));
    }

    /// Revocation is terminal — reconnecting does not reopen it.
    #[test]
    fn revocation_survives_a_reconnect() {
        let mut l = link();
        l.connected();
        l.discovery_refreshed();
        l.revoke();
        assert!(matches!(l.admit(), Err(LinkRefusal::Revoked { .. })));

        l.connected();
        l.discovery_refreshed();
        assert!(
            matches!(l.admit(), Err(LinkRefusal::Revoked { .. })),
            "a revoked partner must not be readmitted by the transport reconnecting"
        );
    }
}
