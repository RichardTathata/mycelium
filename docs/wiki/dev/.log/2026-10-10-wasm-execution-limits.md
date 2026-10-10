## [2026-10-10] ingest | WASM execution limits (plan row D)

- `mycelium-wasm-host`: epoch interruption on every engine, a per-call and per-instantiation wall-clock
  deadline (`DEFAULT_CALL_DEADLINE` 5 s; `WasmHost::with_call_deadline`, `[hosts].call_deadline_ms`) ending in
  `WasmHostError::DeadlineExceeded`; guest calls (and install's compile/instantiate/`describe`) on
  `spawn_blocking`; a trapped instance replaced from the install's compiled component, not recompiled.
- The epoch ticker is a thread per host (weak engine reference) — recorded in
  `docs/design/replay-nondeterminism-inventory.md`: a deadline outcome does not replay, fuel does.
- Pages touched: `dev/security.md` (the wasm host's doors: execution limits); `docs/reference/unit-file.md`
  (`call_deadline_ms`); `docs/operations/what-is-proven.md` (one row); `docs/operations/artifacts.md`;
  guide 16; the crate README.
