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
- The lease plant (margin 0) is the model's rendering of critique finding L13 at a bound; the margin is CR11's
  and is not built.
- 2026-11-14, after the PR's adversarial review: the bridge reads back everything `prepare_slot` returns (two
  faults planted there had passed); the migration's row invariants are named for what they are (guard
  assertions) and `COMPLETION_PROVES` added; the page lists which rule is held by an invariant, a pin, the
  bridge or nothing, and what the model does not check. **Raising the contended migration's bound found a
  counterexample with no fault planted** — a carry between overlapping electorates adopting a stale acceptance —
  found by the model and confirmed on the engine the same day; the fix is decided and in its own PR; the page and the ledger
  (`#carry-across-overlapping-electorates-keeps-value`) carry it.
