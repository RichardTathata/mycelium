# What is proven — and what is not

↑ [Operations](README.md) · beside the [shared-responsibility matrix](shared-responsibility-matrix.md)

**As of 2026-09-28 (v2.16.0).** One dated page for the line between what CI proves on every merge,
what has been demonstrated with its bound stated, and what has not yet been shown. Every line names
its evidence. The places that used to restate this list — `CLAUDE.md` § Active work, the contracts-axis
plan §10, the self-audit series, the publications ledger, both decks — now link here instead, and
`RELEASING.md` § 6 refreshes this page at every release. It exists because each gap below was already
stated honestly somewhere, and a reviewer assembling them from six places assembles them against us.

> **How to read it.** *Proven* means a deterministic gate that turns red on regression, run by CI on
> every merge. *Demonstrated* means it was run and measured, with the bound or the condition stated,
> but not on every change. *Not yet shown* means exactly that, with what would show it.

## Proven in CI on every merge

| Claim | Gate | Where |
|---|---|---|
| After a revocation, **every door runs nothing** — `/mcp`, `/a2a` send and stream, the raw routes, a member's direct call, the SDK serve stream — by the handlers' own counters, with a plant proving the doors reach the handlers beforehand | `test_c7_the_bypass_matrix_no_door_runs_revoked_work` | `src/lib_tests.rs`; `ci.yml` job `test` |
| An acknowledgement **names its rung and nothing above it**; today's `bool`s and `persisted` are pinned | the `floor_*` tests and the golden on-disk fixtures under `tests/fixtures/persistence/`, replayed | `src/lib_tests.rs`; `ci.yml` job `test` |
| A recorded node **replays without divergence** | the checked-in replay corpus and `mycelium-sim`'s tests | `mycelium-sim/tests`; `ci.yml` |
| **Running work stops** when its authority lapses, within its class's bound | `examples/authority_drain`, run and asserted | `ci.yml` line 98 |
| The four CLI demonstrations of the contracts axis, two of which found real defects before release | `procurement_authority`, `a2a_skill_authority`, and the gallery rows | `ci.yml` job `test` (`a2a_skill_authority` line 106) |
| No new nondeterminism outside the seams · no foreign state in the gossip medium · every wiki mutation inside the mandate fence · every `Hlc::current()` read classified · one positioning sentence | `scripts/check-sim-seams.sh` · `check-kv-namespaces.sh` · `check-wiki-mutation-fence.sh` · `check-hlc-current.sh` · `check-positioning.sh` | `make check`; `ci.yml` job `clippy` |
| The **twelve fuzz targets** each reach their own invariant (the reachability registry) — on push and schedule, not on PRs | `fuzz/fuzz_targets/` (12) | `ci.yml` job `fuzz` |
| A 4-node integration cluster, a 3-node overlay cluster, a **two-mesh federation that does not merge with the link cut**, and the confined fleet — **no retries** | the Docker cluster suites | `cluster-suites.yml` jobs `integration` · `overlay` · `federation` · `confined-fleet` |
| Every companion crate's suite, both SDKs, the co-op and AFN smokes, `cargo audit`, the loom model | the per-crate jobs | `ci.yml` jobs `wasm-host` · `agentfacts` · `blackboard` · `effects` · `commitment` · `wiki` · `reason` · `guardrails` · `python-sdk` · `sdk-ts` · `coop-smoke` · `afn-smoke` · `audit` · `loom` |
| A regression is fixed only with a test **seen failing first** | the repository's bar since 2026-09-26; stated in each fix's commit | `CLAUDE.md`; the v2.15.1 and v2.16.0 changelog entries |

## Demonstrated, with the bound stated

| Claim | What was measured | Bound or condition | Where |
|---|---|---|---|
| **100-node** cluster formation, gossip propagation, failover and convergence | the scale suite, `make test-scale` (100 Docker nodes) | runs only on a self-hosted or local runner; single-host Docker-bridge ceiling above ~50 nodes; formation-variance failures documented | `docs/wiki/dev/testing/scale-tests.md`; last green local rows **2026-09-24** (`resilience`, `entries`) |
| Ten councils writing one git store concurrently | 5.5 batches/s shared checkout · 3.0 batches/s clone-per-node | one macOS dev machine; zero spurious failures | `docs/plans/council-substrate-hardening.md` §Phase 6 |
| Hot-path cost | `kv/set` 151 ns · `kv/get` 16 ns · `scan_prefix` 100k entries 622 µs · signal fan-out ≈1 µs | **micro-benchmarks**, one dev machine, no network I/O, measured 2026-07-10 | `docs/operations/tuning.md` §Performance baselines; `benches/` |
| Gateway overhead | ≈40 µs per call on loopback | single node, isolates the HTTP round trip only | `benches/gateway_overhead.rs`; publications ledger 2026-07-13 |
| Hysteresis in the control loops is load-bearing | with the breakers on every swept schedule settles; with them off at least one does not | scenario C's bounded sense; the sharper sentence (loops oscillating together while each is stable alone) is **not shown** — the three loops share one state variable | `docs/plans/v3-contracts-axis.md` §10 (2026-09-22) |
| Trust does not compose across three federated domains | `three_domains_compose_without_trust_composing` | in-process, three domains | `docs/plans/v3-contracts-axis.md` item 2 row 11 |

## Not yet shown — and what would show it

| Gap | State on 2026-09-28 | What would show it |
|---|---|---|
| **A named third-party production deployment** | none. NovusLens is a design partner that consumes the evidence records; the council-minutes corpus is envelope-qualified, not live | a customer pilot run to the `customer-pilot.md` bar, named with consent |
| **V1 — the nightly scale runner** | the hosted job (`scale-nightly.yml`) has never had a runner registered and queues silently; the local launchd runner's last four rows (2026-09-25/26) failed with exit 2 on the environment (Docker/VM), not on the substrate; last green 2026-09-24 | a registered runner and a window of green nightlies, recorded in `results.csv` |
| **Performance under load** | only the micro-benchmarks above; no end-to-end throughput or latency figure for a cluster under sustained load, and the benches are not run in CI | a load run on a real network with the numbers in `tuning.md`, and a bench job with a stored baseline |
| **AE4 — the four joint AWS/GCP runs** with the oversight consumer | the Cedar adapter and the signed exporter are implemented in the private companion, pinned to this release; the joint runs have not happened, and a local fixture does not stand in for them | the runs themselves; §6.8 of the axis plan says neither a fixture nor one cloud can close it |
| **Models pulled from S3 or GCS** | the adapter exists (S2, 2026-09-28: `ObjectStoreFetcher` over the `object_store` crate — S3, GCS, Azure, HTTP, local — ranged, egress-gated, the manifest in the store), exercised in CI against a **MinIO** service and locally over `file://`; **no real bucket has been used**, and no GCS fixture runs | `design-time-tooling.md` S3 (a GCS fixture) and S4 (real buckets, delivery evidence) |
| **The self-audit floor** | 7 / 7 / 7 for Modularity, Performance and Operational Readiness / Developer Experience across many runs (Run 62, 2026-09-26); the calibration ledger holds **48 entries** of scores later proven wrong | fresh execution evidence per dimension, not a re-asserted number — the M2 rule |
| **Combined-feedback oscillation** | not posed — the three control loops share one state variable and neither isolation method can ask the question | a second state variable, or a different instrument |
| **The check-then-act window** | narrowed and stated per site, not eliminated (`late_writes()` counts them) | it cannot be eliminated by this design; the count is the claim |
| **Identity proofs required by default** | `require_identity_proofs` is default-off; a commitment signature's strength rests on it; `clock_sync` is always `Unverified` | a deployment that turns it on and a release that flips the default |
| **The declared fleet** | the unit file, the offline check, the stem topology and agent-authored functions under a fuel budget and a second signature are a plan | `docs/plans/design-time-tooling.md`, W1 onward |

## What this page is not

It is not the threat model, the shared-responsibility matrix or the production-readiness checklist;
those say what the library provides and what you own. This page says what the project has and has
not *shown*. A line moves from the third table to the first only with a gate CI runs, and the move is
dated here.
