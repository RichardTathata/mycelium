//! **An evaluator written from outside the crate**, as the private Cedar adapter (AE-T T2) will be.
//!
//! This file is a separate crate that links `mycelium` as a dependency, so it compiles only if the
//! AE seam's public types can actually be *constructed* by a foreign implementor. The first cut
//! could not: `Decision` and `ActionMapping` are `#[non_exhaustive]` with no public constructor, so
//! `impl ActionEvaluator for MyAdapter` failed with E0639 and a foreign evaluator could never
//! return anything but `Indeterminate`. Found by review, 2026-09-15; this test is the gate that
//! keeps the replaceable-evaluator premise true.
#![cfg(all(feature = "gateway", feature = "tls"))]

use mycelium::{
    ActionEnvelope, ActionEvaluator, ActionMapping, Decision, GossipAgent, GossipConfig,
    MandateBinding, MandateState, NodeId, Verdict,
};
use std::sync::Arc;

/// A minimal third-party adapter: it permits one operation, prohibits another, needs one argument
/// value, and reports its own catalogue — every part of the trait a real adapter uses.
struct ForeignAdapter;

impl ActionEvaluator for ForeignAdapter {
    fn security_relevant_arguments(&self) -> Vec<String> {
        vec!["amount".to_string()]
    }

    fn mapping(&self, operation: &str, _resource: &str) -> ActionMapping {
        if operation == "tools/call" {
            ActionMapping::mapped("foreign-catalogue", "3")
        } else {
            ActionMapping::unmapped("foreign-catalogue", "3")
        }
    }

    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision {
        // AE1: a foreign adapter must be able to *name* the mandate states, or it cannot honour
        // the fence and a revocation can be laundered by whichever evaluator an operator plugged
        // in. Matching on them here is what makes the re-export load-bearing rather than
        // decorative — delete it from the crate's `pub use` and this file stops compiling.
        if let Some(b) = &envelope.mandate {
            match &b.state {
                MandateState::Refused(refusal) => {
                    return Decision::deny(format!("mandate refused: {refusal}"), "foreign-1")
                        .checking([format!("mandate term={} epoch={}", b.term, b.epoch)]);
                }
                MandateState::Unknown(why) => {
                    return Decision::indeterminate("the mandate could not be established", "foreign-1")
                        .with_errors([format!("mandate not established: {why}")]);
                }
                MandateState::Established => {}
            }
        }

        // The values a digest cannot answer: this is why the envelope carries them.
        let amount = envelope.selected_arguments.get("amount").and_then(|v| v.as_f64());
        match amount {
            None => Decision::indeterminate("amount was not supplied", "foreign-1")
                .with_errors(["missing argument: amount"]),
            Some(a) if a > 500.0 => {
                Decision::deny("amount exceeds the granted limit", "foreign-1").checking(["amount <= 500"])
            }
            Some(_) => Decision::permit("within the granted limit", "foreign-1").checking(["amount <= 500"]),
        }
    }
}

fn envelope(amount: Option<f64>) -> ActionEnvelope {
    let via = NodeId::new("127.0.0.1", 9000).unwrap();
    let mut b = ActionEnvelope::builder("oidc:idp/alice", via, "tools/call", "tool:pay@n1")
        .identities("op-1", "op-1/1")
        .scopes(["mcp:invoke"])
        .mapping(ActionMapping::mapped("foreign-catalogue", "3"))
        .validity(0, 60_000);
    if let Some(a) = amount {
        let mut args = serde_json::Map::new();
        args.insert("amount".to_string(), serde_json::json!(a));
        b = b.selected_arguments(args);
    }
    b.build()
}

/// The adapter compiles and decides all three ways from outside the crate.
#[test]
fn a_foreign_evaluator_can_permit_deny_and_be_undecided() {
    let ev = ForeignAdapter;
    assert_eq!(ev.evaluate(&envelope(Some(100.0))).verdict, Verdict::Permit);
    assert_eq!(ev.evaluate(&envelope(Some(5_000.0))).verdict, Verdict::Deny);
    let undecided = ev.evaluate(&envelope(None));
    assert_eq!(undecided.verdict, Verdict::Indeterminate);
    assert!(!undecided.errors.is_empty(), "an undecided adapter says why");
    // And it can name its own catalogue.
    assert_eq!(ev.mapping("tools/call", "tool:pay@n1").catalogue, "foreign-catalogue");
}

/// It also attaches to a real agent — the surface an adopter actually uses. The agent is never
/// started, so no port is bound and no `test-util` helper is needed (which keeps this compiling
/// under `make check`'s plain `clippy --lib --tests`).
#[test]
fn a_foreign_evaluator_attaches_to_an_agent() {
    let mut cfg = GossipConfig::default();
    cfg.bind_port = 0;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), cfg));
    agent.with_action_evaluator(Arc::new(ForeignAdapter));
}


/// A third-party adapter can name every mandate state and act on it (AE1).
///
/// This is the re-export's gate. `MandateBinding` and `MandateState` are public types on a public
/// field, but they were initially not in the crate's `pub use` — so an evaluator in another crate
/// could *receive* a mandate and had no way to match on it. A replaceable evaluator that cannot
/// see the fence cannot honour it, and the plan's whole premise is that the evaluator is
/// replaceable.
#[test]
fn a_foreign_adapter_can_read_and_honour_the_mandate_states() {
    use mycelium::mandate::{MandateRefusal, PrincipalId, TermId};

    let holder = PrincipalId::new("depot-dispatcher").expect("principal");
    let term = TermId::new("term-q3").expect("term");

    // `MandateBinding` is `#[non_exhaustive]`, so a foreign crate reaches it through the
    // constructors rather than a struct literal — which is the point: the three of them are the
    // whole surface, and a later field cannot break this file.
    let with = |binding: MandateBinding| -> ActionEnvelope {
        let via = NodeId::new("127.0.0.1", 9000).unwrap();
        let mut args = serde_json::Map::new();
        args.insert("amount".to_string(), serde_json::json!(10.0));
        ActionEnvelope::builder("oidc:idp/alice", via, "tools/call", "tool:pay@n1")
            .identities("op-1", "op-1/1")
            .scopes(["mcp:invoke"])
            .mapping(ActionMapping::mapped("foreign-catalogue", "3"))
            .validity(0, 60_000)
            .selected_arguments(args)
            .mandate(binding)
            .build()
    };

    // Established: the adapter falls through to its own policy, which permits this amount.
    let ok = ForeignAdapter.evaluate(&with(MandateBinding::established(
        holder.clone(),
        term.clone(),
        "depot-ops",
        4,
    )));
    assert_eq!(ok.verdict, Verdict::Permit, "a live mandate must not block the adapter's own rule");

    // Refused: denied, by name — the amount never gets a say.
    let refused = ForeignAdapter.evaluate(&with(MandateBinding::refused(
        holder.clone(),
        term.clone(),
        "depot-ops",
        4,
        MandateRefusal::Superseded { installed: 9, presented: 4 },
    )));
    assert_eq!(refused.verdict, Verdict::Deny);
    assert!(
        refused.reason.contains("superseded"),
        "the refusal must reach a foreign adapter by name: {:?}",
        refused.reason,
    );

    // Unknown: not established, which is not a denial.
    let unknown = ForeignAdapter.evaluate(&with(MandateBinding::unknown(
        holder,
        term,
        "depot-ops",
        4,
        "fence unreachable",
    )));
    assert_eq!(unknown.verdict, Verdict::Indeterminate);
    assert!(unknown.errors.iter().any(|e| e.contains("fence unreachable")));
}
