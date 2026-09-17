//! Fail-closed authority restart, and durable proposals (v3 item 5 PR 5).
//!
//! Two lines from §4 and §7 of
//! [`docs/design/scoped-mandates.md`](../../../docs/design/scoped-mandates.md):
//!
//! > **Fail-closed authority restart.**
//!
//! > **Durable proposals via the existing log verb** (`KvHandle::append` →
//! > `log/wiki/{group}/proposals`) plus item 1's receipts — not a service database; the evaporating
//! > queue becomes the discovery hint the plan wants.
//!
//! # What "fail closed" has to mean here, specifically
//!
//! A restarted resource has no installed epoch in memory. There are three things it could do, and
//! two of them are catastrophic:
//!
//! - **Assume `0`.** Every superseded mandate then satisfies `epoch >= installed`, so *every
//!   revoked holder is readmitted at once*. This is the failure mode fail-closed exists to prevent,
//!   and it is the tempting one, because `0` is what a `Default` gives you.
//! - **Assume the last value it remembers.** It remembers nothing; that is what a restart is.
//! - **Refuse until it has re-read its durable epoch.** The only honest option, and the one here.
//!
//! [`RestartGuard`] starts [`AuthorityState::Closed`] and admits nothing. It opens only when
//! [`RestartGuard::established`] is given an epoch read back from durable state — and **cannot be
//! reopened by anything else**, because a guard that could be talked open by an ordinary caller
//! would be a guard in name.
//!
//! This is the same shape as the federation link's `Refreshing` state: *reconnected is not ready,
//! and restarted is not authorised*.
//!
//! # Why proposals use the log verb rather than a database
//!
//! §5 refuses a resource-authoritative service process. A durable proposal queue is exactly where
//! one would sneak back in — it looks like storage, not like a control plane. `KvHandle::append` is
//! already an append-only durable log with item 1's receipts behind it, so the proposals go there
//! and the evaporating KV entry stays what it always effectively was: a **discovery hint**, not a
//! record.

use super::{PrincipalId, ResourceAuthority, TermId};
use serde::{Deserialize, Serialize};

/// Whether a restarted resource knows its own authority yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorityState {
    /// Started, and has not re-read its installed epoch. **Admits nothing.**
    Closed,
    /// The installed epoch has been recovered from durable state.
    Open {
        /// The epoch that was read back.
        installed_epoch: u64,
    },
}

/// A resource has restarted and does not yet know what it may accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthorityNotEstablished {
    /// Which scope is still closed.
    pub scope: String,
}

impl std::fmt::Display for AuthorityNotEstablished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "authority for {:?} is not established: the resource has restarted and has not \
             re-read its installed epoch",
            self.scope
        )
    }
}

impl std::error::Error for AuthorityNotEstablished {}

/// Guards a resource's authority across a restart.
#[derive(Debug)]
pub struct RestartGuard {
    scope: String,
    state: AuthorityState,
    authority: Option<ResourceAuthority>,
}

impl RestartGuard {
    /// A resource that has just started: **closed**, admitting nothing.
    pub fn cold(scope: impl Into<String>) -> Self {
        Self { scope: scope.into(), state: AuthorityState::Closed, authority: None }
    }

    /// Where it stands.
    pub fn state(&self) -> &AuthorityState {
        &self.state
    }

    /// Supply the installed epoch, read back from durable state.
    ///
    /// Idempotent and **monotonic**: a later epoch opens or advances it, an earlier one is ignored.
    /// A guard that could be walked backwards would let a stale durable read undo an installation,
    /// which is the decisive invariant's whole subject.
    pub fn established(&mut self, installed_epoch: u64) {
        match &self.state {
            AuthorityState::Open { installed_epoch: held } if *held >= installed_epoch => {}
            _ => {
                self.state = AuthorityState::Open { installed_epoch };
                self.authority = Some(ResourceAuthority::new(self.scope.clone(), installed_epoch));
            }
        }
    }

    /// The authority to check mandates against — **only** once established.
    pub fn authority(&self) -> Result<&ResourceAuthority, AuthorityNotEstablished> {
        self.authority
            .as_ref()
            .ok_or_else(|| AuthorityNotEstablished { scope: self.scope.clone() })
    }
}

// ── durable proposals ─────────────────────────────────────────────────────────────────────────

/// The append-only log stream a group's proposals go to.
///
/// `log/wiki/{group}/proposals`, under the `LOG_WIKI` prefix reserved at item 5 PR 1. A stream
/// name, not a database.
pub fn proposal_stream(group: &str) -> String {
    format!("wiki/{group}/proposals")
}

/// A durable proposal: an edit a proposer is asking a curator to accept.
///
/// Carries its own identity so the receipt for appending it, and any later reference to it, name
/// the same thing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// Who proposed it.
    pub proposer: PrincipalId,
    /// Under which appointment. A proposal outlives the term that made it, and a reader needs to
    /// know which one that was rather than inferring it from a timestamp.
    pub term: TermId,
    /// Which page or resource.
    pub target: String,
    /// The proposed content.
    pub body: Vec<u8>,
    /// When it was made, in epoch milliseconds.
    pub at_ms: u64,
}

impl Proposal {
    /// The bytes appended to the log.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }

    /// Read one back.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        serde_json::from_slice(bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }

    fn mandate(epoch: u64) -> super::super::Mandate {
        super::super::Mandate {
            holder: pid("curator-a"),
            established_by: pid("owner"),
            purpose: "curate".into(),
            scope: "council/alpha".into(),
            operations: vec!["apply".into()],
            epoch,
            term: TermId::new("term-1").unwrap(),
            valid_from_ms: 0,
            valid_until_ms: u64::MAX,
        }
    }

    // ── fail closed ──────────────────────────────────────────────────────────────────────────

    /// A cold resource admits nothing, and says why.
    #[test]
    fn a_restarted_resource_refuses_until_its_epoch_is_re_established() {
        let g = RestartGuard::cold("council/alpha");
        assert_eq!(g.state(), &AuthorityState::Closed);
        assert_eq!(
            g.authority().unwrap_err(),
            AuthorityNotEstablished { scope: "council/alpha".into() }
        );
    }

    /// **The failure fail-closed exists to prevent.** Had the restart assumed epoch `0` — which is
    /// what a `Default` hands you — every superseded mandate would satisfy `epoch >= installed` and
    /// every revoked holder would be readmitted at once.
    #[test]
    fn assuming_zero_on_restart_would_have_readmitted_every_revoked_holder() {
        // What the guard actually does: refuse.
        let closed = RestartGuard::cold("council/alpha");
        assert!(closed.authority().is_err(), "nothing is admitted before the epoch is known");

        // What assuming zero would have done, shown explicitly so the cost is on the record.
        let would_have_been = ResourceAuthority::new("council/alpha", 0);
        assert!(
            would_have_been.check(&mandate(1), "apply", 1_000).is_ok(),
            "a long-superseded mandate passes against an assumed-zero epoch — which is why the \
             guard refuses instead of defaulting"
        );

        // And once the real epoch is recovered, that same mandate is refused.
        let mut g = RestartGuard::cold("council/alpha");
        g.established(9);
        assert!(g.authority().unwrap().check(&mandate(1), "apply", 1_000).is_err());
    }

    #[test]
    fn establishing_opens_the_guard_at_the_recovered_epoch() {
        let mut g = RestartGuard::cold("council/alpha");
        g.established(7);
        assert_eq!(g.state(), &AuthorityState::Open { installed_epoch: 7 });
        assert_eq!(g.authority().unwrap().installed_epoch(), 7);
    }

    /// Monotonic: a stale durable read must not undo an installation, which is the decisive
    /// invariant's whole subject.
    #[test]
    fn a_later_read_cannot_walk_the_epoch_backwards() {
        let mut g = RestartGuard::cold("council/alpha");
        g.established(7);
        g.established(3);
        assert_eq!(g.authority().unwrap().installed_epoch(), 7, "an earlier read is ignored");
        g.established(8);
        assert_eq!(g.authority().unwrap().installed_epoch(), 8, "a later one advances");
    }

    // ── durable proposals ────────────────────────────────────────────────────────────────────

    /// The stream sits under the prefix reserved at PR 1 — `log/wiki/{group}/…`.
    #[test]
    fn the_proposal_stream_is_under_the_reserved_prefix() {
        let stream = proposal_stream("norfolk");
        assert_eq!(stream, "wiki/norfolk/proposals");
        // `KvHandle::append` writes `log/{stream}/{hlc}`, so the full key lands under LOG_WIKI.
        let full = format!("log/{stream}");
        assert!(
            full.starts_with(mycelium_core::signal::kv_ns::LOG_WIKI),
            "the appended key must be inside the reserved namespace: {full}"
        );
    }

    /// A proposal round-trips, and carries the term that made it — it outlives that term, and a
    /// reader needs to know which one it was rather than inferring from a timestamp.
    #[test]
    fn a_proposal_round_trips_with_its_term() {
        let p = Proposal {
            proposer: pid("member-b"),
            term: TermId::new("term-3").unwrap(),
            target: "pages/harvest.md".into(),
            body: b"# Harvest\n".to_vec(),
            at_ms: 1_789_000_000_000,
        };
        let back = Proposal::decode(&p.encode().unwrap()).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.term.as_str(), "term-3");
    }
}
