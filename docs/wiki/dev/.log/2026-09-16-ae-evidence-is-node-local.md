## [2026-09-16] ingest | the correction: AE evidence is node-local, and only a reference gossips

Up: [dev](../dev.md) §AE · record `docs/design/action-envelope-ae0.md` §5 · corrects
[`.log/2026-09-16-ae-gateway-records-what-it-enforces.md`](2026-09-16-ae-gateway-records-what-it-enforces.md) ·
code `src/agent/evidence_journal.rs`, `src/agent/action_evaluator.rs`, `src/agent/http.rs`.

**What happened, plainly.** Earlier the same day PR #224 fixed a real gap — the gateway enforced and
recorded nothing — by sealing the whole decision document into the tamper-evident audit chain. The
audit chain is an ordinary signed KV entry. It **gossips to every node**. So the fix disseminated, to
the whole mesh, the exact resource every call targeted, the reason each policy gave, and the
constraints it checked.

AE0 §5 forbids exactly that, in those words, and says why — it adopted the rule *after reviewing its
own first draft*, which had proposed the same thing:

> `seal_and_write` stores the complete signed record in gossip KV before mirroring it to the sink, so
> "seal it into the chain, export through the sink" would have disseminated sensitive action evidence
> to every node.

**How it was missed.** §11's "still to come" list mentions the journal, and I read that as *a later
enhancement to what I had built* rather than *the shape it was supposed to have*. I never opened §5.
The lesson is narrow and reusable: when an ADR lists something as outstanding, check whether it is an
addition or a **correction of the thing you just wrote** — the two read identically in a bullet list.
Nothing in the tests could have caught it; they asserted the evidence was recorded, and it was.

**No exposure.** The path is inert unless `with_action_evaluator` is called, and the only caller is a
private companion's tests on single-node clusters. Wrong on `main`, never wrong in the field.

**The correction.** Evidence goes to a node-local `EvidenceJournal` — append-only, fsynced, never
gossiped — returning item 1's `LocalDurability` (`OnDisk` only after the sync; `Buffered` is never
produced, because evidence sitting in a page cache cannot gate an effect). The chain carries an
`AeReference`: kind, identities, principal, verdict, policy revision, catalogue id, and the journal
record's **content hash**. The hash is what keeps the chain worth having — its ordering and
hash-linking still cover the evidence, so a journal record that does not match is detectable, without
the evidence travelling. `AeReference` has no field for a resource or a reason, so the mistake now has
to be made on purpose.

**Why not the sink.** `AuditSink::export` returns nothing, runs on a drain task and drops on
saturation. It is a mirror, and a mirror cannot be a durability barrier — which is why AE0 names the
journal as the ack-capable contract, and why an effect may be gated on one and never the other.

**Failure is a decision, not a log line.** Three behaviours, one test each: a full queue is **refused,
never silently dropped** (a dropped evidence record and a decision nobody made are indistinguishable
afterwards); a persistence failure is refused; a lost acknowledgement is `DeliveryUnknown`, **never**
`Failed` — the record may well be on disk, and "failed" claims knowledge nobody has.
`EvidenceProfile::Strict` gates the dispatch on all three; `Lenient` proceeds and the reference says
`evidence: unknown` / `not_established`, so the weaker profile is legible as weaker instead of looking
identical to the stronger one.

**The pin that matters.** `evidence_goes_to_the_journal_and_only_a_reference_gossips` renders every
audit record's `target` and `detail` as raw text and asserts the exact resource strings and the
policy's reason phrases do **not** appear. A field-by-field assertion would have passed against a type
that quietly regained a `resource` field; a substring sweep over what actually gossips will not.

**Gates.** `make check` clean; 513 (`compliance,a2a`) + 453 (`tls,metrics,a2a,llm`) + 343
(`--no-default-features --features gateway`) + 179 (`mycelium-core`).

**Still owed:** the journal's cursor-based exporter (the RA outbox shape, §6.7) — the journal is
written and read, and nothing ships from it yet; and four of §5's five records (`requested`,
`blocked`, the execution records, `outcome observed`) — the enforcement point writes `decided`.
