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

## Coherence assessment (asked and answered 2026-09-04)

*Do the three imports comply with the philosophy, strategy, and architecture?* **Yes**, with
two tensions named rather than hidden.

- **Reservations — compliant, the cleanest of the three.** The router stays a companion over a
  deliberately load-blind substrate (where the 2026-07-08 addendum bound routing policy to
  live). The reservation is a node's own knowledge composed with the shared medium: never
  gossiped, never a pheromone, no wire change, no core change — the Hayek/local-knowledge
  framing the philosophy rests on, not a global schedule. One flat leaf lock (row 36). Soft
  spot: `reservation_weight = 0.1` is a static default in a codebase that prefers auto-derived
  tuning; if it ever matters it belongs under the tuning governor.
- **The façade — compliant in structure, an adapter in concept.** Not a central proxy: every
  node serves `/gateway/reason/v1` from its own view and the client chooses its entry node —
  the posture of every existing gateway route. Mounted under `/gateway/` so it rides the auth
  boundary (which is how the core gap surfaced). Tension: OpenAI clients expect a raw chat
  model; the served side is a *prompt skill*. The mapping is stated honestly (template-bound
  sampling params, unknown token split) but it is a translation layer, not a native primitive
  — acceptable as the adoption path, and to stay labelled so.
- **The vocabulary + collector — compliant and native.** Typed capability attributes matched by
  `CapFilter` constraints are the substrate's own mechanism; this adds convention, not
  machinery, and the convention lives in the companion (no higher-layer law taught to Layer
  I). Serve-side collection, engine-blind router, feature-gated dependency — the companion
  contract holds.
- **The core change is a bug fix, not a new law.** Auth at the HTTP edge is prevention by
  design (WS1), unlike Layer I's detection-only rule; the fix restores what the docs claimed.
- **Strategy:** the imports strengthen wedge ① without moving the crate's boundary. Position
  recorded in the plan addendum: PAIR is the GPU plane, Mycelium the agent plane; the façade
  is the drop-in path that lets them stack instead of compete. Not imported: pairing PIN,
  installer, licence change.

Open for the next `/mycelium-analysis` (gated on the second clean nightly): score the static
reservation default and the façade's mapping loss; neither is in the ratings yet.
