# Votes are not bound to what they voted for — 2026-09-24

**Status: OPEN, unfixed. Safety class, not liveness.** Found by a third-party source review of the
S12 investigation; verified here against `origin/main` @ `7cb3b87`.

Stated in the three-part form this investigation should have used from the start:

- **Observed.** S12 (`leader election`) disagreed once at `8b588c6`: three nodes, two answers.
- **Established (source-verified).** A vote message does not identify the proposal it voted for, and
  concurrent proposers collide on ballot numbers by construction, so one voter's single vote can be
  counted by several proposers toward **different** committed values at the same ballot.
- **Unresolved.** Not demonstrated with a running test, and not shown to be the cause of that
  specific failure. The conflicting-proposal test below is what would settle it.

## The four parts, each checked

1. **The vote carries no value.** `ConsensusMsg::Vote { slot, ballot, voter }` — and
   `VoteWithLocality` adds only a `LocalityPath` (`src/consensus.rs:256`, `:278`). There is no value,
   no digest, no proposer id.
2. **Collection matches on `(slot, ballot)` alone**, then commits *the proposer's own* `value`
   (`src/consensus.rs:1259` → `try_commit_if_ready(..., value, ...)`). `voters.insert(voter, …)` —
   nothing checks what that voter accepted.
3. **Ballots collide by construction.** `let mut ballot = self.read_ballot(&ballot_key) + 1`
   (`src/consensus.rs:752`, `:994`) reads a **shared KV key**, not node identity. Three nodes
   starting an election concurrently all read the same value and all propose ballot `1`.
4. **Votes are broadcast, not unicast.** The voter emits to `sig.scope` — the group scope
   (`src/consensus.rs:1484`). Every proposer listening on `consensus_kind::VOTE` receives it.

## The consequence

In a **properly populated** 3-member group (`compute_quorum_size(0, 3)` = `3/2+1` = 2):

- A proposes `v_A` at ballot 1; B proposes `v_B` at ballot 1.
- C accepts whichever it sees first — say `v_A` — and broadcasts `Vote{slot, 1, C}`.
- A counts: self + C = 2 = quorum → commits `v_A`.
- B counts: self + C = 2 = quorum → commits `v_B`.

**Two different values committed at the same ballot.** The anti-equivocation check stops C from
*accepting* two values at one ballot; it cannot stop the *counting*, because the vote does not say
what C accepted. This is independent of group membership being empty
([the other finding](2026-09-24-s12-leader-election-open.md)) — fixing that does not close this.

## What the API currently promises, and should not

`elect_leader` and `LockService` read as exclusive grants. LWW reconciliation can decide which
*record* survives; it cannot un-do work two callers each performed after being told they had won.
Two capabilities are being conflated:

| Capability | What a success means |
|---|---|
| **Convergent leader preference** | nodes eventually settle on one answer; temporary disagreement is normal |
| **Exclusive leadership / ownership** | conflicting successful grants cannot authorise simultaneous holders in one scope and epoch |

The substrate can offer both, but they need different contracts, and today's success value is the
first being read as the second. Increasing `elect/converge`'s 1 s sleep only changes how often the
difference is visible — which is why that sleep is a symptom-hider, not the cause.

## The fix, in the order to attempt it

1. **Bind votes to the proposal**: value digest **and** membership epoch in `Vote`, checked at
   collection. New message shape ⇒ a wire consideration; old votes must not count for a new
   proposal.
2. Re-check the neighbouring safety rules **together**, not piecemeal: higher-ballot transitions,
   the proposer's self-vote, restart recovery, membership change mid-ballot.
3. Only then revisit the converge sleeps.

**The test that settles it**, and that should exist before any fix: two proposers, one slot, the
same ballot, different values, one shared voter — assert that a vote for A **cannot** contribute to
B's quorum. Deterministic, no timing.
