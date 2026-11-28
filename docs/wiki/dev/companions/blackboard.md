# mycelium-blackboard — content-routed shared working memory

↑ [companions](companions.md) · sibling: [tuple-space](tuple-space.md)

The content-routed sibling of the tuple space: routes by a **predicate over fact
attributes** where the tuple space routes by lane position. WS-G G3, PRs #95–#100. Design:
`docs/plans/v2-wsg-g3-blackboard.md`.

- **The one new primitive is `claim(predicate)`** — competitive destructive
  claim-by-predicate (Linda's `in`): exactly one agent wins; **non-blocking** (loser gets
  `None`, no parked waiters — unlike the tuple space's blocking `take`). `read` (`rd`) is
  shared; `ack` is the idempotent terminal; `release`/deadline re-queue gives
  at-least-once.
- **Predicate language** = the capability attribute-filter grammar (equality + presence),
  not unification.
- **Two layers:** `BoardStore` (pure core + WAL; `transient()`/`persistent()`) and
  `Blackboard` (roles + RPC + failover; `BoardRole` mirrors `TupleRole`).
- **Replication is `Post`/`Ack`-only** — the deliberate divergence from the tuple space: a
  `Claim` doesn't change a mirror's liveness, so a claimed-but-unacked fact stays claimable
  after promotion (= the at-least-once re-queue you want). No heartbeat, no WAL cursor;
  snapshot-on-join + live replication keep the mirror complete.
- **WAL:** magic `MBBWAL`; replay liveness = Posted-and-not-Acked. Integrity as the tuple
  space's (row C, 2026-10-10, `wal.rs`): one owner (`OwnershipLock`), a failed append
  poisons until compaction or reopen, torn final frame truncated vs corrupt-with-data-after
  refused, compaction folds the log itself (not memory) and installs with `sync_data →
  rename → fsync_parent` (it used `std::fs::write` + `rename`, neither synced). Maintenance
  runs on every WAL holder, secondaries included; a refused mirror fact re-syncs from the
  primary (`mirror_gap`); the id high-water mark survives compaction (#597 review). A board with no id history starts at `fresh_id_base` (ER3, 2026-11-27; the tuple space's rule — [tuple-space](tuple-space.md)).
- **Failover reconciles by positive evidence, never absence** (ER6, 2026-11-28, R11-01/R11-04). *Retire what the
  snapshot lacks* is wrong: absence is not an ack, and that rule destroys an unacked fact a promoted mirror or a
  transient primary restarted empty never held. Instead **one acked-id memory, in `BoardInner`** (`wal.rs`
  `Tombstones`, 100 000, a set — never a dense range: ids across lives jump by ms × 1024): `ack` and `discard` record
  it (discard whether or not the fact was present, logging the `Ack` only on first insertion); `fold_wal` rebuilds it
  from every `Ack`, a later `Post` forgetting the id; compaction writes the newest 16 384 as lone `Ack`s **before** the
  `Post`s, and keeps the high-water mark a `Claim` unless the id is a known acked id. `post_with_id` refuses a
  remembered id (counted, `refused_acked`, never `mirror_gap` — no resync cures it) and is a no-op for a held one; the
  old `MirrorIndex` is gone. The **snapshot** (and only it — `read` shares `facts_to_bytes`) gains a trailer
  `[u8 1][u64 next_id][u32 n] n×([u64 start][u32 len])`; `apply_snapshot` discards named ids, adds facts, then
  `fence_past(next_id)` — persisted as a lone `Claim { n-1 }`, which never changes liveness in any order. A running
  mirror probes with body `[1]` (digest only: `[u8 2][u64 rep_seq][u64 count][u64 xor-mix]`) every 3 ticks; a mismatch
  at the same replication sequence twice → full sync. The digest skips **unvouched** facts (held, neither carried nor
  named acked by the last full snapshot — kept and counted, D6c). ER6 needs ER3: a primary restarted at 0 would name
  its new life's ids acked over a mirror's old-life facts.
- **Gates:** `cargo test -p mycelium-blackboard --features gateway` (+ clippy); `microgrid`
  example + CI smoke; cross-node `tests/board_failover.rs`; gateway `POST /gateway/bb/*` +
  `GET /gateway/bb/depth`; py/ts SDKs.
