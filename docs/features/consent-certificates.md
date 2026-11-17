# Developer consent certificates

## What it is

The fleet CA can certify a **developer's consent key**: a P-256 key that lives on the developer's device and decides
grants. The certificate names `mycelium-consent://{issuer}/{name}` for the principal `consent:{issuer}/{name}`, which
sits beside the service's [gateway principal](../guide/00-concepts.md#gateway-caller) `token:{issuer}/{name}` and never
replaces it. The certificate also carries a **grade**: what the enrolment said about where the key lives.
A consent principal **decides grants**: it signs a grant decision, any bearer holding `grant:relay` carries it, and
the node it is addressed to verifies it and records which key signed and the grade its certificate carries
(`signed_with`). Identity-and-lifecycle I7 builds it. This page covers its first two PRs: the certificate and its
principal, and the decision. The developer's own tooling (`dev`, the macOS shim, the prompt) is the third.

## When to use it, and when not

- **Use it when** a person, not a service, is to approve grant requests with a key the fleet CA vouches for, and the
  operator wants that authority kept apart from the person's service key.
- **Not when** the key is to sign calls, hold grants or join the mesh: a consent key does none of these (I7's
  [*Not built*](../plans/identity-and-lifecycle.md)).
- **Not yet in a release.** The grade extension's OID is provisional until the registered IANA Private Enterprise
  Number arc arrives ([`CONSENT_GRADE_OID_IS_PROVISIONAL`](../../mycelium-core/src/tls.rs)). Until then:
  - a binary built without the test-only feature `provisional-consent-grade` (every shipped binary, example and Docker
    image) neither issues nor reads a consent certificate. Its fold ignores one and counts it, and its door refuses one
    (`consent_grade_oid_provisional`);
  - the same binary verifies no consent signature: it refuses a consent principal's decision by name
    (`422 consent_grade_oid_provisional`) before it reads a key, and it refuses to start with a
    `gateway_consent_principals` table rather than accept one it would never honour;
  - the release workflow refuses a tag (`scripts/check-release-blockers.sh --release`, `RELEASING.md` § 2).

## Turn it on

- On the developer's side, export the consent key's public half as SPKI PEM (`-----BEGIN PUBLIC KEY-----`). Raw X9.63
  is refused, so the input names its own algorithm ([`p256_point_from_spki_pem`](../../mycelium-core/src/tls.rs)).
  `dev` creating the key is a later PR of I7.
- On the host that holds the fleet CA's key: `mycelium tls issue --ca-dir <dir> --client <name> --consent --public-key
  <spki.pem> --grade <grade> --issuer <issuer> --out <dir> [--force]`
  ([`tls_issue_consent`](../../src/main.rs)). The grade is what the enrolment told the operator about where the key
  lives, and the operator vouches for it: one of `os_presence(macos, unbound)`, `tty` or `recording`; `service` is
  refused. The flags go with `--client` only: `--consent` with `--node` is refused before anything is written. It writes `consent-{issuer}-{name}.cert.pem` and no key file. Issuing over an existing
  certificate needs `--force`. The issuer is the fleet's
  [`gateway_identity_issuer`](../reference/configuration.md#gateway_identity_issuer), as for a client certificate.
- Upgrade every node first: an older node never reads a consent certificate, and its door refuses one
  ([`deprecations.md` §112](../guide/deprecations.md)).
- [`tls`](../reference/configuration.md#tls): a node judges a consent certificate against its CA anchor.
- [`persistence`](../reference/configuration.md#persistence): keeps a re-enrolment's supersession across a restart.
- Publish the certificate with `POST /gateway/identity/client-cert`, the door client certificates use, under
  `identity:write` in a build with `compliance` ([`gw_identity_client_cert`](../../src/agent/http/compliance.rs)). It
  answers the consent principal, the key as the point's 130 hex digits, and the grade beside `grade_source:
  "operator"`: the certificate's claim, not something the node checked.
- `the_consent_issue_flow_refuses_by_name_and_needs_force_over_an_existing_certificate`, in
  [`src/main.rs`](../../src/main.rs), runs the CLI flow.
- **To let the principal decide**, list it in
  [`gateway_consent_principals`](../reference/configuration.md#gateway_consent_principals) on the gateway its
  decisions are addressed to:

  ```toml
  [[gateway_consent_principals]]
  name = "richard"
  scopes = ["grant:issue"]
  ```

  A consent principal's authority comes from this table and never from a named token's, so granting it gives nothing to the same
  person's `token:` principal, and removing the entry revokes the consent principal alone
  ([`decider_holds_grant_issue`](../../src/agent/http/grants.rs)). `grant:issue` is the only scope the table takes.
- **Give the policy a rule for the consent principal.** A rule for `token:{issuer}/{name}` does not admit
  `consent:{issuer}/{name}`: they are different actors. A rule may also name the grades it accepts, as a set:
  `signed_with = ["os_presence(macos, unbound)"]` in a unit file's `[[rule]]`
  ([`RuleDecl`](../../src/capability_config.rs)), or `Rule::accepting_grades` in code.
- **The decision itself** is the body the [grant](grants.md) decision routes already take, with `signed.k` the
  base64 of the key's 65-byte uncompressed point, `signed.sig` an ASN.1 DER ECDSA signature over SHA-256, and
  `signed.grade` the certificate's grade, inside the signature
  ([`grant_decision_message_graded`](../../src/agent/grant_decision.rs)). Nothing in this tree signs one outside a
  test yet: `dev`'s signer is I7's third PR.

## See it work

No runnable example exists. E6 (`examples/orca_worktree/`) will walk consent after I7's later PRs.
`a_service_key_and_a_consent_key_under_one_name_never_supersede_each_other`, in
[`src/agent/client_cert_tests.rs`](../../src/agent/client_cert_tests.rs), issues one person a service certificate and a
consent certificate, in each order, and re-issues the service key. It shows that neither supersedes the other and that
only the service key is fed as a possession key. `a_consent_certificate_is_issued_over_a_p256_spki_alone_and_reads_back_its_principal_and_grade`,
in [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs), issues over a public key alone and verifies the chain.
`a_consent_decision_is_verified_and_recorded_with_the_grade_its_certificate_carries`, in
[`src/agent/consent_decision_tests.rs`](../../src/agent/consent_decision_tests.rs), has a consent principal issue and
revoke a grant through a relay and reads back, from the evidence journal and the revocation record, which key signed
and the grade recorded with it.
`a_policy_accepting_one_grade_refuses_the_same_decision_at_any_other`, in the same file, shows a rule's accepted set.

## What it guarantees, and what it does not

- **The grade is the operator's statement, not a verified property.** The fleet CA signs what the operator who ran
  `tls issue --grade` was told at enrolment. No verifier can check it, because a verifier holding the public key cannot
  tell a Secure Enclave key from a software key (the spike
  [S-presence](../plans/spikes/2026-11-07-s-presence.md)). The device that created the key promises how it guards the
  key. Neither the CA nor any node can promise that.
- **What a node promises about a decision is what it checked.** The node a decision is addressed to checks the
  signature, the consent certificate's chain and standing, and that the grade the signed body names is the
  certificate's. It then records `signed_with` in its evidence journal: the key's kind (`p256`), the grade,
  `grade_source: "operator"`, and which certificate the grade was read from (its issuance and digest).
  That last field is the limit of the promise: the grade is the issuing operator's statement, carried unaltered, and
  not something the node, the relay or the evaluator observed. A policy that accepts only
  `os_presence(macos, unbound)` accepts that statement. A service's decision is recorded `ed25519`, `service`,
  `grade_source: "certificate_scheme"`: the node read that from the kind of certificate the key is in.
- **`os_presence(macos, unbound)`:** the Enclave asks for the user's finger, but the signing process chooses the
  prompt's text, so presence shows that a person was there, not what they were shown (I7's P5).
- **`tty`:** any process running as the user can sign with the key file, the developer's coding agent included. A
  `tty` decision may therefore have been made by the agent whose request it approves (I7's B).
- **No grade, no consent.** A consent certificate with no grade, a grade outside the set, or the grade twice never
  counts.
- **A key on the curve.** The point must lie on P-256, at issue and at read, so a corrupt public key file can never
  become a person's newest consent certificate and leave their real key superseded.
- **The scheme is bound to the key kind.** The consent scheme over an Ed25519 key, the client scheme over a P-256 key,
  and a certificate naming both count as neither.
- **Its own principal.** A consent certificate supersedes only an older consent key of the same `consent:` principal,
  and never a `token:` key. A `token:` certificate never supersedes a consent key. Under `[persistence]` the
  supersession survives a restart, through the same journal and budget as
  [client certificates](client-certificates.md).
- **Read the grade from the evidence journal, not from a KV record.** The journal is node-local and append-only. A
  refusal's and a revocation's record under `grant/` carry a copy of `signed_with`, and those records are unsigned:
  any admitted member can rewrite the copy, and the status route serves what is there. Nothing decides on it. An
  issue's record is the signed grant and carries no copy at all. To check a KV record, re-verify its `signed` body and
  read `signed.grade`.
- **The grade cannot be altered in transit or by a record.** It is inside the signature, so a relay that rewrites it
  breaks the decision. A body naming a grade other than its certificate's is refused (`decision_grade_mismatch`), and
  one naming none is refused (`decision_grade_missing`). The certificate's grade is read from the certificate the
  principal's mark names: an older certificate for the same key, written back at its path by any member, changes
  nothing — the decision is refused until the newest certificate is on record again, and the node counts it
  (`consent_grades_unread` on `GET /stats`).
- **Consent decides only where an operator wrote it down.** A consent principal holds `grant:issue` through
  `gateway_consent_principals` alone, in every build. A superseded consent key decides nothing, before a restart or
  after one under `[persistence]`. `recording` is accepted only by a `sim` build.
- **A consent principal never holds a grant.** A decision for a request under a `consent:` holder is refused
  `409 consent_holder`.
- **A service key is never a consent key.** An Ed25519 key under a `consent:` principal, and a P-256 key under a
  `token:` one, are refused. A service names the grade `service` or none.
- **Never a possession key, a `cs` signer or a mesh identity.** A consent key is not fed to an execution authority's
  external issuers. `client_key_standing`, which the `cs` check and the doors read, answers `Unknown` for a consent
  principal. The mesh refuses the certificate for its key ("a mesh certificate must carry an Ed25519 identity key"),
  and refuses the consent scheme on an Ed25519 certificate too.
- **Not built yet:** `dev`'s key and its signer, the macOS shim, the prompt and its queue (I7's third PR), so no
  person's run has produced a decision; attestation of a key's access control (none is open to an unmanaged binary);
  an environment form of `gateway_consent_principals`; `mycelium test --faults grant` under a rule naming
  `signed_with` (it drives an unsigned decision). All are listed in
  [identity and lifecycle § 8](../plans/identity-and-lifecycle.md#8--evidence), row I7.

## Go deeper

- **Plan:** [identity and lifecycle § I7](../plans/identity-and-lifecycle.md).
  No design record exists; the plan row is the record.
- **Related:** [client certificates for services](client-certificates.md), [grants](grants.md),
  [node identity, proofs and revocation](node-identity.md).
- **Code:** `consent_identity`, `issue_consent_cert`, `ConsentGrade`, `CONSENT_GRADE_OID` and `verify_consent_p256`
  in [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs); the fold and `consent_key_grade` in
  [`src/agent/client_cert.rs`](../../src/agent/client_cert.rs); `verify` and `SignedWith` in
  [`src/agent/grant_decision.rs`](../../src/agent/grant_decision.rs).
