# ingest — the NVIDIA PAIR comparison → three imports into mycelium-reason 0.6.0, + a core auth gap (2026-09-04)

Prompted by a comparative analysis of NVIDIA's Personal AI Router (announced 2026-09-03) against
Mycelium. Verified its Mycelium claims in code first; position + bindings recorded in
`docs/plans/mycelium-reason.md` (addendum of this date).

- **Two over-credits in our own story:** the load signal (`fill_ratio`, a self-reported channel
  fill) is no richer than PAIR's pending-job count — only the *constraint* vocabulary was richer;
  and the `(fill, id)` rank gave N concurrent callers the same provider until the pheromone caught
  up (the herd PAIR's reservations exist for).
- **Imported:** local in-flight reservations (node-local, never gossiped — lock-order row 36;
  herd gate fails pre-fix) · the OpenAI-compatible façade under `/gateway/reason/v1` (under the
  auth boundary on purpose; one shared router so reservations span requests) · the `llm_meta`
  vocabulary + `refresh_meta` (retraction *observed* before re-advertise; the theorised
  tombstone-vs-first-write LWW race did **not** reproduce — 60 naive flips, no blink — so the
  sequencing is explicitness over a tokio ordering, recorded as such, not a fix) + the `ollama`
  collector and `ollama_serve` example.
- **Verification discipline held:** every new gate was run against the pre-fix behaviour —
  the core auth test (fails), the herd gate at `reservation_weight = 0.0` ≡ old rank (fails),
  the refresh gate against the naive refresh (passes → claim downgraded, above).
- **Found on the way — core:** routers merged via `with_http_routes` were outside the gateway
  auth `route_layer`; every companion's `/gateway/…` surface answered without a bearer while the
  companion docs claimed coverage. Fixed (prefix-guarded layer on merged routers), gated in core +
  reason, ledger entry (Security 8 at Run 59). Lesson: **a doc claim about a security boundary is
  not evidence** — the audits verified the policy and the library's routes and took the
  companions' claim as read; no test ever sent a companion route a bare request.
- **Not imported:** pairing PIN / installer (product, not library); the licence question is the
  owner's.
