## [2026-09-24] ingest | durable knowledge stores over the node-local journal (Boundary H, M2 / K3a)

Up: [dev](../dev.md) · record `docs/design/knowledge-validity.md` §4 (K3a adopted) · code `src/knowledge/durable.rs`,
`src/knowledge/heads.rs`, `src/knowledge/store.rs` · plan `docs/plans/boundary-h.md` (PR #379).

**Finding.** K1's records and K2's checkpoints were in memory, so a restart reset rollback protection. `sim_seam` has
no filesystem seam: filesystem access is admitted only through the baseline, and the substrate already has an
admitted, fsynced, never-gossiped mechanism in `agent::journal` (used by the AE evidence journal and the rights
ledger). The journal's `append` is async, while K2's `CheckpointStore::persist` is sync.

**Change.**
- `DurableKnowledgeStore` and `DurableHeadCheckpoints` over the journal (streams `knowledge/records` and
  `knowledge/heads`), with no new filesystem site and no lock.
- Two-phase cores: `verify_signed` / `insert_verified`, and `evaluate` / `apply`. Persist is awaited to `OnDisk`
  before apply.
- `PutRefusal::NotPersisted`.
- Reopen replays the journal and refuses an unreadable entry. A missing journal is fresh.
- Records restore with their attribution; heads are journalled as deltas.
- Five tests, including rollback refused after a real reopen.

**Kept honest.** No compaction; forks in memory only; node-local, with transport in K3b. The seam gate passes.
