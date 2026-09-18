//! The scoped-mandate contract (v3 contracts axis, item 5 PR 2).
//!
//! The record is [`docs/design/scoped-mandates.md`](../../docs/design/scoped-mandates.md). This is
//! its contract half: the mandate itself, the check that makes the decisive invariant enforceable,
//! and the three lifecycle events kept apart.
//!
//! # The decisive invariant
//!
//! > **Once the protected resource acknowledges installation of epoch E2, no operation authorized
//! > only under E1 can commit there — even if its holder refreshes the content revision, retries,
//! > reconnects or restarts.**
//!
//! The tail is what gives it teeth: *refreshes, retries, reconnects, restarts* are the four ways a
//! revoked holder ordinarily gets a second chance, and each is a normal, blameless thing for a
//! client to do. [`ResourceAuthority::check`] is where that sentence is enforced.
//!
//! # `MandateSuperseded`, never `Conflict`
//!
//! §2 of the record, and the reason is mechanical rather than aesthetic. The wiki's CAS defeats
//! stale **content**; a former curator who re-reads the fresh content and re-submits **passes it**.
//! Its bytes are current, its authority is not.
//!
//! `Conflict` is the *retry loop's input* — a caller receiving it is supposed to re-read and try
//! again. Classify a stale mandate as `Conflict` and you hand the revoked holder to the exact loop
//! that refreshes content and re-submits, and the write eventually succeeds. **The retry loop would
//! launder the revocation.** So the refusal here is a distinct type that no retry loop consumes.
//!
//! # Why epoch and term identity are separate fields
//!
//! The **epoch** orders authority. The **term** identifies *which appointment* this is. Collapsing
//! them makes "the same holder, reappointed after a gap" indistinguishable from "the appointment
//! never lapsed" — and the three lifecycle events below become unrecordable, because each is about
//! a particular term rather than about the holder.
//!
//! # Where the enforcement lives
//!
//! Not here. §5 of the record puts the check **inside the protected resource's own atomic
//! boundary**, and that landed in PR 3: `mycelium-wiki::mandate_fence` puts a `verify` of the
//! mandate ref inside the same `update-ref --stdin` transaction as the content write, and
//! `--atomic` plus `--force-with-lease` on every push.
//!
//! This module is the contract that enforcement carries. Keeping them separate is why the contract
//! could be got right before a write path depended on it.

pub mod handover;
pub mod lock_audit;
pub mod partition;
pub mod restart;
pub mod scenario_b;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// A principal — a holder, or an authority that establishes one. Opaque here.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PrincipalId(Arc<str>);

impl PrincipalId {
    /// Wrap a principal name; empty is refused.
    pub fn new(s: impl AsRef<str>) -> Option<Self> {
        let s = s.as_ref();
        (!s.is_empty()).then(|| Self(Arc::from(s)))
    }
    /// As a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which appointment this is — **not** the same thing as the epoch.
///
/// Two terms held by the same principal are two appointments. A term id makes "reappointed after a
/// gap" a fact the system can state, rather than something a reader infers from a holder name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TermId(Arc<str>);

impl TermId {
    /// Wrap a term id; empty is refused.
    pub fn new(s: impl AsRef<str>) -> Option<Self> {
        let s = s.as_ref();
        (!s.is_empty()).then(|| Self(Arc::from(s)))
    }
    /// As a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TermId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One appointment: who may do what, under whose authority, for how long.
///
/// Shared by curator, primary and proposer **without shared powers** — the shape is common, the
/// `operations` are not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mandate {
    /// Who holds it.
    pub holder: PrincipalId,
    /// Who established it. A mandate with no establishing authority is an assertion.
    pub established_by: PrincipalId,
    /// Why it exists, in the operator's words.
    pub purpose: String,
    /// What it covers — a group, a scope name.
    pub scope: String,
    /// **Enumerated** operations. Absence is denial; there is no wildcard, because a wildcard is how
    /// an operation nobody reviewed becomes permitted.
    pub operations: Vec<String>,
    /// Orders authority. Monotonic per scope.
    pub epoch: u64,
    /// **Which** appointment. Deliberately separate from `epoch` — see the module docs.
    pub term: TermId,
    /// Valid from, epoch milliseconds.
    pub valid_from_ms: u64,
    /// Valid until, epoch milliseconds.
    pub valid_until_ms: u64,
}

impl Mandate {
    /// Does this mandate enumerate `operation`?
    ///
    /// Absence is denial, with no inheritance and no wildcard.
    pub fn permits(&self, operation: &str) -> bool {
        self.operations.iter().any(|o| o == operation)
    }

    /// Is `now_ms` inside the validity window?
    pub fn is_current(&self, now_ms: u64) -> bool {
        now_ms >= self.valid_from_ms && now_ms <= self.valid_until_ms
    }
}

/// Why a mandated operation was refused.
///
/// **`Superseded` is not `Conflict`** and never becomes one. See the module docs: a retry loop that
/// consumed this would launder a revocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MandateRefusal {
    /// The resource has installed a **later** epoch. Terminal for this attempt: refreshing,
    /// retrying, reconnecting and restarting all leave it superseded.
    Superseded {
        /// What the resource has installed.
        installed: u64,
        /// What the caller presented.
        presented: u64,
    },
    /// The mandate does not enumerate this operation.
    NotEnumerated {
        /// What was asked for.
        operation: String,
    },
    /// Outside the validity window.
    OutOfWindow {
        /// Now, in epoch milliseconds.
        now_ms: u64,
        /// The window's end.
        valid_until_ms: u64,
    },
    /// The mandate is for a different scope than the resource being written.
    WrongScope {
        /// The resource's scope.
        resource: String,
        /// The mandate's scope.
        mandate: String,
    },
}

impl std::fmt::Display for MandateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Superseded { installed, presented } => write!(
                f,
                "mandate superseded: the resource has installed epoch {installed}, this operation \
                 is authorized only under {presented}"
            ),
            Self::NotEnumerated { operation } =>
                write!(f, "the mandate does not enumerate {operation:?}"),
            Self::OutOfWindow { now_ms, valid_until_ms } =>
                write!(f, "outside the mandate's window (now {now_ms}, valid until {valid_until_ms})"),
            Self::WrongScope { resource, mandate } =>
                write!(f, "mandate is for scope {mandate:?}, resource is {resource:?}"),
        }
    }
}

impl std::error::Error for MandateRefusal {}

/// What a protected resource knows about who may write to it.
///
/// The resource holds the installed epoch; a caller presents a mandate. This is the shape the real
/// enforcement point carries — for `GitStore`, inside the same `update-ref` transaction as the
/// content write.
#[derive(Clone, Debug)]
pub struct ResourceAuthority {
    scope: String,
    installed_epoch: u64,
}

impl ResourceAuthority {
    /// A resource guarding `scope`, with `installed_epoch` currently in force.
    pub fn new(scope: impl Into<String>, installed_epoch: u64) -> Self {
        Self { scope: scope.into(), installed_epoch }
    }

    /// The epoch this resource has acknowledged.
    pub fn installed_epoch(&self) -> u64 {
        self.installed_epoch
    }

    /// Install a later epoch. **Never goes backwards** — the decisive invariant is about what
    /// happens *after* an installation, so an installation that could be undone would undo it.
    ///
    /// Returns `false` if `epoch` is not an advance.
    pub fn install(&mut self, epoch: u64) -> bool {
        if epoch > self.installed_epoch {
            self.installed_epoch = epoch;
            true
        } else {
            false
        }
    }

    /// May this mandate perform `operation` here, now?
    ///
    /// **Epoch first.** A superseded mandate is refused as superseded even if it would also have
    /// failed some other check — because *that* is the answer the caller needs, and because a
    /// refusal that named something else would invite a fix that does not help.
    pub fn check(
        &self,
        mandate: &Mandate,
        operation: &str,
        now_ms: u64,
    ) -> Result<(), MandateRefusal> {
        if mandate.epoch < self.installed_epoch {
            return Err(MandateRefusal::Superseded {
                installed: self.installed_epoch,
                presented: mandate.epoch,
            });
        }
        if mandate.scope != self.scope {
            return Err(MandateRefusal::WrongScope {
                resource: self.scope.clone(),
                mandate: mandate.scope.clone(),
            });
        }
        if !mandate.is_current(now_ms) {
            return Err(MandateRefusal::OutOfWindow {
                now_ms,
                valid_until_ms: mandate.valid_until_ms,
            });
        }
        if !mandate.permits(operation) {
            return Err(MandateRefusal::NotEnumerated { operation: operation.to_string() });
        }
        Ok(())
    }
}

/// The three lifecycle events, **recorded separately**.
///
/// They have different consequences and conflating them loses the difference: a term that ran out
/// is not a term that was taken away, and neither is the same as work authorized under the old
/// epoch being voided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleEvent {
    /// The term reached the end of its window.
    RoleExpired {
        /// Which appointment ended.
        term: TermId,
        /// When.
        at_ms: u64,
    },
    /// The establishing authority withdrew it before its window ended.
    PermissionWithdrawn {
        /// Which appointment was withdrawn.
        term: TermId,
        /// Who withdrew it.
        by: PrincipalId,
        /// When.
        at_ms: u64,
    },
    /// Work authorized under an old epoch is void.
    OutstandingOperationsInvalidated {
        /// The epoch whose authority no longer commits.
        epoch: u64,
        /// When.
        at_ms: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }
    fn tid(s: &str) -> TermId {
        TermId::new(s).unwrap()
    }

    fn mandate(epoch: u64, term: &str) -> Mandate {
        Mandate {
            holder: pid("curator-a"),
            established_by: pid("owner"),
            purpose: "curate the council wiki".into(),
            scope: "council/alpha".into(),
            operations: vec!["apply".into(), "bulk_ingest".into()],
            epoch,
            term: tid(term),
            valid_from_ms: 1_000,
            valid_until_ms: 100_000,
        }
    }

    // ── the decisive invariant ───────────────────────────────────────────────────────────────

    /// **Once the resource has installed E2, nothing authorized only under E1 commits** — and the
    /// four ways a holder ordinarily gets a second chance are exactly the ones that must not work.
    #[test]
    fn a_superseded_mandate_cannot_commit_however_many_times_it_tries() {
        let mut resource = ResourceAuthority::new("council/alpha", 1);
        let old = mandate(1, "term-1");
        assert!(resource.check(&old, "apply", 5_000).is_ok(), "current while it is current");

        resource.install(2);

        // Refresh, retry, reconnect, restart — all the same answer.
        for attempt in 1..=4 {
            assert_eq!(
                resource.check(&old, "apply", 5_000 + attempt),
                Err(MandateRefusal::Superseded { installed: 2, presented: 1 }),
                "attempt {attempt} must still be superseded"
            );
        }
    }

    /// **`Superseded` is its own refusal and never a `Conflict`.** A retry loop that consumed it
    /// would hand a revoked holder to the loop that refreshes content and re-submits.
    #[test]
    fn supersession_is_not_a_retryable_conflict() {
        let resource = ResourceAuthority::new("council/alpha", 5);
        let err = resource.check(&mandate(1, "term-1"), "apply", 5_000).unwrap_err();
        match err {
            MandateRefusal::Superseded { installed, presented } => {
                assert_eq!((installed, presented), (5, 1), "and it names both epochs");
            }
            other => panic!("a stale mandate must be Superseded, not {other:?}"),
        }
    }

    /// Epoch is checked **first**: a mandate that is both superseded and, say, out of window is
    /// reported as superseded, because that is the answer the caller needs.
    #[test]
    fn supersession_is_reported_before_any_other_failing_check() {
        let resource = ResourceAuthority::new("council/alpha", 9);
        let mut stale = mandate(1, "term-1");
        stale.valid_until_ms = 10; // also long expired
        stale.operations.clear(); // also enumerates nothing
        assert!(matches!(
            resource.check(&stale, "apply", 50_000),
            Err(MandateRefusal::Superseded { .. })
        ));
    }

    /// An installation never goes backwards — the invariant is about what happens *after* one, so
    /// an installation that could be undone would undo it.
    #[test]
    fn an_installed_epoch_never_goes_backwards() {
        let mut r = ResourceAuthority::new("council/alpha", 5);
        assert!(!r.install(4), "an earlier epoch is not an installation");
        assert!(!r.install(5), "nor is the same one");
        assert_eq!(r.installed_epoch(), 5);
        assert!(r.install(6));
        assert_eq!(r.installed_epoch(), 6);
    }

    // ── the contract ─────────────────────────────────────────────────────────────────────────

    /// Absence is denial: no wildcard, no inheritance.
    #[test]
    fn operations_are_enumerated_and_absence_is_denial() {
        let r = ResourceAuthority::new("council/alpha", 1);
        let m = mandate(1, "term-1");
        assert!(r.check(&m, "apply", 5_000).is_ok());
        assert_eq!(
            r.check(&m, "erase", 5_000),
            Err(MandateRefusal::NotEnumerated { operation: "erase".into() })
        );
        assert!(!m.permits("*"), "there is no wildcard to match");
    }

    #[test]
    fn a_mandate_for_another_scope_does_not_apply_here() {
        let r = ResourceAuthority::new("council/beta", 1);
        assert_eq!(
            r.check(&mandate(1, "term-1"), "apply", 5_000),
            Err(MandateRefusal::WrongScope {
                resource: "council/beta".into(),
                mandate: "council/alpha".into(),
            })
        );
    }

    #[test]
    fn a_mandate_outside_its_window_is_refused_with_the_deadline() {
        let r = ResourceAuthority::new("council/alpha", 1);
        assert_eq!(
            r.check(&mandate(1, "term-1"), "apply", 200_000),
            Err(MandateRefusal::OutOfWindow { now_ms: 200_000, valid_until_ms: 100_000 })
        );
    }

    // ── epoch vs term ────────────────────────────────────────────────────────────────────────

    /// **The reason they are two fields.** The same holder, reappointed after a gap, is a different
    /// *term* at a different *epoch* — and without the term id, "reappointed" and "never lapsed"
    /// would be the same state.
    #[test]
    fn the_same_holder_reappointed_is_a_different_term() {
        let first = mandate(1, "term-1");
        let second = mandate(3, "term-2");
        assert_eq!(first.holder, second.holder, "same person");
        assert_ne!(first.term, second.term, "different appointment");
        assert_ne!(first.epoch, second.epoch, "and a later authority");
    }

    /// The three lifecycle events stay distinct, because they mean different things.
    #[test]
    fn the_three_lifecycle_events_are_not_interchangeable() {
        let expired = LifecycleEvent::RoleExpired { term: tid("term-1"), at_ms: 100_000 };
        let withdrawn = LifecycleEvent::PermissionWithdrawn {
            term: tid("term-1"),
            by: pid("owner"),
            at_ms: 50_000,
        };
        let voided = LifecycleEvent::OutstandingOperationsInvalidated { epoch: 1, at_ms: 50_000 };

        assert_ne!(expired, withdrawn, "a term that ran out is not one that was taken away");
        assert_ne!(withdrawn, voided, "withdrawing authority is not voiding work done under it");
        assert_ne!(expired, voided);
    }

    #[test]
    fn a_principal_and_a_term_must_be_named() {
        assert_eq!(PrincipalId::new(""), None);
        assert_eq!(TermId::new(""), None);
    }
}
