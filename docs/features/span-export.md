# Span export

## What it is

A record's identity and outcome sent to an OpenTelemetry collector, as a span — and **not an evidence path**. A
[span export](../guide/00-concepts.md#span-export) reads what the node already keeps, its
[decision trace](../guide/00-concepts.md#decision-trace) and its evidence journal, and turns each record into a span
carrying the [causal fields](../guide/00-concepts.md#causal-records), the rule and its revision, the outcome, a
typed reason where the record has one, and the record's own content hash as an opaque reference. It never carries a
record's inputs, target, effect, subject, resource, arguments, mandate or the audit chain's links: the span's type
has no field for them. A holder of a span reads the record itself by that hash on the node that wrote it.

## When to use it, and when not

- **Use it when** an operator already runs a collector (Jaeger, Tempo, an APM) and wants to see one run's records
  as one trace beside the rest of their telemetry, find a slow or refused run, and then go to the node for the record.
- **Not when** you need to know what was enforced: read the evidence journal on the node (`mycelium explain`),
  never the span store. Not as a complete trace: the decision sink is lossy by design and a record it dropped has no
  span.

## Turn it on

This feature is turned on in code.

- [`SpanCollector`](../../src/span_export.rs) — reads a decision sink by sequence number and an evidence journal by
  cursor, never draining either, and returns the spans it has not returned before; the embedder calls `collect` on
  its own schedule.
- [`RecordSpan`](../../src/span_export.rs) — one record's span; `attributes` is the one rendering, pinned by a golden.
- [`emit`](../../src/span_export.rs) — `span_export::otlp`, feature `otel` (as the skill runner's export is): sends a
  span through an OpenTelemetry `TracerProvider`, its trace id derived from the correlation and its span id from the
  record's hash, at the record's own time; `otlp::provider(endpoint)` builds an OTLP/gRPC one.
- `what_reaches_the_exporter_is_the_pinned_set_and_no_input`, in [`src/span_export.rs`](../../src/span_export.rs):
  a real `TracerProvider` with a capturing exporter receives exactly the pinned attributes, the derived ids and the
  record's time, and no input, subject or prose reason.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The tests, in [`src/span_export.rs`](../../src/span_export.rs):
`what_reaches_the_exporter_is_the_pinned_set_and_no_input`, `the_attribute_set_is_pinned`,
`a_span_carries_the_identity_and_outcome_and_nothing_of_the_inputs` (every part of a record kept off a span, as a
sentinel, searched for in what is sent) and `the_collector_returns_each_record_once_and_never_drains_the_sink`.
The guide's snippet ([guide 19 § sending the records to a collector](../guide/19-replay-and-simulation.md#sending-the-records-to-a-collector-spans-not-evidence))
is a hand-kept copy; no test reads it.

## What it guarantees, and what it does not

- The span's attribute set is `mycelium.schema`, `mycelium.source`, `mycelium.record_sha256`,
  `mycelium.correlation`, `mycelium.parent`, `mycelium.rule`, `mycelium.rule_revision`, `mycelium.outcome` and
  `mycelium.reason`, schema `mycelium.span/1`; a new key is a new schema. An evidence record's `reason` is the
  evaluator's prose, which may name a subject, so its span has none; its outcome is the verdict or the execution
  state. The guarantee report states this as
  [`trace.spans_not_evidence`](../reference/guarantee-catalogue.md) — `enforced` in an `otel` build, by the type,
  whether or not anything is sent, and `not_in_build` without it.
- The hash is the record's content hash — a journal frame's body, a trace line without its newline — the one a
  `parent` names and `mycelium explain --closure` resolves. The same record always exports as the same span, ids
  included: a restarted collector re-sends spans with the ids they had.
- Best-effort: a record the sink dropped, or that another reader drained before a collection, has no span. The
  journal loses nothing, and each evidence record is returned once per collector. A collector's place is in memory.
- **Not built** (the plan row is [`adopter-surface.md`](../plans/adopter-surface.md) Y4): a gateway route that serves
  a record by its hash under `audit:read` — today the holder reads it on the node with `mycelium explain --closure`
  over the files; an export loop in the `mycelium` binary or a configuration section for an endpoint — the embedder
  drives the collector; a durable collector cursor; the collector's OTLP client is not the node's egress-gated one,
  so the endpoint is the embedder's to allow. No span store is read by anything in this repository.

## Go deeper

- Guide: [replay and simulation — sending the records to a collector](../guide/19-replay-and-simulation.md#sending-the-records-to-a-collector-spans-not-evidence).
- Design: no design record; the plan row (decision D17) is the record.
- Code: [`src/span_export.rs`](../../src/span_export.rs).
