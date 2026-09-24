# Open: S12 leader election disagrees, intermittently, cause unknown — 2026-09-24

**Status: OPEN.** This entry exists because the failure briefly had a wrong owner, and a wrong owner
is worse than none — it stops people looking.

## The observation

`tests/overlay/scenarios/s12_leader_election.py` asserts that three nodes concurrently calling
`elect_leader("demo")` converge on one answer. On `8b588c6` it failed:

```
S12 leader election … AssertionError: Nodes disagree on leader:
{'overlay-a': '…0.3:57000', 'overlay-b': '…0.4:57000', 'overlay-c': '…0.4:57000'}
```

Two nodes said one thing, one said another. The federation two-mesh suite failed in the same run.
**Twelve consecutive greens** preceded it on earlier commits; **two greens** followed on later ones.

## What it is not

- **Not `require_identity_proofs`.** That commit flipped the default, and the failure was attributed
  to it for most of a day. The flag is only consulted from inside the
  `if let Some(ref tls_cfg) = self.config.tls` block in `src/agent/lifecycle.rs` — every identity
  writer and both readers (`prewarm_peer_keys`, `start_identity_watcher`) live there. The overlay
  nodes run `examples/three_node_demo.rs`, which contains **no TLS configuration at all**, and
  `tests/overlay/docker-compose.test.yml` sets no TLS env. With no TLS there are no `sys/identity/`
  entries, the validator never runs, and the flag is **inert**. Whatever `8b588c6` did to this test,
  it did not do through identity proofs — and `8b588c6` changed nothing else but docs and tests.
- **Not #369's rendezvous election rule.** That landed *after* the failing commit.

## Why the misattribution happened, since that is the reusable part

The reasoning was: *the flip was the only change in that commit, and the suite went red*. That is a
prior, not a mechanism. Nobody traced a path from the change to the assertion — which would have
taken one grep — before writing the conclusion into six documents. Recorded as rule 3 on the
[testing page](../testing/testing.md) §a green run is evidence about that run.

## Established: the group has no members, so every node decides alone

**Verified on a live three-node overlay cluster**, not inferred: `GET
/gateway/kv/keys?prefix=grp/` returns **HTTP 200 `{"keys":[]}`** on all three nodes. Nothing joins
`s12-demo` — not the scenario (`tests/overlay/scenarios/s12_leader_election.py`), not the demo
binary (`examples/three_node_demo.rs`).

So `overlay_group_propose` (`src/agent/http.rs:2830`) computes `members.len().max(1)` = **1**,
`compute_quorum_size(0, 1)` = `1/2+1` = **1**, and the proposer inserts **its own vote** before it
starts listening (`src/consensus.rs:820`). Each node therefore commits its own candidate unopposed:
three singleton elections wearing the shape of one. Most runs converge by LWW before anyone reads;
occasionally a caller reads first, and the scenario sees two answers.

**The API-level defect is the general one, and it is worth stating apart from S12:** *"I cannot see
members"* silently means *"I have authority to decide alone."* An unknown or empty group should be
an explicit error; a singleton election should require an explicit, valid singleton membership
rather than being inferred from absence.

*(Method note: the first attempt at this query returned empty because `wget` is not in the image —
a missing binary reading as an empty result. Re-run with `curl` and status codes. The same
failure-looks-like-success shape as everything else in this investigation.)*

**Still unresolved:** that this is what happened in the `8b588c6` run. The roster emptiness is
established; the causal link to that specific failure is not.

## And the reason S12 cannot simply be "fixed to join the group"

**There is no gateway route by which a node joins a group.** `grp_prefix` is *read* at
`src/agent/http.rs:2830` and written nowhere in the HTTP surface; `join_group` exists only on the
Rust `mesh()` handle. `POST /gateway/govern/membership` sets a `MembershipIntent { min, max }` —
it governs a roster's permitted *population*, it does not add a member.

So the gateway offers **election over a membership concept it provides no way to populate**, and the
only roster state reachable over HTTP is the empty one — which is precisely the state that grants
solo authority. That is why the recommended first fix ("make S12 establish the intended three-member
group") is not a test-side change: it needs a join/leave route, or a demo that joins in Rust before
serving.

It also sharpens the API finding. This is not "a test forgot to set up its group". It is a surface
on which *every* HTTP caller's election silently degrades to a singleton, because no other outcome
is reachable.

## A secondary hypothesis — the converge sleep (symptom-hider, not cause)

`elect_leader` (`src/agent/consensus_handle.rs:556`) never returns "I committed, therefore I won" —
that was fixed in the 2026-07-15 audit, because an optimistic `Committed` is not mutually exclusive.
Instead it waits for the winning commit to converge and reads the authoritative slot. The wait is a
**fixed sleep**:

```rust
ConsensusResult::Committed { .. } => {
    mycelium_core::sim_seam::sleep_ms("elect/converge", 1000).await;
    leader_from_slot(self).ok_or(ConsistencyError::Superseded)
}
ConsensusResult::Superseded { .. } =>
    leader_from_slot(self).ok_or(ConsistencyError::Superseded),
```

Two things follow, and both match the observed shape (one node dissenting, two agreeing):

1. **The 1 s is a timing assumption, and the project already says so.** The replay-nondeterminism
   inventory calls this pair *"the one whose duration is a correctness assumption"*
   (`docs/design/replay-nondeterminism-inventory.md` §2.3/§4). If gossip convergence exceeds 1 s
   under CI load, the reader returns a value the cluster has not settled on.
2. **The `Superseded` arm has no converge wait at all.** It reads the slot immediately. A node that
   lost a ballot can therefore read its local slot *before* the winner's commit has reached it and
   return whatever is there — including a value from its own earlier optimistic commit.

This is a *mechanism traced to the assertion*, which the first hypothesis lacked — but it is
**subordinate** to the findings above. The sleep decides how often disagreement is *visible*, not
whether it can happen. Lengthening it hides the defect; it does not close it. See also
[votes are not bound to what they voted for](2026-09-24-consensus-vote-binding.md), the
safety-class defect underneath all of this, which is independent of group membership.

**What would confirm it:** a failing run in which the dissenting node's `leader/s12-demo` slot agrees
with the majority when read again a few seconds later. That distinguishes "read too early" from
"genuinely committed two different values". Capture per-node `consensus_get("leader/s12-demo")` at
failure *and* 5 s after.

**What a fix would look like**, in the order I would attempt it: make the `Superseded` arm wait as
the `Committed` arm does (small, strictly closer to the existing intent); then replace *both* fixed
sleeps with a bounded poll for slot stability, so convergence is **observed rather than assumed** —
the same move as the sealed identity record, closing a race by construction instead of by timing.
Neither is done here: consensus timing deserves its own change and its own review, not a rider on a
documentation fix.

## Where to start

- The consensus path, not the identity path: `elect_leader` goes through gossip consensus, so the
  question is how three proposers converge and whether a node can conclude early on a minority view.
- The rate looks low (one in ~15 observed runs), so reproduction needs repetition:
  `make test-overlay` in a loop, capturing each node's view rather than only the assertion.
- Check whether the two-mesh federation failure in the same run shares a cause or was collateral —
  one red run, two suites, is itself a hint about the host rather than the code.
- **Do not assume it is a flake because it is rare.** "Nodes disagree on leader" is a
  correctness-class assertion; a low rate makes it harder to find, not less real.
