# External reference — *A Programming Paradigm for Spatiotemporal Composability* (the "Cordis" paper)

**Vendored 2026-09-24**, to satisfy the precondition plan §13.4 set on itself: *"The paper is not in
this repository and no vendored copy exists; if this question is pursued, cite it properly and
vendor the reference first."* This is that citation. It is a **reference note, not an assessment** —
the assessment is [`v3-contracts-axis.md` §13.4](../v3-contracts-axis.md), and the research question
it opens is §13.5.

## Citation

> Yifan Shi¹˒², Wei Zhang¹, Tianyi Cui². **A Programming Paradigm for Spatiotemporal Composability.**
> arXiv:**2608.25512v1**. ¹Peking University · ²DeepSeek-AI. 84 pp.

Retrieved 2026-09-24. **The PDF itself is deliberately not committed** — 2.2 MB of binary in a git
repository that carries none, against a stable, citable arXiv identifier. If a working copy is
wanted in-tree, add it under this directory and say so here.

## Abstract (as published)

> Modern software—from plugin systems to self-evolving agent harnesses—increasingly requires
> *dynamic composition*, yet its formal foundations remain underdeveloped. We identify two
> orthogonal dimensions of the problem: *temporal composability*, the ability to completely revert a
> component's side effects upon removal, and *spatial composability*, the ability to declare and
> reactively manage inter-component dependencies. We address the two dimensions by lifting classical
> effect and coeffect concepts to runtime mechanisms. In particular, we formalize *revertible
> effects*, in which every context transformation carries an inverse that the runtime holds,
> establishing temporal composability local to one component. We formalize *reactive coeffects*, in
> which every context change is classified against a component's coeffect specification to drive its
> activation and deactivation, establishing spatial composability local to one component. We then
> unify the effect context and the coeffect context into a single context type and mediate every
> effect and coeffect through it, yielding a discipline we call the *context paradigm*; the mediation
> induces an observational equivalence up to which the effects of distinct components interleave
> without disturbing one another. Combining these mechanisms into the notion of a *component*, we
> give a calculus of dynamic composition whose metatheory carries spatiotemporal composability from a
> single component to a whole system of interleaved components. We implement these ideas in *Cordis*,
> a meta-framework of spatiotemporal composability that provides a core library with effect tracking
> and coeffect resolution, as well as a declarative component loader with configuration
> reconciliation and hot module replacement.

## The five stated contributions

1. **Revertible effects** (§3.1) — every context transformation carries an explicit inverse held by
   the runtime; tracking and recovery preserve composition. *Local temporal composability.*
2. **Reactive coeffects** (§3.2) — a component declares required coeffects as a specification; each
   context change is classified as activating, deactivating or neutral. *Local spatial composability.*
3. **The context paradigm** (§3.3) — effect and coeffect contexts unified into one type, every
   effect and coeffect mediated through it, inducing an observational equivalence up to which
   distinct components' effects attain independence.
4. **A calculus of dynamic composition** (§4) — components and fibers, orchestration/lifecycle/
   confinement, with a metatheory carrying composability from one component to a system.
5. **Cordis** (§5) — core library (effect tracking, coeffect ops, component lifecycle, context
   access) plus a declarative loader (configuration reconciliation, hot module replacement).

## Section map — for §13.5's classification work

§13.5 asks which of the paper's results *live entirely below the teardown/compensation line*.
Answering it means classifying results by the widest scope their proof requires, so the map matters
more than the prose:

| Section | Pages | Contains |
|---|---|---|
| 1.1 Dimensions of composability | 4 | the temporal/spatial split |
| 1.2 Motivating examples | 4–5 | plugin systems (VSCode); **self-evolving agent harnesses**; the coarse-grained workaround |
| 2 Preliminaries | 7–8 | effects, coeffects, relationship to dynamic composability |
| **3.1 Revertible effects** | 9–15 | effect context, effect functions, effect iterators |
| **3.2 Reactive coeffects** | 16–20 | coeffect context, specification and notification, isolation and interception |
| **3.3 The context paradigm** | 21–25 | unified context; **observational equivalence** |
| 3.4 Attaining independence | 26–30 | effect independence; coeffect commutativity |
| 4.1–4.2 Components, fibers, calculus | 31–38 | orchestration, lifecycle, confinement |
| **4.3 Metatheory** | 39–54 | preservation; **temporal composability**; **spatial composability**; progress; confluence |
| 5 Implementation (Cordis) | 57–69 | core library, component loader, case study (Koishi) |
| 6 Discussion | 70–76 | system boundary; service multiplexing; access control and sandboxing; mutual dependencies and granularity; dependency typing and versioning |
| 7 Related work | 77–81 | effect/coeffect systems; temporal and spatial composability |

The metatheory (§4.3) is where the transfer question bites: **temporal composability** (4.3.2) is the
result whose proof most plausibly requires an inverse that reaches beyond one component, and is
therefore the first to test against §13.4's scope 3. **Spatial composability** (4.3.3) is the one
Mycelium's existing coeffect surface already embodies.

## Why this is in `plans/external/` and not a design record

The assessment already exists and is **not** neutral about the paper — §13.4 adopts the spatial
half as already-shipped, splits the temporal half into three scopes, and **refuses** the third
(*an effect another participant has acted on*) on Promise-theoretic grounds rather than cost. That
refusal is a position about this paper's central mechanism; vendoring the citation here keeps the
position checkable against the source.

Its §13.4 "Not adopted" list stands unchanged: the gossip KV must not become the Cordis context; no
general-purpose inverse accumulator as a missing primitive; not the word *inverse* for any effect
another participant has acted on; and no claim that the composability proofs transfer to an
eventually consistent mesh without new theory.
