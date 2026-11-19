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
  [`revoke_identity_key`](../../src/agent/mod.rs) on the old key (or `POST /gateway/identity/revoke`, in a build with `compliance`), signed by the
  new current key. `rotate_identity` is for the development posture, where the CA key is on the node
  ([cert rotation § Compromise remediation](../operations/cert-rotation.md#compromise-remediation--rotate-and-revoke-soc-2-ws-b)).
- [`with_membership_authorities`](../../src/agent/membership_ops.rs) and
  [`offer_member_removal`](../../src/agent/membership_ops.rs): an operator's signed
  [`MemberRemoval`](../../src/membership.rs), refused at the handshake once seen.
- `a_pre_issued_node_cert_starts_without_the_ca_key_and_mints_nothing`, in
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs).
- `ca_cert_pem` names **one** certificate, the fleet CA's: a file of two is refused at start by name (unreleased,
  [`deprecations.md` §111](../guide/deprecations.md)). `mycelium tls ca` gives the CA a subject naming the fleet and
  its generation, and every issuer writes that subject as a certificate's issuer, so a certificate from another
  fleet is refused as `UnknownIssuer`
  ([cert rotation § Succeeding the fleet CA](../operations/cert-rotation.md#succeeding-the-fleet-ca-identity-and-lifecycle-i2--what-this-release-has-and-what-it-does-not-yet)).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`identity_one_record`](../../examples/README.md#the-capability-matrix): two nodes that require proofs authenticate
each other's signed claims from the one sealed record. CI runs it. It does not run the race the one record prevents,
which needs two processes and lives in the Docker suites.

## What it guarantees, and what it does not

- **Guarantees:** [`id.proofs_required`](../reference/guarantee-catalogue.md#id.proofs_required),
  [`id.ca_key_off_node`](../reference/guarantee-catalogue.md#id.ca_key_off_node) and
  [`id.revocation_authority`](../reference/guarantee-catalogue.md#id.revocation_authority).
- **Under a change of roots** (plan row I2, PRs 1 and 2a): a node changes only its own trust (under one owner with
  `rotate_identity`), and everything it judged under the old roots is judged again — and a certificate's expiry is
  judged while the node runs, as at a restart. An anchored key keeps its certificates — one per issuer, journaled under `[persistence]` — and
  leaves only when they fail the roots the node trusts now, never because its record left the store; a node left with
  no anchor trusts nothing, never reverting to legacy — in-process and under `[persistence]`; without `[persistence]` a
  restart forgets the dropped anchor, and with legacy open that peer is legacy again. A node whose every root is generation 1 or later (a CA's
  `caGeneration`, under a **provisional** OID a shipped build refuses by name) has no legacy class. Live mesh sessions
  are re-judged before their next frame and closed when the new roots refuse the peer (`sessions_closed_by_roots`).
- **A CA commits to its successor, and a node pins that** (plan row I2, PR 2b-1; `deprecations.md` §120): on the
  issuer's host `mycelium tls commit --ca-dir <dir> --successor <dir>` mints the successor CA and writes
  `ca-commit.pem`, the old CA's signature over its successor — once. An operator copies that file into a node's
  certificate directory; the node assesses it against the root it trusts, pins it under its own `succession/`
  (durably, before it goes on) and from then on refuses a second commitment from that CA, counting each
  (`ca_commitment_conflict`); a document it cannot assess is counted apart (`ca_commitment_unverified`). A pin is
  per committing CA: once the node trusts the successor it pins one commitment from the successor the same way,
  beside the first. It pins from that file only: nodes republish what they pinned at `sys/ca-commit/{node}`
  so that a conflicting commitment — a mistake, or a stolen CA key — is seen and counted, and a copy there is never
  pinned. A node holding a commitment refuses to start without `[persistence]`. The commitment changes nothing a node
  trusts, and no shipped build makes or reads one while the `caGeneration` OID is provisional.
- **The kept anchors' journal is compacted** (the same PR): its budget — 32 certificates per peer — counts the
  anchors a node keeps now, so anchors it has dropped stop counting; the last record of a peer it keeps nothing of
  stays, as its memory that the peer was anchored; a crash before the compaction's rename leaves the journal as it
  was, and one after it leaves the compacted journal whole.
- **Not built:** rotation in place under the off-node posture (recovery is re-issue); the CA key cannot be rotated,
  and its compromise is recovered by re-issue only — CA succession (plan row I2) has its reload path, its standing
  rules, the commitment and the pin in the library and nothing yet that changes a node's roots: the successor-signed
  record, its phases under `succession/` (PRs 2b-2 to 2b-4) and `tls succeed` (PR 3) are I2's next PRs; a node whose
  certificate predates row Q has no standing to revoke until it is re-issued; a removed member's gateway tokens are a
  separate revocation ([cert rotation § Removing a member](../operations/cert-rotation.md#removing-a-member-an-operators-authority-c5)).
- **Evidence:** the ledger rows
  [identity proofs required by default](../operations/what-is-proven.md#identity-proofs-required-default-so-member),
  [a revoked key stops verifying](../operations/what-is-proven.md#key-node-has-revoked-stops-verifying) and
  [a TLS node starts without the fleet CA](../operations/what-is-proven.md#tls-node-starts-without-fleet-ca),
  [a node takes a new root set live](../operations/what-is-proven.md#node-takes-new-roots-live-and-caches-rejudge),
  [a CA's commitment pinned from the operator's file only](../operations/what-is-proven.md#ca-commitment-pinned-from-operators-file-only),
  [the kept-anchor journal compacted](../operations/what-is-proven.md#anchor-journal-compacted-crash-safely) and
  [one root in `ca_cert_pem`](../operations/what-is-proven.md#ca-cert-pem-one-root-and-named-fleet-ca).

## Go deeper

- **Guide:** [09 § Node identity](../guide/09-security.md#node-identity) and
  [§ Identity proofs](../guide/09-security.md#identity-proofs-on-by-default-and-what-turning-them-off-means).
- **Runbook:** [cert rotation](../operations/cert-rotation.md).
- **Design:** [`identity-authentication.md`](../design/identity-authentication.md),
  [`revocation-authority.md`](../design/revocation-authority.md) and [`member-removal.md`](../design/member-removal.md).
- **Code:** [`src/agent/revocation.rs`](../../src/agent/revocation.rs), [`src/membership.rs`](../../src/membership.rs),
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs).
