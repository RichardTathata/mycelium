# ingest — the agentic-control-plane mapping doc (2026-08-18)

Prompted by an enterprise-scaling series (Mazumder, Medium): assessed Mycelium against the
two-layer (deterministic macro engine + bounded micro layer) framing and its five control-plane
pillars. Verdict recorded as a front-door guide doc `docs/guide/agentic-control-plane.md`:
pillars 2 (scoped permissions) + 3 (audit) ship today; loop containment is structural
(tuple-space leases × idempotent apply); pillars 1/4/5 are deliberately adopter-side
(library-not-platform — the saga belongs in the macro engine); plus the philosophy's
counter-position on central spines. All capability claims verified against code before landing
(otel feature, worker_timeout_secs, require_identity_proofs, OIDC, governed groups,
exactly-once-effect design record). Strategy page gained the slot section; guide README indexed.
