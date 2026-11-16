# Developer consent certificates

## What it is

The fleet CA can certify a **developer's consent key**: a P-256 key that lives on the developer's device and decides
grants. The certificate names `mycelium-consent://{issuer}/{name}` for the principal `consent:{issuer}/{name}`, which
sits beside the service's [gateway principal](../guide/00-concepts.md#gateway-caller) `token:{issuer}/{name}` and never
replaces it. The certificate also carries a **grade**: what the enrolment said about where the key lives.
Identity-and-lifecycle I7 builds it, and this page covers I7's first PR, the certificate and its principal.

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

## See it work

No runnable example exists. E6 (`examples/orca_worktree/`) will walk consent after I7's later PRs.
`a_service_key_and_a_consent_key_under_one_name_never_supersede_each_other`, in
[`src/agent/client_cert_tests.rs`](../../src/agent/client_cert_tests.rs), issues one person a service certificate and a
consent certificate, in each order, and re-issues the service key. It shows that neither supersedes the other and that
only the service key is fed as a possession key. `a_consent_certificate_is_issued_over_a_p256_spki_alone_and_reads_back_its_principal_and_grade`,
in [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs), issues over a public key alone and verifies the chain.

## What it guarantees, and what it does not

- **The grade is the operator's statement, not a verified property.** The fleet CA signs what the operator who ran
  `tls issue --grade` was told at enrolment. No verifier can check it, because a verifier holding the public key cannot
  tell a Secure Enclave key from a software key (the spike
  [S-presence](../plans/spikes/2026-11-07-s-presence.md)). The device that created the key promises how it guards the
  key. Neither the CA nor any node can promise that.
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
- **Never a possession key, a `cs` signer or a mesh identity.** A consent key is not fed to an execution authority's
  external issuers. `client_key_standing`, which the `cs` check and the doors read, answers `Unknown` for a consent
  principal. The mesh refuses the certificate for its key ("a mesh certificate must carry an Ed25519 identity key"),
  and refuses the consent scheme on an Ed25519 certificate too.
- **Not built in this PR:** verifying a consent decision (`grant_decision::verify` over P-256), the grade on the
  decision record (`signed_with`), `gateway_consent_principals`, the grade at the evaluator, `dev`'s key, the macOS
  shim, the prompt and its queue. All are listed in
  [identity and lifecycle § 8](../plans/identity-and-lifecycle.md#8--evidence), row I7.

## Go deeper

- **Plan:** [identity and lifecycle § I7](../plans/identity-and-lifecycle.md).
  No design record exists; the plan row is the record.
- **Related:** [client certificates for services](client-certificates.md), [grants](grants.md),
  [node identity, proofs and revocation](node-identity.md).
- **Code:** `consent_identity`, `issue_consent_cert`, `ConsentGrade` and `CONSENT_GRADE_OID` in
  [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs); the fold in
  [`src/agent/client_cert.rs`](../../src/agent/client_cert.rs).
