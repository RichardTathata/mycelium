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
  [`provision_round`](../../mycelium-wasm-host/src/provisioner.rs), with
  [`refresh_catalog`](../../mycelium-wasm-host/src/provisioner.rs) before each round (the catalogue is a snapshot taken at construction);
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
  install, never an instruction; an entry providing a model's `llm-meta` ad is never installed; a stem runs only the
  kinds its `[hosts].kinds` declares, and refuses an entry of any other kind at `prov.eligible` as `kind_not_declared`
  (rev 3) — an empty `kinds` installs nothing.
- **One live build per name, per provisioner** (ER4, R12-01). The name an entry serves — `{name}.shadow` in the
  shadow lane — is held from its reservation until its entry leaves the provisioner's map (a withdraw, a shed, a probe
  failure, a failed install, a stop). A second catalogue line for a held name, another digest, waits:
  `prov.demand_response` defers it `capability_already_held` (rev 2) and names the holder, and a presence floor reads
  the held name as answered. The check sits in the reservation's one hold, beside the `closed` check, so neither pass
  can start a sibling of the other's install (`a_sibling_build_does_not_start_on_the_next_round_while_the_first_installs`).
  A line whose artifact this node hosts as another capability is refused `artifact_hosted_as` and never served under a
  second name. A risk: a long blob pull holds its name for as long as it runs, so a first line that fails delays its
  sibling until it fails.
- **Not built:** an upgrade — a live build stays live whether or not its line is still in the catalogue, and a
  replacement line waits until something withdraws the old build (retire-then-replace is a separate change, D4a of the
  engineering review); one build per name across provisioners — two provisioners on one node (two stems) hold names
  separately; a stale install's brief serving — an install withdrawn while it pulls frees its name at once, and if it
  finishes later its receiver is registered before its completion is refused, so for that moment it answers beside
  the sibling now holding the name (ER5-F1; the one-build guard does not close it); an exact count — self-election overshoots and corrects in later rounds; placement by anything but
  eligibility and the roll; install rights that bind under `Legacy`, `Observe` or `EnforceLocal` (they count what they
  would refuse); anything but WASM components and blobs.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [02, capabilities](../guide/02-capabilities.md); [tutorial 1, your first stem fleet](../guide/tutorials/01-first-stem-fleet.md).
- **Runbook:** [capability lifecycle § Watch — demand pulls the rest in](../operations/capability-lifecycle.md#5--watch--demand-pulls-the-rest-in)
  and [artifacts](../operations/artifacts.md).
- **Design:** [`design/artifact-library.md`](../design/artifact-library.md).
- **Code:** [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).
