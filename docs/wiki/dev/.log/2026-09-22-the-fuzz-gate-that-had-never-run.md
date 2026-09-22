## [2026-09-22] ingest | the fuzz gate that had never run — four defects, and three ways a gate can be hollow

Up: [dev](../dev.md) · [testing](testing/testing.md) §Trust-edge fuzz gate · release `v2.11.1`
(`CHANGELOG.md`) · PRs #347, #349, #350, #351.

**The shape of it.** §12.6 shipped eight trust-edge fuzz targets in v2.10.0 and called the gate done.
For the two days that followed they found nothing — and the reason was not that the parsers were
sound. **Four defects were waiting.** The gate could not reach them, for three independent reasons,
each of which looked like success from the outside.

### 1. The job stopped at the first crash, and nobody looked

The CI fuzz job runs its twelve targets **sequentially**. `presented_call` is fifth, and it began
failing *in the very commit that added the trust-edge targets*. Targets six through twelve therefore
never executed at all. `main` was red for **22 consecutive runs** — through an entire slice of AE
work, and through the v2.11.0 tag.

The job is `main`-only (it is slow and time-boxed), so every PR in that window was green, and
`make check-full` was green, and neither said anything about it. Each fix merely let the queue
advance to the next defect behind it: four, one at a time, over four rounds.

`RELEASING.md` now has **step 2b — check CI on the branch you are releasing *from***. And the
general rule: **when a sequential gate is red, every later stage is unrun**, not passing.

### 2. Most targets never reached their own invariant

Measured rather than assumed, which is the only reason it was found: a 20,000-input **noise** pass
reached the invariant of **0 of the 7** assertion-bearing targets. None. Random bytes essentially
never form a parseable credential, policy, bundle or reply, so those targets were proving *"does not
panic on garbage"* — worth having — and **nothing they claim**.

`presented_call` had asserted round-trip stability since the day it was written and had never once
executed that assertion. `trust_bundle` and `catalog_reply` had no valid seed at all.

This lesson was **already written on the testing page** before any of this — the `caller_envelope`
seed that was merely frame-shaped, the `serde_fixint` sweep that decoded nothing. Writing it down did
not apply it. So the check is now structural: a **reachability registry** names every
invariant-bearing target beside the seed that reaches it, and fails by name when one stops arriving.
It claims completeness the way the lock-order table does — adding a target means adding a row.

### 3. The tests that did exist sampled instead of searching

Two fidelity tests for the bundle codec existed and passed throughout. They used ordinary strings,
and ordinary strings round-trip fine. An exhaustive sweep over the characters that actually break
flat text formats found **7,092 of 17,556 pairs corrupted** — and **1,729 more** behind the first
fix. Both defects lived in characters nobody writes on purpose.

### The four defects, and the one principle under three of them

| Where | Defect |
|---|---|
| `/a2a` credential | `is_ascii()` checked the **encoding**: `"ࠉ"` is an ASCII header carrying a non-ASCII principal. It passed, and `to_header_value` re-emitted it unescaped |
| replay trace | `str::lines()` strips `\r` only *before* `\n`, so a line ending in a bare carriage return lost it |
| bundle object | the value was trimmed **inside** its quotes — the separator had already eaten the opening quote |
| bundle object | `split_once("\": \"")` found its separator inside an **escaped key**; a key of `"` came back as `\` |

Three of the four are one principle: **what a parser accepts, its writer must be able to emit.** A
value this node took in and could not put back out is a value a peer will refuse after we allowed it.
That asymmetry is the bug, and it is invisible to a `parse → write → parse` round trip — which is why
the fidelity tests start from the **value**.

The fourth is more general and could not be patched: *a literal-substring split cannot tell a real
delimiter from one inside a quoted string, because the information it needs is not in the substring.*
It takes a scanner that knows where a string ends. That also retired `unquote`'s "at most one quote
from each end" rule, which only existed to paper over the same split.

### What this costs to believe

An identity field that accepts arbitrary Unicode admits **confusables** — a `principal` that renders
like another one — which is what an ASCII rule on an identity is for. A replay trace is what a
recording replays from, so **what it decodes to *is* the run**: a silently shortened field replays a
different run while looking like the same one. And the bundle carries `Build.commit`, the field a
replay uses to decide it is replaying against the same binary.

### The sentence to keep

**A gate that exists is not a gate that runs, and a gate that runs is not a gate that checks.** All
three failures here were invisible in exactly the same way: the test passed, the count went up, and
the thing it was named for never happened.
