# 4 · Authority at every boundary

↑ [Tutorials](README.md) · Next: [Replay a decision](05-replay-a-decision.md)

## Objective

Trace where authority is checked: gateway dispatch, provider admission and
continuation, and the destination that commits an effect.

## How to run

Prerequisites: Rust and loopback sockets. From the repository root:

```sh
cargo run --example composed_commit --features tls,compliance
cargo run --example authority_drain --features compliance
```

The first shows a `Fresh` commit, `Replayed` duplicate, then a destination refusal
while the gateway still permits. Read back the journal to see both decisions.
The second measures admission and confirmed drain after revocation, and checks
the declared bound. An unconfirmed stop is never counted as stopped.

## What it demonstrates

| Boundary | Source to follow | Evidence |
|---|---|---|
| Gateway | [composed_commit.rs](../../../examples/composed_commit.rs) | Evaluator decision before dispatch |
| Provider | [authority_drain.rs](../../../examples/authority_drain.rs) | Admission refusal and cancellation/confirmation records |
| Destination | `composed_commit`'s resource epoch update | Refused effect despite the earlier permit |

## Try a change, then diagnose it

In `composed_commit`, change the operation ID used for the duplicate call to a new
ID. The duplicate expectation should fail: deduplication is keyed by operation
identity, not by identical arguments. Restore it. The example's resource epoch
advance is the intentional authority failure: keep the gateway state unchanged
and explain why the destination refuses. Do not repair that refusal by weakening
its epoch check.

If the initial request fails, inspect verified caller identity, mandate scope and
policy inputs. `Indeterminate` means authority was not established, not permit.

## Dev notes

These are CI-executed examples with real sockets in one process. They do not prove
a cross-domain trust chain or deployment drain latency. A resource must install
and enforce its own authority state; the example updates it directly. Normal
completion stops the agents. For your handler, preserve verified caller context,
operation identity and the destination fence. See [guide 20](../20-authorising-actions.md)
and [mandates](../21-mandates.md) before claiming coverage.
