# Declared activation

## What it is

An [activation](../guide/00-concepts.md#activation) is how a placed blob reaches a native runtime already on the node —
model weights into a model server, a binary into a supervisor. After a blob that provides `ns/name` is placed and
verified, the stem runs the unit file's `command`, then gates the capability on its `probe`: a failing first probe fails
the install and nothing is advertised, and the stem keeps re-running the probe, withdrawing the install when it fails so
a later round reinstalls. A command can name the placed file, its directory, its content address and a rendered copy in
which every `artifact:<hex>` reference is replaced by that artifact's placed path.

## When to use it, and when not

- **Use it when** a capability is served by a runtime the WASM sandbox cannot host — a model server, a native tool — and
  its bytes should still travel as a signed artifact.
- **Not when** the command is not one you would run yourself: it is the operator's, run with the stem's privileges
  unless the unit names a confinement profile ([confined activations](confined-activations.md)), and confinement
  narrows what a command reaches without making an untrusted command safe.

## Turn it on

Declare the activation in the host's unit file, with `"blob"` in `[hosts].kinds`. The model host's
[`examples/units/model_deploy/model-host.toml`](../../examples/units/model_deploy/model-host.toml), which CI's wire-check
reads, declares one for an Ollama model.

- `[[activation]]` ([unit-file § activation](../reference/unit-file.md#activation)): `ns`, `name`, `command` (an argv, no
  shell unless you name one), `probe` (empty means *the placed file exists*; 30 s timeout), `timeout_secs` (default
  300) and `resolve_artifact_refs`.
- Placeholders `{path}`, `{dir}`, `{artifact}`, `{ns}`, `{name}`, and `{rendered}` only with `resolve_artifact_refs`;
  any other is refused.
- The stem re-runs each probe every 10 s by default (`reprobe_every` in
  [`StemOptions`](../../mycelium-wasm-host/src/stem.rs)).
- `a_failing_initial_probe_is_an_activation_error`, in
  [`mycelium-wasm-host/src/activation.rs`](../../mycelium-wasm-host/src/activation.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`model_deploy`](../../examples/README.md#the-capability-matrix): real weights and a profile placed and activated into
Ollama, the profile's `artifact:` reference rendered to the weights' placed path; it needs Ollama, so it is a manual
run. The run CI does is `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`, in
[`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs).

## What it guarantees, and what it does not

- **Rules:** [`prov.activation`](../reference/rule-catalogue.md#provactivation) — `activated`, or refused
  `command_failed`, `command_timed_out`, `reference_not_placed_yet`, `render_failed`, `confinement_unavailable`,
  `confinement_failed` or `egress_proxy_unavailable`; [`prov.probe`](../reference/rule-catalogue.md#provprobe) —
  `healthy`, or refused `initial_probe_failed`, `probe_failed`, `confinement_unavailable`, `confinement_failed` or
  `confinement_changed`.
- **Not built:** a sandbox by default — an unconfined command runs with the stem's privileges and inherits its
  environment; the probe's health is the command's own exit code, not an observation of the runtime's behaviour.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [09, security](../guide/09-security.md) on what an activation command may reach.
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md) and [artifacts](../operations/artifacts.md).
- **Design:** decision D21 in [`plans/design-time-tooling.md`](../plans/design-time-tooling.md).
- **Code:** [`mycelium-wasm-host/src/activation.rs`](../../mycelium-wasm-host/src/activation.rs) (`hook`, `spawn_reprobe`).
