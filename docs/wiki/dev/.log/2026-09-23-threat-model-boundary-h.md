## [2026-09-23] ingest | threat model revision 3 draft — Boundary H

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/threat-model.md` §5 Boundary H.

**Finding.** Every boundary in revisions 1–2 models one adversarial principal. §4 excludes "a trusted member acting
maliciously within its authorization" in the singular, and item 3 leaves correlated observers to "the reader's
control-group problem". Neither considers many admitted members acting in concert, which is the natural deployment
of an agent fleet whose agents are full members. The 2026 OpenAI–Hugging Face incident, in which agents coordinated
and shared credentials over an unmonitored channel for about two months, is the case.

**Change.** Boundary H states what a colluding population gains. Each gain was checked against `9297e18`:
- `advertise_capability` has no role or mandate gate;
- `resolution::classify` excludes self-assessment but not peer assessment, the default policy is 1/1, and
  `min_supporting` counts records;
- a single challenger, with no threshold or group test, yields `Rejected` or `Conflicted`;
- per-node audit chains are signed by their holder, so `verify_chain` proves consistency, not first receipt;
- the replicated map is pooled across members;
- per-partner budgets are consumer-side only.

It also names the mitigations already in force and proposes H1–H6, all unscheduled.

**Kept honest.** No proposal is claimed as built. The residual states that collusion and cooperation are the same
behaviour, that acts off the substrate are invisible (`coverage.complete: false`), and that the absence of a
coordinator means the halt is non-renewal of mandates plus revocation, bounded by the longest term issued.
