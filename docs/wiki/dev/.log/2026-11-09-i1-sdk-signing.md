## [2026-11-09] ingest | the SDKs sign as a service and present its own grants (I1, PR 3)

`dev/security.md` notes that the SDKs' `McpClient` and `A2aClient` sign with a `ClientSigner` and prove possession of
a client-held grant, the proof pinned by `tests/fixtures/possession/vector.json` and accepted by the gateway's own
verifier. Plan row `docs/plans/identity-and-lifecycle.md` I1 (PR 3 of 4); feature pages
`docs/features/client-certificates.md` and `docs/features/grants.md`.
