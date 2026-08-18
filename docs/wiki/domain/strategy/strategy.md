# domain/strategy — the commercial position

↑ [domain/](../domain.md) · full strategy doc: `docs/internal/commercial.html` (local-only, gitignored)

- **[licensing-and-compliance.md](licensing-and-compliance.md)** — the three-tier model,
  the vendor-assurance moat (corp/SDLC SOC 2 + pentest + control-mapping evidence — **not** a
  runtime certification of the library), beachhead sequence, SI dynamics.
- **[production-readiness.md](production-readiness.md)** — the four sub-gates regulated
  buyers actually probe (now shipped; kept as the framing for those conversations).
- **[deployment-framing.md](deployment-framing.md)** — library, not platform
  (user-confirmed framing; binding on all docs).

The two moats compound: epistemic architectural correctness
([coordinator-trap](../theory/coordinator-trap.md)) + the vendor-assurance apparatus (which
accrues only over calendar time — corp SOC 2, pentest history, executed BAAs/DPAs, references).
By the time a competitor assembles that apparatus, Mycelium has reference customers, a published
corpus, and production learnings — the gap compounds rather than closes.

## The enterprise agentic-control-plane slot (2026-08-18)

The enterprise discourse (Temporal-class macro engine + a trustworthy micro layer) has a named
slot Mycelium fills without stretching: the micro-layer substrate carrying bounded authority,
lease-contained execution, and the audit stream — the properties the genre says LangGraph-class
tools lack. The pillar-by-pillar mapping (ships vs. adopter-owns, same shape as the SOC 2
shared-responsibility matrix) is the front-door doc
[`guide/agentic-control-plane.md`](../../../guide/agentic-control-plane.md); it also states the
composition posture (drive Mycelium from a workflow step via gateway/SDKs; never sell Mycelium
as a BPM engine) and the adoption triggers (no adoption from articles — a driving deployment
pulls any new surface).
