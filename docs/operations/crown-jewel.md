# Mycelium — Crown-Jewel Operations Runbook (data-at-rest + egress)

Operator guide to the WS3 crown-jewel controls: opt-in data-at-rest encryption
and the outbound egress allowlist. Blast-radius context:
[`../threat-model.md`](../threat-model.md). Both controls are **feature-free**
(no cargo feature required) and **opt-in** — absent, behaviour is unchanged.

---

## 1. Data-at-rest encryption

The substrate encrypts the **on-disk** persistence surface (WAL records +
snapshots) through an operator-supplied cipher. It does **not** encrypt the store
in memory or the gossip wire (the wire is the `tls` feature's job).

### Attach a cipher

Implement `DataAtRestCipher` over your KMS/keyring and attach it **before**
`start()`:

```rust
use mycelium::{DataAtRestCipher, GossipAgent};
use std::sync::Arc;

struct KmsCipher { /* handle to your KMS/keyring */ }
impl DataAtRestCipher for KmsCipher {
    fn encrypt(&self, plaintext: &[u8]) -> Vec<u8> { /* AEAD seal */ }
    fn decrypt(&self, ciphertext: &[u8]) -> Option<Vec<u8>> { /* AEAD open; None on auth fail */ }
}

let agent = GossipAgent::new(id, cfg);
agent.with_data_at_rest_cipher(Arc::new(KmsCipher { /* … */ }));
agent.start().await?;
```

### Operating rules

- **Key stability.** The key must be available and identical across restarts, or
  the node cannot replay its own WAL/snapshot (records fail to decrypt and are
  skipped — silent data loss on restart). Source it from a KMS/HSM, not a local file.
- **Use a real AEAD.** The hook hands you opaque bytes; use an authenticated
  cipher (e.g. AES-GCM / ChaCha20-Poly1305) so `decrypt` can return `None` on
  tamper. (The substrate's own test cipher is XOR — illustrative only, never
  ship it.)
- **Rotation** is the operator's concern: re-encrypting an existing on-disk store
  under a new key is an offline migration (decrypt-old → encrypt-new). There is no
  in-place rotation hook yet.
- **The node identity key is also a crown jewel** — protect `tls` key material
  with the same rigor (see [`../threat-model.md`](../threat-model.md) Boundary A).

### Verify it is working

A quick check that bytes are not plaintext on disk:

```bash
# With a cipher attached, a known plaintext value must NOT appear in wal.bin:
grep -a 'MY-KNOWN-VALUE' "$BASE/$NODE_ID/kv/wal.bin" && echo "NOT ENCRYPTED" || echo "encrypted"
```

---

## 2. Outbound egress allowlist

`EgressPolicy.allow_hosts` constrains which external hosts the substrate may
reach. It is a **node-local posture, not a coordinator** — set it per node.

```rust
cfg.egress = mycelium::EgressPolicy {
    allow_hosts: vec![
        "tools.internal".into(),   // exact host
        ".corp.example".into(),    // ".suffix" → host or any subdomain
    ],
};
```

- **Empty `allow_hosts` = allow all** (the default). A non-empty list is
  **fail-closed**: a host not matched — including a URL whose host can't be parsed
  — is denied.
- Matching is case-insensitive; `.suffix` matches the bare suffix and any
  subdomain (`.corp.example` matches `corp.example` and `api.corp.example`).

### Coverage

The gate is a **hostname** allow-list on the outbound HTTP paths the substrate *chooses*
to make. Since 2.23.0 it covers the first URL **and every redirect hop**, and reads a URL's
host with the same parser the client dials with (realignment repairs R3–R4). It does not
resolve names: an allowed name that resolves to an address you meant to deny is outside it.

| Outbound path | Gated in code? | Notes |
|---|:-:|---|
| MCP client bridge (`connect_mcp_server`) | ✓ `EgressPolicy`, every hop | denied → `Transport("egress denied")`; a redirect to an unlisted host fails the connect |
| LLM backend calls (prompt skills) | ✓ `EgressPolicy` | `handle_llm_invoke` → `egress_denied` if the backend endpoint host isn't allowed; `OpenAiBackend::new` follows no redirect, `.with_egress(policy)` re-checks each hop (a stem's `[[serve]]` uses it) |
| LLM backend calls (SkillRunner) | ✓ `EgressPolicy`, every hop | gated against the node's `egress_policy()` before the call |
| Capability HTTP probes | ✓ `EgressPolicy`, every hop | a blocked probe URL fails the probe (capability not advertised) |
| A2A **client** | — | client lives in the SDKs (Python/TS), not the substrate; restrict at the SDK / network layer |
| Federation client (`FederationClient`) | ✓ `EgressPolicy` (since 2026-10-03) | the node's policy applies to the clients it is handed (`with_federation_clients`); a client used outside an agent is built `.with_egress(policy)`. A denied endpoint is `ClientError::Egress`, refused before any byte is sent. **Follows no redirect**: its credential header is not stripped cross-host |
| OIDC JWKS / discovery | ✓ `EgressPolicy` (since 2026-10-03) | an `[oidc]` issuer or `jwks_uri` the allow-list does not permit **refuses `start()`** by name; at runtime a denied host is never dialled (no keys, every token refused), including through a redirect. Add the IdP's host to `allow_hosts` |
| Artifact HTTP library source (`HttpLibrarySource`) | ✓ when built `.with_egress(policy)` | a companion type constructed by the operator; `new()` alone allows all and follows no redirect; with a static header it never follows one |
| Object-store library (`ObjectStoreFetcher`, `--library s3://…`) | ✓ on the **endpoint** it dials | `AWS_ENDPOINT_URL_S3` / `AWS_ENDPOINT`, else `s3.<region>.amazonaws.com`; `storage.googleapis.com` or `GOOGLE_BASE_URL`. **List the endpoint host, not the bucket.** Azure and S3 Express are refused under a non-empty list. Not gated: the cloud identity's credential traffic, and redirects inside `object_store`'s own client |
| Ollama probe (`mycelium-reason`) | ✓ when built `.with_egress(policy)` | `new()` alone is ungated and follows no redirect |
| Wiki git mirror push (`GitMirror`) | ✓ `EgressPolicy` on the remote host | git runs with `http.followRedirects=false`; a remote whose authority holds a backslash, whitespace or a control character is refused |
| Bulk transport peer fetch | n/a | intra-cluster (peer URLs), not external egress — deliberately not gated; follows no redirect |

For the non-gated rows, enforce egress at the **network layer** (firewall rules,
security groups, an egress proxy with its own allowlist).

---

## 3. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| node refuses to start naming `snapshot.bin` or a WAL byte | cipher key changed/unavailable → the state is unreadable and `on_unreadable = "refuse"` (the default) fails closed | restore the exact key; `on_unreadable = "quarantine"` moves the files aside and starts from what was readable |
| plaintext visible in `wal.bin` | no cipher attached, or attached after `start()` | attach `with_data_at_rest_cipher` **before** `start()` |
| `connect_mcp_server` → "egress denied by policy" | target host not in `allow_hosts` | add the host (exact or `.suffix`) |
| data exfiltrated via an outbound path the list does not cover | `allow_hosts` gates the MCP bridge, LLM backends, capability probes, the federation client and OIDC discovery/JWKS (the table above); a path your own code opens is yours | add network-layer egress control (see §2) |
