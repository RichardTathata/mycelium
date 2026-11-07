# Gateway TLS

## What it is

With a `[gateway_tls]` table the [gateway](http-gateway.md) serves HTTPS itself, so bearers and JWTs do not cross
the wire in cleartext. It is server-side TLS only, unlike the mutually authenticated mesh: ordinary HTTPS clients
connect normally. The certificate is either the node's own mesh identity or one the operator supplies with a real
hostname for browser trust. The other way to get the same property is a TLS-terminating proxy in front, which the
node cannot see.

## When to use it, and when not

- **Use it when** a gateway is reached off the host, and there is no proxy in front to terminate TLS.
- **Not when** a TLS-terminating proxy already fronts the gateway, or clients must present certificates: the gateway
  serves server-side TLS only.

## Turn it on

Build with `tls` and `gateway`, and set the table on a node with an `http_port`. The two-domain scaffold's
[`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's two-domains job starts, sets an
empty `[gateway_tls]`, which reuses the node identity's certificate.

- [`gateway_tls`](../reference/configuration.md#gateway_tls): with no paths, the node identity's certificate (needs
  [`tls`](../reference/configuration.md#tls)); with `cert_pem_path` and `key_pem_path`, an operator certificate, which
  also needs `bulk_ca_pem_path` so a bulk fetch from a peer's gateway can be verified.
- [`handshake_timeout_ms`](../reference/configuration.md#handshake_timeout_ms) and
  [`gateway_max_connections`](../reference/configuration.md#gateway_max_connections): the listener's bounds, the same
  for plaintext and HTTPS.
- Both SDKs reach it with a `scheme` of `"https"`: the Python agent's
  [`agent.py`](../../mycelium-py/src/mycelium/agent.py) and the TypeScript agent's
  [`agent.ts`](../../mycelium-ts/src/agent.ts); verification stays on.
- `test_gateway_serves_native_tls`, in [`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs): a rustls
  client trusting the generated CA gets `/health` 200, and a plaintext client fails.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it; the
two-domain scaffold's CI job serves its gateways over HTTPS, pinned by the partner. A transfer between two HTTPS
gateways is `a_bulk_call_completes_between_two_gateway_tls_nodes`, in
[`src/agent/bulk.rs`](../../src/agent/bulk.rs).

## What it guarantees, and what it does not

- **Guarantee:** [`gw.tls`](../reference/guarantee-catalogue.md#gw.tls) — the gateway serves HTTPS. A proxy in front
  also provides it, but the node cannot see that, so its report does not count it.
- **Not built:** no client-certificate demand at the gateway; a connection that keeps talking holds its slot, so an
  exposed gateway still wants a per-source limit in front of it; the not-built items of the bulk path under
  `gateway_tls` are in [gateway TLS](../operations/gateway-tls.md) and row R7 of the
  [external review plan](../plans/external-review-2026-10-31.md).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row specific to gateway TLS.

## Go deeper

- **Guide:** [09, security](../guide/09-security.md).
- **Runbook:** [gateway TLS](../operations/gateway-tls.md) (both options, which certificate, verifying).
- **Design:** no design record; the SOC 2 audit-gap work is the record.
- **Code:** [`src/agent/http/listener.rs`](../../src/agent/http/listener.rs) (the accept loop and its bounds) and
  `prepare_gateway` in [`src/agent/http/mod.rs`](../../src/agent/http/mod.rs); the certificate in
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs).
