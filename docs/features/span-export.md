# Span export

## What it is

A record's identity and outcome sent to an OpenTelemetry collector, as a span — and **not an evidence path**. A
[span export](../guide/00-concepts.md#span-export) reads what the node already keeps, its
[decision trace](../guide/00-concepts.md#decision-trace) and its evidence journal, and turns each record into a span
carrying the [causal fields](../guide/00-concepts.md#causal-records), the rule and its revision, the outcome, a
typed reason where the record has one, and the record's own content hash as an opaque reference. It never carries a
record's inputs, target, effect, subject, resource, arguments, mandate or the audit chain's links: the span's type
has no field for them. A holder of a span reads the record itself by that hash on the node that wrote it, over its
gateway (`GET /gateway/records/{decision,evidence}/{sha256}`, `audit:read`) or with `mycelium explain --closure`: an
evidence record from that node's journal, always; a decision record from its sink while the sink holds it, and after
that only where a trace is written — a `sim` bundle's `decisions.jsonl`, or an embedder that drains its own sink to a
file and declares it — since the `mycelium` binary writes none otherwise.

## When to use it, and when not

- **Use it when** an operator already runs a collector (Jaeger, Tempo, an APM) and wants to see one run's records
  as one trace beside the rest of their telemetry, find a slow or refused run, and then go to the node for the record.
- **Not when** you need to know what was enforced: read the evidence journal on the node (`mycelium explain`),
  never the span store. Not as a complete trace: the decision sink is lossy by design and a record it dropped has no
  span.

## Turn it on

This feature is turned on in code.

- [`SpanCollector`](../../src/span_export.rs) — reads a decision sink through its tap (a bounded channel the sink
  feeds after releasing its lock, so a collector never makes a decision point drop a record) and an evidence journal
  by cursor, never draining either; the embedder calls `collect` on its own schedule.
- [`RecordSpan`](../../src/span_export.rs) — one record's span; `attributes` is the one rendering, pinned by a golden.
- [`emit`](../../src/span_export.rs) — `span_export::otlp`, feature `otel` (as the skill runner's export is): sends a
  span through an OpenTelemetry `TracerProvider`, its trace id derived from the correlation and its span id from the
  record's hash, at the record's own time. `otlp::provider(endpoint, timeout)` builds an OTLP/gRPC one whose batch
  processor runs on a thread of its own, so `emit` only enqueues: an unreachable collector costs dropped spans, never
  a stalled caller (`exporting_to_an_unreachable_collector_never_stalls_the_caller`).
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
- Best-effort: a record the sink dropped has no span. **A sink nothing drains stops keeping records** at whichever
  bound it reaches first — `max_records` (4096 by default) or `max_bytes` by its estimate (1 MiB by default; records
  of a few hundred bytes reach it after a few thousand) — and every later decision is dropped and has no span; each
  `collect` reports how many since the last call, by reason (`dropped`: `full`, `bytes`, `contended`, `oversized`),
  and copies the tap could not hold because the collector fell behind (`tap_dropped`). An embedder that wants spans
  for as long as the node runs also drains its sink, writing the trace. Records kept before the collector tapped the
  sink have no span; one live collector per sink, and a dropped one frees it. The journal loses nothing, and each
  evidence record is returned once per collector. A collector's place is in memory: its journal cursor is a byte
  offset taken after the node opened (and repaired) the journal, so make a new collector after a restart — one that
  outlived a reopen which truncated a torn tail would read misaligned frames, counted as `unparsed`.
- The tap changes no recording: it is not a seam choice, so a recording made with a collector replays identically
  without one (`a_tap_records_nothing_so_a_recording_replays_with_or_without_one`).
- The provider's `shutdown()` flushes within the export timeout, **but not when its queue is full**; a span dropped
  because the queue was full (2048) is counted nowhere this crate can read — the SDK exposes no counter.
- **The correlation is the caller's.** `mycelium.correlation` is the id the request carried, sent as given — the
  substrate checks only its shape. One holding personal data puts it in every span of the run: choose an opaque id.
- A decision's `reason` is one of its rule's typed codes by convention — every shipped decision point passes a code
  from its descriptor — not by the type, which is a `String`. A parent named across runs is an attribute, not a link;
  two byte-identical evidence records are one record by content, so they share a span id.
- The skill runner's own spans (`skillrunner`, also under `otel`) are **outside** `trace.spans_not_evidence`: they
  carry the caller.
- **The record a span names is read on the node, by its hash** — `GET /gateway/records/decision/{sha256}` and
  `GET /gateway/records/evidence/{sha256}`, scope `audit:read`, feature `compliance` (`operations/audit.md` §3a): the
  record's exact bytes, which hash to the span's `mycelium.record_sha256`, and for evidence the reference record in
  the node's signed audit stream that cites it. *Not found* is `never_existed`, `not_written`, or `undetermined` with
  the meanings the node cannot rule out (`evicted` among them); a decision that has left the sink is served only from
  a trace the embedder declares (`GossipAgent::with_decision_trace_file`). No SDK verb reads it.
- **Not built** (the plan row is [`adopter-surface.md`](../plans/adopter-surface.md) Y4): an SDK verb for the
  by-hash read; an export loop in the `mycelium` binary or a configuration section for an endpoint — the embedder
  drives the collector; a durable collector cursor; the collector's OTLP client is not the node's egress-gated one,
  so the endpoint is the embedder's to allow. No span store is read by anything in this repository.

## Go deeper

- Guide: [replay and simulation — sending the records to a collector](../guide/19-replay-and-simulation.md#sending-the-records-to-a-collector-spans-not-evidence).
- Design: no design record; the plan row (decision D17) is the record.
- Code: [`src/span_export.rs`](../../src/span_export.rs).
