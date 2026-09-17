## [2026-09-17] ingest | item 6 PR 3 (first half) — the static forbidden-call check

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §6 · plan §4, D12 ·
code `scripts/check-sim-seams.sh`, baseline `scripts/sim-seams-baseline.txt`.

**Why this before the adapters.** D12 moved this check from PR 7 to PR 3 for a stated reason: *without
it the seams erode while the harness is built*. A kernel that routes today's nondeterminism is worth
nothing if next month's code walks around it — and nobody notices until a replay stops reproducing,
long after the change that caused it.

**Neither mechanism the record suggested was available.** §6 offered a `clippy.toml`
`disallowed-methods` list "scoped to those modules", or a `#[cfg]`-gated `deny` lint. Clippy's lint is
real but **workspace-global** — it cannot be scoped, so it would fire inside the seam implementations
and every test, which is precisely where these calls belong. Rust has no lint for "do not call this
function". So the *rule* the record states — **a new site is admitted only by editing this inventory**
— is enforced by diffing per-file counts against a checked-in baseline: an increase fails, a decrease
reports as progress. The record has been updated to say so; a plan that names a mechanism which turns
out not to exist should be corrected where it was written, not silently worked around.

**The measured debt: 190 sites across 45 files.** That is what PR 3's adapters and PR 4–7 draw down,
and it is now a number rather than an estimate.

**The bug worth writing down.** The first version excluded *everything after the first `#[cfg(test)]`*
— on the reasoning that test modules sit at the end of a file, which is this codebase's convention. A
probe that appended a live `Instant::now()` **below** the test module passed the check. Code after a
test module is ordinary production code and Rust is perfectly happy to put it there. Fixing it to skip
each `#[cfg(test)]` *item* rather than the file's tail raised the count from 165 to 190: **twenty-five
sites a plausible-looking check had been silently ignoring.**

It was found by planting a site and checking that the checker failed. That is the only way these are
found, and it is worth making a habit: *a checker nobody has watched fail is a checker nobody knows
works.* The same reasoning applies to any gate added in a hurry — including the ones added earlier
this week.

**What it cannot see, stated in the script itself.** It matches qualified call sites and the `use`
imports that enable unqualified ones, so an unqualified call still needs a visible import. It does not
parse Rust: a call through an unknown re-export or a type alias is invisible. The honest mitigation is
that the baseline makes *movement* visible even where it cannot attribute it.

**Still to come in PR 3:** the storage and channel adapters themselves — routing `persistence.rs`
(~25 `tokio::fs` calls, the inventory's "right first target"), `connection.rs` (~9 `Instant::now`) and
`tasks.rs` (~6 `fastrand` + ~6 timers) through the seams. The check is what keeps that work from being
undone while it is done.

**Gates.** `make check` clean, with the new check inside it; the checker verified against planted
sites both above and below a test module.
