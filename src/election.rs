//! Ring election — **who holds a single-writer role**, computed identically by every node.
//!
//! Every single-writer ring in this substrate (the tuple space's primary, the blackboard's primary,
//! the wiki's curator) picks its holder the same way: each node resolves the live candidates and
//! applies a **pure, total ordering** to them. Nothing is assigned; the answer is *derived*, which
//! is what lets it be **checked rather than trusted** and why there is no election coordinator.
//!
//! # Why the ordering changed
//!
//! The original ordering was *lowest node id wins*. It is correct, and for one ring it is fine. The
//! problem is only visible **across** rings: the same rule over the same candidates returns the same
//! winner, so a fleet where every node runs every companion puts **every single-writer job on one
//! node** — a coordinator nobody declared, whose loss moves every role at once, as a block, to the
//! next lowest id. Nothing prevented it and, until P10, nothing detected it either
//! (`docs/design/legible-emergence-taxonomy.md`).
//!
//! [`Rule::Rendezvous`] hashes the **ring's own name** into the ordering, so different rings order
//! the same candidates differently and the winners spread. This is rendezvous (highest-random-weight)
//! selection, and it keeps every property the old rule had: pure, total, deterministic, computable
//! by every node from data it already holds.
//!
//! **It is a spread, not a bound.** With three rings over three nodes there is still roughly an 11%
//! chance one node wins all three, and a ~78% chance somebody holds two. Concentration becomes
//! unlikely rather than impossible, which is exactly why P10 stays: mitigate the cause, *and* keep
//! the ability to see the residue.
//!
//! # The rollout problem, and how it is solved here
//!
//! Changing an election rule is not like changing an implementation detail. The companions' safety
//! rests on **every node computing the same answer** — the wiki's split-brain sentinel says so in as
//! many words: *"the lowest always sees itself as lowest and stays; every other steps down."* Deploy
//! a new rule node-by-node and, mid-rollout, an old node and a new one each believe they should hold
//! the role, and **neither resigns**: a stable two-holder state lasting as long as the rollout.
//!
//! So the rule is **negotiated from the candidate set itself**. Each candidate advertises the
//! highest rule it can compute as a capability attribute ([`RULE_ATTR`]); every elector uses
//! [`negotiated`], the **minimum across live candidates**. A candidate advertising nothing is an old
//! node and pins the ring to [`Rule::LowestId`]. No flag day, no operator step: the ring flips by
//! itself once the last old candidate is gone.
//!
//! **What this does not eliminate**, stated because it would be easy to imply otherwise: nodes see
//! the candidate set converge at slightly different moments, so there is a window in which one node
//! still sees an old candidate and another does not, and they compute different rules. That window
//! is bounded by *capability convergence* — seconds — rather than by the rollout, and it is the same
//! transient the sentinel already exists to resolve (it re-evaluates every tick, and as views
//! converge the rule converges, and then the holders do). A flag day's window is hours.

use crate::capability::{CapValue, Capability};
use crate::node_id::NodeId;

/// The capability attribute a candidate advertises to say which election rules it can compute.
///
/// An integer: the **highest** [`Rule`] this node understands. Absent means a node predating
/// negotiation, which is treated as [`Rule::LowestId`] — the only rule it can be relied on to agree
/// with.
pub const RULE_ATTR: &str = "election_rule";

/// How a ring orders its candidates. Ordered by version: `negotiated` takes the minimum.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Rule {
    /// Lowest node id wins. The original rule; kept because a mixed fleet must still agree, and
    /// because it is the only rule a node predating negotiation can compute.
    LowestId = 1,
    /// Lowest `hash(ring, node)` wins — rendezvous selection, so different rings pick different
    /// winners from the same candidates.
    Rendezvous = 2,
}

impl Rule {
    /// The wire/attribute form.
    pub fn as_u64(self) -> u64 {
        self as u64
    }

    /// Parse an advertised value. Anything unrecognised — a future rule this node cannot compute,
    /// or a malformed attribute — reads as [`Rule::LowestId`], which is the **fail-safe** direction:
    /// a node must never claim to agree with an ordering it does not implement.
    pub fn from_advertised(v: u64) -> Rule {
        match v {
            n if n >= 2 => Rule::Rendezvous,
            _ => Rule::LowestId,
        }
    }

    /// The highest rule this build can compute — what a candidate advertises.
    pub const CURRENT: Rule = Rule::Rendezvous;
}

/// FNV-1a over `ring`, a separator, and `node` — the rendezvous weight.
///
/// Written out rather than taken from a library because this value must be **identical on every
/// node, in every build, forever**: a hasher with a per-process seed (`DefaultHasher`) would make
/// each node compute a different winner, which is the one thing an election must never do. FNV-1a
/// is specified by these four lines, so agreement does not depend on a dependency's version.
///
/// **Not a security primitive.** A node that could choose its own id could grind for a low weight
/// on a chosen ring. That was already true, and more cheaply, under *lowest id wins* — where the
/// attack is "pick a low address" and wins **every** ring at once, rather than one ring per grind.
fn weight(ring: &str, node: &NodeId) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x1000_0000_01b3;
    let mut h = OFFSET;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(PRIME);
        }
    };
    eat(ring.as_bytes());
    eat(&[0]); // separator: "ab"+"c" must not hash as "a"+"bc"
    eat(node.to_string().as_bytes());
    h
}

/// The rule a ring uses **right now**: the minimum any live candidate can compute.
///
/// The minimum, not the maximum, because agreement is the property that matters — a ring is only as
/// new as its oldest member. A candidate with no [`RULE_ATTR`] is an old node and pins the ring to
/// [`Rule::LowestId`].
///
/// An **empty** candidate set returns [`Rule::CURRENT`]: there is nobody to disagree with, and
/// returning the old rule would mean a fresh fleet started on the rule it is trying to leave.
pub fn negotiated<'a>(candidates: impl IntoIterator<Item = &'a Capability>) -> Rule {
    candidates
        .into_iter()
        .map(|cap| match cap.attributes.get(RULE_ATTR) {
            Some(CapValue::Integer(v)) if *v >= 0 => Rule::from_advertised(*v as u64),
            // Present but not an integer, or absent: an older node, or one whose advertisement we
            // cannot read. Either way we cannot rely on it computing anything but the old rule.
            _ => Rule::LowestId,
        })
        .min()
        .unwrap_or(Rule::CURRENT)
}

/// The winner of `ring` among `candidates`, under `rule`. Pure and total: every node with the same
/// candidate list and rule gets the same answer, which is the whole basis of a coordinator-free
/// election.
///
/// `ring` is the identity that makes rendezvous spread — the capability name the ring elects for
/// (`"tuple/orders.primary"`, `"wiki/council.curator"`). Two different rings **must** pass different
/// strings or they order identically and nothing is gained.
pub fn winner<'a>(ring: &str, candidates: &'a [NodeId], rule: Rule) -> Option<&'a NodeId> {
    match rule {
        Rule::LowestId => candidates.iter().min_by_key(|n| n.to_string()),
        // Ties on the weight fall back to the node id, so the ordering stays *total* — two nodes
        // whose weights collide must not leave the winner up to iteration order.
        Rule::Rendezvous => candidates.iter().min_by(|a, b| {
            weight(ring, a).cmp(&weight(ring, b)).then_with(|| a.to_string().cmp(&b.to_string()))
        }),
    }
}

/// [`winner`] with the rule negotiated from the resolved candidates — the form a companion calls.
pub fn elect<'a>(ring: &str, resolved: &'a [(NodeId, Capability)]) -> Option<&'a NodeId> {
    let rule = negotiated(resolved.iter().map(|(_, cap)| cap));
    let ids: Vec<NodeId> = resolved.iter().map(|(n, _)| n.clone()).collect();
    // `winner` borrows from `ids`, so map the answer back into `resolved` to return a borrow the
    // caller can keep.
    let won = winner(ring, &ids, rule)?.clone();
    resolved.iter().find(|(n, _)| *n == won).map(|(n, _)| n)
}

/// The attribute a candidate advertises so its peers know which rules it can compute. Attach it to
/// the *candidate* advertisement, not the role advertisement — the negotiation is over who might
/// win, not over who did.
pub fn rule_attribute() -> (&'static str, CapValue) {
    (RULE_ATTR, CapValue::Integer(Rule::CURRENT.as_u64() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(port: u16) -> NodeId {
        NodeId::new("127.0.0.1", port).unwrap()
    }
    fn cap_with(rule: Option<u64>) -> Capability {
        let c = Capability::new("tuple", "x.primary");
        match rule {
            Some(v) => c.with(RULE_ATTR, CapValue::Integer(v as i64)),
            None => c,
        }
    }

    /// **The point of the change.** The same three candidates, three different rings: under the old
    /// rule one node wins everything; under rendezvous the winners are not all the same node.
    #[test]
    fn rendezvous_spreads_winners_where_lowest_id_concentrates_them() {
        let candidates: Vec<NodeId> = (0..3).map(|i| node(50000 + i)).collect();
        let rings = ["tuple/orders.primary", "blackboard/work.primary", "wiki/council.curator"];

        let old: Vec<String> = rings.iter()
            .map(|r| winner(r, &candidates, Rule::LowestId).unwrap().to_string())
            .collect();
        assert_eq!(old[0], old[1], "lowest-id gives every ring the same winner");
        assert_eq!(old[1], old[2], "which is the coordinator nobody declared");

        let new: Vec<String> = rings.iter()
            .map(|r| winner(r, &candidates, Rule::Rendezvous).unwrap().to_string())
            .collect();
        assert!(
            new.iter().collect::<std::collections::HashSet<_>>().len() > 1,
            "rendezvous must not hand every ring to one node: {new:?}",
        );
    }

    /// Determinism is the property the whole design rests on: same inputs, same answer, on every
    /// node and every run. Order of the candidate list must not matter either — one node's resolve
    /// may return them in a different order than another's.
    #[test]
    fn the_winner_is_deterministic_and_order_independent() {
        let forward: Vec<NodeId> = (0..5).map(|i| node(51000 + i)).collect();
        let mut backward = forward.clone();
        backward.reverse();
        for ring in ["a", "b", "c/d.primary"] {
            let f = winner(ring, &forward, Rule::Rendezvous).unwrap();
            let b = winner(ring, &backward, Rule::Rendezvous).unwrap();
            assert_eq!(f, b, "candidate order must not change the winner ({ring})");
        }
    }

    /// **The rollout safety property.** One old candidate in the set pins *everyone* to the old
    /// rule, so a mixed fleet still agrees. This is what makes the change deployable without a
    /// flag day — and without the stable two-holder state a naive flip would cause.
    #[test]
    fn one_old_candidate_pins_the_whole_ring_to_the_old_rule() {
        let new_only = [cap_with(Some(2)), cap_with(Some(2))];
        assert_eq!(negotiated(new_only.iter()), Rule::Rendezvous);

        let mixed = [cap_with(Some(2)), cap_with(None), cap_with(Some(2))];
        assert_eq!(negotiated(mixed.iter()), Rule::LowestId, "a ring is only as new as its oldest member");

        // An attribute we cannot read is treated as old, never as agreement.
        let malformed = [cap_with(Some(2)), Capability::new("tuple", "x.primary")
            .with(RULE_ATTR, CapValue::Text("two".into()))];
        assert_eq!(negotiated(malformed.iter()), Rule::LowestId, "unreadable is not agreement");

        // A rule from the future is clamped to what this build can actually compute.
        let future = [cap_with(Some(99)), cap_with(Some(2))];
        assert_eq!(negotiated(future.iter()), Rule::Rendezvous, "never claim to compute an unknown ordering");

        assert_eq!(negotiated(std::iter::empty()), Rule::CURRENT, "an empty ring starts on the current rule");
    }

    /// The ring name must actually participate — a separator bug that let `"ab" + "c"` hash like
    /// `"a" + "bc"` would silently collapse distinct rings back onto one winner.
    #[test]
    fn the_ring_name_changes_the_ordering() {
        let candidates: Vec<NodeId> = (0..4).map(|i| node(52000 + i)).collect();
        let mut seen = std::collections::HashSet::new();
        for ring in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            seen.insert(winner(ring, &candidates, Rule::Rendezvous).unwrap().to_string());
        }
        assert!(seen.len() > 1, "eight rings over four nodes must not all pick the same node");
        assert_ne!(weight("ab", &candidates[0]), weight("a", &candidates[0]), "the separator must bite");
    }
}
