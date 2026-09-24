//! **A coordinator nobody declared** — how every single-writer role lands on one node, and what
//! stops it.
//!
//! ```text
//! cargo run --example coordinator_by_accretion
//! ```
//!
//! # The claim this exists to make checkable
//!
//! A fleet where every node runs every companion puts the tuple-space curator, the blackboard
//! primary and the wiki curator **on the same node** — not by accident, and not rarely.
//!
//! Each ring elects independently. If they all order candidates the same way, **the same candidate
//! wins every time**: the rule is deterministic, and determinism over an identical candidate set is
//! a constant. Restart the fleet and it happens again, identically. Nobody declared that node a
//! coordinator; it arrived by accretion.
//!
//! | | Arm A — *lowest id wins* | Arm B — *rendezvous* |
//! |---|---|---|
//! | how a ring orders candidates | by node id, the same for every ring | by `hash(ring, node)` — **the ring's own name** participates |
//! | three rings over three nodes | one node holds **all three** | the load spreads |
//! | on failover | every role moves to the next id **as a block** | each ring re-picks independently |
//!
//! # Why no existing detector could see it
//!
//! This is the part worth sitting with. The fleet above is not *degraded* in any way the substrate
//! was watching for:
//!
//! - **P2** watches role **churn** — and a node calmly holding everything produces none.
//! - **P6** watches coverage **gaps** — and here every capability has a provider. The same one.
//!
//! Both are looking down an orthogonal axis. A maximally concentrated fleet reads as **perfectly
//! healthy on every measurement that existed**, right up until the node it all depends on goes
//! away. That is why **P10** exists, and why it stayed after rendezvous fixed the cause:
//! rendezvous is a **spread, not a bound**.

use mycelium::NodeId;
use mycelium::election::{winner, Rule};

/// The three single-writer rings a fully-provisioned co-op node would join.
const RINGS: [&str; 3] = [
    "tuple/depot.primary",
    "blackboard/depot.primary",
    "wiki/depot.curator",
];

/// What share of the single-writer roles does the busiest node hold?
///
/// This is what P10 reports as `role_concentration_pct`. 100 means one node holds every
/// single-writer job in the fleet.
fn concentration_pct(holders: &[String]) -> u64 {
    let mut counts = std::collections::HashMap::new();
    for h in holders {
        *counts.entry(h.clone()).or_insert(0u64) += 1;
    }
    let most = counts.values().copied().max().unwrap_or(0);
    most * 100 / holders.len() as u64
}

fn show(rule: Rule, nodes: &[NodeId]) -> Vec<String> {
    let mut holders = Vec::new();
    for ring in RINGS {
        let w = winner(ring, nodes, rule).expect("a non-empty candidate set elects").to_string();
        println!("   {ring:28} → {w}");
        holders.push(w);
    }
    holders
}

fn main() {
    let nodes: Vec<NodeId> = (0..3)
        .map(|i| NodeId::new("127.0.0.1", 57400 + i).expect("node id"))
        .collect();

    println!("Three nodes, all equally capable, each running every companion:");
    for n in &nodes { println!("   {n}"); }

    // ── Arm A — the rule that produced the pathology ────────────────────────────────────────
    println!("\n── A · lowest candidate id wins ──");
    let a = show(Rule::LowestId, &nodes);
    let a_pct = concentration_pct(&a);
    let a_distinct = a.iter().collect::<std::collections::HashSet<_>>().len();
    println!("   → {a_distinct} distinct holder(s); concentration {a_pct}%");

    assert_eq!(a_distinct, 1, "the whole point: one rule, one candidate set, one winner");
    assert_eq!(a_pct, 100);
    println!("   ⚠ one node holds every single-writer role — and P2 (churn) and P6 (gaps)");
    println!("     both read this fleet as healthy, because neither looks at this axis");

    // ── Arm B — the ring's own name breaks the tie ──────────────────────────────────────────
    println!("\n── B · rendezvous: hash(ring, node) ──");
    let b = show(Rule::Rendezvous, &nodes);
    let b_pct = concentration_pct(&b);
    let b_distinct = b.iter().collect::<std::collections::HashSet<_>>().len();
    println!("   → {b_distinct} distinct holder(s); concentration {b_pct}%");

    assert!(b_distinct > 1, "rendezvous must not reproduce the single-holder outcome here");
    assert!(b_pct < a_pct, "concentration must fall");
    println!("   ✓ the same candidates, spread — because the ring name participates in the order");

    // ── What is deliberately *not* claimed ──────────────────────────────────────────────────
    //
    // Rendezvous makes concentration unlikely, not impossible. Over three rings and three nodes
    // there is still roughly an 11% chance one node wins all three, and ~78% that somebody holds
    // two. A spread is not a bound — which is exactly why the detector stays.
    println!("\n── what this does NOT claim ──");
    println!("   Rendezvous is a spread, not a bound: three rings over three nodes still leave");
    println!("   ~11% chance one node wins all three. Concentration becomes unlikely, not");
    println!("   impossible — so P10 keeps reporting `role_concentration_pct`, and an operator");
    println!("   still sees the residue.");
    println!("\n   Mitigate the cause; keep the ability to see what is left.");

    // ── And the rule is negotiated, not flag-dayed ──────────────────────────────────────────
    //
    // A mixed fleet must still agree. Every candidate advertises the rule it can compute, and each
    // elector takes the *minimum* across live candidates: a ring is only as new as its oldest
    // member. Changing the rule node-by-node would leave two holders each believing itself
    // correct, and neither resigning — a stable split for the length of the rollout, in the one
    // place the design has no coordinator to break the tie.
    println!("\n── rollout ──");
    println!("   The rule is negotiated from the candidate set (minimum across live candidates),");
    println!("   never flag-dayed: a ring is only as new as its oldest member. Current rule is");
    println!("   {:?}.", Rule::CURRENT);
}
