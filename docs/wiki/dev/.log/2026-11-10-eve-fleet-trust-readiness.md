## [2026-11-10] ingest | the Eve fleet's `unknown_signer` flake: readiness in both directions, and the grant given back

The Eve fleet CI job failed once with `-32022 unknown_signer` on the first `/mcp` call, in run 42752640837 (PR #750,
attempt 1); the other failure first attributed to it, run 42625751056, was O2 (#746), a timeout after the gateway's
restart. The nodes printed `ready` once the gateway saw the provider's tool and caller-context marker, and nothing waited
for the provider to trust the gateway's key (its outbound handshake anchor, the gateway's sealed identity record, or its
`sys/identity-cert/` record).

**The plant behind the 3/3 reproduction** (temporary, never committed): `CoreCtx::trusted_keys`
(`mycelium-core/src/context.rs`) answered an empty set for any node other than itself while less than
`REPRO_DISTRUST_MS` had passed since its first call, the variable set to 10 000 on the provider only. Before the fix
`run_demo.mjs` failed 3/3 with the CI message; after it, with the plant still in, it passed 3/3, the provider waiting out
the delay before printing `ready`.

The fix: `examples/eve_fleet_node.rs` and `examples/strands_fleet_node.rs` also wait, under one deadline, for
`trusted_identity_keys` of every other member (`FLEET` / `PROVIDERS`). The library half (#752's review): a provider's
`unknown_signer` is now one of `pre_execution_refusal`'s reasons (`src/agent/http/authz.rs`), so at `/mcp` it gives a
grant's claim back (`a_provider_that_does_not_yet_trust_the_gateway_gives_the_grant_back`); the retry itself and an
immediate fetch of the sender's certificate are not built (#753; `docs/features/gateway-caller-identity.md`,
`what-is-proven.md`'s envelope row). Pages touched: `dev/testing/testing.md` (§ poll the predicate, *in both
directions*).
