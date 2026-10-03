# Capability map

Choose the work you need the fleet to do. These are reader groupings, **not new layers**:
the architecture remains Layer I gossip KV, Layer II signals/boundaries and Layer III consensus.
Capabilities and companions build on those layers. [Choose an integration mode](guide/installation.md)
first; the [example index](../examples/README.md) carries run commands and prerequisites.

| Need | Mechanisms and explanation | Runnable starting point | Operating guidance and limits |
|---|---|---|---|
| Discover participants and signal | [Gossip KV](guide/01-gossip-kv.md), [capabilities](guide/02-capabilities.md), [signals and boundaries](guide/03-signals.md) | [`hello_mesh`](../examples/hello_mesh.rs), [`hello_capability`](../examples/hello_capability.rs) | [Deployment](operations/deployment.md). Eventual views can be stale; admission filters action, not transport dissemination. |
| Coordinate work | [Consensus](guide/04-consensus.md), [pipelines](guide/07-pipelines.md), tuple-space and blackboard | [Fluid pipeline](../examples/fluid_pipeline/README.md), [blackboard](../mycelium-blackboard/examples/README.md) | [Companions](operations/companions.md), [admission control](operations/admission-control.md). Quorum, durability and single-writer assumptions differ by operation. |
| Run tools and reasoning | [Skills](guide/05-skills.md), [MCP](guide/06-tool-discovery.md), [A2A](guide/08-a2a-interop.md), [reason and LangGraph](guide/15-reasoning-and-langgraph.md) | [Chat](../examples/chat/README.md), [LangGraph ladder](../examples/langgraph/README.md) | [Observability](operations/observability.md). Model output quality is separate from routing correctness. |
| Coordinate by meaning | [Semantic coordination](guide/11-semantic-coordination.md): agents that self-organise by what they mean, not by address | [`semantic_coordination`](../examples/semantic_coordination.rs) | [Guardrails](guide/16-guardrails.md) for the structural limits; the [threat model](threat-model.md) on what a shared meaning space does not authenticate |
| Acquire and adapt capabilities | [Wasm host and stem](../mycelium-wasm-host/README.md): signed catalogue, demand, presence floors, shadow acceptance, activation and serving | [Co-op provisioning and deployment examples](../examples/coop/README.md) | [Capability lifecycle](operations/capability-lifecycle.md), [artifacts](operations/artifacts.md). Configured hosts install approved artifacts; activation is not a sandbox; object-store coverage is bounded. |
| Constrain actions | [Authority](guide/20-authorising-actions.md), [mandates and resource fences](guide/21-mandates.md), [guardrails](guide/16-guardrails.md), [stability and budgets](guide/22-stability-and-control.md) | [`authority_drain`](../examples/authority_drain.rs), [`composed_commit`](../examples/composed_commit.rs), [`control_envelope_viz`](../examples/control_envelope_viz.rs) | [Confinement](operations/confined-fleet.md), [control profiles](operations/control-profiles.md). Requires configured enforcement and feature support; unit declarations alone do not enforce authority. |
| Connect trust domains | [Federation](guide/17-federation.md), [AgentFacts](../mycelium-agentfacts/README.md) | [`federated_domains`](../examples/federated_domains.rs), [`federation_trust_is_not_transitive`](../examples/federation_trust_is_not_transitive.rs) | [Federation runbook](operations/federation.md). Public discovery does not grant mesh admission or transitive authority. |
| Inspect outcomes and reproduce | [Contracts and receipts](guide/18-contracts-and-receipts.md), [replay](guide/19-replay-and-simulation.md), [commitments](guide/24-commitments.md) | [`receipt_ladder`](../examples/receipt_ladder.rs), [simulation examples](../mycelium-sim/examples/) | [Audit](operations/audit.md), [diagnostics](operations/diagnostics.md). Receipt rungs differ; a journal is not a replay bundle, and replay covers captured seams. |
| Maintain knowledge and disagreement | [Knowledge](guide/23-knowledge.md), [wiki](../mycelium-wiki/examples/README.md) | [`knowledge_layer`](../examples/knowledge_layer.rs), [wiki examples](../mycelium-wiki/examples/README.md) | [Companion operations](operations/companions.md). Authored judgements and provenance do not establish the truth of a claim. |

## Evidence and next steps

Every row inherits the feature, configuration and demonstration bounds in the dated
[what-is-proven ledger](operations/what-is-proven.md); examples demonstrate specific scenarios,
not universal guarantees. Read the [threat model](threat-model.md) before treating a boundary as
protection against a hostile participant. The [guarantees and rule-catalogue record](plans/guarantees-and-rule-catalogue.md) is delivered end to end — the registry, startup report and profiles (v2.19.0), the rule catalogue and the decision trace (v2.20.0); what a decision-level replay of a whole agent cannot yet show is stated on [what-is-proven.md](operations/what-is-proven.md). A profile validates its stated conditions; it is not a universal security guarantee.

[Prospect: scope a pilot](operations/customer-pilot.md) ·
[Developer: build an integration](guide/building-on-mycelium.md) ·
[User/operator: run the fleet](operations/README.md) ·
[Researcher: reproduce and challenge](publications/research-guide.md)

Maintainers: update this map when a capability changes. Keep API facts in the linked guide/code,
run instructions in the example index, and evidence status in the ledger; do not copy changing
release counts or benchmark results here. `scripts/check-positioning.sh` checks the shared proposition and developer path;
`scripts/check-materials.py` checks capability coverage, audience routes and local resource targets.
