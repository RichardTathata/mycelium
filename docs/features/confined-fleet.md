# Confined fleet

## What it is

The confined-fleet profile is a deployment shape, not a setting. Agent code runs in pods that hold no member key and
can reach nothing on the network except their gateway, so every act an agent takes passes through a door that
authorises and records it. Agents and gateways run in separate pods, because containers in one pod share a network
namespace, and a `NetworkPolicy` on a CNI that enforces it does the confining. A node cannot see its own network, so
it reports the confinement as unverified and says which of the settings the profile wants it does hold.

## When to use it, and when not

- **Use it when** agents run code you do not fully trust, and an act that goes around the substrate — a direct
  network call, a credential it was never issued — must be impossible rather than merely invisible.
- **Not when** the CNI does not enforce `NetworkPolicy`, or agents and gateway share a pod: then the agent can reach
  whatever the gateway can ([confined fleet § 1](../operations/confined-fleet.md#1-before-you-start-does-your-cni-enforce-networkpolicy)).

## Turn it on

This feature is turned on in the deployment. The reference manifests are in
[`deploy/confined-fleet/`](../../deploy/confined-fleet/README.md), and the gateway node's settings are
[confined fleet § 3](../operations/confined-fleet.md#3-configure-the-gateway-node): an egress allow-list of only the
hosts the gateway must reach, identity proofs, an audit sink, and an [action evaluator](action-authorisation.md) with
an [evidence journal](evidence-journal.md), with mandates established at the gateway.

- [`confinement_report`](../../src/agent/confinement.rs): which of the profile's settings this node holds, and the
  network as `Unverified`, always.
- The example's declaration, which CI's wire-check gate reads, is
  [`examples/units/confined_fleet_node/`](../../examples/units/confined_fleet_node/gateway.toml).
- `a_default_node_reports_every_setting_unmet_and_the_network_unverified`, in
  [`src/agent/confinement.rs`](../../src/agent/confinement.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`confined_fleet_node`](../../examples/README.md#the-capability-matrix): one node image that
`scripts/test-confined-fleet.sh` runs as operator, provider and gateway in a kind cluster behind the reference network
policy, passing on the provider's own records of which handlers ran. It runs in a path-filtered workflow (✓ᵖ), not on every
change.

## What it guarantees, and what it does not

- **Guarantee:** [`net.confinement`](../reference/guarantee-catalogue.md#net.confinement) — agent pods reach nothing
  but their gateway. It is an external prerequisite: the network provides it, and a node's report never counts it.
- **Not built:** anything about what agents do through the gateway, which is authorisation's job; confinement on a
  CNI that does not enforce `NetworkPolicy`; a node that knows it is confined
  ([confined fleet § 6](../operations/confined-fleet.md#6-what-this-does-not-give-you)). The kind cluster's pods
  share one node and one clock, and its member CA key is on the members.
- **Evidence:** the ledger row
  [after a revocation every door runs nothing](../operations/what-is-proven.md#revocation-every-door-runs-nothing),
  whose deployment variant is this cluster.

## Go deeper

- **Guide:** [20, authorising actions](../guide/20-authorising-actions.md), for what the gateway decides.
- **Runbook:** [confined fleet](../operations/confined-fleet.md).
- **Design:** [`design/confined-fleet.md`](../design/confined-fleet.md).
- **Code:** [`src/agent/confinement.rs`](../../src/agent/confinement.rs),
  [`examples/confined_fleet_node.rs`](../../examples/confined_fleet_node.rs) and
  [`scripts/test-confined-fleet.sh`](../../scripts/test-confined-fleet.sh).
