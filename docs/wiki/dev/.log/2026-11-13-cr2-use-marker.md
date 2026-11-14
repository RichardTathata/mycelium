## [2026-11-13] ingest | CR2, first PR: the grant use marker a member cannot re-arm

`docs/plans/external-critique-2026-11-12.md` row CR2 records the first PR as delivered, and one decision.

- A door's use of a grant is kept in **its own journal** (`grants/uses.journal`, stream `grant/uses`;
  `GrantUseLedger`, `src/agent/grant_use.rs`), fsynced before the call is admitted, read before the KV and read back
  at a restart. Lock-order row 79 is its fold.
- At the marker key `grant/{holder}/{term}/used/{point}` **anything other than *never written* reads used**
  (`reads_used`): a verified marker (now a record signed by the door's node), a value that does not verify, a
  tombstone, an old-form marker, a release made elsewhere. The reader takes the store's entry, not `kv_get`: a
  tombstone is an entry with no data. Eight counted readings on `/stats` `grant_use` (seven of the key, one of the store refusing the marker).
- The door reads its marker write's receipt (`write_owned_kv_reporting`) and refuses `grant_use_superseded` on
  `Superseded`.
- **Decision (maintainer, 2026-11-13): a release is honoured only by the door that made it.** The key names no
  node, so every door of a class shares it, and a reader without the use on record cannot tell the release of the
  newest use from an older genuine release written back, from a member's own marker-and-release pair, or from a
  release that overlapped another door's admission. So the releasing door's journal is the reason a grant reads
  unused; the signed release in the KV is for the other doors (which read it as used), an auditor and a collector.
  **Consequence:** a retry after a give-back must return to the same gateway (or provider) node.
- The inventory row is **Partly**, not Meets: (c) fails at a door that never recorded the use once a planted
  tombstone is collected (3 000 s at the defaults, inside a grant's 3 600 s cap).
- Nothing collects `grant/` keys yet, and the journal is not compacted: both due with I4's collector.

Pages touched: `owner-signed-records.md` (the row, the six items, *Using this page*), `concurrency/lock-order.md`
(row 79, row 70), `docs/features/grants.md`, `docs/guide/deprecations.md` §106, `docs/operations/metrics.md`,
`observability.md`, `what-is-proven.md` (the first *Not yet shown* row, narrowed).
