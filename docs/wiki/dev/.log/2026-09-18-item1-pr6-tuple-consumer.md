## [2026-09-18] ingest | item 1 PR 6 — the tuple-space consumer with effect recovery

Up: [dev](../dev.md) · record `docs/design/contracts-receipts.md` §7 (the tuple-space row) · code
`mycelium-effects/src/tuple_consumer.rs`, `mycelium-effects/tests/recovery.rs`.

### What landed

A worker over one tuple space and one destination: `take`, commit the effect, then `ack` or `complete`. The
operation id is derived from the item and survives re-delivery, so the second apply is `Replayed`. Behind the
`tuple-space` feature, so the effects crate's core stays uncoupled from both companions.

### Two things worth carrying forward

1. **The replay test does not pin the ordering; the refusal test does.** A consumer that acknowledged before
   committing passes "re-delivered item replays" just the same — the difference appears only when the effect
   is refused, where ack-first loses the item. So the pin is: refuse everything, restart over the WAL, and the
   item must come back with its id intact. Written after noticing the first test's blind spot, and verified by
   planting the inversion.
2. **The crash model is the tuple space's own.** Its lifecycle probe models a dead worker as a restart over the
   same WAL, which re-queues every unacknowledged item with its id intact — no thirty-second lease wait, and the
   same path a real restart takes. Reusing that model kept the tests fast and honest.

### Not done here

Gateway/SDK parity for the receipts (PR 7); a dead-letter policy for `Conflict` (the consumer surfaces it and
does not decide).

### Pages touched

- [history.md](../history.md) — a PR 6 paragraph under the item 1 PR 5 section.
- [companions/companions.md](../companions/companions.md) — the `mycelium-effects` bullet mentions the consumer.
- The receipts record's §7 tuple-space row carries a dated note that its sentence is now code.
