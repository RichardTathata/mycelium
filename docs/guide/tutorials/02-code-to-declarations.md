# 2 · From code to declarations

↑ [Tutorials](README.md) · Next: [Shadow and acceptance](03-shadow-and-acceptance.md)

## Objective

Compare the same provisioning scenario implemented with Rust orchestration and
with generic stem processes. This is the larger, existing co-op example: after
the small echo fleet, follow how its plumbing moves into unit declarations.

## How to run

Prerequisites: Rust, Docker and Docker Compose. The first stem image build can take
several minutes. No model service is needed. From the repository root:

```sh
make examples-both-ways DEMO=provisioning
```

Both runs must reach `All assertions passed`. The scenario also reports
`self-healed` and `shadow-then-accept complete`. A compile alone is not this check.
To inspect the design without starting Docker:

```sh
cargo run --features cli --bin mycelium -- wire-check examples/units/provisioning --library examples/units/provisioning/artifacts
```

## What it demonstrates

Compare [the Rust scenario](../../../examples/coop/src/bin/provisioning.rs) with
[its units](../../../examples/units/provisioning/) and the
[stem compose file](../../../docker/docker-compose.stem-examples.yml).

| Concern | Code form | Declaration form |
|---|---|---|
| Need for `route/optimize` | Application registers a requirement | Worker's `[[requirement]]` |
| Eligible hosts and trust | Host/provisioner configuration | Providers' `[hosts]` |
| Work handoff | Lane setup and producers/consumers | `[[lane]]` roles |
| Executable behaviour | Component/handler implementation | Still executable code; TOML does not implement an optimizer |
| Observation and assertions | Scenario driver | Stem roles and smoke harness still supply scenario behaviour |

**Declaration is not execution.** `[[lane]]`, `[[mandate]]` and `[[rule]]` describe
vocabulary for the offline checker and evaluator; `Stem::start` does not implement
their runtime behaviour. The demo's roles supply the lane work. Writing a rule into
TOML does not attach an authority evaluator or create an enforcement boundary.
See [the stem contract](../../../mycelium-wasm-host/src/stem.rs).

These are equivalent demonstrations of the scenario, not a promise that arbitrary
Rust programs translate to TOML. Bootstrap and deployment are still configured;
capability discovery and installation happen at runtime.

## Try a change, then diagnose it

Copy the units to a temporary directory. Rename the worker's required capability
without changing the artifact's advertised capability, then run `wire-check` on
the copy with its copied `artifacts` directory. Expect `unwired requirement` and
exit 1. Restore the matching name and check again. A green result means the
vocabulary can bind; it does not establish a live provider or sufficient resources.

## Dev notes

The existing stem smoke suite checks the process version; the co-op smoke suite
checks the Rust provisioning binary. See [the evidence ledger](../../operations/what-is-proven.md)
for the exact matrix: model demos and the browser `llm_agent` do not all run both ways.
The harness handles its Compose lifecycle; follow [the units README](../../../examples/units/README.md)
for deployment details. Delete your temporary copy when finished.

For customer code, first extract declarations while retaining useful scenario
assertions. Move only supported fields; keep domain behaviour in artifacts or
handlers. Start with the [integrator contract](../building-on-mycelium.md).
