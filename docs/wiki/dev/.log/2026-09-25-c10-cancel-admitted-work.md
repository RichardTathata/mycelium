## [2026-09-25] ingest | closure plan C10: work already running stops too

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §10 · code
`src/agent/gateway_authority.rs` (`begin`, `WorkGuard`, `sweep`), `src/agent/provider_enforcement.rs` (`run`),
`src/agent/rpc.rs` (`RpcRequest::authority_lapsed`) · lock-order rows 48, 49.

A1 re-checked authority at every *start* (admission, dequeue, retry) and modelled the stop. Nothing stopped a handler
already running. The mechanism is the model's own `ReauthorizeAt`, made real: running work is registered, a sweep
re-runs A1's check, and the first failure cancels. One sweep covers expiry, revocation, staleness and supersession,
which is why it is a sweep and not four separate triggers.

Where the stop lands decides what can be claimed. An MCP handler is a future the node owns, so dropping it is a
confirmed stop. An `rpc_rx` handler is the application's, so cancellation is an offer it can take
(`authority_lapsed()`), and a loop that ignores it stays unconfirmed, which is exactly what the drain model already
calls `Unbounded`.
