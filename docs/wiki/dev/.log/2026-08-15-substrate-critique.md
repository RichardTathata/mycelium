# ingest — step-back critique of the five-phase substrate build (2026-08-15)

Applied the session's adversarial discipline to the day's own work: the five phases against
Mycelium principles, FTT's recorded invariants, and the 391-council scale-out target. **Verdict:
mechanisms sound and honestly gated at meeting scale; the design record's "what remains is
deployment" was an overclaim — corrected same day.** Six gaps, three High:

1. **Format ≠ their format (High).** GitStore renders the mycelium page format; council-wiki is
   entity-type kebab-case front-matter + labeled links + row-stores — their REAL validator would
   refuse every page (the Phase-3 gate used a stub). The byte-identical gate compared my writer to
   my writer: **self-consistency, not fidelity**. Fix: a pluggable `PageFormat` codec (P6.5).
2. **Node-independence broken (High).** The checkout is node-local; a ring-promoted curator on
   another node has a stale clone; NO test exercises failover over GitStore. The companion's
   litmus ("failover transfers nothing") silently does not hold. Fix: pull-on-promote /
   push-per-round with the remote as shared truth (P6.3; decision recorded).
3. **Single-branch write ceiling (High @ scale).** ~12 commits/s global under a shared checkout;
   retry(16) turns contention into spurious Conflicts. Largely dissolved by clone-per-node (P6.3)
   + batch commits (P6.1); measured 10-council run gates any scale claim (P6.4).
4. **Per-page commits violate whole-meetings-only (Med-High)** — their crash invariant; fix =
   `write_pages` batch commit, gate refusal becomes whole-batch atomic (P6.1).
5. **Read plane O(subprocess × corpus) (Med-High)** — a query over Edinburgh ≈ 10k spawns; fix =
   persistent `cat-file --batch` (P6.2).
6. **Per-write gate × their 38–90s validator (Medium)** — batch-level gate with the file list
   (their check-pages.sh already takes one) — folded into P6.1.

**What held:** content-hash CAS (merge-correct, per-section independence in one file), scope
isolation by mechanism, the claim-check payload rule, exactly-once across two companions with the
kill at the worst point, the envelope discipline, five CI-green pushes.

**The sharpening (3rd instance of the class this session):** a green gate at toy scale is not
evidence at deployment scale — after "make check-full ≠ CI green" and "a written fuzz gate ≠ a
validated sweep", now "a 3-page-repo gate ≠ a 5,741-file corpus". FTT's own wiki says it best:
"a tool's summary line is not evidence of what it did — the artefact is."

**Plan:** [`docs/plans/council-substrate-hardening.md`](../../../plans/council-substrate-hardening.md)
— P6.1–P6.6 with owners ([M]/[FTT]/[D]), sequencing, and gates.
