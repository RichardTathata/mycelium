## [2026-11-27] ingest | A live log subscription delivers every entry once (engineering review ER8)

R8-01 and R8-09, built as `docs/plans/engineering-review-urgent-fixes.md` §10 (decisions D10, D10a–D10c).
`subscribe_log` and the gateway's `/log/subscribe` advanced a floor to `hlc + 1`, so a same-stamp sibling or a
lower stamp that arrived after the scan was never sent; both now keep a task-local `LiveCursor`
(`mycelium-core/src/log_cursor.rs`): the floor `since` and the set of key suffixes handed on, pruned to what is still
live on each rescan. Both loops `select!` on `tx.closed()`, the prefix watcher and node shutdown.

Durable knowledge from the build:
- **A live stream cannot use a position.** Any single point in `(hlc, key)` order skips what arrives late and sorts
  before it; row E's `After` position is right for a persisted group offset and wrong here. The live contract is
  therefore arrival order across batches, and a stamp can go down.
- **A task that waits only on a watcher learns of a gone receiver by a failed send** — on a quiet stream, never.
  `tx.closed()` in the `select!` is the fix, with no timer (no sim-seams baseline change).
- **The group consumer keeps the late-row loss** — stated, not fixed (`what-is-proven.md#log-group-skips-a-late-row`).

Pages: `runtime-invariants.md` (the live contract beside the group's), `docs/features/log-overlay.md` (new; registry,
`docs/features/README.md`, `docs/capabilities.md`), `docs/guide/04-consensus.md`, `docs/guide/10-language-bridges.md`,
`docs/operations/what-is-proven.md`, `docs/guide/deprecations.md` §128, `CHANGELOG.md`, the SDK docstrings.
