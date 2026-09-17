## [2026-09-17] ingest | item 6 PR 3 — the peer table's clock, and a branch that had been unreachable

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.1 · code
`mycelium-core/src/{context,swim,swim_membership,connection}.rs`, `src/agent/{mod,tasks,emergent}.rs`.

### The change

`Arc<papaya::HashMap<NodeId, Instant>>` → `Arc<papaya::HashMap<NodeId, u64>>` — monotonic
nanoseconds from the clock seam. It appears on `CoreCtx`, `ConnContext`, `SwimState` and
`TaskContext`, and `SwimMembership`'s four `now` parameters and its `changed` field move with it.
**`CoreCtx::peers` is public**, so this is an API change, though the value's meaning is identical.

Baseline **170 across 43 files**, from 179.

### Two things the work settled

**The peer table and the SWIM membership table are one unit.** `merge_gossip` reads the clock once
and passes the same `now` to `SwimMembership::apply` *and* to `apply_effect`, which writes the peer
table. Converting either alone would have left that function reading two clocks a few instructions
apart — not a bug that would fail a test, just a small permanent inconsistency in the one place the
codebase was careful to avoid one. They were converted together.

**`SwimMembership` needed no restructuring at all.** It has always taken its clock as a parameter —
*"`now` is injected so the table stays pure/testable"*, written long before any of this. Only the
type changed. The module that was already clock-injected was the cheapest thing in the refactor, by
a wide margin, which is the argument for the pattern in one data point.

### What the representation cost, and what it bought

Monotonic nanoseconds count from **process start**. So `Instant::now() - Duration::from_secs(600)`
has no equivalent: a test process alive for milliseconds has no "ten minutes ago" to insert.

That is a real loss, and it surfaced immediately. `compute_view_confidence`'s staleness branch —
*peer known but not heard from inside `HEARD_WINDOW`* — became unreachable from a test. The existing
test covered only the fresh side, so a comparison stuck at "always fresh" would have kept the suite
green.

The fix is the pattern `SwimMembership` already demonstrated: `compute_view_confidence_at(ctx,
now_ns)`, with `compute_view_confidence` supplying the real reading. The branch is now tested in both
directions, and verified by breaking `mono_between` each way — `ZERO` (always fresh) fails the new
test, `MAX` (never fresh) fails both view-confidence tests.

So the representation change did not merely cost testability, it forced an injection that **left the
function more testable than it found it**. Worth remembering when the remaining `Instant` sites come
up: the conversion is the moment to ask what the old type was quietly making impossible to test.

### Coverage, honestly

Of the behaviours moved onto this clock, breaking `mono_between` is caught by
`suspect_then_promote_after_timeout` (SWIM suspicion → dead), the reconnect-backoff test, and the two
view-confidence tests. The legacy non-SWIM **staleness eviction** in `tasks.rs` has no direct test —
it is `swim_enabled == false` only, and its conversion is a pure substitution
(`Instant::now().checked_sub(window)` → `mono_now_ns().checked_sub(window_ns)`, including the `None`
case). Recorded as a known gap rather than implied to be covered.

### Gates

`make check` clean · core **181** / **204** (sim) · mycelium **522** (`compliance,a2a`) / **403**
(`sim`).
