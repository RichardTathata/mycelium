# Mycelium documentation

Start with your task, then use the reference areas below. The [capability map](capabilities.md)
connects what Mycelium can do to explanations, runnable examples, operating guidance and limits.

**New developer?** Follow the [six practical tutorials](guide/tutorials/README.md)
after the guide’s introductory gossip and capability examples.

| Audience | Suggested route |
|---|---|
| **Prospect** — does it fit our problem? | [Buyer deck](publications/customer-pitch.html) → [capabilities](capabilities.md) → [evidence and limits](operations/what-is-proven.md) → [bounded pilot](operations/customer-pilot.md) → [engagement kit](operations/engagement-kit.md) |
| **Developer** — how do I build with it? | [Installation and integration mode](guide/installation.md) → [first examples](../examples/README.md) → [integrator contract](guide/building-on-mycelium.md) → [guide](guide/README.md) → [operational handover](operations/production-readiness.md) |
| **User/operator** — how do I run and understand it? | [Operations](operations/README.md) → [deployment](operations/deployment.md) → [readiness](operations/production-readiness.md) → [observability](operations/observability.md) → [diagnostics](operations/diagnostics.md) → [recovery](operations/deployment.md#backup--restore) |
| **Researcher** — what is the contribution and evidence? | [Research guide](publications/research-guide.md) → [papers](publications/README.md) → [reproduction and limits](publications/research-guide.md#reproduce-a-result) → [open questions](publications/research-guide.md#open-questions) |

Mycelium is an infrastructure library. Its users operate an application, gateway or stem fleet;
it does not provide a single end-user product workflow. Application-specific user instructions
belong with that application, supported by these substrate runbooks.

> Maintainers and coding agents: start at the [wiki](wiki/wiki.md). Code is canon;
> the wiki synthesises it using the [wiki schema](wiki/AGENTS.md).

## Root anchors

The documents you read to understand the system's *position* — referenced from
across the tree, deliberately at the root rather than filed under one area.

| Doc | The stance it anchors |
|---|---|
| [`philosophy.md`](philosophy.md) | **Purpose** — what Mycelium *is* and why (the coordinator-free thesis). The authoritative definition of intent. |
| [`sovereignty.md`](sovereignty.md) | **Positioning** — the coordinator-free thesis one level up: the sovereignty of an *organisation* against a *platform*. Philosophy's four Failure Modes transposed, federation as the mechanism, and why sovereignty and legibility are one argument. §4 is the case against. |
| [`threat-model.md`](threat-model.md) | **Security posture** — the crown-jewel blast-radius model: what an attacker gains at each trust boundary, the mitigations, the residual risk an operator owns. A standing posture (updated as the system evolves), not a point-in-time decision. |

## Reference areas

| Area | Document *type* | What lives here |
|---|---|---|
| [`guide/`](guide/) | **Tutorial** — learn the system | The developer guide, chapters [00–24](guide/README.md) (concepts → gossip/KV → capabilities → … → stability & control → knowledge → commitments), plus the [cookbook](guide/cookbook.md) ("how do I…?"), [error-handling](guide/error-handling.md) and [deprecations](guide/deprecations.md) (the adopter-facing `3.0.0` removal ledger). Start at [00 · Concepts](guide/00-concepts.md). |
| [`operations/`](operations/) | **Runbook** — operate the system | DevOps + Solution/Dev runbooks — **start at the [operations index](operations/README.md)** ("Start here" funnel): the [go-live checklist](operations/production-readiness.md), [deployment](operations/deployment.md), [observability](operations/observability.md) + the [metrics reference](operations/metrics.md), [diagnostics](operations/diagnostics.md), [dynamic-scaling](operations/dynamic-scaling.md), [tuning](operations/tuning.md), [artifacts](operations/artifacts.md), [companions](operations/companions.md), [rbac](operations/rbac.md), [sso](operations/sso.md), [audit](operations/audit.md), [cert-rotation](operations/cert-rotation.md), [crown-jewel](operations/crown-jewel.md). |
| [`design/`](design/) | **Decision record / ADR** — why a design choice | Point-in-time decisions and binding contracts. Use the [capability map](capabilities.md) and linked guide chapters for current behaviour; consult an ADR for rationale and assumptions, and its implementation record for delivery status. |
| [`plans/`](plans/) | **Plan** — proposed work and its implementation record | Status belongs to each plan; a plan is not evidence that a feature shipped. |
| [`reference/`](reference/) | **Manual** — *how to use* a feature | Reference docs: the [configuration reference](reference/configuration.md) (every `GossipConfig` field: its consumer, the feature it needs, where a bad value is refused, restart, default and env var), the [unit-file reference](reference/unit-file.md) (every section and field of a unit file), the generated [rule catalogue](reference/rule-catalogue.md) (every described decision point: trigger, inputs, typed outcomes, guards, relations, code and tests), the generated [guarantee catalogue](reference/guarantee-catalogue.md) (every guarantee the startup report resolves, and a state matrix over reference configurations), the [declaration schema](reference/declaration.schema.json) (`mycelium wire-check --format json`), the [SkillRunner manual](reference/skillrunner.html). |
| [`publications/`](publications/) | **Papers + decks** — the research & pitch track | One directory per paper: [`paper1/`](publications/paper1/) — "The Coordinator Trap" ([paper.md](publications/paper1/paper.md) + LaTeX source); [`paper2a/`](publications/paper2a/) — the substrate-convergence paper (LaTeX + [working draft](publications/paper2a/substrate_convergence.html)). Plus two decks — the architecture/strategy [presentation](publications/presentation.html) (engineer-facing) and the buyer-facing [customer pitch](publications/customer-pitch.html) (value/sovereignty-led). Rendered PDFs are derived, not tracked. Kept honest against shipped reality by the `publication-lint` skill (claims-vs-code + overclaim/framing checks). |
| [`analysis/`](analysis/) | **Audit series** — the project's own report card | **Start at [`analysis/README.md`](analysis/README.md)** — *how Mycelium audits itself* (the self-audit system + calibration ledgers, for technical reviewers). [`ratings.md`](analysis/ratings.md): the periodic 25-dimension M2 self-audit (execution-evidence-gated, with a calibration ledger that scores the scores). Run via the `mycelium-analysis` skill. Plus [`doc-coverage.md`](analysis/doc-coverage.md): the WHAT/WHY/HOW × Dev/Ops documentation-coverage audit (matrix + remediation; a re-run diff target), run via the `doc-coverage` skill. |
| [`wiki/`](wiki/wiki.md) | **Synthesis** — the LLM-maintained knowledge base | The current-state, cross-linked wiki compiled from all the areas above (Karpathy LLM-wiki pattern): [`dev/`](wiki/dev/dev.md) (architecture invariants, concurrency discipline, testing lore, security, companions, delivery history) + [`domain/`](wiki/domain/domain.md) (coordinator-free theory, publications, strategy). Schema: [`AGENTS.md`](wiki/AGENTS.md). Query-first for agents; linted via the `wiki-lint` skill. |

## Which area answers which question?

- *"What is this / why coordinator-free?"* → `philosophy.md`, then [guide 00](guide/00-concepts.md).
- *"How do I build with it?"* → [`guide/`](guide/) + the [cookbook](guide/cookbook.md).
- *"How do I deploy / monitor / scale / secure a cluster?"* → [`operations/`](operations/).
- *"Why was X designed this way (and not Y)?"* → [`design/`](design/) (decisions) + [`plans/`](plans/) (the execution record's reasoning).
- *"What's the security blast radius?"* → [`threat-model.md`](threat-model.md).
- *"Why run our own fleet rather than a platform's?"* → [`sovereignty.md`](sovereignty.md).
- *"How do I use feature Z?"* → [`reference/`](reference/) (+ the relevant guide chapter).
- *"What's the research basis / is it sound?"* → [`publications/`](publications/) + [`analysis/`](analysis/).
- *"What's the current reconciled state of X?" (agent onboarding)* → [`wiki/`](wiki/wiki.md).

> Architecture invariants and the on-ramp for code-assistant sessions live in the
> repo-root [`CLAUDE.md`](../CLAUDE.md); the milestone *design* home is
> [`ROADMAP.md`](../ROADMAP.md). These docs are the elaboration, not a duplicate.
