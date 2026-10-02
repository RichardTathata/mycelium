# 6 · Declared versus observed

↑ [Tutorials](README.md) · Reference: [Capability lifecycle](../../operations/capability-lifecycle.md)

## Objective

Validate a declaration export, join it to one observer's live provider snapshot,
and expose missing or unexpected providers without calling absence proof of failure.

## How to run

Prerequisites: Rust and loopback sockets. Choose a new, nonexistent export directory;
the producer refuses to overwrite an existing directory. From the repository root:

```sh
cargo run -p mycelium-wasm-host --example first_stem_fleet -- /tmp/mycelium-learning-snapshot
cargo run --example compare_stem_observations -- /tmp/mycelium-learning-snapshot
```

Expect `schema valid; matched=2, missing=0, unexpected=0, excess=0`.
The snapshot is taken before graceful removal. The producer still completes its
recovery assertions afterwards; this is not a continuous monitor.

## What it demonstrates

[The producer](../../../mycelium-wasm-host/examples/first_stem_fleet.rs) exports
`declared.json` from the same `wire_check::Report` used by the CLI and records
`observed.json` from a real `resolve` call. It explicitly maps node IDs to unit
names using the fleet it launched. All hosts share a demo principal here; the
mapping is explicit and is not an authenticated production identity registry.
A principal is not assumed to be a node address.

[The consumer](../../../examples/compare_stem_observations.rs) validates the declaration
against the [pinned JSON Schema](../../reference/declaration.schema.json), checks
hosting eligibility and the artifact provider edge for `demo/echo`, refuses duplicate
nodes/units, and compares the observed count
with this scenario's floor of two. An unknown unit or capability is unexpected;
an absent expected provider is missing from this snapshot.

The observation JSON is this tutorial's adapter, **not a public telemetry schema**.
The floor and `demo/echo` expectation are explicit scenario inputs in the consumer's
code: the exported `presence` field is descriptive text, not a typed fleet-floor
contract. This is deliberately not a general declaration reconciliation engine.

## Try a change, then a failure

Save the original `observed.json`. Remove one provider row and rerun the consumer:
expect `missing=1` and nonzero exit. Restore it, then change one row's `unit` to
`unknown-host`: expect an `UNEXPECTED` line, a missing eligible provider and nonzero
exit. Restore again. Finally change `declared.json`'s `schema` value; schema validation
must reject it before comparison. None of these exercises changes the live fleet.

## Dev notes

CI runs the real export, successful comparison, and missing-provider, unknown-unit,
duplicate-observation and invalid-schema cases. Run the same consumer checks with
`python3 scripts/check-stem-observations.py /tmp/mycelium-learning-snapshot`;
they use temporary copies and preserve the original export.
Schema-shape regressions also have `cargo test --test declaration_schema`.
A consumer for a deployment needs a reviewed identity mapping, observation time,
freshness/partition handling, schema IDs and authority evidence appropriate to its
question. This snapshot proves neither successful calls by every provider nor
current authority. Tutorial 1 separately checks one useful invocation.

Delete only your chosen export directory when finished. For other designs,
`mycelium wire-check <units-dir> --library <artifacts-dir> --format json` supplies
the declaration; supply your own observation adapter and comparison policy.
