# Client certificates for services

## What it is

The fleet CA can issue a **service**, such as an SDK agent or a supervisor process, a certificate of its own. The
certificate names `mycelium-client://{issuer}/{name}` for the [gateway principal](../guide/00-concepts.md#gateway-caller)
`token:{issuer}/{name}`. A service's key never joins the mesh, because both mesh verifiers refuse the scheme. A
service's key is retired by issuing the same principal a newer certificate, which **supersedes** the older key at
every node that reads it.

## When to use it, and when not

- **Use it when** a service acting through a gateway needs a key that the fleet CA vouches for, with an issuer the
  fleet fixes (`gateway_identity_issuer`).
- **Use it to sign a call:** the service signs each `tools/call` or A2A send with that key, and the provider checks
  that the call is the one signed and the key is the principal's current one.
- **Use it to hold a grant:** the service asks for one at `POST /gateway/grants/request` and proves possession with
  that key ([grants](grants.md)).
- **Not when** the caller is a node's own agent: a node signs with its identity key, and its grants are held by the
  node ([grants](grants.md)).

## Turn it on

- `mycelium tls issue --ca-dir <dir> --client <name> --issuer <issuer> --out <dir>`, on the host that holds the
  fleet CA's key ([`tls_issue_client`](../../src/main.rs)). The issuer is the fleet's
  [`gateway_identity_issuer`](../reference/configuration.md#gateway_identity_issuer), which must be set explicitly
  to 1–128 of `A-Z a-z 0-9 . _ -`: its default, the node id, holds a `:` and is refused as a client issuer. Issuing
  over an existing key file needs `--force`.
- Upgrade every node first: an older node admits a client certificate at the mesh
  ([`deprecations.md` §86](../guide/deprecations.md)).
- [`tls`](../reference/configuration.md#tls): a node judges a client certificate against its CA anchor, so it needs
  `[tls]`.
- [`persistence`](../reference/configuration.md#persistence): keeps a supersession across a restart.
- `POST /gateway/identity/client-cert`, under `identity:write`, in a build with `compliance`
  ([`gw_identity_client_cert`](../../src/agent/http/compliance.rs)). It publishes the certificate as
  `sys/client-cert/{principal-hex}/{key-hex}` ([`CLIENT_CERT_PREFIX`](../../src/agent/client_cert.rs)). It publishes
  only certificates for its own `gateway_identity_issuer`, and its audit record names the bearer who published.
- Sign each call with a `ClientSigner` (Rust [`ClientSigner`](../../src/agent/client_sig.rs), Python
  `mycelium.ClientSigner`, TypeScript `ClientSigner`) and send what it returns as `params._meta.cs` and
  `params._meta.cn`. A signed call needs the gateway's `gateway_identity_issuer` set, or the door refuses it
  (`gateway_identity_issuer_unset`). Python needs `pip install 'mycelium-py[sign]'`.
- From the SDKs, give the clients the signer and they sign every call: Python `McpClient(url, signer=…)` and
  `A2aClient(url, signer=…)`, TypeScript `new McpClient(url, { signer })` and `new A2aClient(url, { signer })`.
  `McpClient.call_tool` / `callTool` sends a `tools/call` to `/mcp`; `A2aClient`'s `send` and `stream` sign over the
  skill and `{"text": message}`. A grant the service holds is asked for with `request_grant` / `requestGrant`, read
  with `grant_status` / `grantStatus` (or waited for with `wait_for_grant` / `waitForGrant`), and presented with
  `call_tool(…, grant=…)` / `callTool(…, { grant })`, which proves possession with the same key
  (`ClientSigner.prove_possession` / `provePossession`, over the grant's canonical bytes — the vector
  `tests/fixtures/possession/vector.json`, checked in Rust, jest and pytest; the proof binds the gateway's mandate
  digest — members by UTF-8 bytes, TypeScript `mandateArgumentsDigest` — not the signature's JCS digest)
  ([`mcp.py`](../../mycelium-py/src/mycelium/mcp.py), [`mcp.ts`](../../mycelium-ts/src/mcp.ts)).
- `the_mcp_door_carries_a_client_signature_counts_an_unsigned_one_and_needs_an_issuer`, in
  [`src/agent/client_sig_tests.rs`](../../src/agent/client_sig_tests.rs), runs a signed call through `/mcp`.
- `the_client_cert_door_is_identity_write_and_publishes_what_it_judged`, in
  [`src/agent/client_cert_tests.rs`](../../src/agent/client_cert_tests.rs), runs the door.

## See it work

No runnable example exists. `a_client_certificate_is_refused_at_the_mesh_both_ways`, in
[`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs), issues a client certificate and drives a handshake each
way. `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it`, in
[`src/agent/client_cert_tests.rs`](../../src/agent/client_cert_tests.rs), retires a key by re-issue.

## What it guarantees, and what it does not

- **At the mesh:** a certificate naming `mycelium-client://` is refused by the accept side and by a dialling node.
  A node certificate, including a pre-row-Q one that names no node, is still admitted. Measured against the
  binary on the [ledger](../operations/what-is-proven.md#client-certificate-refused-at-mesh).
- **Which key is current:** a record counts only if the fleet CA issued it, it names exactly the principal and key its
  path names, and it was not issued later than the wall clock plus the skew bound. A record's KV write time never
  counts. Per principal the node keeps the newest issuance it has read, and only that key is current. A later
  overwrite or tombstone of the newer record cannot bring the older key back.
- **Across a restart:** under `[persistence]` the newest issuance is journaled, within a budget of 64 journal records
  per principal and 65,536 in all. A node that only reads records spends one record per supersession, so about 64.
  The gateway that publishes them spends two (the new mark and the record it republishes), so about 32. A principal past the budget keeps its newest mark only in this process. Without
  `[persistence]`, a restart forgets the mark until the newer record is read again.
- **Two certificates in one millisecond:** the greater key wins.
- **A signed call:** the provider checks the signature at its receive boundary, before any cache. The key must be the
  principal's current one, so a superseded or unknown key is refused. The signature must
  cover this call's operation, resource and arguments (`client_signature_invalid`). The issue time must be inside
  the caller envelope's freshness window. Each `cn` is admitted once per process: a replay is dropped unanswered, a
  reordered pair is admitted, and two processes under one certificate never collide. A kind whose call the provider
  cannot read refuses a signature (`client_signature_unbound`); `tools/call` and `skill.invoke` bind.
- **One signature, every provider of the tool.** The signature names the resource before `@`, not the provider the
  gateway resolved, and each provider keeps its own `cn` window. So one signed call presented to N providers of the
  same tool runs once at each. Binding the provider would need the client to know which one the gateway will pick, and
  it does not.
- **The `cn` window is in memory.** A provider that restarts forgets it, so a call captured before the restart can
  replay once after it, inside the freshness window.
- **Refusals:** `client_key_unknown` means this node has not read the key's certificate yet and is worth retrying;
  `client_key_superseded` is final. More than 1,024 processes signing under one key inside the window are answered
  `client_instances_exhausted`. The arguments' digest is RFC 8785 (JCS), the same in Rust, Python and TypeScript.
- **Large numbers go as strings.** Any number whose magnitude is 2^53 or more, float or not (e.g. `6.022e23`), is
  refused: send it as a string. RFC 8785 writes such a number as the nearest double, so the signature could not tell
  it from its neighbour. The rule covers floats because JavaScript cannot tell `1e16` from `10000000000000000`: every
  double of that magnitude is an integer. Every signer refuses such arguments, and the provider refuses the call
  (`client_signature_unsafe_integer`).
- **An unsigned call** from a principal with a certificate on record is counted at the `/mcp` and `/a2a` doors and at
  the provider that runs it (`gateway_actions_unsigned`, `provider_actions_unsigned` on `/stats`), not refused yet
  ([`deprecations.md` §87](../guide/deprecations.md)). The gateway's signature does not cover `cs`/`cn`, so a relay
  that strips them makes the call unsigned. The LLM doors refuse a signature (`client_signature_unbound`).
- **A grant presented with a signature that fails is spent at the gateway.** The gateway claims a grant's use at its
  preflight, before the provider checks `cs`, so a call refused `client_signature_invalid` under a grant has used the
  grant at the gateway's door (`a_signed_call_presenting_a_client_grant_runs_at_mcp` states it).
- **A possession proof that does not verify is `WrongHolder`**, whether the key is wrong or the client's digest of the
  arguments differs (the O1 float case): the gateway sees only the proof.
- **Reserved name:** no client certificate may be named `legacy`, the legacy token's principal.
- **Ordering is the issuing host's clock.** The issuance is the serial, in ms, from the clock of the host that ran
  `mycelium tls issue`. A host whose clock runs behind can issue a certificate that orders below the one it
  replaces, and the door refuses it as superseded. A certificate without a readable issuance (a serial longer than
  8 bytes, as other PKI tooling writes) is refused by name.
- **Republished by its publisher.** The gateway that published a record writes it again when it is missing or
  overwritten, so a node that joins later, or restarts without `[persistence]`, still reads the newer certificate.
  Under `[persistence]` the publisher keeps its records across its own restart. A record whose publishing gateway
  is gone for good, and which every remaining node has lost, is not restored. It leaves alone a certificate for the
  same key that the CA issued at or after its own, and journals a record only when it changes, within the budget.
- **Cost:** a refresh scans the records once and checks each certificate's chain once while it is cached (4,096
  certificates). Past that many crafted certificates the cache churns and every chain is checked again. The refresh
  runs in the identity watcher's loop, so a large one delays the revocation fold; moving it off is a follow-up.
- **Not built:**
  - refusing an unsigned action from a principal with a certificate on record (counted now);
  - the SDK clients run against a live node (they are tested against a recording stub, and the possession vector
    against the gateway's verifier);
  - the three-node fleet exit run;
  - an un-upgraded node's reading of `sys/client-cert/`, which was not shown against the previous binary.

  All are listed under
  [identity and lifecycle § I1](../plans/identity-and-lifecycle.md#i1--client-certificates-for-services-d1-d1).

## Go deeper

- **Plan:** [identity and lifecycle § I1](../plans/identity-and-lifecycle.md#i1--client-certificates-for-services-d1-d1).
- **Related:** [node identity, proofs and revocation](node-identity.md), [the HTTP gateway](http-gateway.md),
  [gateway caller identity](gateway-caller-identity.md).
- **Code:** `refuse_if_client`, `client_identity` and `issue_client_cert` in
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs); the fold in
  [`src/agent/client_cert.rs`](../../src/agent/client_cert.rs); the signature in
  [`src/agent/client_sig.rs`](../../src/agent/client_sig.rs) and its vector in `tests/fixtures/cs/vector.json`; the
  SDKs' clients in [`mycelium-py/src/mycelium/mcp.py`](../../mycelium-py/src/mycelium/mcp.py) and
  [`mycelium-ts/src/mcp.ts`](../../mycelium-ts/src/mcp.ts), the possession proof beside the signer, its vector in
  `tests/fixtures/possession/vector.json`.
