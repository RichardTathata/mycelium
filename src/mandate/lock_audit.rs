//! **D4: auditing `LockService` under scenario B** (item 5) — an audit with a verdict, not a feature.
//!
//! The decision register is specific about what D4 asks for:
//!
//! > **D4 (item 5):** *"The lock service's issuance is 'insufficient' for strict mode; build the new
//! > fence"* → **Audit `LockService` under replay scenario B first; no second fence.** *Two fences
//! > beside each other is how guarantees drift.*
//!
//! And **D2 is conditional on the answer**, for a stated reason:
//!
//! > A commit HLC orders epochs; it does not prove its bearer was authorized to establish one.
//!
//! # What `LockService` actually does
//!
//! Read from `ConsensusHandle::distributed_lock` rather than from its summary, because the
//! mechanism is not what the summary suggests:
//!
//! 1. The proposer commits **optimistically** — `cluster_propose` returning `Committed` is *not*
//!    mutually exclusive (#164 bug A). Two proposers can both reach this point.
//! 2. It then waits for convergence and **reads back the authoritative value**. Commit keys are
//!    LWW-resolved by HLC, so exactly one proposal survives.
//! 3. **Only the proposer whose own value survived is handed a `LockGuard`.** Everyone else gets
//!    `ConsistencyError::Superseded` and *never receives a token at all.*
//! 4. The token is the **commit HLC** — monotonic across successive holders — and the resource
//!    rejects a token below the highest it has seen.
//!
//! So the premise "two holders each hold a token and race at the resource" is false for this
//! service. Issuance already filters to one. The fence is not there to arbitrate between
//! *concurrent* holders; it is there for the **stale** one — Kleppmann's case, where a holder's
//! lease lapsed during a pause and its write arrives late.
//!
//! # The verdict, in two parts
//!
//! **1. No second fence.** The resource-side rule `LockService` prescribes is structurally the same
//! fence as [`ResourceAuthority`](super::ResourceAuthority): a monotonic value installed at the
//! resource, with anything below it refused. It already delivers the decisive invariant, and it
//! delivers it for a reason that does not depend on issuance at all — the check is at the resource,
//! so it does not care how many holders believe what. A second mechanism beside it would add nothing
//! to the property that matters, and two fences beside each other is how guarantees drift.
//!
//! **2. What is missing is not a fence, and that is why D2 stays conditional.** Two things a token
//! cannot carry:
//!
//! - **No appointer.** The lock's holder is whoever won an LWW-HLC race among everyone who could
//!   propose to the slot. There is no appointing principal anywhere in the mechanism — nothing
//!   corresponds to [`Mandate::established_by`](super::Mandate). Winning is a fact about timestamps.
//! - **No purpose, scope or enumerated operations.** A token is a `u64`. The refusals
//!   [`WrongScope`](super::MandateRefusal::WrongScope) and
//!   [`NotEnumerated`](super::MandateRefusal::NotEnumerated) are refusals *no token could produce* —
//!   a correctly-fenced holder passes the fence and is still refused by the mandate.
//!
//! That is a **layering** statement, not a duplicated fence: the lock supplies *ordering and
//! exclusion*, the mandate supplies *entitlement*. Which is the honest reading of D2's worry — the
//! gap it names is at establishment, and no amount of fencing reaches it.

#[cfg(test)]
use super::{Mandate, PrincipalId, TermId};

/// A proposer attempting to acquire, and what it would stamp writes with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Acquisition {
    /// A label for the proposer.
    pub proposer: &'static str,
    /// The HLC its optimistic commit carried. The winner's becomes the fencing token.
    pub commit_hlc: u64,
    /// Whether this proposer was the *appointed* one. **The mechanism never sees this** — it is
    /// here only so the audit can ask whether winning happens to track appointment.
    pub appointed: bool,
}

/// What `distributed_lock` hands back: one holder, or nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Granted {
    /// The proposer whose value survived convergence.
    pub proposer: &'static str,
    /// The fencing token — the commit HLC.
    pub token: u64,
    /// Whether the winner was the appointed one.
    pub appointed: bool,
}

/// Model `distributed_lock`'s issuance: optimistic commits, then LWW-HLC convergence, and **only**
/// the proposer whose own value survived is handed a guard.
///
/// This is the step that makes "two holders both hold tokens" unreachable — the losers get
/// `Superseded`, not a lower token.
pub fn converge(proposals: &[Acquisition]) -> Option<Granted> {
    proposals
        .iter()
        .max_by_key(|a| a.commit_hlc)
        .map(|a| Granted { proposer: a.proposer, token: a.commit_hlc, appointed: a.appointed })
}

/// A resource fenced the way `lock_service.rs` prescribes: reject anything below the highest token
/// seen.
#[derive(Debug, Default, Clone)]
pub struct FencedResource {
    highest_seen: u64,
}

impl FencedResource {
    /// A resource that has seen nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// The highest token accepted so far.
    pub fn highest_seen(&self) -> u64 {
        self.highest_seen
    }

    /// Attempt a fenced write. `true` if it lands.
    ///
    /// Exactly the rule the lock service prescribes and nothing more — the fence has no notion of
    /// who was appointed, because a token carries none.
    pub fn write(&mut self, token: u64) -> bool {
        if token < self.highest_seen {
            return false;
        }
        self.highest_seen = token;
        true
    }
}

/// A mandate held by `holder`, for the audit's comparisons.
#[cfg(test)]
fn mandate(holder: &str, epoch: u64, scope: &str, operations: &[&str]) -> Mandate {
    Mandate {
        holder: PrincipalId::new(holder).expect("valid"),
        established_by: PrincipalId::new("owner").expect("valid"),
        purpose: "curate".into(),
        scope: scope.into(),
        operations: operations.iter().map(|o| (*o).to_string()).collect(),
        epoch,
        term: TermId::new(format!("term-{epoch}")).expect("valid"),
        valid_from_ms: 0,
        valid_until_ms: u64::MAX,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mandate::{MandateRefusal, ResourceAuthority};

    const APPOINTED: &str = "curator-a";
    const UNAPPOINTED: &str = "curator-b";

    /// **Issuance already filters to one holder.** The loser does not get a lower token; it gets
    /// nothing. This is the step that makes the "two concurrent holders race at the fence" story
    /// unreachable for `LockService`, and it is why a second fence would be answering a question
    /// that is already answered.
    #[test]
    fn only_the_converged_proposer_comes_away_with_a_token() {
        let granted = converge(&[
            Acquisition { proposer: APPOINTED, commit_hlc: 7, appointed: true },
            Acquisition { proposer: UNAPPOINTED, commit_hlc: 4, appointed: false },
        ])
        .expect("someone wins");

        assert_eq!(granted.proposer, APPOINTED);
        assert_eq!(granted.token, 7);
        // The loser holds no token, so there is nothing for it to stamp a write with.
        assert_eq!(converge(&[]), None, "nobody proposing means nobody holds the lock");
    }

    /// **Part 1 of the verdict: the fence is safe regardless of issuance.**
    ///
    /// Once the resource has seen token T, nothing below T lands — whatever the schedule, whatever
    /// the holders believed. Swept over every interleaving of a small token set, and asserted after
    /// *every* step rather than at the end.
    #[test]
    fn safety_holds_under_every_schedule() {
        let tokens = [1u64, 2, 3];
        let mut schedules = Vec::new();
        for &a in &tokens {
            for &b in &tokens {
                for &c in &tokens {
                    schedules.push(vec![a, b, c]);
                }
            }
        }
        assert_eq!(schedules.len(), 27, "the sweep covers every interleaving");

        let mut refusals = 0usize;
        for schedule in &schedules {
            let mut r = FencedResource::new();
            let mut accepted_high = 0u64;
            for &token in schedule {
                if r.write(token) {
                    assert!(
                        token >= accepted_high,
                        "a write below the highest accepted token landed: {token} in {schedule:?}"
                    );
                    accepted_high = token;
                } else {
                    refusals += 1;
                    assert!(token < accepted_high, "a write was refused for no reason");
                }
            }
        }
        assert!(refusals > 0, "the sweep must actually exercise refusals, saw {refusals}");
    }

    /// **The case the fence actually exists for** — Kleppmann's, and the only one `LockService`'s
    /// issuance does not already exclude: a holder pauses past its lease, the lock is re-acquired,
    /// and the old holder's write arrives late.
    #[test]
    fn the_fence_refuses_the_stale_holders_late_write() {
        let first = converge(&[Acquisition { proposer: APPOINTED, commit_hlc: 10, appointed: true }])
            .expect("acquired");
        let mut resource = FencedResource::new();
        assert!(resource.write(first.token), "the holder writes inside its lease");

        // The lease lapses during a pause; someone else acquires. The HLC is monotonic across
        // successive holders, so the new token is higher.
        let second =
            converge(&[Acquisition { proposer: UNAPPOINTED, commit_hlc: 11, appointed: false }])
                .expect("re-acquired");
        assert!(resource.write(second.token));

        assert!(
            !resource.write(first.token),
            "the paused holder wakes and writes late — the fence refuses it, which is the whole \
             reason the token exists"
        );
        assert_eq!(resource.highest_seen(), 11);
    }

    /// **Part 2a of the verdict: winning is a fact about timestamps, not about appointment.**
    ///
    /// The holder is whoever survived the LWW-HLC race among everyone who could propose to the
    /// slot. Nothing in the mechanism corresponds to an appointing principal, so an unappointed
    /// proposer with the higher commit HLC simply wins.
    ///
    /// This is not a defect in `LockService`, which never claimed to arbitrate entitlement — it is
    /// the precise content of D2's worry, demonstrated instead of asserted.
    #[test]
    fn the_lock_is_won_by_the_hlc_race_not_by_an_appointment() {
        let granted = converge(&[
            Acquisition { proposer: APPOINTED, commit_hlc: 4, appointed: true },
            Acquisition { proposer: UNAPPOINTED, commit_hlc: 9, appointed: false },
        ])
        .expect("someone wins");

        assert_eq!(granted.proposer, UNAPPOINTED);
        assert!(!granted.appointed, "the winner was not the appointed holder");
        assert_eq!(granted.token, 9, "and its token is the higher one, so the fence prefers it");
    }

    /// **Part 2b of the verdict: a token cannot express what a mandate refuses on.**
    ///
    /// A holder that is perfectly fenced — its epoch is the installed one, so the fence admits it —
    /// is still refused by the real `ResourceAuthority` for reasons a `u64` cannot carry: the wrong
    /// scope, and an operation the mandate never enumerated.
    ///
    /// This is the executable form of "the fence is necessary but not sufficient", and it is why
    /// the answer to D2 is a layer, not a second fence.
    #[test]
    fn a_token_cannot_express_what_a_mandate_refuses_on() {
        let authority = ResourceAuthority::new("council/alpha", 5);
        let mut fence = FencedResource::new();
        assert!(fence.write(5), "the fence admits this holder — its token is current");

        // Right epoch, wrong resource.
        let elsewhere = mandate(APPOINTED, 5, "council/beta", &["apply"]);
        assert!(
            matches!(
                authority.check(&elsewhere, "apply", 0),
                Err(MandateRefusal::WrongScope { .. })
            ),
            "a correctly-fenced holder writing to the wrong scope: no token could catch this"
        );

        // Right epoch, right scope, operation never granted.
        let narrow = mandate(APPOINTED, 5, "council/alpha", &["read"]);
        assert!(
            matches!(
                authority.check(&narrow, "apply", 0),
                Err(MandateRefusal::NotEnumerated { .. })
            ),
            "a correctly-fenced holder exceeding its enumerated operations: likewise"
        );

        // And the mandate that is entitled still commits — so this is not a fence that refuses
        // everything, which would satisfy the invariant while being useless.
        let entitled = mandate(APPOINTED, 5, "council/alpha", &["apply"]);
        assert_eq!(authority.check(&entitled, "apply", 0), Ok(()));
    }

    /// Re-presenting the same token is a retry of the same authority, not a regression. Refusing it
    /// would make an idempotent retry look like a fencing violation.
    #[test]
    fn re_presenting_the_same_token_is_a_retry_not_a_regression() {
        let mut r = FencedResource::new();
        assert!(r.write(4));
        assert!(r.write(4), "the same authority, again");
    }
}
