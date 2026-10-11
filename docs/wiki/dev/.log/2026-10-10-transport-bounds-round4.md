## [2026-10-10] ingest | #602's review round 4, folded into row B

- Preemption redesigned: a newcomer at the cap proves itself first (TLS + a valid first frame,
  `connection::read_first_frame`, 16 waiting at most), and a victim must be quiet longer than
  `writer_idle_timeout_secs + handshake_timeout_ms`, not mid-frame, one per `handshake_timeout_ms`;
  off with `writer_idle_timeout_secs = 0`. Opt-in `max_connections_per_source`. The residual (an
  attacker talking more often than the threshold keeps the slots it got free) is in the threat model.
- Replica sync answers no `Persisted` while a chunk is applied but unbatched (`ae_unbatched`);
  batched records count against the WAL channel's depth.
- Evidence sender sets slide and are pruned with their kind; the evictor and recorder act under the
  kind's log lock.
- **Measured:** WAL snapshot 0.47 s / 2.0 s / 7.8 s for 64 MiB / 256 MiB / 1 GiB (debug build); the
  write stall bound is derived from `sync_mode` — 300 s under `flush`, 60 s otherwise.
- Lesson: a preemption threshold below the honest idle period is a displacement attack.
- Pages: `dev/architecture/runtime-invariants.md`; `docs/guide/deprecations.md` §30 (renumbered);
  `configuration.md`, `metrics.md`, `tuning.md`, `threat-model.md` §4.
