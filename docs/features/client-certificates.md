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
- **Not when** you need a call *signed* by that key: the `cs`/`cn` signature, the SDK `tools/call` clients and grants
  held by a client principal are not built yet ([identity and lifecycle § I1](../plans/identity-and-lifecycle.md#i1--client-certificates-for-services-d1-d1)).

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
  `sys/client-cert/{principal-hex}/{key-hex}` ([`CLIENT_CERT_PREFIX`](../../src/agent/client_cert.rs)).
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
- **Across a restart:** under `[persistence]` the newest issuance is journaled, within a budget of 64 supersessions
  per principal and 65,536 in all. A principal past the budget keeps its newest mark only in this process. Without
  `[persistence]`, a restart forgets the mark until the newer record is read again.
- **Two certificates in one millisecond:** the greater key wins.
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
  - a client's call signed by its key (`cs`/`cn`), and its cross-language vectors;
  - refusing an unsigned action from a principal with a certificate on record;
  - the SDK `tools/call` clients;
  - grants held by a client principal, and `POST /gateway/grants/request`;
  - a shared `TrustedExternalIssuers`;
  - refusing at call time a client's call to a gateway without `gateway_identity_issuer`;
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
  [`src/agent/client_cert.rs`](../../src/agent/client_cert.rs).
