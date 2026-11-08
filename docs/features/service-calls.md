# Service calls: RPC, scatter-gather and bulk

## What it is

Three ways for one node to call another by kind, over the [signal mesh](signal-mesh.md). An RPC (`rpc_call`) sends
an Individual-scoped request to one node and waits for its reply on `rpc.result`, or returns `Timeout`; the payload
rides in the signal frame. Scatter-gather (`scatter_gather`) sends the same request to several nodes at once and
returns as soon as `min_ok` of them have replied, or `InsufficientReplies`. A bulk call (`bulk_call`) is for a payload
too large to put in a frame: the caller stages it at `GET /bulk/{id}` on its own gateway and sends only a ticket, and
the responder fetches the bytes from the caller over HTTP — or over HTTPS when the caller's gateway serves TLS,
verifying the caller's certificate — and replies on `bulk.result`. A serve loop's receiver is registered before
`rpc_rx`, `bulk_serve` or a scatter handler returns, so a request that arrives straight after is served.

## When to use it, and when not

- **Use it when** a node needs an answer from a provider it has already resolved — by
  [capability](capability-discovery.md) or by id — and can wait for it.
- **Not when** the request must survive the target being down (that is the [mailbox](mailbox.md)); and not when the
  answer must be exactly-once at a destination: a `Timeout` is unknown, never a negative, so retry only what the
  provider can deduplicate ([destination commit](destination-commit.md)).

## Turn it on

This feature is turned on in code, through `agent.service()`.

- [`rpc_call`](../../src/agent/service_handle.rs) and, on the provider, [`rpc_rx`](../../src/agent/service_handle.rs)
  with [`rpc_respond`](../../src/agent/service_handle.rs); [`rpc_call_with_mandate`](../../src/agent/service_handle.rs)
  carries a [mandate](mandates.md) the provider can verify, and returns `ContextTooLarge` when the envelope would not
  fit.
- [`scatter_gather`](../../src/agent/service_handle.rs): the targets, the kind, the payload, a timeout and `min_ok`.
- [`bulk_call`](../../src/agent/service_handle.rs) and [`bulk_serve`](../../src/agent/service_handle.rs): the caller
  needs an `http_port`, or `NoHttpPort`; a payload over
  [`max_bulk_payload_bytes`](../reference/configuration.md#max_bulk_payload_bytes) (64 MiB by default) is refused before
  it is staged, and a responder refuses a fetch over its own cap. Under
  [`gateway_tls`](../reference/configuration.md#gateway_tls) the fetch is verified against `bulk_ca_pem_path`, or the
  fleet CA from `[tls]`; a gateway serving an operator's certificate (`cert_pem_path`) without `bulk_ca_pem_path`
  refuses the start, and one that `bulk_ca_pem_path` does not verify is refused too. An application serving `/bulk/{id}` from its own server
  calls `set_bulk_serving_port`, `set_bulk_serving_https` and `bulk_staging_get`.
- Over HTTP: `POST /gateway/rpc/call` and `POST /gateway/scatter` under `mesh:write`, and an SDK provider's serve stream
  `GET /gateway/rpc/serve/{kind}` with `POST /gateway/rpc/respond` under `mesh:serve` — scopes checked only in a build
  with `compliance` (the published node has it). The raw routes refuse a protected kind — `mcp.invoke`, `skill.invoke`,
  `llm.invoke` and any the node adds — `403 protected_kind`, so those go through the doors that decide them. Neither SDK
  has a bulk verb.
- `test_rpc_round_trip`, in [`src/agent/rpc.rs`](../../src/agent/rpc.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for service
calls on their own; every example that calls a provider uses them. The end-to-end runs are tests:
`scatter_gather_insufficient_replies`, in [`src/agent/scatter.rs`](../../src/agent/scatter.rs), and
`a_bulk_call_completes_between_two_gateway_tls_nodes` and `a_bulk_call_over_the_cap_is_refused_before_it_is_staged`, in
[`src/agent/bulk.rs`](../../src/agent/bulk.rs).

## What it guarantees, and what it does not

- No guarantee or rule id of its own: a protected kind's call is decided by
  [`ae.preflight`](../reference/rule-catalogue.md#aepreflight) at a gateway and by
  [`provider.enforcement`](../reference/rule-catalogue.md#providerenforcement) at a provider that turns it on, and a
  provider withdrawn by a revocation answers with a refusal body rather than an `RpcError`.
- **Not built:** a typed refusal for a bulk fetch — a responder that refuses (over its cap, a certificate that does not
  verify, no roots for an `https` ticket, too old for one) sends nothing back, so the caller sees `Timeout`, which can
  be permanent, and the responder counts it (`bulk_fetches_refused`); an integrity check on a bulk payload beyond TLS; a
  plaintext ticket is fetched over plaintext, as the caller chose; the fetch goes to the caller's advertised address.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for service calls on their own;
  bulk under `gateway_tls` is the external review's row R7
  ([`plans/external-review-2026-10-31.md`](../plans/external-review-2026-10-31.md)).

## Go deeper

- **Guide:** [cookbook § The service layer](../guide/cookbook.md#reference--the-service-layer-rpc-bulk-scatter-gather-mailbox);
  [03, signals](../guide/03-signals.md) for the kinds.
- **Runbook:** [gateway TLS](../operations/gateway-tls.md) for `bulk_ca_pem_path`; [rbac](../operations/rbac.md) for the
  scopes.
- **Design:** no design record; the module docs are the record.
- **Code:** [`src/agent/rpc.rs`](../../src/agent/rpc.rs), [`src/agent/scatter.rs`](../../src/agent/scatter.rs),
  [`src/agent/bulk.rs`](../../src/agent/bulk.rs) and [`src/agent/service_handle.rs`](../../src/agent/service_handle.rs).
