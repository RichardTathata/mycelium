# 2026-09-19 — hysteresis is load-bearing; the earlier finding was schedule coverage

**The claim under review.** Scenario C recorded, on 2026-09-18, that *"hysteresis is not load-bearing while the
release spacing is on — removing it in shipped code did not fail the sweep, because a 1 s spacing alone bounds
the release rate."* That sentence had stood as an open question against a shipped mechanism, so it was measured.

**First measurement: removing hysteresis alone changes nothing.** Across all 48 schedules, boundary transitions
were identical — 40 with, 40 without — zero schedules differing, zero violations either way. Stronger than the
record said: not merely "not load-bearing for these objectives" but **completely inert**.

**Second measurement: the band is visited, often.** 124 of 282 opaque ticks had the fill inside the hysteresis
band, so the condition where the two rules disagree *is* reached. The proposals must therefore differ while the
outcomes do not — meaning the difference is swallowed downstream.

**What swallows it.** The per-tick fill step reaches **0.575**, nearly **three times** the 0.200 band. The fill
steps clean over the band, and the 1 s release spacing holds the release long enough that by the time one is
permitted, the fill has left the band. Add to that: every schedule ends by dropping inbound to 100‰ before the
quiet tail, so a *persistent* hover never coexists with the window where *at rest* is judged. Both facts are
about the schedules, not the mechanism.

**Third measurement: hold the hover, and the difference is stark.** A three-member group drains faster than any
share it can receive, so the band only exists once churn leaves one node carrying the load. With a lone member
and the load held to the end of the run:

| inbound | transitions, hysteresis on | off | band ticks |
|---|---|---|---|
| 350‰ | 11 | 15 | 67 |
| 400‰ | **1** | **12** | 65 |

**Conclusion: hysteresis is load-bearing.** The earlier finding was an artefact of schedule coverage.

**And the bound, which the measurement also gave.** Hysteresis *damps* a hover; it does not *settle* one. At
350‰ it takes 15 transitions to 11 — a real reduction, not rest. Whether the damping reaches rest depends on
where the load sits relative to what the node drains, which is a property of the workload rather than of the
breaker. The first draft of the gate asserted rest, failed at 350‰, and that failure is where the bound came
from. The test now claims the comparative fact and states the limit in its own doc comment.

**What shipped.** `Breakers::without_hysteresis()` — a **per-breaker** plant, where `Breakers::off()` removes
everything at once and so can never name which breaker did the work. `hover_schedules()`, deliberately separate
from `schedules()` because that sweep's contract is *the disturbance ends, then the loops settle* and these
schedules never end their disturbance. Three tests: the comparative claim, the plant, and one asserting the
**release spacing does not fire** on these schedules — because if it ever did, the two breakers would overlap
and this reasoning would need revisiting.

**The general lesson.** A negative result from a sweep is a statement about the sweep until the condition is
shown to be reachable. The first thing to measure after "removing X changed nothing" is whether the sweep ever
reaches the state where X applies — here it did, which is what made the third measurement worth doing.
