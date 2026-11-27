## [2026-11-27] ingest | Tuple space: a FIFO queue per key, for items and for waiters (engineering review ER7)

`keyed_entries` and `keyed_waiters` (`mycelium-tuple-space/src/store.rs`, `StageInner`) are now maps to `VecDeque`s.
Every path that inserts into a keyed slot pushes — `put_keyed`, `complete_keyed`, `put_with_id` (mirror apply),
`requeue_expired` and WAL replay — and `take_by_key` pops; `dispatch_keyed` is the same pop-waiter / in-flight / send /
undo loop as `dispatch`. Before, one slot per key: a second same-key item overwrote the first (hidden, depth leaked,
back only on WAL reopen) and a second same-key waiter evicted the first.

Durable knowledge from the build:
- **A refusal could not be the rule:** replication and replay must not drop, a requeue must put the item back and
  `complete_keyed` has already consumed its input — so the store holds several items per key on four of its paths
  whatever is decided; a queue is the only total rule (decision D7a).
- **A cold key must not leak:** a timed-out `take_by_key` closes its receiver, drains it (`try_recv`), then prunes
  closed senders under the stage lock and removes the entry if empty (`prune_keyed_waiters`). Registration prunes too
  (dropped, cancelled takes).
- **Close before the last drain** — on the plain `take` too. Without `rx.close()` a dispatcher that popped the sender
  could still send after the failed `try_recv`, into a receiver about to be dropped: the item sat in flight, ownerless,
  until its lease expired.
- **`take_total` is counted once, where an item enters in flight** (a successful send in `dispatch`/`dispatch_keyed`,
  and the buffered pops), never by the producer and the woken taker both. It counts deliveries, redeliveries included,
  so `taken <= admitted` is not an invariant.
- **Keyed waiters count in `waiters`** (`waiters_count`: plain deque length plus every keyed queue's length), as the
  G1a design said and the R11-11 report asked; `TupleDepth::waiters`' doc says so.

Pages: `docs/wiki/dev/companions/tuple-space.md`, `docs/features/tuple-space.md`, `docs/operations/admission-control.md`,
`docs/plans/v2-wsg-coordination.md` (G1a's waiter line), `docs/plans/mycelium-tuple-space.md` (`take_total`),
`docs/guide/deprecations.md` §TBD-ER7, `CHANGELOG.md`.
