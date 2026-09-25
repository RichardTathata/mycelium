## [2026-09-25] ingest | Boundary H closure plan rev 0.3: the gaps in rev 0.2's own answers

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` §7.

**Question.** Did rev 0.2 fully address the external review of #402? Mostly. Three findings are fixed on `main`
(order-independent revocation, the *F* − 2*s* wording, the monotonicity bug), and the clock is fixed in #405.
Four answers were incomplete:
- **C8 forgot epochs.** A restart also resets the installed epoch and the grant verifier's highest epoch, so a
  superseded mandate passes again. C8 now persists them as a floor.
- **C9 was scoped to the wiki.** The check-then-act pause window exists at every A1 site. C9 now states it once
  and gives each site prevent, cancel or detect.
- **The clock audit was missing.** Eleven `hlc.current()` reads remain, one of them a nonce replay window. New C11
  classifies them, adds a regression guard and the reviewer's end-to-end quiet-node test, and makes the
  wall-clock assumption explicit.
- **Stops were only modelled.** New C12 measures T_admit and T_drain in the confined-fleet deployment, after C10.

Checked on the way: `intent::now_ms`, `capability_ops::now_ms` and `federation::edge::now_ms` read the wall
clock, so they are not frozen-clock sites.
