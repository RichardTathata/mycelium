## [2026-09-13] ingest | rev 1.11 — the coordination sense of "contract" restored

Up: [dev](../dev.md). Plan [rev 1.11](../../../plans/v3-contracts-axis.md) §1.2, §6.9, D39.

**Finding.** The epoch was named for *programming by contract rather than by specification* —
tuple space, blackboard, contract net. Two shipped in v2 as companions; contract net was a v3.0
"auction / bidding" packaging candidate that rev 1.0 marked *subsumed* by item 1 and never scheduled.
Meanwhile "contract" in the contracts axis came to mean what an ack/identity/mandate *proves*. The
coordination idea survived only as §13.2's commitment-as-composition, deferred past Phase D.

**Decision.** §1.2 states the two senses and their relation: verification contracts exist to make
coordination contracts trustworthy. §6.9 schedules the **commitment companion** (contract net) after
item 1 PR 2 as CN1–CN3 with a Phase C exit — announce (signal / `declare_requirement`), offers
(`kv().append`), award (the tuple-space lowest-candidate rule or a `group_propose` round; **the award
is a receipt**), report (item 1 receipt), assessment (signed record). One-to-one with §13.2's five
records; no planner; one award per requirement per epoch with a replay witness. `cn/` prefix. D39.
Renamed from "auction": the model is commitment, not price. ROADMAP row and the pattern-coverage
bullet updated to point at §6.9.
