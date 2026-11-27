## [2026-11-27] ingest | ER3: a fresh tuple space or blackboard does not reissue ids

Engineering review R4-01: a transient tuple space (the default) or a missing WAL started its ids at 0, so after a
restart a `TupleConsumer` committed new work under an `operation_id` the destination already held — the same bytes
`Replayed` with no write, changed bytes `Conflict` and stuck in flight; a surviving mirror refused the reused ids, and
a stale in-flight key was cleared by a new item's ack. The durable finding: **a destination's dedup compares content,
so it is only as good as the issuer's promise not to reissue an id.** Fix (decision D3a): `fresh_id_base` in both
stores — wall ms << 10 through `mycelium::sim_seam::wall_now_ms`, strictly increasing in-process — wherever a store has
no id history; a WAL with history is unchanged. The limit (fresh starts within N/1024 ms, the 1 024-per-store bump, N
stores ahead of the clock) is named, not gated.

Pages: `companions/tuple-space.md`, `companions/blackboard.md`, `companions/effects.md`. Outside the wiki: the feature
pages `tuple-space.md`, `destination-commit.md`, `blackboard.md`; `what-is-proven.md#fresh-id-space-not-reissued`;
`deprecations.md` §TBD-ER3; `docs/plans/mycelium-tuple-space.md` (step 5's rationale corrected);
`docs/design/contracts-receipts.md` (the tuple row).
