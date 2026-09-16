## [2026-09-16] ingest | the alarm was in the queue it was watching

Up: [dev](../dev.md) · page [scale-tests](../testing/scale-tests.md) §*The nightly is only evidence
when it runs* · code `.github/workflows/scale-nightly.yml`.

**What was wrong.** Yesterday's `evidence-freshness` job — the hosted alarm that goes red when the
scale nightly has not succeeded in three days — could not fire. `scale-nightly.yml` carried a
**workflow-level** `concurrency` group, and a workflow-level group queues the *whole run*, jobs
included. One run was already sitting queued against the offline self-hosted box, so every later run
went `pending` with **zero jobs created** — the hosted alarm among them.

**The evidence.** Run `35086785839` (2026-09-16 10:46Z) was `pending` with an empty jobs list, behind
run `34960720200` (2026-09-15 10:57Z), still `queued` for the `mycelium-scale` runner a day later.
The alarm shipped at 19:53Z on the 15th and, at the first nightly after it, ran nothing.

**What shipped.** The `concurrency` group moved from the workflow to the `scale` job. The guarantee
that was actually wanted — two scale suites never share the one Docker daemon — is unchanged; the
hosted job in a later run is no longer held behind a self-hosted job in an earlier one. As a
by-product the pile-up is bounded: at job level a newly pending scale job displaces the previously
pending one, instead of a queue of runs expiring one by one.

**The lesson, which is the same lesson one level up.** *A check must not sit inside the failure
domain of the thing it checks.* The alarm was placed in the workflow it watches and inherited that
workflow's blocking resource, so the one condition it existed to report — nothing is running here —
was the one condition under which it could not run. Yesterday's entry said an alarm should fire on
the absence of a signal; this one adds that it must be reachable when the signal is absent.

**What the alarm said once it could run.** Dispatched on the fix branch, the hosted job started and
failed with *"No successful run of the scale suites has ever been recorded."* Checked against the
API: **all 69 runs since 2026-07-10 concluded `cancelled`**, and sampled jobs have an empty
`runner_name` — expired waiting for a runner that never claimed them. The six-day window in
yesterday's entry was not the outage, only the noticed part of it. Every scale number in the docs
rests on operator-run `make test-scale`, never on CI.

**Honest limit, unchanged.** Nothing here brings the box up, and queued-rather-than-failed is still
*consistent with* no labelled runner being online rather than proof of it. What changed is that the
repo now says so out loud every night instead of spinning.
