## [2026-11-13] ingest | a Stateright model of the acceptor (critique plan CR10)

A new workspace crate, `mycelium-model`, checks the consensus acceptor's design at a stated bound in CI: slot
reuse after a release or a lapsed lease, the lease's wall-clock end under a skew bound, the electorate
migration. It calls the engine's own decision functions through a new feature of `mycelium`, `model-internals`;
three planted faults — one per area — are each required to produce a counterexample.

- Pages touched: new `dev/testing/acceptor-model.md` (the areas, how the model stays tied to the code, what a
  moved state-count pin means, the lore); `dev/testing/testing.md` (a section pointing at it, and the child-page
  list); `dev/dev.md` (the testing entry); `dev/security.md` (the `deny(unsafe_code)` list gains the crate).
- Outside the wiki: `docs/operations/what-is-proven.md` (a new *Proven in CI* row with the bound; the acceptor
  and lease gap rows restated), the critique plan's CR10 row (closed on evidence, the *not built* items named),
  `docs/plans/README.md`, `CLAUDE.md`, `CHANGELOG.md`, `docs/features/registry.toml` (two exclusions).
- Code: `mycelium-model/` (`src/slot.rs`, `src/migration.rs`, `src/shared.rs`, `tests/model.rs`,
  `tests/bridge.rs`); `src/model_internals.rs` behind `model-internals`; `electorate_vote_filter`,
  `Carry::report` and `set_aside_finished` in `src/consensus.rs` are `pub(crate)` now. CI job `model`; two
  clippy lines in `make check`.
- Found on the way: nothing in the design as modelled. The lease plant (no margin) is the engine as it stands
  — critique finding L13 as a model trace; the margin is CR11's and is not built.
