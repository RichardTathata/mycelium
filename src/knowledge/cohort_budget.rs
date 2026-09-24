//! **Aggregate budgets over a declared cohort** (Boundary H item H6) —
//! [`docs/design/knowledge-cohorts.md`](../../../docs/design/knowledge-cohorts.md) §6.
//!
//! # The gap this closes
//!
//! A per-caller cap bounds each member. A colluding population stays inside every member's cap and
//! overwhelms the provider together: in the Hugging Face incident, agent load took the artifact
//! repository down. The federation edge already meters calls **per partner domain**
//! (`CallPolicy::max_in_flight_per_partner`); nothing metered a **population inside one domain**.
//!
//! # What this does
//!
//! A provider holds a [`CohortBudget`] and asks it to [`admit`](CohortBudget::admit) each call. The
//! caller's cohorts come from the **provider's own** [`CohortView`] — declarations from operators the
//! provider trusts — resolved from the caller's **authenticated principal**. There is no parameter
//! through which a caller could name its cohort, so a caller-supplied label cannot select a budget.
//!
//! - A caller in several cohorts takes a slot in **every** one and must fit within each cap (merge,
//!   never split: belonging to two fleets never buys twice the room).
//! - A caller no trusted declaration places shares the single **undeclared** pool, so freshly minted
//!   identities cannot each get a budget of their own.
//! - A slot is an RAII guard: dropping it releases the call's place, on every return path.
//!
//! # Stated limits (the same as `max_in_flight_per_partner`'s)
//!
//! - **Per provider instance.** N instances of a provider mean N × the cap.
//! - **Concurrency, not rate.** It bounds calls in flight, not calls per second.
//! - **Only calls the provider admits through it.** A resource reached around the provider is
//!   outside it — which is what the confined-fleet profile (H7) exists to prevent.
//! - The caller's principal must be the one the provider **verified** (`GatewayCaller` /
//!   `request_authorized`); this module trusts what it is given.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::cohort::{CohortKey, CohortView};
use super::IssuerId;

/// Which pool a slot draws from.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BudgetPool {
    /// A declared cohort.
    Cohort(CohortKey),
    /// Every caller no trusted declaration places, together.
    Undeclared,
}

/// Why a call was not admitted.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CohortRefusal {
    /// The pool is carrying its cap. The call was **not** admitted and nothing was taken from any
    /// other pool.
    AtCapacity {
        /// The full pool.
        pool: BudgetPool,
        /// Its cap.
        limit: usize,
    },
}

impl std::fmt::Display for CohortRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtCapacity { pool, limit } => {
                write!(f, "cohort budget at capacity ({limit} in flight) for {pool:?}")
            }
        }
    }
}

impl std::error::Error for CohortRefusal {}

/// A provider's aggregate budgets, one per cohort plus one undeclared pool.
///
/// One lock (lock-order table row 42): a leaf, held for a synchronous count and never across a
/// dispatch — that is what the RAII slot is for.
#[derive(Debug)]
pub struct CohortBudget {
    per_cohort: usize,
    undeclared: usize,
    in_flight: Mutex<HashMap<BudgetPool, usize>>,
}

/// A place in every pool the caller belongs to. Dropping it releases them all.
#[derive(Debug)]
pub struct CohortSlot {
    budget: Arc<CohortBudget>,
    pools: Vec<BudgetPool>,
}

impl Drop for CohortSlot {
    fn drop(&mut self) {
        let mut in_flight = self.budget.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        for pool in &self.pools {
            if let Some(n) = in_flight.get_mut(pool) {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    // The map stays exactly the set of pools with work in flight.
                    in_flight.remove(pool);
                }
            }
        }
    }
}

impl CohortBudget {
    /// Budgets of `per_cohort` calls in flight for each declared cohort, and `undeclared` for every
    /// unplaced caller together. `0` means unlimited for that kind of pool.
    pub fn new(per_cohort: usize, undeclared: usize) -> Arc<Self> {
        Arc::new(Self { per_cohort, undeclared, in_flight: Mutex::new(HashMap::new()) })
    }

    /// **Admit one call** from the authenticated `caller`, placing it through the provider's own
    /// `view` at `now_ms`. Either a slot in every pool the caller belongs to, or a refusal naming the
    /// first full pool — never a partial admission.
    pub fn admit(
        self: &Arc<Self>,
        caller: &IssuerId,
        view: &CohortView,
        now_ms: u64,
    ) -> Result<CohortSlot, CohortRefusal> {
        let placed = view.placement(caller, now_ms, now_ms).cohorts;
        let mut pools: Vec<BudgetPool> = if placed.is_empty() {
            vec![BudgetPool::Undeclared]
        } else {
            placed.into_iter().map(BudgetPool::Cohort).collect()
        };
        pools.sort();

        let mut in_flight = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        // Check every pool first, so a refusal takes nothing from any of them.
        for pool in &pools {
            let limit = self.limit_for(pool);
            if limit != 0 && in_flight.get(pool).copied().unwrap_or(0) >= limit {
                return Err(CohortRefusal::AtCapacity { pool: pool.clone(), limit });
            }
        }
        for pool in &pools {
            *in_flight.entry(pool.clone()).or_insert(0) += 1;
        }
        drop(in_flight);
        Ok(CohortSlot { budget: Arc::clone(self), pools })
    }

    /// Calls currently in flight for `pool`.
    pub fn in_flight(&self, pool: &BudgetPool) -> usize {
        self.in_flight.lock().unwrap_or_else(|e| e.into_inner()).get(pool).copied().unwrap_or(0)
    }

    fn limit_for(&self, pool: &BudgetPool) -> usize {
        match pool {
            BudgetPool::Cohort(_) => self.per_cohort,
            BudgetPool::Undeclared => self.undeclared,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::cohort::{CohortDeclaration, SignedCohortDeclaration};
    use crate::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
    use crate::node_id::NodeId;
    use ed25519_dalek::SigningKey;
    use std::collections::HashMap as Map;

    fn iss(s: &str) -> IssuerId {
        IssuerId::new(s).unwrap()
    }

    /// A view trusting one operator, holding `cohorts` as (name, members).
    fn view(cohorts: &[(&str, &[&str])]) -> CohortView {
        let sk = SigningKey::from_bytes(&[21u8; 32]);
        let op = iss("operator:acme");
        let mut ext = TrustedExternalIssuers::new();
        ext.trust(op.clone(), sk.verifying_key().to_bytes()).unwrap();
        let mut v = CohortView::trusting([op.clone()]);
        for (name, members) in cohorts {
            let declaration = CohortDeclaration {
                operator: op.clone(),
                cohort: (*name).into(),
                seq: 1,
                members: members.iter().map(|m| iss(m)).collect(),
                valid_from_ms: 0,
                valid_until_ms: u64::MAX,
            };
            let signature = mycelium_core::tls::sign_bytes(&sk, &declaration.canonical_bytes()).to_vec();
            v.offer(&SignedCohortDeclaration { declaration, signature }, &Map::<NodeId, MemberKeys>::new(), &ext);
        }
        v
    }

    fn fleet(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("agent-{i}")).collect()
    }

    /// **The gap, closed.** Fifty members, each making one call — no per-caller cap is anywhere
    /// near reached — together exceed the cohort's cap of ten, and the excess is refused.
    #[test]
    fn a_cohort_within_every_per_caller_cap_is_capped_together() {
        let names = fleet(50);
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let v = view(&[("fleet", &refs)]);
        let budget = CohortBudget::new(10, 0);
        let mut held = Vec::new();
        let mut refused = 0;
        for n in &names {
            match budget.admit(&iss(n), &v, 1_000) {
                Ok(slot) => held.push(slot),
                Err(CohortRefusal::AtCapacity { limit: 10, .. }) => refused += 1,
                Err(e) => panic!("unexpected {e:?}"),
            }
        }
        assert_eq!((held.len(), refused), (10, 40));
    }

    /// Another cohort is unaffected by a full one.
    #[test]
    fn a_full_cohort_does_not_block_another() {
        let v = view(&[("a", &["a1", "a2"]), ("b", &["b1"])]);
        let budget = CohortBudget::new(1, 0);
        let _held = budget.admit(&iss("a1"), &v, 1_000).unwrap();
        assert!(budget.admit(&iss("a2"), &v, 1_000).is_err(), "cohort a is full");
        assert!(budget.admit(&iss("b1"), &v, 1_000).is_ok(), "cohort b is its own pool");
    }

    /// **Dropping a slot releases it**, on every path.
    #[test]
    fn dropping_a_slot_releases_its_place() {
        let v = view(&[("fleet", &["x", "y"])]);
        let budget = CohortBudget::new(1, 0);
        {
            let _slot = budget.admit(&iss("x"), &v, 1_000).unwrap();
            assert!(budget.admit(&iss("y"), &v, 1_000).is_err());
        }
        assert!(budget.admit(&iss("y"), &v, 1_000).is_ok());
    }

    /// **Undeclared callers share one pool**, so minting fresh identities buys no room.
    #[test]
    fn undeclared_callers_share_one_pool() {
        let v = view(&[]);
        let budget = CohortBudget::new(0, 2);
        let _a = budget.admit(&iss("fresh-1"), &v, 1_000).unwrap();
        let _b = budget.admit(&iss("fresh-2"), &v, 1_000).unwrap();
        assert_eq!(
            budget.admit(&iss("fresh-3"), &v, 1_000).unwrap_err(),
            CohortRefusal::AtCapacity { pool: BudgetPool::Undeclared, limit: 2 }
        );
    }

    /// **Membership is the provider's view, never the caller's claim.** The API takes an
    /// authenticated principal and nothing else, so the only way into a cohort's budget is to be
    /// declared into it by an operator the provider trusts. A caller outside the declaration draws on
    /// the undeclared pool however it names itself.
    #[test]
    fn a_callers_own_naming_never_selects_a_budget() {
        let v = view(&[("fleet", &["agent-0"])]);
        let budget = CohortBudget::new(1, 1);
        let _member = budget.admit(&iss("agent-0"), &v, 1_000).unwrap();
        // An impostor named like a fleet member, or claiming the cohort's name, is simply undeclared.
        let _impostor = budget.admit(&iss("fleet"), &v, 1_000).unwrap();
        assert_eq!(budget.in_flight(&BudgetPool::Undeclared), 1);
    }

    /// **A caller in two cohorts must fit both**, and a refusal takes nothing from either.
    #[test]
    fn a_caller_in_two_cohorts_needs_room_in_both_and_a_refusal_takes_nothing() {
        let v = view(&[("a", &["both", "a-only"]), ("b", &["both"])]);
        let budget = CohortBudget::new(1, 0);
        let _a = budget.admit(&iss("a-only"), &v, 1_000).unwrap();
        assert!(budget.admit(&iss("both"), &v, 1_000).is_err(), "cohort a is full");
        let b_key = BudgetPool::Cohort(CohortKey { operator: iss("operator:acme"), cohort: "b".into() });
        assert_eq!(budget.in_flight(&b_key), 0, "the refused call took nothing from cohort b");
    }
}
