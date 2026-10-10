## [2026-10-10] ingest | #602's review round 3, folded into row B

- Preemption at `max_connections`: a newcomer closes the inbound connection whose last complete frame
  is oldest and at least `handshake_timeout_ms` old (`TransportBounds::preempt_oldest`) — what reaches a
  trickle inside a frame and a tiny frame every idle period; a 1 KiB/s read floor is the second layer.
- Anti-entropy group commit: `WalHandle::append_batch`, one fsync per chunk after the whole chunk is
  applied. Measured worst chunk apply (debug build, developer Mac, `flush`): 329 s → 3.4 s
  (~145 000 tombstones); `peer_write_stall_timeout_ms` back to 60 s on that number.
- Evidence bounded per kind (1024 senders) and per second (4096 writes), skipping past either; pins last
  at least the caller's window; the evictor/recorder orphan race closed on the recorder side; a drop
  guard on the evictor flag; scan credit lapses with the earliest pin; the read buffer shrinks after a
  large frame.
- Pages: `dev/architecture/runtime-invariants.md` (transport bounds; §Persistence group commit);
  `docs/guide/deprecations.md` §27; `configuration.md`, `metrics.md`, `tuning.md`, `threat-model.md` §4,
  the replay inventory.
