## [2026-09-17] ingest | item 6 PR 3 — channel coverage across the substrate

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.3, §6 · code
`mycelium-core/src/{persistence,signal,writer,connection}.rs`, `src/agent/{audit,evidence_journal,
tasks,topology}.rs`.

**Why these eight.** Widening the forbidden-call check to cover `try_send` surfaced fifteen unrouted
bounded sends — the check's scope had never matched the coverage map's. Eight are now routed, and the
selection was by *what a drop costs*:

- the **WAL append queue** — a full queue skips a record, which is one of the three consequences the
  inventory names when it says fullness is a kernel decision;
- the per-handler **signal** channel — a handler that cannot keep up drops a signal;
- the **StateRequest** writer — a drop skips a state sync;
- the **pong** — a dropped pong looks to the peer exactly like a dead node;
- the **audit export** drain — a dropped audit record is evidence that never existed;
- the **AE evidence journal**, two pings.

**The evidence journal is the one worth naming.** Its saturation behaviour is a *stated guarantee*,
written this morning: *a full queue is refused, never silently dropped, because a dropped evidence
record and a decision nobody made are indistinguishable afterwards.* That guarantee had a unit test
using a deliberately stalled writer. Now the saturation itself can be **recorded and replayed**,
which is a stronger thing: the guarantee can be checked against a run that really saturated rather
than one contrived to.

**Baseline 190 across 43 files**, from 199.

**Left deliberately**, and recorded rather than quietly skipped: four sends inside `tasks.rs` macro
bodies and two SSE sends in `a2a.rs`. The macro ones need care that a mechanical edit would not give
them, and doing them badly at the end of a long pass is how a seam acquires a silent hole.

**Gates.** `make check` clean; core 179/200; mycelium 521 (`compliance,a2a`) and 402 (`sim`).
