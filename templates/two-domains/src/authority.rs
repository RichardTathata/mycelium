//! The authority half of the profile — every node, both domains: an action evaluator that permits exactly the rules
//! `federation.toml` derives and refuses everything else (`Indeterminate`, which the seam refuses), at the gateway and
//! at the provider; the evidence journal its decisions are recorded in before anything is dispatched; and an
//! execution authority whose revocation epochs survive a restart. A real deployment attaches its own policy adapter
//! here — the reference evaluator is a stand-in, and the rules are the scaffold's.
use ed25519_dalek::SigningKey;
use mycelium::knowledge::{issuer::TrustedExternalIssuers, IssuerId};
use mycelium::mandate::authority::{ClockModel, DurableEpochs, ExecutionGate, FreshnessPolicy, ResourceTier};
use mycelium::mandate::grant::{EntitlementTable, GrantVerifier};
use mycelium::mandate::{PrincipalId, ResourceAuthority};
use mycelium::{EvidenceJournal, EvidenceProfile, ExecutionAuthority, GossipAgent, ReferenceEvaluator, Rule};
use std::sync::Arc;

pub fn attach(agent: &GossipAgent, node: &str, domain: &str, operator: &SigningKey, rules: Vec<(String, String, String)>) {
    let data = std::path::PathBuf::from(format!("data/{node}"));
    std::fs::create_dir_all(&data).unwrap_or_else(|e| crate::fleet::refuse(&format!("{}: {e}", data.display())));
    let mut evaluator = ReferenceEvaluator::new(format!("{domain}-1")).with_catalogue(domain, "1");
    for (actor, operation, resource) in rules {
        evaluator = evaluator.map_action(operation.clone(), resource.clone()).allow(Rule::new(actor, operation, resource));
    }
    // The journal first, so the evaluator's first decision has somewhere to be recorded.
    let journal = EvidenceJournal::open(data.join("evidence"), EvidenceProfile::Strict).unwrap_or_else(|e| crate::fleet::refuse(&format!("evidence journal: {e}")));
    agent.with_evidence_journal(journal);
    agent.with_action_evaluator(Arc::new(evaluator));
    agent.with_provider_enforcement();
    // Mandates this domain's operator issues are verified here, under the domain's own key; none is presented in
    // this scaffold, so the gate decides nothing yet — it is attached so one would be checked, its epochs on disk.
    let issuer = format!("operator:{domain}");
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new(&issuer).expect("issuer id"), operator.verifying_key().to_bytes()).expect("trust the operator");
    let mut entitlements = EntitlementTable::new();
    entitlements.entitle(domain, PrincipalId::new(&issuer).expect("principal"));
    let gate = ExecutionGate::strict(
        ResourceAuthority::new(domain, 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 500 },
        FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
    )
    .expect("execution gate");
    let authority = ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), external);
    authority.with_durable_epochs(DurableEpochs::open(data.join("epochs")).unwrap_or_else(|e| crate::fleet::refuse(&format!("epochs: {e}"))));
    agent.with_execution_authority(Arc::new(authority));
}
