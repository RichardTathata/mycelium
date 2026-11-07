# The rule catalogue and the decision trace

## What it is

The [rule catalogue](../guide/00-concepts.md#rule-catalogue) describes every decision point the substrate has, by a
stable id: what triggers it, what it reads with each input's scope and freshness, its typed outcomes and reasons, its
guards, its relations to other rules, and its code and tests. It is generated from the descriptors each crate
registers and gated in CI. The [decision trace](../guide/00-concepts.md#decision-trace) records, at the decision points
that are instrumented, what each one read, how it ended and why, against those ids. It is off unless a sink is
attached, changes no decision, reads no clock, draws nothing and never waits; at its bound the newest record is dropped
and counted, so a trace is a prefix plus its drop counts.

## When to use it, and when not

- **Use it when** you need to know why a node did what it did — why a stem installed, why a governor moved, why a
  signal was shed — from the node's own record rather than from its logs.
- **Not when** you want every decision point traced: most rules are catalogue only; the trace covers the stem
  provisioner, the membership governor and signal admission's refusals and sheds, and the catalogue says which per
  rule.

## Turn it on

This feature is turned on in code; the catalogue needs nothing.

- `mycelium rules [--format md|json]`, in [`src/main.rs`](../../src/main.rs): the catalogue this build registers; the
  whole fleet's, the stem host's rules included, is the checked-in [rule catalogue](../reference/rule-catalogue.md).
- [`DecisionSink`](../../mycelium-core/src/decision.rs): the bounded sink, by records and bytes.
- [`with_decision_trace`](../../src/agent/mod.rs): attach a sink to a node before `start()` (the membership governor,
  signal admission); a stem takes `--trace-dir`, a provisioner [`with_decision_trace`](../../mycelium-wasm-host/src/provisioner.rs).
- `mycelium explain <decisions.jsonl> [--catalogue …]`: a trace rendered as what each decision point decided.
- A run recorded with `GOSSIP_RECORD_BUNDLE_DIR` carries the trace into its bundle as `decisions.jsonl` beside
  `coverage.json` ([replay bundles](replay.md)).
- `admission_records_refusals_and_sheds_only_and_changes_no_delivery`, in
  [`mycelium-core/src/ops.rs`](../../mycelium-core/src/ops.rs): with a sink and without, the same delivery; a shed and
  a refusal recorded with their reasons, an admitted signal recording nothing.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for the
trace. The end-to-end run is a test in
[`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs):
`the_trace_changes_no_decision_and_says_what_the_round_decided` — a provisioner with a sink and one without decide and
host identically, and the traced one says what it decided, in order. The catalogue itself is gated by its generator's
golden test, [`mycelium-wasm-host/tests/rule_catalogue.rs`](../../mycelium-wasm-host/tests/rule_catalogue.rs), which
fails when a descriptor moves and the checked-in catalogue does not.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the catalogue lists the rules, and the trace changes none of them.
- **Not built:** a trace of the rules marked catalogue only; a trace that is complete under load (it is a prefix
  with drop counts); a whole agent's replay decision for decision beyond what the scheduler seam covers
  ([what-is-proven § a persisted node's recording](../operations/what-is-proven.md#persisted-node-s-recording-replays-decision)).
  An absent trace in a bundle reads *trace unavailable*, never *no decisions*. A catalogue entry's code and test
  citations are checked by review, not by a gate: `kv.expiry` cited a function and tests the tree did not have until
  its revision 2 ([gossip KV](gossip-kv.md)).
- **Evidence:** the ledger row
  [a live node's recording replays decision for decision](../operations/what-is-proven.md#live-node-s-recording-replays-decision).

## Go deeper

- **Guide:** [19 § Reading what a node decided](../guide/19-replay-and-simulation.md#reading-what-a-node-decided-the-decision-trace).
- **Runbook:** [diagnostics](../operations/diagnostics.md).
- **Design:** [`plans/guarantees-and-rule-catalogue.md`](../plans/guarantees-and-rule-catalogue.md).
- **Code:** [`mycelium-core/src/rule.rs`](../../mycelium-core/src/rule.rs) (the descriptor and its check),
  [`src/rules.rs`](../../src/rules.rs) (the substrate's rules), [`mycelium-core/src/decision.rs`](../../mycelium-core/src/decision.rs)
  (records and the sink).
