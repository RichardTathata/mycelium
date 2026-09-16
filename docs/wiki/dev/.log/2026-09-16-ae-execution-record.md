## [2026-09-16] ingest | the execution record — what the gateway actually observed

Up: [dev](../dev.md) §AE · record `docs/design/action-envelope-ae0.md` §5 · code
`src/agent/http.rs`, `src/agent/a2a.rs`, `src/agent/action_evaluator.rs`.

**The gap.** Every permitted call exported as `effect: unknown`, because the only record written was
the *decision* and a decision establishes nothing about execution. The gateway had the answer the
whole time — `gateway_rpc_call` returns the provider's reply — and threw it away. So the evidence
could say what an agent was **allowed** to do and never what it **did**, which is most of what anyone
looks at a governance page to find out.

**The shape.** A dispatch now produces two journal records: `decided`, then `execution`. Separate
records rather than one mutated in place, because evidence is append-only and a record revisable in
place is revisable *after* someone has read it. The execution record carries the decision's
identities unchanged — same `operation_id`, `attempt_id`, principal, policy revision — since it is a
statement about the same attempt and correlating them is the consumer's whole job.

**The mapping, and the one arm that matters.**

| observed | recorded |
|---|---|
| a reply arrived | `completed` — or `failed` when the JSON-RPC body carries an `error` |
| refused before sending (no caller context, context too large) | `none` — the one case where *nothing ran* can be stated |
| **timeout, or any transport error** | **`unknown`** |

The last row is the whole point. A timeout means the provider did not answer, **not** that the call
did not run. `Failed` would be a claim about the world, and a consumer reading `failed` will act on
it. `Unknown` is item 1's `DeliveryUnknown` and this project's hot invariant: *a timeout is never a
negative*. The mapping is extracted as `observed_execution` so that rule is unit-tested rather than
reasoned about — testing it through a live gateway would have meant a thirty-second wait per
assertion, which is how a rule ends up untested.

**A reply that arrived is a completed RPC.** Whether the tool inside it succeeded is a different
question, and the consumer reads it as `effect: failed`. Bytes that parse as nothing count as failed:
something answered, and it was not an answer.

**Recording the execution cannot retract the dispatch.** If the journal fails at this point the
effect has already happened; the failure is logged and the reference record carries the journal's
state. Refusing retroactively would be the one lie worse than silence.

**Gates.** `make check` clean; 521 (`compliance,a2a`) + 459 (`tls,metrics,a2a,llm`) + 343
(`--no-default-features --features gateway`) + 179 (`mycelium-core`).

**Still owed:** `requested` and `blocked` (§5's other enforcement-point records), and
`outcome observed` — which needs an *independent* observer and is deliberately not the enforcement
point's to write. Plus the exporter itself, still waiting on a tag.
