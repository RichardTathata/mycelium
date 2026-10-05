## [2026-10-05] ingest | realignment repairs R7 — no persistence directory, no start

**What:** `src/agent/lifecycle.rs` (an uncreatable persistence directory is a start refusal, not a
warning), a witness in `src/lib_tests.rs`, the changelog, the plan's R7 row (and open R8/R9 rows from
the same audit), and this log.

**Durable knowledge:**

- **A guarantee resolved from configuration alone can be true of the config and false of the node.**
  `persist.configured` checks `config.persistence.is_some()` and is resolved before the persistence
  block runs; the block then failed to create the directory, warned, and ran in memory. Either the
  resolution reads the node's actual state, or every failure after it refuses the start. R7 takes the
  second, which is the v2.20.0 rule: *a refusal at `start()` where a node used to start degraded*.
- **Found by documenting, not testing:** the configuration audit behind A2 traced every field's
  consumer and refusal point, and the persistence path stood out as the one place a configured
  durability promise could silently lapse. The same audit found R8 (`http_port`/`gateway_tls`
  ignored in a gateway-free build, with no test ever run in that build) and R9 (runtime setters that
  bypass `validate()`'s bounds), recorded as open rows.
