//! The rights ledger (item 4 PR 3) — §4 of
//! [`docs/design/adaptive-stability.md`](../../../docs/design/adaptive-stability.md).
//!
//! # What a right is, and why it cannot live in the medium
//!
//! Everything a governor reads today is soft state that **evaporates** — a capability advertisement,
//! a load report, an intent. That is the right shape for an observation: a provider that vanishes
//! should stop being offered. It is the wrong shape for a right. An allocation that vanished with
//! its holder's discovery entry would be **issued twice** — once to the holder that is still, in
//! fact, running, and once to whoever is next — and an LWW medium lets a later writer overwrite one
//! with a bigger claim. So the ledger is a **node-local, append-only, fsynced journal that is never
//! gossiped** ([`crate::agent::journal`], the mechanism the AE evidence journal already uses), and
//! what may leave this node is a bounded signed [`RightsHead`] under `rights/head/{holder}`.
//!
//! # The three rules the type enforces
//!
//! - **Persisted before acting.** Every mutation is appended and fsynced *first*, and applied to the
//!   in-memory view only on an `OnDisk` receipt. A right whose record did not reach disk has not
//!   been allocated, and the view says so.
//! - **Never reclaimed because an owner vanished from discovery.** There is no method on this type
//!   that takes a peer set, a membership table or a heartbeat. A right leaves the ledger by its
//!   holder's release, its term's expiry, or its allocator's revocation — and by nothing else.
//! - **`admission.rejected` is a first-class outcome.** A budget that refuses work records that it
//!   refused work, beside the completions, so a refusal is visible as a refusal and not as silence.
//!
//! # What this PR does not do
//!
//! Reservation and reconciliation — the *reserve → act → reconcile* flow that consumes units
//! against a right and settles them — is PR 4, on this ledger. Publishing the head into the medium
//! is PR 4 too. This is the ledger and the head's shape; a `Right`'s units are capacity, and
//! [`admit`](RightsLedger::admit) checks a request against capacity without consuming it.
//!
//! # Encoding
//!
//! Records and the head go through [`mycelium_core::serde_fixint`] — fixed-width, little-endian,
//! golden-pinned, and already the byte layer the audit chain and consensus signatures are computed
//! over. A record the ledger cannot decode **fails the open**: a ledger it cannot read is not a
//! ledger it can act on, and starting with an empty view over a full file would readmit nothing and
//! reissue everything.

use crate::agent::journal::{Appended, Journal, JournalError};
use crate::mandate::{PrincipalId, TermId};
use mycelium_core::receipt::LocalDurability;
use mycelium_core::serde_fixint;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

/// The replay-seam stream this ledger's journal records under — per destination, and not the AE
/// journal's.
const STREAM: &str = "rights/journal";

/// Where a right's units are. **Every state counts** — a unit in `Installing` is as consumed as one
/// in `Serving`, and `Unknown` is a state, not a zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum RightState {
    /// Being installed.
    Installing,
    /// Installed, not yet serving.
    Warming,
    /// Serving.
    Serving,
    /// Being withdrawn.
    Draining,
    /// The ledger does not know — after a crash mid-transition, or a reconcile that timed out.
    Unknown,
}

/// Every state, for counting.
pub const ALL_STATES: [RightState; 5] = [
    RightState::Installing,
    RightState::Warming,
    RightState::Serving,
    RightState::Draining,
    RightState::Unknown,
];

/// One allocated right — an appointment to consume `units` of `resource`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Right {
    /// Who holds it.
    pub holder: PrincipalId,
    /// What it is a right to. A name; the ledger does not interpret it.
    pub resource: String,
    /// How much — in the resource's **native** units. Never money.
    pub units: u64,
    /// Who allocated it. An allocation without an allocator is an assertion.
    pub allocated_by: PrincipalId,
    /// **Which** appointment this is — item 5's term identity, so "the same holder, re-allocated
    /// after a gap" is distinguishable from "never lapsed".
    pub term: TermId,
    /// Where the units are.
    pub state: RightState,
    /// When the term ends, epoch milliseconds. The ledger records expiry when told; it does not
    /// keep a clock.
    pub valid_until_ms: u64,
}

/// A right's identity in the ledger: one holder, one term.
pub type RightKey = (PrincipalId, TermId);

impl Right {
    fn key(&self) -> RightKey {
        (self.holder.clone(), self.term.clone())
    }
}

/// One journal record. Append-only; the view is a fold over these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerEvent {
    /// A right was allocated.
    Allocated(Right),
    /// A right's units moved between states.
    StateChanged {
        /// Whose.
        holder: PrincipalId,
        /// Which appointment.
        term: TermId,
        /// To where.
        to: RightState,
    },
    /// The holder gave it up.
    Released {
        /// Whose.
        holder: PrincipalId,
        /// Which appointment.
        term: TermId,
    },
    /// The term ended.
    Expired {
        /// Whose.
        holder: PrincipalId,
        /// Which appointment.
        term: TermId,
        /// When, as the caller observed it.
        at_ms: u64,
    },
    /// The allocator withdrew it.
    Revoked {
        /// Whose.
        holder: PrincipalId,
        /// Which appointment.
        term: TermId,
        /// Who withdrew it.
        by: PrincipalId,
    },
    /// Work was refused for want of capacity. **Recorded beside completions**, never silent.
    AdmissionRejected {
        /// Who asked.
        holder: PrincipalId,
        /// For what.
        resource: String,
        /// How much.
        requested: u64,
        /// How much was there.
        available: u64,
    },
}

/// Why the ledger refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LedgerRefusal {
    /// The record did not reach disk, so nothing happened. Carries the journal's own reason — a
    /// full queue, a persistence failure, or *unknown*, which is a different claim from failure.
    NotDurable(JournalError),
    /// This `(holder, term)` is already live. A term is an appointment; allocating it twice is the
    /// double issue the ledger exists to prevent.
    Duplicate {
        /// Whose.
        holder: PrincipalId,
        /// Which.
        term: TermId,
    },
    /// No live right by that key.
    Unknown {
        /// Whose.
        holder: PrincipalId,
        /// Which.
        term: TermId,
    },
    /// The request exceeds the holder's live units for that resource. Already recorded as an
    /// [`AdmissionRejected`](LedgerEvent::AdmissionRejected) event by the time this is returned.
    Rejected {
        /// How much was asked.
        requested: u64,
        /// How much was there.
        available: u64,
    },
    /// The record could not be encoded — a defect, not a runtime condition.
    Encoding(String),
}

impl std::fmt::Display for LedgerRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotDurable(e) => write!(f, "not recorded, so not done: {e}"),
            Self::Duplicate { holder, term } => {
                write!(f, "right {holder}/{term} is already live; a term is allocated once")
            }
            Self::Unknown { holder, term } => write!(f, "no live right {holder}/{term}"),
            Self::Rejected { requested, available } => {
                write!(f, "admission rejected: {requested} requested, {available} available")
            }
            Self::Encoding(e) => write!(f, "could not encode the record: {e}"),
        }
    }
}

/// The in-memory fold of the journal. Derived state only; the journal is the truth.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LedgerView {
    live:       BTreeMap<RightKey, Right>,
    retired:    BTreeSet<RightKey>,
    rejections: u64,
}

impl LedgerView {
    /// A live right, if any.
    pub fn right(&self, holder: &PrincipalId, term: &TermId) -> Option<&Right> {
        self.live.get(&(holder.clone(), term.clone()))
    }

    /// The holder's live units of `resource`, **counted across every state** — installing, warming,
    /// serving, draining and unknown alike.
    pub fn held_units(&self, holder: &PrincipalId, resource: &str) -> u64 {
        self.live
            .values()
            .filter(|r| &r.holder == holder && r.resource == resource)
            .map(|r| r.units)
            .sum()
    }

    /// Live units of `resource` by state, for the holder.
    pub fn units_by_state(&self, holder: &PrincipalId, resource: &str) -> BTreeMap<RightState, u64> {
        let mut out = BTreeMap::new();
        for r in self.live.values().filter(|r| &r.holder == holder && r.resource == resource) {
            *out.entry(r.state).or_insert(0) += r.units;
        }
        out
    }

    /// Rights that were released, expired or revoked — and could be allocated again.
    pub fn retired(&self) -> &BTreeSet<RightKey> {
        &self.retired
    }

    /// How many admissions were refused. Beside the completions, never instead of them.
    pub fn rejections(&self) -> u64 {
        self.rejections
    }

    /// Live units per resource for one holder, sorted by resource — what the head carries.
    fn totals(&self, holder: &PrincipalId) -> Vec<(String, u64)> {
        let mut by: BTreeMap<String, u64> = BTreeMap::new();
        for r in self.live.values().filter(|r| &r.holder == holder) {
            *by.entry(r.resource.clone()).or_insert(0) += r.units;
        }
        by.into_iter().collect()
    }

    fn apply(&mut self, ev: &LedgerEvent) {
        match ev {
            LedgerEvent::Allocated(r) => {
                let key = r.key();
                self.retired.remove(&key);
                self.live.insert(key, r.clone());
            }
            LedgerEvent::StateChanged { holder, term, to } => {
                if let Some(r) = self.live.get_mut(&(holder.clone(), term.clone())) {
                    r.state = *to;
                }
            }
            LedgerEvent::Released { holder, term }
            | LedgerEvent::Expired { holder, term, .. }
            | LedgerEvent::Revoked { holder, term, .. } => {
                let key = (holder.clone(), term.clone());
                if self.live.remove(&key).is_some() {
                    self.retired.insert(key);
                }
            }
            LedgerEvent::AdmissionRejected { .. } => self.rejections += 1,
        }
    }
}

/// **The bounded signed head** — a holder's claim of what it holds, and the only part of the
/// ledger that may enter the medium (`rights/head/{holder}`, reserved at PR 1). Checkable against
/// the allocator's own journal, which is the authority for how many rights exist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RightsHead {
    /// Whose claim.
    pub holder: PrincipalId,
    /// The journal sequence of the last event this head reflects.
    pub seq: u64,
    /// Live units per resource, sorted by resource.
    pub totals: Vec<(String, u64)>,
}

impl RightsHead {
    /// The bytes a signature covers.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_fixint::to_vec(self).unwrap_or_default()
    }

    /// Does `signature` verify over this head under `key`?
    #[cfg(feature = "tls")]
    pub fn verify(&self, key: &[u8; 32], signature: &[u8]) -> bool {
        mycelium_core::tls::verify_bytes(key, &self.canonical_bytes(), signature)
    }
}

/// The ledger: a journal and the view folded from it. **Single-owner** (`&mut self`); how it is
/// shared between a governor and its actuator is PR 4's question, and this type adds no lock.
pub struct RightsLedger {
    journal:  Arc<Journal>,
    view:     LedgerView,
    last_seq: Option<u64>,
}

impl RightsLedger {
    /// Open (or create) the ledger at `path`, folding every record already on disk.
    ///
    /// A record that does not decode is an error, not a skip — see the module docs.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let journal = Journal::open(path, STREAM)?;
        let mut view = LedgerView::default();
        let mut last_seq = None;
        // Fold from where the journal says it lives, not from the argument: the journal is the
        // truth about its own file, and this is the one path every build reads.
        for (i, bytes) in crate::agent::journal::read_journal(journal.path())?.into_iter().enumerate() {
            let ev: LedgerEvent = serde_fixint::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("rights ledger record {i} does not decode: {e:?}"),
                )
            })?;
            view.apply(&ev);
            last_seq = Some(i as u64);
        }
        Ok(Self { journal, view, last_seq })
    }

    /// A ledger over a journal supplied by the test — how a stalled writer is arranged.
    #[cfg(test)]
    pub(crate) fn with_journal(journal: Arc<Journal>) -> Self {
        Self { journal, view: LedgerView::default(), last_seq: None }
    }

    /// The folded view.
    pub fn view(&self) -> &LedgerView {
        &self.view
    }

    /// **Persist, then apply.** The one place the view changes, and it changes only after the
    /// journal says `OnDisk`.
    async fn record(&mut self, ev: LedgerEvent) -> Result<Appended, LedgerRefusal> {
        let bytes = serde_fixint::to_vec(&ev).map_err(|e| LedgerRefusal::Encoding(format!("{e:?}")))?;
        let appended = self.journal.append(bytes).await.map_err(LedgerRefusal::NotDurable)?;
        if appended.durability != LocalDurability::OnDisk {
            return Err(LedgerRefusal::NotDurable(JournalError::Failed(format!(
                "receipt was {:?}, not OnDisk",
                appended.durability
            ))));
        }
        self.view.apply(&ev);
        self.last_seq = Some(appended.seq);
        Ok(appended)
    }

    /// Allocate a right. Refused as [`Duplicate`](LedgerRefusal::Duplicate) if the same
    /// `(holder, term)` is live — a term is allocated once.
    pub async fn allocate(&mut self, right: Right) -> Result<Appended, LedgerRefusal> {
        if self.view.live.contains_key(&right.key()) {
            return Err(LedgerRefusal::Duplicate { holder: right.holder, term: right.term });
        }
        self.record(LedgerEvent::Allocated(right)).await
    }

    /// Move a live right's units to another state.
    pub async fn transition(
        &mut self,
        holder: &PrincipalId,
        term: &TermId,
        to: RightState,
    ) -> Result<Appended, LedgerRefusal> {
        self.require_live(holder, term)?;
        self.record(LedgerEvent::StateChanged { holder: holder.clone(), term: term.clone(), to }).await
    }

    /// The holder gives the right up.
    pub async fn release(&mut self, holder: &PrincipalId, term: &TermId) -> Result<Appended, LedgerRefusal> {
        self.require_live(holder, term)?;
        self.record(LedgerEvent::Released { holder: holder.clone(), term: term.clone() }).await
    }

    /// The term ended, as observed by the caller at `at_ms`.
    pub async fn expire(
        &mut self,
        holder: &PrincipalId,
        term: &TermId,
        at_ms: u64,
    ) -> Result<Appended, LedgerRefusal> {
        self.require_live(holder, term)?;
        self.record(LedgerEvent::Expired { holder: holder.clone(), term: term.clone(), at_ms }).await
    }

    /// The allocator withdraws the right.
    pub async fn revoke(
        &mut self,
        holder: &PrincipalId,
        term: &TermId,
        by: &PrincipalId,
    ) -> Result<Appended, LedgerRefusal> {
        self.require_live(holder, term)?;
        self.record(LedgerEvent::Revoked { holder: holder.clone(), term: term.clone(), by: by.clone() })
            .await
    }

    /// May `holder` be admitted for `requested` units of `resource`? Pure: `Ok(available)` or
    /// `Err(available)`. Consumes nothing — capacity accounting is PR 4.
    pub fn may_admit(&self, holder: &PrincipalId, resource: &str, requested: u64) -> Result<u64, u64> {
        let available = self.view.held_units(holder, resource);
        if requested <= available { Ok(available) } else { Err(available) }
    }

    /// Admit, or **record the rejection** and refuse. A rejection that is not recorded is a silence,
    /// and a silence is indistinguishable from work that was never asked for.
    pub async fn admit(
        &mut self,
        holder: &PrincipalId,
        resource: &str,
        requested: u64,
    ) -> Result<u64, LedgerRefusal> {
        match self.may_admit(holder, resource, requested) {
            Ok(available) => Ok(available),
            Err(available) => {
                self.record(LedgerEvent::AdmissionRejected {
                    holder: holder.clone(),
                    resource: resource.to_string(),
                    requested,
                    available,
                })
                .await?;
                Err(LedgerRefusal::Rejected { requested, available })
            }
        }
    }

    /// The holder's head over the current view.
    pub fn head(&self, holder: &PrincipalId) -> RightsHead {
        RightsHead {
            holder: holder.clone(),
            seq: self.last_seq.unwrap_or(0),
            totals: self.view.totals(holder),
        }
    }

    fn require_live(&self, holder: &PrincipalId, term: &TermId) -> Result<(), LedgerRefusal> {
        if self.view.live.contains_key(&(holder.clone(), term.clone())) {
            Ok(())
        } else {
            Err(LedgerRefusal::Unknown { holder: holder.clone(), term: term.clone() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }
    fn t(s: &str) -> TermId {
        TermId::new(s).unwrap()
    }
    fn right(holder: &str, term: &str, resource: &str, units: u64) -> Right {
        Right {
            holder: p(holder),
            resource: resource.into(),
            units,
            allocated_by: p("allocator"),
            term: t(term),
            state: RightState::Installing,
            valid_until_ms: u64::MAX,
        }
    }
    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("rights-{name}-{}", fastrand::u64(..)));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("rights.log")
    }

    /// **The gate that matters, half one.** A holder that vanishes from discovery keeps its right.
    /// There is no method on this type that takes a peer set, so "vanishing" is not an operation the
    /// ledger can even be told about — and re-allocating the same term, which is what a reissue *is*,
    /// is refused while the right is live.
    #[tokio::test]
    async fn a_right_whose_holder_vanished_from_discovery_is_not_reissued() {
        let mut l = RightsLedger::open(temp("vanish")).unwrap();
        l.allocate(right("a", "term-1", "gpu-slots", 4)).await.expect("allocated");

        // The holder stops being heard. Nothing here observes that; nothing here can.
        assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 4);
        assert_eq!(
            l.allocate(right("a", "term-1", "gpu-slots", 4)).await.unwrap_err(),
            LedgerRefusal::Duplicate { holder: p("a"), term: t("term-1") },
            "the same term cannot be issued twice while it is live"
        );
        assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 4, "and the refused reissue changed nothing");
    }

    /// **Half two.** A right that is released, expired or revoked *is* reissuable — the units are
    /// free, and the term may be allocated again. A ledger that passed only half one would never free
    /// anything.
    #[tokio::test]
    async fn a_released_expired_or_revoked_right_is_reissuable() {
        for (label, retire) in [("release", 0u8), ("expire", 1), ("revoke", 2)] {
            let mut l = RightsLedger::open(temp(label)).unwrap();
            l.allocate(right("a", "term-1", "gpu-slots", 4)).await.expect("allocated");
            match retire {
                0 => l.release(&p("a"), &t("term-1")).await.expect("released"),
                1 => l.expire(&p("a"), &t("term-1"), 9_000).await.expect("expired"),
                _ => l.revoke(&p("a"), &t("term-1"), &p("allocator")).await.expect("revoked"),
            };
            assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 0, "{label}: the units are free");
            assert!(l.view().retired().contains(&(p("a"), t("term-1"))));
            l.allocate(right("a", "term-1", "gpu-slots", 4)).await.expect("reissued after {label}");
            assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 4, "{label}: reissued");
        }
    }

    /// **Persisted before acting.** A record that does not reach disk allocates nothing: the view is
    /// unchanged and the refusal carries the journal's own reason.
    #[tokio::test]
    async fn an_allocation_that_did_not_reach_disk_is_not_an_allocation() {
        let mut l = RightsLedger::with_journal(Journal::stalled("rights/journal"));
        assert_eq!(
            l.allocate(right("a", "term-1", "gpu-slots", 4)).await.unwrap_err(),
            LedgerRefusal::NotDurable(JournalError::DeliveryUnknown)
        );
        assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 0, "nothing was recorded, so nothing was done");
        assert_eq!(l.head(&p("a")).seq, 0);
    }

    /// **`admission.rejected` beside the completions.** An over-request is refused *and recorded*,
    /// and the record survives a reopen.
    #[tokio::test]
    async fn an_over_request_is_recorded_as_a_rejection_beside_completions() {
        let path = temp("reject");
        let mut l = RightsLedger::open(&path).unwrap();
        l.allocate(right("a", "term-1", "gpu-slots", 5)).await.expect("allocated");

        assert_eq!(l.admit(&p("a"), "gpu-slots", 3).await, Ok(5), "within capacity");
        assert_eq!(
            l.admit(&p("a"), "gpu-slots", 6).await.unwrap_err(),
            LedgerRefusal::Rejected { requested: 6, available: 5 }
        );
        assert_eq!(l.view().rejections(), 1, "the refusal is a record, not a silence");

        drop(l);
        let reopened = RightsLedger::open(&path).unwrap();
        assert_eq!(reopened.view().rejections(), 1, "and it is on disk");
    }

    /// **Every state counts**, `Unknown` included.
    #[tokio::test]
    async fn units_are_counted_across_all_five_states() {
        let mut l = RightsLedger::open(temp("states")).unwrap();
        for (i, state) in ALL_STATES.iter().enumerate() {
            let term = format!("term-{i}");
            l.allocate(right("a", &term, "gpu-slots", 1)).await.expect("allocated");
            l.transition(&p("a"), &t(&term), *state).await.expect("moved");
        }
        assert_eq!(l.view().held_units(&p("a"), "gpu-slots"), 5, "one unit in each of five states");
        let by = l.view().units_by_state(&p("a"), "gpu-slots");
        assert_eq!(by.get(&RightState::Unknown), Some(&1), "unknown is a state, not a zero");
        assert_eq!(by.len(), 5);
    }

    /// The journal is the truth: reopening folds it back to the same view.
    #[tokio::test]
    async fn reopening_folds_the_journal_back_to_the_same_view() {
        let path = temp("reopen");
        let mut l = RightsLedger::open(&path).unwrap();
        l.allocate(right("a", "term-1", "gpu-slots", 4)).await.unwrap();
        l.allocate(right("a", "term-2", "gpu-slots", 2)).await.unwrap();
        l.allocate(right("b", "term-1", "disk-gb", 100)).await.unwrap();
        l.transition(&p("a"), &t("term-1"), RightState::Serving).await.unwrap();
        l.release(&p("a"), &t("term-2")).await.unwrap();
        let _ = l.admit(&p("b"), "disk-gb", 500).await;
        let before = l.view().clone();
        let head_before = l.head(&p("a"));
        drop(l);

        let reopened = RightsLedger::open(&path).unwrap();
        assert_eq!(reopened.view(), &before);
        assert_eq!(reopened.head(&p("a")), head_before, "the head reflects the same last event");
    }

    /// A journal the ledger cannot read fails the open rather than starting empty over a full file.
    #[tokio::test]
    async fn a_corrupt_journal_fails_closed_on_open() {
        let path = temp("corrupt");
        {
            let raw = Journal::open(&path, "test/journal").unwrap();
            raw.append(b"not a ledger event".to_vec()).await.unwrap();
        }
        let err = RightsLedger::open(&path).err().expect("must not open over undecodable records");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    /// The head is a claim over the live totals, sorted by resource, at the last recorded sequence.
    #[tokio::test]
    async fn the_head_carries_sorted_live_totals_at_the_last_sequence() {
        let mut l = RightsLedger::open(temp("head")).unwrap();
        l.allocate(right("a", "term-1", "gpu-slots", 4)).await.unwrap();
        l.allocate(right("a", "term-2", "disk-gb", 100)).await.unwrap();
        l.allocate(right("b", "term-1", "gpu-slots", 9)).await.unwrap();
        let h = l.head(&p("a"));
        assert_eq!(h.holder, p("a"));
        assert_eq!(h.seq, 2);
        assert_eq!(h.totals, vec![("disk-gb".to_string(), 100), ("gpu-slots".to_string(), 4)]);
        assert_ne!(h.canonical_bytes(), l.head(&p("b")).canonical_bytes());
    }

    /// The head's signature covers its canonical bytes; a changed total does not verify.
    #[cfg(feature = "tls")]
    #[tokio::test]
    async fn a_signed_head_does_not_verify_once_a_total_changes() {
        use ed25519_dalek::SigningKey;
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pubkey = key.verifying_key().to_bytes();

        let mut l = RightsLedger::open(temp("sign")).unwrap();
        l.allocate(right("a", "term-1", "gpu-slots", 4)).await.unwrap();
        let head = l.head(&p("a"));
        let sig = mycelium_core::tls::sign_bytes(&key, &head.canonical_bytes());
        assert!(head.verify(&pubkey, &sig));

        let mut forged = head.clone();
        forged.totals[0].1 = 40;
        assert!(!forged.verify(&pubkey, &sig), "a bigger claim under the old signature is caught");
    }
}
