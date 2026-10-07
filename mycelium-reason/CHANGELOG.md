# Changelog — mycelium-reason

All notable changes to this crate. It versions **independently** of the Mycelium substrate (at
2.x) — it is built on the public `mycelium` 2.x API only. Versioning follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

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
