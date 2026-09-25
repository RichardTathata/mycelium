## [2026-09-25] ingest | closure plan C8: a restart does not restore revoked authority

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §4,
`docs/plans/boundary-h-closure.md` C8 · code `src/mandate/authority.rs` (`RevocationView::started_at`,
`DurableEpochs`), `src/agent/gateway_authority.rs`, `mycelium-wiki/src/execution.rs` · lock-order row 47.

**Two things a restart forgot.** The revocation view, so a still-fresh checkpoint from before a revocation,
replayed after the restart, was believed; and the installed epoch, so a superseded mandate passed again.

**Fixes.**
- A reader records when it started and takes freshness only from checkpoints issued since. That is sound because
  checkpoints are cumulative, which is now the authority's written contract. Revocations in older checkpoints still
  count, because #402 made revocation order-independent.
- Epochs go to the node-local journal before they take effect, and come back as a floor.

**Why not persist the whole revocation view instead.** The start rule needs no new state and fails closed. Persisting
the view would need the journal on the hot path of every checkpoint offer, for a window that is at most one
checkpoint interval.
