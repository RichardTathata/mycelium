# Gateway TLS

↑ [operations](README.md)

The embedded HTTP gateway (`http_port`) serves the REST/SSE/WS surface — capability advertise,
KV, signals, RPC, governance, audit query. **By default it is plaintext HTTP.** Bearer tokens and
OIDC JWTs sent to `/gateway/*` therefore traverse the wire in the clear unless you terminate TLS.
Two supported ways to fix that (SOC 2 CC6 "encryption in transit"):

## Option 1 — front it with a TLS-terminating proxy (unchanged, always available)

Run the gateway on loopback (`http_addr = "127.0.0.1"`, the default) behind nginx / Envoy / a
service-mesh sidecar that terminates TLS with your own cert. Nothing to configure in Mycelium.
This is the right choice when you already run a proxy/mesh or want a hostname cert managed centrally.

## Option 2 — native gateway TLS (`GossipConfig::gateway_tls`, 2026-07-22)

Set `gateway_tls` and the gateway serves HTTPS directly — no proxy required. It is **server-side
TLS only** (no client-cert demand), unlike the mutually-authenticated gossip transport (`tls`), so
ordinary HTTPS clients (curl, browsers) connect normally. **The SDKs through `mycelium-py` 0.2.8,
`mycelium-ts` 0.2.2 and `langgraph-checkpoint-mycelium` 0.3.1 do not:** each builds an `http://` URL
(`mycelium-py/src/mycelium/agent.py`, `mycelium-ts/src/agent.ts`, the checkpointer's `saver.py`), so from them
a gateway serving only HTTPS is reached only through something on the client's side that originates TLS (a
service-mesh sidecar, a local proxy). Check your SDK's README for a `scheme` option before relying on it.

```rust
use mycelium::{GossipConfig, GatewayTlsConfig, TlsConfig};

let mut cfg = GossipConfig::default();
cfg.http_port = Some(9443);

// (a) Reuse the node identity cert — both fields None. Requires `tls`.
cfg.tls = Some(TlsConfig::default());
cfg.gateway_tls = Some(GatewayTlsConfig::default());

// (b) Operator-supplied cert with a real hostname SAN (browser trust):
cfg.gateway_tls = Some(GatewayTlsConfig {
    cert_pem_path: Some("/etc/mycelium/gateway-cert.pem".into()),
    key_pem_path:  Some("/etc/mycelium/gateway-key.pem".into()),
    // Required with an operator cert (R7): the CA that signs the fleet's gateway certificates,
    // which a bulk fetch from a peer's gateway is verified against.
    bulk_ca_pem_path: Some("/etc/mycelium/gateway-ca.pem".into()),
});
```

**Which cert?**

| Mode | Cert used | Trust story |
|---|---|---|
| Both fields `None` (default) | The node identity cert (from `tls`), served with no client-cert demand | Carries an **IP SAN** only — fits **CA-pinning clients** and proxied setups, **not** hostname/browser trust. Requires `GossipConfig::tls` (else startup errors). Rotates automatically with the node identity (hot cert rotation). |
| `cert_pem_path` + `key_pem_path` | Your PEM cert chain + PKCS8 key | Use a cert with a real **DNS SAN** for browser/hostname clients. Both fields must be set together. You own its rotation. |

**Feature gate.** Native gateway TLS needs the `tls` feature (which `compliance` implies). A
`[gateway_tls]` table in a build without `tls` **refuses to start** by name (since 2.18.1) — it is never
silently plaintext.

**Start-time binding (since 2.20.0).** `start()` binds the gateway port and loads its certificate before
the node reports ready. A port already in use, or an unreadable `cert_pem_path` / `key_pem_path`, fails
`start()` with `InvalidField { field: "gateway", .. }` naming the address and the cause, rather than
leaving a dead listener behind a node that answers `/ready`. Free the port or fix the paths and start
again.

**Bulk transfers are covered under native gateway TLS (since R7 of the external review of 2026-10-31).** A
`bulk_call` stages its payload on the caller's gateway and the responder fetches it from there. A caller whose
gateway serves TLS says so on its ticket, and the responder fetches over `https`, verifying the caller's gateway
certificate **by the caller's IP address** — the address its frames came from — against the union of two root sets:

- the fleet CA from `[tls]`, with the mesh's removal and revocation checks (a node-cert gateway's certificate);
- `bulk_ca_pem_path`, when set: the CA that signs the fleet's operator certificates. A fleet certificate the fleet
  check refuses (removed, revoked, expired) is never re-tried against these.

So a fleet may mix node-cert and operator-cert gateways, provided every node that fetches from an operator-cert
gateway names its CA in `bulk_ca_pem_path`. On a node serving a gateway (`http_port` set) on an operator certificate,
`start()` refuses by name (`InvalidField { field: "gateway_tls.bulk_ca_pem_path" }`) unless `bulk_ca_pem_path` is set
**and the node's own certificate verifies against it for the node's own IP address**: each operator certificate must
carry its node's **IP SAN** as well as any hostname. A `bulk_ca_pem_path` that cannot be read or holds no certificate
is refused the same way. The operator certificate is read once at `start()`; to rotate it, replace the files and
restart. The node-cert gateway presents the current identity at every handshake, so after `rotate_identity` it serves
the new certificate at once.

A fetch whose certificate does not verify fetches nothing, and a fetch is never retried in plaintext; nor is a status
other than 2xx, or a payload over `max_bulk_payload_bytes` (default 64 MiB), handed to the handler. Each such refusal
is logged with its reason and counted (`SystemStats::bulk_fetches_refused`, `mycelium_bulk_fetches_refused_total{reason}`)
— the caller sees only its `Timeout`. The `https` fetch client keeps no idle connections, so every fetch is a fresh
handshake checked against the live removal and revocation sets (the reason the mesh turned resumption off).

**Not covered:**
- **A plaintext gateway still carries bulk payloads in the clear.** Without `gateway_tls` the caller's ticket says
  `http`, and the payload crosses the network unencrypted whatever `[tls]` does for the mesh.
- **A TLS-terminating proxy in front of a plaintext gateway is bypassed for bulk.** The ticket names the gateway's own
  `http_port` and the scheme `http`, and the responder fetches from the caller's IP directly, not through the proxy.
  Under a proxy, bulk payloads are plaintext between nodes; use native gateway TLS where bulk matters.
- An integrity check on the payload beyond TLS (no per-payload signature); the fetch target is still the address the
  caller's frames came from.
- A responder older than R7 drops an `https` ticket, so a call to it times out until it is upgraded — as every bulk call
  to or from a TLS gateway did before R7.

**Behaviour.** The listener terminates rustls (ring provider) per connection and serves the axum app
over the TLS stream. On shutdown it stops accepting; in-flight connections drain. A handshake from a
client that does not trust the cert simply fails — the gateway never falls back to plaintext.

## Verifying

```bash
# node-cert mode — pin the generated cluster CA (IP SAN matches 127.0.0.1):
curl --cacert ./mycelium-tls/ca-cert.pem https://127.0.0.1:9443/health
# operator-cert mode with a hostname SAN:
curl https://gateway.example.com:9443/health
```

Regression gate: `agent::http::tests::test_gateway_serves_native_tls` (a rustls client validating
against the generated CA completes a real handshake and gets `/health` 200; a plaintext client fails).

## Shared-responsibility note

Encryption-in-transit for the gateway is **Shared**: Mycelium provides native TLS *or* you terminate
with a proxy — pick one; do not leave the gateway plaintext on a routable interface. See the
[shared-responsibility matrix](shared-responsibility-matrix.md).
