## [2026-10-03] ingest | zero gaps — the close (Z0)

**What:** `docs/plans/zero-gaps.md` delivered (rev 1.0): Z6–Z9 #507, Z2 #508, Z4 #509, Z3 #510, Z5 #511,
Z1 #512; the doc-coverage matrix carries no `~` that says *code gap*; `what-is-proven.md`'s third table
holds only lines that need a counterparty, hardware or research; v2.22.0.

**Durable knowledge:** six PRs that each append a changelog bullet under one heading conflict pairwise
on the anchor, and a conflicting PR runs no CI — the small four's merge silently parked the other four
until each was rebased with both sides kept. The order that costs least is one rebase per merge, not a
chain: a chain re-bases every survivor anyway. (Memory note updated.)
