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
  refused, compaction `sync_data → rename → fsync_parent` (it used `std::fs::write` +
  `rename`, neither synced).
- **Gates:** `cargo test -p mycelium-blackboard --features gateway` (+ clippy); `microgrid`
  example + CI smoke; cross-node `tests/board_failover.rs`; gateway `POST /gateway/bb/*` +
  `GET /gateway/bb/depth`; py/ts SDKs.
