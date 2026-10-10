# 04 — Consensus: quorum agreement on demand

> **Read this before using it for anything exclusive.** Layer III gives you value-bound votes, a
> leased commit and a fencing token — *agreement*, not the textbook "strong consistency". The quorum
> is `floor(n/2)+1` over the members **this node observes**, so two nodes with different views can
> each count a majority. The **supported profile** for a lock, a leader or a single writer is stated
> once, in the [threat model §7](../threat-model.md#7-safety-sensitive-agreement-the-supported-profile),
> and checked in [production-readiness.md](../operations/production-readiness.md): fixed
> `quorum_size`, `use_trust_slices` with every voter declaring the same set, opacity not counted as
> absence, membership changes outside the profile — and exclusivity enforced **at the resource** by
> refusing a stale fencing token. Everything below is the mechanism; that paragraph is the contract.

## Concept

Mycelium's default is eventual consistency: fast, partition-tolerant, no
coordinator. But some operations need ballot-serialized ordering — a counter that must not
tick twice, a lock that must be held by exactly one node, a leader election
that must produce exactly one winner.

Rather than making the whole system pay for consensus, Mycelium provides a
thin **consistency overlay** that builds ballot-serialized operations on top
of the eventual-consistency substrate. You pay for consensus only where you actually
need it — and everything outside the overlay keeps its gossip-speed
performance.

The overlay uses epidemic voting: a proposer broadcasts to the group, each
node votes, and the proposal commits when a quorum of votes accumulate in the
KV store. There is no distinguished leader for the consensus protocol itself
(though `elect_leader` can nominate one for your application logic).

```mermaid
sequenceDiagram
    participant P as Proposer
    participant A as Voter A
    participant B as Voter B
    participant C as Voter C

    P->>A: Prepare(slot=42, ballot=b)
    P->>B: Prepare(slot=42, ballot=b)
    P->>C: Prepare(slot=42, ballot=b)
    Note over A,C: Prepare goes to the whole scope;<br/>each answer goes back to P alone
    A-->>P: PrepareAck(promised b; accepted: none)
    B-->>P: PrepareAck(promised b; accepted: none)
    Note over P: P counts itself — with A and B a quorum (3 of 4) promised;<br/>nothing accepted, so P may carry its own value "x"<br/>(else the highest-ballot accepted value)
    P->>A: Propose(slot=42, ballot=b, value="x")
    P->>B: Propose(slot=42, ballot=b, value="x")
    P->>C: Propose(slot=42, ballot=b, value="x")
    A-->>P: Vote(b, digest("x"))
    C-->>P: Vote(b, digest("x"))
    Note over A,C: votes are broadcast to the group;<br/>only the proposer counts them
    Note over P: P's own vote, A and C — 3 of 4, bound to "x"
    P->>A: Commit(slot=42, b, "x")
    P->>B: Commit(slot=42, b, "x")
    P->>C: Commit(slot=42, b, "x")
    Note over A,C: committed/42 and decided/42 written to KV<br/>acceptor memory kept (2.30.0)
```

**Available operations**

| Operation | Handle | Description |
|-----------|--------|-------------|
| `consistent_set(key, value)` | `consensus()` | Ballot-serialized write — committed only after quorum accept; quorum is a majority of current peers |
| `consistent_get(key)` | `consensus()` | Read latest ballot-committed value visible to this node (local, lease-aware) |
| `append(stream, entry)` | `kv()` | Append to an ordered log — entries keyed by HLC, so ordering is causal and cluster-wide unique |
| `scan_log(stream, from, to)` | `kv()` | Range scan the log by HLC window |
| `distributed_lock(name, ttl)` | `consensus()` | Acquire an exclusive lock (returns a `LockGuard`); TTL prevents deadlock |
| `elect_leader(group)` | `consensus()` | Nominate one node as leader — returns the id only, so it cannot tell you *how* it knows |
| `elect_leader_receipt(group)` | `consensus()` | **Prefer this.** Returns `Leadership { leader, epoch, basis }` — the rung the answer reached, and a fencing token |
| `join_group(name)` / `leave_group(name)` | `mesh()` | Join or leave a group. **An election needs an electorate**: a group nobody has joined is refused, not decided alone |
| `emit_reliable(kind, scope, payload)` | `service()` | Signal with explicit ACK |

---

## What a successful election means

**An election needs an electorate.** A group whose roster this node cannot see — unknown, or one
nobody joined — is **refused** (`ElectorateUnavailable`), not decided alone. **And the proposer must be in it**: a
node that is not in the group's roster is refused by name (`NotAMember`, 2.32.0) rather than counting its own vote
toward a quorum the roster does not contain. Join the group first:

```rust
agent.mesh().join_group("my-group");            // embedded
// or over HTTP: POST /gateway/mesh/group {"group":"my-group"} (mesh:write) — or, for a group under a membership
// intent (a governed group), POST /gateway/govern/group (govern:write); the mesh route refuses it 403.
```

Before 2026-09-24 an empty roster counted as one member with a quorum of one, satisfied by the
proposer's own vote — so every node committed its own candidate and the fleet agreed only if gossip
happened to reconcile before anyone looked. *"I cannot see members"* must not mean *"I have
authority to decide alone."* An **explicit** one-member group still elects normally; what is refused
is inferring authority from absence.

**Then read the rung, not just the name:**

```rust
let l = agent.consensus().elect_leader_receipt("my-group").await?;
match l.basis {
    LeadershipBasis::Decided  => { /* a quorum chose us, at this ballot, bound by digest */ }
    LeadershipBasis::Observed => { /* this is what our replica says; fine for following */ }
}
```

**Neither rung is an exclusive grant that stays true**, and no coordinator-free protocol can offer
one — leadership can be superseded at any later ballot. If you need exclusivity, **fence on
`l.epoch`** at the resource: it is the commit's HLC, monotonic across successive holders, so a
resource that refuses a lower token is genuinely fenced. Do not fence on the ballot, which regresses
under gossip lag.

The distinction is not pedantry. LWW can decide which *record* survives; it cannot undo work two
callers each performed after being told they had won. *Convergent leader preference* and *exclusive
ownership* are two capabilities, and only the second needs a fence.

**Changing an electorate.** The prepare phase makes every later proposer learn what a quorum
accepted — *provided its promise quorum shares an acceptor with that accept quorum*. Quorums are
counted, not agreed: each proposer computes its quorum from the roster it sees and counts any
member's promise or vote, including a node that has just joined and is in nobody's roster yet. Two
quorums must share an acceptor only if together they exceed the number of nodes that can answer.
Views that differ by two or more members (a swap counts as two), an unannounced joiner, or a fixed
`quorum_size` at or below half the group can each break that, and then two proposers can commit
different values. That is why the [supported profile](../threat-model.md#7-safety-sensitive-agreement-the-supported-profile)
fixes the membership for the life of a slot and the quorum as a strict majority of it. If you
change a governed group anyway: one node at a time, letting the roster converge on **every** node
(`GET /gateway/mesh/group?group=G` answers for the node you ask) before the next change; and fence
exclusive work on the epoch, with the resource refusing any token not greater than the last it
accepted. A conflict that does happen is counted in `commit_conflicts` by a member that receives the
second COMMIT while holding the first — check every node; one that learned both values by gossip
counts nothing — and LWW converges every node to whichever value was re-stamped last. Both
proposers may have been told they won. A versioned electorate (membership epochs, joint-consensus
transitions) is recorded as protocol work for a later plan, not built — see *Discovery is not an
electorate* below.

**Run it.** All three claims as one narrative, each act asserting:

```bash
cargo run --example coordination_integrity
```

It elects on a group nobody joined (refused), joins and elects (`Decided`, with an epoch), then has
a *stale* holder — one whose own call also returned `Ok` and which never learned it was
superseded — try to write. The fence stops it. The election did not, and was never going to: those
are different jobs.

---

## Discovery is not an electorate

Two capabilities that are easy to conflate, and Mycelium keeps apart **by design**
([decision record](../design/consensus-electorate.md)):

- **Discovery** — gossip membership, capability groups, emergent groups, the elastic membership
  governor — answers *who is here and what can it do*. It is dynamic and eventually consistent:
  nodes join and leave without anyone's permission.
- **The electorate** answers *whose votes decide this slot*. For a lock, a leader or any exclusive
  outcome it is a **fixed set for the life of the decision** — the
  [supported profile](../threat-model.md#7-safety-sensitive-agreement-the-supported-profile).
  Quorum intersection is a property of that set, never of discovery converging.

A group's roster serves as the electorate only while you hold it still. What the code enforces:
a roster this node cannot see, or one below a fresh `MembershipIntent.min`, is refused
(`ElectorateUnavailable`); a proposer outside the group is refused (`NotAMember`); and a
**governed group** — one under a live membership intent — moves only through
`POST`/`DELETE /gateway/govern/group` (`govern:write`, audited), while `/gateway/mesh/group` refuses
it `403 governed_group`.

**An electorate group** (post-360 plan row P2) is the governed group made fixed — by identity and by
epoch, and changed only by its own agreement. Declare it from its roster:

```rust
// every member joined first (mesh().join_group / POST /gateway/govern/group), each running the listener
let e = agent.declare_electorate("ledger-council", true).await?;   // or POST /gateway/govern/electorate
assert_eq!(e.epoch, 1);                                              // {"group":"ledger-council","exclusive_default":true}
```

The first declaration is **genesis**: the roster this node sees becomes the member set, and every
member must accept it. After that each declaration is a **step** — the next epoch, the roster as the
member set — which must differ from the current electorate by **one** member (`StepTooLarge`
otherwise) and is decided by a strict majority of the **current** members. Two members stepping at
once are one decision: at most one commits. Each epoch's record carries a certificate of the votes
that decided it, and a node adopts an epoch only when the certificate verifies — a record written
straight into the store is not adopted (`electorate_records_refused` counts it).

From genesis on: the membership governor does not roll on the group and the emergent watcher does
not join or leave it; a proposal to it is **refused unless the roster equals the epoch's members by
identity** (`ElectorateMismatch`, counted by `electorate_roster_mismatches` — a swap included, and an
embedded `join_group` or `grp/` write is refused at the next proposal, not prevented); promises and
votes count only from the members; the quorum is at least a strict majority, whatever `quorum_size`
or opacity say; and every member answers only a proposal naming its own epoch — a proposer that has
not learned a step is refused by those that have (`ElectorateStale`), and a member that has accepted
a step stops answering the epoch before it. So a change is: move one node
(`/gateway/govern/group`), then declare — and the group decides nothing in between.

**What that guarantees, exactly.** A step **drains** the epoch it leaves: its promise fences every
member that gives it, each answers with what it holds for the group's slots, and the step's proposer
runs each reported slot to completion at the old epoch before it proposes the step. So every value
that may have been chosen — **committed or not** — is held by a majority of each epoch in turn, and a
proposer at any later epoch adopts it: single-decree safety per slot across any number of one-member
steps. Within an epoch at most one value commits; a proposer at a superseded epoch cannot complete. The
drain is bounded — 256 open slots of a group per member, 4 MiB of values — and past it the step is
refused by name (`DrainRefused`, `409 drain_refused`) and the electorate does not move
([decision record §8.3](../design/consensus-electorate.md#83-exactly-what-holds--and-why-drain-before-step)).

**Requiring it.** Set `consensus_require_electorate = true` (env `GOSSIP_CONSENSUS_REQUIRE_ELECTORATE`;
the `secure-single-domain` profile requires it, rev 4). A **safety-sensitive** proposal — flagged
`ConsensusConfig::safety_sensitive`, or in the `lock/`, `leader/`, `consistent/` families that
`distributed_lock`, `LockService`, `elect_leader`, `consistent_set` and their gateway routes use — is
then refused `ElectorateNotGoverned` (gateway `403 electorate_not_governed`) unless its scope is an
electorate group. The cluster-scoped exclusive verbs (locks, `consistent_set`, the log claim,
`mycelium-commitment`'s award, `set_capability_authz_via_consensus`) decide in the group marked
`exclusive_default` — a fleet record, so every node agrees; `consensus_electorate` may only restate
it, and a node whose setting disagrees refuses them by name. Those slots have one home: a `lock/`,
`consistent/`, `capauthz/`, commitment-award or log-claim slot proposed on any other group — and a
`leader/{g}` slot on any group but `g` — is refused `ElectorateMismatch`. The proposer must be a member, so a lock
is taken by an electorate member (a non-member asks through a member's gateway). Off — the default —
nothing changes, and such a proposal is counted (`mycelium_consensus_ungoverned_safety_total`). So for
exclusive work: declare the electorate, require it, and fence at the resource.

Consensus here is a **protocol run by whichever nodes are in the group**, never a service: no
deployment shape makes a named node set "the consensus nodes". Versioned electorates with
joint-consensus transitions — an electorate that can change under a live slot — are a later plan,
not built.

---

## The Example

`examples/three_node_demo.rs` includes an `overlay` role that exposes all
consensus operations as HTTP endpoints. The `tests/overlay/` directory
contains Python scripts that exercise consistent_set, distributed_lock, and
elect_leader against a live three-node cluster.

**Prerequisites**

```bash
cargo build --example three_node_demo
```

**Run — 3-node overlay cluster**

```bash
# Terminal 1
MYCELIUM_ROLE=overlay MYCELIUM_PORT=57010 MYCELIUM_HTTP_PORT=8400 \
  MYCELIUM_PEERS="127.0.0.1:57011,127.0.0.1:57012" \
  cargo run --example three_node_demo

# Terminal 2
MYCELIUM_ROLE=overlay MYCELIUM_PORT=57011 MYCELIUM_HTTP_PORT=8401 \
  MYCELIUM_PEERS="127.0.0.1:57010,127.0.0.1:57012" \
  cargo run --example three_node_demo

# Terminal 3
MYCELIUM_ROLE=overlay MYCELIUM_PORT=57012 MYCELIUM_HTTP_PORT=8402 \
  MYCELIUM_PEERS="127.0.0.1:57010,127.0.0.1:57011" \
  cargo run --example three_node_demo
```

**Exercise the overlay**

```bash
# Ballot-serialized write — single-decree agreement on a stable roster, not linearizable
curl -X POST http://localhost:8400/gateway/overlay/consistent/set \
  -H 'Content-Type: application/json' \
  -d '{"key":"counter","value_b64":"MQ=="}'   # "1"; a cluster-wide round — the route takes no group

# Read committed value
curl http://localhost:8400/gateway/overlay/consistent/get?key=counter

# Acquire a distributed lock (TTL 10s)
curl -X POST http://localhost:8400/gateway/overlay/lock/acquire \
  -H 'Content-Type: application/json' \
  -d '{"name":"my-lock","ttl_secs":10}'

# Append to a log stream
curl -X POST http://localhost:8400/gateway/overlay/log/append \
  -H 'Content-Type: application/json' \
  -d '{"stream":"events","value_b64":"aGVsbG8="}'   # "hello"
```

The key is yours to choose except under `sys/` and `consensus/`, which the substrate owns: those answer **403**
`protected_key` (2.26.0); since 2.27.0 every namespace the substrate or a companion owns is refused the same way, and the topology override has its own route (below).

**What to observe**

- Kill one overlay node and re-run the consistent_set — it still succeeds
  (2-of-3 quorum). Kill two and it blocks (no quorum).
- Watch `ballot/` keys appear in the KV dump (`curl
  http://localhost:8400/gateway/kv/keys?prefix=consensus/`) as votes propagate.
- Run two writes of **different** values to the same key at once from different nodes: at most one answers
  `{"ok": true, …}`; the other **409** `{"ok": false, "error": "superseded"}` — the later proposer found the
  earlier's value already committed, or accepted by a quorum, which it then committed in its own place (2.30.0)
  — or **504** on a timeout, which does not mean its value lost (read the key).

---

## How It Works

From within Rust code, the overlay operations are accessed via the consensus handle:

```rust
// Consistent write via ConsensusHandle
agent.consensus().consistent_set("config/feature-flag", &b"true"[..]).await?;

// Acquire a lock — returns a guard; drop the guard to release
let guard = agent.consensus().distributed_lock("migration-lock", Duration::from_secs(30)).await?;
// ... do the critical section ...
drop(guard);  // or let it expire after 30s

// Elect a leader for a group
let leader_id = agent.consensus().elect_leader("workers").await?;

// Append to an ordered log — synchronous; returns the entry's HLC stamp
let hlc = agent.kv().append("audit-log", entry_bytes);
let entries = agent.kv().scan_log("audit-log", 0, u64::MAX); // [from, to) HLC window
```

Voting blocs are **emergent groups**: a `CapabilityGroupDef` names a filter,
and every node that matches self-joins — `group_propose` then computes its
quorum from the group's current membership. No coordinator registers members:

```rust
// Each node evaluates this for itself; matching nodes join "overlay".
let _grp = agent.capabilities().define_capability_group(
    "overlay",
    CapabilityGroupDef {
        filter: CapFilter::new("role", "overlay"),
        topology_policy: None,    // or Some(GroupTopologyPolicy { .. })
        provides: vec![],
        requires: vec![],
    },
    Duration::from_secs(30),
);

// Propose to the group's quorum (a ConsensusResult, not a Result — match it):
let outcome = agent.consensus()
    .group_propose("overlay", "config/epoch", value, ConsensusConfig::default())
    .await;
```

Topology constraints (e.g. "votes must span ≥ 2 availability zones") are
declared per group in `GossipConfig::topology_policies` — config always wins
over the group definition's own `topology_policy`.

---

## Leased commits — decisions that expire

By default a commit is **permanent**: the value stays authoritative until it is released (a lock
release tombstones it); a different value proposed against it returns `Superseded`. Set `ConsensusConfig::committed_lease_secs` to make it **self-expiring** — the commit
carries an epoch-lease window (written to `consensus/lease/{slot}`, gossiped like any entry), and
once it lapses the slot **reopens**: the value stops being served and the next proposer wins a
fresh round.

```rust
let cfg = ConsensusConfig { committed_lease_secs: Some(30), ..Default::default() };
agent.consensus().group_propose("workers", "epoch/leader", value, cfg).await;
```

The read side is **lease-aware**: `consistent_get` / `live_committed_value` return `None` once the
lease has lapsed — even though the raw committed entry lingers in KV until anti-entropy GCs it (the
`GET /consensus/{slot}` endpoint — bearer-gated when a token is set, scope `consensus:read` —
distinguishes the two with `lease_expired: true`). Leases are the
basis for the [distributed lock](#the-distributed-lock-service)'s `ttl` — a crashed holder's lock
clears when its commit lease lapses — and for any "reopen after N seconds" pattern (time-boxed
config epochs, auto-reopening leader election). A permanent commit (`None`, the default) never
reopens.

---

## The distributed lock service

`agent.consensus().distributed_lock(name, ttl)` is the raw **try-lock**. Most callers want the
ergonomic layer, `agent.consensus().locks()` — a [`LockService`](../../src/agent/lock_service.rs)
that adds **blocking acquire** and a **scoped critical section**:

```rust
let locks = agent.consensus().locks();

// Blocking: wait up to 10 s for the lock, hold it for at most 30 s.
let guard = locks.lock("shard-7", Duration::from_secs(30), Duration::from_secs(10)).await?;
do_exclusive_work(guard.token);   // stamp resource writes with the fencing token (below)
drop(guard);                       // release (or let the 30 s lease expire)

// Recommended: scoped — release is guaranteed on every exit path (return, `?`, panic).
locks.with_lock("shard-7", Duration::from_secs(30), Duration::from_secs(10), |g| async move {
    do_exclusive_work(g.token);
}).await?;
```

**Runnable:** [`examples/distributed_lock.rs`](../../examples/distributed_lock.rs) — three nodes
contend for one lock and fence a shared resource (`cargo run --example distributed_lock`).

### The two rules that make it correct

1. **It's a *leased* lock.** You hold it for `ttl`, then it auto-expires — the safety net so a
   crashed holder never wedges the cluster. Pick `ttl` comfortably larger than your critical
   section; keep the section short.
2. **Fence the resource with the token.** A leased lock can't promise you *still* hold it at the
   instant you touch the resource (a pause can outlast the lease). `LockGuard::token` is a
   **monotonic fencing token** (the commit's HLC — strictly increasing across successive
   holders). Stamp every write to the protected resource with it and have the resource **reject a
   token lower than the highest it has seen.** Then a stale holder's late write is refused. This
   is the standard leased-lock discipline (Kleppmann).

### Which primitive? (don't reach for a lock when you want something else)

| You want… | Use |
|---|---|
| Exclusive access to a named resource, willing to wait | `locks().lock` / `with_lock` |
| Take-it-now-or-move-on | `locks().try_lock` / `distributed_lock` |
| Elect one leader/owner for a group | `elect_leader` |
| Hand each **work item** to exactly one of many workers | `mycelium-tuple-space` (`take`) — a lock *serialises*, a queue *distributes*; don't build a queue from one lock |
| One active consumer of an ordered log | `subscribe_log_group` |
| Agree a **value** under contention (config, a decision) | `consistent_set` |

Coarse-grained by design — a consensus round per acquire (~1 s to converge) — so it fits leader
election, shard/config ownership, and migrations, **not** high-rate fine-grained locking.

> **Deciding lock-vs-ring?** The lock is **CP** (blocks without quorum); the capability ring is
> **AP** (always elects). Mycelium's own companions — tuple space, blackboard, wiki — each need a
> single writer and **deliberately don't use the lock**, choosing the ring so they stay
> partition-available. When to reach for the lock and when not, and why each companion chose as it
> did: **[design/coordination-approaches.md](../design/coordination-approaches.md)**.

---

## Dev Notes

**Quorum sizing.** Quorum is always a majority — `floor(n/2)+1` — computed
from live membership at proposal time: `cluster_propose`/`consistent_set` count
`peers + self`, `group_propose` counts the emergent group's current members.
For a 3-node cluster that's 2; for 5, it's 3. There is no "any-1" escape
hatch by design: if you can tolerate non-majority confirmation you want
`set_with_replica_sync` (item 1 PR 4b, Layer III-flavoured — it **asks** each peer whether it holds the
operation and returns a receipt naming who answered and who is unknown), not consensus. Note
`kv().set_with_min_acks` is its deprecated predecessor and cannot succeed: it watched the gossip stream
for evidence the substrate does not carry (`docs/design/contracts-receipts.md` §1a).

**When to use `consistent_set` vs gossip `set`.**

| Scenario | Use |
|----------|-----|
| Config that must not be applied twice | `consistent_set` |
| Heartbeats, presence, capability ads | `set` (gossip) |
| Work assignment that must not be double-issued | `distributed_lock` + gossip `set` |
| Ordered event log | `append` |
| Counter increments | `append` (derive count from sequence) or `consistent_set` with read-modify-write |

**`append` vs `consistent_set` for sequencing.** `append` is cheaper for
ordered-log use cases because it doesn't require a read-modify-write cycle.
Each call gets a monotonically increasing sequence number. Use `append` for
audit logs, event sourcing, and task queues. Use `consistent_set` for config
that has a well-defined key.

**`distributed_lock` TTL + fencing.** The `ttl` is the lease — a safety net for crashes, not a
renewal mechanism — so set it *longer* than your operation (5 s work → 10–15 s ttl). Because a
lease can still lapse under a pause, correctness comes from the **fencing token** (`LockGuard::token`,
a monotonic HLC): stamp resource writes with it and reject stale tokens. Prefer the
[lock service](#the-distributed-lock-service) (`agent.consensus().locks()`) for blocking acquire +
a scoped `with_lock` that guarantees release.

**Hard topology.** `TopologyEnforcement::Hard` (in a group's
`GroupTopologyPolicy`) rejects a quorum whose votes don't satisfy the
declared locality spread — e.g. `spread_min_distinct: 2` at `spread_depth:
Some(1)` demands voters from at least two availability zones. Use it when
correctness depends on failure-domain diversity (compliance, split-brain
resistance). The operator can relax a live group as an escape hatch:
`POST /gateway/govern/topology-override {"group": "G", "override": true}` (scope `govern:write`, audited in a `compliance` build with `[tls]`;
`"override": false` releases it). It writes `sys/topology-override/{group}`, which the gate reads as active only
when its value is exactly `true`; the raw KV routes refuse that key since 2.27.0. An embedded node sets the
same key directly (`agent.kv().set("sys/topology-override/G", &b"true"[..])`). The override has **no lease**: it
stays until released. A group commit the gate refuses answers **409** `{"ok": false, "error":
"topology_unsatisfied"}` over HTTP (`ConsistencyError::TopologyUnsatisfied` in Rust); the operator's runbook
is [diagnostics.md § Consensus refused](../operations/diagnostics.md#consensus-refused--topology_unsatisfied).

**Consensus and partition tolerance.** The overlay is CP (consistent,
partition-tolerant) within the quorum group — it blocks, not fails, when
quorum is unavailable. Gossip KV is AP — it continues under partitions.
Design your system so only the operations that truly need it use the overlay;
the rest uses gossip.

→ Next: [05-skills.md](05-skills.md) — LLM agents as first-class mesh citizens.

---

## Reference — Layer III protocol & API

*Moved from the repo README (2026-07-10).*

Single-decree Paxos built directly on top of the signal mesh — no extra wire format, no separate
consensus port. All consensus messages ride existing `Signal` frames.

#### Protocol sketch

```
Prepare → (promises, each reporting what it accepted) →
  Propose (the highest-ballot reported value, or the proposer's own) →
  (votes bound to that value) → Commit → KV committed/{slot}
```

**Phase 1 (2.30.0).** Before proposing, a proposer asks for promises; each acceptor that promises
reports what it has accepted, and the proposer must carry the highest-ballot value its promise
quorum reports. When the promise quorum shares an acceptor with an accept quorum — a fixed roster
and a strict-majority quorum guarantee that — a value that quorum accepted reaches the later
proposer before it may propose anything (proposers older than 2.30.0 skip this, and carry no such
guarantee). A proposal that ends up carrying another caller's value returns `Superseded`, not
`Committed`. Before 2.30.0 a proposer learned an accepted value only
from a refusal, and a strictly higher ballot is never refused: on a stable roster, two concurrent
proposers could each commit a different value for one slot, and both were told they had won.

Three rules keep it so. Ballots are drawn from a shared key, so an acceptor promises a ballot to one
proposing **node**, and a node proposes one value per ballot — its own acceptor is the gate every
proposal from it passes, however many it runs for the slot at once. An acceptor **never forgets** a
promise: its memory outlives the commit. And a commit records the ballot it was decided at
(`consensus/decided/{slot}`); a ballot at or below it is refused, and once a node sees that decision
is over — its lease expired, or a lock released it — a new decision ignores acceptances at or below
it, the previous decision's. A node that has the floor but not yet the commit ignores nothing.

Committed values are written to `consensus/committed/{slot}` and anti-entropy-synced to
late joiners automatically via the existing KV mechanism.

#### API

```rust
use mycelium::{ConsensusConfig, ConsensusResult};
use bytes::Bytes;

// Every node that should vote calls this once.
let _listener = agent.consensus().start_consensus_listener(ConsensusConfig::default());

// Propose within a group — blocks until quorum or timeout.
let cfg = ConsensusConfig { quorum_size: 0, ..ConsensusConfig::default() };
match agent.consensus().group_propose("workers", "coordinator", Bytes::from("node-7"), cfg).await {
    ConsensusResult::Committed { slot, value, ballot, persisted, .. } => {
        println!("committed: {} = {:?} @ ballot {}", slot, value, ballot);
        if !persisted {
            // Committed cluster-wide and applied here, but this node's WAL append failed
            // (writer stopped / disk error) — after a restart it recovers only via anti-entropy.
            eprintln!("slot {slot} committed but not on local stable storage");
        }
    }
    ConsensusResult::Timeout { ballots_tried, votes_last_ballot, quorum_required, .. } => {
        println!("no quorum after {} ballots; last ballot got {}/{} votes",
                 ballots_tried, votes_last_ballot, quorum_required);
    }
    ConsensusResult::Superseded { slot, ballot } => {
        // The slot was decided for another value — another proposer committed first, or this
        // proposal carried a value a quorum had already accepted (2.30.0). Read what was decided:
        if let Some(v) = agent.consensus().consensus_get(&slot) {
            println!("superseded at ballot {}: {:?}", ballot, v);
        }
    }
    ConsensusResult::ElectorateUnavailable { observed_members, declared_min, .. } => {
        // This node cannot see an established electorate (2.14.0): an empty roster, or fewer
        // members than a fresh MembershipIntent declares. Not a vote against — no ballot ran.
        eprintln!("no electorate: saw {observed_members}, declared min {declared_min}");
    }
    // `ConsensusResult` is `#[non_exhaustive]` (2.14.0). A variant this build does not know
    // is *not committed* — fail closed, never treat the unknown arm as success.
    _ => eprintln!("consensus: unrecognised outcome — treating as not committed"),
}

// `persisted` (since v2.4.2) reports *local* durability of the committed slot: the WAL append is
// forced to fdatasync in every SyncMode; `false` means the cluster commit stands but this node did
// not get it onto disk (logged at error). Match with `..` if you don't need it. The gateway's
// propose / overlay/consistent/set JSON carries the same field as "persisted" — and, since v2.8.0,
// "local_durability" beside it: the receipt's `LocalDurability` as "on_disk" · "buffered" ·
// "not_configured" · "failed" (+ "local_durability_error" for the last), which separates *on disk*
// from *nothing was promised* — two states `persisted` folds into one `true`. In Rust, ask
// `cluster_propose_receipt` / `group_propose_receipt` for the same receipt instead of the bool.

// System-wide proposal (all known peers vote).
let _ = agent.consensus().cluster_propose("global/epoch", Bytes::from("42"), ConsensusConfig::default()).await;

// Subscribe to a slot — fires whenever the slot is committed.
let mut rx = agent.consensus().consensus_rx("coordinator");

// Quorum trust slices (SCP §3.1). With `use_trust_slices: true` this proposer counts only
// votes from the declared set — the fixed eligible voter set a safety-sensitive profile needs.
// (The quorum *size* is unchanged; slice-based intersection is not implemented.)
agent.consensus().declare_trust("workers", &[peer_a, peer_b]);
let slices = agent.consensus().group_trust("workers");
```

#### Key design decisions

| Decision | Rationale |
|---|---|
| Ballot numbering (SCP §6.2) | Monotonic counter at `consensus/ballot/{slot}`, kept across commits; `consensus/decided/{slot}` records the ballot a commit was decided at — a floor acceptors refuse at or below (2.30.0) |
| Prepare phase (2.30.0) | A proposer asks a quorum what it has accepted before proposing, and carries the highest-ballot value reported — the property that keeps two concurrent proposers from committing different values |
| Group-scoped votes | Votes are broadcast to the group, but only the proposer counts them and commits. If it crashes after a quorum accepted, the next proposer's prepare phase learns the accepted value and finishes it |
| Proposer self-votes | Proposer always counts as one voter; no listener required for single-node quorum |
| LWW commit idempotency | Two simultaneous commits of the same value are safe; higher-ballot commit wins via LWW timestamp |
| Optimistic commit / converged-holder | Before 2.30.0 `group_propose` committed against a node's *local* committed view, so under gossip lag two proposers could both return `Committed`; the prepare phase prevents that when the two quorums intersect, and it remains possible across an electorate change ([§ What a successful election means](#what-a-successful-election-means)). Only the LWW-by-HLC **converged** value (`live_committed_value`) is authoritative — confirm the holder there; don't treat a `Committed` return as exclusive on its own. This is why locks fence on the commit HLC (see [the two rules](#the-two-rules-that-make-it-correct)) |
| No ordering log | Each slot is an independent KV entry (CASPaxos-style); no WAL required |
| Signing | With `tls` feature: all consensus payloads are Ed25519-signed; forged ballots are dropped. Without: trusted-domain only; Byzantine fault tolerance is out of scope |

`quorum_size = 0` uses `floor(N/2) + 1` (simple majority). `phase1_timeout` is tunable via
`ConsensusConfig`; the `max_peers` cap is a `GossipConfig` field.

---

## Reference — the opt-in consistency & ordering overlay

*Moved from the repo README (2026-07-10): `consistent_set`/`consistent_get`, the distributed lock, leader election, the durable log + consumer groups, reliable delivery.*

Mycelium's thesis is **consistency as a service, not a foundation** — the epidemic substrate
is always fast; stronger guarantees are opt-in per operation. The overlay layer surfaces these
as first-class APIs without touching the gossip core.

#### Consensus KV (`consistent_set` / `consistent_get`)

Runs a prepare and a voting round before writing. When two callers race with **different** values for one key
and their quorums intersect, at most one sees `Ok(())`; the other gets `Err(Superseded)` — the later proposer
found the earlier's value already committed, or accepted by a quorum, which it then committed in its own place —
or `Err(Timeout)`. A timeout does not mean your value lost: a later proposer may have adopted and committed it, so
read `consistent_get`. Two callers writing the **same** value can both see `Ok(())`. Across an electorate change,
or against proposers older than 2.30.0, none of this is guaranteed
([§ What a successful election means](#what-a-successful-election-means)).

`consistent_get` is a **local read** — it returns the latest committed value that has
anti-entropy-propagated to this node, which may lag by up to one gossip round. This is
suitable for leader election and distributed locks where HLC-based fencing tokens protect
against lower-ballot writers; it is not a substitute for linearizable reads.

```rust
// Any node can write — Ok(()) only if YOUR value was decided; Err(Superseded) means another caller's was
agent.consensus().consistent_set("config/endpoint", &b"https://api.v2/"[..]).await?;
let val = agent.consensus().consistent_get("config/endpoint"); // local read, eventually consistent
```

#### Distributed Lock (`distributed_lock`)

Consensus-backed named lock. The returned `LockGuard` releases (tombstones the lock key)
on drop. The `token` field is a monotonic fencing token drawn from the commit's HLC (not the
ballot, which can regress under gossip lag — see the fencing-token discipline above).

```rust
let guard = agent.consensus().distributed_lock("job-42", Duration::from_secs(30)).await?;
println!("fencing token: {}", guard.token);
// exclusive work here
drop(guard); // or guard.release()
```

#### Leader Election (`elect_leader`)

One-shot election per group. If this node loses it reads the committed winner and returns
that `NodeId` — so all nodes converge on the same answer.

```rust
let leader = agent.consensus().elect_leader("shard-0").await?;
if leader == *agent.node_id() {
    // I won — start serving shard-0
}
```

#### Ordered Durable Log (`append` / `scan_log` / `subscribe_log`)

HLC-keyed entries written to the gossip KV under `log/{stream}/{hlc:016x}`. Lexicographic
key order equals causal time order.

```rust
// Producer
let cursor = agent.kv().append("events", b"order-placed");

// Consumer — one-shot scan
let entries = agent.kv().scan_log("events", 0, u64::MAX);

// Live subscriber — mpsc channel, new entries arrive on each gossip tick
let mut rx = agent.kv().subscribe_log("events", 0);
while let Some(entry) = rx.recv().await {
    println!("{} {:?}", entry.hlc, entry.value);
}

// Trim old entries
agent.kv().compact_log("events", checkpoint_hlc);
```

#### Consumer Groups (`subscribe_log_group`)

At most one consumer per group advances at a time. The offset (`clog/{stream}/{group}/offset`)
is persisted in the gossip KV so any node can take over if the current holder fails.

```rust
let mut rx = agent.kv().subscribe_log_group("events", "workers").await;
while let Some(entry) = rx.recv().await {
    process(&entry);
    // offset committed before next entry is delivered
}
```

#### Reliable Delivery (`emit_reliable`)

Send a payload to a specific node and wait for an explicit application-level ACK (the
receiver calls `rpc_respond`). Returns `AckResult::Acknowledged` or `AckResult::Timeout`.

```rust
let ack = agent.service().emit_reliable(target, "task.assign", payload, Duration::from_secs(5)).await;
```
