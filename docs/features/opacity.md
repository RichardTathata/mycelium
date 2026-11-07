# Opacity and stigmergic routing

## What it is

[Opacity](../guide/00-concepts.md#opacity) is a node reporting its own saturation and nothing else. An opacity governor
watches how full the node's inbound channel for a kind is; past a threshold it writes a pheromone under
`sys/load/{node}/{kind}` and emits a boundary-opaque signal, and once the fill falls a full hysteresis below the
threshold it clears both. Other nodes read the trail and route around a busy node on their own — no coordinator, no
message to the dispatcher, no failure detector. A stale mark stops reading as opaque, and an Individual-scoped signal is
delivered to an opaque node regardless.

## When to use it, and when not

- **Use it when** several providers of one kind share load and callers should choose among them by how busy each is,
  without a load balancer.
- **Not when** you need a hard admission limit at the provider: opacity informs choices, and a caller that ignores the
  trail still reaches the node ([admission control](../operations/admission-control.md) bounds the companions' queues).

## Turn it on

This feature is turned on in code.

- [`manage_opacity`](../../src/agent/opacity.rs) with an [`OpacityHint`](../../mycelium-core/src/signal.rs): start the
  governor for a kind — `threshold` (clamped to `[0.4, 0.95]`), `hysteresis`, and `release_spacing_ms`; dropping the
  handle stops it.
- [`is_node_opaque`](../../src/agent/capability_handle.rs), [`peer_load`](../../src/agent/opacity.rs) and
  [`suggest_leader`](../../src/agent/capability_handle.rs): read the trail when choosing a provider.
- [`set_control_profile`](../../src/agent/control_profile.rs): a release inside its spacing is held only under
  `EnforceLocal` or above, counted under `Observe`, and not consulted under `Legacy`, the default; a shed is never
  held ([adaptive stability](adaptive-stability.md)).
- `opacity_clears_only_after_fill_falls_a_full_hysteresis_below_threshold`, in
  [`src/agent/opacity.rs`](../../src/agent/opacity.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`stigmergy`](../../examples/README.md#the-capability-matrix): three depots run opacity governors over their intake, a
dispatcher reads the trail and routes around the busy one, and the depot rejoins when its queue drains. CI runs it;
[`stigmergy_viz`](../../examples/README.md#the-capability-matrix) is the browser view.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the governor's spacing is the control contract's
  ([adaptive stability](adaptive-stability.md)), and skipping an opaque provider is
  [`cap.match`](../reference/rule-catalogue.md#capmatch)'s `opaque_skipped`.
- **Not built:** the pheromone is self-reported, so a node can under- or over-report; the trail is as fresh as gossip;
  a reader's routing is a hint it may ignore. Opacity is not inhibition: forwarding never stops
  ([signal mesh](signal-mesh.md)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for opacity on its own; the
  [hysteresis row](../operations/what-is-proven.md#hysteresis-control-loops-load-bearing) is about the control loops
  together, in scenario C's bounded sense.

## Go deeper

- **Guide:** [03 § Reference — the signal mesh in depth](../guide/03-signals.md#reference--the-signal-mesh-in-depth)
  (opacity versus inhibition, signals versus pheromone trails); [11, semantic coordination](../guide/11-semantic-coordination.md).
- **Runbook:** [admission control](../operations/admission-control.md).
- **Design:** [`adaptive-stability.md`](../design/adaptive-stability.md) for the release spacing.
- **Code:** [`src/agent/opacity.rs`](../../src/agent/opacity.rs) and `OpacityHint` in
  [`mycelium-core/src/signal.rs`](../../mycelium-core/src/signal.rs).
