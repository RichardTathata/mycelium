# Mycelium examples

Use the [capability map](../docs/capabilities.md) to connect this material to other mechanisms, operating guidance and evidence.


Every example is a real, runnable program built on the **public API** — no private hooks. This page
is the index, the shared setup (so no example re-explains it), and the **doc template** all example
READMEs follow.

**One grid, three ways to read it.** The [capability matrix](#the-capability-matrix) below fingerprints
every example by the stack **layer** it teaches *and* its facets — how deep (*Level*), how you watch it
(*Surface*), whether it needs a model (*LLM*), and which operational surfaces it lights up (*Audit*,
*Metrics*). Scan **down a column** to filter ("show me the browser demos", "the zero-LLM ones"), or read
**across a row** to characterize one example at a glance. New here? Start at the **Intro** rows and climb.

> **Colour-coded, scannable, opens offline:** [`docs/wiki/dev/examples-layer-matrix.html`](../docs/wiki/dev/examples-layer-matrix.html)
> is the same matrix rendered — layer dots + facet chips + a per-layer summary strip.

## Recommended paths

**Start here — five steps**, the same five on every front door ([`docs/positioning.md`](../docs/positioning.md)):

<!-- path:start -->
1. **[`hello_mesh`](hello_mesh.rs)** — two embedded agents share state by gossip: 30 seconds, no setup.
2. **[`hello_capability`](hello_capability.rs)** — one node says what it does, another finds it by name and calls it: no registry, no addresses.
3. **[Your first stem fleet](../docs/guide/tutorials/01-first-stem-fleet.md)** — generic hosts discover and install a signed echo component, answer a call, and restore a declared provider floor after graceful removal.
4. **[guide 20](../docs/guide/20-authorising-actions.md) with [`authority_drain`](authority_drain.rs)** — what an agent may do, checked where the work happens, and stopped when its authority lapses.
5. **[`what-is-proven.md`](../docs/operations/what-is-proven.md)** — what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- path:end -->

**Learn the newer surfaces:** [six developer tutorials](../docs/guide/tutorials/README.md) — stem fleets,
declarations, shadow acceptance, authority boundaries, replay, and declared versus observed.

The matrix below is complete and is the architect's view. These five paths are the integrator's:
each answers *which example fits my situation*, *what must I change*, and *how do I know it worked*.
Every step is a run doc that follows the **tutorial contract** at the end of this section.

| Reader goal | Sequence | Finish with |
|---|---|---|
| **Understand the substrate** | [`hello_mesh`](../docs/guide/01-gossip-kv.md) → [`hello_capability`](../docs/guide/02-capabilities.md) → [`stigmergy`](coop/README.md) | change a capability's advertisement and watch discovery converge |
| **Integrate an existing application** | a [Python](a2a_langchain/README.md) or [TypeScript](../mycelium-ts/README.md) client, or [`hello_mesh`](../docs/guide/01-gossip-kv.md) in Rust → your tool through MCP ([`mcp_tool_authority`](../docs/guide/20-authorising-actions.md)) or A2A ([`a2a_skill_authority`](../docs/guide/20-authorising-actions.md)) → the authority check refusing a call | one customer function runs through a named, observable boundary, and one refused call proves the boundary is there |
| **Demonstrate adaptive operations** | [`provisioning_viz`](coop/README.md) → kill the active node → recovery → [`diagnostics`](coop/README.md) | recovery *and* the evidence of its limits, on one screen |
| **Demonstrate governed autonomy** | [`procurement_authority`](coop/README.md) → a real gateway with an evaluator ([`mcp_tool_authority`](../docs/guide/20-authorising-actions.md)) → resource-side enforcement ([`authority_drain`](../docs/design/authority-at-execution.md)) → the effect refused where it commits ([`composed_commit`](../docs/design/composed-effect.md)) → the consumer's evidence | permission, execution and outcome visibly distinguished; a bypass evidenced |
| **Operate across organisations** | [`federated_domains`](../docs/guide/17-federation.md) → disconnect → reconnect → revoke → [`federation_trust_is_not_transitive`](../docs/guide/17-federation.md) | independent domains stay independent, and a revoked partner stays revoked |

**By outcome** (the matrix's layer dots say *what a demo exercises*; these say *what it shows*):
**recovery** — `provisioning`, `reheal_deploy`, `rotation`, `diagnostics`, `curator_handover` ·
**authority** — `procurement_authority`, `mcp_tool_authority`, `a2a_skill_authority`, `authority_drain`, `composed_commit`,
`coordinator_by_accretion` · **evidence** — `procurement_authority` (acts 5–6), `auditor_questions`,
`receipt_ladder`, `replay_a_bundle` · **domain isolation** — `federated_domains`,
`federation_trust_is_not_transitive`, `federation_facts`. A demo that is all-· in the matrix
(`replay_a_bundle`, `curator_handover`) is not exercising *nothing*; it is exercising a contract that
cuts across the layers, which is what these tags are for.

**The tutorial contract** — what a run doc on a recommended path carries, in this order, and
nothing the API reference already says: *prerequisites* · the *exact pinned command* · the *expected
visible result* (the line to look for) · *key code* locations · one *exercise* · one *induced
failure* · what the result *proves* · what it *does not prove* · *cleanup* · the *adaptation* to
customer code · and, where the example declares anything, its *units* — the declaration directory
under `examples/units/`, which is part of the example. The co-op README's entries 12 and 13 are the shape; the rest of the suite READMEs
are being brought to it path by path.

**For a multi-demo presentation:** every visual demo has its own default port (below); the two that
used to share `:8096` (`control_envelope_viz`, `guardrail_viz`) now differ and honour
`MYCELIUM_VIZ_PORT` to move either. Check the port column before opening more than one.

## The capability matrix

**Layers:** ● primary · ○ also exercises · · none — **I** gossip-KV (state) · **II** signal-mesh
(events, opacity) · **III** consensus · **IV** capability/agent. **Facets:** *Level* Intro/Adv (★ flagship) ·
*Surface* Web (browser UI) / CLI · *LLM* real (needs a model) / mock (echo, no key) / · none · *Audit* ✓
emits a signed tamper-evident trail · *Metrics* ✓ built with the Prometheus recorder (the Ops Console
**Metrics** tab climbs live) · *CI* ✓ **executed** on every change (the root `cargo run` gallery in
`ci.yml`, a suite's `ci_smoke.sh`, the coop smoke, the Docker cluster suites, or the LangGraph rungs and the
reason nodes they run against) — **checked both ways**: `scripts/check-example-matrix.py` (in `make check` and
CI) finds, for every ✓ row, a CI step that executes it, and fails on an example CI executes whose row says ·
or that has no row; each `cargo run … --example` in `ci.yml` goes through `scripts/example-case.sh`, so the
`test-coverage` job sees it as a case that executed;
*Declared* ✓ the example has a **declaration directory** — `examples/units/<example>/`, its units in the
`src/capability_config.rs` format, checked offline by `mycelium wire-check` in CI
(`scripts/wire-check-examples.sh`; the `*_viz` variants share their base's directory) ·
· compiled in CI but **not executed** — every browser demo and every manual one. Two group headers
used to say "all run in CI" in prose; they did not (`coordination_viz`, `control_envelope_viz`,
`destination_commit` were not run), and a claim per row is checkable where a claim per group was not.

**The groups, in one line each.** *The v3 contracts axis:* one decisive demonstration per item —
`replay_a_bundle` is all-· by construction because the replay kernel is not a layer. *Coordination &
identity integrity:* an election needs an electorate, winning is not a grant, one node quietly holding
everything reads as healthy, and a node's identity travels as one record. The rest name themselves.

Each example name links to its **run doc** (a README or guide chapter that tells you how to start it) —
not to raw source. The suite READMEs carry the per-example walkthrough + the exact command.

| Example | I | II | III | IV | Level | Surface | LLM | Audit | Metrics | CI | Declared |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| **Start here** — the zero-setup ladder, one file each | | | | | | | | | | | |
| [`hello_mesh`](../docs/guide/01-gossip-kv.md#the-example) · [src](hello_mesh.rs) | ● | · | · | · | Intro | CLI | · | · | · | · | · |
| [`hello_capability`](../docs/guide/02-capabilities.md#02--capabilities-find-nodes-by-what-they-do) · [src](hello_capability.rs) | · | · | · | ● | Intro | CLI | · | · | · | · | [✓](units/hello_capability/) |
| [`first_stem_fleet`](../docs/guide/tutorials/01-first-stem-fleet.md) · [src](../mycelium-wasm-host/examples/first_stem_fleet.rs) | ○ | · | · | ● | Intro | CLI | · | · | · | ✓ | · |
| [`compare_stem_observations`](../docs/guide/tutorials/06-declared-versus-observed.md) · [src](compare_stem_observations.rs) | · | · | · | · | Intro | CLI | · | · | · | ✓ | · |
| [`conway`](../docs/guide/01-gossip-kv.md#the-example) · [src](conway.rs) | ● | ○ | · | · | Intro | Web | · | · | ✓ | · | · |
| [`distributed_lock`](../docs/guide/04-consensus.md#the-distributed-lock-service) · [src](distributed_lock.rs) | · | · | ● | · | Intro | CLI | · | · | · | · | · |
| [`invoke_skill`](community/README.md#manual) · [src](invoke_skill.rs) | ○ | · | · | ● | Intro | CLI | · | · | · | · | · |
| [`semantic_coordination`](../docs/guide/11-semantic-coordination.md#the-problem-type-erased-coordination) · [src](semantic_coordination.rs) | · | ● | · | ○ | Intro | CLI | · | · | · | · | [✓](units/semantic_coordination/) |
| **Top-level** — beyond the ladder | | | | | | | | | | | |
| [`llm_agent`](../docs/guide/02-capabilities.md#the-example) · [src](llm_agent.rs) | ○ | ○ | · | ● | Adv | Web | mock | · | ✓ | · | [✓](units/llm_agent/) |
| [`coordinator_comparison`](../docs/plans/three_arm_workdist.md#outcome-metrics-decision-level-metrics-deliberately-absent) · [src](coordinator_comparison.rs) | ● | · | · | ● | Adv | CLI | · | · | · | · | · |
| [`three_arm_workdist`](../docs/plans/three_arm_workdist.md#run-it) · [src](three_arm_workdist.rs) | ● | · | · | ● | Adv | CLI | · | · | · | · | · |
| [`three_node_demo`](chat/README.md#how-to-run) · [src](three_node_demo.rs) ★ | ● | ● | ● | ● | Adv | Web | real | · | · | ✓ | [✓](units/three_node_demo/) |
| [`ops_console`](ops_console/README.md#how-to-run) · [src](ops_console) † | ○ | ○ | ○ | ○ | Adv | Web | · | · | · | · | · |
| **The v3 contracts axis** — one decisive demonstration per item | | | | | | | | | | | |
| [`receipt_ladder`](../docs/guide/18-contracts-and-receipts.md#run-the-ladder) · [src](receipt_ladder.rs) | ● | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`replay_a_bundle`](../docs/guide/19-replay-and-simulation.md#run-it) · [src](../mycelium-sim/examples/replay_a_bundle.rs) | · | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`curator_handover`](../docs/guide/21-mandates.md#run-the-demonstration) · [src](../mycelium-wiki/examples/curator_handover.rs) | · | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`strict_eligibility`](../docs/guide/21-mandates.md#eligibility-unknown-history-is-never-eligible-2240) · [src](strict_eligibility.rs) | ○ | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`control_envelope_viz`](../docs/guide/22-stability-and-control.md#run-the-demonstration) · [src](control_envelope_viz.rs) | ○ | · | · | ● | Adv | Web | · | · | ✓ | · | · |
| [`knowledge_layer`](../docs/design/knowledge-layer.md#7-what-lands-next) · [src](knowledge_layer.rs) | ● | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`federated_domains`](../docs/guide/17-federation.md#encrypting-the-link-the-pin-is-the-anchor) · [src](federated_domains.rs) | ○ | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`procurement_authority`](coop/README.md#13--procurement_authority--the-governed-autonomy-flagship) · [src](coop/src/bin/procurement_authority.rs) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`mcp_tool_authority`](../docs/guide/20-authorising-actions.md#which-tools-an-agent-may-actually-call) · [src](mcp_tool_authority.rs) | ○ | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | · |
| [`a2a_skill_authority`](../docs/guide/20-authorising-actions.md#the-other-door-and-why-it-is-the-one-that-matters) · [src](a2a_skill_authority.rs) | ○ | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | [✓](units/a2a_skill_authority/) |
| [`destination_commit`](../docs/operations/companions.md#mycelium-effects--the-transactional-destination) · [src](../mycelium-effects/examples/destination_commit.rs) | ● | · | · | · | Adv | CLI | · | · | · | ✓ | · |
| [`counting_destination`](../docs/operations/companions.md#mycelium-effects--the-transactional-destination) · [src](../mycelium-effects/examples/counting_destination.rs) | ● | · | · | · | Adv | CLI | · | · | · | · | · |
| [`redistribution_cn`](../docs/guide/24-commitments.md#run-it) · [src](../mycelium-commitment/examples/redistribution_cn.rs) | · | ● | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`authority_drain`](../docs/design/authority-at-execution.md#4-what-this-does-not-claim) · [src](authority_drain.rs) | · | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | · |
| [`composed_commit`](../docs/design/composed-effect.md#9-enforced-at-the-destination-2026-09-29) · [src](composed_commit.rs) | · | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | · |
| [`confined_fleet_node`](../docs/operations/confined-fleet.md#1-before-you-start-does-your-cni-enforce-networkpolicy) · [src](confined_fleet_node.rs) | ○ | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | [✓](units/confined_fleet_node/) |
| **Coordination & identity integrity** | | | | | | | | | | | |
| [`coordination_integrity`](../docs/guide/04-consensus.md#what-a-successful-election-means) · [src](coordination_integrity.rs) | · | · | ● | ○ | Adv | CLI | · | · | · | ✓ | · |
| [`coordinator_by_accretion`](../docs/guide/14-patterns-and-pitfalls.md#12--expect-one-node-to-end-up-holding-every-single-writer-role) · [src](coordinator_by_accretion.rs) | · | · | ○ | ● | Adv | CLI | · | · | · | ✓ | · |
| [`coordination_viz`](../docs/guide/14-patterns-and-pitfalls.md#12--expect-one-node-to-end-up-holding-every-single-writer-role) · [src](coordination_viz.rs) | · | · | ○ | ● | Adv | Web | · | · | ✓ | · | · |
| [`identity_one_record`](../docs/guide/09-security.md#identity-proofs-what-turning-them-on-actually-requires) · [src](identity_one_record.rs) | ● | · | · | ○ | Adv | CLI | · | · | · | ✓ | · |
| [`auditor_questions`](../docs/guide/09-security.md#three-questions-an-auditor-asks) · [src](auditor_questions.rs) | ● | · | · | ○ | Adv | CLI | · | ✓ | · | ✓ | · |
| [`federation_trust_is_not_transitive`](../docs/guide/17-federation.md#three-domains--the-question-two-cannot-ask) · [src](federation_trust_is_not_transitive.rs) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| **Food-Rescue Co-op** — [`coop/README.md`](coop/README.md), one constructive world | | | | | | | | | | | |
| [`mailbox_llm`](coop/README.md#01--mailbox_llm) · [src](coop/src/bin/mailbox_llm.rs) | ○ | · | · | ● | Adv | CLI | mock | · | · | ✓ | [✓](units/mailbox_llm/) |
| [`stigmergy`](coop/README.md#how-to-run) · [src](coop/src/bin/stigmergy.rs) | · | ● | · | ○ | Adv | CLI | · | · | · | ✓ | [✓](units/stigmergy/) |
| [`stigmergy_viz`](coop/README.md#browser-showcases) · [src](coop/src/bin/stigmergy_viz.rs) | · | ● | · | ○ | Adv | Web | · | · | ✓ | · | [✓](units/stigmergy/) |
| [`elastic_intent`](coop/README.md#03--elastic_intent) · [src](coop/src/bin/elastic_intent.rs) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | [✓](units/elastic_intent/) |
| [`provisioning`](coop/README.md#04--provisioning--the-flagship) · [src](coop/src/bin/provisioning.rs) ★ | · | · | · | ● | Adv | CLI | · | · | · | ✓ | [✓](units/provisioning/) |
| [`provisioning_viz`](coop/README.md#browser-showcases) · [src](coop/src/bin/provisioning_viz.rs) ★ | · | · | · | ● | Adv | Web | · | · | ✓ | · | [✓](units/provisioning/) |
| [`federation_facts`](coop/README.md#05--federation_facts) · [src](coop/src/bin/federation_facts.rs) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | [✓](units/federation_facts/) |
| [`rotation`](coop/README.md#06--rotation) · [src](coop/src/bin/rotation.rs) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`consensus`](coop/README.md#07--consensus) · [src](coop/src/bin/consensus.rs) | · | ○ | ● | · | Adv | CLI | · | · | · | ✓ | · |
| [`llm_pipeline`](coop/README.md#08--llm_pipeline) · [src](coop/src/bin/llm_pipeline.rs) | · | · | · | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`mcp_toolgrowth`](coop/README.md#09--mcp_toolgrowth) · [src](coop/src/bin/mcp_toolgrowth.rs) | ○ | · | · | ● | Adv | CLI | mock | · | · | ✓ | [✓](units/mcp_toolgrowth/) |
| [`llm_council`](coop/README.md#10--llm_council) · [src](coop/src/bin/llm_council.rs) | · | · | · | ● | Adv | CLI | mock | · | · | ✓ | [✓](units/llm_council/) |
| [`llm_council_viz`](coop/README.md#browser-showcases) · [src](coop/src/bin/llm_council_viz.rs) | · | · | · | ● | Adv | Web | mock | · | ✓ | · | [✓](units/llm_council/) |
| [`catalog`](coop/README.md#11--catalog) · [src](coop/src/bin/catalog.rs) | ○ | · | · | ● | Adv | CLI | · | · | · | ✓ | [✓](units/catalog/) |
| [`catalog_viz`](coop/README.md#browser-showcases) · [src](coop/src/bin/catalog_viz.rs) | ○ | · | · | ● | Adv | Web | · | · | ✓ | · | [✓](units/catalog/) |
| [`model_deploy`](coop/README.md#m--model_deploy-manual--a-real-llm-through-the-library) · [src](coop/src/bin/model_deploy.rs) | ○ | · | · | ● | Adv | CLI | real | · | · | · | [✓](units/model_deploy/) |
| [`reheal_deploy`](coop/README.md#m--reheal_deploy-manual--the-real-model-reheal-flagship) · [src](coop/src/bin/reheal_deploy.rs) | ○ | · | · | ● | Adv | CLI | mock | · | · | · | [✓](units/reheal_deploy/) |
| [`diagnostics`](coop/README.md#12--diagnostics) · [src](coop/src/bin/diagnostics.rs) | · | ● | · | ○ | Adv | CLI | · | · | · | ✓ | · |
| **Companions** — blackboard · tuple-space · wiki, atop I/II | | | | | | | | | | | |
| [`microgrid`](../mycelium-blackboard/examples/README.md#microgrid) · [src](../mycelium-blackboard/examples/microgrid.rs) | ○ | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`microgrid_viz`](../mycelium-blackboard/examples/README.md#microgrid_viz) · [src](../mycelium-blackboard/examples/microgrid_viz.rs) | ○ | · | · | ● | Adv | Web | · | · | ✓ | · | · |
| [`redistribution`](../mycelium-tuple-space/examples/README.md#redistribution) · [src](../mycelium-tuple-space/examples/redistribution.rs) | ○ | · | · | ● | Adv | CLI | · | · | · | · | · |
| [`redistribution_viz`](../mycelium-tuple-space/examples/README.md#redistribution_viz) · [src](../mycelium-tuple-space/examples/redistribution_viz.rs) | ○ | · | · | ● | Adv | Web | · | · | ✓ | · | · |
| [`admission`](../docs/operations/admission-control.md#the-example) · [src](../mycelium-tuple-space/examples/admission.rs) | ○ | · | · | ● | Adv | CLI | · | · | · | · | · |
| [`fluid_pipeline`](fluid_pipeline/README.md#how-to-run) · [src](fluid_pipeline) | · | · | · | ● | Adv | CLI | · | · | · | ✓ | · |
| [`wiki_chat`](../mycelium-wiki/examples/README.md#wiki_chat) · [src](../mycelium-wiki/examples/wiki_chat.rs) | ○ | · | · | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`wiki_council_viz`](../mycelium-wiki/examples/README.md#wiki_council_viz-) · [src](../mycelium-wiki/examples/wiki_council_viz.rs) ★ | ○ | · | · | ● | Adv | Web | real | · | ✓ | · | · |
| **Reasoning** — [`mycelium-reason/examples/README.md`](../mycelium-reason/examples/README.md) | | | | | | | | | | | |
| [`fleet_reasoning`](../mycelium-reason/examples/README.md#fleet_reasoning) · [src](../mycelium-reason/examples/fleet_reasoning.rs) | · | · | · | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`reason_node`](../mycelium-reason/examples/README.md#reason_node) · [src](../mycelium-reason/examples/reason_node.rs) | ○ | · | · | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`reheal_node`](../mycelium-reason/examples/README.md#reheal_node) · [src](../mycelium-reason/examples/reheal_node.rs) | ○ | · | ○ | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`ollama_serve`](../mycelium-reason/examples/README.md#ollama_serve) · [src](../mycelium-reason/examples/ollama_serve.rs) | · | · | · | ● | Adv | CLI | real | · | · | · | · |
| [`openai_serve`](../mycelium-reason/examples/README.md#openai_serve) · [src](../mycelium-reason/examples/openai_serve.rs) | · | · | · | ● | Adv | CLI | real | · | · | · | · |
| **Guardrails** — [`mycelium-guardrails/examples/README.md`](../mycelium-guardrails/examples/README.md) | | | | | | | | | | | |
| [`guardrail_fleet`](../mycelium-guardrails/examples/README.md#guardrail_fleet) · [src](../mycelium-guardrails/examples/guardrail_fleet.rs) | · | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | · |
| [`guardrail_wedge`](../mycelium-guardrails/examples/README.md#guardrail_wedge) · [src](../mycelium-guardrails/examples/guardrail_wedge.rs) | · | · | · | ● | Adv | CLI | · | ✓ | · | ✓ | · |
| [`guardrail_viz`](../mycelium-guardrails/examples/README.md#guardrail_viz) · [src](../mycelium-guardrails/examples/guardrail_viz.rs) ★ | · | · | · | ● | Adv | Web | · | ✓ | ✓ | · | · |
| **Python interop** — external agents & skills | | | | | | | | | | | |
| [`a2a_langchain`](a2a_langchain/README.md#4--run-the-langchain-agent) · [src](a2a_langchain) | · | · | · | ● | Adv | CLI | real | · | · | · | · |
| [`langgraph`](langgraph/README.md#how-to-run) · [src](langgraph) | ○ | · | ○ | ● | Adv | CLI | mock | · | · | ✓ | · |
| [`community`](community/README.md#end-to-end-demo-recommended) · [src](community) | · | · | · | ● | Adv | Web | real | ✓ | · | ✓ | · |

**Harness binaries — deliberately not rows above.** `federation_node` (the one binary of the
two-mesh Docker suite), `scrape_fleet_node` and `scrape_worker_node` (launcher + sidecar for the
analytics-scraper fleet) are *fixtures a suite starts*, not demonstrations you read. They are named
here so this page is a complete enumeration: a list that is silently short is indistinguishable from
one that is out of date, and this index has drifted that way before.

★ **flagship** — the marquee demo of its world. † `ops_console` *observes* every layer and both ops
surfaces (`/audit`, `/metrics`) rather than emitting them — point it at any node below. Every link above
goes to a **run doc** (README or guide chapter), never raw source; the exact commands live in each suite
README (and, for the visual demos, [Browser showcases](#browser-showcases) below).

## Browser showcases

Open-and-watch demos — the `/state`-feed-behind-a-canvas pattern `conway` established: run continuously
(Ctrl-C to stop; **not** in any CI smoke), open `http://127.0.0.1:80xx/`. All follow the
[UI-example contract](../docs/wiki/dev/ui-example-contract.md) — gateway+metrics on, Ops Console linked,
opt-in audit, a "what you're seeing" concepts box. See [shared setup](#shared-setup) first; each name
links to its walkthrough README.

| Showcase | Port | Run |
|---|:--:|---|
| [`microgrid_viz`](../mycelium-blackboard/examples/README.md) | `:8091` | `cargo run -p mycelium-blackboard --example microgrid_viz --features gateway,metrics` |
| [`stigmergy_viz`](coop/README.md) | `:8092` | `cargo run -p mycelium-coop-examples --bin stigmergy_viz --features metrics` |
| [`control_envelope_viz`](../docs/design/adaptive-stability.md) | `:8096` | `cargo run --example control_envelope_viz --features metrics` |
| [`redistribution_viz`](../mycelium-tuple-space/examples/README.md) | `:8093` | `cargo run -p mycelium-tuple-space --example redistribution_viz --features gateway,metrics` |
| [`llm_council_viz`](coop/README.md) | `:8094` | `cargo run -p mycelium-coop-examples --bin llm_council_viz --features metrics` |
| [`provisioning_viz`](coop/README.md) ★ | `:8101` | `cargo run -p mycelium-coop-examples --features wasm,metrics --bin provisioning_viz` |
| [`catalog_viz`](coop/README.md) | `:8098` | `cargo run -p mycelium-coop-examples --features wasm,metrics --bin catalog_viz` |
| [`wiki_council_viz`](../mycelium-wiki/examples/README.md) ★ | `:8095` | `cargo run -p mycelium-wiki --example wiki_council_viz --features gateway,llm,metrics` |
| [`guardrail_viz`](../mycelium-guardrails/examples/README.md) ★ | `:8097` | `cargo run -p mycelium-guardrails --example guardrail_viz --features compliance,gateway,metrics-export` |
| [`conway`](../docs/guide/01-gossip-kv.md) | `:8090` | `cargo run --example conway --features metrics` |
| [`coordination_viz`](../docs/guide/14-patterns-and-pitfalls.md) | `:8100` | `cargo run --example coordination_viz --features metrics` |
| [`conway-gpu`](conway-gpu/README.md) | — | `cargo run --release -p conway-gpu` (GPU/wgpu; no gateway) |

`wiki_council_viz` phrases each specialist's grounded answer via a **local model served on the mesh**
(Ollama), falling back to grounded extraction if absent — no cloud, no key. `guardrail_viz` fires
invocations at a Tier-C gate and rebuilds the **cryptographic denial proof** live. The two
**artifact-library** showcases make the deploy/install flow watchable: `provisioning_viz` — a capability
**self-provisions** from unmet demand, then **heals onto a standby** when you kill the active node (no
coordinator); `catalog_viz` — the origin (librarian) **dies and its library is deleted**, yet a late
node still **installs from a verified peer cache**. The `*_viz` set are visual variants of the batch
coop/companion demos, which stay the CI-gated versions.

**Everything else** — the starter ladder, the Food-Rescue Co-op suite, guardrails, the community
skills cluster, reasoning/LangGraph, AFN, A2A, and the interactive chat — is in the
[capability matrix](#the-capability-matrix) above; click any row for its walkthrough + exact command.

## Ops Console

A generic, read-only dashboard over *any* gateway-enabled node's operational endpoints — `/stats` ·
`/gateway/fleet` · `/gateway/diagnose` (the Legible-Emergence fleet narrative) · `/gateway/audit` ·
`/gateway/kv/keys` · `/metrics` — in one place. A *dev/reference* tool, **not** a shipped control plane
(library, not platform); a customer forks it or points Grafana at `/metrics`.

```
cargo run --example ops_console            # → http://127.0.0.1:8099/  (default target 127.0.0.1:9050)
```

Full docs — the tab-by-tab endpoint map, the `ui/viz` two-way linking convention, and which host box to
point at each demo/showcase — are in **[`ops_console/README.md`](ops_console/README.md)**.

## Research artifacts

Paper 1 / 2a experiment runners — reproducible, not tutorials:
[`coordinator_comparison.rs`](coordinator_comparison.rs) (+ [runner](coordinator_comparison_runner.sh)/[plot](coordinator_comparison_plot.py)) ·
[`three_arm_workdist.rs`](three_arm_workdist.rs) (+ [runner](three_arm_runner.sh)/[plot](three_arm_plot.py)) —
complementary, not redundant: `coordinator_comparison` is the two-arm *decision-level* probe (broker vs
gossip prediction, staleness/misroute), `three_arm_workdist` adds the **pull** arm and measures
*outcomes* (latency/throughput/fairness). See each file's header for the experiment design.

## Shared setup

Every cluster below assumes some subset of these. An example's README names which it needs and its
own one-line build; it does **not** re-explain the install — it links here.

**Rust toolchain** (all examples). The pinned toolchain builds automatically:
```bash
cargo build --example hello_mesh     # or the specific --example / --bin an example names
```

**Ollama** (LLM examples — free, no API key). Any OpenAI-compatible endpoint works instead.
```bash
ollama serve                 # in its own terminal
ollama pull llama3.2         # the common default; some examples name a stronger model
```
To use a non-Ollama backend, set `OLLAMA_BASE_URL` + `OLLAMA_MODEL` (or `OPENAI_API_KEY` +
`OPENAI_MODEL` where the README says so). Small models sometimes mis-pick tools — for reliable
tool-calling use a stronger local model (`qwen3:14b` is verified) or `gpt-4o-mini`.

**Python tier** (the A2A + LangGraph examples). Python ≥ 3.11, in a venv:
```bash
python -m venv .venv && source .venv/bin/activate
pip install './mycelium-py[typed]' ./langgraph-checkpoint-mycelium   # + any per-example deps
```

**Docker Compose v2** (only the containerised examples, e.g. `fluid_pipeline`): `docker compose version`.

---

## The doc template (for contributors)

Example READMEs drifted into three names for the same section ("Run"/"Quick start", "What you'll
see"/"What to observe") and re-typed setup. **New and edited example docs follow the layout below.**
There are two variants; both share the same **per-example block**.

### Per-example block (the reusable unit)

1. **Objective** — 1–3 sentences: what this example demonstrates and *why it matters*. Lead with the
   capability, not the plumbing.
2. **How to run** — the exact commands. Link to [shared setup](#shared-setup) for the toolchain;
   show only this example's build + run + expected first output.
3. **What it demonstrates** — the walkthrough: what to watch, tied back to the concept, **with links
   into the guide/wiki for the idea and into `src/` (or the example source) for the mechanism**. This
   is where a reader connects "what I saw" to "how it works" — the section that earns the example.
4. **Dev notes** *(optional)* — gotchas, tuning knobs, "when NOT to use this."

Standard section names: `## Objective` · `## How to run` · `## What it demonstrates` · `## Dev notes`.
Retire the variants (`Concept`, `Quick start`, `What you'll see`, `What to try`).

### Variant A — single example

Title → **Objective** → **How to run** → **What it demonstrates** → *Dev notes*. One block, top to
bottom. (`chat/`, `fluid_pipeline/`, `a2a_langchain/`.)

### Variant B — suite / cluster (many examples under one theme)

Title → **Objective** (the cluster's theme + shared harness) → **How to run** (the one bring-up every
member shares) → a **per-example block** for each demo (Objective · Run · What it demonstrates+links)
→ **CI**. (`coop/` is the reference implementation of this shape; `community/`, `langgraph/`.)

> **Where narrative lives:** walkthroughs stay *in the example README* (a developer running it wants
> them right there); link *out* to the guide/wiki for the concept and to `src/` for the mechanism —
> don't duplicate either.
