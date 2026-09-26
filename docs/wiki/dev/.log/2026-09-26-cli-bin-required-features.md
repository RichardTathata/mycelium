## [2026-09-26] fix | the node binary requires `cli`

Up: [dev](../dev.md) · `Cargo.toml` `[[bin]] mycelium` · CI `ci.yml` (the no-default-features clippy line).

`src/main.rs` sets up logging with `tracing-subscriber`, which only the `cli` feature brings. The binary was
auto-discovered with no feature requirement, so `--no-default-features` builds of bins or tests failed to
compile it. It is now declared with `required-features = ["cli"]`, so such builds skip it. The default build
is unchanged.

CI never saw the failure: its no-default-features clippy line was `--lib` only. It now also covers
`--bins --tests`.

Not fixed here: five examples (`a2a_skill_authority`, `coordination_integrity`, `distributed_lock`,
`llm_agent`, `three_node_demo`) still fail `--all-targets --no-default-features`, because they use features
they do not declare.
