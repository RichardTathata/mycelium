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

In a config file (`GossipConfig::load_from_file`, the `mycelium` binary's `-c`, a stem's `--config`):

```toml
[egress]
allow_hosts = ["tools.internal", ".corp.example"]
```

Since 2.23.0 the list is checked against the **first URL and every redirect hop** (at most five, never
https → http), with the host read by the HTTP client's own parser.

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
| OIDC JWKS / discovery | ✓ `EgressPolicy` (since 2026-10-03) | an `[oidc]` issuer, or a `jwks_uri` **you configured**, that the allow-list does not permit **refuses `start()`** by name; a `jwks_uri` taken from discovery is checked when keys are fetched — off the list, the node starts, logs `oidc: the egress policy does not permit the JWKS host; no keys, every token refused`, and answers every JWT 401 (Google: issuer `accounts.google.com`, keys at `www.googleapis.com` — allow both, or set `jwks_uri`); at runtime a denied host is never dialled (no keys, every token refused), including through a redirect. Add the IdP's host to `allow_hosts` |
| Artifact HTTP library source (`HttpLibrarySource`) | ✓ when built `.with_egress(policy)` | a companion type constructed by the operator; `new()` alone allows all and follows no redirect; with a static header it never follows one |
| Object-store library (`ObjectStoreFetcher`, `--library s3://…`) | ✓ on the **endpoint** it dials | `AWS_ENDPOINT_URL_S3` / `AWS_ENDPOINT`, else `s3.<region>.amazonaws.com` (region from `AWS_REGION` / `AWS_DEFAULT_REGION`, default `us-east-1`; `<bucket>.s3.<region>.amazonaws.com` under `AWS_VIRTUAL_HOSTED_STYLE_REQUEST=true` — `.amazonaws.com` covers both); `storage.googleapis.com` or `GOOGLE_BASE_URL`. **List the endpoint host, not the bucket.** Azure and S3 Express are refused under a non-empty list. Not gated: the cloud identity's credential traffic, and redirects inside `object_store`'s own client |
| Ollama probe (`mycelium-reason`) | ✓ when built `.with_egress(policy)` | `new()` alone is ungated and follows no redirect |
| Wiki git mirror push (`GitMirror`) | ✓ `EgressPolicy` on the remote host | git runs with `http.followRedirects=false`; a remote whose authority holds a backslash, whitespace or a control character is refused |
| Bulk transport peer fetch | n/a | intra-cluster (peer URLs), not external egress — deliberately not gated; follows no redirect |

For the non-gated rows, enforce egress at the **network layer** (firewall rules,
security groups, an egress proxy with its own allowlist).

### Credentials attached at egress (post-360 plan row L, 2.35.0)

A secret need not sit in a unit file, a component's environment or a backend's fields. Declare a
**credential binding** on the node; a caller holds only its **placeholder**
(`mycelium-credential:<name>`, `mycelium::credential_placeholder`) and puts it where the secret would
go. The node's egress gate (`mycelium::EgressCredentials`) swaps the placeholder for the value **only**
on a request to the host the binding names:

```toml
[[egress_credentials]]
name        = "openai"
host        = "api.openai.com"        # exact host — never a suffix
# port      = 443                     # optional: the one port (the scheme's default counts); REQUIRED for localhost (any spelling) and any IP literal
path_prefix = "/v1"                   # optional; /v10 and /v1/../admin are outside it
form        = "bearer"                # or "header" + header = "x-api-key", or "basic" + username = "svc"
value_file  = "/run/secrets/openai"   # or value_env = "OPENAI_API_KEY"; read once, at start()
# allow_plaintext = true              # attach over http to a non-loopback host (default: refused)
```

Env form, which replaces the TOML's bindings: `GOSSIP_EGRESS_CREDENTIALS="openai|api.openai.com/v1|bearer|file:/run/secrets/openai"`
(`name|host[:port][/prefix]|bearer, header:<name> or basic:<user>|env:<VAR> or file:<path>[|plaintext]`, entries split by `;`).
A refusal names a binding by its index and name and the field that failed — never a field's contents, so a value
pasted into the wrong field does not reach the start-time log.

What the gate does, per request:

- **attaches** the value in its form, replacing the header that carried the placeholder, when the URL's host
  is the bound host (case-insensitive), its port is `port` (when set), its normalised path is under `path_prefix`,
  and the scheme is https (or http to `localhost` / a loopback address, or `allow_plaintext`). A binding to
  `localhost` in any spelling (`localhost.`, `*.localhost`) or to any IP literal (loopback, IPv4-mapped, `0.0.0.0`,
  `[::]` included) must name its port: each port is a different process, and the rule is on what the host is, not
  how it is spelled;
- **refuses** the request — before a byte is sent — anywhere else, when a placeholder names no binding, and when two
  bindings would set the same header;
- follows redirects itself — 301, 302, 303, 307 and 308 only; any other 3xx is handed back as it arrived — through the
  allow-list's per-hop verdict (≤ 5, never https → http), and **drops** the credential on a hop the binding does not
  authorise, another port on the same host included (no value, no placeholder);
- **counts** each binding's attachments, refusals and redirect drops (`EgressCredentials::uses()`; under
  `metrics`, `mycelium_egress_credential_total{binding, event}`) — the label is the binding's name.

The value is held only in the gate, as a sensitive header value: no `Debug`, `Display`, error message, log
line or trace field carries it, and nothing writes it to the evidence journal. Guarantee
`egress.credentials` reports `enforced` when a binding is configured.

Who sends through the gate: `[[serve]]`'s `credential = "<binding>"` (a stem refuses to start if the node has
no such binding, or if the binding does not authorise the section's endpoint), `OpenAiBackend::new(url, credential_placeholder("openai"), model).with_credentials(gate)`,
and your own code via `agent.egress_credentials()` → `EgressCredentials::send(request)` (or `send_once`, which
follows no redirect). A placeholder request goes out under the egress policy the gate captured at `start()` — the
node's — not a backend's own `with_egress` policy.

**An `[[activation]]` command** reaches the gate through its stem's **egress proxy** (rows L and M, after 2.35.0):
an `[activation.confine.egress]` table names the hosts the command may reach and the bindings it may name
(`credentials = ["openai"]`); the stem runs a forward proxy on loopback for that activation, confines the command
to the proxy's port, and starts it with a **cleared environment** (as every confined command now starts). At the
stem's start, for every confined activation, a binding's value is kept from the command three ways, each a refusal by
name: an `[activation.confine] env` entry naming a binding's `value_env`; an `env` entry whose **value** holds a
binding's value, alone or inside a longer string; a binding's `value_file` under the placement root, a declared
`read` or `write` path, or the platform's always-readable set, or with a second hard link. This is checked once, at
start: a file moved or linked into a readable path afterwards is not seen. The command sends
`http://api.openai.com/v1/…` through `HTTP_PROXY` with `Authorization: Bearer mycelium-credential:openai`; the proxy
checks the host against the activation's list, the binding against its `credentials`, and sends the request on over
the entry's `https` through `EgressCredentials::send_once`, which attaches the value only where the binding
authorises. Every answer relayed back has the granted bindings' values **masked** — headers, and the body across
chunk boundaries (`mycelium::Redactor`) — and an encoded body is refused where a value could hide in it, so an
upstream that echoes the value hands the command nothing. An `https://` URL the command dials itself goes through
`CONNECT` — a tunnel, no credential attached (the proxy does not terminate TLS) — and a tunnel to a granted
binding's host is refused, so that host is only reached where its answers are checked. The proxy requires a per-proxy token (in the proxy URL the command's environment
carries), so another local process cannot have a credential attached through it. Every field, and what it holds per
backend: [`unit-file.md` § `[activation.confine.egress]`](../reference/unit-file.md#activationconfineegress).

**The places a secret is read into a node, and which use a binding:**

| Secret | Path today | Behind a binding? |
|---|---|---|
| `[[serve]]` bearer | `api_key` (literal), `api_key_env` (read at stem start) | **yes**, with `credential = "<binding>"`; the two old forms are kept unchanged |
| `OpenAiBackend` key (prompt skills you register) | the `api_key` field | **yes**, with a placeholder and `.with_credentials(gate)`; a literal still works |
| SkillRunner's LLM key | its own config's `api_key` | unchanged — a separate binary with its own config |
| `HttpLibrarySource` static headers | `.with_header(…)` by the embedding code | unchanged — a companion without the gateway's client; it follows no redirect while it holds a header |
| Federation credential | a per-call header signed with the node's key | unchanged — a signature over the call, not a static secret; its client follows no redirect |
| Object-store credentials (S3/GCS) | the environment, read by `object_store` | unchanged — request signing inside `object_store`'s client, not a header the gate can attach |
| Wiki git mirror push | git's own credential helpers | unchanged — git's process, not an HTTP client of the node's |
| OIDC | none sent (JWKS and discovery are public) | n/a |
| Artifact signing keys (`mycelium-artifact`) | `--key` (a file) or `--key-env` (a variable) | unchanged — used to sign locally, never sent |
| An `[[activation]]` command | inherits the stem's whole environment — unless its profile names `[activation.confine.egress]` | **yes**, with `[activation.confine.egress] credentials = ["<binding>"]`: the stem's egress proxy attaches the value at the bound host and masks it in what comes back; every confined command starts with a cleared environment, and a binding whose value the command could read (a `value_env` or same-valued `env` entry, a `value_file` within its read paths) refuses the stem's start. **Unchanged** for an **unconfined** activation, which inherits the stem's whole environment (a `value_env` variable included — prefer `value_file`) and can read what the stem's user can; and not covered: a value an upstream returns transformed (re-encoded, hashed) rather than as its bytes |
| Gateway tokens, mTLS keys | inbound credentials, not egress | n/a |

**Not covered:** a placeholder in a query string or a request body (sent as written — an opaque name); zeroising the
value when the gate is dropped; a binding whose `name`, `header` or `username` is itself a secret (pasted into the wrong field)
is printed — its `Debug`, a refusal naming the binding, the drop log line — and used as the metric label or as the
header name sent (`value_env` and `value_file` are redacted in `Debug`, and an env refusal names its entry by index
alone); the gate's client honours `HTTP_PROXY` / `HTTPS_PROXY`, so a configured proxy sees a
plaintext or loopback request's header (an https request's is inside the tunnel). At an activation's egress
proxy: a placeholder inside a `CONNECT` tunnel or in a URL or body; a credential the command reads from a file its
profile lets it read; under Landlock, a connection to the proxy's port on another address (Landlock limits ports,
not addresses — the `stem.activation_egress` guarantee says so).

---

## 3. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| node refuses to start naming `snapshot.bin` or a WAL byte | cipher key changed/unavailable → the state is unreadable and `on_unreadable = "refuse"` (the default) fails closed | restore the exact key; `on_unreadable = "quarantine"` moves the files aside and starts from what was readable |
| plaintext visible in `wal.bin` | no cipher attached, or attached after `start()` | attach `with_data_at_rest_cipher` **before** `start()` |
| `connect_mcp_server` → "egress denied by policy" | target host not in `allow_hosts` | add the host (exact or `.suffix`) |
| `egress: the redirect target … is not on the allow-list` / `egress: a redirect from https to http is refused` / `egress: more than 5 redirects` (2.23.0) | an allowed URL redirects off the list, downgrades, or loops | allow the target host, or point the setting at the final URL |
| `egress policy denies <url>: the store dials <host>` (2.23.0) | an object store's **endpoint** is not on the list (a bucket name admits nothing) | allow the endpoint host — [artifacts.md § Remote blob stores](artifacts.md) has the table |
| every JWT answers 401; log `oidc: the egress policy does not permit the JWKS host` | a discovered `jwks_uri` on a host not on the list | allow the JWKS host or set `jwks_uri` explicitly (then `start()` checks it) |
| data exfiltrated via an outbound path the list does not cover | `allow_hosts` gates the MCP bridge, LLM backends, capability probes, the federation client, OIDC discovery/JWKS, object stores, the wiki git mirror, and — when built `.with_egress` — the Ollama probe and HTTP library source (the table above); a path your own code opens is yours | add network-layer egress control (see §2) |
