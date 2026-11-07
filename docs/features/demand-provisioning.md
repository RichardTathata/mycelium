# Demand-driven provisioning

## What it is

A node that needs a capability nobody provides declares a [requirement](../guide/00-concepts.md#requirement); the
requirement with no provider is [demand](../guide/00-concepts.md#demand). Every host running a
[provisioner](../guide/00-concepts.md#provisioner) sees the same demand and decides for itself: it checks it can run the
entry's [runtime kind](../guide/00-concepts.md#runtime-kind) within its budget and headroom, that a publisher it
trusts signed it, and — through [self-election](../guide/00-concepts.md#self-election) — rolls a damped probability
before acting, so not every host installs at once. The one that installs fetches, verifies, places and hosts the
artifact, advertises it, and keeps it only while its health holds; a provider that dies leaves demand again, and
another host installs. Restart is provisioning. The library never provisions on its own.

## When to use it, and when not

- **Use it when** what a fleet needs changes over time, and you want capabilities to appear where there is room for
  them rather than be placed by hand.
- **Not when** you need a capability up before anyone asks: demand follows a requirement; keeping providers live with no
  demander is a [presence band](presence-bands.md).

## Turn it on

A [stem](stem.md) with a `[hosts]` table runs the provisioner; the requirement is declared by whoever needs the
capability. The model host's [`examples/units/model_deploy/model-host.toml`](../../examples/units/model_deploy/model-host.toml),
which CI's wire-check reads, is a `[hosts]` table.

- `[hosts]` ([unit-file § hosts](../reference/unit-file.md#hosts)): `kinds`, `install_budget_bytes`, `headroom`,
  `trusted_publishers` (empty is refused unless `accept_unsigned = true`), `placement_root`, and the fuel and deadline
  bounds.
- On the needing side, a unit file's `[[requirement]]`, or
  [`declare_requirement`](../../src/agent/capability_handle.rs) in code.
- In code, [`Provisioner::new`](../../mycelium-wasm-host/src/provisioner.rs) and
  [`provision_round`](../../mycelium-wasm-host/src/provisioner.rs);
  [`with_install_rights`](../../mycelium-wasm-host/src/provisioner.rs) adds an allocation each install takes from,
  refused only under the `EnforceAllocated` control profile ([adaptive stability](adaptive-stability.md)).
- `wasm_component_full_lifecycle_install_invoke_shed_reinstall`, in
  [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`provisioning`](../../examples/README.md#the-capability-matrix), the co-op's flagship: a requirement no depot meets
buffers work, a depot self-provisions a real WASM component and drains it, and when the provider is killed a standby
provisions again. CI runs it. [`mcp_toolgrowth`](../../examples/README.md#the-capability-matrix) grows an MCP tool the
same way.

## What it guarantees, and what it does not

- **Rules:** [`prov.demand_response`](../reference/rule-catalogue.md#provdemand_response),
  [`prov.self_election`](../reference/rule-catalogue.md#provself_election),
  [`prov.eligible`](../reference/rule-catalogue.md#proveligible),
  [`prov.rights_admission`](../reference/rule-catalogue.md#provrights_admission),
  [`prov.install`](../reference/rule-catalogue.md#provinstall),
  [`prov.advertise`](../reference/rule-catalogue.md#provadvertise),
  [`prov.health_pass`](../reference/rule-catalogue.md#provhealth_pass) and
  [`prov.withdraw`](../reference/rule-catalogue.md#provwithdraw). An unmet requirement is a reason to consider an
  install, never an instruction; an entry providing a model's `llm-meta` ad is never installed.
- **Not built:** an exact count — self-election overshoots and corrects in later rounds; placement by anything but
  eligibility and the roll; install rights that bind under `Legacy`, `Observe` or `EnforceLocal` (they count what they
  would refuse); anything but WASM components and blobs. **Known gap, fixed by a separate PR:** `[hosts].kinds` does
  not keep components out — the provisioner always registers the component runtime and the stem reads `kinds` only to
  add the blob runtime, so a `kinds = ["blob"]` stem still installs third-party WASM components.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [02, capabilities](../guide/02-capabilities.md); [tutorial 1, your first stem fleet](../guide/tutorials/01-first-stem-fleet.md).
- **Runbook:** [capability lifecycle § Watch — demand pulls the rest in](../operations/capability-lifecycle.md#5--watch--demand-pulls-the-rest-in)
  and [artifacts](../operations/artifacts.md).
- **Design:** [`design/artifact-library.md`](../design/artifact-library.md).
- **Code:** [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).
