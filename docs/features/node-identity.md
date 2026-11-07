# Node identity, proofs and revocation

## What it is

Under `tls` each node has an identity key, published with its key history and a proof in one sealed KV record, and
peers reject an identity entry they cannot authenticate (on by default). A node moves to a fresh key and revokes the
old one; a revocation counts only when a newer certificate the fleet CA issued for that node signs it, and once it
gossips every verifier of the node's keys — KV writes, consensus, the audit chain, caller envelopes, the handshake —
refuses the revoked key. An operator can remove a member outright, naming every key it held. All of it rests on the
fleet CA's key being off the node.

## When to use it, and when not

- **Use it when** a key may be stolen or a member must leave for good, and the fleet must stop trusting it without a
  coordinator.
- **Not when** the CA key itself may be compromised: a holder of it can issue a certificate that outranks any node's
  key, and recovery is re-issue only ([what-is-proven § the compromised CA key](../operations/what-is-proven.md#recovery-from-compromised-fleet-ca-key)).

## Turn it on

The proofs are on by default on a [mesh with mTLS](mesh-mtls.md); the CA key comes off the node by issuing its
certificate where the CA key is. The two-domain scaffold's
[`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which CI's two-domains job starts, does both.

- [`require_identity_proofs`](../reference/configuration.md#require_identity_proofs): `true` by default; `false`
  re-admits only a pre-2.3.0 node, and is inert without `[tls]`.
- [`tls`](../reference/configuration.md#tls) with `cert_pem` and `key_pem` from `mycelium tls issue`: no `ca-key.pem`
  in the node's directory, and a certificate naming the node by URI, which revocation authority needs.
- With the CA key off the node, a node cannot rotate in place: [`rotate_identity`](../../src/agent/mod.rs) re-signs
  with the CA key, which it then does not hold. The compromise flow is re-issue — `mycelium tls issue --force` a new
  certificate where the CA key is, restart the node with it — then
  [`revoke_identity_key`](../../src/agent/mod.rs) on the old key (or `POST /gateway/identity/revoke`), signed by the
  new current key. `rotate_identity` is for the development posture, where the CA key is on the node
  ([cert rotation § Compromise remediation](../operations/cert-rotation.md#compromise-remediation--rotate-and-revoke-soc-2-ws-b)).
- [`with_membership_authorities`](../../src/agent/membership_ops.rs) and
  [`offer_member_removal`](../../src/agent/membership_ops.rs): an operator's signed
  [`MemberRemoval`](../../src/membership.rs), refused at the handshake once seen.
- `a_pre_issued_node_cert_starts_without_the_ca_key_and_mints_nothing`, in
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`identity_one_record`](../../examples/README.md#the-capability-matrix): two nodes that require proofs authenticate
each other's signed claims from the one sealed record. CI runs it. It does not run the race the one record prevents,
which needs two processes and lives in the Docker suites.

## What it guarantees, and what it does not

- **Guarantees:** [`id.proofs_required`](../reference/guarantee-catalogue.md#id.proofs_required),
  [`id.ca_key_off_node`](../reference/guarantee-catalogue.md#id.ca_key_off_node) and
  [`id.revocation_authority`](../reference/guarantee-catalogue.md#id.revocation_authority).
- **Not built:** rotation in place under the off-node posture (recovery is re-issue); the CA key cannot be rotated,
  and its compromise is recovered by re-issue only; a node whose
  certificate predates row Q has no standing to revoke until it is re-issued; a removed member's gateway tokens are a
  separate revocation ([cert rotation § Removing a member](../operations/cert-rotation.md#removing-a-member-an-operators-authority-c5)).
- **Evidence:** the ledger rows
  [identity proofs required by default](../operations/what-is-proven.md#identity-proofs-required-default-so-member),
  [a revoked key stops verifying](../operations/what-is-proven.md#key-node-has-revoked-stops-verifying) and
  [a TLS node starts without the fleet CA](../operations/what-is-proven.md#tls-node-starts-without-fleet-ca).

## Go deeper

- **Guide:** [09 § Node identity](../guide/09-security.md#node-identity) and
  [§ Identity proofs](../guide/09-security.md#identity-proofs-on-by-default-and-what-turning-them-off-means).
- **Runbook:** [cert rotation](../operations/cert-rotation.md).
- **Design:** [`identity-authentication.md`](../design/identity-authentication.md),
  [`revocation-authority.md`](../design/revocation-authority.md) and [`member-removal.md`](../design/member-removal.md).
- **Code:** [`src/agent/revocation.rs`](../../src/agent/revocation.rs), [`src/membership.rs`](../../src/membership.rs),
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs).
