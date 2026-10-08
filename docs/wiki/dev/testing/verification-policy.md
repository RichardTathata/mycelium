# Verification policy — check what the change did not describe

↑ [testing](testing.md) · adopted 2026-10-06 (`CLAUDE.md` § Verification policy)

**The rule this page exists for: a change is verified against the surface it touches, not against its own
description.** A test, a review or a release gate derived from the change's description can only confirm
what the author already thought of. The defects that survive green CI and repeated analysis runs sit at the
edges a description never names: a sibling function, another door to the same setting, a promise in the plan
that did not make it into the PR, a test file missing from an allow-list.

## Why — the evidence (2026-10-06)

Every one of these passed CI, a fail-first witness, and at least one analysis run, and was found only when
an auditor enumerated surfaces from the code instead of the description:

| Gap | The check that passed | What it was derived from |
|---|---|---|
| TypeScript `scatterGather` refused 400 on every call for four months | a contract test with a mocked `fetch` | the line being changed (its timeout), not the request body |
| the same `kind`/`method` mistake fixed in `rpcCall` (2.15.0), left in `scatterGather` one function down | that fix's test | the reported site, not its siblings |
| R9 bounded the two runtime setters; `POST /gateway/govern/timing` still published any value | a fail-first test on the setters | the finding's named sites — while the change's own log said *one bound, three doors* |
| A3 marked merged with its history source unbuilt | the evaluator's tests | the PR, not the plan row's promises |
| S5's *corrupt content stays distinguishable* not built | the client's new error | the client half of the promise |
| five node-free Python test files in no CI job | CI green | an explicit file list, which fails open |

## The four rules

1. **Enumerate before fixing.** Before changing an invariant — a bound, a refusal, a field name, a durability
   rule — list every entry point that reads or writes the same thing (setters, HTTP routes, SDK verbs in each
   language, config loading, env overrides, intents, sibling functions with the same shape), with the grep
   that found them, in the PR. Test each, or say in the PR why one is out of scope.
2. **Plan rows close on evidence.** A delivery-table row is marked merged only beside a quotation of each of
   its promises and the code or test that delivers it. A promise not delivered is written as *not built* in
   the row, never left implied. The check is made by an agent or person who did not write the PR.
3. **CI collects tests by discovery, not by list.** A test runner is pointed at a directory (`--test '*'`, a
   pytest directory, bare `jest`), and a test that needs a live node skips when no node is configured — except in
   the step that brings the node up, which sets the suite's `*_LIVE_REQUIRED` guard so a missing node fails. A
   name is used only to select what a feature or cfg gate confines to a few tests (`--test decision_trace_replay`
   under `sim`, each fuzz target), and `check-test-inventory.py` proves every test requirement has a step.
4. **Independent adversarial review after each PR.** Before merge, an agent other than the author — with no
   access to the author's reasoning, given only the diff and the repository — enumerates the surfaces the
   change touches and tries to break them: siblings, other doors, the plan row's promises, literal execution of
   every instruction the PR adds. `/adversarial-review` is the procedure; its findings are fixed or answered in
   the PR before merge.

## Where it is enforced

- `CLAUDE.md` § Verification policy — every session reads it.
- `.github/pull_request_template.md` — the enumeration, the plan-row evidence and the review link are fields.
- `RELEASING.md` § 5b — no release while a delivery row marked merged lacks its evidence.
- **The record: the `test-coverage` CI job** (`scripts/ci-test-coverage.py`). After every test job finishes,
  it reads this run's own job logs and fails unless every test it knows of **executed** — passed or failed;
  not skipped, ignored, filtered out, or in a step that did not run. It knows a test if any log names it
  (including as skipped or ignored) or the universe lists it: the `test-universe` job (`cargo test --workspace
  --all-features -- --list`, then every workspace crate's own build without default features — the crate list
  read from `cargo metadata`, so a new crate is covered the day it joins — and `pytest --collect-only` over both
  Python test directories) and the TypeScript job (`jest --listTests` per file, and every test's title from jest's
  `--json` report, skipped ones included — `scripts/ci-jest-universe.py`, which refuses two same-titled tests in one
  file because the verbose log keys a test by its leaf title), and the **script-style suites**' cases: each
  suite's `--list` prints one `@@case-list@@ <suite>::<case>` line per case without running anything, and a run
  prints `@@case@@ <suite>::<case>` as each case starts (keyed `script <suite>::<case>`; a marker inside GitHub's
  echo of a step's source, `##[group]Run …`, is not a run). A shell suite whose cases are calls (`run_demo`,
  `run_scenario`, `leg`, `run`) derives its list from them with `scripts/list-script-cases.py`, which reads the
  shell grammar and fails the listing on a call it cannot read rather than dropping it. That covers
  `scripts/test-*` (except the inventory's own mutation suite, below), `wire-check-examples.sh` (one case per
  example directory), the seven `ci_smoke.sh` suites (each demo, mode or section a case; one case where the
  suite is one dependent scenario) and the LangGraph ladder (listed from the directory). The **Docker suites**
  (`cluster-suites.yml`: the 13 integration scenarios, overlay S11–S13, the ten federation legs, the
  confined-fleet phases, the stem-examples demos) run in a workflow this job never reads, so that workflow
  checks its own: before it starts Docker, each job lists its suite's cases on the host — derived from the
  runner's calls (`run.sh`, `run_federation.sh`), from the runner's scenario table (`run.py`), from the
  script's phase list (`test-confined-fleet.sh`) or from the Makefile's `STEM_DEMOS` (`make -s
  list-stem-examples`) — in a listing named for the job, and its `case-coverage` job runs `ci-test-coverage.py
  --scripts-only --require <the five jobs>` over the run's logs. Every listing is named (`@@test-universe@@
  begin <source>`) and each required one must be present and non-empty — `rust-python`, `typescript` and
  `scripts` here, one per suite job there — so one dropped listing fails the check rather than hiding behind
  the others. It also fails if a job it does not wait for is unfinished. The exceptions file serves both
  workflows: a `script` exception is judged (used, or stale) only where its suite is listed, a Rust, Python or
  TypeScript one only in this job. Tests that never run in CI by
  design — loom's broken twins, perf smokes, the fixture regenerator, `ignore` doctest fragments — are listed
  with a reason in `scripts/test-coverage-exceptions.txt`, and an exception that matches nothing fails. On
  #541's own CI (2026-10-07): 1,950 known, 1,936 executed, 14 excepted. Its first real run found the flake
  tier skipping test binaries behind a flake (`--no-fail-fast`, below). Self-tests: `scripts/test-ci-test-coverage.py`
  (the parser), `scripts/test-ci-retest.sh` (the retry tier).
- **What the observed job cannot see**, so the static check below still matters: a test compiled under no
  feature set the universe builds — the universe lists `--all-features` and every workspace crate's own featureless
  build (#551, 2.27.0; before it, only `mycelium`'s and `mycelium-core`'s libraries). What the universe cannot
  list — a gate no build in the run satisfies, such as a root-crate test gated off `gateway` or `tls` (the root's
  dev-dependencies unify both back in), an `all(a, not(b))` gate, a `--cfg` other than the loom job's — the
  static check refuses before push since 2026-10-08: it evaluates each gate against the features the step's
  **test build** enables, dev-dependency unification included. A test that needs `gateway` or `tls` off lives in
  `mycelium-gateway-free-tests` or `mycelium-tls-free-tests`. Of the script-style suites: a step's inline commands
  and single `cargo run --example` demonstrations (an exit code, no cases), a case a suite runs but does not list
  (it counts as executed — the list catches the opposite drift, a listed case that stopped running), and the scale
  suites (`scale-nightly.yml`, a self-hosted runner that is offline — V1). TypeScript
  tests are known by name since #551 (jest's `--json` report lists skipped ones too). It keys an integration
  test by file and name, not crate, so two crates' same-named tests would mask each other — the static check
  refuses that pair.
- `scripts/check-test-inventory.py` (in `make check` and CI, through `scripts/with-pyyaml.sh`) — the fast,
  **approximate** pre-push half: rule 3 checked positively from source and workflow text. It inventories every test *requirement*: per crate, the library's, each integration test's,
  each binary's and the doctests' test code under every distinct `cfg` gate (a file's `#![cfg]`, the gate on the
  `mod` that declares it, `#[cfg(all(test, feature = …))]` on a module, `#[cfg]` on a test function,
  `required-features`) — resolved to features to enable, features to leave off, and bare cfgs `RUSTFLAGS` must
  set; a cfg it cannot evaluate is uncovered, never "needs nothing". Then Python and TypeScript files (a live one
  only by a step that sets its `*_LIVE_REQUIRED` guard) and fuzz targets. A step counts only if it would run
  tests: its workflow triggers on push or pull request, it is not `if: false` or `continue-on-error`, and its
  command is not `--no-run`, `-- --list`, `-- --ignored`, or filtered past the gate (a name filter counts only if
  it provably selects the whole gate — the module's path or the one gated function's name). Exceptions go in
  `scripts/test-inventory-exceptions.txt` with a reason; there are none.
- `scripts/test-check-test-inventory.py` — the static check's own **mutation suite** (also in `make check` and
  CI): 49 edits that each leave a test unrun, every one of which the check must fail on — the round-3 ones
  asserting the exact key reported. They are the bypasses three adversarial reviews of #541 found: the first
  version refused *named* test files and was bypassed fourteen ways; the second inventoried targets but not
  library unit tests, read non-feature cfgs as "needs nothing", and counted steps that never run tests (20 of
  38 mutations passed it); the third read only a cfg directly above `#[test]` or a test `mod`, treated any
  `if:` but `false` as running, and flattened shell control flow (about 40 bypasses). The scan is now
  scope- and string-aware, follows `#[path]`, inline modules, binary roots and integration-test submodules,
  reports files it cannot reach, allow-lists step conditions, and reads manifest switches.
- **What the inventory found** (each now runs in CI, all passing): `mycelium-wasm-host`'s `rule_catalogue` and
  `gateway` integration tests, its provisioner's `gateway` tests and the stem's `llm` (`[[serve]]`) tests; the
  skillrunner binary's unit tests; the root and co-op doctests (one, `lock_service`'s `with_lock`, had stopped
  compiling); `mycelium-core`'s `tls`-gated unit tests (crypto-shred erasure, key extraction).
- **Live suites declare a guard.** `mycelium-py/tests/live` and `mycelium-ts/tests/live` read
  `MYCELIUM_LIVE_REQUIRED`; the reason-node suites (`test_reason.py`, `test_typed.py`, the checkpointer's
  `test_checkpointer.py`) read `MYCELIUM_REASON_LIVE_REQUIRED`. Without the variable they skip; with it and
  without the node they fail — so the step that brings up the node sets it, and the inventory checks that it
  does.
- `.github/workflows/ci.yml` — Python runs `pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests`; root
  integration tests run as `--test '*'`. The switch ran eight tests no CI job had ever run (five Python files,
  three Rust integration tests) — all passed. Replacing the named lines removed seven from `ci.yml`.
- `make check-full` is a local subset of CI, not a mirror; CI's coverage is what the inventory proves.
- `.claude/commands/adversarial-review.md` — rule 4's procedure.

## What it does not promise

That nothing is missed: an enumeration is only as complete as the grep behind it, which is why rule 1 asks for
the grep in the PR — so the reviewer of rule 4 can see what was *not* searched.
