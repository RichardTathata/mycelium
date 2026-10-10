## [2026-10-10] ingest | WASM execution limits — the adversarial review's findings (#599)

- `deadline_ticks` saturates instead of truncating (`Duration::MAX` had meant one tick; a count just under
  `u64::MAX` overflowed wasmtime's unchecked add); `Duration::ZERO` is zero ticks.
- Start-up past the deadline is `WasmHostError::DeadlineExceeded`, not an `Instantiate` string.
- An uninstalled install does not re-instantiate after its in-flight call traps (`Served::gone`).
- Shutdown cancellation of a guest call logs at debug; the long-call test pins `with_call_deadline(None)`;
  the serve loop's deadline path and the stem's `call_deadline_ms` wiring have tests (planted).
- Pages touched: `dev/security.md` (execution limits); the crate README; CHANGELOG.
