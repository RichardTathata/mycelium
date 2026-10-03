# Zero gaps — the nine recorded code gaps, closed

**Status:** adopted 2026-10-03, rev 0.1. Source baseline: `main` at v2.21.0 plus #501–#505 (the guarantee
catalogue, two lint passes, doc-coverage run 19, the paper 2a sweep). Delivery tracked in §4; the
record of what is proven stays `docs/operations/what-is-proven.md`, which this plan empties of code
gaps and does not touch for anything that needs a counterparty or hardware.

## 1 · Why one plan

After the guarantees plan closed, the repository recorded **nine** code-shaped gaps and nothing else
of that shape: five on `what-is-proven.md` § not yet shown, two carried as `~` cells on the
doc-coverage matrix since run 18, two found by the publication lint of 2026-10-03. Each is small
enough to have been skipped at every release and real enough that a reader of the proof page meets
it. One plan, nine increments, one exit criterion: **the proof page lists no gap a commit can
close**, and the doc-coverage matrix has no `~` cell that says *code gap*.

The nine, where each is recorded, and what a reader is told today:

| # | Gap | Recorded | What the record says would show it |
|---|---|---|---|
| 1 | A stem cannot read an object store | `what-is-proven.md` | a store-backed `StemSource` and a librarian `--manifest-source` flag, run against the S3 mock in the stem-examples suite |
| 2 | `[[serve]].api_key` is a literal in the unit file | `what-is-proven.md`, `unit-file.md` § limit | an `api_key_env` field, read at stem start, with a test |
| 3 | Large models over the mesh | `what-is-proven.md` | a stem fetching a model past the 10 MiB frame cap over a bulk path, in the suite |
| 4 | `llm_agent`'s MCP tools as stems | `what-is-proven.md`, `examples/units/llm_agent/app.toml` | a tool row in the unit format, or the tools as components bridged by the runtime |
| 5 | A live node's recording replayed decision for decision | `what-is-proven.md`, inventory row 141 | the scheduler seam over every periodic loop |
| 6 | `mycelium-effects` has no refusal counter | doc-coverage run 18 (`~`) | a counter beside the evidence journal's landing |
| 7 | The SDKs' `set()` discard the receipt | doc-coverage run 18 (`~`) | `set()` returning what `POST /gateway/kv` already returns |
| 8 | `provisioning_viz` and `guardrail_viz` both bind :8097 | publication lint run 5 | one of them moves |
| 9 | No connection-level peer gauge | publication lint run 5 | `gossip_peers_connected` beside `gossip_store_entries` |

## 2 · What already exists (the reconnaissance of 2026-10-03, by `file:line`)

**Gap 1.** `StemSource { Mesh { timeout }, Library(PathBuf) }` (`mycelium-wasm-host/src/stem.rs:42-52`),
resolved to an `Arc<dyn ArtifactSource>` in `Stem::start` (`stem.rs:181-212`); the ticker prefetches for
the mesh variant only (`stem.rs:266-298`). `ObjectStoreFetcher` (`object_store_source.rs:36-42`, feature
`object_store`) implements `BlobFetcher`, `RangedBlobFetcher` and `ManifestSource` and takes its
credentials from the environment (`from_url`, `:57`). `DiskStagedSource` (`http_source.rs:301-426`)
bridges any `RangedBlobFetcher` to the synchronous `ArtifactSource + RangedArtifactSource` the
provisioner needs, staging to a directory in 4 MiB ranges; the S2 test already chains the two
(`object_store_source.rs:359-361`). `LibrarianConfig.manifest_source` exists and `sync_once` honours it
(`librarian.rs:40-55, 107-111`); every caller passes `None`. CI's `wasm-host` job runs Adobe S3Mock on
`:9090` with `MYCELIUM_S3_TEST_URL` and the `AWS_*` variables (`ci.yml:229-260`); the stem-examples
compose file has no store service and `Dockerfile.stem` builds without `object_store`.

**Gap 2.** `ServeDecl.api_key: Option<String>` (`src/capability_config.rs:216-232`); its one read is
`serve.rs:31`, `unwrap_or("none")` into `OpenAiBackend::new` at registration time inside the tick loop.
`validate()` (`:738-751`) checks nothing about it. No unit-file field reads the environment; the
precedents are `mycelium-artifact --key-env VAR` (fails by name if unset) and `GossipConfig::
apply_env_overrides`. Activation placeholders are a closed, validated set (`:262, :727-734`).

**Gap 3.** `MAX_FRAME_BYTES = 10 MiB`, `MAX_KV_WRITE_BYTES = MAX_FRAME_BYTES − 64 KiB` (`framing.rs:16,25`).
The mesh fetch is one whole-object RPC, kind `artifact.fetch` (`mesh_source.rs:24-65`), into an in-memory
cache; there is no ranged mesh fetch. Core's `bulk_call`/`bulk_serve` (`src/agent/bulk.rs`) moves a large
*request* caller→server over HTTP staging, but its *reply* rides a `bulk.result` signal and is
frame-bound — the wrong direction for a pull. `DiskStagedSource` is reusable; `FsLibrarySource` is
ranged; `BlobRuntime::install` already streams through `as_ranged()` (`runtime.rs:580-635`). The
model demos' 19 MB GGUF already exceeds the cap, which is why every model host runs `--library /lib`.

**Gap 4.** The runtime already bridges any installed component whose `provides.ns == "tool"` into an
MCP tool over `mcp.invoke` (`runtime.rs:433-462`, feature `gateway`), with a generic `{"type":"object"}`
schema; `mcp_toolgrowth` runs on it with no tool section. The four `llm_agent` tools are Rust closures
(`examples/llm_agent.rs:181-249`), carried in the unit files only as attrs (`n-0.toml`, `n-1.toml`).
A component is a 33-line `wit-bindgen` crate built for `wasm32-wasip2` and committed as a fixture
(`tests/fixtures/unit-convert-component/`); the target is installed here. The stem driver's
`llm_agent()` asserts never invoke a tool (`stem_driver.rs:308-347`); `mcp_toolgrowth()` does (`:446-480`).

**Gap 5.** Every one of the fifteen periodic loops ticks through `sim_seam::interval_ms` (the explorer's
table: `a2a/sweep` … `swim/probe`). `Kernel::decide` (`mycelium-sim/src/kernel.rs:122-169`) records a
choice after the wait and, on replay, refuses a request that is not the next recorded one. The
"scheduler seam's first arm" is `pause_clock_for_replay` (`sim_seam.rs:503`): it orders waits *of
different lengths*. **The divergence is narrower than the page says**: with `health_check_interval_secs
= 1`, `membership/tick` and `health/tick` share a 1 s period, fall due at the same paused instant, and
tokio's wake order decides which checks in first — the case the inventory lists as not owned
(`replay-nondeterminism-inventory.md:278-281`). Also: the replay `Ticker` sleeps a *relative* `nominal`
from the call rather than the recording's absolute grid (`sim_seam.rs:567-575, 624-681`), and its
`first`/`deferred_ms` state is mutated before the await, so a `select!` sibling that cancels a tick
leaves it wrong. `ChoiceKind::Sched` exists and nothing emits it. The test file has only the recording
half (`tests/decision_trace_replay.rs:51-86`).

**Gap 6.** `EffectDestination::apply_composed` (`mycelium-effects/src/lib.rs:322-340`) calls
`check_composition` then `apply`; a refusal is an `Err(EffectRefusal)` to the caller and nothing counts
it. The crate has no `metrics` dependency; the gateway counts its refusals with `metrics::counter!`
(`http.rs:2430-2441`).

**Gap 7.** `POST /gateway/kv` returns `{"ok", "operation_id", "local_durability", "local_durability_error"?}`
(`http.rs:2643-2700`). `mycelium-py` `set()` (`agent.py:644-655`) posts and returns `None`; `mycelium-ts`
`set()` (`agent.ts:340-342`) returns `Promise<void>`. The artifacts verbs already return receipts in both
SDKs (`PublishReceipt`). Versions: py 0.2.4, ts 0.1.1.

**Gap 8.** `provisioning_viz` `HTTP_PORT = 8097` (`examples/coop/src/bin/provisioning_viz.rs:65`);
`guardrail_viz` `DEFAULT_HTTP_PORT = 8097` (`mycelium-guardrails/examples/guardrail_viz.rs:36`, overridable by
`MYCELIUM_VIZ_PORT`). Ports 8090–8100 are all taken; 8101 is free.

**Gap 9.** The live connection set is `GossipAgent::peer_writers: Arc<papaya::HashMap<NodeId, WriterEntry>>`
(`mod.rs:651`), reported as `SystemStats::cached_connections` (`introspect.rs:160`) and swept by the GC
tick (`tasks.rs:1141`). Gauges are set at the point of change (`gossip_store_entries`, `store.rs:774`).
The emergent gauges `peers_known` / `peers_heard` are a different fact (the view, not the sockets).

## 3 · Decisions

- **D1 — an object store is a stem source by URL, staged to disk.** `StemSource::Store { url, stage_dir }`
  → `ObjectStoreFetcher::from_url(url, egress)` inside `DiskStagedSource::open(fetcher, stage_dir)`; the
  ticker stages every catalogue entry before `provision_round`, as the mesh variant prefetches. The CLI
  keeps one flag: `--library <dir | url>` (a URL contains `://`, as `mycelium-artifact` detects it), plus
  `--stage-dir` (default: a temp dir under the placement root) and `--manifest-source <url>` for the
  librarian. Credentials stay in the environment (D13 of the tooling plan); the egress list applies
  (the fetcher already takes it). `stem` does not pull `object_store`; the store path is `--features
  stem,object_store`, and `Dockerfile.stem` builds with it so the suite can use it.
- **D2 — `api_key_env`, resolved at start, refused by name.** A new `ServeDecl.api_key_env:
  Option<String>`; `validate()` refuses both fields set or an empty name; `Stem::start` resolves every
  `[[serve]]`'s key **before** `serve::spawn` and refuses the start when the variable is unset, naming it
  — the house rule since v2.18.1: a setting that would silently degrade refuses instead. `api_key` stays
  for the literal case the docs then call what it is. The key never appears in the trace, the report or
  a log line.
- **D3 — a ranged fetch over the mesh, not the bulk transport.** The bulk transport moves a request the
  wrong way and its reply is frame-bound; a pull is the puller's choice of ranges. New RPC kinds
  `artifact.size` and `artifact.fetch_range` (`id | offset | len`, `len ≤ 4 MiB`, each reply a fraction of
  the frame), served by `serve_artifacts` from any `RangedArtifactSource`; a `MeshRangedFetcher:
  RangedBlobFetcher` over `rpc_call`; `StemSource::Mesh` becomes a `DiskStagedSource` over it, so no blob
  lives in RAM and `pull_to_temp` streams as it already does for a library. Whole-object `artifact.fetch`
  stays for WASM-sized artifacts (one round-trip). A holder re-serves what it staged.
- **D4 — tools are components; the format gains no tool section.** The bridge exists and `[[serve]]` is
  the declared analogue for prompt skills; a `[[tool]]` section would duplicate what `provides.ns =
  "tool"` already says. The four `llm_agent` tools become four `capability-component` fixtures with
  `artifacts/*.toml` descriptions, hosted by `n-0` and `n-1` (`[hosts] kinds = ["wasm-component"]`),
  published by `prepare.sh`, invoked by the driver. **A real schema by self-description:** the bridge
  asks the component `handle({kind: "describe"})` once at install and publishes its `inputSchema` and
  description when it answers, the generic schema when it does not — the manifest line and the
  signature are untouched. The browser demo keeps its in-process tools; the stem cut has none.
- **D5 — an arbiter for same-instant ticks, and the recording's grid.** In replay, when the next recorded
  choice is a *timer* choice for a different stream and this request is also a timer choice, the kernel
  answers *not yet* rather than *divergence*; the sim `Ticker` (and `sleep_ms`) yields and asks again,
  bounded (a tick that never becomes next within the bound is a divergence naming both). Two tasks
  runnable at one paused instant then wake in the recorded order. The replay `Ticker` keeps an absolute
  deadline advanced by its period (`sleep_until`), and mutates its state only after the wait completes,
  so a cancelled tick is re-asked, not skipped. `ChoiceKind::Sched` stays unused: this closes inventory
  row 141 (periodic loops), not row 142 (`select!` branch choice), and the plan says so.
- **D6 — the counter is a value, metrics are optional.** `RefusalCounts` (atomics, by leg and by
  refusal kind) on a `Counting<D: EffectDestination>` wrapper that implements the trait and counts every
  `Err`; `counts()` returns a snapshot. Feature `metrics` (off by default) also increments
  `mycelium_effects_refusals_total{kind, leg}`. The crate's core still couples to nothing.
- **D7 — `set()` returns the receipt it was discarding.** `KvReceipt { operation_id, local_durability,
  local_durability_error }` in both SDKs, the same names the route and `CommitResult` use; additive —
  a caller that ignored `None`/`void` ignores a value. Minor version bumps (py 0.2.5, ts 0.1.2).
- **D8 — `provisioning_viz` moves to :8101.** `guardrail_viz` keeps 8097 (its README and the deck name
  it); the co-op showcases are a block 8090–8098 plus 8101, listed in `examples/README.md`.
- **D9 — `gossip_peers_connected` is set where the writer map changes.** At the GC tick's sweep and on
  the stats path (`SystemStats::cached_connections` and the gauge read the same map), plus a row on
  `metrics.md`.

## 4 · Increments

Each lands as its own PR, test seen failing first and said so in the commit, gated in CI, and moves
its row on `what-is-proven.md` (or its `~` cell) in the same PR. Order is dependency order; Z6–Z9 are
one PR.

| Increment | Delivers | Exit gate |
|---|---|---|
| **Z6–Z9 · the small four** | `Counting<D>` + `RefusalCounts` (+ feature `metrics`); `KvReceipt` from both SDKs' `set()`; `provisioning_viz` on :8101; `gossip_peers_connected` | effects: a refused composed apply counts by leg (`a_refused_composed_effect_is_counted`); py: `test_set_returns_receipt` against a live gateway; ts: `tsc --noEmit` + the receipt type; the gauge mirrors `cached_connections` in a two-node test under `metrics` |
| **Z2 · `api_key_env`** | the field, `validate()`, resolution at start, refusal by name, `unit-file.md` + the limit paragraph rewritten | `a_served_skill_reads_its_key_from_the_environment` and `a_serve_whose_key_variable_is_unset_refuses_the_start` (`stem.rs`), both seen failing; `wire_check` unchanged |
| **Z3 · ranged mesh fetch** | `artifact.size` / `artifact.fetch_range`, `MeshRangedFetcher`, `DiskStagedSource` behind `StemSource::Mesh`, re-serving from the stage | in-process: a 12 MiB synthetic blob pulled over a two-node mesh and installed (`a_blob_past_the_frame_cap_installs_over_the_mesh`, seen failing with `FrameTooLarge`); suite: `model_deploy`'s `model-host` drops `--library /lib` and installs the 19 MB GGUF over the mesh in CI |
| **Z1 · the object-store source** | `StemSource::Store`, the ticker's stage step, `--library <url>`, `--stage-dir`, `--manifest-source`, `Dockerfile.stem` with `object_store`, an `s3mock` service and a `catalog_store` profile in the compose file | `a_store_backed_stem_installs_from_the_bucket` under `object_store,stem` in the `wasm-host` job (S3Mock is already there), seen failing; the `catalog_store` profile in `STEM_DEMOS` with the installer reading `s3://…` and the librarian's manifest from the store |
| **Z4 · tools as stems** | four tool components + descriptions + `prepare.sh`; `n-0`/`n-1` host `wasm-component`; the bridge's `describe` self-schema; the driver invokes `calculate` and `weather` | `stem_driver::llm_agent` asserts `tools/{calculate,weather,search,ping}/{node}` and a `tools/call` on `calculate` returns the arithmetic (seen failing: no tool in KV); `a_tool_component_publishes_its_own_schema` (`runtime.rs`) |
| **Z5 · the arbiter** | `Kernel::decide`'s *not yet* answer for same-instant timer choices, the bounded yield in `Ticker`/`sleep_ms`, absolute deadlines, cancellation-safe ticker state | `tests/decision_trace_replay.rs` gains the replay half under `pause_clock_for_replay` and asserts `compare(..).reproduces` with the trace fully consumed — **run first on the unarbitrated kernel and seen diverging at the 20th choice**; a core pair: two equal-period tickers recorded in one order replay in that order whichever tokio wakes first; the inventory row 141 closes |
| **Z0 · the close** | `what-is-proven.md` with no code gap; the matrix's `~` cells; `CLAUDE.md`; `CHANGELOG`; the wiki; **v2.22.0** | `grep -c "code gap" docs/analysis/doc-coverage.md` → 0; the proof page's third table holds only counterparty, hardware and research lines |

## 5 · What this does not claim

- **Not** a named production deployment, the nightly scale box, performance under load, the joint
  AWS/GCP runs, a real S3/GCS bucket (S3Mock is a mock; S4 stays), or the three-arm experiment's phase
  2. Each needs something other than a commit and stays on the proof page under its own line.
- **Not** `ChoiceKind::Sched` or a `select!` seam: D5 orders ticks due at one instant by the recording;
  which branch of a `select!` wins when two *different* events are ready remains tokio's (inventory row
  142, unchanged).
- **Not** `require_identity_proofs = true` by default — a release decision after a deployment runs
  with it on, recorded as not shown, not as a gap.
- **Not** a tool section in the unit format (D4) or a tool schema in the signed manifest line.

## 6 · Open questions, decided here

- **Q1 — should the store source also serve the mesh?** Yes, as any source does: a store-backed host
  re-serves what it staged through `serve_artifacts`, so one node with credentials can feed peers
  without them. Credentials never travel.
- **Q2 — where does the stage directory live?** Under the placement root by default (`<root>/stage`),
  reclaimed like placed blobs; `--stage-dir` overrides. A staged blob is content-addressed and
  verified on read, so a stale stage is harmless.
- **Q3 — does `api_key_env` reach the gateway's `POST /gateway/units/declare`?** The route refuses
  `[[serve]]` already (422); unchanged.
