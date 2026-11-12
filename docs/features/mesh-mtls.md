# Mesh mTLS and signed writes

## What it is

With a `[tls]` table, in a build with `tls`, the gossip mesh is mutually authenticated: every peer presents a
certificate the fleet CA signed, a node joins by holding one, and a removed or revoked member is refused at the
handshake. A reconnect is a full handshake, never a resumed session. On top of the transport, every KV write frame is
signed by its writer, and a TLS node refuses an unsigned or bare-signed frame, so a member cannot write as another by
not signing. Peer admission is the CA, never `cluster_name` ([things called admission](../guide/00-concepts.md#admission)).

## When to use it, and when not

- **Use it when** a fleet runs anywhere but one trusted host: it is the base every identity, revocation, audit and
  profile guarantee stands on.
- **Not when** you expect it to authenticate anti-entropy: a state reply's entries carry no author and are applied
  by last-writer-wins ([what-is-proven § anti-entropy](../operations/what-is-proven.md#anti-entropy-entries-authenticated)).

## Turn it on

Build with `tls` (on `mycelium` and `mycelium-core`; the published node has it) and set the table. The two-domain
scaffold's [`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's two-domains job starts,
sets `[tls]` with a certificate issued off-node.

- [`tls`](../reference/configuration.md#tls): `auto_cert_dir` alone generates a CA and certificates for development;
  `cert_pem` and `key_pem`, with only the CA's certificate in `auto_cert_dir`, take a certificate issued off-node,
  which keeps the CA key off the node
  ([node identity](node-identity.md)).
- `mycelium tls ca` and `mycelium tls issue`, in [`src/main.rs`](../../src/main.rs): make the fleet CA and issue a
  node's certificate where the CA key is. With `--format json` each, and `tls pin`, prints one document, schema
  `mycelium.tls/1`, of paths and SPKI fingerprints only — never a key
  ([the CLI's JSON documents](../reference/cli-json.md#mycelium-tls----format-json--myceliumtls1);
  `each_tls_subcommand_as_json_matches_its_golden_and_carries_no_secret` in [`tests/cli_json.rs`](../../tests/cli_json.rs)).
- `an_unsigned_data_frame_is_refused_whatever_the_setting_says`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for mTLS on
its own, and many of its examples run on a TLS mesh. The end-to-end run is a test in
[`src/lib_tests.rs`](../../src/lib_tests.rs): `a_revoked_keys_certificate_is_refused_at_a_peers_handshake`.

## What it guarantees, and what it does not

- **Guarantees:** [`mesh.tls`](../reference/guarantee-catalogue.md#mesh.tls) — every peer presents a certificate the
  fleet CA signed; [`mesh.signed_writes`](../reference/guarantee-catalogue.md#mesh.signed_writes) — an unsigned or
  bare-signed KV write frame is refused.
- **Not built:** anti-entropy entries are not authenticated (a wire change, v13); another member's identity stays
  writable under the writer's own signature unless identity proofs are required; a holder of the fleet CA key can
  mint any identity ([what-is-proven § the compromised CA key](../operations/what-is-proven.md#recovery-from-compromised-fleet-ca-key)).
- **Evidence:** the ledger rows
  [KV writes are signed under their own domain](../operations/what-is-proven.md#kv-writes-signed-under-their-own),
  [a TLS node refuses unsigned KV frames](../operations/what-is-proven.md#tls-node-refuses-unsigned-kv-frames) and
  [a revoked key stops verifying](../operations/what-is-proven.md#key-node-has-revoked-stops-verifying).

## Go deeper

- **Guide:** [09 § Enabling transport security](../guide/09-security.md#enabling-transport-security) and
  [§ Wire-level KV signing](../guide/09-security.md#wire-level-kv-signing-v10).
- **Runbook:** [cert rotation](../operations/cert-rotation.md) (rotate, revoke, remove a member, issue off-node).
- **Design:** [`identity-authentication.md`](../design/identity-authentication.md).
- **Code:** [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs) (certificates, the handshake's verifiers) and
  the `Data` and `SignedData` arms of `handle_connection` in
  [`mycelium-core/src/connection.rs`](../../mycelium-core/src/connection.rs).
