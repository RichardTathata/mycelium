# 2026-09-28 — proposition alignment: one sentence, one proof page, one path

**What:** a step-back review of the story as the front doors tell it found four positioning sentences,
the proof gaps stated in six places, three funnels with three first steps, and three defects (a GitHub
description a year stale, both SDK READMEs and twelve `Cargo.toml` `repository` fields on the pre-move
account, the crate doc saying *two primitives*). Plan: `docs/plans/proposition-alignment.md`; executed
the same day (PR #434).

**Durable knowledge:**
- `docs/positioning.md` is the single source of the sentence and the five-step path;
  `scripts/check-positioning.sh` (in `make check`) fails a door that does not quote it verbatim, a
  funnel whose path differs (link targets stripped), or a live `RichardEko` link. The GitHub description
  and the `Cargo.toml` descriptions are outside its reach — `RELEASING.md` §6 names them.
- `docs/operations/what-is-proven.md` is the one dated page for proven / demonstrated / not yet shown;
  `CLAUDE.md`, the axis plan §10, the publications ledger and both decks link it rather than restating.
- The buyer deck leads with authority and evidence (D3); both decks share one title (D2).
- One correction to the review itself: performance *is* micro-benchmarked (`tuning.md` §Performance
  baselines, 2026-07-10); what is missing is any figure under load, and a bench job in CI.

**Pages touched:** `wiki.md` (the sentence), this log. `dev/history.md` gained a dated entry.
