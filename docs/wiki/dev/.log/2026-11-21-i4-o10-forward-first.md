## [2026-11-21] ingest | A gateway's cancel is forwarded first; only the acknowledgement waits for the disk (I4, §9 O10)

The maintainer's decision of 2026-11-20 on #816's finding, built. `call_issuer::revoke` is split into
`begin_revoke` (no await: the revoking checkpoint signed, held as issued, offered to this node's own authority and
swept) and `finish_revoke` (the required-sync write). `call_cancel::cancel` runs the forward and the write together
(`tokio::join!`) and answers once both are done; `cancel_at_door` spawns the forward first and awaits the write;
`cancelled_before_sent` (nothing was sent) still takes both steps at once. **A reversal:** a revocation the disk
refuses has still been forwarded — `CancelRefusal::NotDurable` now means *perhaps stopped, not on disk*.

Durable knowledge from the build:
- **The crash bound is real and pinned**: a required-sync write applies nothing before its sync, so a forwarded
  revocation whose gateway crashes before the sync is in no KV after the restart; the provider holds it in memory
  only (D-4), and a provider restart re-admits the call inside its window. The caller of the crashed gateway gets no
  answer at all.
- **A second WAL wait on the cancel path, found here**: under `sync_mode = "flush"` the gateway's connection reader
  awaits the WAL's ack for each inbound record (`apply_inbound_then_append` → `append_inbound`), so the provider's
  answer to the forward can sit behind the same burst and the receipt read `DeliveryUnknown` though the call stopped
  in 160 ms. Honest (never a negative), and §9 O11's family, not fixed here.
- **At `/a2a` the stop's own answer could complete the task** before the cancel removed it (the forward now beats the
  disk), turning the cancel's answer into "task already completed". `TaskReservation::complete` leaves a task whose
  cancel has begun (`CallSink::cancel_asked`) for the cancel.

Pages: `docs/features/mandates.md`, `docs/operations/what-is-proven.md` (`call-cancel-behind-collector`,
`call-soak`), `docs/guide/deprecations.md` §124, the plan's §9 O10 and I4 row.
