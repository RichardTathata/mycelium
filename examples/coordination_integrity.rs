//! **Coordination integrity** — an election needs an electorate, and winning is not a grant.
//!
//! ```text
//! cargo run --example coordination_integrity
//! ```
//!
//! # The claim this exists to make checkable
//!
//! Three sentences that are easy to nod at and easy to get wrong in code. Each is a numbered act
//! below, and each **fails the program** if the substrate stops honouring it.
//!
//! | # | The claim | What would happen without it |
//! |---|---|---|
//! | 1 | *"I cannot see members" is not "I may decide alone"* | an empty roster counts as one member, quorum one, satisfied by the proposer's own vote — every node elects **itself** |
//! | 2 | An answer **names the rung it reached** | a bare node id cannot distinguish *a quorum chose me* from *this is what my replica says* |
//! | 3 | Exclusivity is enforced **at the resource**, by a fence | two callers each told they won both act, and LWW cannot undo work already done |
//!
//! # Why an election is not a grant
//!
//! A successful election is a **decision about a value at a ballot**. It is not a lease, and
//! nothing keeps it true: leadership can be superseded at any later ballot, and no coordinator-free
//! protocol can promise otherwise. That is not a gap in this substrate — it is what *coordination
//! without a coordinator* costs, and the honest response is to make the cost visible in the types
//! rather than paper it with a sleep.
//!
//! So the instrument for exclusivity is not the leader's identity, and not the fact that the call
//! returned `Ok`. It is the **fencing token** — `Leadership::epoch`, the commit's HLC, monotonic
//! across successive holders. A resource that refuses a lower token is genuinely fenced. A resource
//! that trusts the caller's word is not, however the election went.

use mycelium::{
    ConsensusConfig, ConsistencyError, GossipAgent, GossipConfig, LeadershipBasis, NodeId,
};
use std::sync::Arc;

/// A resource that can actually be fenced: it remembers the highest token it has honoured and
/// refuses anything not strictly greater.
///
/// This is the whole discipline, and it lives **here**, in the application — not in the election.
/// Four lines of state is the difference between "we elected a leader" and "only one writer can
/// have effect".
struct FencedResource {
    highest_seen: u64,
    writes:       Vec<String>,
}

impl FencedResource {
    fn new() -> Self { Self { highest_seen: 0, writes: Vec::new() } }

    /// Accept a write only from a token at least as high as the highest honoured so far.
    fn write_fenced(&mut self, token: u64, what: &str) -> Result<(), String> {
        if token < self.highest_seen {
            return Err(format!(
                "refused: token {token} is stale (already honoured {})", self.highest_seen));
        }
        self.highest_seen = token;
        self.writes.push(what.to_string());
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    // A fixed port: this example is a narrative, not a cluster — one node is the whole electorate.
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57310);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).expect("node id"), cfg));
    agent.start().await.expect("start");
    let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());

    const RING: &str = "depot-dispatch";

    // ── Act 1 — absence is not authority ────────────────────────────────────────────────────
    //
    // Nobody has joined `depot-dispatch`. A substrate that treated an unseen roster as "one
    // member, quorum one" would elect this node its leader on the strength of its own vote — and
    // so would every other node, separately, each believing itself the winner.
    println!("── 1 · an election with no electorate ──");
    match agent.consensus().elect_leader_receipt(RING).await {
        Err(ConsistencyError::ElectorateUnavailable { observed_members, declared_min }) => {
            println!("   refused: {observed_members} members visible, {declared_min} declared");
            println!("   ✓ \"I cannot see members\" did not become \"I may decide alone\"");
        }
        Ok(l) => panic!(
            "REGRESSION: elected {} from an empty roster — every node would elect itself", l.leader),
        Err(e) => panic!("expected an electorate refusal, got {e}"),
    }

    // ── Act 2 — the answer names its rung ───────────────────────────────────────────────────
    //
    // Joining makes the electorate explicit. A one-member group is a legitimate electorate: what
    // was refused above is *inferring* authority from absence, never solo authority somebody
    // actually established.
    println!("\n── 2 · with an explicit electorate ──");
    agent.mesh().join_group(RING);

    let leadership = agent.consensus().elect_leader_receipt(RING).await
        .expect("an established electorate elects");

    println!("   leader : {}", leadership.leader);
    println!("   basis  : {:?}", leadership.basis);
    println!("   epoch  : {}", leadership.epoch);

    assert_eq!(leadership.basis, LeadershipBasis::Decided,
               "our own proposal committed at quorum — the strongest rung, and it should say so");
    assert!(leadership.was_decided_here());
    assert!(leadership.epoch > 0, "the fencing token must be usable");
    println!("   ✓ `Decided` — a quorum chose this value, not merely \"the slot says so\"");

    // The legacy call agrees about *who*. What it cannot tell you is *how* — which is the whole
    // reason the receipt form exists.
    let legacy = agent.consensus().elect_leader(RING).await.expect("legacy call");
    assert_eq!(legacy, leadership.leader);
    println!("   ↳ `elect_leader` returns {legacy} — the same node, and no rung");

    // ── Act 3 — the fence is what makes it exclusive ────────────────────────────────────────
    //
    // Being elected is where the substrate's job ends. Whether two writers can both have effect
    // is decided at the resource, by whether it refuses a stale token.
    println!("\n── 3 · fencing at the resource ──");
    let mut resource = FencedResource::new();

    resource.write_fenced(leadership.epoch, "dispatch: van-3 → north depot")
        .expect("the current holder writes");
    println!("   accepted write at epoch {}", leadership.epoch);

    // Now a *stale* holder returns — a node that was leader before, whose call also returned `Ok`,
    // and which never learned it had been superseded. It has a real token. It is simply old.
    let stale = leadership.epoch - 1;
    match resource.write_fenced(stale, "dispatch: van-3 → SOUTH depot") {
        Err(why) => println!("   {why}"),
        Ok(()) => panic!("REGRESSION: a stale token was honoured — the resource is not fenced"),
    }
    println!("   ✓ the second writer was stopped by the *fence*, not by the election");

    assert_eq!(resource.writes.len(), 1, "exactly one write took effect");

    println!("\nOne writer had effect — because the resource refused a lower token.");
    println!("The election chose; the fence enforced. Those are different jobs.");

    agent.shutdown().await;
}
