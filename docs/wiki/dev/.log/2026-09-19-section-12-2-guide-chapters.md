# [2026-09-19] ingest | §12.2 — the seven how-to chapters and the axis vocabulary

Up: [dev](../dev.md) · ledger [history](../history.md) · plan `docs/plans/v3-contracts-axis.md` §12.2 ·
PRs #304, #305, #306, #311 (#307–#310 closed, superseded).

**Why this was overdue rather than early.** §12.2 ties each how-to chapter to *its item's release gate*, and
the concept vocabulary to *its item's ADR*. Every one of those gates passed in v2.8.0 and v2.9.0. S2's
phase-exit condition is "no gap in the matrix at the phase exit", which makes this a **Phase C item**, not a
tidy-up after the fact.

## What shipped

Seven chapters, one per item, each ending by naming what it does **not** establish:

| Ch | Subject | The claim it leads with |
|---|---|---|
| 18 | contracts & receipts | a receipt names its rung and nothing above it |
| 19 | replay & simulation | a seed is not a durable reproduction artefact |
| 20 | authorising actions | a gateway check is self-imposed prevention *for the routes it fronts* |
| 21 | mandates | CAS asks *did anything change under me*; a mandate asks *am I still allowed* |
| 22 | stability & control | an isolated node is **uncertain, not fresh** |
| 23 | knowledge | judging is not recording |
| 24 | commitments | the award is a receipt-bearing operation, never a write alone |

Plus: eight concept-pair paragraphs and ten glossary rows in `00-concepts.md`; `ReceiptError` added to
`error-handling.md`'s taxonomy (**it was missing entirely**); six cookbook recipes; chapter 14's one-workload
comparison of tuple space / blackboard / contract net.

## The thesis, written down for the first time

Nearly every refusal type in the codebase encodes the same rule, and nothing said so:

> **Refusals are typed by what the caller should do next.** A refusal a retry loop would consume is a
> *different type* from one it would not.

`MandateSuperseded` ≠ `Conflict` (the retry loop would launder the revocation) · `BadSignature` ≠ `NotPermitted`
(forgery vs. policy gap — "sends the operator to edit the wrong file") · `Indeterminate` ≠ `Deny` ·
`InsufficientEvidence` ≠ `Rejected` · `LocalDurability::Failed` ≠ absent · `DeliveryUnknown` ≠ "nothing happened",
and there is deliberately **no variant** meaning that.

## Seven drift defects, six of them pre-existing

Found by checking prose against code rather than by reading prose:

1. `00-concepts.md` gave rung 1 as `Applied`/`Superseded`; the code has **four** variants — the store's "nothing
   changed" covers three different truths.
2. The same paragraph **omitted `Buffered`** from rung 2 — the one variant whose entire point is being a
   *different* claim from `OnDisk` rather than a weaker one.
3. It closed by saying the receipt types *"land with item 1 PRs 2–4"*. They landed in v2.5.0.
4. The guide index called federation *"contract only, no transport yet"*. The transport shipped in v2.8.0; the
   chapter body was already correct, only the index line was stale.
5. `CLAUDE.md`'s active work still listed the federation transport and the scheduler seam as open, and said the
   2.8.0 cut awaited the operator's word.
6. `CLAUDE.md`'s hot invariants had the ack-semantics rule but **not** the seam rule §12.2 asks for.
7. **Mine.** Chapter 18's first draft said the bool from `kv().set` means *"applied here, now"*. The code says it
   means **queued for gossip**, and its `false` is ambiguous — the local store may still have been updated with
   anti-entropy to follow, or the write may have been rejected outright. `00-concepts.md` had this right all
   along.

## The method correction that came out of it

Chapter 18 was written from memory and verified afterwards. That produced four errors: three wrong signatures
(both receipt verbs are `async` and take the operation identity **first**; `set_with_replica_sync` takes a
*timeout*, not an identity and a replica count) and a missed `retry_with_receipt` — which matters, because
retrying through the ordinary write ticks a fresh HLC stamp and re-ranks the same operation under LWW. Three were
caught before the commit; the `set` error was not.

**From chapter 19 onward every name was verified before writing.** That caught two errors in my own research
notes: the uncertainty variant is `Stale`, not `TooStale`; and `Divergence` is a **struct**, not an enum.

## A process correction mid-flight

Chapters 21–24 were first opened as four *stacked* PRs, one per chapter. That meant five full CI runs for
Markdown-only changes, with four competing for runners simultaneously — one check sat pending over half an hour.
Collapsed into a single PR (#311) with the same four commits cherry-picked onto `main` unchanged; #307–#310
closed pointing at it. **Batch docs changes; do not stack them.**

## What this does not establish

**Nothing in CI enforces any of it.** `/doc-coverage` and `/wiki-lint` are operator-run skills, not gates, so
nothing mechanically prevents this prose drifting again — and six pre-existing corrections after two releases is
what silent drift looks like. Every PR description says so rather than leaving it implied.

Still owed by §12.2: chapter 17 is **not** restructured into *public discovery* and *federated domains*; the
SDK narrative side is untouched; the wiki pages per new mechanism are unwritten. §12.3's operations runbook has
not been started.
