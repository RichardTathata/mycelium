# Serving a model as a skill

## What it is

A `[[serve]]` section turns a model a stem hosts into a routable prompt skill: the stem registers `{ns}/{name}`, backed
by an OpenAI-compatible endpoint, while the install named by `while_live` is live on this node, and retracts it when
that install is withdrawn. So a router resolving the skill reaches a node that has the model now, and fails over to
another when the model moves ([serve, in the glossary](../guide/00-concepts.md#serve)). The endpoint's key comes from the
environment, a literal, or a credential binding the node's egress gate attaches, so the unit file need carry no secret.

## When to use it, and when not

- **Use it when** a model is deployed as an artifact on stems — placed and activated into a local model server — and
  callers should reach it by skill name wherever it lands.
- **Not when** the model runs outside the fleet: register it as a skill or a reason-router backend directly; and not over
  `/a2a`, which refuses a served skill and names the route to use.

## Turn it on

Build the stem with `llm` (the published `mycelium-stem` has it; without it, a stem refuses a unit file with a
`[[serve]]`). The reheal demo's [`examples/units/reheal_deploy/survivor.toml`](../../examples/units/reheal_deploy/survivor.toml),
which CI's wire-check reads, declares one.

- `[[serve]]` ([unit-file § serve](../reference/unit-file.md#serve)): `ns` (default `llm`), `name`, `endpoint`, `model`,
  `while_live` (refused when it names the skill itself), one of `api_key`, `api_key_env` (an unset variable refuses the
  start) or `credential` ([credentials at egress](credentials-at-egress.md)), and `max_tokens`, `temperature`.
- Call it through `POST /gateway/llm/call` (`llm:invoke`) or the reason router (`/gateway/reason/route`,
  `/gateway/reason/v1/chat/completions`).
- `a_served_skill_follows_the_install_it_waits_for`, in [`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs):
  the skill appears when the install it waits for does and is retracted when it goes.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`reheal_deploy`](../../examples/README.md#the-capability-matrix): a model reheals onto the surviving node and answers
through routed inference after its origin dies; it needs Ollama and a GGUF, so it is a manual run. The run CI does with
a credential binding is `a_served_skill_sends_its_binding_value_which_the_stem_never_held`, in
[`mycelium-wasm-host/src/stem.rs`](../../mycelium-wasm-host/src/stem.rs).

## What it guarantees, and what it does not

- No guarantee or rule id of its own: a served skill is called through the LLM doors, where the
  [action evaluator](action-authorisation.md) and [inference routing](inference-routing.md) decide; the key's handling
  is `egress.credentials`' when a binding carries it.
- **Not built:** the skill's availability follows the install's advertisement, not the endpoint's health — a model server
  that is up but failing still serves until its probe withdraws the install; `api_key` as a literal is for a file that
  is not committed; a served skill is not on the A2A card.
- **Evidence:** the ledger row
  [every artifact-shaped co-op demo reaches the same outcome as stem nodes](../operations/what-is-proven.md#coop-demos-same-outcome-as-stems).

## Go deeper

- **Guide:** [15, reasoning and LangGraph](../guide/15-reasoning-and-langgraph.md).
- **Runbook:** [capability lifecycle](../operations/capability-lifecycle.md) §1, on keeping keys out of unit files.
- **Design:** decision D22 in [`plans/design-time-tooling.md`](../plans/design-time-tooling.md).
- **Code:** [`mycelium-wasm-host/src/serve.rs`](../../mycelium-wasm-host/src/serve.rs).
