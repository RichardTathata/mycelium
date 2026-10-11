## [2026-10-11] ingest | the review of ee3d9e4b (row A)

- Finding 1: removing the legacy `decision_over` set-aside let a 2.31 permanent leadership, released the legacy way,
  come back as an unreleasable permanent decision. Now `decided` sets aside only acceptances reported raw, and only
  when the legacy reading says over; an adopted legacy value is wrapped in a lease (30 s when the adopter asked for
  permanence). Ported the reviewer's probe as the failing-first test.
- Finding 2 and 7: the renewal rule and "never overwrite a tombstone" each now have a test that fails against its
  mutant. Collection fsyncs the top before tombstoning below it; `GET /consensus/{slot}` reads the record.
- Layer boundary: the scope index is generic (`KvState::with_scoped_prefixes`); the frame filter is its own feature
  (`test-frame-filter`), enabled by CI's lib runs only; anti-entropy entries carry the answering peer.
- Flakes: test meshes dial only earlier nodes (reconnect backoff otherwise outlasts the wait); row 4 waits for M.
- Pages touched: `docs/design/lock-lifecycle.md` (§2.2 Layer I wording, §7 the set-aside rule, §8 steady state),
  `dev/architecture/runtime-invariants.md`.
