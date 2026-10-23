//! The purchasing remit, as a policy: one ledger is in remit, one is not, and anything else is undecided.
//!
//! An [`ActionEvaluator`] decides per call, at the provider, before the tool runs. Three verdicts, and the third is
//! the one that matters: *permit* and *deny* are answers; *indeterminate* is the policy saying it could not decide,
//! and the provider refuses rather than letting a gap read as permission. The envelope carries only the argument
//! values this policy declared it needs (`ledger`), so the rest of the payload stays out of the evidence record.
use mycelium::{ActionEnvelope, ActionEvaluator, ActionMapping, Decision};

/// What the evidence record names as the policy that decided. A real adapter derives it from the policy bytes.
pub const REVISION: &str = "governed-call-remit-1";

pub struct Remit;

impl ActionEvaluator for Remit {
    fn security_relevant_arguments(&self) -> Vec<String> {
        vec!["ledger".to_string()]
    }

    fn mapping(&self, operation: &str, resource: &str) -> ActionMapping {
        if operation == "tools/call" && resource.starts_with("tool:purchase.place@") {
            ActionMapping::mapped("purchasing-remit", REVISION)
        } else {
            ActionMapping::unmapped("purchasing-remit", REVISION)
        }
    }

    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision {
        match envelope.selected_arguments.get("ledger").and_then(|v| v.as_str()) {
            Some("petty-cash") => Decision::permit("petty-cash is in the remit", REVISION).checking(["ledger=petty-cash"]),
            Some("capital") => Decision::deny("the capital ledger is outside the remit", REVISION).checking(["ledger=capital"]),
            Some(other) => Decision::indeterminate(format!("no clause covers ledger `{other}`"), REVISION),
            None => Decision::indeterminate("`ledger` absent: the remit cannot be applied", REVISION),
        }
    }
}
