# [2026-09-19] ingest | §12.3–§12.6 — the delivery surfaces, and the drift they exposed

Up: [dev](../dev.md) · ledger [history](../history.md) · plan `docs/plans/v3-contracts-axis.md` §12 ·
PRs #313–#321 (the §12.2 half is [its own entry](2026-09-19-section-12-2-guide-chapters.md)).

**The shape of this stretch.** §12.2 was writing what did not exist. §12.3–§12.6 turned out to be
mostly *correcting what did* — the axis shipped in v2.8.0 and v2.9.0 and the surfaces around it never
moved. Ten pieces of drift, every one found by reading prose against the thing it describes rather
than by reading prose.

## What shipped

| Surface | PRs |
|---|---|
| §12.3 operations: the federation runbook, rows across eight runbooks, the matrix | #313, #314, #315 |
| §12.4 both decks | #318 |
| §12.5 philosophy | #317 (it was **already done** — see below) |
| §12.6 front door, companion onboarding checklist | #317, #319, #320, #321 |
| §12.2 tail: chapter 17's restructure | #316 |
| **Code**: three contracts-axis metrics | #314 |

## The one code change, and why it was not "because the plan said so"

§12.3 told the runbook to document **receipt counters, control envelopes and per-verdict decision
counters**. Going to write those rows, I went looking for the metric names and **they did not exist**.

Rev 1.12's own log says of those additions: *"Delivery surfaces only; no engineering change."* So the
plan did **not** oblige building them, and the failure mode to avoid was documenting metrics that do
not exist — which checking prevented.

But a real gap sat underneath, independent of the plan. The evidence journal records permits *and*
refusals, and the code says why: *"an evidence stream that omits its permits cannot support any
statement about what an agent was allowed to do."* The **metrics plane counted refusals only**, so a
dashboard had a numerator and no denominator: ten denials could be ten out of ten requests or ten out
of ten million, and an alert on denial volume fires on traffic growth.

Three counters were built on that reasoning, not on the plan's say-so:

- `mycelium_ae_decisions_total{verdict, mapping}` — **the denominator**. Labels are the evidence
  document's own vocabulary, asserted equal to its serialised form, so a dashboard and a record cannot
  drift apart. A useful consequence falls out: **`permit` with `mapping="unmapped"` should always be
  zero**, because an unmapped operation cannot be permitted — a non-zero series is a defect, not a
  policy question.
- `mycelium_control_decisions_total{decision, class}` — `would-hold` counted **as itself**, never
  folded into `proceed`: under `observe` it is the only signal the rung produces.
- `mycelium_kv_receipts_total{local_durability}` — the durability *distribution*, invisible in
  aggregate before this because a receipt reports per response.

**Deliberately not built:** evidence freshness and export lag. They measure how far an exporter has
fallen behind, and there is no exporter in the public tree to be behind — the gauge would measure
nothing. It lands with the exporter.

Both label tests were broken in the opposite direction: folding `would-hold` into `proceed` fails the
control test, and renaming `permit` fails the wire-form test.

## The ten drift findings

1. `00-concepts.md` gave receipt rung 1 as two variants; the code has **four**.
2. The same paragraph **omitted `Buffered`** from rung 2 — the variant whose whole point is being a
   *different* claim from `OnDisk`, not a weaker one.
3. It said the receipt types *"land with item 1 PRs 2–4"*. They landed in v2.5.0.
4. The guide index called federation *"contract only, no transport yet"*. Shipped in v2.8.0.
5. `CLAUDE.md` listed the federation transport and scheduler seam as open, and said the 2.8.0 cut
   awaited the operator's word.
6. `CLAUDE.md` had the ack-semantics invariant but not §12.2's **seam rule**.
7. **Mine.** Chapter 18's first draft said the bool from `kv().set` means *"applied here, now"*. The
   code says **queued for gossip**, and its `false` is ambiguous.
8. `deployment.md` said a per-write durability receipt *"is the v3.0 contracts plan's first item"*.
   It shipped in v2.5.0.
9. Chapter 17 **contradicted itself**: one paragraph said PR 10b closed the gate's last caveat, the
   next said *"The release gate is not met."* The gate was met in v2.8.0.
10. The root README mentioned receipts **zero** times and federation **zero** times; `docs/plans/README.md`
    said the axis has six items and stopped at four mid-September PRs; the README sized the guide at
    17 chapters when it has 25; `philosophy.md` told the reader to answer *"these three questions"*
    above a list of five.

## §12.5 was already done

Worth recording so nobody redoes it. Property 8 (*the contract — an ack names what it proves*) exists,
litmus tests 4 and 5 carry posture rules 3 and 6, and Property 7's epistemic symmetry is already
extended from state to evidence inside Property 8. Only the question count was stale.

## The companion onboarding checklist, and using it

[`onboarding-checklist.md`](../companions/onboarding-checklist.md) — seven rows, three conditional,
and the mechanism is that **the condition must be written down**: *"not applicable"* is a valid
answer, a blank cell is not. Applying it found four gaps, and all four were closed rather than filed:

- **`mycelium-effects` had no runnable demonstration anywhere.** The sharpest, because row 6 is the
  row with the track record — of six written for the v2.9.0 gallery, two found real defects.
  `destination_commit.rs` closes it, and it is a gate: planting the dedup row so it survives a failed
  business change fails the example with *"a rolled-back attempt left no dedup row"*.
- Six companions had no operator row (#320); three had no maintainer page (#321).
- **`mycelium-sim`'s missing operator row closed by being stated** as deliberate — it has no
  production surface, because the seam is off in every shipped build.

## Two process lessons

**Verify before writing, not after.** Chapter 18 was drafted from memory and checked afterwards,
producing four errors, one of which survived into a commit. From chapter 19 onward every name was
checked first, which then caught two errors in my own research notes (`Stale` not `TooStale`;
`Divergence` is a struct, not an enum).

**Batch docs PRs; do not stack them.** Chapters 21–24 opened as four stacked PRs — five full CI runs
for Markdown-only changes, four competing for runners, one check pending over half an hour. Collapsed
into one.

## The honest gap, unchanged

**Nothing in CI enforces doc-vs-code accuracy.** `/doc-coverage`, `/wiki-lint` and `/publication-lint`
are operator-run skills. Ten findings after two releases is what that costs, and the publication lint's
own header records that the last overclaim was caught by *a human, not a mechanism*.

## Still owed by §12

§12.4's two positioning sentences and regenerated PDFs; §12.6's migration notes per deprecation, fuzz
targets for the new trust-edge parsers, and the **Phase-C adversarial self-audit** over items 1 + 2 + 7.
From §12.2: the SDK narrative side and a wiki page per new *mechanism* (the companions now have theirs).
