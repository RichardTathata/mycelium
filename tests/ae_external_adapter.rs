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
    ActionEnvelope, ActionEvaluator, ActionMapping, Decision, GossipAgent, GossipConfig, NodeId,
    Verdict,
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
