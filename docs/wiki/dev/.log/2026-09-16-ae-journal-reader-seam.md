## [2026-09-16] ingest | the evidence journal's reader seam — §6.7's outbox shape

Up: [dev](../dev.md) §AE · record `docs/design/action-envelope-ae0.md` §5, §6.7 · code
`src/agent/evidence_journal.rs`.

**Why this, now.** The correction earlier today moved evidence out of the gossiped audit chain into
the node-local journal. That closed a confidentiality hole and opened a plumbing one: an exporter
built on `AuditSink` — which is what the private companion's T3 exporter is — now sees only
*references*. It would ship nothing and report every record as foreign. The evidence is durable and
unreachable, which is not obviously better than the problem it replaced.

**The seam.** `read_evidence_journal_from(path, cursor, max_records, max_bytes) -> JournalPage`.
Batch, ship, advance. The `EvidenceCursor` carries a **byte offset** as well as a sequence: a reader
that had to re-walk the file to find its place would get slower for exactly as long as the node kept
producing evidence, which is the wrong shape for something that runs forever.

**The correlation that makes the split work.** Each `JournalEntry` carries the record's content hash,
and it is the *same* hash the chain's `AeReference` cites. Pinned by a test, because if those ever
diverge the evidence and its chain record stop being about each other and nothing says so.

**Two bounds decisions worth keeping.** A record larger than the caller's `max_bytes` is returned
**alone** rather than skipped — silently dropping evidence for being inconveniently large is the
failure mode this whole slice exists to avoid, and a reader that skips is worse than one that stalls
because it looks like it is working. And `more` distinguishes *stopped at a bound* from *reached the
end*, so a caller never concludes it is caught up when it is not.

**The torn tail.** A page that meets a half-written record ends cleanly **before** it and leaves the
cursor there, so when the writer finishes the record a later read picks it up. The alternative —
skipping past it — would lose exactly one record per crash, permanently and invisibly.

**Gates.** `make check` clean; 519 (`compliance,a2a`) + 459 (`tls,metrics,a2a,llm`) + 343
(`--no-default-features --features gateway`) + 179 (`mycelium-core`). Twelve journal tests now.

**Still owed:** the exporter itself (signing, delivery, retries — the private companion's half, which
needs a tag before it can consume this), and four of §5's five records.
