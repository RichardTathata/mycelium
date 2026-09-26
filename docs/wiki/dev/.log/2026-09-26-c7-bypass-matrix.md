## [2026-09-26] ingest | closure plan C7: the bypass matrix

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/plans/boundary-h-closure.md` C7 · test
`src/lib_tests.rs` → `test_c7_the_bypass_matrix_no_door_runs_revoked_work`.

The closure plan's question was: once an agent's authority is revoked, can it still act? This is the test that
answers it for every door the code has, in one place. Its pass condition is the handlers' own counters, never the
absence of an error, because a refusal that happened after the handler ran would still look like a refusal. The
plant runs first: with a valid mandate, the front doors do reach the handlers.

What it does not cover is listed in the test: federation (the same preflight, gated in its own suite), the wiki store
(its own crate), and a removed member reconnecting (C5, still proposed). The deployment variant waits on C12's node
image.
