# Hosted WASM components

## What it is

`mycelium-wasm-host` runs a capability shipped as a WASM component, sandboxed in the node's own process by wasmtime and
pulled by content address. A component reaches the node only through the imports of its WIT world — a confined KV, a
confined emit, a log — never the node's own handles: its KV lives under `comp/{node}/{namespace}/…`, a subtree it cannot
leave, and it may emit only kinds under `comp/{namespace}/…` (or kinds the host listed for it) and never a protected
RPC kind, because an emit from inside the process would reach that kind's handlers past the door that checks
authority. Each instance has a memory cap and a wall-clock deadline per call; where `fuel_per_call` or
`operator_fuel_per_call` is set, fuel is metered by who signed the entry, and with neither set nothing is metered;
and guest code runs on a blocking thread, never on the node's async workers.

## When to use it, and when not

- **Use it when** a capability is small, portable code that should arrive as data and run anywhere a host can load it —
  a tool, a transform, a policy check — including one an agent wrote.
- **Not when** it needs the network, the filesystem or a native runtime: the world gives none of them, and a
  [declared activation](declared-activation.md) of a blob is the path for a native runtime.

## Turn it on

This feature is turned on in code, or by a [stem](stem.md) with `"wasm-component"` in `[hosts].kinds` — a stem without it
neither installs nor stages a component (`kind_not_declared`); `wasmtime` is confined to this crate and never enters
`mycelium` or `mycelium-core`.

- `[hosts]` ([unit-file § hosts](../reference/unit-file.md#hosts)): `fuel_per_call` for entries an agent signed,
  `operator_fuel_per_call` for the operator's own keys, and `call_deadline_ms` (default 5 s) for every call.
- In code, [`WasmHost::new`](../../mycelium-wasm-host/src/host.rs) and a [`HostState`](../../mycelium-wasm-host/src/host.rs)
  per component, with [`with_protected_kinds`](../../mycelium-wasm-host/src/host.rs) for the node's own
  `protected_rpc_kinds` — only the built-in protected kinds are refused without it; the provisioner's runtime passes the
  node's list — and [`with_emit_kinds`](../../mycelium-wasm-host/src/host.rs) for further kinds (not settable from a
  unit file) and a memory cap of `DEFAULT_MEMORY_LIMIT_BYTES` (256 MiB) unless set.
- The world is [`mycelium-wasm-host/wit/host.wit`](../../mycelium-wasm-host/wit/host.wit).
- `a_component_cannot_emit_a_protected_kind_but_can_emit_in_its_own_namespace`, in
  [`mycelium-wasm-host/src/host.rs`](../../mycelium-wasm-host/src/host.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`mcp_toolgrowth`](../../examples/README.md#the-capability-matrix): an agent needs a tool nobody offers, a dark tool host
installs a WASM component that arrives over the mesh, and the component is bridged as an MCP tool. CI runs it.

## What it guarantees, and what it does not

- **Rule:** [`host.emit_admission`](../reference/rule-catalogue.md#hostemit_admission) — `admitted`, or refused
  `protected_kind`, `foreign_kind` or `malformed`; a refused emit is dropped and counted
  (`mycelium_wasm_host_emits_refused_total{reason}`).
- **Not built:** a component's resource use beyond memory, fuel and the deadline; a guest-to-guest boundary beyond the
  per-component namespace; anything but what the world imports — the host is only as safe as wasmtime, which is why
  its advisories are releases here. A call past its deadline or fuel is stopped and recorded by name.
- **Evidence:** the ledger rows
  [a hosted guest call is bounded in wall-clock time](../operations/what-is-proven.md#guest-call-bounded-in-wall-clock-time)
  and [a looping entry is stopped at its fuel budget](../operations/what-is-proven.md#looping-entry-stopped-at-fuel-budget).

## Go deeper

- **Guide:** [16 § Agent-authored functions](../guide/16-guardrails.md#agent-authored-functions-the-legitimate-uses-of-dynamic-load).
- **Runbook:** [artifacts § Trust and provenance](../operations/artifacts.md#trust--provenance) for fuel by publisher.
- **Design:** the crate's [README](../../mycelium-wasm-host/README.md) (the host ⇄ component boundary) and
  [`design/artifact-library.md`](../design/artifact-library.md).
- **Code:** [`mycelium-wasm-host/src/host.rs`](../../mycelium-wasm-host/src/host.rs) and
  [`confine.rs`](../../mycelium-wasm-host/src/confine.rs).
