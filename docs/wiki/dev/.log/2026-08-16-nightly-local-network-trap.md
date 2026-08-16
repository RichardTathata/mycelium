# ingest — the local nightly's macOS Local-Network trap (2026-08-16)

Found via analysis Run 59's evidence check: the launchd scale runner dark since 2026-07-14, every
row exit-2 at image build. Root-caused to the macOS Local Network TCC prompt raised by the
script's between-rounds `colima stop -f && colima start` — attributed to the launchd session's
own identity (interactive shells ride the terminal's grant, which is why casual testing never
reproduced it), unanswered at 02:00 ⇒ VM session cannot reach its NAT gateway for DNS ⇒ registry
unreachable. Fixed by one supervised `launchctl kickstart` + user-clicked Allow (persists
per-binary); verified same-day — entries PASS (first green row in a month) and resilience PASS
post-grant; the scale suite's FAIL was the documented formation-variance ceiling (ran fully, 100
containers up). Also found + fixed: the boot-volume runner clone (~/Mycelium) had drifted a month
stale (P6.4) — fast-forwarded; it does not auto-pull. Page updated:
[scale-tests](../testing/scale-tests.md) §CI status. Cautions recorded there: brew-upgrade
re-prompts once; keep the clone current.
