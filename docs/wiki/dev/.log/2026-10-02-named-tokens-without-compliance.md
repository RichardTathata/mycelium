## [2026-10-02] ingest | a token table the build cannot enforce fails closed

**What:** `GossipAgent::start()` refuses `gateway_named_tokens` / `gateway_scoped_tokens` in a `gateway` build
without `compliance` (`src/agent/lifecycle.rs`); before, the default binary ran an open gateway with the table set.
Found by doc-coverage run 18. Regression test `a_token_table_this_build_cannot_enforce_refuses_to_start`, seen
failing (`got Ok(())`) on the unfixed code.

**Durable knowledge:** a configuration a build cannot enforce must fail closed at start; and a feature-gated
check belongs in the crate that owns the gate, since a dependency's feature can be unified on independently.

**Pages touched:** `dev/security.md` (§ the open-gateway section), `operations/rbac.md` §1, `operations/tuning.md`,
`deploy/kubernetes/README.md`, CHANGELOG.
