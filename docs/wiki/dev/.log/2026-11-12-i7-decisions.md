## [2026-11-12] ingest | row I7: #764's review taken, the maintainer's decisions recorded (identity-and-lifecycle rev 0.16)

`docs/plans/identity-and-lifecycle.md` row I7 after #764's adversarial review (M1–M6, L1–L9) and the maintainer's
decisions of 2026-11-12:
- **P2 (a):** the developer signs with a fleet-certified P-256 **consent** key, issued over its public point alone with
  the CA off-node (`tls issue --client --public-key`, new), verified with `ring` at `grant_decision::verify` only. It
  widens I1's door, fold, supersession and `client_key_standing`; it never reaches `client_sig`, the possession proof or
  the provider's checks.
- **P3:** the CryptoKit blob path through a Swift shim; no Apple team, no signed bundle.
- **P1:** the macOS grade is `os_presence(macos, unbound)`, an enrolment claim no verifier can check (D7's *Revised
  from* records the departure from "whose result the repository verifies").
- **P4:** `signed_with` on the consent record, inside the signature: D7's three grades plus `service`.
- **P5:** a same-user process choosing the prompt text is a stated residual.
- **P6:** re-enrolment by I1's supersession; every node upgraded first (an old node keeps the Ed25519 key `Current`).

Corrected facts: the mesh refuses a P-256 certificate in `refuse_if_removed` (no Ed25519 key), before the scheme check;
`keys.sh` at `d974fc76` counts 73 / 37 lines and 22 `[u8; 64]` mentions. **P7**, decided the same day: on Linux and Windows a `tty` consent is signed by a fleet-certified
P-256 consent key held in software (0600 in 0700; a user-only ACL on Windows), so one consent-key kind and one verifier
everywhere; an Ed25519 service key presented as a consent key is refused. I7 is buildable. N7 depends on P1, P3 and P7.

After #764's confirm round, two more decisions of the same day:
- **H1:** a consent key lives under its own principal, `consent:{issuer}/{name}` (SAN `mycelium-consent://`), beside
  `token:{issuer}/{name}`. Nothing supersedes across the two, and `feed_client_keys` feeds `token:` principals only.
  This closes a superseded service key's possession proof that would have kept verifying at `verify_client`.
- **H2:** the fleet CA writes the grade into the consent certificate (a private-OID extension) at
  `tls issue --client --consent --public-key <spki.pem> --grade`, and a body whose grade differs is refused. This
  revises P4. The grade set is `os_presence(macos, unbound)`, `tty`, `recording` and `service`, and the residual is that
  the grade is what enrolment attested.
