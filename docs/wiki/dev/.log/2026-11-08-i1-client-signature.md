## [2026-11-08] ingest | a client's own signature on a gateway call (I1, second half, PR 1)

`dev/security.md` states where `cs`/`cn` is checked (the provider's `verify`, before any cache) and what a relay can
still do (strip it: counted, not yet refused); `dev/concurrency/lock-order.md` gains row 76 for the client replay
window. Plan row `docs/plans/identity-and-lifecycle.md` I1; feature page `docs/features/client-certificates.md`.
