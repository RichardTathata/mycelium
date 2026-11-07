# Capability discovery

## What it is

A [capability](../guide/00-concepts.md#capability) is a node saying "I provide `ns/name`", with a schema id and typed
attributes, written under `cap/` and refreshed on an interval. Any node resolves a filter against what it has seen,
by one matching rule shared with the offline check: namespace, name, schema id and attribute constraints. There is no
registry to ask: an advertisement its writer stops refreshing is discarded by `resolve`, the watch and the other
readers that apply freshness once it is older than three refresh intervals, so a crashed provider evaporates from
resolution on its own. Resolution does not consult opacity: a caller that wants to route around a busy provider
reads its trail ([opacity](opacity.md)).

## When to use it, and when not

- **Use it when** callers should find a provider by what it does rather than where it is, and providers come and go.
- **Not when** you need to know a provider is up right now: resolution is as fresh as the gossip and the evaporation
  window allow, and a provider can be gone for up to three intervals before readers drop it.

## Turn it on

This feature is turned on in code, on the agent's [`capabilities`](../../src/agent/mod.rs) handle.

- [`advertise_capability`](../../src/agent/capability_handle.rs): advertise a [`Capability`](../../src/capability.rs)
  on an interval; dropping the returned handle retracts it with a tombstone.
- [`resolve`](../../src/agent/capability_handle.rs) with a [`CapFilter`](../../src/capability.rs): the providers this
  node sees; [`watch_capabilities`](../../src/agent/capability_handle.rs) streams changes.
- [`declare_requirement`](../../src/agent/capability_handle.rs): publish an unmet need, which a stem's
  demand-driven provisioning can answer.
- A unit file's `[[capability]]` declares the same advertisement for `wire-check` and a stem.
- `test_wsd_capability_authz_routes_around_unauthorized_advertiser`, in [`src/lib_tests.rs`](../../src/lib_tests.rs):
  a second node resolves both providers' advertisements across the mesh before a policy narrows them.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`hello_capability`](../../examples/README.md#the-capability-matrix): one node advertises, another resolves it by filter;
CI compiles it and checks its declaration, and does not run it. That a retired advertisement leaves the view is
`test_capability_reg_drop_tombstones_advertisement`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## What it guarantees, and what it does not

- **Rule:** [`cap.match`](../reference/rule-catalogue.md#capmatch) — matched, or `ns_or_name_differs`,
  `schema_id_differs`, `constraint_unmet` (rule revision 2: revision 1 also listed `opaque_skipped`, which no matcher
  does).
- **Held:** a capability name is one key segment at every door.
- **Not built:** an advertisement is a claim, not proof the provider serves it or is authorised to — authority is
  checked where the work runs ([provider enforcement](provider-enforcement.md)); resolution is per node and can
  differ between nodes while gossip settles; three `cap/` readers do not apply freshness yet — the A2A provider
  resolution, the A2A card's skill list and the membership governor's eligibility — a known gap fixed by a separate
  PR.
- **Evidence:** the ledger row
  [a capability name is one key segment at every door](../operations/what-is-proven.md#capability-name-key-segment-door).

## Go deeper

- **Guide:** [02, capabilities](../guide/02-capabilities.md).
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md).
- **Design:** no design record; the capability model is the module doc of [`src/capability.rs`](../../src/capability.rs).
- **Code:** [`src/capability.rs`](../../src/capability.rs) (the capability, the filter, freshness) and
  [`src/agent/capability_handle.rs`](../../src/agent/capability_handle.rs).
