## [2026-11-08] ingest | client certificates for services (I1, first half)

`dev/security.md` now states the client-certificate invariant beside the handshake revocation check: a
`mycelium-client://` certificate is refused by both mesh verifiers, `verify_member_cert` is left unchanged so the
client-cert fold can judge issuance, and supersession is a per-principal mark that only rises. Plan row
`docs/plans/identity-and-lifecycle.md` I1; feature page `docs/features/client-certificates.md`.
