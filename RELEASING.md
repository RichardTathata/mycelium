# Releasing Mycelium

The release process is **manual and deliberate** (no `cargo publish` — releases are git tags;
see *Publishing* below). This runbook is the contract; follow it top to bottom.

## 1. Decide the version (SemVer)

- **PATCH** (`x.y.Z`) — backwards-compatible bug fixes only.
- **MINOR** (`x.Y.0`) — backwards-compatible new public API / features. Wire version *may* bump
  as long as `PREV_WIRE_VERSION` still covers the previous release (rolling upgrade holds).
- **MAJOR** (`X.0.0`) — a breaking change to the public API **or** a wire break (a node at the new
  version can no longer talk to the previous release).

Read `CHANGELOG.md` `[Unreleased]`: the *highest* change class there sets the bump.

## 2. Pre-release gate

```bash
make check-full        # clippy feature-matrix + wasm-host clippy + the test suites
```
Must be green. (`make check` is the fast pre-push gate; a *release* runs `check-full`.)

## 2b. CI on the branch you are releasing **from**

```bash
gh run list --branch main --limit 5          # the most recent push run must be green
```

**`make check-full` green is not the same as `main` green, and neither is a green PR.** Some jobs
run only on `push` — the **fuzz** job is `main`-only by design (it is time-boxed and slow), so a PR
can show every check passing while the very same commit fails on `main` minutes later.

That is not hypothetical: **v2.11.0 was tagged on a `main` whose fuzz job had already failed**, and
the failure was a real defect in the `/a2a` credential parser that §12.6's own target had found
(`CHANGELOG.md`, 2.11.1). `check-full` was green, the wire gate was green, the PR was green — and
none of them ran the job that mattered. Look at the branch, not at what fed it.

If the latest `main` run is still in progress, **wait for it**. A release is the one place where
"probably fine" costs a tag that cannot be honestly rewritten.

## 3. Rolling-upgrade check (wire compatibility)

The claim every release makes — *"backwards-compatible rolling upgrade"* — is backed by a
**deterministic gate**; run it every release:

```bash
# Two commands, not one: `cargo test` takes a SINGLE filter, so the old one-liner
# (`cargo test -p mycelium-core rolling prev_wire_version`) errored with
# "unexpected argument" and had never run — found cutting 2.10.0.
cargo test -p mycelium-core rolling             # 3 rolling_upgrade_* tests
cargo test -p mycelium-core prev_wire_version   # 2 prev_wire_version_* / read_frame tests
```
Expect **3** and **2**. If either prints `0 passed`, the filter has drifted from the test names —
list them with `-- --list` before trusting a green run.
These prove, against real `PREV_WIRE_VERSION`-encoded bytes: `read_frame` accepts both versions
within the window (and rejects older), a `PREV`-version KV write **decodes and converges** into a
current store, forwarding **re-encodes at `WIRE_VERSION`**, and the v(prev)↔v(cur) KV round-trip is
lossless. (`mycelium-core/src/framing.rs` `tests`.)

**If you bumped `WIRE_VERSION`,** also do the live two-binary check (the deterministic gate covers
the codec/apply boundary; this covers real TCP + gossip + anti-entropy end-to-end):

```bash
PREV=<last-commit-at-the-previous-wire-version>          # e.g. the parent of the WIRE_VERSION-bump commit
git worktree add /tmp/myc-prev "$PREV"
( cd /tmp/myc-prev && CARGO_TARGET_DIR=/tmp/myc-prev-target cargo build --bin mycelium )
cargo build --bin mycelium
# Node A = previous wire, node B = current, peered:
/tmp/myc-prev-target/debug/mycelium --port 8091 &                                   # A (prev)
target/debug/mycelium --port 8092 --http-port 9092 --peers 127.0.0.1:8091 &         # B (cur)
# Write on A (its stdin REPL: `set k v`), then confirm it converged on B
# (B's gateway: curl 'http://127.0.0.1:9092/gateway/kv?key=k', or B's node log).
# Then reverse. Tear the nodes down; `git worktree remove /tmp/myc-prev`.
```
> Note: a *much older* previous binary may predate the HTTP gateway (`--http-port`) — read that
> node via its stdout/logs or the interactive REPL. This live check is a **manual** pre-release
> step, not CI: it builds an old commit (which can drift) and drives an interactive REPL — the
> deterministic gate above is the automated regression guard.

## 4. Bump versions

Bump every workspace crate currently on the shared train (2.x) — **not** the independently-versioned
companions (`mycelium-reason`, `mycelium-guardrails` on their own `0.x` track).

**Derive the list, never hard-code it.** This step used to name seven crates explicitly; the v3 axis
added `mycelium-commitment`, `mycelium-effects` and `mycelium-sim`, and the hard-coded loop would
have left all three on the old version while the `expect 7` check passed — a green release step that
had stopped covering what it named (found cutting 2.10.0). Ask the tree instead:

```bash
OLD=2.9.1; NEW=2.10.0
# every crate on the shared train, derived
mapfile -t TRAIN < <(grep -rln "^version = \"$OLD\"" --include=Cargo.toml . | grep -v '^./target')
printf '%s\n' "${TRAIN[@]}"                      # eyeball it: 10 at 2.10.0
for f in "${TRAIN[@]}"; do
  perl -i -pe "s/^version = \"$OLD\"/version = \"$NEW\"/" "$f"
done
cargo metadata --format-version 1 >/dev/null     # refresh Cargo.lock
# Count only OUR crates. A bare `grep -c "version = \"$NEW\"" Cargo.lock` counts every package at
# that version, including third-party ones: at 2.12.0 `ipnet` is also 2.12.0, so the naive check
# reported 11 for 10 crates and looked like a double-count. It happened not to collide at 2.10.0 or
# 2.11.x, which is exactly how a check like this survives to mislead someone later.
grep -B3 "version = \"$NEW\"" Cargo.lock | grep -c '^name = "mycelium'   # expect ${#TRAIN[@]}
```
Then verify nothing is left behind — a remaining hit is an inter-crate dep spec that must move too:

```bash
grep -rn "\"$OLD\"" --include=Cargo.toml . | grep -v '^./target'   # expect no output
```

## 5. CHANGELOG

Cut `## [Unreleased]` → `## [NEW] — YYYY-MM-DD` (note wire version + whether it changed), consolidate
duplicate `### Added` blocks, and open a fresh empty `## [Unreleased]` at the top.

## 6. Update the version-state anchors

`ROADMAP.md` (Status line) · `docs/wiki/wiki.md` (Version state) · `docs/wiki/dev/history.md`
(a dated release section) · `CLAUDE.md` (the on-ramp line) · **`docs/guide/building-on-mycelium.md`
(the install snippet's `tag = "…"` pins** — an integrator copies that block, and it was one release
behind within hours of v2.15.1 because nothing in this step named it; wiki-lint 2026-09-26). Keep
the *wire version* claim honest (state whether it changed — a wrong wire-compat note misleads
upgraders).

## 7. Commit, tag, push

```bash
git add -A && git commit -m "release: vNEW"
git tag -a vNEW -m "vNEW — YYYY-MM-DD ..."      # annotated; summarize the highlights + wire status
git push origin main && git push origin vNEW
```

## 8. Publish the GitHub Release

A tag is not an announcement. The Releases page is what a reader who watches releases rather than
commits actually sees, and **it silently stopped being updated after v2.4.4** — so from 2026-09-12
to 2026-09-23 the page said *"Latest: v2.4.4"* while eleven tags shipped behind it, **four of them
carrying security fixes**. Nobody noticed because nothing fails when this step is skipped; the tag
is there, CI is green, and only the page is wrong. Backfilled 2026-09-23.

```bash
NEW=2.13.0
title=$(git tag -l --format='%(contents:subject)' "v$NEW")
git tag -l --format='%(contents:body)' "v$NEW" > /tmp/notes.md
gh release create "v$NEW" --title "$title" --notes-file /tmp/notes.md --verify-tag --latest
```

The **annotated tag message is the release body** — it was written for this, at the moment the
release was cut, and re-summarising it later produces a second account that can drift from the
first. `--verify-tag` refuses if the tag is not on the remote, which catches the "tagged but never
pushed" mistake.

> **`--notes-from-tag` is silently incompatible with `--repo`.** Together they print a usage error
> and exit 1 — and in a `for` loop over tags that reads as *no output*, which looks exactly like
> success. Either run from inside the repository (as above) or write the notes to a file. Found
> while backfilling, by checking the Releases page rather than the loop's exit status.

**A security release also needs the check an operator can run**, in the first paragraph of the body
rather than the notes' middle — for v2.13.0, *"an unauthenticated `GET /gateway/kv/keys` answers 200
today and 401 after this release"*. Someone deciding whether they are affected should not have to
read a changelog to find out.

## Publishing

Releases are **git tags only** — the crates are not published to crates.io (`cargo publish` is
irreversible and the crates are not currently intended to be public). If that changes, publish in
dependency order (`mycelium-core` first, then `mycelium`, then companions), each needing a
`CARGO_REGISTRY_TOKEN` — a separate, deliberate decision.
