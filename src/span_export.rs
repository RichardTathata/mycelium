//! **Trace export as spans** (`docs/plans/adopter-surface.md` row Y4, decision D17): a record's identity and outcome,
//! sent to an OpenTelemetry collector — and **not an evidence path**.
//!
//! A [`RecordSpan`] carries, for one decision record ([`DecisionRecord`]) or one evidence record ([`AeEvidence`]):
//! the run it belongs to (`correlation`), the record it depends on (`parent`, by content hash), the rule and its
//! revision, the outcome, a typed reason where the record has one, and the record's own **content hash** as an
//! opaque reference. That is the whole type: there is no field for a decision's inputs, its target or its effect, nor
//! for an evidence record's subject, resource, destination, arguments digest, checked constraints, history, route
//! facts or mandate digest, nor for the audit chain's links. Those are kept on the node on purpose — the evidence
//! journal is node-local and never gossiped — so leaving them out is the type's shape rather than a filter that
//! could be forgotten, as [`AeReference`](crate::AeReference) is for the chain.
//!
//! **Not an evidence path.** A span store holds what a collector was sent, and nothing in this repository reads one.
//! A span says *which* record and *what it decided*; what was enforced is the record itself, read by the hash the
//! span carries on the node that wrote it — over its gateway, `GET /gateway/records/{decision,evidence}/{sha256}` under
//! `audit:read` (feature `compliance`), or with `mycelium explain --closure <hash>` over its evidence journal and
//! decision trace (X4) — and verified there. The guarantee report says so: `trace.spans_not_evidence`.
//!
//! **What the evidence span leaves out that the decision span keeps.** A decision record's `reason` is one of its
//! rule's typed reason codes, never free text, and is carried. An evidence record's `reason` is the evaluator's
//! human-legible prose, which may name a subject or a resource; it is **not** carried, and the span's `reason` is
//! absent. Its outcome is the verdict (`permit` / `deny` / `indeterminate`) or, for an execution record, what became
//! of the dispatch (`completed`, `failed`, …).
//!
//! **Pull, from what the node already keeps, without taking its locks.** A [`SpanCollector`] reads a [`DecisionSink`]
//! through the sink's tap ([`DecisionSink::tap`]) — a bounded channel the sink feeds, after releasing its own lock,
//! with a copy of each record it keeps — and an evidence journal by cursor, the exporter shape the journal already
//! has. It never takes the ring's lock (a reader that did would make a concurrent decision point drop its record
//! `Contended`, since a decision point never waits), and never drains the ring, so a replay bundle still gets every
//! record. The embedder decides when to collect; this module runs no loop, reads no clock and draws no random number:
//! a span's time is the record's own event time, and its trace and span ids are derived from the correlation and the
//! hashes ([`RecordSpan::trace_id`], [`RecordSpan::span_id`]), so the same record always exports as the same span.
//!
//! **Best-effort, as the trace is.** The decision sink is lossy by design and a record it dropped has no span. **A
//! sink nothing drains fills**: it holds at most `max_records` records (4096 by default) **and** `max_bytes` of
//! them by its estimate (1 MiB by default) — whichever is reached first; with records of a few hundred bytes the byte
//! bound comes first, after a few thousand. Every later decision is dropped (`Full` or `Bytes`) and has no span either
//! — [`Collected::dropped`] says how many since the last call, by reason; an embedder that wants spans for as long as
//! the node runs drains its sink (writing the trace) as well. A copy the tap could not hold
//! because the collector fell behind is counted ([`Collected::tap_dropped`]). Records kept before the collector
//! tapped the sink have no span. The journal is not lossy, and every evidence record is returned once per collector.
//! A collector is not durable: a new one reads the journal from the beginning, so a restarted exporter re-sends spans
//! a collector already accepted — the same span, with the same ids. Its journal cursor is a byte offset taken after
//! the node opened (and repaired) the journal; a collector that outlives a reopen that truncated a torn tail would read
//! from a stale offset — misaligned frames, counted as `unparsed`, never guessed at. Make a new collector after a
//! restart.
//!
//! **The correlation is the caller's.** `mycelium.correlation` is the id the request carried, sent as given; the
//! substrate checks only its shape. A caller that puts personal data in it puts personal data in every span of the
//! run — choose an opaque id.
//!
//! The OTLP side (`otlp`) is under the `otel` feature, as the skill runner's export is; the mapping is in every
//! build so its field set is tested in every build.

#[cfg(all(feature = "gateway", feature = "tls"))]
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;

#[cfg(all(feature = "gateway", feature = "tls"))]
use crate::agent::action_evaluator::{AeEvidence, RecordKind};
use crate::agent::causal::content_hash;
#[cfg(all(feature = "gateway", feature = "tls"))]
use crate::agent::evidence_journal::{read_evidence_journal_from, EvidenceCursor, JournalEntry};
use crate::decision::{AlreadyTapped, DecisionRecord, DecisionSink, DropCounts};

/// The schema id a span carries as `mycelium.schema`; a change to its attribute set is a new one.
pub const SPAN_SCHEMA: &str = "mycelium.span/1";

/// Every attribute key a [`RecordSpan`] can carry, in the order [`RecordSpan::attributes`] renders them. Pinned by a
/// golden (`span_export::tests::the_attribute_set_is_pinned`): adding a key is a schema change made in the open.
pub const ATTRIBUTE_KEYS: &[&str] = &[
    "mycelium.schema",
    "mycelium.source",
    "mycelium.record_sha256",
    "mycelium.correlation",
    "mycelium.parent",
    "mycelium.rule",
    "mycelium.rule_revision",
    "mycelium.outcome",
    "mycelium.reason",
];

/// Which record a span names.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanSource {
    /// A decision trace record (`decisions.jsonl`): best-effort, the sink drops and counts.
    Decision,
    /// An evidence record of kind `decided`: the verdict at an enforcement point.
    EvidenceDecided,
    /// An evidence record of kind `execution`: what became of the dispatch.
    EvidenceExecution,
}

impl SpanSource {
    /// The span's name and its `mycelium.source` value.
    pub fn label(self) -> &'static str {
        match self {
            SpanSource::Decision => "mycelium.decision",
            SpanSource::EvidenceDecided => "mycelium.evidence.decided",
            SpanSource::EvidenceExecution => "mycelium.evidence.execution",
        }
    }
}

/// One record's identity and outcome, as a span carries it — and nothing else (D17).
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RecordSpan {
    pub source: SpanSource,
    /// Lowercase hex SHA-256 of the record's bytes as written: a journal frame's body, a trace line without its
    /// newline — the hash a `parent` names and `mycelium explain --closure` takes. Opaque to a collector.
    pub record_sha256: String,
    /// The run the record belongs to (X4), when it carries one.
    pub correlation: Option<String>,
    /// The content hash of the record it depends on (X4), when it is not a root.
    pub parent: Option<String>,
    /// A decision's rule id; an evidence record's enforcement point.
    pub rule: String,
    /// A decision's rule revision; an evidence record's policy revision.
    pub rule_revision: String,
    /// A decision's outcome kind; an evidence record's verdict or execution state.
    pub outcome: String,
    /// A decision's typed reason code. Absent for evidence, whose reason is prose (module doc).
    pub reason: Option<String>,
    /// The record's own event time (epoch ms), used as the span's time; not an attribute.
    pub at_ms: Option<u64>,
}

/// A serde label for a unit enum (`snake_case` everywhere it is used here), or `unknown` for a shape that is not one.
fn label_of<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => "unknown".to_string(),
    }
}

#[cfg(all(feature = "gateway", feature = "tls"))]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The first `N` bytes of a lowercase hex string, or `None` when it is not hex of at least that length.
fn hex_prefix<const N: usize>(h: &str) -> Option<[u8; N]> {
    let h = h.as_bytes();
    if h.len() < N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        let s = std::str::from_utf8(&h[i * 2..i * 2 + 2]).ok()?;
        *b = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

impl RecordSpan {
    /// The span for one decision record, as the sink holds it. Its hash is the hash of the line the trace writes
    /// ([`DecisionRecord::to_string_line`]), which is the one a `parent` cites.
    pub fn from_decision(r: &DecisionRecord) -> Self {
        RecordSpan {
            source: SpanSource::Decision,
            record_sha256: content_hash(r.to_string_line().as_bytes()),
            correlation: r.correlation.clone(),
            parent: r.parent.clone(),
            rule: r.rule.clone(),
            rule_revision: r.rule_revision.to_string(),
            outcome: label_of(&r.outcome),
            reason: Some(r.reason.clone()),
            at_ms: r.at_ms,
        }
    }

    /// The span for one evidence record, from its journal entry; `None` for a frame that is not evidence JSON (it is
    /// skipped and counted by the collector, never guessed at). The hash is the journal's own.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub fn from_evidence(entry: &JournalEntry) -> Option<Self> {
        let ev: AeEvidence = serde_json::from_slice(&entry.bytes).ok()?;
        let (source, outcome) = match ev.kind {
            RecordKind::Decided => (SpanSource::EvidenceDecided, ev.decision.label().to_string()),
            RecordKind::Execution => (SpanSource::EvidenceExecution, label_of(&ev.execution)),
        };
        Some(RecordSpan {
            source,
            record_sha256: hex(&entry.content_hash),
            correlation: ev.correlation,
            parent: ev.parent,
            rule: ev.enforcement_point,
            rule_revision: ev.policy_revision,
            outcome,
            reason: None,
            // `0` is the field's default for a record written before it existed: visibly not a time.
            at_ms: (ev.at_ms != 0).then_some(ev.at_ms),
        })
    }

    /// The attributes the span carries, keys from [`ATTRIBUTE_KEYS`] in its order; an absent value is omitted, never
    /// sent empty. The one rendering: the OTLP side sends exactly this.
    pub fn attributes(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![
            ("mycelium.schema", SPAN_SCHEMA.to_string()),
            ("mycelium.source", self.source.label().to_string()),
            ("mycelium.record_sha256", self.record_sha256.clone()),
        ];
        if let Some(c) = &self.correlation {
            v.push(("mycelium.correlation", c.clone()));
        }
        if let Some(p) = &self.parent {
            v.push(("mycelium.parent", p.clone()));
        }
        v.push(("mycelium.rule", self.rule.clone()));
        v.push(("mycelium.rule_revision", self.rule_revision.clone()));
        v.push(("mycelium.outcome", self.outcome.clone()));
        if let Some(r) = &self.reason {
            v.push(("mycelium.reason", r.clone()));
        }
        v
    }

    /// The span's trace id: the first 16 bytes of SHA-256 over the correlation, so every record of one run is one
    /// trace in a collector; a record with no correlation is a trace of its own (its own hash's first 16 bytes).
    pub fn trace_id(&self) -> [u8; 16] {
        let basis = match &self.correlation {
            Some(c) => content_hash(c.as_bytes()),
            None => self.record_sha256.clone(),
        };
        hex_prefix::<16>(&basis).unwrap_or([0u8; 16])
    }

    /// The span's id: the first 8 bytes of the record's hash.
    pub fn span_id(&self) -> [u8; 8] {
        hex_prefix::<8>(&self.record_sha256).unwrap_or([0u8; 8])
    }

    /// The parent span's id — the first 8 bytes of `parent` — when the record has a parent and shares a trace with
    /// it, which it does only through a correlation: a parent named without one is an attribute, not a link.
    pub fn parent_span_id(&self) -> Option<[u8; 8]> {
        self.correlation.as_ref()?;
        hex_prefix::<8>(self.parent.as_deref()?)
    }
}

/// What one [`SpanCollector::collect`] read.
#[non_exhaustive]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub spans: Vec<RecordSpan>,
    /// Decisions the sink dropped since the last collection, by reason — `full` (it held `max_records`), `bytes` (it
    /// held `max_bytes` by its estimate), `contended`, `oversized`. They have no span, and nothing here can give them
    /// one: a sink nothing drains stops keeping records at whichever bound it reaches first.
    pub dropped: DropCounts,
    /// Decision copies the tap dropped since the last collection because the collector had not kept up.
    pub tap_dropped: u64,
    /// Journal frames that were not evidence JSON — skipped, counted. Always 0 in a build without the journal.
    pub unparsed: u64,
    /// Whether the journal read stopped at its page bound rather than the end, so a caller collects again.
    pub more: bool,
}

/// Reads what the node already keeps and returns the spans it has not returned before. Pull-based: the embedder calls
/// [`collect`](Self::collect) on its own schedule.
#[derive(Debug)]
pub struct SpanCollector {
    sink: Option<(Arc<DecisionSink>, tokio::sync::mpsc::Receiver<DecisionRecord>)>,
    seen_dropped: DropCounts,
    seen_tap_dropped: u64,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    journal: Option<PathBuf>,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    cursor: EvidenceCursor,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    page_records: usize,
    #[cfg(all(feature = "gateway", feature = "tls"))]
    page_bytes: usize,
}

impl Default for SpanCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanCollector {
    /// A collector with nothing to read; add a sink, a journal or both. A journal page is 1024 records or 4 MiB.
    pub fn new() -> Self {
        SpanCollector {
            sink: None,
            seen_dropped: DropCounts::default(),
            seen_tap_dropped: 0,
            #[cfg(all(feature = "gateway", feature = "tls"))]
            journal: None,
            #[cfg(all(feature = "gateway", feature = "tls"))]
            cursor: EvidenceCursor::default(),
            #[cfg(all(feature = "gateway", feature = "tls"))]
            page_records: 1024,
            #[cfg(all(feature = "gateway", feature = "tls"))]
            page_bytes: 4 << 20,
        }
    }

    /// Read this decision sink — the one handed to the node's decision points — through its tap
    /// ([`DecisionSink::tap`]): a copy of each record it keeps from now on, `capacity` waiting at most. The collector
    /// never takes the ring's lock, so it never makes a decision point drop a record, and never drains the ring.
    /// `capacity` is clamped to `1..=`[`MAX_TAP_CAPACITY`](crate::decision::MAX_TAP_CAPACITY). A sink tapped by another
    /// live collector is refused; once that collector is dropped, its sink can be tapped again.
    pub fn with_decision_sink(mut self, sink: Arc<DecisionSink>, capacity: usize) -> Result<Self, AlreadyTapped> {
        let rx = sink.tap(capacity)?;
        self.seen_dropped = sink.drop_counts();
        self.seen_tap_dropped = sink.tap_dropped();
        self.sink = Some((sink, rx));
        Ok(self)
    }

    /// Read the evidence journal at this path — the one `with_evidence_journal` opened — by cursor. The journal exists
    /// only where the evaluator seam does (`gateway` + `tls`).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub fn with_evidence_journal(mut self, path: impl Into<PathBuf>) -> Self {
        self.journal = Some(path.into());
        self
    }

    /// One journal page's bounds, records and bytes (a record larger than the byte bound is returned alone).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub fn with_page(mut self, records: usize, bytes: usize) -> Self {
        self.page_records = records.max(1);
        self.page_bytes = bytes.max(1);
        self
    }

    /// The spans for every record kept since the last collection: the copies the sink's tap holds, then one journal
    /// page from the cursor, with what the sink dropped since the last call. A journal that does not exist yet reads as
    /// empty.
    pub fn collect(&mut self) -> std::io::Result<Collected> {
        let mut out = Collected::default();
        if let Some((sink, rx)) = &mut self.sink {
            while let Ok(r) = rx.try_recv() {
                out.spans.push(RecordSpan::from_decision(&r));
            }
            let (dropped, tap) = (sink.drop_counts(), sink.tap_dropped());
            out.dropped = dropped.since(&self.seen_dropped);
            out.tap_dropped = tap.saturating_sub(self.seen_tap_dropped);
            (self.seen_dropped, self.seen_tap_dropped) = (dropped, tap);
        }
        #[cfg(all(feature = "gateway", feature = "tls"))]
        if let Some(path) = &self.journal {
            let page = read_evidence_journal_from(path, self.cursor, self.page_records, self.page_bytes)?;
            for e in &page.entries {
                match RecordSpan::from_evidence(e) {
                    Some(s) => out.spans.push(s),
                    None => out.unparsed += 1,
                }
            }
            self.cursor = page.next;
            out.more = page.more;
        }
        Ok(out)
    }
}

/// The OTLP side: a [`RecordSpan`] sent through an OpenTelemetry `TracerProvider`, as the skill runner sends its own.
#[cfg(feature = "otel")]
pub mod otlp {
    use std::time::{Duration, SystemTime};

    use opentelemetry::trace::{
        SpanContext, SpanId, SpanKind, TraceContextExt as _, TraceFlags, TraceId, TraceState, Tracer as _,
        TracerProvider as _,
    };
    use opentelemetry::{Context, KeyValue};
    use opentelemetry_sdk::trace::TracerProvider;

    use super::RecordSpan;

    /// The instrumentation scope's name.
    pub const SCOPE: &str = "mycelium.records";

    /// The span's time: the record's own event time, or `None` (the SDK stamps it).
    fn at(span: &RecordSpan) -> Option<SystemTime> {
        span.at_ms.map(|ms| SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
    }

    /// Send one span: named by its source, its attributes exactly [`RecordSpan::attributes`], its ids derived from
    /// the record, its parent the record it depends on when they share a trace. A zero-length span at the record's
    /// time — a record is an instant, not an interval.
    pub fn emit(provider: &TracerProvider, span: &RecordSpan) {
        let tracer = provider.tracer(SCOPE);
        let trace_id = TraceId::from_bytes(span.trace_id());
        let mut builder = tracer
            .span_builder(span.source.label())
            .with_kind(SpanKind::Internal)
            .with_trace_id(trace_id)
            .with_span_id(SpanId::from_bytes(span.span_id()))
            .with_attributes(span.attributes().into_iter().map(|(k, v)| KeyValue::new(k, v)));
        if let Some(t) = at(span) {
            builder = builder.with_start_time(t).with_end_time(t);
        }
        let parent = match span.parent_span_id() {
            Some(p) => Context::new().with_remote_span_context(SpanContext::new(
                trace_id,
                SpanId::from_bytes(p),
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            )),
            None => Context::new(),
        };
        let mut s = builder.start_with_context(&tracer, &parent);
        match at(span) {
            Some(t) => opentelemetry::trace::Span::end_with_timestamp(&mut s, t),
            None => opentelemetry::trace::Span::end(&mut s),
        }
    }

    /// Send every span in order.
    pub fn export(provider: &TracerProvider, spans: &[RecordSpan]) {
        for s in spans {
            emit(provider, s);
        }
    }

    /// The longest one export to the collector may take before it is abandoned (and its spans dropped).
    pub const EXPORT_TIMEOUT: Duration = Duration::from_secs(5);
    /// Spans waiting to be sent, at most; past that a span is dropped by the processor, never waited for.
    pub const MAX_QUEUE: usize = 2048;

    /// A provider that sends to an OTLP/gRPC collector at `endpoint` **without blocking the caller**: a batch processor
    /// on a thread of its own ([`TokioCurrentThread`](opentelemetry_sdk::runtime::TokioCurrentThread)), at most
    /// [`MAX_QUEUE`] spans waiting and each export abandoned after `export_timeout` (use [`EXPORT_TIMEOUT`]). `emit`
    /// only enqueues, so an unreachable or slow collector costs a dropped span, never a stalled decision loop — on a
    /// current-thread runtime as on a multi-thread one. Call it inside a Tokio runtime (tonic's channel is built on
    /// it). `shutdown()` flushes, waiting at most the timeout — **but not when the queue is full**: the processor's
    /// shutdown message cannot be queued then, and `shutdown` returns an error without flushing. A span dropped because
    /// the queue was full is not counted anywhere this crate can read (the SDK logs it and exposes no counter). Egress: the endpoint is the embedder's to put on the
    /// allow-list; this client is not the node's gated one.
    pub fn provider(endpoint: &str, export_timeout: Duration) -> Result<TracerProvider, opentelemetry::trace::TraceError> {
        use opentelemetry_otlp::WithExportConfig as _;
        use opentelemetry_sdk::trace::{BatchConfigBuilder, BatchSpanProcessor};
        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .with_timeout(export_timeout)
            .build()?;
        let processor = BatchSpanProcessor::builder(exporter, opentelemetry_sdk::runtime::TokioCurrentThread)
            .with_batch_config(
                BatchConfigBuilder::default().with_max_queue_size(MAX_QUEUE).with_max_export_timeout(export_timeout).build(),
            )
            .build();
        Ok(TracerProvider::builder().with_span_processor(processor).build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(all(feature = "gateway", feature = "tls"))]
    use crate::agent::journal::JournalEntry as Entry;
    use crate::decision::{InputSnapshot, OutcomeKind, Provenance};

    /// Sentinels for every part of a record D17 keeps off a span. A test fails if any reaches an exported attribute.
    pub(super) const SECRET: &[&str] = &[
        "SECRET-INPUT-VALUE",
        "SECRET-INPUT-SOURCE",
        "SECRET-TARGET",
        "SECRET-EFFECT",
        "SECRET-TRIGGER",
        "SECRET-SUBJECT",
        "SECRET-RESOURCE",
        "SECRET-OPERATION",
        "SECRET-DESTINATION",
        "SECRET-EXECUTION-IDENTITY",
        "SECRET-ARGS-DIGEST",
        "SECRET-CHECKED",
        "SECRET-PROSE-REASON",
        "SECRET-MANDATE",
        "SECRET-OPERATION-ID",
        "SECRET-ATTEMPT-ID",
        "SECRET-CATALOGUE",
        "SECRET-NODE",
        "SECRET-ROUTE-DESTINATION",
        "SECRET-DATA-CLASS",
        "SECRET-REFUSED-DESTINATION",
        "SECRET-REFUSAL-REASON",
        "SECRET-HISTORY-ATTEMPT",
        "SECRET-HISTORY-UNAVAILABLE",
        "SECRET-ORIGIN-DOMAIN",
        "SECRET-SUPERSEDES",
        "SECRET-DURABILITY-FAILURE",
    ];

    pub(super) fn decision() -> DecisionRecord {
        DecisionRecord::new("prov.install", 3, "SECRET-TRIGGER", OutcomeKind::Refusal, "not_eligible")
            .node("SECRET-NODE", 1)
            .at(1_700_000_000_123)
            .target("SECRET-TARGET")
            .effect("SECRET-EFFECT")
            .correlation("run-1")
            .parent_hash("ab".repeat(32))
            .input(InputSnapshot::new("SECRET-INPUT-SOURCE", "SECRET-INPUT-VALUE", Provenance::Supplied))
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub(super) fn evidence_bytes(kind: &str, execution: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema": "mycelium.ae/evidence/1",
            "kind": kind,
            "at_ms": 1_700_000_000_456u64,
            "subject": "SECRET-SUBJECT",
            "operation": "SECRET-OPERATION",
            "resource": "SECRET-RESOURCE",
            "destination": "SECRET-DESTINATION",
            "execution_identity": "SECRET-EXECUTION-IDENTITY",
            "decision": "deny",
            "execution": execution,
            "policy_revision": "rev-7",
            "policy_digest": null,
            "enforcement_point": "gateway:mcp/tools/call",
            "catalogue": "SECRET-CATALOGUE",
            "catalogue_revision": "1",
            "mapping_status": "mapped",
            "operation_id": "SECRET-OPERATION-ID",
            "attempt_id": "SECRET-ATTEMPT-ID",
            "arguments_digest": "SECRET-ARGS-DIGEST",
            "checked": ["SECRET-CHECKED"],
            "reason": "SECRET-PROSE-REASON",
            "correlation": "run-1",
            "parent": "cd".repeat(32),
            "node": "SECRET-NODE",
            "mandate_digest": "SECRET-MANDATE",
            "route": {
                "destination": { "name": "SECRET-ROUTE-DESTINATION", "locality": "external" },
                "data_class": { "class": "SECRET-DATA-CLASS", "source": "declared" },
                "refused": [{ "destination": { "name": "SECRET-REFUSED-DESTINATION", "locality": "in_domain" }, "reason": "SECRET-REFUSAL-REASON" }],
            },
            "history": {
                "scope": "enforcement_point", "window_ms": 1000, "as_of_ms": 5,
                "attempts": ["SECRET-HISTORY-ATTEMPT"], "complete": false, "unavailable": "SECRET-HISTORY-UNAVAILABLE",
            },
            "origin_domain": "SECRET-ORIGIN-DOMAIN",
            "supersedes": "SECRET-SUPERSEDES",
            "effect_durability": { "failed": "SECRET-DURABILITY-FAILURE" },
        }))
        .unwrap()
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    pub(super) fn entry(bytes: Vec<u8>) -> Entry {
        use sha2::Digest as _;
        Entry { seq: 0, content_hash: sha2::Sha256::digest(&bytes).into(), bytes }
    }

    /// Everything a span would send — its name and every attribute value — as one string to search.
    pub(super) fn sent(s: &RecordSpan) -> String {
        let mut out = s.source.label().to_string();
        for (k, v) in s.attributes() {
            out.push_str(&format!(" {k}={v}"));
        }
        out
    }

    #[test]
    fn the_attribute_set_is_pinned() {
        // The golden: a fully populated decision span renders exactly these keys, in this order. A new key is a new
        // `SPAN_SCHEMA`, decided in the open (D17).
        let golden = [
            "mycelium.schema",
            "mycelium.source",
            "mycelium.record_sha256",
            "mycelium.correlation",
            "mycelium.parent",
            "mycelium.rule",
            "mycelium.rule_revision",
            "mycelium.outcome",
            "mycelium.reason",
        ];
        assert_eq!(ATTRIBUTE_KEYS, golden);
        let s = RecordSpan::from_decision(&decision());
        let keys: Vec<_> = s.attributes().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, golden);
        assert_eq!(SPAN_SCHEMA, "mycelium.span/1");
        // The evidence span has no reason; every other key is the same set.
        #[cfg(all(feature = "gateway", feature = "tls"))]
        {
            let e = RecordSpan::from_evidence(&entry(evidence_bytes("decided", "none"))).unwrap();
            let keys: Vec<_> = e.attributes().into_iter().map(|(k, _)| k).collect();
            assert_eq!(keys, &golden[..8]);
        }
    }

    #[test]
    fn a_span_carries_the_identity_and_outcome_and_nothing_of_the_inputs() {
        let d = decision();
        let s = RecordSpan::from_decision(&d);
        let text = sent(&s);
        for secret in SECRET {
            assert!(!text.contains(secret), "{secret} reached the decision span: {text}");
        }
        assert_eq!(s.rule, "prov.install");
        assert_eq!(s.rule_revision, "3");
        assert_eq!(s.outcome, "refusal");
        assert_eq!(s.reason.as_deref(), Some("not_eligible"));
        assert_eq!(s.correlation.as_deref(), Some("run-1"));
        assert_eq!(s.parent, Some("ab".repeat(32)));
        // The hash is the trace line's, the one `explain --closure` resolves.
        assert_eq!(s.record_sha256, content_hash(d.to_string_line().as_bytes()));

        #[cfg(all(feature = "gateway", feature = "tls"))]
        for (kind, execution, outcome) in [("decided", "none", "deny"), ("execution", "failed", "failed")] {
            let bytes = evidence_bytes(kind, execution);
            let parsed: AeEvidence = serde_json::from_slice(&bytes).expect("the fixture is an evidence record");
            assert!(
                parsed.route.is_some() && parsed.history.is_some() && parsed.origin_domain.is_some()
                    && parsed.supersedes.is_some() && parsed.effect_durability.is_some() && parsed.mandate_digest.is_some(),
                "every kept-off field is set on the fixture, so its sentinel is tested"
            );
            let e = RecordSpan::from_evidence(&entry(bytes.clone())).unwrap();
            let text = sent(&e);
            for secret in SECRET {
                assert!(!text.contains(secret), "{secret} reached the {kind} evidence span: {text}");
            }
            assert_eq!(e.rule, "gateway:mcp/tools/call");
            assert_eq!(e.rule_revision, "rev-7");
            assert_eq!(e.outcome, outcome);
            assert_eq!(e.reason, None);
            assert_eq!(e.record_sha256, content_hash(&bytes));
            assert_eq!(e.at_ms, Some(1_700_000_000_456));
        }
    }

    #[test]
    fn ids_derive_from_the_record_so_one_run_is_one_trace() {
        let s = RecordSpan::from_decision(&decision());
        #[cfg(all(feature = "gateway", feature = "tls"))]
        {
            let e = RecordSpan::from_evidence(&entry(evidence_bytes("decided", "none"))).unwrap();
            assert_eq!(s.trace_id(), e.trace_id(), "one correlation, one trace");
        }
        assert_eq!(s.trace_id(), hex_prefix::<16>(&content_hash(b"run-1")).unwrap());
        assert_eq!(s.span_id(), hex_prefix::<8>(&s.record_sha256).unwrap());
        assert_eq!(s.parent_span_id(), Some([0xab; 8]));
        // Without a correlation, a parent is an attribute, not a link, and the trace is the record's own.
        let lone = RecordSpan::from_decision(&DecisionRecord::new("r", 1, "t", OutcomeKind::Action, "x").parent_hash("ab".repeat(32)));
        assert_eq!(lone.parent_span_id(), None);
        assert_eq!(lone.trace_id(), hex_prefix::<16>(&lone.record_sha256).unwrap());
        // A parent that is not hex is not a link either.
        let odd = RecordSpan::from_decision(&DecisionRecord::new("r", 1, "t", OutcomeKind::Action, "x").correlation("c").parent_hash("zz"));
        assert_eq!(odd.parent_span_id(), None);
        // The same record is the same span.
        assert_eq!(RecordSpan::from_decision(&decision()), s);
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn a_frame_that_is_not_evidence_is_counted_never_guessed() {
        assert!(RecordSpan::from_evidence(&entry(b"not json".to_vec())).is_none());
        assert!(RecordSpan::from_evidence(&entry(b"{\"schema\":\"other\"}".to_vec())).is_none());
    }

    #[test]
    fn the_collector_reads_the_sink_by_sequence_number_and_never_drains_it() {
        let sink = Arc::new(DecisionSink::default());
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 64).unwrap();
        assert!(c.collect().unwrap().spans.is_empty());
        sink.record(decision()).unwrap();
        sink.record(decision().correlation("run-2")).unwrap();
        let got = c.collect().unwrap();
        assert_eq!(got.spans.len(), 2);
        assert_eq!(sink.snapshot().len(), 2, "the collector reads the sink, it does not drain it");
        assert!(c.collect().unwrap().spans.is_empty(), "each record once");
        // A drain by another reader (a bundle's export) does not make the collector re-send or lose its place.
        let _ = sink.drain();
        sink.record(decision()).unwrap();
        let again = c.collect().unwrap();
        assert_eq!(again.spans.len(), 1);
        assert_eq!(again.unparsed, 0);
    }

    /// #684's review, finding 1: a collector that took the ring's lock (a blocking `snapshot`) made a concurrent
    /// decision point drop its record `Contended` — 1,250 in the reviewer's probe. Through the tap, none.
    #[test]
    fn a_collector_never_makes_a_decision_point_drop_a_record() {
        let sink = Arc::new(DecisionSink::new(crate::decision::SinkConfig { max_records: 1 << 20, max_bytes: 1 << 30, max_input_bytes: 256 }));
        // A ring already holding ten thousand records, as a node's would: a reader that took its lock would hold it for
        // as long as a copy of all of them takes, and every decision meanwhile would drop.
        for _ in 0..10_000 {
            sink.record(decision()).unwrap();
        }
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 1 << 16).unwrap();
        let writer = {
            let sink = Arc::clone(&sink);
            std::thread::spawn(move || {
                for _ in 0..20_000 {
                    let _ = sink.record(decision());
                }
            })
        };
        let mut spans = 0usize;
        let mut collections = 0usize;
        while !writer.is_finished() {
            spans += c.collect().unwrap().spans.len();
            collections += 1;
        }
        writer.join().unwrap();
        spans += c.collect().unwrap().spans.len();
        let st = sink.stats();
        assert_eq!(st.dropped_contended, 0, "a collector caused {} contended drops over {collections} collections", st.dropped_contended);
        assert_eq!(st.recorded, 30_000);
        assert_eq!(spans as u64 + sink.tap_dropped(), 20_000, "every kept record is a span or a counted tap drop");
    }

    /// #684's review, finding 3: a sink nothing drains is full after `max_records`, and later decisions have no span —
    /// the collector says how many, per call.
    #[test]
    fn a_full_sink_is_reported_not_silent() {
        let sink = Arc::new(DecisionSink::new(crate::decision::SinkConfig { max_records: 2, ..Default::default() }));
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 8).unwrap();
        for _ in 0..5 {
            let _ = sink.record(decision());
        }
        let got = c.collect().unwrap();
        assert_eq!((got.spans.len(), got.dropped.full), (2, 3));
        let _ = sink.record(decision());
        let again = c.collect().unwrap();
        assert_eq!((again.spans.len(), again.dropped.full), (0, 1), "since the last call, not cumulative");
    }

    /// #684's confirmation round, N1: under the default `SinkConfig` records of a few hundred bytes reach the 1 MiB
    /// byte bound before the 4096-record one, so a report of `Full` alone read 0 while thousands were dropped.
    #[test]
    fn every_drop_reason_is_reported_under_the_default_bounds() {
        let sink = Arc::new(DecisionSink::default());
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), crate::decision::MAX_TAP_CAPACITY).unwrap();
        let realistic = || {
            decision()
                .input(InputSnapshot::new("peer.load", "x".repeat(200), Provenance::Gossiped))
                .input(InputSnapshot::new("config.policy", "y".repeat(120), Provenance::Configured))
        };
        for _ in 0..6_000 {
            let _ = sink.record(realistic());
        }
        let got = c.collect().unwrap();
        assert!(got.dropped.bytes > 0, "the byte bound filled first: {:?}", got.dropped);
        assert_eq!(got.spans.len() as u64 + got.dropped.total() + got.tap_dropped, 6_000, "every decision is a span or a counted drop: {:?}", got.dropped);
    }

    /// N2: a collector that is dropped frees its sink for the next one, and a tap whose reader has gone costs nothing.
    #[test]
    fn a_dropped_collector_frees_the_tap_and_costs_nothing() {
        let sink = Arc::new(DecisionSink::default());
        let c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 4).unwrap();
        drop(c);
        for _ in 0..3 {
            sink.record(decision()).unwrap();
        }
        assert_eq!(sink.tap_dropped(), 0, "a tap with no reader is not sent to, so nothing is counted against it");
        let mut again = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 4).expect("re-tapped once the reader is gone");
        sink.record(decision()).unwrap();
        assert_eq!(again.collect().unwrap().spans.len(), 1);
    }

    /// #684's round 3, R1: a refused `tap()` used to take the tap's write lock, so a decision recorded meanwhile
    /// skipped its copy silently. Now a refusal never takes the write lock, and a skipped copy is counted.
    #[test]
    fn a_refused_tap_never_makes_a_kept_record_vanish() {
        let sink = Arc::new(DecisionSink::new(crate::decision::SinkConfig { max_records: 1 << 20, max_bytes: 1 << 30, max_input_bytes: 256 }));
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), crate::decision::MAX_TAP_CAPACITY).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let retrier = {
            let (sink, stop) = (Arc::clone(&sink), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut refused = 0u64;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if sink.tap(1).is_err() {
                        refused += 1;
                    }
                }
                refused
            })
        };
        let mut spans = 0u64;
        for i in 0..20_000 {
            sink.record(decision()).unwrap();
            if i % 1000 == 0 {
                spans += c.collect().unwrap().spans.len() as u64;
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(retrier.join().unwrap() > 0, "the retry loop ran against the live tap");
        spans += c.collect().unwrap().spans.len() as u64;
        assert_eq!(spans + sink.tap_dropped(), 20_000, "every kept record is a span or a counted tap drop");
    }

    /// N5 and N4: a capacity outside `1..=MAX_TAP_CAPACITY` is clamped, never a panic; the refusal is an error type.
    #[test]
    fn a_tap_capacity_is_clamped_and_its_refusal_is_an_error() {
        let sink = Arc::new(DecisionSink::default());
        let _a = SpanCollector::new().with_decision_sink(Arc::clone(&sink), usize::MAX).unwrap();
        let refused = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 0).unwrap_err();
        let boxed: Box<dyn std::error::Error> = Box::new(refused);
        assert!(boxed.to_string().contains("already tapped"));
        let other = DecisionSink::default();
        assert!(other.tap(0).is_ok(), "zero is clamped to one");
    }

    #[test]
    fn one_reader_per_tap_and_a_full_tap_is_counted() {
        let sink = Arc::new(DecisionSink::default());
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 2).unwrap();
        assert!(SpanCollector::new().with_decision_sink(Arc::clone(&sink), 2).is_err(), "a second collector is refused");
        for _ in 0..5 {
            sink.record(decision()).unwrap();
        }
        let got = c.collect().unwrap();
        assert_eq!((got.spans.len(), got.tap_dropped), (2, 3));
        assert_eq!(sink.snapshot().len(), 5, "the ring kept every record the tap could not");
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn the_collector_returns_each_record_once_and_never_drains_the_sink() {
        let dir = std::env::temp_dir().join(format!("mycelium-span-export-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("evidence.log");
        let _ = std::fs::remove_file(&path);
        let sink = Arc::new(DecisionSink::default());
        let mut c = SpanCollector::new().with_decision_sink(Arc::clone(&sink), 64).unwrap().with_evidence_journal(&path);
        // Nothing yet, and no journal file: empty, not an error.
        let first = c.collect().unwrap();
        assert!(first.spans.is_empty());

        sink.record(decision()).unwrap();
        let j = crate::EvidenceJournal::open(&path, crate::EvidenceProfile::Strict).unwrap();
        j.append(evidence_bytes("decided", "none")).await.unwrap();
        j.append(b"not evidence".to_vec()).await.unwrap();
        j.append(evidence_bytes("execution", "completed")).await.unwrap();

        let got = c.collect().unwrap();
        let sources: Vec<_> = got.spans.iter().map(|s| s.source).collect();
        assert_eq!(sources, [SpanSource::Decision, SpanSource::EvidenceDecided, SpanSource::EvidenceExecution]);
        assert_eq!(got.unparsed, 1);
        assert_eq!(sink.snapshot().len(), 1, "the collector reads the sink, it does not drain it");
        // The spans' hashes are the hashes `explain --closure` resolves over the same files.
        let frames = crate::read_evidence_journal(&path).unwrap();
        assert_eq!(got.spans[1].record_sha256, content_hash(&frames[0]), "the journal hash is the hash of the frame body");

        // Again: nothing new.
        assert!(c.collect().unwrap().spans.is_empty());
        sink.record(decision()).unwrap();
        let again = c.collect().unwrap();
        assert_eq!(again.spans.len(), 1);
        assert_eq!(again.spans[0].source, SpanSource::Decision);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The OTLP side, through a real `TracerProvider` and a capturing exporter: what reaches the exporter is exactly
/// [`RecordSpan::attributes`], with the derived ids and the record's time.
#[cfg(all(test, feature = "otel"))]
mod otlp_tests {
    use std::sync::{Arc, Mutex};

    use opentelemetry::trace::{SpanId, TraceId};
    use opentelemetry_sdk::export::trace::{ExportResult, SpanData, SpanExporter};
    use opentelemetry_sdk::trace::TracerProvider;

    use super::tests::decision;
    use super::*;

    #[derive(Debug, Clone, Default)]
    struct Capture(Arc<Mutex<Vec<SpanData>>>);

    impl SpanExporter for Capture {
        fn export(
            &mut self,
            batch: Vec<SpanData>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ExportResult> + Send + 'static>> {
            self.0.lock().unwrap_or_else(|p| p.into_inner()).extend(batch);
            Box::pin(async { Ok(()) })
        }
    }

    #[test]
    fn what_reaches_the_exporter_is_the_pinned_set_and_no_input() {
        let cap = Capture::default();
        let provider = TracerProvider::builder().with_simple_exporter(cap.clone()).build();
        let mut spans = vec![RecordSpan::from_decision(&decision())];
        #[cfg(all(feature = "gateway", feature = "tls"))]
        spans.push(RecordSpan::from_evidence(&super::tests::entry(super::tests::evidence_bytes("execution", "failed"))).unwrap());
        otlp::export(&provider, &spans);
        let got = cap.0.lock().unwrap().clone();
        assert_eq!(got.len(), spans.len());

        for (data, span) in got.iter().zip(spans.iter()) {
            assert_eq!(data.name, span.source.label());
            let attrs: Vec<(String, String)> =
                data.attributes.iter().map(|kv| (kv.key.to_string(), kv.value.to_string())).collect();
            let expected: Vec<(String, String)> =
                span.attributes().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
            assert_eq!(attrs, expected, "the exporter receives exactly the rendering, nothing added");
            let everything = format!("{data:?}");
            for secret in super::tests::SECRET {
                assert!(!everything.contains(secret), "{secret} reached the exporter: {everything}");
            }
            assert_eq!(data.span_context.trace_id(), TraceId::from_bytes(span.trace_id()));
            assert_eq!(data.span_context.span_id(), SpanId::from_bytes(span.span_id()));
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(span.at_ms.unwrap());
            assert_eq!((data.start_time, data.end_time), (t, t));
        }
        // The parent link is the record's parent; both records belong to run-1, so they are one trace.
        assert_eq!(got[0].parent_span_id, SpanId::from_bytes([0xab; 8]));
        #[cfg(all(feature = "gateway", feature = "tls"))]
        {
            assert_eq!(got[0].span_context.trace_id(), got[1].span_context.trace_id());
            assert_eq!(got[1].parent_span_id, SpanId::from_bytes([0xcd; 8]));
        }
    }
    /// Guide 19's snippet, compiled (#684's confirmation round, N4: as first written it did not compile).
    /// `the_guide_19_snippet_is_this_code` keeps the guide's block and this body the same, line for line.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[allow(dead_code)]
    fn guide_19_snippet(sink: Arc<DecisionSink>) -> Result<(), Box<dyn std::error::Error>> {
        // GUIDE-19-BEGIN
        let provider = otlp::provider("http://collector:4317", otlp::EXPORT_TIMEOUT)?;
        let mut spans = SpanCollector::new()
            .with_decision_sink(Arc::clone(&sink), 4096)?
            .with_evidence_journal("/var/lib/node/evidence.log");
        let got = spans.collect()?;
        otlp::export(&provider, &got.spans);
        // GUIDE-19-END
        Ok(())
    }

    /// The code lines of a Rust fence, comments and the `use` line removed, trimmed.
    fn code_lines(text: &str) -> Vec<String> {
        text.lines()
            .map(|l| l.split(" //").next().unwrap_or("").trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("use "))
            .collect()
    }

    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[test]
    fn the_guide_19_snippet_is_this_code() {
        let guide = include_str!("../docs/guide/19-replay-and-simulation.md");
        let section = &guide[guide.find("## Sending the records to a collector").expect("the section")..];
        let fence = section.split("```rust,ignore\n").nth(1).and_then(|f| f.split("```").next()).expect("the snippet");
        let src = include_str!("span_export.rs");
        let body = src.split("// GUIDE-19-BEGIN\n").nth(1).and_then(|b| b.split("// GUIDE-19-END").next()).expect("the markers");
        assert_eq!(code_lines(fence), code_lines(body), "guide 19's snippet and the compiled copy have drifted");
    }

    /// #684's review, finding 2: with the simple exporter, `emit` blocked the calling thread on every span — on a
    /// current-thread runtime against a refused port it never returned. Through the batch processor, a thousand spans
    /// and the provider's shutdown return within a bound.
    #[test]
    fn exporting_to_an_unreachable_collector_never_stalls_the_caller() {
        let port = crate::test_util::alloc_port();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async move {
                let provider = otlp::provider(&format!("http://127.0.0.1:{port}"), std::time::Duration::from_millis(500)).unwrap();
                let span = RecordSpan::from_decision(&decision());
                let started = std::time::Instant::now();
                for _ in 0..1000 {
                    otlp::emit(&provider, &span);
                }
                let emitted = started.elapsed();
                let _ = provider.shutdown();
                let _ = done_tx.send((emitted, started.elapsed()));
            });
        });
        let (emitted, total) = done_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("emit or shutdown stalled the caller against a refused port");
        assert!(emitted < std::time::Duration::from_secs(2), "a thousand emits took {emitted:?}");
        assert!(total < std::time::Duration::from_secs(10), "emit and shutdown took {total:?}");
    }
}

