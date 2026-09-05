# Persistence durability — three P1 fixes (2026-09-05)

An external code review (Codex, 2026-09-05) of the tree at `cd9e69e` found five issues; the three
persistence ones were **reproduced with deterministic probes** before we touched anything, and
verified against the code line by line (all held). Branch `fix/persistence-durability-p1`.

## The findings (all P1, all present since persistence shipped)
1. **Snapshot erased an acked, fsynced write.** Writer: `ack.send(result)` → threshold
   `do_snapshot` in the same poll. Caller (`kv_set_async`, and the gossip receive path in
   `connection.rs` — so replicated writes too): `wal.append().await` → *then* `apply_and_notify`.
   Scan misses the key, WAL truncated, write gone. Deterministic on a current-thread runtime.
2. **Replay filtered by `timestamp > snapshot_hlc`** — an HLC used as a WAL position. A delayed
   remote update (older HLC, accepted post-snapshot) was dropped on restart.
3. **`Ok` without durability**: `rx.await.unwrap_or(Ok(()))` on a dead writer; `append_sync`
   documented an unconditional fsync but the writer only synced in `Flush`; consensus `let _ =` on
   the result and reported `Committed`.

## What changed
- `persistence.rs`: module-doc durability contract; `WalHandle` acks are `BrokenPipe` on a gone
  writer; `WalMsg::Append { force_sync }` + `wal_append(sync: bool)`; `decode_wal_records` factored
  out of `replay` and reused by `do_snapshot`'s **WAL-tail LWW merge** (`sync_entry_wins` over
  `store::lww_wins`, now `pub(crate)`); replay filter removed; `KvSnapshot::snapshot_hlc`
  informational.
- Apply-then-append at all 10 write sites (`ops.rs` ×4, `connection.rs` ×4, `mailbox.rs`,
  gateway `kv_write`).
- `consensus.rs`: `persist_committed` + `write_lease -> bool` → `Committed { persisted }`
  (additive field; every in-tree matcher uses `{ .. }`); gateway JSON `"persisted"` on propose
  and `overlay/consistent/set`.
- Wiki: [runtime-invariants §Persistence](../architecture/runtime-invariants.md); CLAUDE.md hot
  invariant; CHANGELOG `[Unreleased]`.

## Reusable lessons
- **A timestamp is not a log position.** HLCs order *causally related* events; a WAL orders
  *acceptance*. Filtering one by the other silently drops concurrent history.
- **Ack-then-side-effect in the same poll is a race against every awaiting caller.** Either the
  side effect must not depend on the caller's post-ack work, or the side effect must read the
  caller's input from its own durable record (the WAL merge does the latter).
- **`unwrap_or(Ok(()))` on a channel is a lie by default.** A closed channel is the failure case
  the ack exists to report.

## Not done here (findings 4–5 of the same review)
- `/mcp` (and `/signals/{kind}`, `/bulk/{corr_id}`) are public by routing comment while the
  wiki's security page lists only `/health|/ready|/stats|/metrics` — a design decision + doc
  drift, tracked separately.
- `langgraph-checkpoint-mycelium` `alist` selects rows synchronously (docstring-acknowledged).
