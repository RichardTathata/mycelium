# What is proven — and what is not

↑ [Operations](README.md) · beside the [shared-responsibility matrix](shared-responsibility-matrix.md)

**As of 2026-10-03 (v2.20.0).** One dated page for the line between what CI proves on every merge,
what has been demonstrated with its bound stated, and what has not yet been shown. Every line names
its evidence. The places that used to restate this list — `CLAUDE.md` § Active work, the contracts-axis
plan §10, the self-audit series, the publications ledger, both decks — now link here instead, and
`RELEASING.md` § 6 refreshes this page at every release. It exists because each gap below was already
stated honestly somewhere, and a reviewer assembling them from six places assembles them against us.

> **How to read it.** *Proven* means a deterministic gate that turns red on regression, run by CI on
> every merge. *Demonstrated* means it was run and measured, with the bound or the condition stated,
> but not on every change. *Not yet shown* means exactly that, with what would show it.

## Developer teaching checks added 2026-10-02

The [new learning path](../guide/tutorials/README.md) adds the
`first_stem_fleet` example and `scripts/check-stem-observations.py` to the
`wasm-host` CI job. The checks cover a signed echo component, useful invocation,
graceful provider replacement, a schema-validated declaration/observation join,
and missing-provider, unknown-unit, duplicate and invalid-schema refusals.
The workflow is configured; its first hosted CI result is still pending. The local
fleet run and all five consumer checks passed on 2026-10-02; validation scope is
recorded in the [teaching-path ingest](../wiki/dev/.log/2026-10-02-developer-teaching-path.md).

This is one local fleet and a scenario-specific consumer. It does **not** close
the deployment, multi-machine recovery or NovusLens consumer-rendering gaps below.

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
| A **composed effect** is refused at the destination unless it is attributed (the principal is the mandate's holder) and authorised (`ResourceAuthority::check`, now) — each leg planted missing leaves no row; the domain leg is carried, not re-verified | `mycelium-effects/tests/composed.rs`, written before the check and seen committing every planted leg | `ci.yml` job `effects` |
| **A node says which guarantees it enforces, and why not the rest.** Every registered guarantee resolves to one of five states against the node as built and configured; a node with no gateway does not fail a gateway guarantee; an external prerequisite is listed and never counted; a companion's registration cannot duplicate or override a core id. | `cargo test --lib guarantee::` (both feature arms), the confinement view's tests | `src/agent/guarantee.rs`; plan I2. **Also:** under `profile = "secure-single-domain"` a node with an open gateway or plaintext gossip refuses to start, naming each unmet guarantee (I3). **Not yet:** the per-route counter test under the profile; the per-guarantee exhaustive state matrix and a checked-in golden |
| **The declared fleet checks offline and runs as stem nodes.** Every example's declaration directory checks clean and turns red when the last provider of any capability is deleted; the co-op fixtures name their ghost, their unauthorisable edge and their schema window; a proposed artifact is *would bind after acceptance*; the declaration document validates against its pinned schema | `tests/wire_check_examples.rs`, `tests/wire_check_fixtures.rs`, `tests/declaration_schema.rs`, `scripts/wire-check-examples.sh` | `ci.yml`; `docs/reference/declaration.schema.json` |
| Every artifact-shaped co-op demo — `provisioning` (with shadow-then-accept), `catalog` (with the peer-cache late joiner), `mcp_toolgrowth`, `model_deploy`, `reheal_deploy`, `llm_agent` — reaches the **same asserted outcome as stem nodes** fed only their declaration directories; the three model demos against a **real model in Ollama** (placed, activated by `[[activation]]`, served by `[[serve]]`, real tokens, a survivor answering after the origin is killed). **What CI runs is the stem half:** the code half of `provisioning`, `catalog` and `mcp_toolgrowth` runs in the co-op smoke, so those three are compared both ways on every merge; the code half of `model_deploy` and `reheal_deploy` is manual (it needs a local Ollama and `MODEL_GGUF`), `llm_agent`'s is the browser demo, and `make examples-both-ways` is not a CI job | the stem-examples suite (`make test-stem-examples`); the co-op smoke (`examples/coop/ci_smoke.sh`) for the three code halves | `cluster-suites.yml` job `stem-examples`; `docker/docker-compose.stem-examples.yml` |
| An agent-published entry that loops is **stopped at its fuel budget** and the record says so, the operator's runs unbounded; a **proposed** entry loads only into the shadow lane until a listed reviewer accepts it, and a forged acceptance changes nothing | `an_agent_published_entry_that_loops_is_stopped_at_its_budget_and_the_operators_is_not`, `a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it`, the co-op `provisioning` demo's wave 3 | `ci.yml` jobs `wasm-host`, `coop-smoke` |
| A signed catalogue line published through the gateway **reaches a second node with provenance intact**, and every refusal answers by name | `mycelium-wasm-host/tests/gateway.rs` | `ci.yml` job `wasm-host` |
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

| Gap | State on 2026-10-03 | What would show it |
|---|---|---|
| **A named third-party production deployment** | none. NovusLens is a design partner that consumes the evidence records; the council-minutes corpus is envelope-qualified, not live | a customer pilot run to the `customer-pilot.md` bar, named with consent |
| **V1 — the nightly scale runner** | the hosted job (`scale-nightly.yml`) has never had a runner registered and queues silently; the local launchd runner's last four rows (2026-09-25/26) failed with exit 2 on the environment (Docker/VM), not on the substrate; last green 2026-09-24 | a registered runner and a window of green nightlies, recorded in `results.csv` |
| **Performance under load** | only the micro-benchmarks above; no end-to-end throughput or latency figure for a cluster under sustained load, and the benches are not run in CI | a load run on a real network with the numbers in `tuning.md`, and a bench job with a stored baseline |
| **AE4 — the four joint AWS/GCP runs** with the oversight consumer | the Cedar adapter and the signed exporter are implemented in the private companion, pinned to this release; the joint runs have not happened, and a local fixture does not stand in for them | the runs themselves; §6.8 of the axis plan says neither a fixture nor one cloud can close it |
| **Models pulled from S3 or GCS** | the adapter exists (S2: `ObjectStoreFetcher` over the `object_store` crate — S3, GCS, Azure, HTTP, local — ranged, egress-gated, the manifest in the store), exercised in CI against an **S3-compatible mock** (Adobe's S3Mock) and locally over `file://`; the same test takes a GCS URL, but **no GCS emulator runs**: `fake-gcs-server` accepts only signed-URL uploads and the crate puts with a plain PUT (S3 ◐, two CI runs); **no real bucket has been used** | `design-time-tooling.md` S4 (real buckets, delivery evidence, dated) |
| **The self-audit floor** | 7 / 7 / 7 for Modularity, Performance and Operational Readiness / Developer Experience across many runs (Run 62, 2026-09-26); the calibration ledger holds **48 entries** of scores later proven wrong | fresh execution evidence per dimension, not a re-asserted number — the M2 rule |
| **Combined-feedback oscillation** | not posed — the three control loops share one state variable and neither isolation method can ask the question | a second state variable, or a different instrument |
| **The check-then-act window** | narrowed and stated per site, not eliminated (`late_writes()` counts them) | it cannot be eliminated by this design; the count is the claim |
| **Identity proofs required by default** | `require_identity_proofs` is default-off; a commitment signature's strength rests on it; `clock_sync` is always `Unverified` | a deployment that turns it on and a release that flips the default |
| **A TLS node without the fleet CA's private key** | `tls::load_or_create_ca` regenerates a CA when the key is absent, so every TLS node holds it at start and `id.ca_key_off_node` is reportable but not requirable (found by the secure profile's acceptance test, 2026-10-03) | a TLS init that starts from `ca_cert_pem` + a pre-issued `cert_pem`/`key_pem` without the CA key, with a test that a node so provisioned joins the fleet |
| **`llm_agent`'s MCP tools as stems** | the demo's tools (weather, ping, search, calculate) are in-process handlers; the unit format has no row for a tool, so its stem run recuts the capabilities, the dataset and the model, not the tools | a tool row in the unit format, or the tools rewritten as components bridged by the runtime (as `mcp_toolgrowth`'s converter is) |
| **A stem reading an object store** | a stem's byte sources are a library directory (`--library`) or a mesh pull from a librarian (`StemSource::Library`, `StemSource::Mesh`) — there is no store-backed `StemSource`, and the stem binary's librarian reads its manifest from a local file (`manifest_source: None`). A store-backed library reaches a stem only through a mounted or synced directory | a store-backed `StemSource` and a librarian `--manifest-source` flag, run against the S3 mock in the stem-examples suite |
| **Large models over the mesh** | the mesh path rides the gossip frame, bounded by `MAX_FRAME_BYTES` (10 MiB), so a stem pulling a large model over the mesh cannot receive it; the model demos use `--library` on a mounted volume | a stem fetching a model past the frame cap over the bulk transport, in the suite |
| **A keyed `[[serve]]` endpoint without a secret in the unit file** | `[[serve]].api_key` is a literal in the file (default `"none"`); there is no environment indirection, so a keyed endpoint puts a secret in the unit file, which must then be kept out of source control. The workaround is a local unauthenticated proxy in front of the keyed endpoint | an `api_key_env` (or similar) field, read at stem start, with a test |
| **The declaration as a consumer record, rendered by the consumer** | the schema is pinned and every edge carries its call shapes (W6, public); the private companion ships the `deployment_declaration` batch on the `v2.17.0` pin, retry-identical, accepted by its stub consumer, the join tested on the co-op golden (companion PR #4, 2026-09-30) | the consumer (NovusLens) accepting a real batch and rendering a declared edge beside its observations — on their side of the handover |

## What this page is not

It is not the threat model, the shared-responsibility matrix or the production-readiness checklist;
those say what the library provides and what you own. This page says what the project has and has
not *shown*. A line moves from the third table to the first only with a gate CI runs, and the move is
dated here.
