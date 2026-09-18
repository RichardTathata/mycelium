# dev/testing — conventions

↑ [dev/](../dev.md) · child pages: [scale-tests.md](scale-tests.md) ·
[cluster-suites.md](cluster-suites.md)

## Run the full feature matrix before pushing

**`make check` is the one-command pre-push gate** — clippy across the feature matrix CI enforces
(feature-matrix + `--no-default-features` + core), in ~3 min with no wasmtime compile. Run it before
every push. `make check-full` adds the test suites + the (slow) wasm-host clippy; run it before a
release or when you have touched wasm-host / a feature-conditional path.

`make check` expands to CI's clippy set; the full CI gate (for reference) is:

```bash
cargo test --lib --features tls,metrics,a2a,llm
cargo clippy --lib --tests -- -D warnings                        # DEFAULT features — the middle of the matrix; an item live only under `tls` is dead here (2026-09-04)
cargo clippy --lib --tests --features tls,metrics,a2a,llm -- -D warnings
cargo clippy --lib --tests --features compliance -- -D warnings   # in make check since 2026-07-22 — WITHOUT this, compliance-gated code went un-linted locally (the guardrails CI job caught it)
cargo test --lib --features compliance          # WS1 RBAC + WS2 audit + WS4 OIDC + WS5 rotation
cargo test --lib --no-default-features --features gateway   # consensus-free embed
cargo test -p mycelium-core                                 # the substrate suite (codec/framing/hlc/store/swim) — RUNS as of 2026-07-11
cargo clippy -p mycelium-core --lib --tests -- -D warnings  # core's own tests are a separate lint scope
cargo clippy --lib --no-default-features -- -D warnings     # minimal embed — catches feature-gated dead code
cargo clippy -p mycelium-wasm-host --all-targets -- -D warnings   # wasm-host embeds mycelium default-features=false
```

**`make check-full` is NOT the whole CI gate — a green local run does not prove CI is green.** The
block above is the *Rust lib + clippy* set. CI **also** runs **live-node / cross-language integration
jobs** that `make check-full` does not: **Reason (v3.0 Tier-3)** and **Python SDK + LangGraph** (spin
up real `reason_node` binaries and drive them over HTTP/Python), the **Food-Rescue coop demo suite**
(`examples/coop/ci_smoke.sh` — 12 multi-node demos), **Blackboard / WASM-host / AFN** smokes, the
**Docker cluster suites**, and the nightly **fuzz** job. These exercise *runtime behaviour across the
network and across crates* that no `cargo test --lib` reaches.

**The lesson (2026-07-16, cost ~25 red commits reported "green").** A pass-1 audit fix salted the
`kv().append` on-disk key with the node id (`log/{stream}/{hlc:016x}/{node}` — core BUG 4). That
changed a **shared core *contract***, and `mycelium-reason::replay` — a **separate crate** with its
**own** `log/`-key parser — read the HLC from the (now wrong) last segment and silently dropped every
trace event. `make check-full` was green the whole time (reason's unit tests covered only formatting,
never a `record→replay` round-trip); only CI's live reason/Python jobs caught it, and it sat red for
~25 commits while each was reported "green" off the local gate alone. Two durable rules fall out:
- **After pushing, check `gh run list` — CI green is the real bar, not `make check-full`.** The local
  gate is necessary, not sufficient.
- **When you change a shared contract** (an on-disk/wire key layout, a KV-prefix convention, an encode
  format), **grep the *whole workspace* for other consumers** — companion crates and integration
  surfaces parse these too, and their parser may not be in any lib test. (The BUG-4 commit fixed the
  `kv_handle`/http log parsers it knew about; reason's was the blind spot.) Fixed by parsing the HLC
  from the salted layout + adding `mycelium-reason` `trace_record_replay_round_trips` (a real
  round-trip gate that fails on the old parser).

**Corollary — a *correct* substrate change can make a timing-sensitive demo flakier.** Same session:
the stricter (correct) `cross_group_quorum` made a 2-node bloc's quorum unanimous, and TLS-signed
consensus drops a vote until the signer's `sys/identity/` gossips in — together they made the coop
`07 · consensus` demo occasionally `Timeout`. The fix is in the **demo's readiness gate** (wait for
identity propagation before proposing + a little ballot headroom), **not** the substrate. A flaky
example demo can be a *correct* substrate change meeting a demo whose startup gate predates it.

**Second corollary — KV-level gates prove *visibility*, not *deliverability*** (the `01 ·
mailbox_llm` flake, 2026-07-21, `1ffe9ea`): a demo can see peers, capabilities, and all
`sys/identity/` keys and *still* drop its first Individual-scoped RPC leg, because each hop's
active forwarding set is published event-driven in the first seconds (the health monitor's first
reconcile is ~`health_check_interval` out). A demo that asserts on an RPC must **warm-up-probe
the exact round-trip it asserts** (retry a throwaway call until one succeeds — a structural gate
at the layer under test), never let the first asserted call double as the path's first exercise.
Any TLS demo doing early Individual-scoped sends needs **both** gates: identity propagation *and*
the round-trip probe. (`catalog` / `provisioning` / `mcp_toolgrowth` make early Individual sends
with neither — same theoretical exposure, so far unexpressed.)

**Feature-gated dead code is a real trap** (bit the diagnostics work, 2026-07-03): an item used
only under `gateway`/`metrics` is *dead* in a `--no-default-features` build (the CI "Gateway-free"
+ "WASM host" jobs run exactly that), and `-D warnings` fails there even though the default and
feature-matrix gates pass. **The fast catcher is `cargo clippy --lib --no-default-features`** (in
`make check`) — it lints the same gateway/metrics-off mycelium lib the slow wasm-host job compiles,
so you rarely need the wasmtime build to catch the trap.

**mycelium-core's suite runs in CI as of 2026-07-11.** Before that it was clippy-*compiled*
(`clippy -p mycelium-core --lib --tests`) but never *run*: `cargo test --lib` tests only the root
`mycelium` package (core is a compiled dependency there, its `#[cfg(test)]` invisible), and there was
no `-p mycelium-core` test job — every *companion* crate had one, core didn't. So the whole substrate
suite (codec/framing/hlc/store/swim, 131 tests), including the wire back-compat tests, was unenforced.
Same class of gap as the decoder mini-fuzz that sat uncaught until M2 Run-20. Now in the CI `Test`
job + `make check-full`.

**Input-fuzz gate (no panic on untrusted input).** The most-repeated bug family across the 2026-07-15
audit series (`docs/analysis/ratings.md`, Runs 50–58) is a *peer- or operator-supplied numeric value*
— a SWIM incarnation, an `is_fresh` interval, an `hlc`/`offset`/rate aggregate, a `fill_ratio`, a live
`TimingIntent` — put through unchecked `+`/`*`/`<<` that overflows → panics (debug/CI; kills the task or
node) or wraps (release; a silent-divergence / limiter-bypass class). **Invariant: arithmetic on any
untrusted (gossiped or config) value must be `saturating`/`checked`/`clamp`ed.** The structural gate is
a suite of proptests that run **under overflow-checks in `cargo test`**, so an unguarded op fails the
build: `store::prop_tests::fuzz_apply_observe_tick_never_panics`,
`config::config_fuzz::fuzz_validate_never_panics`, `capability::tests::fuzz_is_fresh_never_panics`,
`rate::tests::fuzz_reconcile_throttle_never_panics`, `hlc::prop_tests::observe_then_tick_never_wraps`,
`swim_membership::tests::fuzz_apply_never_panics_on_arbitrary_update` — plus the nightly cargo-fuzz
`frame_apply` decode→process target. It is **not yet comprehensive** (rate/opacity/timing internals were
only added in pass 5, after a fifth audit pass found the un-fuzzed `rate.rs` overflow) — the Robustness
dimension in `ratings.md` stays at its floor until a pass validates the sweep by finding nothing.

**Wire back-compat gate.** `codec::tests::decode_wire_v11_agrees_with_v12_on_every_shared_variant`
proves the current decoder reads a **PREV-version (v11)** frame for every shared `WireMessage` variant
— the rolling-upgrade contract (`StateRequest`'s deliberate Merkle-digest change is covered separately
by `decode_wire_v11_downgrades_state_request`). **Corpus discipline** on a `WIRE_VERSION` bump:
regenerate `GOLDENS`, freeze the *outgoing* version's bytes as `V{N}_*` fixtures, add a
`decode_wire_v{N}`, and extend the gate so new code still decodes vN frames. A live two-binary
mixed-version *cluster* test remains a documented (unbuilt) nightly-tier follow-up.

## Golden on-disk fixtures replay in CI (V2, contracts axis)

`tests/fixtures/persistence/<format>/{wal.bin,snapshot.bin,expected.json}` are real files written by
the persistence writer of a released format; `golden_fixture_replays_every_released_on_disk_format`
(`mycelium-core/src/persistence.rs`, `durability_tests`) walks every directory, replays it through the
production `replay` + LWW apply path and checks `expected.json`. **A format change adds a directory;
it never edits one** — every released file must keep replaying. Regenerate a family only with its own
writer, by hand: `cargo test -p mycelium-core regenerate_golden_fixture_fixint_v1 -- --ignored`. The
fixture deliberately holds a WAL-only record *older* than the snapshot watermark (durability
invariant 2) and a tombstone. Record: `docs/design/contracts-receipts.md` §9.

## Coop demos: wasm is opt-in (fast non-wasm builds)

`examples/coop` gates `mycelium-wasm-host` (→ wasmtime/cranelift) behind a `wasm` feature. Four
bins need it (`required-features = ["wasm"]`): `provisioning`, `catalog`, `mcp_toolgrowth`, and
the manual `model_deploy`; the other demos — e.g. `cargo run --bin diagnostics` — build
**without** compiling wasmtime. `ci_smoke.sh` enables `--features wasm` for the three CI demos
that need it; a dev iterating on any non-wasm demo skips the heavy build entirely.

CI additionally gates `tsc --noEmit` + `jest` (mycelium-ts — the node-free `auth.test.ts`; the live suite self-skips), the AFN smoke (pull+push), the coop
smoke, time-boxed fuzz (skipped on PRs), and `cargo audit` (RUSTSEC). **Don't trust a
memorised test count** — the counts grow every PR; run the suites for the live total (the
CLAUDE.md count bullet drifted twice before this rule).

## Toolchain is pinned — bump it deliberately

`rust-toolchain.toml` pins `channel = "1.96.0"` (was floating `stable`), and the CI jobs pin
`dtolnay/rust-toolchain@1.96.0` (the fuzz job stays on `@nightly` by necessity). This exists
because a new stable ships new clippy lints that redden `-D warnings` on unrelated PRs the
moment a runner picks up a newer stable than a dev has locally — it bit twice in one session
(`int_plus_one`, `manual_is_multiple_of`; analysis Runs 28–29). To upgrade: bump the file
**and** the 10 CI `@1.96.0` refs together, in their own PR, after running the full
`-D warnings` matrix on the new version — never let it float again.

## Multi-node consensus tests need listeners everywhere

`cluster_propose`/`consistent_set` compute quorum from live peers; peers without
`start_consensus_listener` never vote and every ballot times out. A test omitting this
passes only via accidental single-node quorum. Pattern (and the peer-ready poll) in
`src/lib_tests.rs::consensus_pair`.

## Structural polling, not fixed sleeps

Assert cluster state with a predicate poll (`poll_until(|| !a.peers().is_empty(), …)`), not
`sleep(300ms)`. A fixed sleep passes by luck on fast machines and hides the race on slow
ones; the structural poll converts a timing race into a deterministic failure.

**Poll the predicate you actually depend on.** A structural poll is only as good as what it
proves, and the easy mistake is gating on the readiest signal rather than the needed one. CI's
reason-node job waited for `/health` and then dispatched through the gateway: `/health` proves the
HTTP server is up, while a secure-profile gateway refuses to route to a provider whose
`sys/caller-context/` marker it cannot see yet (item 7), correctly answering HTTP 412 rather than
running the call as the node. That marker is published lazily and must then gossip, so between
"healthy" and "dispatchable" every call is a 412 — which reddened the Python SDK job on 2026-09-15.
The gate now polls until both markers are visible on the dispatching node. Integration scenario 13
had the sibling version of this (a gossiped role record that survives a restart, so "primary" can be
stale), and the client-deadline section below has the third. Before trusting a readiness poll, ask:
*if this predicate is true and the next line still fails, what did I fail to prove?*

The dual of that rule: **verify a new concurrency regression test against the broken code
before trusting it.** A timing-shaped test can pass on the bug it was written to catch — the
2026-09-02 pool-eviction gate's first draft (two threads + a start barrier) passed on the
pre-fix code because the barrier synchronized both first borrows past the buggy path, and even
a staggered start self-damped under real scheduling. The reliable gate asserted the *rule*
deterministically (drive two `asyncio.new_event_loop()`s and check which entries survive a
miss) rather than hoping timing exposes its violation
([.log entry](../.log/2026-09-02-360-review-fixes.md)).

## Env-var tests serialise on a lock

`apply_env_overrides` reads **all** `GOSSIP_*` vars, so any test that mutates one races
every other env test in parallel threads. Hold `config::tests::env_test_lock()` for the
guard's lifetime (added Run 28 after exactly this race).

## Ports

Use `crate::test_util::alloc_port` (process-unique, bind-verified, confined below the OS
ephemeral floor — PR #110 retired the parallel-suite flake family). Never hardcode.

## The nondeterminism inventory and the coverage map (replay, item 6 PR 1)

[`docs/design/replay-nondeterminism-inventory.md`](../../../design/replay-nondeterminism-inventory.md) names every
production site that depends on something the process did not decide — 22 wall-clock, 71 monotonic-clock, 27
RNG, 46 timer, 15 filesystem sites, every `select!`, the one unseeded shared hasher (`framing.rs` `shard_hasher`),
and the papaya CAS retries — and assigns each an owner: the `mycelium-sim` kernel's seams (two clocks, five named
RNG streams, timers, scheduler, channel fullness, storage with volatile/durable/directory distinctions), **Loom** for
CAS interleavings (D13), fuzz for decoders, Docker suites for real timing. It also lists the sleeps whose duration is
a correctness assumption (the 1 s convergence wait after a lock commit first among them), fixes the choices-trace
and bundle shape (D14: exact reproduction with divergence detection from PR 2), and moves the static forbidden-call
check to PR 3 (D12). A new nondeterminism site on a covered path is admitted only by editing that inventory.

## Replay scenarios A and B (item 6 PRs 4–5, 2026-09-17)

The inventory above named what must be reproducible; these are the first two things reproduced with it.

**Scenario A — the WAL/snapshot race** (`mycelium-core/src/persistence.rs`, PR #241). A controlled schedule
with a `cfg(test)` **merge-removed witness** (`MergeRemoved`, RAII) that *must* fail — the Phase A exit gate.
Replaying it found three bugs in the harness itself and one in the product: replay suppressed writes, so a run
could not read its own; effect requests embedded absolute paths, so no bundle replayed elsewhere; an injected
fault did not prevent the effect because the seam acted before deciding (now `kernel_fs` + `planned_fs`: decide,
then act); and **a snapshot's bytes depended on papaya iteration order** — two nodes with identical logical
state wrote byte-different files. The fix is a canonical sort by key; the test that pins it is
`a_snapshot_is_byte_identical_for_the_same_state_whatever_order_it_was_built_in`.

**Scenario B — scoped mandates** (`src/mandate/scenario_b.rs`, PR #260). The decisive test of
`docs/design/scoped-mandates.md` §8, built as a **schedule sweep** rather than five hand-written cases: the
invariant (*nothing authorized only under a superseded epoch commits*) is asserted after **every step of every
schedule**. The case a hand-written test omits is the one the ADR singles out — revocation with **no**
subsequent write — so the sweep includes idle schedules that knock only much later. It also asserts its own
size, so it cannot quietly shrink.

**The discipline both established, worth reusing: verify a gate by breaking the thing it guards, in both
directions.** Every gate here was checked by planting the failure it exists to catch (a resource that ignores
its installed epoch; a byte-order dependence) *and* by confirming the honest case still passes (a current
mandate still commits; the same state still snapshots identically). A gate that only refuses is satisfied by a
system that does nothing — scenario B's `a_current_mandate_still_commits` and the knowledge gate's positive
controls exist for exactly that reason. Ledger: [history](../history.md) → *item 6 PRs 4–5*.

## Replay scenario C — the interacting governors (item 6 PR 6, 2026-09-18)

`src/control/scenario_c.rs` (test-only): the combined-feedback harness `docs/design/adaptive-stability.md` §5
promised as *replay stage 6, built once, reusing the governors' pure decision functions*. Same shape as B — a
schedule sweep (48 schedules × 2 profiles) with the invariants asserted after every step, a witness, and a size
assertion — over the **shipped** decisions: `TuningGovernor::gate_at`/`acted_at`, `opacity_state_for` →
`opacity_transition` → `spaced_transition`, `membership_governor::{decide, classify}` under `control::decide`,
with their real spacing, settling and hysteresis. What is *modelled* is the plant that couples them: this node's
share of a fleet inbound, halved while opaque, minus what the writer drains; an advisor recommending a writer
depth from group size and load. The objectives are numbers — two **releases** never closer than 300 ms, two
knob changes never closer than 200 ms, at rest 15 ticks after the last disturbance, no routine scale-down on a
stale view under an enforcing profile — so the witness (every breaker off) can violate them, and does: release
flaps and knob chatter.

**Three lessons.** *State the objective on what the breaker governs:* the first sweep said "any two transitions"
and failed under the shipped breakers on a release re-shed 100 ms later — the decisive rule working, not a
flap. *A first-only violation report masks:* the witness said "no flap" while the same schedules chattered.
*A plant that is not caught is a finding:* removing the hysteresis in shipped code did **not** fail the sweep —
the 1 s release spacing alone bounds the release rate, so a lost hysteresis only changes how often a proposal is
held; recorded as what the sweep does not prove. The tuning plant (no spacing) was caught at once. **Not
shown:** the ADR's sharper sentence — loops oscillating together while each is stable alone — the witness
removes every breaker at once. Ledger: [history](../history.md) → *item 6 PR 6*. *Since the profile ladder was wired through every governor
(item 4 §7, 2026-09-18):* the "settles" sweep runs under the enforcing profiles, where the breakers act; under
`Legacy` the same breakers are not consulted and the schedules flap; under `Observe` they flap while counting
every hold not made — `Legacy` is the ladder's own witness.

## The replay corpus (item 6 PR 7, 2026-09-18)

`mycelium-core/tests/replay-corpus/<name>/` holds checked-in bundles; the first is scenario A (thirteen
effects). Two tests in `persistence.rs`'s `durability_tests` own it. **The recorder**,
`record_scenario_a_into_the_corpus`, runs only under `MYCELIUM_RECORD_CORPUS=1` and rewrites the entry from a
fresh recording — the same code records the same bytes, so a re-record is reviewed as a diff of `choices.trace`,
the way the seams baseline is regenerated on purpose. **The gate**,
`the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording`, runs in every `--features sim`
suite (CI's `mycelium-core --features sim` job): the committed schedule must replay on *this* machine without
divergence, the bundle must still name the witness toggle this build knows, and a fresh recording must ask for
the same effects in the same order. That last check is what makes the corpus a gate rather than a souvenir: a
new effect or changed bytes in the snapshot path fails the suite, with the differing line printed from both
sides, until someone re-records deliberately. Verified by tampering one content hash in the committed trace (the
gate fails on that effect) and re-recording (identical bytes). Not built: the minimiser and a replay binary. Log:
[`.log/2026-09-18-item6-pr7-replay-corpus.md`](../.log/2026-09-18-item6-pr7-replay-corpus.md).

## Loom: permutation model-checking of the atomic patterns

Deterministic unit tests and stress loops surface a lock-free bug only by luck — the buggy
interleaving is a narrow window. The `loom-spike` crate model-checks the project's recurring
"act on a stale read" bug family (the one the calibration ledger keeps re-recording) by
exhaustively exploring **every** thread interleaving and weak-memory execution, turning a
probabilistic race into a deterministic, reproducible failure with a printed schedule.

**Why a sibling crate.** `--cfg loom` is global to a build, and tokio gates its `net`/`fs`
modules behind `#![cfg(not(loom))]`, so any tokio-linked crate (`mycelium-core`, `mycelium`)
fails to compile under it. `loom-spike` is deliberately tokio-/papaya-/mycelium-free so loom
can actually run; the models re-implement the pattern with loom's instrumented atomics
(production code is intentionally NOT retrofitted — it isn't loom-instrumentable). Without
`--cfg loom` the crate compiles to an empty crate with zero dependencies, so it is invisible
to a normal `cargo build`/`test`.

**Patterns modelled** (`loom-spike/tests/`, each `#![cfg(loom)]`, each citing the real code):
- `once_guard.rs` — the exactly-once `AtomicBool` init guard (`FilterOpacityRegistry::spawned`
  `swap`/CAS; `capability_handle.rs:265`).
- `unique_id.rs` — the monotonic `fetch_add` unique-ID allocator (`next_pred_watcher_id`;
  `mycelium-core/src/ops.rs:237`, `kv_handle.rs:183`).
- `publish.rs` — publish-then-observe release/acquire (`soft_state_advertised.store(true,
  Release)` at `kv_persist.rs:57` paired with the `Acquire` load in `is_ready()`,
  `introspect.rs:125`). loom's weak-memory model **does** surface the all-`Relaxed` variant:
  the reader sees the flag set while reading stale data.

**Run it:**

```bash
RUSTFLAGS="--cfg loom" cargo test -p loom-spike --release   # CI's `loom:` job (LOOM_MAX_PREEMPTIONS: 3)
```

This runs only the CORRECT tests, which pass — so the `loom:` CI job is green. Each model also
ships a `#[ignore]`d **broken twin** (a `load`-then-`store` allocator, an all-`Relaxed`
publish) kept as **executable proof loom catches the race**: run one with `-- --ignored` and it
FAILS with the offending schedule. `#[ignore]` keeps the default run green while the bug-catch
stays one command away.

## The CI flake tier (structural, Run-38 floor fix)

Socket-binding / multi-node suites run in CI through `scripts/ci-retest.sh`, not bare
`cargo test`: on failure the wrapper re-runs **only the failed tests, individually, once**. A
test that fails twice is a real failure and reds the build; a test that passes on isolated
retry keeps the build green **but emits a loud per-test flake annotation + step-summary line**.
The policy that makes this safe against the Run-37 masking failure mode: **a flake annotation
is a bug report** — recurring annotations get a root-cause dig (the wiki port race and the
opacity shed bug were both found that way), and "fixing" a flake by widening a timeout is
forbidden. Deterministic unit gates stay on bare `cargo test`. This is the class-level
prevention Run 37 asked for: a wall-clock flake can no longer red main *or* hide silently.

Companion integration tests now reach the same allocator: `alloc_port` is exposed under the
core's `test-util` cargo feature (Run-39 floor fix), and every companion with a real-agent
`tests/` suite pulls `mycelium = { path = "..", features = ["test-util"] }` as a **dev-dependency**
and calls `mycelium::test_util::alloc_port()` in place of its old `free_port()`. That is the
class-level prevention — the bind-`:0`-read-drop idiom is gone from the `tests/` surface, so no
companion re-opens the TOCTOU window (only the `examples/` `free_port` remain, a follow-up).

The bind retry stays as defense-in-depth for the residual case — an agent under test binds a port
`alloc_port` returned but a foreign process grabbed it first: **retry the bind, never bare-`unwrap`
it**. The old bind-`:0`-read-drop idiom (`free_port()`) opened a TOCTOU window against parallel
test binaries (`AddrInUse` flaked `mycelium-wiki/tests/failover.rs` in CI, 2026-07-07). Retry at
the granularity the topology
forces: per-agent with fresh ports when nodes join one at a time (the wasm-host tests'
16-attempt loop), **per-pair** when mutual bootstrap fixes both ports before either agent
starts (`start_pair()` in the wiki tests — shut the half-started survivor down before
re-attempting, and shut a discarded `Wiki` down explicitly, the Run-32 task-leak lesson).

## A client deadline below the server's budget is a defect, not a flake

"Never fix a flake by widening a timeout" (above) has a sharp exception that is easy to
misapply it to, and the 2026-09-15 scenario-13 failures are the worked example. Ask which of
two things the deadline is:

- **A tolerance for slowness.** The operation would have answered correctly, just later than
  the test was willing to wait. Widening it hides a real latency regression. Forbidden.
- **The window in which the answer is allowed to arrive at all.** Set below the server's own
  budget, the assertion is *unobservable by construction*: the client can only ever report that
  it gave up first, never the verdict it was written to check. Raising it does not weaken the
  test — it is what makes the test a test.

Scenario 13 was the second kind. A tuple put costs up to 16 s server-side (`resolve_primary_blocking`
waits 3 × `cap_refresh`, then one `rpc_call` at 10 s; a take's RPC deadline is `timeout_secs + 5`),
and the scenario allowed the client 5 s for a put and exactly 10 for a take. It passed for weeks
because the first attempt usually succeeds; it failed twice in three runs once scenarios 03/04/05
started leaving an outbound writer in reconnect backoff, where a dropped request frame makes the
caller wait out the full RPC deadline. **Before adjusting any deadline, write down the callee's
budget and compare.** If the client's is smaller, that is the bug.

Two diagnostic rules fell out of the same dig, both cheap and both worth copying:

- **Never pipe `curl -sf` into a parser in an assertion.** It reports a lost request as
  `jq exited 28` — no iteration, no status code, no body. Capture `%{http_code}` and the body and
  say which iteration failed and what the server actually answered. The take loop learned this in
  #150; the put loop beside it did not, and stayed blind for a year.
- **Truncate the `-o` file before every request.** curl leaves it untouched when no response
  arrives, so the failure report prints the *previous* iteration's body. The failing run reported
  `take #9 ... body='{"id":8,…}'`, which reads as a wrong-id bug and is actually a timeout — a
  diagnostic that invents a second, fictional defect is worse than none.

The deeper rhyme with the contracts axis: each of these is a **record claiming more than the
underlying event established** — curl's impatience reported as the server's answer, one
iteration's body reported as another's. Same defect class as the receipts work, in the test
harness rather than the product ([contracts-receipts](../../../design/contracts-receipts.md)).
