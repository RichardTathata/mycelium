# Signal mesh: admission, forwarding, suppression

## What it is

Layer II: a [signal](../guide/00-concepts.md#signal) is an ephemeral event of a kind, addressed to a scope — the whole
cluster, a group, or one node — that spreads epidemically through the mesh. Forwarding is unconditional: every node
relays a signal with hops left whether or not it acts on it; the two carve-outs are a frame addressed to this node,
which terminates here, and a removed member's frames, dropped at the connection.
What a node acts on is decided by its [boundary](../guide/00-concepts.md#boundary), the groups it has joined:
admission is scoped, and under load a node sheds group and cluster signals before they reach its handlers, never an
individual one. A node can suppress a kind for a while: until then its signals are dropped at delivery, not held.

## When to use it, and when not

- **Use it when** nodes should react to events — work available, a request for one provider, a state change —
  without a broker, and an event that is missed can be missed.
- **Not when** the event must be delivered once and in order whatever happens: that is the [mailbox](mailbox.md).
  Admission filters action, not dissemination, so a signal reaches nodes outside its scope as a relay.

## Turn it on

This feature is turned on in code: every node has it, from [`mesh`](../../src/agent/mod.rs) on the agent.

- [`emit`](../../mycelium-core/src/mesh_handle.rs): send a signal of a kind to a [`SignalScope`](../../mycelium-core/src/signal.rs)
  — `Cluster`, `Group` or `Individual`.
- [`signal_rx`](../../mycelium-core/src/mesh_handle.rs): receive a kind this node admits.
- [`join_group`](../../mycelium-core/src/mesh_handle.rs) and [`leave_group`](../../mycelium-core/src/mesh_handle.rs):
  change this node's boundary.
- [`suppress`](../../mycelium-core/src/mesh_handle.rs) and [`unsuppress`](../../mycelium-core/src/mesh_handle.rs):
  drop a kind's signals at delivery for a duration — counted, never delivered later — while the sender log and the
  last-signal time still record them, so quorum counting sees them.
- [`default_ttl`](../reference/configuration.md#default_ttl): how many hops a signal is forwarded.
- `test_signal_two_node_propagation`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`mailbox_llm`](../../examples/README.md#the-capability-matrix), guide 3's example: an Individual-scoped request reaches
one provider through the mesh, whose boundary admits it. CI runs it; it relies on the mesh rather than showing scope,
since an Individual request reaches its target whatever the boundaries say. What scope and admission do are tests in
[`src/lib_tests.rs`](../../src/lib_tests.rs): `test_group_signal_only_reaches_members` (a Group signal fires on its
members' handlers and not on a non-member's), and `forwarding_is_unconditional_through_non_member_relay` (forwarding
does not depend on membership).

## What it guarantees, and what it does not

- **Rules:** [`signal.admission`](../reference/rule-catalogue.md#signaladmission) — admitted, `scope_not_admitted`,
  or `shed_under_load` (never Individual, never a boundary transition);
  [`signal.forwarding`](../reference/rule-catalogue.md#signalforwarding) — forwarded, or `ttl_exhausted`,
  `terminates_here`, `seen`; [`signal.suppression`](../reference/rule-catalogue.md#signalsuppression) — `suppressed` (dropped) or
  `delivered`, rev 2 (rev 1 described a hold the code does not have), never changing forwarding.
- **Not built:** delivery guarantees — a signal can be shed, dropped by a full receiver or missed by a node that was
  down; a boundary is self-imposed, so it does not stop a node acting on what it relays (a
  [guardrail](guardrails.md)'s Tier A); a signal's `sender` field is not signed, so a handler should not authorise on
  it.
- **At the gateway, a client's kinds are not the substrate's.** The doors that take a kind from an HTTP caller — the
  two signal streams, the serve stream, the mailbox stream and the six raw send routes — refuse the substrate's own
  kinds (`consensus.propose`, `consensus.vote`, `consensus.commit`, `consensus.nack`, `sys.explain`,
  `sys.replica-sync.v1`, `invoke.bulk`, and any kind starting `consensus.` or `sys.`) `403 protected_kind` for
  every token; the send routes refuse the RPC reply kinds, and the streams and the send routes a hosted
  capability's `cap.invoke/…`
  ([deprecations §103](../guide/deprecations.md), `src/agent/http/substrate_kind_tests.rs`). A member in the mesh
  is not held to this: Layer II forwards and delivers every kind. **Not built:** a signature or a scope on the
  supervisor signals — any `mesh:write` holder can emit `agent.veto` (it fails closed); the other kinds the library
  speaks (`boundary.*`, `agent.state`, `artifact.*`, `reason.blob.fetch`) are observable under `mesh:read` and
  sendable under `mesh:write`, each open by a recorded decision, and a kind outside every list is open
  ([rbac.md](../operations/rbac.md)).
- **Evidence:** the ledger rows
  [one stalled subscriber does not veto its kind, and the signal log is bounded](../operations/what-is-proven.md#silent-slow-stalled-peers-closed-stalled)
  and the [hot-path costs](../operations/what-is-proven.md#hot-path-cost), which give the signal fan-out.

## Go deeper

- **Guide:** [03, signals and boundaries](../guide/03-signals.md).
- **Runbook:** [admission control](../operations/admission-control.md) and [observability](../operations/observability.md).
- **Design:** the [runtime invariants](../wiki/dev/architecture/runtime-invariants.md): individual-scope forwarding is
  unconditional, and its two carve-outs.
- **Code:** [`mycelium-core/src/signal.rs`](../../mycelium-core/src/signal.rs) (scopes, boundaries, suppression),
  [`mycelium-core/src/ops.rs`](../../mycelium-core/src/ops.rs) (emit, forward, deliver),
  [`mycelium-core/src/mesh_handle.rs`](../../mycelium-core/src/mesh_handle.rs).
