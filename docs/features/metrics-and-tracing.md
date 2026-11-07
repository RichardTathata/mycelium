# Metrics and tracing

## What it is

In a build with `metrics`, the gateway serves Prometheus metrics at `/metrics` on its HTTP port: the gossip hot path,
the emergent diagnosis, the governors, the artifact library, guardrails, reason routing, and the counters each
companion adds under its own `metrics` feature. Logs go through `tracing`; the `mycelium` and `mycelium-stem` binaries log at INFO
and do not read `RUST_LOG` today (only `skillrunner` does — a separate PR adds it). In a build with
`otel`, the skill runner exports a span per invocation over OTLP, and the substrate's span export turns records into
spans that carry a record's hash, never its content ([span export](span-export.md)). The node's `/stats` and
`system_stats()` give the same counts without a scraper.

## When to use it, and when not

- **Use it when** you run a fleet you need to watch: scrape each node's `/metrics`, or read `/stats` from a script or
  the Ops Console.
- **Not when** you need evidence of what a node decided: a metric or a span is telemetry, and the record is the
  [evidence journal](evidence-journal.md) or the [audit trail](audit-trail.md).

## Turn it on

This feature is turned on in code: build with `metrics` (on `mycelium`, which turns on `mycelium-core`'s; the published
node has it) and give the node an [`http_port`](../reference/configuration.md#http_port). For OTLP, build with `otel`.

- `metrics` on a companion — `mycelium-tuple-space`, `mycelium-blackboard` or `mycelium-wiki` — adds its families, and
  forwards `mycelium/metrics`.
- [`cluster_name`](../reference/configuration.md#cluster_name): a label on every series, nothing more.
- Logs: the `mycelium` binary logs at INFO (WARN under `mycelium test`) and `mycelium-stem` at INFO, both ignoring
  `RUST_LOG`; `skillrunner` reads
  `RUST_LOG`; an application that embeds the library installs its own subscriber.
- `the_connected_peer_gauge_counts_live_writers`, in [`src/lib_tests.rs`](../../src/lib_tests.rs): a series read from
  `/metrics` against the node's own writer map.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`observed_fleet`](../../examples/README.md#the-capability-matrix): three gateways the Ops Console reads, run in CI and
its page rendered by `scripts/ops-console-render.sh`. Each browser showcase in the index's dashboard table serves
`/metrics` too.

## What it guarantees, and what it does not

- No guarantee for metrics themselves — they are observation, and nothing in the
  [guarantee catalogue](../reference/guarantee-catalogue.md) depends on them; the guarantee report lists `metrics` and
  `otel` among a node's build features. A span built by span export is covered by
  [`trace.spans_not_evidence`](../reference/guarantee-catalogue.md#trace.spans_not_evidence) — a span carries a
  record's identity and hash, never its content ([span export](span-export.md)).
- **Not built:** `/metrics`, `/stats`, `/health` and `/ready` are public on a credentialed node (the
  [gateway](http-gateway.md)'s stated exception); a dropped signal or a slow subscriber is logged, not surfaced to the
  client; the series reflect one node's view, not the fleet's.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for metrics on their own.

## Go deeper

- **Guide:** [14, patterns and pitfalls](../guide/14-patterns-and-pitfalls.md), whose entries several log lines name.
- **Runbook:** [observability](../operations/observability.md) and [metrics](../operations/metrics.md), the list of
  every series.
- **Design:** no design record; the runbooks are the record.
- **Code:** `prometheus_handle` in [`src/agent/http/mod.rs`](../../src/agent/http/mod.rs) and `metrics_handler` in
  [`src/agent/http/node.rs`](../../src/agent/http/node.rs).
