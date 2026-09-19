# 2026-09-19 — item 4's decisive demonstration: the control envelope

**What shipped.** `examples/control_envelope_viz.rs` + `.html` (port `:8096`), the gallery row and
the browser-showcase row, plan §12.1 row for item 4 marked delivered. **With it the axis' §12.1
gallery is complete** — all six demonstrations exist and the four that are CLI run in CI.

**The design problem, and how it was resolved honestly.** §12.1 asks for *allocated rights and
budgets under load, the enforce-allocated profile versus advisory, and the combined-feedback
scenario*. The three real governors (membership, tuning, opacity) are **crate-private loops fed by a
live cluster** — an example cannot construct or drive them, and one that pretended to would be
showing its own scaffolding. What *is* public is the contract all three call: `decide`,
`spacing_allows`, `may_propose`, `ConfidenceBound`, `Profile`, and `RightsLedger`. The dashboard
drives those, under a load generator, and says on screen that it is doing so.

**The shape that makes it worth watching.** The same proposal stream is run past the envelope under
**all four profiles simultaneously**, as four columns. One run of the load produces, at a glance:

| profile | proceeded | would hold | held |
|---|---|---|---|
| legacy | 12 | 0 | 0 |
| observe | 12 | 6 | 0 |
| enforce-local | 6 | 0 | 6 |
| enforce-allocated | 6 | 0 | 6 |

That is the entire argument for shadow-first in one picture: `observe` and `legacy` take the *same*
actions, and the only difference is a number an operator can read before deciding. The live rung is
the one whose actions actually happen; the others are counted in shadow, which is what `observe` is.

**The budget half, verified live.** Stepping to `enforce-allocated` consumes the granted eight units
and then refuses — `granted 8 · held 8 · admitted 8 · refused 3 · recorded_rejections 3`. The
rejections are in the ledger, which is the property that matters: a refusal nobody wrote down is
indistinguishable from work nobody asked for.

**What it does not show, on screen as well as here.** The combined-feedback scenario is replay
**scenario C** — a schedule sweep over the governors' shipped decisions, whose claim is about *every*
interleaving in the sweep. A live dashboard shows one interleaving, so it would be a weaker thing
wearing the same name. That belongs in a test and stays there.

**Not in CI's run set, deliberately.** Browser showcases run continuously (Ctrl-C to stop) and the
examples index says so; they are built by `cargo build --examples`, which is the drift check
available for them. The four CLI demonstrations are the ones CI runs.
