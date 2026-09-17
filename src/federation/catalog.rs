//! Filtered catalogs and the remote resolver (v3 contracts axis, item 2 PR 3).
//!
//! Two halves of the same rule, from `docs/design/federated-domains.md`:
//!
//! - **Outbound** — what a partner is allowed to *see* of our exports (§4: "allowlist catalogs").
//!   Filtering happens before the catalog leaves, so an ungranted export is **invisible**, not
//!   merely refused. A partner that can enumerate what it may not call has been told something.
//! - **Inbound** — what this gateway has *observed* of a partner's catalog, and when (§4:
//!   "attributable per-gateway observations"). These are not facts about the fleet. They are
//!   things a particular gateway saw at a particular time, and [`CatalogObservation`] says so in
//!   its fields rather than in a comment.
//!
//! # Expiry is the partition behaviour, not a cache policy
//!
//! §10: *"On disconnect: discovery **expires**, calls fail **explicitly**, and authority already
//! issued lasts only to its stated expiry. Nothing is silently extended."*
//!
//! So [`RemoteResolver`] resolves nothing from an observation older than its freshness window, and
//! says which of the two it was — never seen, or seen too long ago. A resolver that quietly served
//! a stale entry would turn a partition into a wrong answer, which is the failure mode the whole
//! §10 list exists to prevent.
//!
//! The age comes from `sim_seam::mono_elapsed`, so a recorded run replays an expiry instead of
//! waiting for one.

use super::{DomainId, DomainPolicy};
use std::time::{Duration, Instant};

/// A capability offered by a **partner domain**.
///
/// Deliberately a distinct type from a native capability (§4: "a distinct `RemoteCapability`").
/// They are not interchangeable and must not be: a native capability is something this mesh
/// advertises and can be resolved through `cap/`; this is something a *foreign* domain said it
/// offers, reachable only across the federation edge and only if policy permits. One type for both
/// would make "is this ours" a question about a string prefix.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteCapability {
    /// The domain offering it.
    pub domain: DomainId,
    /// The export name, as that domain published it.
    pub export: String,
}

/// What **this gateway** saw of a partner's catalog, and when.
///
/// Every field is part of the claim. `observed_by` is why two gateways disagreeing is a fact about
/// the gateways rather than a contradiction to resolve; `observed_at` is why an old observation can
/// be refused instead of believed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogObservation {
    /// Whose catalog this is.
    pub partner: DomainId,
    /// Which gateway saw it. An observation with no observer is an assertion.
    pub observed_by: String,
    /// When this gateway saw it.
    pub observed_at: Instant,
    /// The exports the partner published *to us* — already filtered on their side.
    pub exports: Vec<String>,
}

/// Why a remote capability did not resolve.
///
/// `Unknown` and `Expired` are kept apart for the same reason `InsufficientEvidence` and `Rejected`
/// are in the knowledge layer: *"we never heard of it"* and *"we heard, a while ago"* lead to
/// different operator actions, and collapsing them throws away the one fact the caller needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveFailure {
    /// No observation of that domain at all.
    UnknownDomain,
    /// The domain is known, but it never listed that export.
    NotExported,
    /// Seen, but longer ago than the freshness window allows — §10's "discovery expires".
    Expired {
        /// How stale the newest observation was.
        age: Duration,
    },
}

impl std::fmt::Display for ResolveFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDomain => write!(f, "no observation of that domain"),
            Self::NotExported => write!(f, "the domain did not list that export"),
            Self::Expired { age } => write!(f, "discovery expired: newest observation is {age:?} old"),
        }
    }
}

impl std::error::Error for ResolveFailure {}

// ── Outbound: the filtered catalog ────────────────────────────────────────────────────────────

/// What `partner` may see of our exports under `policy`.
///
/// **Filter, then publish.** An export the policy does not grant is absent from the result, not
/// present-and-refused: a catalog is a disclosure, and disclosing the existence of a capability is
/// already telling a partner something about this domain.
///
/// `exports` is the domain's full export list (from its `DomainDescriptor`); the result preserves
/// its order, because the issuer wrote that order and reordering it here would change what a
/// signature over the catalog covers.
pub fn filtered_catalog(exports: &[String], policy: &DomainPolicy, partner: &DomainId) -> Vec<String> {
    exports
        .iter()
        .filter(|e| policy.permits(partner, e))
        .cloned()
        .collect()
}

// ── Inbound: the remote resolver ──────────────────────────────────────────────────────────────

/// This gateway's cache of what partners have published to it.
///
/// **Not in the gossip KV namespace** — §8 of the record forbids a `federation/` prefix and
/// `scripts/check-kv-namespaces.sh` enforces the absence. Foreign observations live here, in the
/// companion's own memory, because once foreign state is in the medium it is replicated,
/// anti-entropied, and indistinguishable from state this mesh produced itself.
#[derive(Debug, Default)]
pub struct RemoteResolver {
    observations: Vec<CatalogObservation>,
    /// How old an observation may be and still resolve. §10's "discovery expires".
    freshness: Duration,
}

impl RemoteResolver {
    /// A resolver whose observations expire after `freshness`.
    pub fn new(freshness: Duration) -> Self {
        Self { observations: Vec::new(), freshness }
    }

    /// Record what a gateway saw. A newer observation for the same `(partner, observer)` replaces
    /// the older one — a gateway's latest sighting is its view, not an addition to it.
    pub fn observe(&mut self, obs: CatalogObservation) {
        if let Some(slot) = self
            .observations
            .iter_mut()
            .find(|o| o.partner == obs.partner && o.observed_by == obs.observed_by)
        {
            *slot = obs;
        } else {
            self.observations.push(obs);
        }
    }

    /// Resolve `export` on `domain`, or say why not.
    ///
    /// Fresh observations only, and **the caller's own policy is not consulted here** — this answers
    /// "did a partner offer this, recently enough to believe", which is a separate question from
    /// "may we invoke it". Authorisation is the gateway's, at the invocation edge; merging the two
    /// would let a discovery cache grant something.
    pub fn resolve(&self, domain: &DomainId, export: &str) -> Result<RemoteCapability, ResolveFailure> {
        let seen: Vec<&CatalogObservation> =
            self.observations.iter().filter(|o| &o.partner == domain).collect();
        if seen.is_empty() {
            return Err(ResolveFailure::UnknownDomain);
        }

        let mut newest_listing: Option<Duration> = None;
        for o in &seen {
            if o.exports.iter().any(|e| e == export) {
                let age = mycelium_core::sim_seam::mono_elapsed(&o.observed_at);
                newest_listing = Some(match newest_listing {
                    Some(best) if best <= age => best,
                    _ => age,
                });
            }
        }

        match newest_listing {
            None => Err(ResolveFailure::NotExported),
            Some(age) if age > self.freshness => Err(ResolveFailure::Expired { age }),
            Some(_) => Ok(RemoteCapability { domain: domain.clone(), export: export.to_string() }),
        }
    }

    /// Drop every observation of `partner` — what a disconnect does.
    ///
    /// §10 again: discovery expires on disconnect rather than lingering. Expiry by age handles the
    /// case where nobody noticed; this handles the case where somebody did.
    pub fn forget(&mut self, partner: &DomainId) {
        self.observations.retain(|o| &o.partner != partner);
    }

    /// Which gateways have observed `partner`, for diagnostics. Two gateways disagreeing is a fact
    /// about the gateways, and this is how an operator sees it.
    pub fn observers(&self, partner: &DomainId) -> Vec<&str> {
        self.observations
            .iter()
            .filter(|o| &o.partner == partner)
            .map(|o| o.observed_by.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did(s: &str) -> DomainId {
        DomainId::new(s).unwrap()
    }

    fn policy_granting(pairs: &[(&str, &str)]) -> DomainPolicy {
        DomainPolicy {
            domain: did("alpha.example"),
            revision: 1,
            grants: pairs.iter().map(|(p, e)| (did(p), (*e).to_string())).collect(),
        }
    }

    fn exports() -> Vec<String> {
        vec!["invoice.submit".into(), "invoice.status".into(), "ledger.audit".into()]
    }

    fn seen(partner: &str, by: &str, at: Instant, ex: &[&str]) -> CatalogObservation {
        CatalogObservation {
            partner: did(partner),
            observed_by: by.to_string(),
            observed_at: at,
            exports: ex.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    // ── outbound ─────────────────────────────────────────────────────────────────────────────

    /// **An ungranted export is invisible, not refused.** Disclosing that a capability exists is
    /// already telling a partner something about this domain.
    #[test]
    fn a_partner_sees_only_what_it_is_granted() {
        let p = policy_granting(&[("beta.example", "invoice.submit")]);
        let visible = filtered_catalog(&exports(), &p, &did("beta.example"));
        assert_eq!(visible, vec!["invoice.submit".to_string()]);
        assert!(
            !visible.iter().any(|e| e == "ledger.audit"),
            "an ungranted export must not appear in the catalog at all"
        );
    }

    #[test]
    fn a_partner_with_no_grants_sees_an_empty_catalog() {
        let p = policy_granting(&[("beta.example", "invoice.submit")]);
        assert!(
            filtered_catalog(&exports(), &p, &did("gamma.example")).is_empty(),
            "absence is denial — an unlisted partner sees nothing, not everything"
        );
    }

    /// The issuer's order is preserved: a signature over a catalog covers a sequence.
    #[test]
    fn the_filter_preserves_the_issuers_order() {
        let p = policy_granting(&[
            ("beta.example", "ledger.audit"),
            ("beta.example", "invoice.submit"),
        ]);
        let visible = filtered_catalog(&exports(), &p, &did("beta.example"));
        assert_eq!(visible, vec!["invoice.submit".to_string(), "ledger.audit".to_string()],
            "order comes from the export list, not from the grant list");
    }

    // ── inbound ──────────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_fresh_observation_resolves_and_carries_its_domain() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        r.observe(seen("beta.example", "gw-1", Instant::now(), &["invoice.submit"]));
        let cap = r.resolve(&did("beta.example"), "invoice.submit").expect("fresh");
        assert_eq!(cap, RemoteCapability {
            domain: did("beta.example"),
            export: "invoice.submit".into(),
        });
    }

    /// **The three failures are distinct**, because they lead to different operator actions:
    /// never heard of them · heard, but they never offered that · heard too long ago.
    #[test]
    fn the_three_resolve_failures_are_told_apart() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        assert_eq!(
            r.resolve(&did("beta.example"), "invoice.submit"),
            Err(ResolveFailure::UnknownDomain)
        );

        r.observe(seen("beta.example", "gw-1", Instant::now(), &["invoice.submit"]));
        assert_eq!(
            r.resolve(&did("beta.example"), "ledger.audit"),
            Err(ResolveFailure::NotExported),
            "known domain, unlisted export — not the same as never having heard of it"
        );

        // Seen, but long ago.
        let stale = Instant::now() - Duration::from_secs(600);
        let mut r2 = RemoteResolver::new(Duration::from_secs(60));
        r2.observe(seen("beta.example", "gw-1", stale, &["invoice.submit"]));
        match r2.resolve(&did("beta.example"), "invoice.submit") {
            Err(ResolveFailure::Expired { age }) => {
                assert!(age >= Duration::from_secs(600), "the refusal reports how stale: {age:?}");
            }
            other => panic!("expected Expired, got {other:?}"),
        }
    }

    /// §10: on disconnect, discovery **expires** — it is not silently extended. A resolver that
    /// served a stale entry would turn a partition into a wrong answer.
    #[test]
    fn discovery_expires_rather_than_being_extended() {
        let mut r = RemoteResolver::new(Duration::from_secs(1));
        r.observe(seen(
            "beta.example",
            "gw-1",
            Instant::now() - Duration::from_secs(5),
            &["invoice.submit"],
        ));
        assert!(
            matches!(r.resolve(&did("beta.example"), "invoice.submit"), Err(ResolveFailure::Expired { .. })),
            "past the freshness window the answer is a refusal, not the last known good value"
        );
    }

    /// A disconnect a gateway *noticed* drops the observations outright.
    #[test]
    fn forgetting_a_partner_removes_its_discovery() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        r.observe(seen("beta.example", "gw-1", Instant::now(), &["invoice.submit"]));
        r.forget(&did("beta.example"));
        assert_eq!(
            r.resolve(&did("beta.example"), "invoice.submit"),
            Err(ResolveFailure::UnknownDomain),
            "after a disconnect the domain is unknown again, not merely stale"
        );
    }

    /// Observations are **per gateway**. Two gateways disagreeing is a fact about the gateways —
    /// the newer sighting resolves, and both observers stay visible to an operator.
    #[test]
    fn observations_are_attributable_to_the_gateway_that_made_them() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        r.observe(seen("beta.example", "gw-1", Instant::now() - Duration::from_secs(30), &[]));
        r.observe(seen("beta.example", "gw-2", Instant::now(), &["invoice.submit"]));

        assert!(r.resolve(&did("beta.example"), "invoice.submit").is_ok(),
            "one gateway having seen it is enough to resolve");
        let mut who = r.observers(&did("beta.example"));
        who.sort_unstable();
        assert_eq!(who, vec!["gw-1", "gw-2"], "both observers remain visible; neither is merged away");
    }

    /// A gateway's later sighting replaces its earlier one — a view, not an accumulation. Otherwise
    /// a withdrawn export would linger because it was once seen.
    #[test]
    fn a_gateways_newer_sighting_replaces_its_older_one() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        r.observe(seen("beta.example", "gw-1", Instant::now(), &["invoice.submit", "ledger.audit"]));
        r.observe(seen("beta.example", "gw-1", Instant::now(), &["invoice.submit"]));
        assert_eq!(
            r.resolve(&did("beta.example"), "ledger.audit"),
            Err(ResolveFailure::NotExported),
            "a withdrawn export must not survive because an earlier observation listed it"
        );
        assert_eq!(r.observers(&did("beta.example")).len(), 1, "one view per gateway");
    }

    /// **Discovery is not authorisation.** The resolver answers "was this offered, recently"; it
    /// never answers "may we call it". Merging the two would let a cache grant something.
    #[test]
    fn resolving_does_not_consult_or_confer_authority() {
        let mut r = RemoteResolver::new(Duration::from_secs(60));
        r.observe(seen("beta.example", "gw-1", Instant::now(), &["ledger.audit"]));
        // The partner offered it; our own policy grants us nothing. Resolution still succeeds —
        // and it is the invocation edge, not this, that refuses.
        assert!(r.resolve(&did("beta.example"), "ledger.audit").is_ok());
        let ours = policy_granting(&[]);
        assert!(
            !ours.permits(&did("beta.example"), "ledger.audit"),
            "authorisation is a separate question, answered elsewhere"
        );
    }
}
