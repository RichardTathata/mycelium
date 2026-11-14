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
| The lease's wall-clock end under a skew bound δ | no holder granted while another is inside its own deadline less δ (`slot::GRANT_OUTSIDE_SKEW`, `slot::ONE_HOLDER`) | `SlotModel::{lease, lease_edge}` | a ballot is one step; each clock, each record learned and each release its own |
| The electorate migration | the two groups decide one value (`migration::ONE_VALUE_ACROSS`); a visible completion proves the retirement decided and the carry done (`COMPLETION_PROVES`); and the row's sentence, nothing decided by a successor before the predecessor's retirement is visible, as guard assertions (`RETIREMENT_FIRST`, `COMPLETION_FIRST` — see below) | `mycelium-model/src/migration.rs`, `MigrationModel::{rolling, stalled_carry, contended, overlap}` | the predecessor's ballots message by message; the successor's one step each; genesis, retirement and completion each a certificate of votes |

The bound of each configuration is its constructor; the number of states it explores is pinned in
`mycelium-model/tests/model.rs`, beside the reachability properties the run must carry; both are restated in
the ledger row.

## The finding — found by the model 2026-11-14 and confirmed on the engine the same day

With **overlapping electorates** the successor's carry can decide a value the predecessor did not. Predecessor
{0, 1, 2}, successor {1, 2, 3}:

1. node 1 accepts `v1` in the predecessor at ballot 1; no quorum follows it;
2. nodes 0 and 2 decide `v0` at ballot 2;
3. nodes 1 and 2 retire the group; their manifests name `v0` as the highest acceptance, so the carry proposes
   `v0` at ballot 3;
4. the rest of the carry's quorum is node 1. Its promise reports the acceptance it holds — `v1`@1, made in the
   predecessor: an acceptor's memory of a slot is one record, whichever group asks;
5. phase 1 carries the highest report of its promise quorum (`choose_after_prepare`) and knows nothing of the
   manifests. The successor decides `v1`.

The engine reading: `propose_carry`'s `above` only lifts the carry's ballot (`src/consensus.rs:1835`–`1837`);
`src/agent/migration.rs` states the intent as "an acceptance the successor already holds above it wins", and
nothing checks "above it". **Limits:** in the engine an acceptor holding a live decision record of another value
refuses, and so does the committer, so it needs `v0`'s record not yet at the carry's quorum; and the engine
counts it afterwards (`Carried::Conflict`, `commit_conflicts`). The model has no decision record in the
migration and re-implements the carry; two engine tests of the interleaving, written the same day, are what
confirm it.

Why the first bound missed it: `contended` had ballots to 2 and the carry runs above every ballot the manifests
name, so a ballot-2 decision could never be carried. At ballots to 3 that configuration is 110,929,684 states
(about eight minutes on twelve cores) — not a CI run. CI holds the trace in `MigrationModel::overlap` (ballots
fixed, node 0 a bystander to the migration's own steps) as
`found_a_carry_adopts_a_stale_acceptance_of_an_overlapping_member_and_the_groups_decide_two_values`, **a test
that expects the counterexample**. Ledger:
`docs/operations/what-is-proven.md#carry-across-overlapping-electorates-keeps-value`.

**The fix is decided (maintainer, 2026-11-14) and built in its own PR:** the acceptor record carries the group
each acceptance was made in, and a carry hears only acceptances made in the successor; an acceptance with no
recorded group is treated as today. After that PR merges the model gives each acceptance its group and the
expected-failure test is inverted.

**A fix the model passed and that is wrong.** A model-side switch — the carry's phase 1 counting the manifests'
highest acceptance as one more report — held in `overlap` and in the full configuration at ballots to 3. It is
unsound: it compares ballots across two groups, so a carry run again whose manifests show a predecessor
acceptance at a higher ballot than the successor's own decision would overwrite that decision — a case this
model cannot reach (it has no restarted carry after the successor has decided). The switch is not in the tree.
A pass at a bound says the bound holds, not that the change is right.

## How the model stays tied to the code

- **It calls the engine.** `mycelium`'s cargo feature `model-internals` (off by default; only this crate
  enables it — the `fuzz-internals` precedent) exposes `src/model_internals.rs`, a module of wrappers over
  `prepare_slot`, `claim_envelope`, `phase1_choice` with `set_aside_finished`, `consensus_life::read_slot`, the
  envelope and `electorate_vote_filter`; `ElectorateDecl::quorum` and `contains` are already public.
  `mycelium-model/src/shared.rs` is the bridge: it converts the model's small hashable state to the engine's
  types, calls, and converts back — and caches, since each function is pure. `tests/bridge.rs` holds the bridge
  to the engine's rules.
- **A pinned state count is a tripwire on those functions — a weaker one than an invariant.** A change to a
  shared function that changes the set of reachable states fails a passing run on its pin. Re-run, read whether
  an invariant still holds, and move the pin **with** the ledger row, in the same PR. A pin moved with its row
  is also how a wrong change could be waved through, so the table below says which rules have nothing better.
- **Read this page before changing `prepare_slot`, the accept rule (`claim`, `may_accept`), the phase-1 rule
  (`phase1_choice`, `choose_after_prepare`, `set_aside_finished`, `Carry::report`) or `read_slot`**: the model
  calls them. Since the review of #784 the bridge reads back everything the engine returns — the acceptor after
  a promise or an acceptance, and what a promise reports — so a fault in any of those reaches the model.
  `model_internals::phase1` composes three engine functions in the order `prepare_phase` does; only text ties
  the two (`tests/bridge.rs`, `the_phase_one_wrapper_composes_what_the_engines_prepare_phase_composes`).
- **What the model writes again does not notice.** The order of a proposer's steps (`propose_inner`,
  `prepare_phase`), the acceptor's answers around the shared calls, the gate (`claim_gated`) and the migration's
  readiness checks (`successor_ready`, `migration_complete`, `carry_admissible`) are engine methods over a
  `TaskCtx`; the model has its own copy. A change there must be made in the model by hand — the reviewer's
  question for any PR touching those functions is "does `mycelium-model` still describe this?" Sharing them is
  critique plan row CR19 (the `consensus.rs` split).

## What holds each rule — from the review's mutation runs (#784)

The reviewer planted faults in the engine's shared functions and in the model's own rules and ran the suite.
What caught each, at these bounds:

| Rule | Caught by |
|---|---|
| a promise reports what was accepted; a promise keeps the acceptance (`prepare_slot`) | since the bridge fix: `tests/bridge.rs` and `ONE_VALUE_PER_LIFE` (`reuse`, `lease`). Before it: **nothing** |
| phase 1 adopts the highest report | the bridge; `ONE_VALUE_PER_LIFE` in `reuse` only |
| the lapse is `>`, not `>=` (`read_slot`) | the bridge; `ONE_VALUE_PER_LIFE` in three runs |
| the vote filter; the quorum; `set_aside_finished` dropping everything | the bridge; invariants in several runs |
| `set_aside_finished` dropping nothing; `read_slot` ignoring a marker | the bridge; reachability only (no reuse happens) |
| **one value per ballot in the accept rule** | **the bridge test only** — no model run or pin moves |
| **the floor in `claim` and in `prepare_slot`** | the bridge; in the model runs **a pin only** |
| **an acceptor refuses a proposal while it holds a live decision of another value** (the model's copy) | **a pin only** |
| **a retirement promise closes the group** (the model's copy) | **pins only** |
| **the carry runs above the manifests' ballots** (the model's copy) | `ONE_VALUE_ACROSS` in `contended`; **a pin only** in `rolling` |
| **the top record has not ended before a guard is issued** (the model's copy) | **nothing** — in the model a lapsed top also fails the deadline check, and a holder cannot have released a guard it was not yet issued, so the check never decides |
| a manifest only after closing; manifests from a majority; a carry only with the retirement visible; a completion only after the carry | `ONE_VALUE_ACROSS` or `RETIREMENT_FIRST` |

A promise given twice at one ballot, and an acceptance below the promise, made runs **grow without end** rather
than fail (one reached 37 GB): the CI job has a `timeout-minutes` for that.

## What the model does not check — beyond the crate's *not modelled* list

- **Contention inside a reused life, at message grain.** In `reuse_after_lapse` and `reuse_after_release` no
  two proposers are in flight once a first decision exists; only `reuse` (a ballot as one step) contends a
  second and third life. A three-proposer message-grain run did not finish (18.1 million states when stopped).
- **The lease at message grain.** `lease` and `lease_edge` run a ballot as one step; capped message-grain runs
  in review were not exhaustive.
- **Margin δ against δ − 1.** On whole ticks the least margin that holds is δ − 1. `lease` (margin δ) shows δ is
  enough and cannot tell it from δ − 1; `lease_edge` (margin δ − 1) and its planted twin (δ − 2) show the edge.
  None of it speaks to whether a real holder's comparison is strict, or to clocks that run at different rates.
- **The node outside the electorate, in a passing run.** It is inert there — the run without it explores the
  same states. It exists for the plant.
- **`RETIREMENT_FIRST` and `COMPLETION_FIRST` as searches.** They read the flags the model's own guard read in
  the step that set them: no interleaving can break them, only a guard switched off. They are guard
  assertions. `ONE_VALUE_ACROSS` and `COMPLETION_PROVES` are what the checker searches for.
- **In the migration:** a completion vote requires the carry done, as one global fact — no acceptor in the
  engine checks that; it is the completer's discipline (`src/agent/migration.rs`). There is one carrier and it
  is outside the predecessor; a second, racing or restarted completer never occurs; the carry-conflict path
  (`Superseded` → `Conflict`, the completion still proposed, `src/agent/migration.rs:355`–`376`) is not
  modelled; the predecessor closes on a retirement promise or a manifest only (the engine also closes on a
  visible retirement); the retirement is decided by a majority of promises (the engine needs votes); and there
  is no decision record, so no live-record refusal.

## The model can fail — and that is tested

The `planted_*` tests each switch one fault on and require the checker to return a counterexample, then check
the trace's last state: the row's three — votes counted from outside the electorate
(`with_outside_votes_counted`), a lease ended with no margin (`with_no_margin`), a successor deciding early
(`with_successor_deciding_early`) — and two from the review: a margin one tick short of the least, and a
completion that proves nothing (`with_completion_unproven`). A model whose
invariants cannot be broken proves nothing; these are the equivalent of loom's broken twins, but they run, and
pass by failing.

The second plant sets the model's margin to 0, which is what the engine's holder has today (critique finding
L13; the margin is row CR11's and is not built). Its trace is **the model's** — a ballot as one step, the
model's own guard, δ of two ticks — not a run of the engine, whose holder uses a monotonic deadline from the
acquisition's start (`src/agent/consensus_handle.rs:822`). The lease area therefore checks CR11's design, not
the code, and the ledger says so. When CR11 lands, the margin should be one of the shared functions and the plant should stay.

Each passing run must also **reach** what its bound can reach (Stateright's `sometimes` properties: a second
life, a second holder, a carried value), so a configuration that silently stopped deciding anything fails.

## Lore from building it

- **Time needs two ticks of skew.** Readers end a lease at `now > expiry` and a holder stops at
  `now < deadline`, both strict, so on a discrete clock a skew of one tick gives no double grant even with no
  margin. The lease configuration uses δ = 2 ticks for that reason; with δ = 1 the plant is not found. The same
  two strict comparisons are why the least margin on whole ticks is δ − 1.
- **A bound can hide the case it was named for.** `contended` at ballots to 2 could not carry a ballot-2
  decision; the review said so, and raising the bound is what found the 2026-11-14 finding. A reachability
  property for the thing a run is named after is cheap; write it.
- **A bridge that converts must read back everything.** The first bridge kept the model's own copy of what a
  promise reports and retains, and two textbook acceptor faults planted in `prepare_slot` passed every test.
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
cargo test -p mycelium-model --release --locked # CI's `model` job; about a minute and a half on a 12-core laptop
cargo test -p mycelium-model --release -- planted --nocapture   # the three counterexamples, printed
cargo clippy -p mycelium-model --all-targets -- -D warnings     # in `make check`
cargo clippy --lib --no-default-features --features model-internals -- -D warnings   # the exported module
```

Release, not debug: the checker's inner loop is the whole cost. Stateright is pinned (`=0.31.0`) and is in the
root `Cargo.lock` only for this crate — it is not a dependency or a dev-dependency of `mycelium`,
`mycelium-core` or any published crate. Adding it left every existing lock entry as it was; a future
`cargo update` that touches `dashmap` will want to move `lock_api`, `once_cell` and `parking_lot_core` with it.
