## [2026-11-16] ingest | The developer's identity, PR 1: the consent certificate and its principal (I7)

I7's row defines no PR split; this PR states one (plan §8, "I7 (PR 1 of 3)"): PR 1 the certificate, the issue flow,
the consent principal's fold and the refusals; PR 2 the decision (P-256 verifier, the grade in the signed message,
`gateway_consent_principals`, `signed_with`); PR 3 `dev`'s side (software key, Swift shim, prompt, queue).

- `consent_identity` beside `client_identity`, one shared TBS walk reading the grade extension; `client_identity`
  refuses the consent scheme; `refuse_if_client` refuses it at the mesh.
- The fold reads both kinds through one helper; a consent key is SHA-256 of its point inside the fold, the point in
  the path; `client_key_standing` `Unknown` for `consent:`; `feed_client_keys` `token:` only.
- The grade OID: rcgen cannot write a `2.25` UUID arc; the maintainer (2026-11-15) chose a registered IANA PEN arc;
  provisional constant under the documentation PEN 32473 until it arrives; the issuer refuses outside a test build;
  a release is blocked on it.
- Pages touched: `dev/security.md` (identity paragraph). Outside the wiki: the plan's I7 row and §8 row,
  `deprecations.md` §112, `CHANGELOG.md`, `features/consent-certificates.md` and the registry, `reference/cli-json.md`.
