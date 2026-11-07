# Confined activations

## What it is

An `[activation.confine]` table runs an [activation](../guide/00-concepts.md#activation)'s command and probe under OS
confinement — Seatbelt on macOS, Landlock with seccomp on Linux — instead of with the stem's own privileges. The
command reads only the system set, the placement directory and the paths it declares, writes only its declared write
paths, connects only to its declared TCP ports (and, with egress, its proxy), and starts with a cleared environment. A
profile this node cannot apply refuses the activation by name and runs nothing — never unconfined. An empty table is
the strictest profile: the placement directory read-only and no network.

## When to use it, and when not

- **Use it when** an activation command should not be able to read the node's keys or configuration, write outside its
  directory, or reach the network beyond what it names.
- **Not when** you need resource limits — CPU, memory, processes — or limits by address and path: those are not built
  (row M of the [post-360 plan](../plans/post-360-hardening.md)); and on Linux a `localhost:<port>` is refused, because
  Landlock limits a port, not an address.

## Turn it on

Add the table to each `[[activation]]` in the unit file; a stem registers the guarantee that reports it.

- `[activation.confine]` ([unit-file § activation.confine](../reference/unit-file.md#activationconfine)): `backend`
  (`auto`, `seatbelt` or `landlock`), `read`, `write`, `connect` (`localhost:<port>` or `*:<port>`; a hostname is
  refused — name it under egress), `egress`, and `env` (variables passed through to the cleared environment).
- Paths are absolute or start with `{dir}` or `{path}`, with no `..`; each must exist when the activation runs.
- An unknown key in the table, the `[[activation]]` or the file is refused at load, so a misspelt table cannot load as
  no profile ([deprecations §49](../guide/deprecations.md#49-a-unit-file-with-an-unknown-key-is-refused-not-read-without-it-2350-post-360-plan-row-m)).
- Hosts reached by name go through the [activation egress proxy](activation-egress-proxy.md).
- `a_confined_activation_reads_and_writes_only_what_its_profile_names`, in
  [`mycelium-wasm-host/src/activation.rs`](../../mycelium-wasm-host/src/activation.rs): CI sets
  `MYCELIUM_CONFINE_LIVE_REQUIRED`, so it fails rather than skips where the backend is missing.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for confined
activations. The end-to-end run is a test in [`mycelium-wasm-host/src/activation.rs`](../../mycelium-wasm-host/src/activation.rs):
`a_confined_activation_reaches_only_its_declared_tcp_ports`, run by CI on Linux (Landlock and seccomp) and on macOS
(Seatbelt).

## What it guarantees, and what it does not

- **Guarantee:** `stem.activation_confined`, registered by the stem, not in the core catalogue — `enforced` when every
  `[[activation]]` names a profile this node can apply, `not_configured` when one is unconfined or names one it cannot,
  `not_applicable` with none (`confinement_guarantee` in
  [`mycelium-wasm-host/src/activation_sandbox.rs`](../../mycelium-wasm-host/src/activation_sandbox.rs)).
- **Rules:** a profile that cannot be applied is [`prov.activation`](../reference/rule-catalogue.md#provactivation)'s
  `confinement_unavailable` or `confinement_failed`, and a re-probe whose declared path was replaced is
  [`prov.probe`](../reference/rule-catalogue.md#provprobe)'s `confinement_changed`.
- **Not built:** resource limits; limits by address or path; the OpenShell backend (decided *skip for now*,
  2026-10-17); under Landlock no DNS, so a name resolves only through `/etc/hosts`. Confinement narrows what a command
  reaches; it does not make an untrusted command safe.
- **Evidence:** the ledger row
  [an activation command runs under its confinement profile](../operations/what-is-proven.md#activation-command-runs-under-confinement-profile).

## Go deeper

- **Guide:** [09 § Credentials attached at egress](../guide/09-security.md#credentials-attached-at-egress), for the egress
  half.
- **Runbook:** [unit-file § activation.confine](../reference/unit-file.md#activationconfine) holds the per-backend
  table; there is no separate runbook.
- **Design:** no design record; row M of the [post-360 plan](../plans/post-360-hardening.md) is the record.
- **Code:** [`mycelium-wasm-host/src/activation_sandbox.rs`](../../mycelium-wasm-host/src/activation_sandbox.rs) (the
  backends and the guarantee) and [`activation.rs`](../../mycelium-wasm-host/src/activation.rs).
