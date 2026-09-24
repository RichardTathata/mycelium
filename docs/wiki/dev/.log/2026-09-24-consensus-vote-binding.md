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


---

## Two more, found while verifying the above

### The gateway write succeeds at writing nothing

`POST /gateway/kv` reads **`value_b64`**. If that field is absent — misspelled, or a caller that
assumes a plain `value` — it writes `Bytes::new()` and answers **`{"ok": true}`**
(`src/agent/http.rs:2355`). A typo in a field name therefore *erases* a key and reports success.

This is not hypothetical: `tests/overlay/scenarios/helpers.py` had been posting
`{"key": …, "value": host}` since it was written, so **every readiness sentinel it ever wrote was
empty**, and every readiness check passed anyway — because the check only counted keys under a
prefix. Two failure-looks-like-success mechanisms stacked, each hiding the other.

**Recommendation:** a missing `value_b64` should be `400`, exactly as a missing `key` already is.
Writing an empty value must be something a caller asks for explicitly (an empty string encodes
fine), not what happens when the request is not understood. Tombstoning has its own verb
(`DELETE`), so silent-empty has no legitimate caller.

### The readiness check could not fail after its first success

`wait_for_cluster_ready` wrote fixed sentinel keys (`test/cluster-ready/{host}`) and polled until
*enough keys existed* under that prefix. Once those keys had propagated — in the first run of the
first scenario — every later call was satisfied by the residue, regardless of current connectivity,
and regardless of the writes having been empty. **A precondition that cannot fail after it first
passes is a decoration.**

Fixed here: a fresh nonce per attempt, the write's status checked, and every node must read back
every sentinel with its **exact expected value** (`found`, and the decoded `value_b64`). That
proves fresh bidirectional propagation at the moment of asking — which is a real precondition, and
still not a statement about consensus safety.


---

## The self-vote defeats the binding, and had to land with it

Binding votes to a value closes the *cross-proposer* counting hole. It does **not** close a node
equivocating with **itself**, and that gap would have made the binding decorative.

**The mechanism.** A node plays two roles with **two separate memories**:

- as an **acceptor**, `run_consensus_listener` keeps `seen_ballot` and `voted_value` as
  **task-local** variables, and `may_cast_vote` refuses a second value at one ballot;
- as a **proposer**, the ballot loop does `voters.insert(self.task_ctx.node_id, …)` — an
  unconditional **self-vote** that never consults that memory.

So node N can accept `v_X` at ballot 1 (from a remote proposer, broadcasting a vote bound to
`v_X`), and *simultaneously* propose `v_Y` at ballot 1 and self-vote for it. With a three-member
group and quorum two: A counts N's bound vote for `v_X` plus its own → commits `v_X`; N counts its
own self-vote plus any other → commits `v_Y`. **Two values, one ballot** — the same violation the
binding was added to prevent, reached through the one voter that never had to send a message to
vote.

**The fix is to give the two roles one memory.** Acceptor state moves onto `TaskCtx` (a `papaya`
map, so no lock-order row — the `compute` closures are compare-and-set and therefore retry-safe),
and the proposer passes through the *same* rule as every other acceptor before self-voting:
proposing a value **is** accepting it, and a node that has already accepted a different value at
this ballot cannot propose at it.

**Stated as the property:** *a node casts at most one vote per ballot, for exactly one value,
whichever role it is playing when it casts it.*


---

## Restart recovery — **built 2026-09-24**, design below as recorded

**The hole.** `TaskCtx::consensus_accepted` is in-process. A node that restarts mid-ballot forgets
what it accepted, and the property the other three changes establish — *a node casts at most one
vote per ballot, for exactly one value* — is only true **for the lifetime of the process**. Restart
it and it can vote again, for a different value, at the same ballot. Every acceptor-side guarantee
in classical consensus depends on that memory being **durable**, and ours is not.

`consensus/ballot/{slot}` is not the answer: it is a **gossiped, LWW** cluster value ("highest
ballot anyone has seen"), not this node's own record of what **it** accepted, and it carries no
value.

**The design.** Persist `(ballot, value_digest)` per slot, node-locally, and prewarm
`consensus_accepted` from it before the listener starts — the same shape as
`prewarm_peer_keys`/`start_identity_watcher` for identity.

Three decisions the implementation has to make, recorded because each has a wrong-looking-right
answer:

1. **Digest, not value.** The in-memory map holds the whole `Bytes`, but the only operation on it is
   **equality** (`may_cast_vote`). Storing the SHA-256 digest instead makes the record 40 bytes
   regardless of proposal size, which is what makes persisting it affordable at all. `claim_vote`
   compares digests; `VoteForValue` already carries one.
2. **Where it lives.** A `sys/`-prefixed key gossips, which is the wrong instinct to suppress too
   quickly: an acceptance is **already public** — votes are broadcast to the group scope — so
   publishing *"I accepted digest D at ballot B for slot S"* leaks nothing the vote did not. The
   real objection is **LWW**: a peer's write to the same key would clobber this node's own record,
   so it must be strictly self-owned (`sys/consensus-accepted/{node}/{slot}`) and read back only
   for `{self}`. A new reserved prefix means a `src/lib.rs` namespace row and the front-door lists
   the `check-kv-namespaces.sh` gate enforces.
3. **When it is written.** Before the vote is emitted, never after — a vote that reaches a proposer
   while the acceptance is unrecorded is precisely the lost memory this exists to prevent. That
   ordering is the same *"apply to the store, then hand the record to the WAL"* rule the persistence
   invariant already states, one layer up.

**What it costs, stated honestly:** a durable write on the voting path, which is the hot path. The
alternative — accepting that a restarted node may equivocate — is what the substrate does today, and
it is not defensible for a slot anyone builds exclusivity on.

### Built — and the one place the design changed under contact

`sys/consensus-accepted/{node}/{slot}` = `ballot(8, LE) ‖ digest(32)`, written **before** the vote
leaves at both roles, recovered by `prewarm_accepted` at startup before any listener can vote, and
deleted when the slot commits.

**The cost turned out lower than the design feared.** The acceptor was *already* doing a gossiped KV
write per acceptance (`consensus/ballot/{slot}`), so the durable record is one extra small write
beside an existing one, not a new write on a previously write-free path. That is worth recording
because the objection — *"a durable write on the hot path"* — was the reason to hesitate, and it was
answered by reading the path rather than by argument.

**What did change: `Promise` and the digest pull apart.** Accepted-value preservation needs the
**value** so a proposer can adopt it; the durable record wants a **digest** so it is small enough to
write. Resolved by splitting the live entry from the recovered one — `Accepted::Full(Bytes)` in
process, `Accepted::DigestOnly([u8; 32])` after a restart. A recovered node can therefore **refuse**
a conflicting vote, which is the safety property, and **cannot** report a value in a `Promise`,
which is only a liveness aid to some proposer. Safety survives the restart; that assistance does
not, and the asymmetry is the right way round.

`may_cast_vote` is now asked of a digest, so a recovered record answers it identically to a live
one. The value-taking twin was deleted rather than kept as a dead alias, and its regression gate
retargeted at the live function.

Pinned by `acceptor_memory_survives_a_restart` (write the record, lose the map, recover, find the
conflicting vote still refused), `a_recovered_acceptance_reports_no_value`, and
`a_malformed_acceptor_record_is_no_record` — malformed is **no record**, never a partial one,
because a half-understood memory refuses votes it cannot justify, which is worse than an absent
one.
