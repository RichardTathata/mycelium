# ingest — nightly outage closed: session-scoped TCC grant; schedule moved to 13:00 (2026-08-18)

The netprobe discriminator delivered: host gateway/DNS/registry green at 01:58/02:02/04:30 while
the 02:00 suites died between the first two probes — the WAN theory is refuted. The operator
found a fresh Local Network prompt on screen the same morning: the grant for bare CLI tools under
launchd is **session-scoped** — it held for the rest of the grant-day (all supervised runs green)
and was gone by the next night, so an unattended sleeping-hour run re-prompts and dies by
construction. Structural fix, not another click: `StartCalendarInterval` 02:00 → **13:00**
(repo plist + installed agent + runner clone, comment updated with the why). netprobe agent
stays until the first green 13:00 run, then bootout. Pages updated:
[scale-tests](../testing/scale-tests.md) (diagnosis CLOSED + the portable lesson); ratings
Run-59 Addendum 3. Lesson: a macOS launchd job whose tooling starts a VM must run when a human
is at the screen — the TCC prompt kills unattended runs silently, and it pattern-matches to
"network down."
