## [2026-11-10] ingest | `gateway_a2a_default_skill` names the default `/a2a` skill from configuration

`dev/security.md` records that a message naming no `metadata.skillId` reaches the embedder's `with_a2a_default_skill`
or the configuration's `GossipConfig::gateway_a2a_default_skill` (`"ns/name"`, `GOSSIP_GATEWAY_A2A_DEFAULT_SKILL`).
`preflight()` refuses the field by name where it would do nothing or name no skill: not one key segment each
(`capability_name_problem`), `/a2a` not served (`TaskCtx::a2a_served`), a different default set in code, a build
without `a2a`; `start()` sets it after `preflight()`. The published stem stays without `a2a` by decision (2026-11-09).
Feature page `docs/features/a2a.md`; configuration row `gateway_a2a_default_skill`; upgrade note `deprecations.md` §94.
