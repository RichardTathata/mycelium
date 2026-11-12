Run the **independent adversarial review** of a pull request — rule 4 of the verification policy, and the reviewer's half of rule 5
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
   (`src/agent/http/`, one file per route family, companions' routers), both SDKs (`mycelium-ts/src`, `mycelium-py/src`), config
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
7. **The feature page** (rule 5). If the diff adds or changes a feature, find its entry in
   `docs/features/registry.toml` and its page under `docs/features/`. The obligation: the PR adds or updates both,
   with two exceptions it must state — why the change leaves the page true, or that the entry is at
   `page = "todo"` and predates the close of `docs/plans/feature-pages.md`'s row F3 (after F3 closes, a new feature
   lands with its page in the same PR). A missing update without a stated exception is a finding, and so is a
   stated exception that does not hold. Read the page against the code: a *What it is*, *Turn it on* or *What it
   guarantees* the change has made untrue is a finding. `scripts/check-feature-pages.py` covers only ids from its
   seven sources (`SOURCES` in the script: glossary concepts, guarantee and rule catalogue ids, unit-file keys, Cargo
   features, companion-registered guarantees, workspace crates) and checks only that a page's links, settings and
   tests exist. **A configuration field, an HTTP route or an SDK verb has no id**, so you are the only check: find
   the feature it belongs to through its concept or crate id in the registry and check that page covers it, or
   report that no page does. Before F3 closes, a field, route or verb whose feature is at `page = "todo"` or
   belongs to no feature is reported but is not a blocking finding; after F3 closes, it is a finding.

8. **Records read from the gossip KV** (the owner-signed rule, `docs/wiki/dev/owner-signed-records.md`). Any
   admitted member can write or tombstone any key. For every KV record the change reads to make a decision —
   authority, admission, cancellation, retention, audit — give the grep that found the reads, and say which of
   these holds, citing the code: (a) the record carries a signature under the key of the principal its key path
   names, or, where the path names no principal or names the record's subject, of the authority the reader is
   configured to trust for it; (b) the reader verifies it, that the body's identifying fields equal the key's,
   and that it is current (a sequence, issue time or supersession rule checked at read), before use; (c) a record
   that fails is treated as absent, absence fails closed — or to a fallback the change names — and the failure is
   counted; (d) a test writes the record as another member would (forged, replayed, tombstoned) and shows the
   reader is not moved. Ask of each read what the reader does when the record is *deleted*: an absence that
   permits is a finding. The page's inventory gives each record a **Status**. A decision read with (a)–(d) not
   all holding is a finding unless the change adds or updates the record's row with the Status that is true of
   it — **Partly** or **Does not hold**, naming the failing condition — or shows the record is outside the rule:
   **Outside: availability** (the worst forgery is a refusal or a delay), **Outside: no decision**, or
   **Outside: bounded soft state** (the worst case detected and bounded, the detector and the bound cited in the
   row). Soft state with a bound and no detector — or one detected only at the owner while another node acts on
   it — is **Bounded, not detected**, which is a finding until the change cites the detector. A row at **Meets** without the verifying `file:line` and the forged-record test is a finding.

## The report and what to do with it

The reviewer returns findings ranked by severity, each with `file:line`, a concrete failure scenario, and
whether it is a defect or a question. Post them on the PR (`gh pr comment`) under **Adversarial review**,
then fix each defect in the PR — with a test that fails first — or answer it in the thread. Merge only when
every finding is fixed or answered. A finding that reveals a gap a *prior* review should have caught goes in
the PR's description as well, so the next author sees it.
