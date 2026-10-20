# Activation egress proxy

## What it is

A confined [activation](../guide/00-concepts.md#activation) command can reach hosts by name through an egress
proxy its [stem](../guide/00-concepts.md#stem) runs for it on loopback — only the listed hosts, plus whatever its own
`connect` entries name directly — and it starts with a cleared environment. The profile adds the proxy's port to the
command's own `connect` entries, and under Landlock that port is open on any address. The activation's list of hosts
can be replaced while the stem runs, and the proxy attaches a
[credential binding](../guide/00-concepts.md#credential-binding)'s value itself, so the secret never enters
the command.

## When to use it, and when not

- **Use it when** an activation command must call an outside service, such as a model registry or a keyed API,
  and should reach nothing else and hold no secret.
- **Not when** the command runs unconfined (no proxy exists for it); the platform cannot confine it (the activation
  is refused, [unit-file § `[activation.confine]`](../reference/unit-file.md#activationconfine)); you need a limit by
  resolved address or by path; or you need a credential inside a connection the command encrypts itself (a
  `CONNECT` tunnel carries none).

## Turn it on

- In the unit file, give the `[[activation]]` an [`[activation.confine]`](../reference/unit-file.md#activationconfine)
  table and, inside it, [`[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress) with
  `hosts` and, for credentials, `credentials`.
- On the node, declare each binding in
  [`egress_credentials`](../reference/configuration.md#the-fields), and keep each host permitted by
  [`egress`](../reference/configuration.md#the-fields) (`allow_hosts`); a binding's fields are in
  [crown-jewel.md § Credentials attached at egress](../operations/crown-jewel.md#credentials-attached-at-egress-post-360-plan-row-l-2350).
  A stem built without feature `gateway` or `llm` has no egress gate and refuses a `credentials` entry at start.
- To change the list live, send `mycelium-stem` a `SIGHUP` after replacing its unit file, or call
  `Stem::set_activation_egress_hosts` (both in
  [unit-file.md § `[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress)).
- A unit file a test loads: the example in that same section, read from the page by
  `unit_file_md_egress_example_loads_as_written` in [`src/capability_config.rs`](../../src/capability_config.rs).

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. Two tests run it end to end.
[`mycelium-wasm-host/tests/stem_sighup.rs`](../../mycelium-wasm-host/tests/stem_sighup.rs) starts the real
`mycelium-stem` binary, has a confined activation report its proxy, sends requests through it, and rewrites the
unit file under `SIGHUP`; it sends no credential. The credential path is
`activation::tests::a_placeholder_is_replaced_only_at_the_bound_host_and_the_value_is_never_in_the_commands_environment`,
in [`mycelium-wasm-host/src/activation.rs`](../../mycelium-wasm-host/src/activation.rs). The ledger row below names
the tests and where they run.

## What it guarantees, and what it does not

- **Rule:** [`prov.activation_egress`](../reference/rule-catalogue.md#provactivation_egress), and
  [`prov.activation`](../reference/rule-catalogue.md#provactivation)'s refusal `egress_proxy_unavailable` for a
  proxy that cannot start.
- **Guarantees:** `stem.activation_egress` and `stem.activation_confined`, which `mycelium-stem` registers itself,
  so they are not in the [guarantee catalogue](../reference/guarantee-catalogue.md#the-descriptors); their states are
  stated in [unit-file.md § `[activation.confine]`](../reference/unit-file.md#activationconfine) and
  [§ `[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress). Under Landlock (Linux) the
  proxy's port is open on any address, so `stem.activation_egress` reads `not_configured` there. The node's own
  [`egress.credentials` and `egress.allow_list`](../reference/guarantee-catalogue.md#the-descriptors) apply
  through the proxy.
- **Not built:** the *Not built* paragraph of
  [unit-file.md § `[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress), and rows L and M
  of the [post-360 plan](../plans/post-360-hardening.md#33--the-batch), each closed on evidence in
  [§3.5](../plans/post-360-hardening.md#35--delivered-rows-2320-onward--each-promise-beside-its-evidence).
- **Evidence:** the activation egress row of
  [what-is-proven § Proven in CI on every merge](../operations/what-is-proven.md#proven-in-ci-on-every-merge).

## Go deeper

- **Guide:** [09 § Credentials attached at egress](../guide/09-security.md#credentials-attached-at-egress).
- **Runbook:** none for the proxy itself.
  [Crown-jewel § Credentials attached at egress](../operations/crown-jewel.md#credentials-attached-at-egress-post-360-plan-row-l-2350)
  covers the bindings, and [capability lifecycle § Running a stem](../operations/capability-lifecycle.md#running-a-stem)
  covers building and starting the stem.
- **Design:** no design record; the plan's rows L and M (above) are the record. Upgrade notes:
  [deprecations §51](../guide/deprecations.md#51-an-activation-egress-proxy-confinedecl-gained-two-fields-and-profile-one-refusal-a-variant-2360-post-360-plan-rows-l-and-m).
  Maintainer notes: [wiki dev/security § The wasm host's doors](../wiki/dev/security.md#the-wasm-hosts-doors-2026-10-10-pr-586).
- **Code:** [`activation_egress.rs`](../../mycelium-wasm-host/src/activation_egress.rs) (the proxy and
  `egress_guarantee`), [`peer_uid.rs`](../../mycelium-wasm-host/src/peer_uid.rs) (only the stem's own user may
  connect), [`activation_sandbox.rs`](../../mycelium-wasm-host/src/activation_sandbox.rs) (the confinement and
  `confinement_guarantee`), [`stem.rs`](../../mycelium-wasm-host/src/stem.rs) (`set_activation_egress_hosts`,
  `reload_egress_hosts`), [`bin/mycelium-stem.rs`](../../mycelium-wasm-host/src/bin/mycelium-stem.rs) (the
  `SIGHUP` handler).
