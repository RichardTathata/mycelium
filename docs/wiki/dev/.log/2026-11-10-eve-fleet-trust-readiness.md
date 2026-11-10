## [2026-11-10] ingest | the Eve fleet's `unknown_signer` flake: readiness in both directions

The Eve fleet CI job failed twice in about 40 runs with `-32022 unknown_signer` on the first `/mcp` call: the nodes
printed `ready` once the gateway saw the provider's tool and caller-context marker, and nothing waited for the provider
to trust the gateway's key (its outbound handshake anchor or the gateway's `sys/identity-cert/` record). Reproduced
3/3 by delaying how the provider learns keys; `examples/eve_fleet_node.rs` and `examples/strands_fleet_node.rs` now
also wait for `trusted_identity_keys` of every other member (`FLEET` / `PROVIDERS`). Page touched:
`dev/testing/testing.md` (§ poll the predicate, *in both directions*).
