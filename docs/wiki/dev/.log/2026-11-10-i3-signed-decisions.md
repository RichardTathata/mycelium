## [2026-11-10] ingest | signed grant decisions, relayed under `grant:relay` (I3, PR 2, D3 and D5)

`dev/security.md`: a grant decision may be signed by a supervisor holding a client certificate. The node it is
addressed to records the signer as the actor, and a client carrying the decision needs only `grant:relay`. The
decision routes are the first to admit one of two scopes. Unsigned decisions are counted until the refusal release.
Plan `docs/plans/identity-and-lifecycle.md` I3 (PR 2 of 3) and §9 O6; feature page `docs/features/grants.md`;
`deprecations.md` §95; `docs/operations/rbac.md`.
