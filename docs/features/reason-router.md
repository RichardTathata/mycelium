# The reason router

## What it is

`mycelium-reason` is the reasoning companion. Its centre is the `InferenceRouter`: a model call goes to a healthy node
serving that model, with no central proxy — it resolves the model's providers, drops opaque nodes, ranks the rest by
their load trail and its own in-flight reservations, and fails over down the list. It is reachable from Rust, at
`POST /gateway/reason/route`, and through an OpenAI-compatible façade, `POST /gateway/reason/v1/chat/completions`,
where a client's `model` becomes an `llm/{model}` route across the fleet. Beside it: replayable, HLC-ordered reasoning
traces, a model-dependency wait for a resumed graph, and a bounded, content-addressed blob store the Python LangGraph
checkpointer uses. With an action evaluator, routing is decided as policy ([inference routing](inference-routing.md)).

## When to use it, and when not

- **Use it when** several nodes serve the same model, callers should not know which, and a dead or busy server should be
  routed around — from Rust, an OpenAI-speaking client or LangGraph.
- **Not when** you need output quality judged: routing is about where a call goes, not what the model says; and the
  locality a policy decides on is the provider's own declaration.

## Turn it on

This feature is turned on in code, from the `mycelium-reason` crate. Its features: `llm` for the serve side
(`serve_model`); `gateway` for the HTTP routes; `compliance` to anchor a trace into the audit chain; `ollama` for the
collector that fills a served model's `llm-meta` ad from a local daemon; `knowledge` for the trace's knowledge-layer
adapter ([knowledge layer](knowledge-layer.md)).

- [`serve_model`](../../mycelium-reason/src/route.rs) with a [`ModelProfile`](../../mycelium-reason/src/route.rs)
  (`llm`): advertise a model and its metadata.
- [`InferenceRouter::new`](../../mycelium-reason/src/route.rs) and `call`: route a call; a
  [`RouterConfig`](../../mycelium-reason/src/route.rs) sets the ranking and failover.
- [`reason_router`](../../mycelium-reason/src/http.rs) (`gateway`): mount the routes with `with_http_routes`;
  `reason_router_classified` adds the data-class door.
- [`OllamaProbe`](../../mycelium-reason/src/ollama.rs) and `spawn_meta_refresher` (`ollama`).
- `route_and_failover_across_providers`, in [`mycelium-reason/tests/reason.rs`](../../mycelium-reason/tests/reason.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`fleet_reasoning`](../../examples/README.md#the-capability-matrix): a three-node co-op where a coordinator's model calls
are routed and failed over, its reasoning traced and replayed, and a model dependency awaited. CI runs it against a
mock engine. [`reason_node`](../../examples/README.md#the-capability-matrix) and
[`reheal_node`](../../examples/README.md#the-capability-matrix) keep a gateway up for the Python side.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: a routed call through the gateway is
  [`ae.preflight`](../reference/rule-catalogue.md#aepreflight)'s, each candidate decided before dispatch.
- **Not built:** load-aware routing beyond the trail and this router's own reservations — another router's in-flight
  calls are not seen; blobs above 8 MiB (one RPC frame) or a store past its quota (1 GiB by default; nothing evicts);
  read-your-writes across nodes — checkpoint metadata is gossip-eventual; a trace anchored into the audit chain, except
  under `compliance`.
- **Evidence:** the ledger row
  [a routed model call is decided per candidate](../operations/what-is-proven.md#routed-model-call-decided-per-candidate).

## Go deeper

- **Guide:** [15, reasoning and LangGraph](../guide/15-reasoning-and-langgraph.md), including
  [§ The three differentiators](../guide/15-reasoning-and-langgraph.md#the-three-differentiators-mycelium-reason-tier-3) and
  [§ Honest limits](../guide/15-reasoning-and-langgraph.md#honest-limits).
- **Runbook:** [companions](../operations/companions.md).
- **Design:** [`plans/mycelium-reason.md`](../plans/mycelium-reason.md).
- **Code:** [`mycelium-reason/src/`](../../mycelium-reason/src/lib.rs) (`route.rs`, `http.rs`, `trace.rs`, `blob.rs`,
  `ollama.rs`).
