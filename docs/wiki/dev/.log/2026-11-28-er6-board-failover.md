## [2026-11-28] ingest | ER6: a promoted blackboard does not bring back, or reuse, what was completed

Engineering review R11-01 and R11-04, with the fence half of ER6-T. A blackboard mirror's sync only added the
snapshot's live facts, so a persistent mirror that returned from its WAL served a fact acked in its absence after a
promotion; and both companions' mirror fences followed live ids only, so a promoted mirror reissued an acked id (ER3
masks it only when the mirror's store is the younger — the reproductions create the mirror's store first). The durable
finding: **a mirror reconciles by positive evidence, never absence** — *retire what the snapshot lacks* destroys a fact
a promoted mirror or a transient primary restarted empty never held. Fix (D6a–D6d): one acked-id memory in the board's
`BoardInner`, rebuilt from the WAL, kept through compaction as lone `Ack`s written before the `Post`s; a snapshot
trailer (next id + acked runs); `fence_past` persisted as a lone `Claim` (compaction keeps it a `Claim`); a digest probe
over vouched facts (four consecutive mismatches start a sync; review round 1 dropped a replication-sequence gate that
never fired under steady writes, folded refused-but-live ids into the digest, and bounded the trailer). In the tuple space every record id fences, and the last state
chunk ends with a lone `Take` — not the `Ack` the design named, which removes a live item and poisons the mirror
index.

Pages: `companions/blackboard.md`, `companions/tuple-space.md`, `concurrency/lock-order.md` (rows 27, 29). Outside the
wiki: the feature pages `blackboard.md`, `tuple-space.md`; `what-is-proven.md#board-failover-retires-acked`;
`deprecations.md` §TBD-ER6; `operations/companions.md`, `operations/admission-control.md`;
`plans/mycelium-blackboard.md`.
