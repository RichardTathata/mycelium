# 2026-10-25 — X3, second PR: single use at the doors, revocation, grants across nodes, the restart re-wait

Adopter-surface X3's enforcement remainder: each door admits a grant once (`claim_grant_use` + `…/used/{door}`,
`GrantUsed`); `POST /gateway/grants/{agent}/{request}/revoke` at the issuer publishes the revoking checkpoint and the
sweep stops work under the grant within the drain bound; a grant is issued at any gateway for an agent on a member
node, its checkpoints fed to every authority from the KV (`entitle_grants_from_fleet` on the agent's node); a restarted
agent re-waits on the WAL's request. Found and fixed: the sweep cancelled granted work at its first tick (the resource
gate's scope rule). Pages: [operations](../operations.md), `docs/features/grants.md`, guide 21, `deprecations.md` §63.
