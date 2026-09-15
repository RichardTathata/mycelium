## [2026-09-15] ingest | item 1 PR 3 — the required local sync, and the prepared write

Up: [dev](../dev.md) · record `docs/design/contracts-receipts.md` §2.1, §2.2, §9a · plan §3 (PR 3), D8 ·
code `mycelium-core/src/ops.rs` (`kv_set_requiring_sync`), `kv_handle.rs`, `receipt.rs`.

**What shipped.** A write that must be durable. `set_requiring_sync` forces an `fdatasync` in every
`SyncMode`, so its receipt is always `OnDisk` where the ordinary path would say `Buffered`, and the
ordering is reversed to **persist → apply → gossip**. That reversal is the point: every other write
site applies first, so a durability failure leaves a value readers, subscribers and peers may already
have seen, while here a failure means *this attempt* applied nothing. A node with no persistence is
refused rather than handed a receipt claiming nothing — prevention requested by the caller, one of the
three admissible shapes (posture rule 3(i)). Persist-first is admissible *only* because the snapshot
merges the WAL tail before truncating; the two regressions pinning that are cited at the call site.

**External review before merge (2026-09-15), two findings, both taken — one overturning my own
reasoning.** (1) *A failed sync can still produce a visible value after recovery:* `wal_append` writes
before it syncs, so a failed fsync leaves a complete record that replay restores. The documentation
said "the value never became visible here" — true of the call, false of the recovery. The error now
states the post-restart outcome is **unknown** and names what permanent non-application would cost.
Pinned by `regression_an_unsynced_record_still_replays`. (2) *The §9a deferral was argued wrongly.* I
had claimed a per-node operation registry was destination-side dedup in disguise; it is not —
remembering a *local* write's stamp claims nothing about an external business transaction, and a KV
write has no destination in it. The deferral also had a cost review reproduced: a caller that loses
the acknowledgement cannot use the safe retry path at all, and re-issuing by `OperationId` mints a
fresh stamp that clobbers a newer value — D11's hazard, reintroduced.

**What replaced it.** A **caller-held prepared operation**, not node-held status: `prepare_write`
stamps identity, key, content and HLC before dispatch and returns a small serialisable token; every
commit reuses that stamp. The state lives with the party that needs it across process boundaries, so
there is nothing to size, expire, or explain when a retry arrives after eviction. Bounded local status
stays available as a convenience; it is no longer load-bearing for safety.

**Two lessons, both process.** *First:* I deferred part of the plan on an argument I found persuasive
without running the failure it was meant to serve against it. A deferral is a design claim and needs
its scenario tested like any other. *Second:* this entry and the release note for PR 3 went missing
for a day. The script that wrote them died on an ambiguous anchor (`### Added` matched twice in the
CHANGELOG), and because the step was backgrounded together with the test gates, only the gate log was
read — the traceback sat unexamined in the task output. The code and the ADR landed; the record did
not. Found while preparing the 2.5.0 release, which is exactly what a release runbook is for. **Read
the output of every step that edits files, not only the step that proves the code works.**
