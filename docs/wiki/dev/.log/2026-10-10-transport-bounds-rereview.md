## [2026-10-10] ingest | #602's adversarial re-review, folded into row B

- Nine findings: `sys/quorum/` evidence only for a kind with a local worker or a live query pin (the
  `*_persistent` reads pin), and never suppressed for one; read and write progress bounds split —
  `peer_read_stall_timeout_ms` 60 s, `peer_write_stall_timeout_ms` 600 s, the read floor off by default;
  taps never exempt a kind; query pins lapse after a window; eviction single-flight and amortised when
  little is evictable, the last-seen stamp written before the log and removed only if unchanged; the
  seam gate reads whole `use` statements (planted-file test `scripts/test-check-sim-seams.sh`); a
  frame header reserves at most 64 KiB; an outbound stall counter.
- **Measured:** one ~10 MB anti-entropy chunk applied into a WAL that fsyncs each append took 329 s
  for 70 000 × 64 B entries on a developer Mac (41.8 s for 9 000 × 1 KiB; 0.78 s for 152 × 64 KiB). The
  writer's bound must outlast the *receiver's apply*, which the reader's bound never sees — that is
  why the two differ.
- Lesson: a per-connection rate floor is a bandwidth requirement multiplied by the number of senders.
- Pages: `dev/architecture/runtime-invariants.md` (transport bounds); outside, `configuration.md`,
  `metrics.md`, `tuning.md`, `threat-model.md` §4, the replay inventory row.
