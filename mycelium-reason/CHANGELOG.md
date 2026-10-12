# Changelog — mycelium-reason

All notable changes to this crate. It versions **independently** of the Mycelium substrate (at
2.x) — it is built on the public `mycelium` 2.x API only. Versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.9.0] — 2026-10-12

**Bounded, and every caller-chosen id one segment** (post-360 hardening row H, #TBD).

- **The blob store is bounded.** `FsBlobStore` held a per-blob ceiling (`MAX_BLOB_BYTES`, 8 MiB) and **no total**: it
  grew without limit under `PUT /gateway/reason/blob` and the mesh write-back. It now holds at most its quota —
  `DEFAULT_BLOB_QUOTA_BYTES` (**1 GiB**) from `open`, or what `FsBlobStore::open_with_quota(dir, bytes)` says
  (`u64::MAX` for no bound; `0` refused by name) — counted from the blob-named files at open plus every byte put since,
  the space reserved atomically *before* a write so concurrent puts cannot overshoot, released if the write fails. A
  put past it is `ErrorKind::QuotaExceeded` with nothing written; a blob already held costs nothing; replacing a damaged
  copy costs the growth. `quota_bytes()` / `used_bytes()` say where a store stands. The gateway answers **507
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
  `bad_run_id` with `param: "run_id"` — each before anything is routed or read.
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
