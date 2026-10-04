## [2026-10-04] ingest | realignment repairs R1 — the journal truncates its torn tail and owns its file

**What:** `src/agent/journal.rs` (`open` takes an exclusive lock on `<path>.lock`, scans to the last
complete frame, truncates there with file and directory synced, then spawns the writer; one write
per frame; a failed append poisons the writer until a reopen), `src/agent/journal_repair_tests.rs`
(seven witnesses, each seen failing on `5e4dd12b`), the pinning test
`a_truncated_tail_does_not_stop_the_node_from_starting` inverted to read its record back,
`rust-version` 1.88 → 1.89 across the workspace (`std::fs::File::try_lock`), the sim-seam baseline
row for the journal 5 → 12 with its inventory entry (§2.4), `testing.md`'s journal row, and this
log. The persistence invariant page and the threat model wait for R2 and R5.

**Durable knowledge:**

- **The v2.10.0 journal fix was counting only.** `a_torn_tail_is_not_counted_as_a_record` made
  `count_records` and `read_journal_from` agree about *how many* records a torn file holds; nothing
  removed the torn frame, so the next append was acknowledged behind it and the frame's claimed
  length swallowed the new bytes. The log of 2026-09-16 reasoned "when the writer finishes the record
  a later read picks it up" — right for a live write, wrong after a crash, where nothing ever
  finishes it. Logs are not edited; this entry is the correction.
- **Every folding consumer fails to start once appends outgrow the torn frame:** the evidence
  journal, the rights ledger, the knowledge durable stores and `DurableEpochs` — the last carrying
  v2.15.0's *a restart does not restore revoked authority*. While the claimed length still exceeds
  the appended bytes the new records are silently invisible instead. Regression:
  `an_epoch_recorded_after_a_torn_tail_survives_a_restart`.
- **Ownership is the OS's lock, held by the handle, not the writer task.** The last `Arc` dropping
  releases it synchronously, which is what lets a restart reopen without racing the old task's
  shutdown. The one window it does not cover: a write still in flight past `ACK_TIMEOUT` when the
  handle drops. The private RA outbox has used this exact pattern since RA3; the public WAL takes it
  in R2.
- **The scan cannot see corruption inside a complete frame** — there is no checksum on disk — so a
  frame of the right length and the wrong bytes reaches the consumer, which refuses it by name on
  decode. A journal written before R1 with an acknowledged record already behind a torn frame is in
  that class; R1 repairs nothing retroactively.
- **A fault seam for the one failure production cannot produce on demand:** `open_with_fault`
  (test-only) turns the next frame write into a short write plus an error. The poisoning test was
  seen failing with the poisoning line toggled off (the record acknowledged `OnDisk` at seq 1, then
  gone after reopen), the way `feedback_fail_first_toggle_touch` prescribes.
