# The publications corpus — status and framework corrections

↑ [domain/](domain.md) · canonical index: `docs/publications/README.md` (read order, all DOIs, dependency graph — link, don't copy)

**All four papers published (2026-06-23),** each citing its predecessors' real DOIs:

1. **Monetary Ecology** (MCB/P/S/Î framework) — DOI `10.5281/zenodo.20811062`. The citable
   origin of the evaluative axes.
2. **Paper 1 — The Coordinator Trap** — DOI `10.5281/zenodo.20665238` (Zenodo CC BY 4.0 +
   SSRN; SSRN over arXiv is deliberate — reaches the procurement-adjacent audience). Repo
   tag `paper-submission-v2` is cited by name in §8 — re-point it if the text changes.
3. **Paper 2a — HLKS** (heterogeneous local knowledge systems; the convergence argument) —
   DOI `10.5281/zenodo.20813058`. The `mycelium-tuple-space` crate is its constructive
   pull-vs-push evidence.
4. **Paper 2b — The Capture Problem** — DOI `10.5281/zenodo.20813463`.

## Framework corrections (post-review — these supersede any older notes)

- **HLKS tuple is `(A, K, δ, λ, R)`** — R is a *correctness relation* ("adequate relative
  to current joint local state"), NOT an optimal decision function D (D smuggles in a
  planner-computable optimum Hayek's own argument denies).
- **2a claims "structurally homologous", NOT "exact"** — the domains instantiate a common
  problem under HLKS assumptions; convergence is *evidence, not proof*; 2a **sharpens**
  Paper 1's hedge rather than resolving it; the theorem **motivates** (not implies) the
  four properties.
- **2b properties are 5–8:** Capture Resistance, Mandate TTL, Epistemic Symmetry, **Exit**
  (Hirschman).
- Calibration discipline throughout: narrowest defensible claim; tendencies not
  determinism (monetary's `P⤳S`); no single load-bearing overclaimed word.

## Paper 1 related-work landscape (competitive positioning)

Canonical: `docs/publications/paper1/related-work.md` + `references.bib` (link, don't copy).
The scan splits the field on one axis — **who mediates coordination**:

- **Camp A (mediated):** a central agent routes/decides — Cisco *Mycelium* (name clash;
  a `CognitiveEngine` mediator), Solace `OrchestratorAgent`. These are what the Coordinator
  Trap paper argues *against*.
- **Camp B (substrate / coordinator-free) — fellow travellers:** IBM's GEACL + gossip-vision
  (Habiba, *personal-capacity* preprints — a sustained agentic-AI research thread, worth
  monitoring), Terrarium, EvoGit, and `pressure-fields` (Rodriguez). **The differentiator
  is the same for all of them:** none has Mycelium's **receiver-side signal/boundary control
  plane** — they gossip *state* but don't scope *admission*.
- **`pressure-fields` is an ally, not a rival** (full 65-pp read): an *application-level*,
  single-machine artifact-refinement algorithm whose empirical result (mediated control
  applied zero accepted changes in 66.7 % of runs; ~30× worse than stigmergic) independently
  corroborates Paper 1's §2.1 thesis — cited there as `\cite{pressure-fields-decay}`.

Honesty guardrail applied across the section: gmail-authored preprints are flagged as
personal-capacity, not institutional output.

## Pending: the three-arm work-distribution experiment (research-track)

The remaining empirical work for the corpus: a three-arm comparison harness
(coordinator-dispatch vs gossip-KV vs tuple-space pull) feeding Paper 1/2a's latency
figures. Plan: `docs/plans/three_arm_workdist.md`; runners
`examples/three_arm_runner.sh` + `examples/three_arm_plot.py`; pilot data under
`docs/publications/paper2a/data/three_arm/` (one sweep stall-contaminated — see its
README). Engineering-complete substrate; this is measurement + writing.

## Pending: the composition hypothesis (research-track, after the contracts axis' items 3/4/5)

Recorded 2026-09-06 in the plan of record, [`docs/plans/v3-contracts-axis.md` §13](../../plans/v3-contracts-axis.md):
the three-arm harness extended to **four arms** (fixed workflow · orchestrator-led · axis with fixed policies · axis
choosing among explicit arrangements) to test whether composing requirements, voluntary acceptance, scoped mandates,
allocated resources and attributable outcomes lets local participants reorganise work with less manual coordination
and no weakened safety boundary. Must report where the fixed/orchestrated arms win. Plus item 3's behavioural
experiment (evidence-aware resolution vs ordinary resolution under misleading, correlated, stale and contradictory
evidence; errors *and* opportunity costs). Not runnable until `mycelium-knowledge`, `mycelium-control` and the
mandates work exist; the co-op supply-disruption demonstration is the acceptance artefact.

## Pending: evaporative vs inverse-applied undo (research-track, needs no implementation)

Recorded 2026-09-09 in [`docs/plans/v3-contracts-axis.md` §13.5](../../plans/v3-contracts-axis.md): **which
spatiotemporal composability results survive when undo is *evaporative* rather than *inverse-applied*?** A
third-party "context paradigm" paper reduces component unloading to a LIFO stack of runtime-owned inverses;
Mycelium's advertisements instead evaporate at 3× their refresh interval when the refresher stops. The two have
opposite failure modes — an inverse stack runs only if the runtime survives to unload time, evaporation needs
nobody alive — so **the crash case is the discriminator**: a result that holds when the unloading component never
runs is one an evaporating substrate keeps. Method: state both undo models precisely, classify each of the paper's
results, and draw the boundary where the local fiber runtime becomes necessary. No code required, and independent
of the four-arm composition experiment above. **The paper is not in the repository — vendor the reference and cite
it properly before writing.**

## Pending: the "Blood Money → Monetary Ecology" article revision (planned, not started)

Three load-bearing improvements: (1) pull P and S apart as cause→effect (suppress
polycentricity → concentrate surplus; fixes the P notation collision); (2) promote Î to the
*diagnostic* role (opacity = inverted Î; how you detect substrate capture); (3) make
MCB-for-the-voiceless the sharp edge — a substrate only admits agents that can *signal*, so
ecosystems/future generations need a *proxy*, which re-imports the coordinator/capture
problem. Plus the Bitcoin emergent-MCB section and NANDA positioning from
[coordinator-free-recursion](theory/coordinator-free-recursion.md). The Mycelium mapping:
MCB ↔ Layer I/II admission · P ↔ Layer III structure · S ↔ downstream outcome · Î ↔ Layer II
transparency — the article is the monetary instance of the Coordinator-Trap argument.
