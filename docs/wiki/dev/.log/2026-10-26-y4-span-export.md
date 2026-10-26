## [2026-10-26] ingest | Y4: trace export as spans

Adopter-surface Y4 (D17): `mycelium::span_export` — `RecordSpan` (identity and outcome only, schema
`mycelium.span/1`, the attribute set pinned), `SpanCollector` (the decision sink by sequence number, never drained;
the evidence journal by cursor), `span_export::otlp` under `otel`; the core guarantee `trace.spans_not_evidence`.
Fail-first: an input value and, separately, the evidence record's prose reason planted onto the span each failed the
sentinel test, the golden and the OTLP test. Pages: [testing/replay](../testing/replay.md) § spans are a view of the
trace; `docs/features/span-export.md`; guide 19; the glossary's `span-export`.
