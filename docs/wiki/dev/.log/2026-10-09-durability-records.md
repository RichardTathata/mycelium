# 2026-10-09 — four durability/correctness repairs (audit head, WAL poison, snapshot opacity, `persist.sync_mode`)

Branch `fix/durability-records`. Four P2 findings from one review, each verified against the code before
the fix and each with a regression test seen failing first.

- **Audit chain head restarts from seq 0** — `AuditChainState::new()` was the only constructor and
  nothing read `sys/audit/{self}/` back at `start()`; a restart with `[persistence]` overwrote genesis.
  Fixed by `audit::restore_chain_head` (verified fold from genesis or the newest checkpoint; a record
  that does not verify is sealed over, logged at `error`). Readers/writers of `audit_chain`
  (`grep -rn audit_chain src`): `seal_and_write`, `create_checkpoint`, the constructor at
  `agent/mod.rs`, and now `restore_chain_head`. Not covered: a node without persistence that receives
  its own old stream by anti-entropy after start still restarts at 0.
- **WAL writer appends after a failed write** — `WriterState::poison`; every `WalMsg::Append` sender
  (`append`, `append_try`, `append_sync`, `append_acked` → `send_and_await`) is refused until a
  snapshot succeeds; `Sync` answers `Err`; `dropped_appends` is shared with the writer. Test seam
  `spawn_wal_writer_with_fault` mirrors the journal's `open_with_fault`. On-disk format unchanged.
- **A failed snapshot latches the node self-opaque** — steps 2–4 moved to `snapshot_body` so step 5
  runs on every exit; `WriterState::note_snapshot` logs once per distinct failure; `is_self_opaque`
  takes `opaque_freshness_ms` (callers: `consensus.rs is_overloaded`, `emergent.rs ViewConfidence`,
  `lifecycle.rs` defer hook; `consensus_handle.rs` now reads the same helper).
- **`persist.sync_mode` `Enforced` for `Os`** — resolver matches `Flush` only; golden regenerated;
  `deployment.md`, `configuration.md` say `os` is `async` to the writer.

Pages touched: `runtime-invariants.md` §Persistence (item 5 + gates), `what-is-proven.md` row,
`docs/operations/audit.md` §1 + §5, `deployment.md`, `configuration.md`, CHANGELOG.
