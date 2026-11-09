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

[`examples/eve_weather/run_client_grant.mjs`](../../examples/eve_weather/run_client_grant.mjs) runs a service end to
end on three nodes, the gateway the `mycelium` binary with `[authority]`. It issues and publishes the service's
certificate, asks for a grant on Eve's weather tool, has the treasurer issue it, calls under it through `McpClient`
with a `ClientSigner`, restarts the gateway and calls under a second grant, revokes a third, and supersedes the key.
The [example index](../../examples/README.md#the-capability-matrix) lists its tool's provider,
[`eve_fleet_node`](../../examples/README.md#the-capability-matrix) ([`examples/eve_fleet_node.rs`](../../examples/eve_fleet_node.rs)). It ends with
`CLIENT GRANT OK` (CI's `eve-fleet` job; the release builds' run is on
[the ledger](../operations/what-is-proven.md#client-grant-three-node-fleet)).

`a_client_certificate_is_refused_at_the_mesh_both_ways`, in
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
- **Refusals:** `client_key_unknown` means this node has not read the key's certificate yet and is worth retrying (under
  a grant too, which the gateway gives back);
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
- **The gateway checks the signature before it claims a grant's use.** At `/mcp` and both `/a2a` doors the gateway
  checks, before its preflight runs:
  - `cs` over the call's binding;
  - the key's standing;
  - the issue time on the provider's window (`stale`).

  So a call refused there leaves a grant it presents unspent
  (`a_signed_call_presenting_a_client_grant_runs_at_mcp`, `both_a2a_doors_carry_a_client_signature_and_the_skill_sees_it`,
  `the_door_refuses_a_stale_an_unsafe_and_an_unverifiable_signature_by_name`). It does not check `cn`: the replay
  window is the provider's, which checks everything again.
- **At `/mcp`, a grant the provider refused before running is given back.** The gateway's fold of client certificates
  can lead the provider's. When the provider node answers `client_key_*`, `client_signature_*` or `stale`, nothing ran:
  its tool serve loop refused the call before the handler, answering `-32022` with the reason in `data.reason`. The
  gateway then tombstones its `…/used/{door}` marker and releases its claim, so `client_key_unknown` is retriable under
  the same grant (`a_grant_the_provider_refused_before_running_is_given_back`).
  - **Only the node authors such a refusal.** A native tool's answer is wrapped in `result` or answered `-32603`. A
    bridged MCP server's reply in the node's refusal shape is rewritten into an ordinary tool error, `-32000`
    (`a_bridged_tool_answering_like_a_refusal_after_running_leaves_the_grant_spent`). A tool that ran its effect cannot
    have its grant given back.
  - **Trust:** the release trusts the provider node itself. A node that ran the call and answered a refusal could have
    it run twice, as a node able to run calls at will could anyway.
  - **What stays spent:** a tool's own error, a timeout, and any other answer.
  - **History limits:** a released grant's call stays counted against its history, as an attempt the permit admitted.
  - **At `/a2a` a provider's refusal spends the grant.** A skill answers with raw output, which the node does not
    author, so the A2A doors give nothing back. A grant is only ever for a tool call, so the case does not arise there
    today.
- **Refusal reasons reach the caller.** At `/mcp` a provider's caller-context refusal is `-32022` with the reason in
  `data.reason`. At `/a2a` it is a JSON-RPC error naming the reason, or a failed task on a stream. The evidence journal
  records it `failed`, not `completed` (`a_providers_refusal_reaches_an_mcp_client_as_json_rpc_with_its_reason`,
  `a_providers_refusal_is_a_failed_task_at_both_a2a_doors_and_on_the_record`). A skill's reply is its raw output, so a
  skill whose output is exactly `{"error": "…", "reason": "…"}` is read as refused.
- **A gateway with no identity.** A build with `tls` and no `[tls]` holds no CA anchor, so it judges no client key: a
  signed call is refused `client_signature_unverifiable`.
- **A name holding `@`.** The signed message binds the bare resource whole, `tool:weather@v2`, in all three languages
  (the cs vector's `at_case`). The provider reads the tool or skill from the envelope's claim up to its last `@`, so
  such a name binds and is not confused with another.
- **After a peer restarts, its first calls can time out.** A gateway restarted gracefully can dispatch calls that time
  out until the provider notices the dead connection (`/stats` reports both peers throughout): 1–2 s measured with a
  1 s health tick, and a probe each way before calling narrows the window without closing it (five of twelve exit runs
  still timed out once). A default fleet's window is inferred to be longer from the 10 s default tick; it was not measured. A
  timed-out call has spent the grant it presents. See [identity and lifecycle §9 O2](../plans/identity-and-lifecycle.md#9--open-items-tracked) and
  [issue #746](https://github.com/RichardTathata/mycelium/issues/746).
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
- **Not built:** refusing an unsigned action from a principal with a certificate on record (counted now), listed under
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
