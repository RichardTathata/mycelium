# LLM prompt skills

## What it is

In a build with `llm`, any node can host a prompt skill: a capability backed by an OpenAI-compatible model, its prompt
template stored in the gossip KV under `prompts/{ns}/{name}` and read fresh on every call. The hosting node advertises
the capability and serves `llm.invoke` calls; any node calls it by name and the mesh finds a provider. Because the
template is an ordinary key, updating it takes effect on the next call, fleet-wide, with no restart. `llm.invoke` is a
protected kind, so the gateway's LLM doors run the action preflight before a call is sent.

## When to use it, and when not

- **Use it when** a model call should be a fleet capability — callable from any node or SDK, with the prompt managed as
  shared configuration rather than baked into code.
- **Not when** the skill should be its own long-lived node defined by a manifest, which is a [skill](skills.md); and
  not when several models should be chosen among per request, which is the [reason router](reason-router.md).

## Turn it on

This feature is turned on in code: build with `llm` (the published node has it).

- [`register_prompt_skill`](../../src/agent/llm_handle.rs) with a `PromptTemplate` and a backend: the template is
  written, the `llm.invoke` receiver registered, and the capability advertised last; dropping the handle retracts it.
- [`call_prompt_skill`](../../src/agent/llm_handle.rs) from any node; over HTTP, `POST /gateway/llm/call` under
  `llm:invoke`.
- [`update_prompt`](../../src/agent/llm_handle.rs): replace the template; the next call reads it.
- `OpenAiBackend::new` follows no redirect; build it `.with_egress(policy)` to gate its endpoint, or
  `.with_credentials(gate)` to have the node attach its key ([credentials at egress](credentials-at-egress.md)).
- `the_llm_invoke_receiver_exists_when_a_prompt_skill_is_registered`, in [`src/agent/llm.rs`](../../src/agent/llm.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`mailbox_llm`](../../examples/README.md#the-capability-matrix): a kitchen node registers the prompt skill
`routing/suggest` over a test backend, and a depot finds it by capability and invokes it. CI runs it.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: a call through the gateway's LLM doors is [`ae.preflight`](../reference/rule-catalogue.md#aepreflight)'s,
  and a template update is an ordinary KV write.
- **Not built:** output quality — the substrate routes and records the call, not what the model says; a template is
  last-writer-wins, so two concurrent updates keep one; a prompt skill is not on the A2A card, and `/a2a` refuses it
  by name.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for prompt skills on their own.

## Go deeper

- **Guide:** [05 § Reference — prompt skills](../guide/05-skills.md#reference--prompt-skills-llm-backed-capabilities-via-the-kv-substrate)
  and [§ Live prompt-template updates](../guide/05-skills.md#live-prompt-template-updates-no-restart).
- **Runbook:** [observability](../operations/observability.md); [rbac](../operations/rbac.md) for `llm:invoke`.
- **Design:** no design record; the guide is the record.
- **Code:** [`src/agent/llm_handle.rs`](../../src/agent/llm_handle.rs) and [`src/agent/llm.rs`](../../src/agent/llm.rs);
  the gateway's door in [`src/agent/http/llm_bridge.rs`](../../src/agent/http/llm_bridge.rs).
