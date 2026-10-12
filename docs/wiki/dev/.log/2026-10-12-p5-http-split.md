## [2026-10-12] ingest | P5 — `src/agent/http.rs` split by route family

`src/agent/http.rs` (9,889 lines) became `src/agent/http/`: `mod.rs` (state, listener, TLS, router, public
surface), `authz.rs` (the shared authorisation-and-evidence layer), `node.rs`, `mcp_bridge.rs`, `signals.rs`,
`caps.rs`, `mesh.rs`, `kv.rs`, `overlay.rs`, `govern.rs`, `compliance.rs`, `llm_bridge.rs`,
`federation_routes.rs`, and the test modules `http_tests.rs` (module `tests`), `gateway_caller_tests.rs`,
`ae_seam_tests.rs`. Moved by line range, byte-identical, no behaviour change (post-360 plan row P5, #610).

Pages touched: `dev/security.md`, `dev/operations.md`, `dev/architecture/contracts.md`,
`dev/architecture/runtime-invariants.md`, `dev/concurrency/lock-order.md` (rows 15, 16, 55 name their files),
`dev/testing/testing.md` — each `src/agent/http.rs` citation re-pointed to the file that holds the item. Also
`docs/guide/10-language-bridges.md`, `deprecations.md`, `docs/operations/{audit,companions,sso,rbac}.md`,
`docs/design/{consensus-electorate,federated-domains,replay-nondeterminism-inventory}.md`, the `wiki-lint`,
`adversarial-review` and `mycelium-analysis` skills, `scripts/sim-seams-baseline.txt`,
`scripts/kv-namespaces-nonkeys.txt`.

Lesson for the next split: a gate that greps a file by path passes on nothing once the file moves — enumerate
`grep -rn "agent/http.rs"` over `scripts/ .github/ .claude/ docs/` first; and name test files `*_tests.rs`, since
`check-sim-seams.sh` and `check-kv-namespaces.sh` exempt test code by that filename, not by `#[cfg(test)]`.
