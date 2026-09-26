## [2026-09-26] fix | examples declare the features they use

Up: [dev](../dev.md) · `Cargo.toml` `[[example]]` entries · follows
[the node binary requires `cli`](2026-09-26-cli-bin-required-features.md).

Five examples used optional features without declaring them, so `cargo check --all-targets --no-default-features`
failed on them. Each now has an `[[example]]` entry with `required-features`, so builds without those features skip
it:

| Example | Requires | Because |
|---|---|---|
| `a2a_skill_authority` | `tls`, `a2a` | the action evaluator is `gateway` + `tls`; CI already ran it with these |
| `coordination_integrity` | `consensus` | `agent.consensus()`, `ConsensusConfig`, `LeadershipBasis` |
| `distributed_lock` | `consensus` | `agent.consensus().locks()` |
| `llm_agent` | `cli` | its logging setup is `tracing-subscriber` |
| `three_node_demo` | `consensus`, `cli` | both of the above |

Each was checked to pass clippy `-D warnings` with only its declared features. Every existing invocation (CI,
Makefile, the Dockerfiles) builds with default features, which include all of these, so none changes.

CI now runs `cargo check --all-targets --no-default-features` next to the no-default-features clippy line, so a
new example that forgets its features fails there. It is a compile check, not clippy: examples have never been
linted in CI, and `coordination_viz` carries three lints (`manual_is_multiple_of`, `format!` in `format!` args)
that are left for a separate change.
