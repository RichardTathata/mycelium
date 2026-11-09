## [2026-11-09] ingest | grants held by a client principal; the shared issuer set (I1, second half, PR 2)

`dev/security.md` states how a client holds a grant (the hex holder segment, possession by the certificate key fed
into the execution authority's `SharedExternalIssuers`) and that membership never reads a client key;
`dev/concurrency/lock-order.md` gains row 77 for the shared set. Plan row `docs/plans/identity-and-lifecycle.md` I1;
feature pages `docs/features/grants.md` and `docs/features/client-certificates.md`.
