## [2026-09-15] ingest | scenario 13's client deadlines could not observe the answers they asserted on

Up: [dev](../dev.md) · page [testing](../testing/testing.md) §*A client deadline below the server's
budget is a defect, not a flake* · code `tests/integration/scenarios/13_tuple_space.sh` ·
callee budgets in `mycelium-tuple-space/src/lib.rs`.

**The signal.** The 4-node Docker suite went red on `main` at the v2.5.0 tag. It had been green for at least 97
consecutive runs — back to 2026-07-16, the limit of the run listing — and then failed twice in three — on the #217 merge and on the release commit —
with the scheduled run on the *identical* commit in between passing. Two different symptoms, both in
scenario 13: a put dying as `jq -r '.id' exited 28`, and `take #9 ... HTTP 000`.

**Ruling out the regression first.** The obvious read was that PR 3 broke something. It did not:
`git diff -U0 6420b01 00bffe0` over the core files is `@@ -149,0 +150,156 @@` and `@@ -386,0 +387,72 @@`
— pure insertion, zero lines removed from any executing path, and the tuple space's own sources were
untouched but for a version bump. A merge landing next to a failure is not the failure's cause, and the
diff is cheap enough that there was no excuse for assuming otherwise.

**The actual cause, which is arithmetic.** Every tuple operation carries its own server-side budget:
`resolve_primary_blocking` waits up to 3 × `cap_refresh` (2 s in the demo, so 6 s), then one `rpc_call`
at a flat 10 s — or, for a take, at `timeout_secs + 5`. Worst case, 16 s. The scenario allowed the
client **5 s** for a put and **exactly 10 s** for a take. Every single-shot assertion in the file was
set below the budget of the call it made. It passed for weeks only because a first attempt usually
succeeds; once scenarios 03/04/05 began leaving an outbound writer in reconnect backoff — where a
dropped request frame makes the caller wait out the full RPC deadline — the collision surfaced.

**Why raising these is not the forbidden fix.** [testing](../testing/testing.md) bans curing a flake by
widening a timeout, and that ban is right when the deadline is a *tolerance for slowness*. It does not
apply when the deadline is the *window in which the verdict may arrive at all*: set below the callee's
budget, the assertion is unobservable by construction and the test can only ever report that it gave up
first. The page now carries that distinction as a rule, with the instruction that precedes any deadline
change — write down the callee's budget and compare.

**Two diagnostics that made it worse.** The put loop piped `curl -sf` into `jq`, so a lost request
surfaced as `jq exited 28` with no iteration, status or body; the take loop beside it had been given
code-and-body capture in #150 and the put loop never was. And neither truncated the `-o` file, which
curl leaves untouched when nothing arrives — so the report printed take #9's failure carrying take #8's
body, reading as a wrong-id bug rather than the timeout it was. Both fixed.

**The rhyme worth keeping.** Each of these is a record claiming more than the underlying event
established: curl's impatience reported as the server's answer, one iteration's body reported as
another's. That is the same defect class the contracts axis exists to close, found this time in the
test harness rather than the product.

**Left open, deliberately.** `/api/tuple` aggregates gossiped `sys/tuple/{node}/{ns}/role` records that
carry no freshness stamp and are never cleared on shutdown — while the backpressure pheromone written by
the *same* loop stamps `written_at_ms` and evaporates after 3× the cadence. So the endpoint can name a
departed node as primary indefinitely. It was **not** the cause here (that phase passed in both failing
runs) and it is not fixed here, because it changes an operator-facing response shape hours after a tag.
Recorded in the CHANGELOG under *Known issues* and queued against the axis: it is
`ViewConfidence::staleness_known`, which 2.5.0 shipped for exactly this shape, applied one endpoint over.
