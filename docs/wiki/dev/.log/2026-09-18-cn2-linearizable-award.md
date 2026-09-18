## [2026-09-18] ingest | CN2 — the linearizable award, the double-award witness, and a gap pinned

Up: [dev](../dev.md) · plan `docs/plans/v3-contracts-axis.md` §6.9 (CN2 ◐) · inventory
`docs/design/replay-nondeterminism-inventory.md` §2.3 (the scheduler row, measured) · code
`mycelium-commitment/src/lib.rs`, `src/lib.rs` (`sim_seam` re-exported under `sim`).

### What landed

The award through a consensus round (`commit_award_linearizable`), the plain award split into `plan_award` /
`commit_award` so two declarers' steps can be interleaved, and the witness the plan asked for — written out as
an interleaving rather than scheduled, because the kernel cannot schedule two tasks yet. The plain path lets
both declarers commit and LWW keeps one silently; the linearizable path refuses the second with the committed
award in hand.

### Three things worth carrying forward

1. **A failing experiment is worth more pinned than deleted.** The plan says "the award replays". Recording
   a whole node under the kernel and replaying it diverged; the honest deliverable is a test that asserts the
   divergence, names the two seam requests whose order flipped (a governor's `rng jitter` draw, the round's
   `consensus/defer` timer), and will fail the day the scheduler seam lands — a pin that flips, not a
   `#[ignore]` that rots.
2. **Two divergences, two different causes, and only one is nondeterminism.** The first replay ran on a node
   with a fresh port and diverged at a gossip `try_send`: the shard is a hash of the key and the key carries
   the node id. That is a *different node*, not a nondeterministic run, and the fix is the same identity for
   both runs. The second divergence — task interleaving — is the real one, and it is the inventory's one
   unrouted row. Reading the diverging requests told the two apart; the numbers alone would not have.
3. **Refuse with the committed award in hand.** `Superseded` from the round is turned into
   `AlreadyAwarded(existing)` by reading the slot the other declarer committed, so the loser learns *who* won,
   not only that it lost. And `award_of` reads the consensus slot before the KV head, so every reader — plain
   or linearizable — sees the one award.

### Not done

The replay of an award under the kernel (needs the scheduler seam); a multi-node race (both declarers share one
node here, so consensus is local — the mechanism is shown, not the distributed race); CN3.

### Pages touched

- [history.md](../history.md) — the CN2 section.
- The inventory's §2.3 scheduler row carries the measurement; the plan's §6.9 table marks CN2 partial;
  the crate README and the companions bullet say what CN2 is and is not; CHANGELOG.
