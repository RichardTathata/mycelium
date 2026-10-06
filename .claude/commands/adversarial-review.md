Run the **independent adversarial review** of a pull request — rule 4 of the verification policy
(`CLAUDE.md` § Verification policy; `docs/wiki/dev/testing/verification-policy.md`). The argument is a PR
number or a branch; with none, the current branch against `main`.

**The point.** Every check the author ran was derived from the change's description, so it confirms what the
author thought of. This review is derived from the **repository**: it enumerates the surfaces the change
touches and tries to break them. A review that reads the PR description and agrees with it has not happened.

## Independence (the whole rule)

Dispatch the review to a **fresh agent** (`Agent`, `general-purpose`) — never review your own change in the
session that wrote it. Give it only:

- the PR number or branch, the worktree path, and read-only instructions (no edits, no commits, no pushes);
- the plan row the PR claims to advance, **quoted from the plan file**, not paraphrased;
- this file's checks, verbatim.

Do **not** give it the PR body, the commit messages' reasoning, or your own account of what the change does —
it reads the diff and the code. Tell it its final message *is* the report.

## The checks (give these to the reviewer verbatim)

1. **The invariant and its doors.** From the diff, name every invariant the change establishes or alters (a
   bound, a refusal, a field name, a wire shape, a durability or ordering rule). For each, enumerate *from the
   code* every entry point that reads or writes the same thing — Rust setters and constructors, HTTP routes
   (`src/agent/http.rs`, companions' routers), both SDKs (`mycelium-ts/src`, `mycelium-py/src`), config
   loading and env overrides (`mycelium-core/src/config.rs`), intents and governors, examples. Give the grep for
   each. Report every entry point the change did not cover and whether it now disagrees with the ones it did.
2. **Siblings.** For each function the diff changes, find functions of the same shape (same handler family,
   same request builder, the same mistake one function up or down) and check them for the same defect.
3. **The plan row, promise by promise.** Quote each promise of the row the PR advances. For each, cite the
   code or test that delivers it, or write *not built*. A row the PR marks merged with a promise *not built*
   is a finding.
4. **Tests that cannot fail.** For each new test: could it pass against the unfixed code (a mock that accepts
   any input, an assertion on a value the test itself supplies, a live test that skips silently)? Is it run by
   CI — under a directory a CI step runs, not missing from a list?
5. **Instructions run literally.** Every command, config snippet, code snippet or `curl` the PR adds to a doc:
   run it, or trace it to code that makes it work. One that fails as written is a finding.
6. **Break it.** Try at least three concrete inputs or sequences the author did not test — boundary values,
   the empty case, a concurrent or repeated call, the build without the feature, a restart in the middle — and
   say what happens, from the code or by running it.

## The report and what to do with it

The reviewer returns findings ranked by severity, each with `file:line`, a concrete failure scenario, and
whether it is a defect or a question. Post them on the PR (`gh pr comment`) under **Adversarial review**,
then fix each defect in the PR — with a test that fails first — or answer it in the thread. Merge only when
every finding is fixed or answered. A finding that reveals a gap a *prior* review should have caught goes in
the PR's description as well, so the next author sees it.
