# Consensus, electorates, locks and leadership

## What it is

Layer III: [consensus](../guide/00-concepts.md#consensus) proposes a value for a slot to the whole cluster, or to a
group, and commits it once a quorum accepts, with a prepare phase so two concurrent proposers cannot commit different
values. A lock, a lease and a leadership are one decision record over it, leadership leased by default. For an
exclusive outcome — a lock, a leader, a consistent write, an award — an electorate makes it safe against a roster a
node cannot see: a group pinned by member identity at genesis, decided only by the members it names, changed only by
a migration that terminates. An electorate member must have `[persistence]` for its acceptor memory: without it the
membership is refused by name.

## When to use it, and when not

- **Use it when** a fleet must agree on one value — who holds a lock, who leads, which bid won — and a stale answer
  must be detectable by its fencing token.
- **Not when** last-writer-wins is enough: gossip KV is cheaper; and not when the group may lose its majority for
  good, because a minority cannot authorise its own repair
  ([what-is-proven § disaster recovery](../operations/what-is-proven.md#disaster-recovery-electorate-lost-majority)).

## Turn it on

This feature is turned on in code: `consensus` is a default feature of `mycelium`. Every voting node runs a listener.

- [`start_consensus_listener`](../../src/agent/consensus_handle.rs): registers synchronously; a node with none does
  not vote.
- [`cluster_propose`](../../src/agent/consensus_handle.rs), [`group_propose`](../../src/agent/consensus_handle.rs) and
  [`elect_leader`](../../src/agent/consensus_handle.rs); [`LockService`](../../src/agent/lock_service.rs) for locks.
- Form the group first — every member joins it ([`join_group`](../../mycelium-core/src/mesh_handle.rs), or a
  membership intent) — then [`declare_electorate`](../../src/agent/electorate.rs) from a
  member: it reads the roster as that node sees it and proposes a genesis every named member must decide. Mark one
  `exclusive_default` for the cluster-scoped exclusive verbs.
- [`consensus_require_electorate`](../reference/configuration.md#consensus_require_electorate): refuse a
  safety-sensitive proposal outside an electorate.
- [`persistence`](../reference/configuration.md#persistence): required on an electorate member.
- `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`, in [`src/lib_tests.rs`](../../src/lib_tests.rs):
  a leadership outside an electorate is refused, then the group is joined, declared and decides.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`coordination_integrity`](../../examples/README.md#the-capability-matrix): what a successful election means, and what
it does not, run in CI. [`consensus`](../../examples/README.md#the-capability-matrix) (co-op) and
[`distributed_lock`](../../examples/README.md#the-capability-matrix) are the other two. None of the three declares an
electorate; the electorate's run is the test named above, `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`,
in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## What it guarantees, and what it does not

- **Guarantee:** [`cons.safety_profile`](../reference/guarantee-catalogue.md#cons.safety_profile) — an exclusive
  outcome is decided only by an electorate group.
- **Rule:** [`cons.acceptor_memory`](../reference/rule-catalogue.md#consacceptor_memory) — detection, not
  prevention: an electorate member without `[persistence]` is refused, one restarted over an empty store is warned
  once its genesis is known, and a forgotten promise withholds the slot only when a peer's copy of this node's record
  arrives before the next higher ballot. A member restarted over an empty or older store is otherwise not detected
  ([what-is-proven § disaster recovery](../operations/what-is-proven.md#disaster-recovery-electorate-lost-majority)).
- **Not built:** leadership is not an exclusive grant that stays true — exclusivity is enforced at the resource, by
  refusing a stale fencing token; single-decree safety is not a proof of the whole protocol; an electorate's
  members cannot change except by migration, and a group that lost its majority stays stopped.
- **Evidence:** the ledger rows
  [an exclusive outcome is decided only by an electorate group](../operations/what-is-proven.md#exclusive-outcome-decided-only-electorate-group),
  [an electorate's members change by a migration that terminates](../operations/what-is-proven.md#electorate-s-members-change-migration-terminates)
  and [a lock, a lease and a leadership end where every node can see it](../operations/what-is-proven.md#lock-lease-leadership-end-where-node).

## Go deeper

- **Guide:** [04, consensus](../guide/04-consensus.md), including
  [§ What a successful election means](../guide/04-consensus.md#what-a-successful-election-means) and
  [§ The distributed lock service](../guide/04-consensus.md#the-distributed-lock-service).
- **Runbook:** [cert rotation § A member that goes down](../operations/cert-rotation.md#a-member-that-goes-down-restore-it-with-its-state-or-retire-it--never-reissue-it-empty).
- **Design:** [`consensus-electorate.md`](../design/consensus-electorate.md).
- **Code:** [`src/consensus.rs`](../../src/consensus.rs), [`src/agent/consensus_handle.rs`](../../src/agent/consensus_handle.rs),
  [`src/agent/electorate.rs`](../../src/agent/electorate.rs), [`src/agent/acceptor_guard.rs`](../../src/agent/acceptor_guard.rs).
