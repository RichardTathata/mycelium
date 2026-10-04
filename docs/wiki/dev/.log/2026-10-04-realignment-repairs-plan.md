## [2026-10-04] ingest | realignment repairs — the review verified, the plan adopted

**What:** `docs/plans/realignment-repairs.md` (new; the index entry in `docs/plans/README.md`; the
CLAUDE.md active-work line) and, in the private repository, `docs/plans/realignment-exporter.md` with
an amendment on its v3 implementation plan. No page changed yet: every correction the plan names
(the persistence invariant, the threat model's egress section, the guarantee catalogue row, the
journal-reader log of 2026-09-16) lands with the increment that makes it true.

**Durable knowledge, recorded now because the code has not changed yet:**

- The KV WAL's torn tail is repaired by the **startup snapshot** (`lifecycle.rs:235-250`), not by
  `replay`; a direct embedder of `replay` + `spawn_wal_writer` is not covered, and the snapshot's
  failure is discarded. Not stated on the persistence page until R2 writes it there.
- The shared `Journal` (`src/agent/journal.rs`) never truncates a torn tail; the v2.10.0 fix was
  counting only, and `a_truncated_tail_does_not_stop_the_node_from_starting` pins the bug. Every
  consumer that folds its journal on open — ledger, durable heads, `DurableEpochs` — fails to start
  once appends outgrow the torn frame. The 2026-09-16 log's "a later read picks it up" is wrong after
  a crash.
- `EgressPolicy::host_of_url` and reqwest disagree on `\` in an authority; every outbound client
  follows redirects; the object-store fetcher gates on the bucket name. The catalogue row and the
  threat model claim more than the code enforces until R3–R5.
- The TypeScript SDK's live suite has never run in CI (`MYCELIUM_TEST_HOST` unset) and fails 7 of its
  own assertions against a real node; the Python SDK is correct on every one of those routes, so the
  Rust gateway is the reference.
