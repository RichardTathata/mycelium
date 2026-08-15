# ingest — wiki change sinks: git as projection, not substrate (2026-08-15)

**Shipped:** `ChangeSink`/`AppliedRound` on the `CuratorBrain` (control-plane) + the `GitMirror` sink
(feature `git-mirror`, zero added deps — `git` CLI). Design record: `docs/design/wiki-git-store.md`.
Companion page updated (`companions/wiki.md` § Change sinks + Gates); CI Wiki job gained the
`git-mirror` test+clippy steps.

**How it happened.** Assessing the FTT council-decisions wiki (one of the two use cases behind the
2026-07-03 pivot) surfaced the gap: `mycelium-wiki` had no git-auditability story, and the obvious
`GitStore: WikiStore` adapter was sketched — then a principles review found it fails on seven counts
(a branch ref = a global sequencer in the write path; git history forfeits erasability and cannot
compose with crypto-shred without losing legible diffs; the CAS token pollutes the canon; an
unpoliced egress path; a store-downcast layering leak; readers stop being dumb-store reads; lock
across I/O + a heavy dep). The resolution inverts it: **the store stays the airtight per-object-CAS
system of record; git is a best-effort curator-side projection** — the same shape as WS2's
`AuditSink`. One commit per applied round, pure rendered documents, `EgressPolicy`-gated push with a
post-push divergence tripwire, `try_lock`-and-skip so the sink never blocks the drain, `rebuild()`
as the heal-everything + erasure path. GitStore-as-truth is retained behind an eligibility envelope
(E1 public-record · E2 single-curator · E3 force-push-locked remote + tripwire · E4 reader tooling
accepted) — not built.

**Durable lesson:** when an external system (git, a DB, a queue) looks like a natural `WikiStore`/
substrate backend, first ask whether it belongs in the **write path** or as a **projection the
curator emits** — the projection usually preserves coordinator-freedom, erasability, and egress
policy at the cost of the external system no longer being "the truth," and that trade should be a
recorded product decision, not an implementation default.
