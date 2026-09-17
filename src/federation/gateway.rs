//! Two-gateway operation: budgets, failover, and outcomes (item 2 PR 5).
//!
//! §10 of `docs/design/federated-domains.md`, made executable:
//!
//! - **at least two replaceable gateways, with no federation leader**
//! - **failover only for repeatable exports**; otherwise the caller gets `DeliveryUnknown`
//! - **fixed per-gateway quota slots**, so one partner cannot consume another's budget
//!
//! # Why "no leader" is a design constraint and not a preference
//!
//! A leader among gateways would be a coordination dependency — something that must be up, and
//! agreed upon, for two healthy domains to keep talking. That is the thing the substrate exists not
//! to need. So [`GatewayPool::admit`] picks by a **fixed local order**, with no election, no shared
//! state between gateways and no message exchanged to decide. Two gateways making the same choice
//! independently is fine; that is not a conflict, it is two gateways working.
//!
//! # Why a budget is per *partner*, not per gateway
//!
//! A single pool of slots would let a busy partner exhaust the gateway and starve everyone else —
//! availability for one bought at the cost of the rest, with no one having decided that. Slots are
//! therefore fixed **per partner**, and a partner at its limit is refused while others proceed
//! untouched. [`GatewayPool::admit`] returns [`CallOutcome::NoCapacity`] rather than queueing,
//! because an unbounded queue is the same starvation with a longer delay.
//!
//! # Why an unknown outcome is not a failure
//!
//! Item 1's vocabulary, applied here: **a timeout is `DeliveryUnknown`, never a negative.** A
//! gateway that stops answering has told us nothing about whether the far side ran the call. For a
//! repeatable export that does not matter and we may try elsewhere. For an at-most-once export it
//! matters entirely, and retrying would risk doing it twice — so the caller is told *unknown*, which
//! is the truth, rather than *failed*, which is a guess.

use super::{call::CallRefusal, DomainId};
use std::collections::HashMap;

/// Whether invoking an export twice is safe.
///
/// Declared **per export by the domain that offers it**, because only the provider knows. A default
/// would be wrong either way round: defaulting to repeatable risks doing something twice, and
/// defaulting to at-most-once quietly disables failover for everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeatability {
    /// Invoking twice has the same effect as once. Safe to fail over.
    Repeatable,
    /// Must run at most once. **Never failed over** — an unknown outcome stays unknown.
    AtMostOnce,
}

/// What became of a federated call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallOutcome {
    /// It ran and answered.
    Completed,
    /// It was refused, with the reason the verifier gave.
    Refused(CallRefusal),
    /// **The fate is unknown.** A gateway stopped answering; nothing was learned about whether the
    /// far side ran the call. Carries what was tried, so an operator can go and look.
    DeliveryUnknown {
        /// The gateways attempted, in order.
        attempted_via: Vec<String>,
        /// Why the last one stopped answering.
        reason: String,
    },
    /// No gateway had a free slot for this partner. Refused rather than queued.
    NoCapacity {
        /// Whose slots were full.
        partner: DomainId,
        /// How many each gateway allows.
        slots_per_partner: usize,
    },
}

/// One gateway's fixed per-partner slots.
#[derive(Debug, Clone)]
struct Gateway {
    id: String,
    slots_per_partner: usize,
    in_flight: HashMap<DomainId, usize>,
}

impl Gateway {
    fn has_room(&self, partner: &DomainId) -> bool {
        self.in_flight.get(partner).copied().unwrap_or(0) < self.slots_per_partner
    }
}

/// The replaceable gateways this domain runs. **At least two, and no leader among them.**
#[derive(Debug, Clone)]
pub struct GatewayPool {
    gateways: Vec<Gateway>,
}

impl GatewayPool {
    /// A pool over `ids`, each allowing `slots_per_partner` concurrent calls per partner.
    pub fn new(ids: impl IntoIterator<Item = impl Into<String>>, slots_per_partner: usize) -> Self {
        Self {
            gateways: ids
                .into_iter()
                .map(|id| Gateway {
                    id: id.into(),
                    slots_per_partner,
                    in_flight: HashMap::new(),
                })
                .collect(),
        }
    }

    /// How many gateways this pool has. §10 wants at least two, and a caller can check.
    pub fn len(&self) -> usize {
        self.gateways.len()
    }

    /// Is the pool empty?
    pub fn is_empty(&self) -> bool {
        self.gateways.is_empty()
    }

    /// Take a slot for `partner`, returning which gateway to use.
    ///
    /// **Fixed local order, no election.** The first gateway with room wins. Nothing is exchanged to
    /// decide this, because a decision that required agreement would be the coordinator §10 refuses.
    ///
    /// `excluding` lets a failover skip gateways already tried.
    pub fn admit(&mut self, partner: &DomainId, excluding: &[String]) -> Result<String, CallOutcome> {
        let slots = self.gateways.first().map(|g| g.slots_per_partner).unwrap_or(0);
        for g in self.gateways.iter_mut() {
            if excluding.iter().any(|x| x == &g.id) {
                continue;
            }
            if g.has_room(partner) {
                *g.in_flight.entry(partner.clone()).or_insert(0) += 1;
                return Ok(g.id.clone());
            }
        }
        Err(CallOutcome::NoCapacity { partner: partner.clone(), slots_per_partner: slots })
    }

    /// Give the slot back.
    pub fn release(&mut self, gateway_id: &str, partner: &DomainId) {
        if let Some(g) = self.gateways.iter_mut().find(|g| g.id == gateway_id)
            && let Some(n) = g.in_flight.get_mut(partner)
        {
            *n = n.saturating_sub(1);
        }
    }

    /// In-flight calls for `partner` on `gateway_id` — for diagnostics and for the tests that pin
    /// the isolation property.
    pub fn in_flight(&self, gateway_id: &str, partner: &DomainId) -> usize {
        self.gateways
            .iter()
            .find(|g| g.id == gateway_id)
            .and_then(|g| g.in_flight.get(partner).copied())
            .unwrap_or(0)
    }
}

/// A gateway stopped answering mid-call. Decide what the caller is told.
///
/// **This is where §10's "failover only for repeatable exports" lives**, and the asymmetry is the
/// whole point:
///
/// - `Repeatable` — try the next gateway. Doing it twice is by declaration harmless.
/// - `AtMostOnce` — **stop**. The far side may have run it. Retrying would risk a second execution,
///   and reporting failure would assert something we do not know. The caller gets
///   [`CallOutcome::DeliveryUnknown`], which is the only honest answer.
pub fn on_gateway_silent(
    pool: &mut GatewayPool,
    partner: &DomainId,
    repeatability: Repeatability,
    attempted: &mut Vec<String>,
    reason: &str,
) -> Result<String, CallOutcome> {
    // The slot on the silent gateway is released either way — it is not coming back.
    if let Some(last) = attempted.last() {
        pool.release(last, partner);
    }

    match repeatability {
        Repeatability::AtMostOnce => Err(CallOutcome::DeliveryUnknown {
            attempted_via: attempted.clone(),
            reason: reason.to_string(),
        }),
        Repeatability::Repeatable => match pool.admit(partner, attempted) {
            Ok(next) => {
                attempted.push(next.clone());
                Ok(next)
            }
            // Every gateway tried or full: still unknown, not failed. The distinction survives
            // running out of options.
            Err(CallOutcome::NoCapacity { .. }) | Err(_) => Err(CallOutcome::DeliveryUnknown {
                attempted_via: attempted.clone(),
                reason: format!("{reason}; no further gateway available"),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did(s: &str) -> DomainId {
        DomainId::new(s).unwrap()
    }

    fn pool() -> GatewayPool {
        GatewayPool::new(["gw-1", "gw-2"], 2)
    }

    // ── budgets ──────────────────────────────────────────────────────────────────────────────

    /// **The isolation property.** A partner at its limit must not touch anyone else's capacity —
    /// otherwise availability for one is bought at the cost of the rest, with nobody deciding it.
    #[test]
    fn one_partner_exhausting_its_slots_does_not_starve_another() {
        let mut p = GatewayPool::new(["gw-1"], 2);
        let beta = did("beta.example");
        let gamma = did("gamma.example");

        assert!(p.admit(&beta, &[]).is_ok());
        assert!(p.admit(&beta, &[]).is_ok());
        assert!(
            matches!(p.admit(&beta, &[]), Err(CallOutcome::NoCapacity { .. })),
            "beta is at its limit"
        );

        assert!(
            p.admit(&gamma, &[]).is_ok(),
            "gamma's budget is its own — a busy partner must not consume it"
        );
        assert_eq!(p.in_flight("gw-1", &beta), 2);
        assert_eq!(p.in_flight("gw-1", &gamma), 1);
    }

    /// Refused, not queued. An unbounded queue is the same starvation with a longer delay.
    #[test]
    fn a_full_budget_refuses_rather_than_queueing() {
        let mut p = GatewayPool::new(["gw-1"], 1);
        let beta = did("beta.example");
        p.admit(&beta, &[]).unwrap();
        match p.admit(&beta, &[]) {
            Err(CallOutcome::NoCapacity { partner, slots_per_partner }) => {
                assert_eq!(partner, beta);
                assert_eq!(slots_per_partner, 1, "the refusal says what the limit is");
            }
            other => panic!("expected NoCapacity, got {other:?}"),
        }
    }

    #[test]
    fn releasing_returns_the_slot() {
        let mut p = GatewayPool::new(["gw-1"], 1);
        let beta = did("beta.example");
        let gw = p.admit(&beta, &[]).unwrap();
        p.release(&gw, &beta);
        assert_eq!(p.in_flight("gw-1", &beta), 0);
        assert!(p.admit(&beta, &[]).is_ok(), "the slot is usable again");
    }

    /// A second gateway is capacity, not a standby: the pool spills over when the first is full.
    #[test]
    fn a_second_gateway_is_used_when_the_first_is_full() {
        let mut p = GatewayPool::new(["gw-1", "gw-2"], 1);
        let beta = did("beta.example");
        assert_eq!(p.admit(&beta, &[]).unwrap(), "gw-1");
        assert_eq!(p.admit(&beta, &[]).unwrap(), "gw-2", "spills to the next, with no election");
    }

    /// **No leader.** Selection is a fixed local order — nothing is exchanged, so two pools with the
    /// same configuration reach the same answer without having agreed on anything.
    #[test]
    fn selection_is_local_and_deterministic_with_no_election() {
        let beta = did("beta.example");
        let mut a = pool();
        let mut b = pool();
        assert_eq!(a.admit(&beta, &[]).unwrap(), b.admit(&beta, &[]).unwrap());
    }

    // ── failover and outcomes ────────────────────────────────────────────────────────────────

    /// **The asymmetry §10 asks for.** An at-most-once export is never failed over, and the caller
    /// is told *unknown* — which is true — rather than *failed*, which would be a guess.
    #[test]
    fn an_at_most_once_export_is_never_failed_over() {
        let mut p = pool();
        let beta = did("beta.example");
        let first = p.admit(&beta, &[]).unwrap();
        let mut attempted = vec![first];

        match on_gateway_silent(&mut p, &beta, Repeatability::AtMostOnce, &mut attempted, "timeout") {
            Err(CallOutcome::DeliveryUnknown { attempted_via, reason }) => {
                assert_eq!(attempted_via, vec!["gw-1".to_string()], "it names what was tried");
                assert_eq!(reason, "timeout");
            }
            other => panic!("an at-most-once export must not fail over: {other:?}"),
        }
        assert_eq!(attempted.len(), 1, "and no second gateway was attempted");
    }

    #[test]
    fn a_repeatable_export_fails_over_to_the_other_gateway() {
        let mut p = pool();
        let beta = did("beta.example");
        let first = p.admit(&beta, &[]).unwrap();
        let mut attempted = vec![first];

        let next = on_gateway_silent(&mut p, &beta, Repeatability::Repeatable, &mut attempted, "timeout")
            .expect("repeatable may fail over");
        assert_eq!(next, "gw-2");
        assert_eq!(attempted, vec!["gw-1".to_string(), "gw-2".to_string()]);
    }

    /// Running out of gateways does not turn unknown into failed. The distinction survives
    /// exhausting the options — that is exactly when it is easiest to lose.
    #[test]
    fn exhausting_every_gateway_still_reports_unknown_not_failed() {
        let mut p = GatewayPool::new(["gw-1"], 1);
        let beta = did("beta.example");
        let first = p.admit(&beta, &[]).unwrap();
        let mut attempted = vec![first];

        match on_gateway_silent(&mut p, &beta, Repeatability::Repeatable, &mut attempted, "timeout") {
            Err(CallOutcome::DeliveryUnknown { attempted_via, reason }) => {
                assert_eq!(attempted_via, vec!["gw-1".to_string()]);
                assert!(reason.contains("no further gateway"), "and says why it stopped: {reason}");
            }
            other => panic!("expected DeliveryUnknown, got {other:?}"),
        }
    }

    /// The silent gateway's slot is released — it is not coming back, and holding it would leak
    /// capacity on every timeout.
    #[test]
    fn a_silent_gateways_slot_is_released() {
        let mut p = GatewayPool::new(["gw-1", "gw-2"], 1);
        let beta = did("beta.example");
        let first = p.admit(&beta, &[]).unwrap();
        assert_eq!(p.in_flight("gw-1", &beta), 1);

        let mut attempted = vec![first];
        let _ = on_gateway_silent(&mut p, &beta, Repeatability::Repeatable, &mut attempted, "timeout");
        assert_eq!(p.in_flight("gw-1", &beta), 0, "the slot on the silent gateway is freed");
    }

    /// `DeliveryUnknown` is not `Refused`. One says we do not know; the other says we decided.
    #[test]
    fn unknown_and_refused_are_different_outcomes() {
        let unknown = CallOutcome::DeliveryUnknown {
            attempted_via: vec!["gw-1".into()],
            reason: "timeout".into(),
        };
        let refused = CallOutcome::Refused(CallRefusal::NotPermitted);
        assert_ne!(unknown, refused);
    }
}
