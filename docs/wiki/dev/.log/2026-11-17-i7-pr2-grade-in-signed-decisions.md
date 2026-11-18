## [2026-11-17] ingest | I7 PR 2: the grade in signed decisions

A consent principal decides grants. `grant_decision::verify` reads a decision's key by its kind and checks a consent
signature with the one P-256 verifier (`verify_consent_p256`, gated like the issuer and the reader); the grade is
inside the signed message and must be the grade of the certificate at the principal's mark (`consent_key_grade`), so
an older certificate for the same key written back at its path changes nothing; `signed_with` — key, grade,
`grade_source` — is on the answer's record, the evidence record and the `ActionEnvelope`; consent's authority is
`gateway_consent_principals`; a rule accepts a set of grades. The grade stays the operator's statement everywhere it
is shown. Behaviour changes: `docs/guide/deprecations.md` §118.

Pages: [security](../security.md) § client certificates; [owner-signed-records](../owner-signed-records.md), the
`sys/client-cert/` row; `docs/features/consent-certificates.md`.
