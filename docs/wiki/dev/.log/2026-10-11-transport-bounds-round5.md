## [2026-10-11] ingest | #602's round 5: row B simplified and shipped

- The maintainer's decision: **simplify and ship.** Removed what review rounds 3–4 added and each later
  round found fault in: connection preemption (probation, `read_first_frame`, the rate limit,
  `inbound_connections_preempted`) and WAL group commit (`AppendBatch`, `append_batch`,
  `QueuedRecords`, `ae_unbatched` and its replica-sync gating). Anti-entropy appends per record again.
- Kept: the handshake/idle/read-stall/write-stall bounds, the 1 KiB/s read floor, the subscriber-veto
  fix, the bounded signal log, the stopped-agent fix, incremental `read_frame` growth, the outbound
  stall counter, the seam-gate alias fix, the opt-in `max_connections_per_source` (its guard now holds
  its own counter; exact IPs only).
- Evidence sets no longer slide: past 1024 senders a write is skipped until a slot ages out of the
  quorum window. `SignalLog::trim` removes a log only under its lock and only if still the table's.
- The write stall default is one value, from a `flush` receiver applying the worst chunk entry by
  entry (measured; see `configuration.md`).
- **The residual, stated** (threat model, `configuration.md`, CHANGELOG, plan row B): an attacker that
  keeps talking can hold the connection slots it got from free permits; mTLS and the per-source cap
  mitigate.
- Lesson: mechanisms added under review to close a residual drew most of the later findings; stating
  the residual was the cheaper fix.
