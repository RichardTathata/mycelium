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
grep -c "version = \"$NEW\"" Cargo.lock          # expect ${#TRAIN[@]}
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
(a dated release section) · `CLAUDE.md` (the on-ramp line). Keep the *wire version* claim honest
(state whether it changed — a wrong wire-compat note misleads upgraders).

## 7. Commit, tag, push

```bash
git add -A && git commit -m "release: vNEW"
git tag -a vNEW -m "vNEW — YYYY-MM-DD ..."      # annotated; summarize the highlights + wire status
git push origin main && git push origin vNEW
```

## Publishing

Releases are **git tags only** — the crates are not published to crates.io (`cargo publish` is
irreversible and the crates are not currently intended to be public). If that changes, publish in
dependency order (`mycelium-core` first, then `mycelium`, then companions), each needing a
`CARGO_REGISTRY_TOKEN` — a separate, deliberate decision.
