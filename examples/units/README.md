# examples/units — the declaration directories (design-time-tooling.md §16, X1)

One directory per example that **advertises a capability, declares a requirement, defines a group
or names a lane**, in the unit-file format `src/capability_config.rs` documents: one file per
deployable unit, its `principal` the node's name in the example. An example that provisions carries
`artifacts/` beside its units — the descriptions `wire-check --library` reads. The `*_viz` variants
share their base's directory.

They are the design-time half of each example: `mycelium wire-check examples/units/<example>` says
whether the vocabulary is consistent *before* the fleet exists, and `scripts/wire-check-examples.sh`
runs every one of them in CI. The gate that keeps them honest: deleting any capability block from
any directory turns its row red — precisely, deleting the last provider of any advertised capability
(`tests/wire_check_examples.rs`) — which is why every advertised capability has a requirer here: the
example's own caller, written down.

**The stem run (X2).** `make examples-both-ways DEMO=provisioning` (or `catalog`, `mcp_toolgrowth`, `model_deploy` — whose code run needs a local Ollama and `MODEL_GGUF`) runs the demo as the
in-process binary and as stem nodes from one image fed this directory
(`docker/docker-compose.stem-examples.yml`), and greps the same markers from both. The hosting units'
`trusted_publishers` is the public half of the suite's test seed (`make stem-keys`), a fixture.

**Not here, and why:** `mcp_tool_authority`, `authority_drain`, `composed_commit` and
`procurement_authority` declare MCP tools and mandates only — a tool is not a capability and the
unit format has no row for it; `federation_node` and `invoke_skill` take their names from the
environment or the command line; `identity_one_record` advertises roles, not capabilities. The
companion crates' examples advertise through their libraries (`blackboard/*`, `tuple/*`, `llm/*`)
and are a second pass.
