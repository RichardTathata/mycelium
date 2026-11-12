## [2026-11-12] ingest | I4's build opened: a `call:` mandate at the execution authority (PR 1a)

`docs/plans/identity-and-lifecycle.md` row I4 gains its build record (rev 0.19): the split (1a, 1b, 2, 3, 4), the
maintainer's decisions on Q1 (`StopRecord` unchanged; `NamedStop`) and Q2 (the cancel receipt, no receipt on the A2A or
MCP wire), the build's engineering defaults, and exits 7 and 12 reworded.

PR 1a is the authority's half, in `src/agent/gateway_authority.rs` and `src/mandate/grant.rs`:
- `ExecutionAuthority::assess_call` takes lock-order row 43 once and, under that hold, verifies the mandate
  (`check_call_mandate`: scope `call:{term}`, issuer = holder = the envelope's gateway, no entitlement table), inserts
  the scope's window, reads this node's own KV copy of the checkpoint, offers it and the carried one, and judges. The
  read under the hold is what closes N2's race; reading before the hold is the spike's shape and misses a revocation
  written between the read and the insert.
- A `call:` scope's checkpoints are held only where a call was admitted and only until the mandate's window ends.
  `offer_checkpoint` is the one place that enforces "only for a held window" (`CheckpointOffer::NotHeld`), so the
  feeder, a test or an embedder cannot plant a scope the sweep never reclaims.
- The `call:` feeder reads one key per held window on a wake and never scans `call/`: the wake is a coalescing watch
  that carries no key.
- `named_stop_records` names the mandate a stopped registration ran under.

- The one KV record it reads, `call/{gateway}/{term}/checkpoint`, is verified at read (body = key, then the gateway's
  signature); a failing copy is treated as absent and counted (`call_checkpoints_refused`).

Pages: [lock-order](../concurrency/lock-order.md) rows 43, 48 and 49; [owner-signed records](../owner-signed-records.md),
one inventory row. Not built yet: the provider reading `cm` (1b),
the gateway's mint and the `call/` family's owner (2), the cancel doors (3).
