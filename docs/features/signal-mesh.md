# Signal mesh: admission, forwarding, suppression

## What it is

Layer II: a [signal](../guide/00-concepts.md#signal) is an ephemeral event of a kind, addressed to a scope — the whole
cluster, a group, or one node — that spreads epidemically through the mesh. Forwarding is unconditional: every node
relays a signal with hops left whether or not it acts on it, and only a frame addressed to this node terminates here.
What a node acts on is decided by its [boundary](../guide/00-concepts.md#boundary), the groups it has joined:
admission is scoped, and under load a node sheds group and cluster signals before they reach its handlers, never an
individual one. A node can suppress a kind for a while, holding it until the hold ends.

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
  hold a kind for a duration; held signals are delivered when the hold ends.
- [`default_ttl`](../reference/configuration.md#default_ttl): how many hops a signal is forwarded.
- `test_signal_two_node_propagation`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`mailbox_llm`](../../examples/README.md#the-capability-matrix), guide 3's example: an Individual-scoped request reaches
one provider through the mesh, whose boundary admits it. CI runs it. That forwarding does not depend on membership
is `forwarding_is_unconditional_through_non_member_relay`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## What it guarantees, and what it does not

- **Rules:** [`signal.admission`](../reference/rule-catalogue.md#signaladmission) — admitted, `scope_not_admitted`,
  or `shed_under_load` (never Individual, never a boundary transition);
  [`signal.forwarding`](../reference/rule-catalogue.md#signalforwarding) — forwarded, or `ttl_exhausted`,
  `terminates_here`, `seen`; [`signal.suppression`](../reference/rule-catalogue.md#signalsuppression) — held until a
  time, released on flush, never changing forwarding.
- **Not built:** delivery guarantees — a signal can be shed, dropped by a full receiver or missed by a node that was
  down; a boundary is self-imposed, so it does not stop a node acting on what it relays (a
  [guardrail](guardrails.md)'s Tier A); a signal's content is not authenticated beyond the frame's sender.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger's
  [hot-path costs](../operations/what-is-proven.md#hot-path-cost) give the signal fan-out; no row covers admission on
  its own.

## Go deeper

- **Guide:** [03, signals and boundaries](../guide/03-signals.md).
- **Runbook:** [admission control](../operations/admission-control.md) and [observability](../operations/observability.md).
- **Design:** the [runtime invariants](../wiki/dev/architecture/runtime-invariants.md): individual-scope forwarding is
  unconditional, and its two carve-outs.
- **Code:** [`mycelium-core/src/signal.rs`](../../mycelium-core/src/signal.rs) (scopes, boundaries, suppression),
  [`mycelium-core/src/ops.rs`](../../mycelium-core/src/ops.rs) (emit, forward, deliver),
  [`mycelium-core/src/mesh_handle.rs`](../../mycelium-core/src/mesh_handle.rs).
