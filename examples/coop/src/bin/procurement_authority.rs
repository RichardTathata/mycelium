//! Example — **procurement authority**: what a gateway can promise about an agent's actions, and
//! what it cannot.
//!
//! §12.1's AE gallery row, on the **public seam and the reference evaluator**. The Cedar adapter
//! and the evidence exporter are private; everything shown here is in the `mycelium` crate, so an
//! adopter can run it, read it, and build against it.
//!
//! A food co-op buys from its suppliers through an agent. The agent holds an approved purchasing
//! remit with a ceiling on it. Six things happen, and the fifth is the one worth staying for:
//!
//!   1. **A purchase within the remit** — permitted, and the decision says what it checked.
//!   2. **An over-limit purchase** — *denied*, with no business effect. Note who attests that.
//!   3. **A purchase nobody wrote a rule about** — **not denied**. *Authority not established*,
//!      which is a different fact and must never be reported as drift.
//!   4. **A steward's intervention** — a different principal, authorised on its own terms.
//!   5. **A misconfigured route** — denied, and it happened anyway. The evidence says **both**.
//!   6. **A corrected observation** — the supplier's first report was wrong; the correction cites
//!      the record it replaces and the original stays on disk.
//!
//! Finally it runs the **contract fixtures** against the evaluator, which is what an operator does
//! to a replacement evaluator before trusting it.
//!
//! Run:  cargo run -p mycelium-coop-examples --bin procurement_authority

use mycelium::ae_contract::{self, Expected, ReferenceUnderTest};
use mycelium::{
    ActionEnvelope, ActionEvaluator, ActionMapping, AeEvidence, Decision, DecisionKind,
    EvidenceJournal, EvidenceProfile, Execution, NodeId, RecordKind, ReferenceEvaluator, Rule,
};
use std::sync::Arc;

const BUYER: &str = "oidc:coop-idp/buyer-agent";
const STEWARD: &str = "oidc:coop-idp/steward";
const PURCHASE: &str = "tool:purchase_order@procurement";
const HALT: &str = "tool:halt_purchasing@procurement";
const POLICY_REVISION: &str = "procurement-2026-q3.r7";
const ENFORCEMENT_POINT: &str = "coop-gateway";

/// The deployed policy: the approved declaration, as rules this enforcement point evaluates.
///
/// The ceiling is an explicit **prohibition**, not the absence of an allowance. That distinction is
/// the whole of step 2 versus step 3 below: a prohibition means an authority decided *no*, and an
/// uncovered action means nobody decided anything. Evidence that conflated them would report an
/// unreviewed purchase as a policy violation.
fn deployed_policy() -> Arc<dyn ActionEvaluator> {
    Arc::new(
        ReferenceEvaluator::new(POLICY_REVISION)
            .allow(Rule::new(BUYER, "tools/call", PURCHASE).requiring_scopes(["mcp:invoke"]))
            .allow(Rule::new(STEWARD, "tools/call", HALT).requiring_scopes(["mcp:invoke"]))
            // Over the approved ceiling: refused by name, whoever asks.
            .prohibit(Rule::new("*", "tools/call", PURCHASE).requiring_value(
                "over_ceiling",
                serde_json::Value::Bool(true),
            )),
    )
}

fn envelope(actor: &str, resource: &str, operation_id: &str, attempt: &str) -> ActionEnvelope {
    let via = NodeId::new("127.0.0.1", 9000).expect("a node id");
    ActionEnvelope::builder(actor, via, "tools/call", resource)
        .identities(operation_id, attempt)
        .scopes(["mcp:invoke"])
        .mapping(ActionMapping::mapped("cat-procurement", "7"))
        .expected_policy_revision(POLICY_REVISION)
        .validity(1_000, 61_000)
        .build()
}

/// A purchase envelope, flagged when it exceeds the approved ceiling.
///
/// The flag is a *selected argument* — a security-relevant value the enforcement point carries so
/// the policy can decide on it. A digest alone could not: it proves the arguments did not change,
/// not what they said.
fn purchase(attempt: &str, amount_pence: u64, ceiling: u64) -> ActionEnvelope {
    let mut args = serde_json::Map::new();
    args.insert("amount_pence".into(), serde_json::json!(amount_pence));
    // **Always stated, both ways.** `Rule::requiring_value` treats an *absent* argument as still
    // matching, deliberately — so the seam can report it as a fact that was not established rather
    // than let the rule silently fall through. The consequence here is that a prohibition keyed on
    // `over_ceiling` would match an envelope that simply never mentioned it, and every purchase
    // would be denied. Saying `false` out loud is the difference between a rule about amounts and
    // a rule about silence.
    args.insert("over_ceiling".into(), serde_json::Value::Bool(amount_pence > ceiling));
    let via = NodeId::new("127.0.0.1", 9000).expect("a node id");
    ActionEnvelope::builder(BUYER, via, "tools/call", PURCHASE)
        .identities("op-purchase", attempt)
        .scopes(["mcp:invoke"])
        .selected_arguments(args)
        .mapping(ActionMapping::mapped("cat-procurement", "7"))
        .expected_policy_revision(POLICY_REVISION)
        .validity(1_000, 61_000)
        .build()
}

fn say(step: &str, outcome: &Expected, decision: Option<&Decision>) {
    let verdict = decision.map(|d| format!("{:?}", d.verdict)).unwrap_or("—".into());
    println!("  {step}");
    println!("      seam      : {outcome:?}");
    println!("      verdict   : {verdict}");
    if let Some(d) = decision {
        println!("      because   : {}", d.reason);
        if !d.checked.is_empty() {
            println!("      checked   : {}", d.checked.join(", "));
        }
        println!("      policy    : {}", d.policy_revision);
    }
    println!();
}

#[tokio::main]
async fn main() {
    println!("\n── procurement authority ──────────────────────────────────────────────\n");
    println!("Approved declaration : the buyer agent may raise purchase orders, under a ceiling");
    println!("Reviewed mapping     : cat-procurement r7 binds tools/call + {PURCHASE}");
    println!("Deployed policy      : {POLICY_REVISION}\n");

    let policy = deployed_policy();
    let ceiling = 50_000u64;
    let now = 2_000;

    // ── 1. within the remit ───────────────────────────────────────────────────────────────────
    let within = purchase("a-1", 20_000, ceiling);
    let (o1, d1) = ae_contract::outcome_of(&policy, &within, now);
    say("1. a £200.00 order, inside the approved ceiling", &o1, d1.as_ref());
    assert_eq!(o1, Expected::Admitted);

    // ── 2. over the ceiling ───────────────────────────────────────────────────────────────────
    let over = purchase("a-2", 250_000, ceiling);
    let (o2, d2) = ae_contract::outcome_of(&policy, &over, now);
    say("2. a £2,500.00 order, over the ceiling", &o2, d2.as_ref());
    assert_eq!(o2, Expected::Denied, "a ceiling is a prohibition: an authority decided no");

    // ── 3. nobody wrote a rule ────────────────────────────────────────────────────────────────
    let unruled = envelope(BUYER, "tool:sell_asset@procurement", "op-sell", "a-3");
    let (o3, d3) = ae_contract::outcome_of(&policy, &unruled, now);
    say("3. selling an asset — no rule covers it", &o3, d3.as_ref());
    assert_eq!(
        o3,
        Expected::NotEstablished,
        "an uncovered action is NOT a denial; reporting it as one would invent a violation",
    );
    println!("      ↳ refused, but the reason is *authority not established*. A page that read");
    println!("        this as a policy violation would be reporting drift that never happened.\n");

    // ── 4. the steward intervenes ─────────────────────────────────────────────────────────────
    let halt = envelope(STEWARD, HALT, "op-intervene", "i-1");
    let (o4, d4) = ae_contract::outcome_of(&policy, &halt, now);
    say("4. the steward halts purchasing — a different principal", &o4, d4.as_ref());
    assert_eq!(o4, Expected::Admitted);

    // ── 5 & 6. the evidence ───────────────────────────────────────────────────────────────────
    let dir = std::env::temp_dir().join(format!("coop-procurement-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place for the journal");
    let path = dir.join("evidence.log");

    let denial = d2.clone().expect("a decision");
    let permit = d1.clone().expect("a decision");

    // The misconfigured route: denied here, and it happened anyway because something reached the
    // supplier without passing this gateway.
    let mut gap_decided = AeEvidence::for_decision(&over, &denial, Execution::None, ENFORCEMENT_POINT);
    gap_decided.kind = RecordKind::Decided;
    let mut gap_ran = AeEvidence::for_decision(&over, &denial, Execution::Completed, ENFORCEMENT_POINT);
    gap_ran.kind = RecordKind::Execution;

    // The corrected observation: first reported completed, later established failed.
    let mut first = AeEvidence::for_decision(&within, &permit, Execution::Completed, ENFORCEMENT_POINT);
    first.kind = RecordKind::Execution;
    let first_hash = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(serde_json::to_vec(&first).expect("encode")))
    };
    let mut corrected =
        AeEvidence::for_decision(&within, &permit, Execution::Failed, ENFORCEMENT_POINT);
    corrected.kind = RecordKind::Execution;
    let corrected = corrected.correcting(&first_hash);

    {
        let journal = EvidenceJournal::open(&path, EvidenceProfile::Strict).expect("open");
        for r in [&gap_decided, &gap_ran, &first, &corrected] {
            journal
                .append(serde_json::to_vec(r).expect("encode"))
                .await
                .expect("the journal acknowledges");
        }
    }

    let records: Vec<AeEvidence> = mycelium::read_evidence_journal(&path)
        .expect("the journal survives")
        .into_iter()
        .map(|b| serde_json::from_slice(&b).expect("decode"))
        .collect();

    println!("5. a misconfigured route — denied, and it ran anyway");
    let denied_then_ran = records.iter().any(|r| {
        r.attempt_id == over.attempt_id && r.execution == Execution::Completed
    }) && records
        .iter()
        .any(|r| r.attempt_id == over.attempt_id && r.decision == DecisionKind::Deny);
    println!("      the journal holds BOTH: a denial, and an execution that completed.");
    println!("      denied-and-ran present in the evidence: {denied_then_ran}");
    println!("      ↳ this is an ENFORCEMENT GAP and is exported as one. The tempting bug is to");
    println!("        resolve it — trust the decision and report no effect, or trust the execution");
    println!("        and report a permit. Either turns the finding an operator most needs into a");
    println!("        tidy line. The gateway fronts one route; it cannot speak for the others.\n");
    assert!(denied_then_ran, "the gap must be visible in the evidence");

    println!("6. a corrected observation — history kept");
    let survivors: Vec<&AeEvidence> =
        records.iter().filter(|r| r.attempt_id == within.attempt_id).collect();
    println!("      records for this attempt : {}", survivors.len());
    println!("      the correction cites     : {}", &first_hash[..16]);
    println!("      original still on disk   : {}", survivors.iter().any(|r| r.execution == Execution::Completed));
    println!("      corrected reading        : {}", survivors.iter().any(|r| r.is_correction() && r.execution == Execution::Failed));
    println!("      ↳ a correction SUPERSEDES; it never edits. A record that could be revised in");
    println!("        place could be revised after someone read it.\n");
    assert_eq!(survivors.len(), 2, "both readings are retained");

    // ── the conformance check an operator runs ────────────────────────────────────────────────
    println!("── checking the evaluator against the contract ────────────────────────\n");
    let report = ae_contract::run(&ReferenceUnderTest);
    println!("  {}", report.summary());
    println!("  conformant: {}", report.conformant());
    println!();
    println!("  This is what you run against YOUR evaluator before trusting it. The fixtures state");
    println!("  the seam's contract, not this implementation's shape — `mycelium::ae_contract`.");
    println!("  {}\n", ae_contract::COVERAGE);
    assert!(report.conformant());

    // ── what this does NOT establish ──────────────────────────────────────────────────────────
    println!("── what this does not establish ───────────────────────────────────────\n");
    println!("  • Not hard prevention. A gateway decision is a preflight at ONE route. Step 5 is");
    println!("    what that looks like when another route exists.");
    println!("  • Not coverage of routes this gateway does not front — the evidence names them");
    println!("    rather than implying an all-clear.");
    println!("  • Not a claim about arguments the policy did not declare: they never crossed.\n");

    let _ = std::fs::remove_dir_all(&dir);
    println!("All assertions passed — a remit enforced at one route, and the gap where it is not.\n");
}
