## [2026-09-17] ingest | reverting a MAJOR nobody needed — the clock seams go additive

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.1 · code
`mycelium-core/src/sim_seam.rs` and the types that went back to `Instant`.

### What happened

Item 6 PR 3 converted the peer table, `SwimMembership`, `SignalHandlers` and
`MeshHandle::last_signal` from `Instant` to `u64` monotonic nanoseconds, so the replay kernel could
own the clock. Those are **public types** — `CoreCtx::peers`, `ConnContext::peers`,
`pub mod swim_membership`, `pub mod signal`, `pub mod mesh_handle` — so the change forced a MAJOR.

Asked what the major was actually buying, the answer was: nothing anyone uses. **Every external use
in the workspace is `agent.peers()`, the method, which never changed.** No companion crate, no
example and no test outside `mycelium-core` reads `CoreCtx::peers` the field or calls
`MeshHandle::last_signal`.

And the cost was concrete rather than theoretical: `main` could no longer ship a PATCH or a MINOR.
A security fix would have meant shipping a premature 3.0.0 or backporting to a 2.7.x branch — a
constraint created for a representation change with no consumers.

So it is reverted, and the release stays a **MINOR**.

### The principle that made the revert cheap

**What a replay must reproduce is the decision, not the representation.**

That was true all along and the first pass missed it. Every staleness question is one of three
shapes, and each gets a seam — so the stored `Instant` never needs reproducing:

| Shape | Seam | Used by |
|---|---|---|
| "how long since this" | `mono_elapsed(&Instant)` | sender-log window, quorum windows, `last_signal_age` |
| "how long between these two" | `mono_span(&Instant, &Instant)` | SWIM suspicion timeout, quorum-evidence rate limit, reorder hold |
| "has this deadline passed" | `mono_before(&Instant, &Instant)` | suppression expiry, trim cutoff, peer eviction |

**The third is the one the revert would otherwise have lost.** Suppression expiry, the sender-log
trim cutoff and peer eviction compare *two stamps the process took at different moments*. Going back
to `Instant` without seaming those would have left a replay comparing **its own** elapsed wall time
rather than the recording's — a silent fidelity regression traded for API stability. Recording the
**verdict** keeps both.

The forbidden-call check is what forced that to be faced: reverting the representation reintroduced
`Instant::now()` sites and the check failed immediately. The easy response was to re-baseline; the
right one was to ask which of the reintroduced sites a decision actually branches on. Three did.

`MeshHandle::last_signal_age` is **added**, not substituted — `last_signal` keeps its
`Option<Instant>`. The age is what callers computed anyway and matches
`last_signal_persistent`, but removing the old signature bought nothing.

### Two corrections made while doing it

**I under-reported the break surface.** I told the project owner it was two items; it was six —
`CoreCtx::peers`, `ConnContext::peers`, `MeshHandle::last_signal`, `SignalHandlers::last_signal`,
`SignalHandlers::suppress`, and `SwimMembership`'s four `now:` parameters. Corrected before the work
started, because it changed the size of the job even though it did not change the decision.

**`MONO_ORIGIN_NS`'s stated rationale went stale.** It was introduced because `SignalLog::seed`
needed a point before process start; `seed` is back on `Instant` and gets that natively. The offset
is kept — the property is still real for any future caller — but the doc now says so instead of
naming a caller that no longer needs it. A rationale that has quietly stopped being true is worse
than no rationale, because it will be believed.

### Admission, recorded per the check's own rule

Baseline **180 sites across 43 files** (from 159). The reintroduced `Instant::now()` calls take a
stamp and nothing branches on them directly; §2.1 of the inventory now carries the table above and
the reason. The check's instruction for a deliberate site is *"add it to the inventory §2, then
`--update`"* — both done, in that order.

### Gates

`make check` clean · core **191** / **218** (sim) · `mycelium-sim` 24 + 6 · mycelium **473**
(`tls,metrics,a2a,llm`) / **535** (`compliance,a2a`).
