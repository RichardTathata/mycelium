# Developer consent certificates

## What it is

The fleet CA can certify a **developer's consent key**: a P-256 key that lives on the developer's device and decides
grants. The certificate names `mycelium-consent://{issuer}/{name}` for the principal `consent:{issuer}/{name}`, which
sits beside the service's [gateway principal](../guide/00-concepts.md#gateway-caller) `token:{issuer}/{name}` and never
replaces it. The certificate also carries a **grade**: what the enrolment said about where the key lives.
A consent principal **decides grants**: it signs a grant decision, any bearer holding `grant:relay` carries it, and
the node it is addressed to verifies it and records which key signed and the grade its certificate carries
(`signed_with`). Identity-and-lifecycle I7 builds it. This page covers the certificate and its principal, the
decision, and the developer's side for a **software** key: `mycelium dev consent`, which creates the key, shows the
request and signs what is typed. The macOS Secure Enclave key and its Swift shim are I7's last PR.

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
  - the same binary creates no consent key, enrols no certificate and signs nothing: each `mycelium dev consent`
    verb refuses by name (`consent_grade_oid_provisional`) before it resolves a directory;
  - the release workflow refuses a tag (`scripts/check-release-blockers.sh --release`, `RELEASING.md` § 2), for the
    provisional OID and, separately, while a consent decision does not name what its approver was shown.

## Turn it on

- On the developer's side, export the consent key's public half as SPKI PEM (`-----BEGIN PUBLIC KEY-----`). Raw X9.63
  is refused, so the input names its own algorithm ([`p256_point_from_spki_pem`](../../mycelium-core/src/tls.rs)).
  `mycelium dev consent key --issuer <issuer> --name <name>` creates a software key and writes that file
  ([`dev_cli`](../../src/main.rs), [`SoftwareConsentKey`](../../mycelium-core/src/consent_key.rs)):
  - the key is PKCS#8 in a `0600` file in a `0700` directory under the user's home:
    `~/Library/Application Support/mycelium/consent/` on macOS, `$XDG_DATA_HOME/mycelium/consent/` (default
    `~/.local/share/mycelium/consent/`) on Linux, as `consent+{issuer}+{name}.key.p8`. It is never the directory
    `dev` runs in;
  - the directory's **real path**, with every symbolic link resolved, must have no `.git` at or above it
    (`consent_dir_in_worktree`): a repository's worktree, a linked worktree, a home with a `.git` in it, and a link
    anywhere above the directory that leads into one. **Not detected:** a home tracked by a repository whose `.git`
    is elsewhere (`git --git-dir=… --work-tree=$HOME`, a bare dotfiles repository);
  - it prints where the public key is, a fingerprint of it, and **the grade that describes this key: `tty`**. Ask
    the CA's holder for `--grade tty` and nothing else: a software key is a file any process running as you can
    read. The private half is never printed;
  - a key already there is kept (`consent_key_exists`) unless `--force`, since a new key voids the old certificate.
    A replacement is written beside the old key and renamed over it;
  - Windows is not built: no user-only ACL is set there, so `dev` keeps no key (`consent_key_platform_not_built`).
    No CI job compiles that refusal.
- On the host that holds the fleet CA's key: `mycelium tls issue --ca-dir <dir> --client <name> --consent --public-key
  <spki.pem> --grade <grade> --issuer <issuer> --out <dir> [--force]`
  ([`tls_issue_consent`](../../src/main.rs)). The grade is what the enrolment told the operator about where the key
  lives, and the operator vouches for it: one of `os_presence(macos, unbound)`, `tty` or `recording`; `service` is
  refused. The flags go with `--client` only: `--consent` with `--node` is refused before anything is written. It writes `consent-{issuer}-{name}.cert.pem` and no key file. Issuing over an existing
  certificate needs `--force`. The issuer is the fleet's
  [`gateway_identity_issuer`](../reference/configuration.md#gateway_identity_issuer), as for a client certificate.
- Back on the developer's device: `mycelium dev consent enrol --issuer <issuer> --name <name> --cert <cert.pem>
  [--gateway <url>] [--token <bearer>]`. `dev` refuses a certificate that names another principal
  (`certificate_for_another_principal`), another key (`enrolled_key_mismatch`), **any grade but `tty`**
  (`grade_not_this_keys`), or whose validity has ended (`certificate_expired`). It keeps a good one beside the key:
  the certificate alone, re-encoded, so nothing else in the file given is kept or sent on. With `--gateway` it
  publishes the certificate through the door below, under a bearer holding `identity:write`.
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
  signed_with = "tty"
  ```

  `signed_with` is required: the one consent grade this gateway accepts for the principal, the grade its certificate
  was issued at. **Re-certifying the key at another grade is two deliberate acts, in this order**: first update this
  entry at every gateway that admits the principal, then re-certify at the CA (`tls issue --grade`) and publish.
  Between the two the gateways refuse the principal's decisions (`decision_grade_not_accepted`), which is the
  fail-closed side. Done the other way round, a gateway whose entry still names the old grade has a window: one of
  its nodes that never read the new certificate, and finds only the older one at the path, still admits a decision
  at the old grade until the entry is updated.

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
  ([`grant_decision_message_graded`](../../src/agent/grant_decision.rs)).
- **To decide, at a terminal:** `mycelium dev consent approve --issuer <issuer> --name <name> [--gateway <url>]
  [--token <bearer>] [--ttl-secs <n>] <request>… | -` ([`answer_one`](../../src/dev_consent.rs)).
  - It takes request **ids** and nothing else: on the command line, or one per line on a piped standard input with
    `-`. `-` is refused when standard input is the terminal (`requests_from_a_terminal`): the ids and the answers
    would come from one keyboard. A line longer than 256 bytes is no id (`request_id_too_long`).
  - For each id it reads the key and its certificate again, then the request's record from the gateway (`GET
    /gateway/grants/pending`, a bearer holding `grant:read`).
  - **It checks the record against itself before it shows anything**
    ([`PromptRecord::find`](../../src/dev_consent.rs)). The arguments must hash to the digest the record names,
    which is what a grant binds: the RFC 8785 digest, or the older form for as long as the nodes still honour it
    ([`deprecations.md` §93](../guide/deprecations.md)). The resource must be the record's tool at its node. A
    record that fails either prompts nothing (`arguments_unverified`, `record_unreadable`). So does a request whose
    arguments the record withholds (a data class the policy does not render) or cuts (over the 16 KiB rendering
    cap): `dev` refuses it and does not show it with a caveat, so such a request cannot be approved with `dev`.
  - It shows the record at the terminal it opens by name (`/dev/tty`). The arguments come first, cut to 400
    characters with what was cut said. The request, holder, tool, resource, digest and terms come **last**,
    directly above the question, each identifier cut to 96 characters, so they are on the screen when the answer
    is typed whatever the record holds. Every character that is not printable ASCII, a letter or a digit is written
    as an escape; an identifier shows only printable ASCII.
  - Type `approve` to sign an issue (for `--ttl-secs`, default 300) or `refuse` to sign a refusal. Anything else,
    an empty line or no terminal signs nothing and sends nothing. **Whatever was typed before the prompt appeared
    is discarded**, entered or not, and the prompt says how many entered characters went.
  - It reads the record again, signs only if it still says what was shown, and sends the signed decision to the
    gateway under the same bearer (`grant:relay`). The bearer signs nothing.
  - At most 8 requests wait; one more is answered `busy` at once and never shown. Ids on the command line are all
    queued before the first prompt, so of more than 8 the rest are `busy`. `busy` is printed by `dev` only: the
    requesting agent learns nothing until its request lapses.
  - Run one `dev` per terminal. Two sharing a terminal each show a prompt, and a typed answer goes to whichever
    reads first.
  - It prints one JSON line per request (`mycelium.dev.consent/1`): `signed` with the gateway's own status and
    answer quoted, or `dismissed`, `busy`, `unknown_request`, `ambiguous_request`, `record_unreadable`,
    `arguments_unverified`, `record_changed`, `enrolment_refused`, `request_id_too_long`, `no_terminal`,
    `not_signed`, `gateway`. Exit 0 when every request was signed and admitted, 1 when one was not, 2 on a refusal
    by name.

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
`a_decision_typed_at_devs_prompt_is_signed_by_its_software_key_and_recorded_tty`, in
[`src/agent/dev_consent_tests.rs`](../../src/agent/dev_consent_tests.rs), has `dev`'s own code create the key, enrol,
read the record, and sign an approval, a dismissal and a refusal against a node, with a scripted approver in place of
a person. `an_answer_typed_at_the_terminal_is_what_dev_signs_and_the_key_stays_off_the_worktree`, in
[`tests/dev_consent_cli.rs`](../../tests/dev_consent_cli.rs), runs the binary at a pseudo-terminal against a stub
gateway.

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
  `tty` decision may therefore have been made by the agent whose request it approves (I7's B). The tests of
  `mycelium dev consent` are the demonstration: a scripted approver, and a helper typing at a pseudo-terminal, each
  produce decisions a node records `tty`. No person was involved.
- **What `dev` promises is its own behaviour, on the developer's device.**
  - *How the key is guarded, and exactly what is checked.* Before a byte of the key is read
    ([`SoftwareConsentKey::load`](../../mycelium-core/src/consent_key.rs)):
    - the file is opened without following a link, and the open file must be a regular file, **owned by this
      user**, closed to group and others by its mode bits, with **one name** (`consent_key_too_open`,
      `consent_key_not_owned`, `consent_key_hard_linked`);
    - its directory must be a directory, not a link, owned by this user and closed to group and others
      (`consent_dir_too_open`);
    - no directory above it, **up to the home** (or `XDG_DATA_HOME` where that names the place), may be writable
      by group or others or belong to another user (`consent_dir_ancestor_too_open`). Nothing above the home is
      examined;
    - its real path is in no worktree, as above.
  - *What is not checked.* **An access-control list is not read.** On macOS an ACL can let another user read a
    file whose mode says `0600`, and a directory's inheritable ACL puts one on each key created in it. `dev` loads
    such a key. Reading an ACL needs a call into the platform's C library that this crate's no-`unsafe` rule keeps
    out. So on macOS `tty` claims the owner, the mode bits, the link count and the path, and not the ACL. Check
    with `ls -le`: a `+` after the mode means an ACL is present.
  - *It claims no more than it does.* `dev` signs with a software key only under a certificate graded `tty`. If the
    operator certified that key as `os_presence(macos, unbound)` or `recording`, `dev` refuses to enrol and refuses
    to sign (`grade_not_this_keys`): the certificate would say something about the key that the device does not do.
    This is `dev`'s refusal about its own key. It is not a check any node can make, and another signing program
    holding the same file is not bound by it.
  - *The key on use:* before **every prompt** `dev` reads the key and the certificate again and repeats every
    check above ([`serve`](../../src/dev_consent.rs)). A key widened, replaced or re-certified while `dev` runs is
    refused for the requests that follow (`enrolment_refused`).
  - *What it showed:* the prompt is rendered from the record a gateway served under the id, never from text a caller
    passed, and only from a record whose arguments hash to its digest. A request with no record, or an id two
    holders share, prompts nothing.
  - *What it signed:* only `approve` or `refuse`, typed whole at the terminal after the prompt was shown. It has no
    flag that approves without a prompt, standard input is never read for an answer, and whatever was typed ahead
    of a prompt is discarded.
- **What `dev` cannot promise.**
  - *That a person typed.* A process with a pseudo-terminal types as a person does, and so does any process that
    can write to the developer's terminal: it types after the prompt appears and is signed. Discarding typed-ahead
    input protects a person from their own early keystrokes. It stops no such process.
  - *That the gateway told the truth.* The record is what the gateway at `--gateway` answered, over the transport
    its URL names. The default is plain `http://` to this machine. Nothing signs the record, so `dev` checks it
    against itself and not against the requester.
  - *What the grant binds.* The signed decision names the request's id, holder and terms. It does **not** name the
    tool, the resource or the digest. The addressed node issues the grant from its own copy of the request record,
    which is unsigned in the KV ([owner-signed records](../wiki/dev/owner-signed-records.md), row
    `grant/{agent}/{request}`). So today, after a person typed `approve` and with `dev`'s second read in place:
    - any admitted member can change the **tool**, the **arguments digest**, the **resource** and the **expiry**
      that the approval is applied to, by rewriting the record before the node reads it;
    - leaving the resource alone while changing the tool carries a tool past a rule written for the resource: the
      evaluator is asked about the resource, and the grant permits the tool;
    - a member who tombstones the answer (`…/issued`) can have **the same signed decision admitted again**, for
      another rewritten record, for as long as the decision is fresh: 300 s plus clock drift;
    - what cannot be changed: the holder, the request id, the addressed node, the decision, the ttl, the grade and
      the decider.
    `dev`'s second read catches a change it can see before it signs, and nothing after. **This is closed by the
    approver signing what it saw**, which the maintainer decided on 2026-11-18 as its own PR after this one
    ([identity and lifecycle, row I7-S](../plans/identity-and-lifecycle.md)). Until it is built no release carries
    consent decisions: `scripts/check-release-blockers.sh --release` refuses the tag
    (`CONSENT_DECISION_DOES_NOT_NAME_WHAT_WAS_SHOWN`).
  - *That the decision was admitted.* `dev` reports that it signed and sent, and quotes the gateway's answer.
  - *That a word is the word it looks like.* In the arguments, a letter of another script that resembles a Latin
    one is shown as itself.
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
- **Three statements must agree, and one of them is not in the KV.** A consent decision is admitted only when the
  grade the signed body names, the grade of the certificate the node read, and the grade the gateway's own
  `gateway_consent_principals` entry accepts are the same. The first two disagreeing is `decision_grade_mismatch`;
  the entry disagreeing is `decision_grade_not_accepted`, counted on `GET /stats` (`consent_grades_not_accepted`).
  The certificate is a KV record that any member can replace with an older, genuine one; the entry is the gateway
  operator's configuration, which no member can write. So a node never relies on which certificate reached it
  first: a gateway that joins late, or restarts with an empty persistence directory, and finds only an older
  certificate at a different grade, admits nothing under it.
- **What that does not cover.** It binds the grade, not the key. A node that never read a newer certificate for a
  *different, superseded* key at the accepted grade takes that key as current: the supersession residual of
  [client certificates](client-certificates.md). That is a node restarted without `[persistence]`, and equally a
  node that joins late, with `[persistence]`, while the newer record is absent from its view.
  - The holder of a superseded consent key can then approve, refuse and revoke grants as that principal, within
    policy.
  - It needs the old private key, the newer record absent from that node's view, and a gateway that joined after,
    or lost its persistence directory.
  - It ends when the newer certificate is read there.
  - Meanwhile an operator can stop it by removing or renaming the principal's `gateway_consent_principals` entry at
    that gateway.
- **The grade cannot be altered in transit or by a record.** It is inside the signature, so a relay that rewrites it
  breaks the decision. A body naming a grade other than its certificate's is refused (`decision_grade_mismatch`), and
  one naming none is refused (`decision_grade_missing`). At a node that has read the newer certificate, the grade is
  read from the certificate the principal's mark names: an older certificate for the same key, written back at its
  path by any member, gives the key no grade — the decision is refused until the newest certificate is on record again, and the node counts it
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
- **Not built yet:** the approver signing what it saw (row I7-S, a release blocker); the macOS Secure Enclave key
  and its Swift shim (I7's last PR), so nothing in this tree signs at `os_presence(macos, unbound)` and no person's
  Touch ID run has produced a decision; a software key on Windows; reading a macOS ACL; a `recording` signer for a
  `sim` build; a lock between two `dev` processes on one terminal; how another process hands a running `dev` a
  request id other than its standard input (N4's and N7's); `dev` verifying its certificate's chain (the nodes
  judge it); attestation of a key's access control (none is open to an unmanaged binary);
  an environment form of `gateway_consent_principals`; `mycelium test --faults grant` under a rule naming
  `signed_with` (it drives an unsigned decision). All are listed in
  [identity and lifecycle § 8](../plans/identity-and-lifecycle.md#8--evidence), row I7.

## Go deeper

- **Plan:** [identity and lifecycle § I7](../plans/identity-and-lifecycle.md).
  No design record exists; the plan row is the record.
- **Related:** [client certificates for services](client-certificates.md), [grants](grants.md),
  [node identity, proofs and revocation](node-identity.md).
- **Code:** `consent_identity`, `issue_consent_cert`, `ConsentGrade`, `CONSENT_GRADE_OID` and `verify_consent_p256`
  in [`mycelium-core/src/tls.rs`](../../mycelium-core/src/tls.rs); `SoftwareConsentKey` in
  [`mycelium-core/src/consent_key.rs`](../../mycelium-core/src/consent_key.rs); `dev`'s store, prompt, queue and
  signer in [`src/dev_consent.rs`](../../src/dev_consent.rs); the fold and `consent_key_grade` in
  [`src/agent/client_cert.rs`](../../src/agent/client_cert.rs); `verify` and `SignedWith` in
  [`src/agent/grant_decision.rs`](../../src/agent/grant_decision.rs).
