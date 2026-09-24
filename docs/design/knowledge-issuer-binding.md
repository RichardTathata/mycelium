# Knowledge issuer binding and portable authority (ADR, Boundary H items P1 + P2)

**Status:** P1 **adopted and implemented** 2026-09-24 (`src/knowledge/issuer.rs`, `src/agent/knowledge_keys.rs`).
P2 **proposed**: decisions recorded here, implementation to follow. Plan:
[`docs/plans/boundary-h.md`](../plans/boundary-h.md) (rev 0.3, proposed) §6 P1 and §7 P2. Threat model:
[`docs/threat-model.md`](../threat-model.md) §5 Boundary H (revision 3 draft).

> Posture, once: **a signature proves who issued a statement.** It does not prove that the issuer was entitled to
> issue it, or that no conflicting statement exists. P1 settles *who*. P2 settles *entitled* and *current*. Neither
> settles *true*.

---

## 1. The problem

An `IssuerId` was an opaque string. `IssuerId::new` accepted any non-empty name, and `KnowledgeRecord::verify`
took the key from its caller. Nothing bound an issuer to an admitted identity, so **one member could be many
issuers**. Every count the resolver makes (supporting issuers, independent control groups, challengers) could be
inflated from inside a single member, before any colluding population is involved. H2 (#381) made support count
issuers rather than records. That only helps once an issuer cannot be minted at will.

## 2. Decision (P1): two admissible verification paths

A reader attributes a record through exactly one of two paths, and never takes the key from the record or from
whoever presents it.

| Path | Issuer form | Key source | Trusted because |
|---|---|---|---|
| **Member** | `node:{node_id}`, from `IssuerId::for_node` | The reader's retained identity keys for that node (`peer_keys`, learned from `sys/identity/`), with validated revocations reported separately | The node is admitted, and the key is its identity key |
| **Configured external** | Any name **outside** `node:` | `TrustedExternalIssuers`, set by the reader | The reader configured it: operators, auditors, outside observers |

Anything else is `Authenticity::Unverifiable`, with one of four reasons:
- `UnknownMember`: a member this reader has no keys for;
- `MalformedMemberIssuer`: a `node:` name that does not parse;
- `UntrustedExternal`: an unconfigured external issuer;
- `BadSignature`: a known issuer whose keys do not verify the signature.

It is never a silent drop and never a pass.

**Rules that fall out:**
- **One issuer per member.** The member path's only key is the member's own identity key, so a member cannot be
  a second member issuer. Inventing an external name gets it nothing unless a reader configured that name.
- **The member namespace is reserved.** `TrustedExternalIssuers::trust` refuses any `node:` name
  (`ExternalIssuerError::MemberNamespace`), so configuration can never stand in for a member's identity.
- **A node signs only as itself.** `GossipAgent::sign_knowledge_record` refuses a record naming any issuer but
  `node:{self}` (`SignAsMemberError::NotThisMember`). A key holder can still sign arbitrary bytes with
  `sign_with_identity`, but such a signature verifies only under its own keys, so it cannot make the signer another
  issuer.

## 3. Decision (P1): authentic is not current

`verify_issuer` returns one of three outcomes:
- **`Current`**: signed under a key the reader currently trusts.
- **`Revoked`**: signed under a key the issuer held and has since **validly revoked** (a `sys/revocation/` entry
  signed by the node's current key, for a key in its own history). The record is attributable as history, and
  carries no present standing.
- **`Unverifiable`**: see §2.

Revoked keys are kept in the retained set on purpose. Dropping them, as `known_verifying_keys` does for role claims
and audit chains, would make a revoked-key record *unattributable*, erasing *who said it, then*. That contradicts
posture rule 5: attribution outlives authority. Whether a `Revoked` record counts toward a present decision is the
resolver's call (plan item K1b). This module only reports the difference.

Rotation is not revocation. A signature under a retained, unrevoked older key is `Current`.

## 4. What P1 does not claim

- **Its strength rests on `require_identity_proofs`, which is off by default.** Without proofs, `sys/identity/` is
  accepted and flagged, so another admitted member can inject a key into a node's retained set (the documented
  identity-poisoning residual). With proofs required, or the sealed identity record (Phase 3b), a key enters the
  retained set only if authenticated. The confined-fleet profile (plan H7) requires proofs.
- **It does not verify records on storage.** `KnowledgeStore::put` still accepts records without signatures. K1
  makes the store call `verify_issuer`; records carry no embedded signature today.
- **It does not decide entitlement, currency of authority, or truth.** Those are P2, K1b and nobody,
  respectively.
- **It does not bind an external issuer to an organisation.** An external issuer is exactly as trustworthy as the
  reader's configuration of it.

## 5. Decision (P2, proposed): signed, portable mandate grants

Recorded now so P1's types do not need to change for it. It is implemented in a later PR.

- **Issuance.** A `MandateGrant` is a threat-model §6 scoped attestation over the mandate's canonical bytes
  (holder, scope, enumerated operations, epoch, term, validity window). It is signed by `established_by` and
  carries no transferable credential.
- **Who can sign.** `established_by` is verified through P1's paths. An operator is a configured external issuer.
  A member authority is a member issuer.
- **Entitlement.** A reader accepts a grant only if the signer is entitled for that scope. Until the consensus
  acceptance gate passes (plan §5), entitlement comes only from a configured `scope → authorities` table. A
  signature never establishes entitlement on its own.
- **Currency.** A reader retains, durably, the highest epoch it has verified per scope.
  - A lower epoch is `Superseded`.
  - Two different grants with the same scope and epoch are `ConflictingAppointments`. Neither backs an exclusive
    operation, and both are reported.
- **Possession.** A grant is presented together with the holder's signature over the digest of the specific
  request or advertisement. A grant presented by anyone else is refused.
- **Tests required before adoption:**
  - an altered operation list;
  - expiry;
  - an authentic grant from a non-entitled authority;
  - a wrong-holder presentation;
  - a superseded grant;
  - conflicting equal-epoch grants.

## 6. Alternatives considered

- **A per-issuer key registry in KV.** Rejected. It is a second identity system beside `sys/identity/`, with its
  own poisoning surface, and composition before primitives says to reuse the one that exists.
- **Binding issuers through the CA certificate.** Deferred. Certificates are not in KV, and readers verify
  gossip-learned keys rather than certificates. It is worth revisiting with the sealed identity record.
- **Rejecting revoked-key records outright.** Rejected (§3): it erases attribution.
- **Letting `IssuerId::new` refuse the `node:` prefix.** Rejected. Deserialisation and `for_node` both construct
  member issuers. The binding is enforced where it matters, at verification, where a `node:` name must verify under
  that node's keys.

## 7. Gates

- `src/knowledge/issuer.rs` unit tests:
  - member round trip;
  - one member cannot sign as another;
  - invented, unknown and malformed issuers are unverifiable;
  - configured versus unconfigured external issuers;
  - the member namespace is refused for external issuers;
  - revoked-key attribution without currency;
  - a rotated retained key is current;
  - tampered content fails.
- `src/lib_tests.rs::test_boundary_h_p1_issuer_binding_on_live_nodes` (`compliance`). Two TLS nodes:
  - B attributes A's record to A;
  - B's API refuses to sign as A;
  - A's issuer signed with B's key fails;
  - after A rotates and revokes, B reports the old record `Revoked`.
