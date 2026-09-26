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

**Trust-edge fuzz gate (§12.6, 2026-09-20; audited and corrected in v2.11.1, 2026-09-22).** A parser
sits on a **trust edge** when it reads bytes a partner, a client or an operator controls *before
anything about them has been verified*. Eight targets cover the v3 axis' three such surfaces —
`caller_frame` and `caller_envelope` (`src/agent/gateway_caller.rs`), `presented_call` /
`catalog_reply` / `federation_objects` / `trust_bundle` (`src/federation*`), and `replay_trace` /
`replay_bundle` (`mycelium-sim`). Nightly `cargo-fuzz`, plus mutation passes over valid seeds in the
in-suite `mini_fuzz_decoders_survive_adversarial_bytes` on every PR (that line needs
`--features fuzz-internals,tls`, or two of the eight are not compiled and the pass silently covers six).

> **Read the next four lessons as one story.** For the first two days these targets existed they
> found nothing, and the reason was not that the parsers were sound: **four defects were waiting**,
> and the gate could not reach them. v2.11.1 is the correction. A gate that exists is not a gate that
> runs, and a gate that runs is not a gate that checks.

The discipline that distinguishes this from the gate above: **assert the invariant the parser is relied
on for, not that it survived.** A crash is the easy case; a wrong-but-well-formed parse is the one that
ships. Byte conservation for the frame (no input byte lost or invented, whichever way it is classified),
signing-field stability for the objects whose signatures cover bytes rebuilt from the parsed fields.
Three lessons, each paid for:

- **A derived `Deserialize` on a validating newtype validates nothing.** `DomainId::new` enforces
  `[a-z0-9.-]` and 253 bytes; the derive wrote the inner field directly, so the rule held only for
  *constructed* ids while most are *parsed* from partner bytes. Same shape in `PrincipalId` / `TermId`.
  **Check every newtype whose constructor validates: the wire path needs a manual `Deserialize`.**
- **`parse → write → parse` is the weak invariant.** A stably-lossy reader reproduces its own mangling,
  so that round trip passes over already-corrupted text. Start from the **value**, not the text:
  `write → read` fidelity is what found `mycelium-sim`'s bundle codec dropping a newline-bearing field.
- **A seed that does not reach the layer tests nothing.** The first `caller_envelope` seed was merely
  frame-shaped, so every envelope assertion was unreachable; the case count was identical before and
  after adding a whole layer. The same trap caught a `serde_fixint` sweep from the other direction:
  40,000 *random* inputs produced **zero successful decodes**, because noise dies on the first length
  prefix — so "0 panics" measured nothing at all until the sweep mutated a *valid* encoding instead.

  **This lesson was written here and then not applied**, which is the part worth remembering. When
  v2.11.1 finally measured it across the whole set, a 20,000-input noise pass reached the invariant of
  **0 of the 7** assertion-bearing targets — none, not few. `presented_call` had asserted round-trip
  stability since the day it was written and had **never executed that assertion**; `trust_bundle` and
  `catalog_reply` had no valid seed at all. Writing the lesson down is not the same as checking it, so
  the check is now structural: a **reachability registry** in the mini-fuzz names every target beside
  the seed that reaches it and fails by name if one stops arriving. It claims completeness in the same
  sense the [lock-order table](../concurrency/lock-order.md) does — **adding a trust-edge target means
  adding a row**.

- **A sequential gate hides everything behind its first failure.** The CI fuzz job runs its twelve
  targets one after another and stops at the first crash. `presented_call` is fifth, and it began
  failing *in the very commit that added the trust-edge targets*. So targets six through twelve never
  executed at all, and `main` was red for **22 consecutive runs** — through an entire slice of AE work
  and through a tagged release. Each fix merely let the queue advance to the next defect behind it:
  four, one at a time.

  Two consequences. **Check CI on the branch you are releasing *from***, not on what fed it — the fuzz
  job is `main`-only, so a green `make check-full` and a green PR say nothing about it
  (`RELEASING.md` step 2b, which exists because v2.11.0 was tagged on exactly that gap). And when a
  sequential gate is red, treat **every later stage as unrun**, because it is.

- **What a parser accepts, its writer must be able to emit.** Three of the four defects were the same
  shape: the accept-set and the emit-set had drifted apart, so a value this node took in, it could not
  put back out. An `/a2a` credential whose ASCII check read the *encoding* (`"\u0809"` is an ASCII
  header carrying a non-ASCII principal); a replay trace losing a bare `\r`, because `str::lines()`
  strips one only when it precedes `\n`; a bundle value trimmed *inside* its quotes. Each parses, none
  survives being written back — and a peer forwarding what we accepted would refuse it.

  The fourth is the general case: `split_once("\": \"")` searched for a delimiter that also occurs
  **inside an escaped key**, so a key of `"` came back as `\`. That one could not be patched — *a
  literal-substring split cannot tell a real delimiter from one inside a quoted string, because the
  information it needs is not in the substring*. It takes a scanner that knows where a string ends.

- **Search the alphabet, do not sample it.** Two fidelity tests for the bundle codec already existed
  and passed throughout, because they used ordinary strings and ordinary strings round-trip fine. An
  exhaustive sweep over the characters that break flat text formats — quote, backslash, the whitespace
  family, the delimiters, the control bytes, every pair up to length two — found **7,092 of 17,556
  pairs corrupted**, then **1,729 more** behind the first fix. Both defects lived in characters nobody
  writes on purpose, which is exactly what an example-based test cannot reach
  (3,808 decodes, 0 panics). **Noise tests the entry check; only mutation tests the decoder.** Every
  mini-fuzz seed now **asserts its own reachability** before being mutated.
- **A public type is not public until it is re-exported, and only an out-of-crate test can tell you.**
  AE1 added `MandateBinding` / `MandateState` as public types on a public field of `ActionEnvelope`
  and left them out of the crate's `pub use`. Every in-crate test passed — in-crate code resolves
  them through the module path — while an evaluator in another crate could *receive* a mandate and
  had no way to name it, which breaks the seam's whole premise that the evaluator is **replaceable**.
  `tests/ae_external_adapter.rs` exists for exactly this and did not catch it, because its foreign
  adapter never used a mandate. **When a seam's promise is "another crate can implement this", the
  test that proves it must exercise every new surface**, or it certifies the surfaces it happens to
  touch and nothing else. Deleting the `pub use` now fails it with `unresolved imports`.
- **A test whose name outruns what it can detect is worse than no test.** The journal's allocation
  bound has no failing test: `vec![0u8; want]` goes through `alloc_zeroed`, which the OS satisfies
  with lazy zero pages, so a 4 GiB request succeeds instantly, the next `read_exact` fails, and the
  outcome is *identical* with or without the bound. Deleting the bound was tried; every assertion
  still passed. The rule was extracted into a `record_fits` predicate that **is** falsifiable, the
  behavioural test was renamed to what it shows, and the limit is written in its doc comment. Gating
  the allocation itself would need a counting `#[global_allocator]` across the whole test binary —
  rejected as disproportionate, worth revisiting if the family recurs.

**The four the sweep named, and what measuring them showed.** An unqualified list of "unfuzzed
parsers" reads as a list of vulnerabilities; this one overstated the risk, so each was measured:

| Parser | Outcome |
|---|---|
| `agent/journal.rs` framing | **Real, fixed.** A `u32` length straight into a `vec` (bound checked *after* the read, never for the first record), and `count_records` counting a torn tail because **a seek past EOF succeeds** — so it disagreed with `read_journal_from` about how many records exist, and it drives the next append's seq. |
| `control/ledger.rs:333` | Reserved surface — returns `Option`, cannot panic, every caller is a `#[test]`. |
| `control/ledger.rs:362` + `serde_fixint` | Live, and **survives**: 20k mutations, 3,808 decodes, 0 panics. The unchecked `remaining()` subtraction is unreachable — reads check length first. Fuzz-targeted anyway, because a hand-rolled binary decoder on disk is the M2 Run-20 shape. |
| `mycelium-commitment/src/lib.rs:482` | `serde_json`, memory-safe. The gap is **provenance, not a decoder bound**: `Offer`/`Award` are unsigned and `award()` picks from them. A design question for an open contract net, still open. |

**Wire back-compat gate.** `codec::tests::decode_wire_v11_agrees_with_v12_on_every_shared_variant`
proves the current decoder reads a **PREV-version (v11)** frame for every shared `WireMessage` variant
— the rolling-upgrade contract (`StateRequest`'s deliberate Merkle-digest change is covered separately
by `decode_wire_v11_downgrades_state_request`). **Corpus discipline** on a `WIRE_VERSION` bump:
regenerate `GOLDENS`, freeze the *outgoing* version's bytes as `V{N}_*` fixtures, add a
`decode_wire_v{N}`, and extend the gate so new code still decodes vN frames. A live two-binary
mixed-version *cluster* test remains a documented (unbuilt) nightly-tier follow-up.

## A local gate predicts CI only where it runs the same things (2026-09-22)

Three times in one week a green local gate meant less than it looked like, in three different ways.
The pattern is worth more than any of them:

| Gate said green | What it had not run |
|---|---|
| `make check-full` before tagging v2.11.0 | the **fuzz** job is `push`-to-`main` only — it had been red for 22 runs, and seven of twelve targets had never executed |
| a unit test on `PreflightRefusal::error_data` | the **surface**: `/mcp`'s refusal body was asserted over a socket, `/a2a`'s never was, which is why the two drifted |
| `make check` on the body-binding change | `cargo build --examples` — CI builds them, the gate did not, and a gallery example used the changed signature |

**A gate that does not run what CI runs is not a prediction, it is a hope.** Two fixes came out of it:
`RELEASING.md` step 2b (check CI on the branch you are releasing *from*), and `make check` now
builds the examples. The general rule is the cheap one: when a local gate and CI disagree about
*scope*, the local gate is the one that is wrong, because CI is what decides.

The corollary is about sequential gates. The fuzz job stops at its first crash, so while it was red
**every later target was unrun, not passing** — four defects were queued behind one. Treat a red
sequential stage as a wall, not a single failure.

## A green run is evidence about that run (2026-09-24)

The page above is about gates that did not *run* the thing. This one is about gates that ran and
still told you nothing — two cases on one day, which is what made the shape visible.

| What went green | Why the green was empty |
|---|---|
| every pre-merge gate on the `require_identity_proofs` default flip | nothing was testing the *default* — every test that exercises the flag sets it explicitly. (The flip was then reverted for a failure it turned out **not** to have caused; see the third rule below) |
| `auto_election_is_deterministic` for four days after the rule became rendezvous | the assertion (*lowest id wins*) had become a **coin flip** — kernel-assigned ports, and `hash(ring, node)` favours neither — so two passes on `main` were two heads, not two checks |

**Neither test was wrong about what it asserted. Both were wrong about what their passing implied.**
The first shipped a default nothing was checking. The second passed twice by luck and failed on the
next unrelated branch — an identity-proofs revert — which is the only reason anybody looked.

Three rules come out of it. The third was learned the hard way, hours after the first two were
written:

1. **A config default whose only failure mode is a race between processes is not testable by the
   suite that gates the PR.** Flipping one needs the Docker suites deliberately, before merge, or
   it needs the window designed out. A green `make check` on such a change is not evidence; it is
   the absence of a gate. (The revert and its full account:
   [`../security.md`](../security.md), `.log/2026-09-24-identity-proof-default-revert.md`.)
2. **Assert the property, or compute the expectation from the rule — never restate the rule in the
   assertion.** `assert!(ts1.is_primary())` names an answer the rule happened to give; *exactly one
   primary, and it is the node `mycelium::election::winner` names* is the property, and it stays
   true across a rule change or fails honestly. The wiki's failover assertions
   (`is_curator() ^ is_curator()`) survived the rendezvous change untouched for exactly this reason
   — they never named the rule. The tuple-space one did, and rotted silently.

3. **A red run is evidence about that run too — including about its cause.** The identity-proofs
   flip was reverted on the reasoning *it was the only change in that commit, and the suite went
   red*. That reasoning never checked whether the mechanism could reach the failing test. It could
   not: the flag is inert without TLS, and the overlay suite's nodes configure none. The conclusion
   was written into six documents before anyone looked at `lifecycle.rs`. **Before attributing a
   failure to a change, find the path from the change to the assertion** — "only change in the
   commit" is a prior, not a mechanism. The leader-election intermittency it was blamed on is still
   open.

The failure mode to watch for in rule 2 is specific and easy to name once seen: **an assertion that
a rule change makes probabilistic rather than false.** It does not go red on the PR that breaks it.
It goes red later, somewhere unrelated, and looks like a flake.

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

## Replay — its own page

Determinism, the nondeterminism inventory, the seams, scenarios A/B/C, the checked-in corpus and
the scheduler seam are [testing/replay.md](replay.md). They moved there when this page's replay
material outgrew a section: the lore is one subject, and it is the subject a session reaches for
when a recorded run does not reproduce.

## The federation transport's two-mesh test (item 2 PR 8, 2026-09-18)

`src/lib_tests.rs` → `federation_transport` (features `gateway` + `tls` + `a2a`, so it runs in the main
`cargo test --lib --features tls,metrics,a2a,llm` job). Two real meshes in one process, bootstrapped only within
themselves; domain A's first node also runs a gateway with `with_a2a()` and `with_federation_edge(...)`; domain
B talks to it only through a `FederationClient` over `reqwest`. The PR 1 harness's `assert_never_merged` was
lifted to a free function over `&[&GossipAgent]` so a node held in an `Arc` (the provider task needs
`request_principal`) can be checked by the same code.

What it proves, and how it is kept non-vacuous:

- **Bytes crossed.** A counter on the provider: exactly two dispatches in the whole run — the federated call
  and one anonymous A2A call (the unchanged path) — and *zero* from the eight plants at the gateway.
- **Never merged, with bytes crossing.** The harness assertions run last, after the call; the gateway node's
  peer table has exactly its own mesh.
- **Refused before any byte** is asserted by the error variant (`Link(Down)`, `Resolve`), not by absence of a
  dispatch alone.
- **A silent gateway is a real one:** a second client lists a dead port first; at-most-once is
  `DeliveryUnknown { attempted_via: ["gw-dead"] }`, repeatable fails over to the live one.

**Readiness, not sleeps.** The gateway's listener binds after `start()` returns. The tests wait on the
capability key (as the A2A caller test does) or, for a bare gateway with no capability, on a bounded
connect loop — the first draft probed too early and failed on `ConnectionRefused` (recorded here so the next
gateway test does not repeat it).

**What it does not prove:** the traces leg of the release gate; anything about TLS (plain HTTP on loopback);
policy change *mid-partition* (a revocation mid-session is exercised, a partition is not). *(PR 10a added the
signed catalogue: the first test's client requires alpha's key, and a client holding the wrong key is refused
`BadSignature` before anything is relied on; the second test's unkeyed edge is refused `Unsigned` by a requiring
client.)*

## The release gate's choreography (item 2 PR 9, 2026-09-18)

`lib_tests::federation_transport::the_release_gates_choreography_over_the_transport`, same features as above.
Every node runs the **enforced domain profile** (`DomainProfile::Enforced`: TLS, SWIM off) and the two meshes
use two different `auto_cert_dir`s, so two auto-generated CAs — which is what "independently admitted" reduces
to in one process. The provider lives on a plain node; the gateways only route, which is what makes one
replaceable. Steps: discover, invoke, a consensus round in each mesh, lose the only gateway, keep working
locally (gossip and consensus on both sides), change the grant mid-partition, bring up the replacement
gateway (its ports allocated up front — the client must know ≥ 2 gateways from the start), reconnect, retire
the dead gateway, present a credential issued before the partition and an expired one, then the harness's
three legs (below), then a rogue node holding B's CA bootstrapped at A.

The harness (`assert_never_merged`, now a free function over `&[&GossipAgent]`) has **three legs**: the
membership tables; the `cap/ grp/ sys/ consensus/` namespaces over keys *and* values; and the **connection
tables** (`GossipAgent::connected_peers`, the transport's own record of whom it wrote to — the *traces* leg).
Its non-vacuity test (a deliberately merged pair must fail) now also checks the merged pair shows up in the
connection table, so leg 3 is checking a record a merge actually writes. The choreography adds explicit
`consensus_get` cross-checks of each mesh's slots on the other mesh's nodes, because the generic namespace
scan only recognises node ids, not foreign slots.

**Three things this test taught, so the next gateway test does not relearn them:**
- **TLS formation needs fast pings.** Peer registration happens on Ping receipt; under TLS with default
  intervals the meshes did not form inside an 8 s poll. `reconnect_backoff_secs = 1` and
  `health_check_interval_secs = 1`, as the WS1 TLS test sets them.
- **A secure-profile gateway dispatches only to a provider whose `sys/caller-context` marker it has seen**
  (item 7), and the marker reaches it by gossip like the capability does. Poll for both before the first call;
  the first draft polled only for the capability and got `-32021`.
- **An export needs a skill behind it.** A grant added mid-partition is only callable if the provider
  advertises that skill; the gateway answers `-32001 skill not found` otherwise. Not a federation refusal —
  the test now advertises both exports and the pre-call poll checks both.

**What it does not prove:** process isolation and a real network severance (the Docker suite's claim: here the
severed link is a shut-down gateway, and the meshes share an address space); the catalogue's integrity in *this*
test (its edge is unkeyed and its client does not require a signature — the signed path is covered in the two
tests above); anything under the `sim` kernel (the choreography is wall-clock, with structural polls); the rogue-CA plant is a
timing-bounded negative (1.5 s), with gw2's join as its positive control.

## The two-mesh Docker suite (item 2 PR 10b, 2026-09-18)

```bash
make test-federation          # ~4 min after the image build; CI job `federation`
make test-federation-clean    # tear down, including the plant
make federation-keys          # derive the suite's public keys from its fixed seeds
```

The same choreography as the in-process test above, with the two caveats that test could not
remove: **process isolation** (one container per node) and a **real network severance**
(`docker network disconnect`). This is the only place item 2's release gate is claimed without a
caveat. Files: `examples/federation_node.rs` (one binary, four roles — `member`, `gateway`,
`probe`, `keys`), `docker/docker-compose.federation.yml`, `docker/Dockerfile.federation`,
`tests/integration/run_federation.sh`.

**Four networks, and why the fourth is not optional.** `alpha-net` and `beta-net` carry each mesh;
`edge` carries the federation path (alpha's two gateways, beta's probe); `control` carries the
runner's commands to the probe. The runner severs the link by disconnecting the probe from `edge` —
if it drove the probe over `edge` it would cut its own control channel in the same instant, and the
test would hang instead of observing. The runner is deliberately **not** on `edge`.

**Everything is asserted from a table, never a log line.** Each node serves `/fed-admin/tables`
(membership, the `cap/ grp/ sys/ consensus/` entries with keys *and* values, and
`connected_peers`), plus propose / committed / kv / policy / revoke. Those routes mount through
`with_http_routes` **outside** `/gateway/`, so they are unauthenticated by the library's documented
rule for merged routers — they are test-only, on a private network, and nothing in `src/` depends
on them.

**What it found, which is why it exists.** With the edge network disconnected the client **hung**
instead of returning. A *refusing* partner sends a TCP reset and fails fast; a **blackholed** one —
interface gone, default route still present — sends nothing, and an unbounded connect waits
forever. Since PR 5 promises `DeliveryUnknown` for a silent gateway, a client that never returns
cannot deliver that verdict: a contract defect, not a test artefact. Fixed with a bounded HTTP
client (`FederationClient::with_timeouts`, defaults 5 s connect / 30 s request) and pinned
in-process by `a_blackholed_gateway_is_unknown_within_a_bound_rather_than_hanging`, which points a
client at `192.0.2.1` (RFC 5737 TEST-NET-1) and asserts the **bound**, not the error — on a host
that answers `ENETUNREACH` the call fails fast and the bound still holds. **The in-process test
could not have found this:** its "severance" is a shut-down gateway, and a refusal fails fast.

**Four things the first drafts got wrong, recorded so the next Docker suite does not repeat them:**
- *The harness cannot share the path it severs* (the four-network point above).
- *A bind mount of the repo is empty* on any host whose checkout is outside Docker Desktop's
  file-sharing list — the runner exited 127 on a script that was plainly there. The script is baked
  into the runner image instead, and its Dockerfile sits in `tests/integration/` because the
  repo-root `.dockerignore` excludes that directory from the root context on purpose.
- *The runner image has `docker-cli`, not the compose plugin*, and no compose file is mounted into
  it. The admission plant therefore starts with `docker run`, which is why the image and the CA
  volumes carry pinned names (`mycelium-federation-node:test`, `mycelium-fed-{alpha,beta}-ca`).
- *Two shell bugs cost a full run each.* `${2:-{}}` closes the expansion at the first `}` and
  appends a stray one, silently corrupting every request carrying a body — which is why `connect`
  passed while `call` did not. And `sh -c "post …"` spawns a shell where the helper function does
  not exist: 26 checks failed for that reason alone while the code under test was fine. Every check
  now calls a function **in the runner's own shell**, and `check` prints the last probe body on
  failure.

**Non-vacuity.** The plant asserts the rogue container is *up* before asserting it has no peers, so
"beta's CA cannot join alpha" is about admission rather than about a container that failed to
start; the capability polls before the first call make "the call crossed" a statement about
federation rather than about a race with gossip.

**What it does not prove:** TLS on the federation edge (intra-mesh traffic is TLS under each
domain's CA, as the enforced profile requires; the gateways' HTTP is plain inside the compose
network); a hostile network between domains; more than two domains; anything under the `sim` kernel.

## A per-partner claim is unfalsifiable with one partner (2026-09-23)

Item 2's record states a dozen guarantees about **a partner**: this partner sees only its grants, this
partner's credential authorises this export, revoking this partner stops only this partner. Every one of
them was tested, and every test configured **one** partner.

With one partner in the bundle, "the key for the claimed origin" and "the only key there is" are the same
key, so a whole family of defects is invisible. The decisive plant: make `TrustBundle::acceptable_keys`
return **every** non-revoked key in the bundle instead of the claimed domain's.

| | Result under the plant |
|---|---|
| 24 pre-existing federation tests | **all green** |
| the impersonation gate (two trusted partners) | fails: beta's key authenticates a credential naming gamma |
| the three-domain end-to-end gate | fails: the forged call returns **200**, so it *ran* |

That defect is a complete authentication bypass between partners, and the existing suite would have shipped
it. The same shape caught a union catalogue (beta learns gamma's export exists) and an over-broad revoke.

**The rule: a claim quantified over X needs two Xs to be a test.** One partner, one domain, one tenant, one
gateway: a suite built at cardinality one cannot distinguish *per-X* behaviour from *global* behaviour, and
the failure it misses is usually the one where X's authority leaks to Y. It is cheap to fix and easy to
forget, because a cardinality-one suite is green and looks complete.

The companion claim needs three: **a common neighbour must not be a bridge.** Two domains that never spoke,
both federated with a third, must not appear in each other's membership, native namespaces or connection
tables. `lib_tests.rs` → `three_domains_one_edge_and_the_middle_domain_is_not_a_bridge`.

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
