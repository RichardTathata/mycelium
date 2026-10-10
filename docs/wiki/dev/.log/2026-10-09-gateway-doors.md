# 2026-10-09 — the gateway's doors answer only what they were asked, by whom

## [2026-10-09] ingest | gateway doors (branch `fix/gateway-doors`, unreleased)

Seven confirmed gateway findings, each with a test seen failing first:

- **The signal SSE streams** (`/signals/{kind}`, `/gateway/signal/sse/{kind}`) registered on the same
  `SignalHandlers` table `rpc/serve` and the MCP tools use, so a `mesh:read` holder read every protected
  request's frame (caller envelope, carried mandate, nonce). Both now call `refuse_protected_kind` — the C1
  closure's SSE half; C7 matrix row added. `sse_doors_refuse_protected_kinds`.
- **`rpc_pending` claimed on nonce alone**; a forged `rpc.result` from another node consumed the oneshot and the
  caller timed out although the provider answered. The map now carries the node the call was sent to
  (lock-order row 2 updated); a mismatch is counted in `SystemStats::rpc_reply_sender_mismatches` and ignored.
  `a_forged_reply_from_the_wrong_sender_does_not_consume_the_pending_call`.
- **`rpc/respond` bound to no request**: any `mesh:serve` holder could answer any in-flight call by nonce and
  release its parked admission. `HttpCtx::served_rpcs` (lock-order row 55; `(sender, nonce, principal)`, 300 s
  TTL, capped) records what `rpc/serve` streamed to whom; `rpc/respond` answers only that, once — `403
  unserved_request`. `rpc_respond_answers_only_a_request_this_principal_was_handed`.
- **`mycelium-reason`'s façade dispatched `llm.invoke` as the node** and ran no preflight. New public API in the
  root crate (`ResolvedPrincipal` readable, `ServiceHandle::rpc_call_as`, `GossipAgent::gateway_preflight` /
  `gateway_record_execution`, `GatewayDispatchError`); `InferenceRouter::call_as`; `mycelium-reason` 0.8.0.
  `the_facade_dispatches_as_the_http_client_not_the_node`, `the_facade_runs_the_action_preflight`.
- **A2A `tasks/get` / `tasks/cancel` had no owner check** (the Phase-C audit closed the federated half only).
  `A2aTask::owner`; a task answers only the identity that created it, `-32004`.
  `a_task_is_readable_and_cancellable_only_by_the_identity_that_created_it`.
- **`GET /api/tuple` was outside the auth prefix** → `GET /gateway/tuple/overview` (`tuple:read`); old path 404.
  `http::PUBLIC_PATHS` + `the_public_surface_is_exactly_the_documented_list` make rbac.md's "the routing code
  asserts" claim true. `the_overview_sits_behind_the_gateway_bearer`.
- **`/gateway/llm/call|stream` ran no preflight** though the raw routes name them as `llm.invoke`'s door:
  `ae_preflight` under `gateway:llm/call` / `gateway:llm/stream`, mandate carried with the resource.
  `the_llm_doors_run_the_action_preflight`. And `resolve_token` compares bearers with `subtle::ConstantTimeEq`.

Pages touched: `dev/concurrency/lock-order.md` (row 2 revised, row 55 added); this log. Operator docs:
`docs/operations/rbac.md`, `what-is-proven.md`, `companions.md`; `docs/guide/08-a2a-interop.md`,
`09-security.md`, `deprecations.md` §25; `CHANGELOG.md` `[Unreleased]`; `mycelium-reason/CHANGELOG.md` 0.8.0;
the Phase-C log's finding 2 carries a dated note. Lesson for the lint: a door that *observes* protected work
(SSE) is as much a door as one that sends it, and a scope that names a route (`mesh:serve`) binds neither a
kind nor a request unless the handler does.
