## [2026-10-11] ingest | #602's final review: row B's documentation made exact

- The read floor is described as on at 1 KiB/s everywhere; the attacker-cost sentence corrected.
- The write-side residual stated (no write rate floor: a one-byte-per-window reader parks its writer and
  reply indefinitely) with the memory bound (`max_peers` × the divergent dump).
- Evidence: a bounded **rate** (~1024 keys per kind per signal window), not a bounded total.
- `max_connections_per_source` refusals counted (`SystemStats::inbound_per_source_refused`); tests for
  the `SourceGuard` give-back, `SignalLog::trim`'s fix (a test hook), and the sender-slot test seen
  failing against a toggle of its mechanism.
