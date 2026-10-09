# Contributing to Mycelium

## Contributor License Agreement — required before any PR is merged

Mycelium is dual-licensed: AGPL-3.0 for open-source use, and a commercial license for
proprietary use. To keep dual licensing viable, the project needs to own the copyright to
all contributions.

**Before your first pull request is merged, you must sign the CLA.**

The CLA is an inbound = outbound agreement: you grant the project a perpetual,
irrevocable, royalty-free licence to use, reproduce, modify, sublicense, and distribute
your contribution under any licence (including commercial licences). You retain your own
copyright. See [`CLA.md`](CLA.md) for the full text.

A CLA bot will prompt you automatically when you open a PR. PRs from contributors who
have not signed will not be merged.

## What to contribute

**Welcome:**
- Bug fixes with a clear reproduction case
- Performance improvements with benchmarks
- Documentation fixes and clarifications
- New examples that demonstrate existing features
- Test coverage improvements

**Out of scope (already planned):**
The following items are tracked in the active plan files and will be implemented by the
core team — please do not open PRs for these without coordinating first:

- Signal reorder buffer (`plan_signal_reorder_buffer.md`)
- Watcher scalability C2 residual (`plan_watcher_scalability_c2.md`)
- `--features compliance` (compliance and audit features)

See [`ROADMAP.md`](ROADMAP.md) for the full feature history and what is deferred.

## How to contribute

1. Fork the repository and create a branch from `main`
2. Make your changes; see the code style section below
3. Run the full test matrix (see below)
4. Open a pull request — the CLA bot will prompt you if this is your first contribution
5. Update `CHANGELOG.md` under `[Unreleased]` with what changed and why

Keep PRs focused. A PR that fixes one bug is easier to review than one that fixes three.
For new features or architectural changes, open an issue first so the design can be
discussed before you write a large amount of code.

## Building

```sh
# Core library (no optional features)
cargo build --lib

# Full feature set — what CI runs
cargo build --lib --features tls,metrics,a2a,llm

# Gateway-free embedded build — must always compile cleanly
cargo build --lib --no-default-features

# CLI binary
cargo build --bin mycelium
```

The toolchain is pinned in `rust-toolchain.toml` (1.96.0, in lockstep with CI; the crate's `rust-version` floor is
1.89). No nightly features are used outside the fuzz job.

## Testing

Run the gate before pushing. Do not trust a remembered test count — run the suite and read its total.

```sh
# The pre-push gate: clippy across the feature matrix CI enforces (incl. --no-default-features, which
# catches feature-gated dead code), the examples build, and the repository's checks — sim seams, test
# inventory, examples matrix, KV namespaces, the wiki mutation fence, positioning, materials. No wasmtime
make check

# The same plus the test suites and the WASM host's clippy
make check-full

# Integration scenarios on a 4-node Docker cluster (requires Docker)
make test
```

The Makefile's `check` and `check-full` targets are the canonical command lists; read them rather than a
copy. **Prerequisites beyond the Rust toolchain:** `make check` needs **Python ≥ 3.11** (the test-inventory
check imports `tomllib`; PyYAML is fetched into `target/check-venv` if missing). `make check-full` also runs the
Python suites, so first, in a virtualenv:
`pip install -e './mycelium-py[typed]' -e langgraph-checkpoint-mycelium pytest pytest-asyncio langgraph httpx-sse`
(CI's own line).

The integration suite requires Docker. The first run builds images from scratch;
subsequent runs reuse the layer cache and are fast.

### Verification policy

A change is verified against the surface it touches, not its own description — four rules, with the
evidence that produced them, in [`docs/wiki/dev/testing/verification-policy.md`](docs/wiki/dev/testing/verification-policy.md):
enumerate every entry point to an invariant before fixing it; close a plan row only on quoted evidence;
CI discovers tests rather than listing them; an independent adversarial review before merge. The PR
template asks for each.

### Adding a test or an example so CI runs it

CI collects tests by discovery, and `make check` fails on either omission below — `check-test-inventory.py`
for tests, `check-example-matrix.py` for examples.

- **A Rust integration test** goes in `tests/` (or the crate's own `tests/`); CI runs `--test '*'`, so never
  add it to a list.
- **A test that needs `gateway` or `tls` off** goes in `mycelium-gateway-free-tests` or
  `mycelium-tls-free-tests`: the root crate's dev-dependencies turn both features back on in every other test
  build, so a `#[cfg(not(feature = "gateway"))]` test in the root crate never runs.
- **A test that needs a live node** skips itself without one and reads its `*_LIVE_REQUIRED` guard
  (`MYCELIUM_LIVE_REQUIRED`, `MYCELIUM_REASON_LIVE_REQUIRED`); set the guard in the CI step that starts the
  node, so a missing node fails there instead of skipping.
- **A script-style suite** answers `--list` with one `@@case-list@@ <suite>::<case>` line per case and prints
  `@@case@@ <suite>::<case>` as each case starts — the `test-coverage` job reads those lines.
- **An example CI runs** is written `scripts/example-case.sh cargo run --example NAME` in the workflow, and its
  CI cell in [`examples/README.md`](examples/README.md) is ✓ (an every-change workflow), ✓ᵖ
  (`cluster-suites.yml`, path-filtered) or · (not run).
- **A deliberate exclusion** goes in `scripts/test-coverage-exceptions.txt`, with its reason.

### Test conventions

**Structural polling, not fixed sleeps.** Use `for _ in 0..40 { if condition { break; } sleep(50ms) }` rather than `sleep(500ms)`. A structural assertion fails deterministically and points to the root cause.

**Multi-node consensus tests need listeners on every node.** `cluster_propose` computes
`quorum = ⌊(peers+1)/2⌋ + 1`. If peer nodes have no `ConsensusListener`, ballots time
out. The required pattern (a listener on every node, a structural peer-ready poll) is in
[the testing page](docs/wiki/dev/testing/testing.md#multi-node-consensus-tests-need-listeners-everywhere).

**Unit tests run in-process.** No Docker, no network. A unit test that spawns real TCP
connections is an integration test and belongs in `tests/`.

## Code style

**No comments that explain what code does** — well-named identifiers already do that.
Write a comment only when the *why* is non-obvious: a hidden constraint, a subtle
invariant, or a workaround for a specific bug. No multi-line comment blocks, no
`// ---` separators, no TODO comments in submitted code (open an issue instead).

**No `unwrap()` in production code.** Use `.expect("infallible: <reason>")` where the
invariant is genuinely infallible, or return an error with `?`. The reason string must
explain *why* the branch cannot be reached.

**No unsafe code.** The crate has `#![deny(unsafe_code)]`.

**Error types.** Use the domain-specific error type for each handle (`ConsistencyError`,
`RpcError`, etc.). Do not wrap everything in `GossipError`. Use structured variants
rather than `GossipError::Network(format!("..."))` for typed conditions — callers need
to match on kind without parsing strings.

**Atomics.** Follow the [memory-ordering policy](docs/wiki/dev/concurrency/lock-free-and-atomics.md#memory-ordering-policy-for-atomics).
Do not use `SeqCst` unless you can justify it with a concrete data race.

## Layer rules

Mycelium is built in three layers. Each layer writes to its own key prefix in the gossip
KV store — the namespace table in `src/lib.rs` is canon; this table summarises it. Respect this separation:

| Layer | Key prefix | Notes |
|-------|-----------|-------|
| I — KV | raw user keys | Substrate; no signal mesh, no consensus |
| II — Signals | none of its own | Signals travel as wire messages, not KV keys; reads Layer I |
| III — Consensus | `consensus/` | Builds on both layers |
| Capability | `cap/`, `gcap/`, `sys/load/` | Reads and writes its own prefix only |

A change that writes a `cap/` key from Layer I code is a layer violation and will not
be merged.

## Wire protocol

Changing the wire format requires a version bump. See the rolling-upgrade policy in
`mycelium-core/src/framing.rs` (the `WIRE_VERSION` block comment). The steps:

1. Add a `WireMessageVN` struct with the *old* field layout.
2. Increment `WIRE_VERSION`; set `PREV_WIRE_VERSION` to the old value.
3. Implement `From<WireMessageVN>` → `WireMessage` with sensible defaults for new fields.
4. Test that a v(N-1) frame decodes correctly via the shim path.

Do not change field order or types in existing `WireMessage` variants without a version
bump — bincode fixed-int encoding is not self-describing.

## Scale tests (optional)

```sh
make test-scale               # 100-node cluster
make test-scale-resilience    # 20-node resilience + late-joiner
```

These are slow (~10 min cold) and RAM-intensive. Run them for changes that touch
`mycelium-core/src/connection.rs`, `mycelium-core/src/writer.rs`, `mycelium-core/src/store.rs`, or
`src/agent/tasks.rs`.

## Licensing

By contributing you agree that your contribution is licensed under AGPL-3.0 (for
open-source use) and under the commercial license (for proprietary use), as described in
the CLA. The project's `LICENSE` file contains the full AGPL-3.0 text.

If you have questions about the licensing model, contact the maintainers directly —
the detailed commercial / go-to-market strategy is kept in a private internal
document, not in the public repository.

## Code of Conduct

This project follows the [Contributor Covenant v2.1](CODE_OF_CONDUCT.md). Please read it
before participating.
