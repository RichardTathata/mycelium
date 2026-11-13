# The acceptor's model (Stateright)

↑ [dev/testing/](testing.md) · beside [replay](replay.md) (one recorded run, replayed) and the loom models
([testing § Loom](testing.md#loom-permutation-model-checking-of-the-atomic-patterns): thread interleavings of an
atomic idiom) · the row it delivers: critique plan CR10 (`docs/plans/external-critique-2026-11-12.md`) · the
ledger row: `docs/operations/what-is-proven.md#acceptor-design-model-checked`

**What it is.** `mycelium-model` is a workspace crate that checks the **design** of the single-decree acceptor
at a **stated bound**: it enumerates every interleaving of a small, finite configuration and checks invariants
in every state. It does not prove the implementation, and it is not a linearizability checker over recorded
histories. Canon for what is shared with the engine, what is written again and what is not modelled is the crate
documentation, `mycelium-model/src/lib.rs` — this page does not repeat those tables.

## The three areas, and where each lives

| Area | Invariant (the constant in the code) | Model | Granularity |
|---|---|---|---|
| Slot reuse after a release or a lapsed lease | at most one value decided per slot and life (`slot::ONE_VALUE_PER_LIFE`) | `mycelium-model/src/slot.rs`, `SlotModel::{acceptor, reuse_after_lapse, reuse_after_release, reuse}` | every message its own step, except `reuse` (a ballot is one step) |
| The lease's wall-clock end under a skew bound δ | no holder granted while another is inside its own deadline less δ (`slot::GRANT_OUTSIDE_SKEW`, `slot::ONE_HOLDER`) | `SlotModel::lease` | a ballot is one step; each clock, each record learned and each release its own |
| The electorate migration | nothing decided by a successor before the predecessor's retirement is visible (`migration::RETIREMENT_FIRST`, with `COMPLETION_FIRST` and `ONE_VALUE_ACROSS`) | `mycelium-model/src/migration.rs`, `MigrationModel::{rolling, contended}` | the predecessor's ballots message by message; the successor's one step each; genesis, retirement and completion each a certificate of votes |

The bound of each configuration is its constructor; the number of states it explores is pinned in
`mycelium-model/tests/model.rs`; both are restated in the ledger row.

## How the model stays tied to the code

- **It calls the engine.** `mycelium`'s cargo feature `model-internals` (off by default; only this crate
  enables it — the `fuzz-internals` precedent) exposes `src/model_internals.rs`, a module of wrappers over
  `prepare_slot`, `claim_envelope`, `phase1_choice` with `set_aside_finished`, `consensus_life::read_slot`, the
  envelope and `electorate_vote_filter`; `ElectorateDecl::quorum` and `contains` are already public.
  `mycelium-model/src/shared.rs` is the bridge: it converts the model's small hashable state to the engine's
  types, calls, and converts back — and caches, since each function is pure. `tests/bridge.rs` holds the bridge
  to the engine's rules.
- **A pinned state count is a tripwire on those functions.** Change the promise rule, the accept rule, the
  phase-1 rule or `read_slot` and the set of reachable states changes: a passing run then fails on its pin, not
  on an invariant. That is the model noticing an engine change. Re-run, read whether an invariant still holds,
  and move the pin **with** the ledger row, in the same PR.
- **What the model writes again does not notice.** The order of a proposer's steps (`propose_inner`,
  `prepare_phase`), the acceptor's answers around the shared calls, the gate (`claim_gated`) and the migration's
  readiness checks (`successor_ready`, `migration_complete`, `carry_admissible`) are engine methods over a
  `TaskCtx`; the model has its own copy. A change there must be made in the model by hand — the reviewer's
  question for any PR touching those functions is "does `mycelium-model` still describe this?" Sharing them is
  critique plan row CR19 (the `consensus.rs` split).

## The model can fail — and that is tested

Three `planted_*` tests each switch one fault on and require the checker to return a counterexample, then check
the trace's last state: votes counted from outside the electorate (`with_outside_votes_counted`), a lease ended
with no margin (`with_no_margin`), a successor deciding early (`with_successor_deciding_early`). A model whose
invariants cannot be broken proves nothing; these are the equivalent of loom's broken twins, but they run, and
pass by failing.

The second plant is **the engine as it stands**: the holder's deadline has no margin (critique finding L13; the
margin is row CR11's and is not built). The lease area therefore checks CR11's design, not the code, and the
ledger says so. When CR11 lands, the margin should be one of the shared functions and the plant should stay.

Each passing run must also **reach** what its bound can reach (Stateright's `sometimes` properties: a second
life, a second holder, a carried value), so a configuration that silently stopped deciding anything fails.

## Lore from building it

- **Time needs two ticks of skew.** Readers end a lease at `now > expiry` and a holder stops at
  `now < deadline`, both strict, so on a discrete clock a skew of one tick gives no double grant even with no
  margin. The lease configuration uses δ = 2 ticks for that reason; with δ = 1 the plant is not found.
- **"Visible" includes the proof.** The model's first migration run broke `RETIREMENT_FIRST` in the unplanted
  model: a successor member that held the completion but never the retirement record answered. That is the
  design — `predecessor_state` reads a verified completion as proof of the retirement — so the invariant reads
  *the retirement or its proof*. The second break was the model's own: a carry ballot that did not decide was
  counted as carried. `agent::migration` requires `Committed`; so does the model now.
- **The decided floor is left out on purpose.** The model's floor is the ended record's ballot alone, so its
  acceptors refuse *less* than the engine's: the invariants hold without the floor's help.
- **Stateright's `timeout` and `target_state_count` are not bounds to rely on**: the first is checked only
  between jobs, the second counts generated states, not distinct ones. The configurations are finite instead,
  and explored depth-first (the visited set is all that is kept; breadth-first held tens of gigabytes of
  frontier at the same sizes). Planted runs go breadth-first and stop at the first counterexample.
- **Sizes grow fast.** A fourth acceptor, a third ballot or a second call per proposer each multiplied a
  message-level configuration past what a CI job should run; that is why the node outside the electorate is
  stateless (it promises and votes for everything — the worst case for the filter, at no cost when the filter
  works), and why three lives and skewed clocks are checked with a ballot as one step.

## Running it

```bash
cargo test -p mycelium-model --release          # CI's `model` job; about a minute on a 12-core laptop
cargo test -p mycelium-model --release -- planted --nocapture   # the three counterexamples, printed
cargo clippy -p mycelium-model --all-targets -- -D warnings     # in `make check`
cargo clippy --lib --no-default-features --features model-internals -- -D warnings   # the exported module
```

Release, not debug: the checker's inner loop is the whole cost. Stateright is pinned (`=0.31.0`) and is in the
root `Cargo.lock` only for this crate — it is not a dependency or a dev-dependency of `mycelium`,
`mycelium-core` or any published crate. Adding it left every existing lock entry as it was; a future
`cargo update` that touches `dashmap` will want to move `lock_api`, `once_cell` and `parking_lot_core` with it.
