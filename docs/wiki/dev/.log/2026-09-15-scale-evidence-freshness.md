## [2026-09-15] ingest | a job nobody runs never goes red — the scale nightly's silent gap

Up: [dev](../dev.md) · page [scale-tests](../testing/scale-tests.md) §*The nightly is only evidence
when it runs* · code `.github/workflows/scale-nightly.yml`.

**What was wrong.** The scale suites run on a self-hosted box labelled `mycelium-scale`, because a
hosted runner cannot reach 100 nodes. When that box is offline the nightly does not fail — it queues,
indefinitely, and expires as *cancelled* days later. Every nightly from 2026-09-10 to 2026-09-15
produced nothing: four expired, two were still queued days later. A release and five merges went past
in that window and nobody noticed, because **nothing ever went red**.

**Why it matters more than it looks.** The plan's own line is "nothing scale-related can be evidenced
until it is green" (§10, item 8). An unevidenced scale claim is exactly the defect class this axis
exists to close, one level up: the documentation asserts numbers, the numbers rest on a nightly, and
the nightly had not run in six days. The spinner was doing the work the green tick was supposed to do.

**What shipped.** A hosted `evidence-freshness` job that asks a narrower question — *when did this
workflow last actually succeed?* — and fails when the answer is older than three days or **never**,
naming the likely cause and where to look. It cannot start the box and does not pretend to. It makes
the silence loud.

**The honest limit.** Queued-rather-than-failed is *consistent with* no labelled runner being online;
the runner list needs admin scope to read, so the cause is inference and the job's message says so
rather than asserting it. Bringing the box up is the user's, not the repo's.

**The shape, again.** Today's other four findings were checks that proved less than the thing they
stood for. This is the degenerate case: a check that proves nothing at all, while looking like it is
working. Alarms should fire on the absence of a signal, not only on a bad one — absence is what this
was failing at.
