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
3. **CI collects tests by discovery, not by list.** A test runner is pointed at a directory, never at named
   files; a test that needs a live node marks itself to skip when no node is configured. A new test file runs
   the day it lands.
4. **Independent adversarial review after each PR.** Before merge, an agent other than the author — with no
   access to the author's reasoning, given only the diff and the repository — enumerates the surfaces the
   change touches and tries to break them: siblings, other doors, the plan row's promises, literal execution of
   every instruction the PR adds. `/adversarial-review` is the procedure; its findings are fixed or answered in
   the PR before merge.

## Where it is enforced

- `CLAUDE.md` § Verification policy — every session reads it.
- `.github/pull_request_template.md` — the enumeration, the plan-row evidence and the review link are fields.
- `RELEASING.md` § 5b — no release while a delivery row marked merged lacks its evidence.
- `scripts/check-test-discovery.sh` (in `make check` and CI) — fails if a workflow names a test file or test
  binary without a `# discovery-exception: <reason>` on the line above (rule 3 made mechanical). Its first run
  flagged four; one was replaced by discovery, three carry their reason.
- `.github/workflows/ci.yml` — Python runs `pytest langgraph-checkpoint-mycelium/tests mycelium-py/tests`; the live
  gateway suite skips itself without `MYCELIUM_TEST_HOST`; root integration tests run as `--test '*'`. The switch
  ran eight tests no CI job had ever run (five Python files, three Rust integration tests) — all passed.
- `.claude/commands/adversarial-review.md` — rule 4's procedure.

## What it does not promise

That nothing is missed: an enumeration is only as complete as the grep behind it, which is why rule 1 asks for
the grep in the PR — so the reviewer of rule 4 can see what was *not* searched.
