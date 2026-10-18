# Changelog — mycelium-reason

All notable changes to this crate. It versions **independently** of the Mycelium substrate (at
2.x) — it is built on the public `mycelium` 2.x API only. Versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.10.0] — unreleased

**Inference routing as policy** (post-360 hardening row R). The router chose a provider by load and reservation, and
nothing could say whether a request *may* go there: the gateway's preflight saw `prompt:llm/{model}@{provider}` and
nothing about where that provider's inference runs or what the request carries.

- **The evaluator sees the destination.** Each candidate's preflight now runs through
  `GossipAgent::gateway_preflight_routed` with `llm/{model}@{provider}` and the provider's declared **locality** —
  the new `llm_meta::LOCALITY` attribute of its `llm-meta/{model}` ad (`in_domain` · `external`; set with
  `ModelProfile::in_domain()` / `external()`; `InferenceRouter::localities` reads them). Undeclared is
  `Locality::Unknown`, which a rule about locality reads as not established, never as in-domain.
- **Per candidate, before the router commits.** A candidate the policy refuses is passed over for the next in the
  router's order, without counting as an attempt, so a classified request is routed to a permitted model when one
  exists and is refused when none does — `RouteError::Refused`'s `data.refused_destinations` names every candidate with
  its locality and reason, `data.data_class` the class. A decision that could not be recorded stops the route. A
  successful `Routed` names the class (`data_class`) and the providers passed over (`refused`); `/route` and the
  façade's `mycelium` block carry both when present.
- **A data classification.** `DataClassPolicy` (classes least to most restrictive): a floor bound to the caller's
  verified principal (`floor_for`, `floor_for_all`), a client declaration in the `x-mycelium-data-class` header
  (`DATA_CLASS_HEADER`) that may raise it and **never lower it**, and for an unclassified request the most restrictive
  class unless `unclassified_as` says otherwise. `InferenceRouter::with_data_classes`, `call_as_classified`, and
  `reason_router_classified` mount it. An unknown class, or a declaration to a router that classifies nothing, is
  `RouteError::Unclassifiable` — `400 unclassifiable` on `/route`, `400 invalid_request_error` / `data_class` on the
  façade — and nothing is routed.
- **A client cannot set a locality, or stand in for a model.** The substrate's `capability/advertise` and
  `units/declare` refuse any `llm-meta` ad from a gateway client (`403 reserved_namespace`); a catalogue entry
  providing one is never installed; and `GET /gateway/rpc/serve/llm.invoke` (with `mcp.invoke`, `skill.invoke`) is
  refused, since a serve stream received every prompt the node was sent — found by the round-1 review. A fleet member
  can still write another node's ad: an ad's key is not bound to its writer (stated, not built).
- **The default class is a floor.** A caller with no floor bound is held at the default (most restrictive) class: it
  may declare higher, never lower. Only an explicitly bound floor sits lower. The header given twice is `400`.
- **No SDK declares a class yet** — `mycelium-py`'s `reason.route()` and `mycelium-ts` have no class parameter, so an
  SDK caller is classified by its floor.
- **One operation id per call.** Every candidate's evidence record for one routed call carries the same
  `operation_id` (`reason:{node}/…`), so the refused candidates and the dispatched one join.
- **Write class policies as allow-lists.** A class rule names one class; the evaluator does not know the classifier's
  order, so a prohibition on `confidential` does not cover a stricter class (guide 20).
- **Residuals, stated:** a provider refreshing its `llm-meta` ad reads as unknown meanwhile (refused, closed); an
  attempt gets the short `failover_timeout` while untried candidates remain even if the policy refuses them all; a
  route where some candidates failed and the rest were refused answers `Exhausted`.
- **Metric.** `mycelium_reason_route_policy_refusals_total` counts candidates passed over by the policy.
- **Evidence.** Each candidate's decision is recorded with the substrate's new `AeEvidence::route`: the class and who
  declared it, the destination and its locality, and the destinations refused before it.
- **Behaviour change with no evaluator:** a request carrying `x-mycelium-data-class` to a router that classifies
  nothing is now `400`, not ignored.
- **Behaviour change for any evaluator:** a per-provider refusal used to answer `403` at the first refused
  candidate; now the router tries the next. A policy that refuses every candidate still answers `403`, with the same
  `reason` and `code` the first refusal carried.
- **Breaking (hence the MINOR):** `Routed` gained `data_class` and `refused`, and `RouteError` gained `Unclassifiable`
  (an exhaustive `match` needs an arm).
- **Not built:** content inspection (the class is declared, never detected); observing a provider's backend (the
  locality is its declaration); classification of the in-process `call` (the node's own action runs no preflight);
  a `[[serve]]` skill's locality (it advertises no `llm-meta` ad, so it is unknown).

## [0.9.0] — 2026-10-12

**Bounded, and every caller-chosen id one segment** (post-360 hardening row H, #609).

- **The blob store is bounded.** `FsBlobStore` held a per-blob ceiling (`MAX_BLOB_BYTES`, 8 MiB) and **no total**: it
  grew without limit under `PUT /gateway/reason/blob` and the mesh write-back. It now holds at most its quota —
  `DEFAULT_BLOB_QUOTA_BYTES` (**1 GiB**) from `open`, or what `FsBlobStore::open_with_quota(dir, bytes)` says
  (`u64::MAX` for no bound; `0` refused by name) — accounted **per digest**: the blob-named files at open plus each new
  digest put since, counted once, reserved before its first write (a put of a digest already on disk or already being
  written reserves nothing) and released by the last failing writer of a digest that never reached the disk. This
  PR's first head reserved per put, so racing puts of one blob grew the account N×len until reopen (#609's review,
  finding 2) — `concurrent_puts_of_one_digest_reserve_it_once` was seen failing 3/3 on it (38.8–50.9 MB counted
  against 16.8 MB on disk). `.tmp-*` files are removed at open as abandoned (one store per directory); a file added or
  deleted by hand is credited at the next open. **Each file costs at least one 4 KiB block** (`BLOB_FILE_MIN_COST`):
  an 8-byte blob takes a block, an inode and an account entry, so a byte-only quota admitted 20,000 files under
  160,000 bytes (the review's second round; `a_tiny_blob_costs_a_block` seen failing at 8). A filesystem with larger
  blocks is under-counted by the difference. A put past the quota is `ErrorKind::QuotaExceeded` with nothing
  written; a blob already held costs nothing; replacing a damaged copy costs the growth. `quota_bytes()` / `used_bytes()` say where a store stands. The gateway answers **507
  `quota_exceeded`** (`{"error","detail"}`); the mesh write-back of a fetched blob that no longer fits is logged and
  the bytes are still returned. Nothing evicts — files are named by content address, so what to drop is the operator's
  call. `reason_node` reads an optional `BLOB_QUOTA_BYTES`.
- **A run id is one bounded segment.** `reason/{run_id}/{node}` is the stream, so a run id carrying `/` wrote into
  **another run's prefix**: a recorder for `victim/x` landed under `log/reason/victim/`, and `replay("victim")` — and
  `anchor`, which seals what replay returns — carried the injected event as the victim's own; an empty or 10 KiB id was
  written as given. `validate_run_id` (non-empty, ≤ `MAX_RUN_ID_BYTES` = 128 bytes, ASCII letters, digits and
  `- _ . : @ +`, not `.`/`..`) now runs at every door: `TraceRecorder::new` returns `Result<_, RunIdError>`; `replay`
  reads only keys of the one shape a recorder writes (`{node}/{hlc}/{salt}` under the run — a deeper key is
  skipped with a warning) and replays nothing under an invalid id; `GET /gateway/reason/trace/{run_id}` and the
  `run_id` of `POST /gateway/reason/route` answer **400 `bad_run_id`**, the façade `400 invalid_request_error` /
  `bad_run_id` with `param: "run_id"` — each before anything is routed or read. **Validated, not owned:** any caller
  holding `llm:invoke` can record into any valid run id; a trace says what was written under a name, not who owns it.
- **Breaking (hence the MINOR):** `TraceRecorder::new` returns a `Result` (`?` or `.expect` at the call site);
  `FsBlobStore::open` is bounded at 1 GiB by default — a node holding more at open refuses the next new put with
  507 until it is opened with a larger quota (an existing file is never touched); `RunIdError` is `#[non_exhaustive]`.
  `FsBlobStore` derives `Debug`.
- Seen failing first: `tests/reason.rs::a_run_id_with_a_slash_cannot_write_into_another_runs_trace` (two events replayed
  under `victim`, one of them mallory's), `tests/gateway.rs::every_run_id_door_refuses_an_id_that_is_not_one_bounded_segment`
  (`GET …/trace/victim%2Fx` answered 200); the quota tests as a missing method. Requires `mycelium` ≥ 2.32.0.

## [0.8.0] — 2026-10-11

**The façade acts as the HTTP client, under the evaluator.** `POST /gateway/reason/route` and
`/gateway/reason/v1/chat/completions` dispatched `llm.invoke` with `rpc_call` — the node's own action — so under
`gateway_caller_profile = secure` the provider saw the gateway node as the caller, not the client whose bearer the
gateway had just resolved, and no `ActionEvaluator` was consulted while `/mcp`, `/a2a` and `/gateway/llm/call`
refused the same call. Both handlers now read the gateway's `ResolvedPrincipal` and route through the new
`InferenceRouter::call_as(caller, enforcement_point, …)`, which runs `GossipAgent::gateway_preflight` per attempt
(`gateway:reason/route`, `gateway:reason/v1/chat/completions`), dispatches with `ServiceHandle::rpc_call_as` and
records the outcome. A denial is `403`: `{"error": "policy", "reason", "code", "detail", "data"}` on `/route`;
`permission_error` / `policy` with a `mycelium: {reason, code, data}` block on the OpenAI envelope.

- **Added:** `InferenceRouter::call_as` (feature `gateway`); `RouteError::Refused { code, reason, message, data }`.
- **Unchanged:** `InferenceRouter::call` — the embedder's in-process route, framed as the node, no preflight.
- **Behaviour:** under the secure profile a provider whose caller-context marker has not reached the gateway is
  refused for a façade call and failed over, as `/gateway/llm/call` does; it used to be called as the node.
- **Breaking:** an exhaustive `match` on `RouteError` needs the `Refused` arm (hence the MINOR).
- **Also:** `#![deny(unsafe_code)]` (post-360 P3; the crate had no `unsafe`).
- Seen failing first: `tests/gateway.rs::the_facade_dispatches_as_the_http_client_not_the_node` (the provider saw
  `node:…`), `the_facade_runs_the_action_preflight` (200, and the provider ran). Requires `mycelium` ≥ 2.32.0.

## [0.7.1] — 2026-10-08

**A refusal is not a corrupt copy** (#564). `MeshBlobStore::fetch` counted every non-empty reply that failed the content
address as corrupt — including the provider's RPC layer answering for it with a caller-context or provider-enforcement
refusal, so a transient condition (a signer key not yet gossiped) ended every retry loop as a non-retriable `corrupt`.
A refusal (a JSON object with an `error`) is now read by its `reason`: one that holds until something changes — a
removed member, a denied action, a malformed, mismatched, unsigned or badly signed envelope — counts as **refused**, and
when every holder that could be asked refused for good the route answers **403 `refused`** (`BlobMiss::Refused`, which
the checkpointer reads as its non-retriable `unauthorized`); any other reason, or none (an older peer), counts as a holder
not yet askable — **503 `unavailable`**, retriable. The content address is checked first, so a blob that is such JSON is
still a blob. Needs the substrate's refusal `reason` (2.28.0); against an older substrate every refusal reads as
transient. A blob fetch miss also logs the providers it asked and each answer (#563). Tests
`an_rpc_refusal_is_not_a_corrupt_copy` (seen failing first) and the classification table.

## [0.7.0] — 2026-10-07

**Why a blob is missing stays distinguishable** (realignment repairs S5, #542). `MeshBlobStore::fetch` returns
a `BlobMiss` — `NotFound`, `Unavailable`, `Corrupt` (corrupt only when every copy currently on offer fails its
content address) — and `GET /gateway/reason/blob/{id}` answers 404 / 503 / 502 with the reason in its body,
where it answered 404 for all three. Damage at rest (a failing hash, an unreadable file) is corruption:
`FsBlobStore::read` returns `LocalRead::Damaged`, the stock blob server answers a damaged copy with
`DAMAGED_REPLY`, and `put` of the right bytes repairs it. **Upgrade order:** `langgraph-checkpoint-mycelium`
0.3.0 before the reason nodes (a 0.2.x checkpointer reads a 503 or 502 as an unrelated HTTP error). A MINOR:
the route's status codes change. `OllamaProbe::with_egress` takes `impl Into<EgressPolicy>`.

0.6.0–0.6.2 are recorded in the repository's root `CHANGELOG.md` (2026-09-04 → 06).

## [0.5.0] — 2026-07-26

Maturity re-version: promoted from `0.1.0` to reflect the shipped tranche (PRs #130–#136,
2026-07-08) — capability-routed inference (`InferenceRouter`), fleet-reasoning traces
(`TraceRecorder` / `replay` / `narrate`), artifact-aware resume (`require_model`), the
content-addressed blob tier + `/gateway/reason/{blob,trace}`, `mycelium.call_typed`, and
`langgraph-checkpoint-mycelium`.

**Stays pre-1.0 deliberately.** The API is not frozen because real not-yet-built work remains that
may still shape it: a real LLM backend beyond `EchoBackend`, chunked blob transfer past the 8 MiB
single-frame ceiling, conversation memory, and run-level evals. 1.0 follows external adoption that
shakes out the routing/trace API.

### Note

No code change — a versioning correction. `0.1.0` under-signalled a crate that is CI-gated (the
`llm` + `llm,gateway` test matrices + clippy + a smoke job), documented (guide chapter 15), and
inside the self-audit scope.
