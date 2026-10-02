## [2026-09-30] ingest | F1: fuel by publisher (D19)

**What:** `docs/plans/design-time-tooling.md` §17 F1. `WasmHost::metered()` (count without a default
budget) + `instantiate_with_fuel`/`provision_with_fuel` (per instance); `FuelPolicy` on
`WasmComponentRuntime` / `Provisioner::set_fuel_policy`, decided per entry from its verified `signer`;
`WasmHostError::FuelExhausted { budget }` from the `OutOfFuel` trap; the runtime's invocation log
(`InvocationRecord`/`InvocationOutcome`, lock-order row 50, counter
`mycelium_artifact_invocations_total{outcome}`); `[hosts].operator_publishers` /
`operator_fuel_per_call` with validation; the stem wires it. Fixture `spin_component.wasm` (a guest that
never returns) beside the echo one, built the same out-of-band way.

**Durable knowledge:**
- **The classification rests on provenance.** Operator-vs-agent is the entry's signer, which is inside
  the signature and checked against `trusted_publishers` before install. Without a trusted list a
  signer is a claim, so `validate()` refuses `operator_publishers` without `trusted_publishers`, and
  any operator key outside it.
- **Fuel is an engine property; the budget is an instance property.** wasmtime enables `consume_fuel`
  per engine, so one node running both metered and unbounded entries needs a metered engine with the
  unbounded instances refuelled to `u64::MAX` each call. A budget requested on an unmetered host is
  refused by name rather than ignored.
- **The record is node-local and crate-local.** The evidence journal (`AeEvidence`) lives behind
  `gateway`+`tls` in the core crate and is a gateway-side enforcement record; a serve loop in the
  wasm-host crate does not reach it. The stop is recorded where it happens, with the budget named, and
  the RPC reply carries the same words; a gateway that fronted the call records that reply as it
  would any failed execution.
- **A stopped call is not a dead install — but only because the instance is replaced.** Found by
  the gate's last assertion: a trap **poisons** a wasmtime component instance (the next call answers
  *cannot enter component instance*), so before this the serve task survived while every later call
  failed — a dead install behind a live advertisement. The serve loop now keeps the verified bytes
  and instantiates afresh after a fuel or host trap (`Reinstantiate`, `runtime.rs`); if that fails it
  stops serving so the probe withdraws and reinstalls. The guest's in-memory state is lost with the
  instance; its KV subtree is not. The next call gets a fresh budget — the bound is per call.

**Pages touched:** `mycelium-wasm-host/README.md`, lock-order table (row 50), plan F1 row, CHANGELOG.
