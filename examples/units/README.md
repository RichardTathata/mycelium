# examples/units — the declaration directories (design-time-tooling.md §16, X1)

One directory per example that **advertises a capability, declares a requirement, defines a group
or names a lane**, in the unit-file format `src/capability_config.rs` documents: one file per
deployable unit, its `principal` the node's name in the example. An example that provisions carries
`artifacts/` beside its units — the descriptions `wire-check --library` reads. The `*_viz` variants
share their base's directory.

They are the design-time half of each example. From the repository root,

```sh
cargo run --features cli --bin mycelium -- wire-check examples/units/<example>
# an example with an artifacts/ directory needs it as the library:
cargo run --features cli --bin mycelium -- wire-check examples/units/<example> --library examples/units/<example>/artifacts
```

says whether the vocabulary is consistent *before* the fleet exists. Pass `--library` only where an
`artifacts/` directory exists; without it a provisioning example reports its installable
capabilities as `unwired requirement` and exits 1. `scripts/wire-check-examples.sh` runs every
directory this way in CI. Each directory is also a template: copy one, rename the units, and edit
the sections — the [unit-file reference](../../docs/reference/unit-file.md) lists every field. The gate that keeps them honest: deleting any capability block from
any directory turns its row red — precisely, deleting the last provider of any advertised capability
(`tests/wire_check_examples.rs`) — which is why every advertised capability has a requirer here: the
example's own caller, written down.

**The stem run (X2).** Prerequisites: Docker with Compose, which builds the stem image from
`docker/Dockerfile.stem` on first run. `make examples-both-ways DEMO=provisioning`
(or `catalog`, `mcp_toolgrowth`) runs the Rust binary and the stem version and checks
both. `model_deploy` and `reheal_deploy` also support that target, but their code
runs require local Ollama and `MODEL_GGUF`; only their stem versions run in CI.
For `llm_agent`, the target explicitly skips the interactive browser code run and
checks only the stem recut; the browser still reads `examples/node_n*.toml`.
Stems use `docker/docker-compose.stem-examples.yml`. The hosting units'
`trusted_publishers` is the public half of the suite's test seed (`make stem-keys`), a fixture.

**Not here, and why:** `mcp_tool_authority`, `authority_drain`, `composed_commit` and
`procurement_authority` declare MCP tools and mandates only — a tool is not a capability and the
unit format has no row for it; `federation_node` and `invoke_skill` take their names from the
environment or the command line; `identity_one_record` advertises roles, not capabilities. The
companion crates' examples advertise through their libraries (`blackboard/*`, `tuple/*`, `llm/*`)
and are a second pass.

`catalog/README.md` is the one directory with two deployments — the library on a volume (`catalog`) and in an
object store (`catalog_store`) — because the byte source is a stem flag, not a declaration.
