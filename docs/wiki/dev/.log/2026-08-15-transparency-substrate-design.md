# ingest — Transparency-Platform substrate design (2026-08-15)

**Recorded:** `docs/design/transparency-council-substrate.md` — the adoption architecture for the
Transparency Platform's council-wiki (UC2, one of mycelium-wiki's two driving use cases). Designed,
**not built**; five-phase build list in the record.

**The decision.** Mycelium is adopted as the coordination fabric for *both* of FTT's scaling
problems — distributed pipeline compute and concurrent agent writers — while **git stays the
datastore**: a control-plane adoption over a `GitStore` data plane (curator election/failover,
proposals, reconcile, broker, MCP), with the curator's apply = a commit scoped `councils/<slug>`.
FTT's hardened git apparatus (boundary commits, run-id tags, restore points, hooks, validator)
survives verbatim.

**How it evolved (three corrections in one conversation, each worth keeping):**
1. First assessment (against the shipped store-as-truth + GitMirror shape): *not* a substrate fit —
   FsStore keeps no history (FTT's recovery model dies), gating moves after-the-fact, ETL volume
   doesn't belong in the KV proposal queue.
2. The multi-writer question flipped it: **group = council** maps exactly onto FTT's hand-enforced
   per-council write domain, and council-wiki is the **first deployment satisfying the GitStore
   eligibility envelope** (E1 public record — with the councillor-GDPR caveat; E2 single curator;
   E3 owned locked remote; E4 git-native readers). So the right data plane is the envelope's
   GitStore, not GitMirror.
3. User correction, accepted: "the pipeline needs parallelising and Mycelium is the obvious
   approach" — the earlier "Mycelium adds nothing to the pipeline" conflated planes. The **write
   plane** must stay serial per council (git index lock + byte-determinism) — that writer IS the
   curator; the **compute plane** parallelises across councils and per-PDF via tuple-space leases,
   with results staged in S3 and handed to the curator as references (payloads never in KV).
   FTT's skip-if-exists idempotency is what makes at-least-once redistribution safe.

**Cross-updates:** envelope-satisfied note in `wiki-git-store.md`; plan status in
`plans/mycelium-wiki.md`; companion page pointer; CLAUDE.md active-work line (the build would double
as Paper 1's production work-distribution case study).
