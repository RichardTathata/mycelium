# The stem node

## What it is

A [stem](../guide/00-concepts.md#stem) is a generic node that declares from its unit file and holds nothing
application-specific at deploy time. It advertises every `[[capability]]` the file names, declares every
`[[requirement]]` and defines every `[[group]]`; with a `[hosts]` table it also runs a provisioner that installs signed
artifacts — WASM components and blobs — when the fleet's demand or a presence band calls for them, and withdraws them
when it no longer does. A stem fleet is many stems from one image, each holding only its own file. `[[lane]]`,
`[[mandate]]` and `[[rule]]` are vocabulary for the offline check and the evaluator; a stem does nothing with them at
runtime and says so once at start — unless its configuration's `[authority]` names its unit, when `start()` builds the
evaluator from that unit's `[[rule]]`s (below).

## When to use it, and when not

- **Use it when** capabilities should come and go with demand on generic hosts — a model, a tool, a component — and
  you would rather publish an artifact than roll an image.
- **Not when** the capability is new native code: only WASM components and blobs for a runtime already on the host load
  dynamically, so anything else is a new image and a rolling upgrade
  ([capability lifecycle § What this page does not claim](../operations/capability-lifecycle.md#what-this-page-does-not-claim)).

## Turn it on

Run the `mycelium-stem` binary — published in every release tarball and the image, built with
`stem,gateway,llm,object_store,compliance` — with a unit file; from source, build `mycelium-wasm-host` with its `stem`
feature.

The co-op's provisioning declarations, such as
[`examples/units/provisioning/provider-a.toml`](../../examples/units/provisioning/provider-a.toml), are among those the
stem-examples Docker suite runs as stems, in a path-filtered workflow (`cluster-suites.yml`).

- `mycelium-stem --units <file>`, with `--library` (a directory or object-store URL; without it, artifacts are pulled
  over the mesh), `--config`, `--peers`, `--tick-ms`, `--self-elect` and `--trace-dir`
  ([capability lifecycle § Running a stem](../operations/capability-lifecycle.md#running-a-stem)).
- **Credentials and authority:** with [`compliance`](../../mycelium-wasm-host/Cargo.toml) (in the published set), a
  stem's configuration takes named and scoped gateway tokens, `[oidc]` and
  [`[authority]`](declared-authority.md); a stem built without it refuses each at start by name. `compliance` brings
  mycelium's `tls`, so the published stem also takes `[tls]` and `[gateway_tls]` and can join an mTLS fleet, which a
  2.39.0 stem refused. The stem calls
  `GossipAgent::start()`, so `[authority]` is planned and attached there — the evaluator from the unit's `[[rule]]`s,
  the execution authority, provider enforcement and the evidence journal — as on the `mycelium` binary
  (`a_compliance_stem_starts_with_a_token_table_and_attaches_its_authority`, in
  [`mycelium-wasm-host/tests/stem_compliance.rs`](../../mycelium-wasm-host/tests/stem_compliance.rs)).
- The `[hosts]` table ([unit-file § hosts](../reference/unit-file.md#hosts)) is what makes a stem install anything;
  without it, it only declares ([demand-driven provisioning](demand-provisioning.md)).
- In code, [`Stem::start`](../../mycelium-wasm-host/src/stem.rs) with [`StemOptions`](../../mycelium-wasm-host/src/stem.rs).
- `the_stem_fleet_fills_a_presence_floor_and_reheals`, in
  [`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`first_stem_fleet`](../../examples/README.md#the-capability-matrix): three generic hosts keep two providers of a
signed echo component, invoked by discovery, and recover when one goes. CI runs it, and the
[first tutorial](../guide/tutorials/01-first-stem-fleet.md) walks it.

## What it guarantees, and what it does not

- No guarantee id of its own: the stem registers its activation guarantees (on the
  [activation pages](declared-activation.md)) and its provisioning rules are
  [demand-driven provisioning](demand-provisioning.md)'s.
- **Not built:** the catalogue is the supply chain, so `[hosts].trusted_publishers` is the posture to review; a stem's
  placement is decided at runtime, never by the file; a stem built without `llm` refuses a unit file with a `[[serve]]`.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [tutorial 1, your first stem fleet](../guide/tutorials/01-first-stem-fleet.md) and
  [tutorial 6, declared versus observed](../guide/tutorials/06-declared-versus-observed.md).
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md) (declare, check, deploy, publish, watch).
- **Design:** [`plans/design-time-tooling.md`](../plans/design-time-tooling.md) §13.
- **Code:** [`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs) and the binary
  [`mycelium-wasm-host/src/bin/mycelium-stem.rs`](../../mycelium-wasm-host/src/bin/mycelium-stem.rs).
