## [2026-10-10] ingest | #602's adversarial review, folded into row B

- Seven findings fixed: the bounds time **progress, not frames** (`mycelium-core/src/stall.rs`
  `StallGuard`, `peer_stall_timeout_ms` + `peer_min_rate_bytes_per_sec` replace `peer_write_timeout_ms`;
  the handshake bound covers a frame's first byte only); `sys/quorum/` evidence bounded with the
  log; LRU eviction of kinds and senders, exempting subscribed and queried kinds; fill over
  work-bearing subscribers, SSE routes as taps; the loaded tripwire test's doc made true and shedding
  tested directly; `check-sim-seams.sh` sees a grouped `time as X` alias (it had hidden `writer.rs`);
  the anti-entropy retry stated per failure detector.
- Lesson: a bound on a whole frame is a bandwidth requirement in disguise — 10 MB in 10 s is 8 Mbit/s.
  Bound progress and state the floor.
- Pages touched: `dev/architecture/runtime-invariants.md` (transport bounds), `dev/concurrency/lock-order.md`
  row 18; outside, `docs/reference/configuration.md`, `docs/operations/{metrics,tuning}.md`,
  `docs/threat-model.md` §4, the replay inventory row (corrected).
