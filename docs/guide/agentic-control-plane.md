# Mycelium and the enterprise agentic control plane

*Which slot Mycelium fills in the emerging "control plane for enterprise AI agents" architecture —
what it provides out of the box, what deliberately stays on your side of the line, and how it
composes with a deterministic workflow engine. Written 2026-08-18 against a representative
framing of the problem ([Mazumder, "Scaling AI in the Enterprise"](https://medium.com/@smazumder_20424/series-overview-scaling-ai-in-the-enterprise-ce89b96c3e74));
the mapping holds for the genre, not just that series.*

## The two-layer framing, and Mycelium's slot

The enterprise consensus is converging on a two-layer shape:

- a **macro layer** — a deterministic business-workflow engine (Temporal, Camunda, Step
  Functions) that owns SLAs, immutable business rules, and compliance-critical sequencing;
- a **micro layer** — bounded agent work executing inside workflow steps, where the criticism of
  LangGraph-class tools is that they orchestrate tasks but carry **no enterprise durability,
  authority, or audit guarantees**.

**Mycelium is a micro-layer substrate, and does not compete with the macro engine.** It is an
embedded library the agent fleet runs on: a workflow step in your macro engine drives
Mycelium-coordinated work through the [HTTP gateway](09-security.md) or the
[Python/TypeScript SDKs](10-language-bridges.md), and what the step gets underneath is bounded
authority, lease-contained execution, and an audit stream — the properties the micro layer is
usually missing. There is no Mycelium daemon or control-plane server in the deployment picture;
a cluster is emergent from network reachability plus CA admission.

One caveat we argue rather than hide: for large *heterogeneous* fleets, routing everything
through the central engine eventually hits an epistemic limit — the engine cannot know what the
fleet knows (`docs/philosophy.md`, the Coordinator Trap). The composition we recommend keeps the
deterministic engine for the genuinely invariant compliance paths and lets bounded, observable
coordination emerge underneath it — not a central spine for everything.

## The five control-plane pillars, mapped

The genre's control plane has five recurring pillars. For each: what Mycelium ships today, and
what the adopter owns — the same shared-responsibility shape as the
[SOC 2 matrix](../operations/shared-responsibility-matrix.md).

| Pillar | Mycelium provides (shipped) | Adopter owns |
|---|---|---|
| **1 · Approval gates** | The *gate points*: pre-commit write gates that refuse a whole batch with findings (`mycelium-wiki` `validate_cmd`), membership-gated store access, consensus quorums for decisions that need agreement (`cluster_propose`/`group_propose`) | The approval *workflow*: risk-based routing, human escalation UI, who approves what |
| **2 · Scoped permissions** | The capability/requirement model with **TTL-decaying advertisements** (`src/capability.rs`) — grants are scoped and expire rather than persisting; CA-gated cluster admission (`tls`); signed identity proofs (`require_identity_proofs`, v2.3); scoped signals `Cluster · Group · Individual`; governed groups; gateway RBAC/OIDC; fail-closed `EgressPolicy` on outbound paths | Mapping *business* roles onto capabilities; credential custody; which operations demand which scopes |
| **3 · Audit trails** | `AuditSink` export with retention checkpointing (v2.3); OTEL span export from `SkillRunner` (`otel` feature); consensus records for committed decisions; the wiki knowledge plane's per-round provenance (curated commits, [git mirror or git-as-truth](../operations/companions.md)) | Shipping the stream to *your* SIEM; retention policy; tying spans to business case IDs |
| **4 · Rollback / compensation** | **Not a saga engine — by design.** What exists underneath: at-least-once redelivery × idempotent apply = exactly-once *effect* (`docs/design/exactly-once-effect.md`, tested through worker death); claim-check resubmission is a safe no-op; `LockService` for mutual exclusion | Cross-system compensation logic (the saga) — this belongs in the macro engine, which is exactly what it is good at |
| **5 · Quality monitoring** | Detection machinery: the curator's structural/semantic lint loop, tripwires + counters (violations made legible, never silently prevented), `/metrics`, the guardrails companion (`mycelium-guardrails`) | LLM-as-judge sampling, CI/CD quality gates, regression loops — application-layer concerns |

The asymmetry in the table is the point: **pillars 2 and 3 are substrate properties and Mycelium
ships them; pillars 1, 4, and 5 are workflow opinions, and a library that baked them in would be
imposing one enterprise's process on every deployment.** Runaway-loop containment — the genre's
headline fear — lands between the pillars: a tuple-space worker holds a **lease**
(`worker_timeout_secs`), so a runaway or dead agent's work times out and requeues instead of
wedging the pipeline, and idempotent apply makes the retry safe.

## Adoption triggers

Do not adopt from an article — adopt when one of these fires in a real deployment:

- a macro-engine step needs to fan work across a **heterogeneous agent fleet** and the current
  tool gives you no authority boundary between agents;
- an audit or compliance review asks **"which agent decided this, under what grant, and who saw
  it?"** and the current stack cannot answer from records;
- a runaway loop or dead worker has actually **wedged a production pipeline** and the fix on the
  table is another timeout hack.

Start with the [FAQ](faq.md) ("is this for me?"), then [Building on Mycelium](building-on-mycelium.md);
the [cookbook](cookbook.md) has the task-by-task recipes.
