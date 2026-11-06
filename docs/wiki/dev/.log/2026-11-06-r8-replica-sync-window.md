# 2026-11-06 — R8: a replica-sync answer covers the apply-to-append window

## [2026-11-06] ingest | external review 2026-10-31, F3 → row R8 (#720)

The receive path applies an inbound record, then appends it (invariant 1, unchanged — decision D3). A
`sys.replica-sync.v1` query between the two answered `Persisted`: the value was in the store, the dropped-append
count (bumped after the apply) was zero, and the WAL sync covered a queue the record had not entered. Reproduced
through the real inbound path in both sync modes with a test-only pause (`WalHandle::pause_inbound_after_apply`).

The fix: the three inbound sites (`Data`, `SignedData`, the anti-entropy entry) share
`connection::apply_inbound_then_append`; `WalHandle::begin_inbound` opens a window before the apply and
`append_inbound` closes it once the record is on the writer's channel; `replica_sync::answer` declines while
`inbound_in_flight` is non-zero. Two things the review's first round found and the PR fixed, worth keeping:

- **The two loads have an order.** An abandoned window counts its drop, then releases the in-flight count with
  `Release`. Reading `dropped_appends` first could see no drop, then — the window closing in between — nothing in
  flight, and answer `Persisted` for the dropped record. In-flight (`Acquire`) first, then dropped. Pinned by a pause
  between the two loads (`WalHandle::pause_answer_between_counts`).
- **Enqueue, not the ack.** The answer's `sync()` is a message on the same channel, handled in order by the one
  writer loop, so a `Sync` sent after the window closed is queued behind the record. Holding the count to the
  `flush` ack only made a slow disk answer `Failed`.

Also found: an assertion through the origin's `collect` that could not fail — its query rode the very connection
whose receive task was paused. Removed; the receipt is built only from `answer()`.

Pages: `dev/architecture/runtime-invariants.md` §Persistence (the window paragraph), `dev/architecture/contracts.md`
(the rung's condition), `docs/design/contracts-receipts.md`, `src/agent/replica_sync.rs` module doc,
`deprecations.md` §81, `metrics.md` (`gossip_wal_inbound_in_flight`).
