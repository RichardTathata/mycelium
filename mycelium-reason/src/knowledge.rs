//! **The trace adapter** (v3 item 3 PR 7) — a fleet-reasoning trace event as a knowledge-layer
//! **observation**, §6 of `docs/design/knowledge-layer.md`:
//!
//! > `TraceEvent { hlc, node, kind, detail }` has **no parent link**. Its "causal story" is HLC
//! > *adjacency*, which is an ordering, not a cause. The adapter therefore **adds `derived_from`
//! > links explicitly**. Treating adjacency as derivation would manufacture causal claims out of
//! > timestamps […] and it would be invisible afterwards because the resulting graph would look
//! > identical to one someone asserted.
//!
//! # The rule is structural, not a convention
//!
//! There are two entry points, and between them there is no way to get an inferred link:
//!
//! - [`observation`] converts **one** event and takes `derived_from` as an argument. A link exists
//!   only because the caller asserted it.
//! - [`observations`] converts a **run** and emits records with **no links at all**. It has no
//!   parameter through which derivation could be supplied, so it cannot invent any.
//!
//! A future helper that "helpfully" chained a run by HLC order would be exactly the thing §6
//! forbids, and it would produce a graph indistinguishable from an honest one. That is why the
//! batch converter is link-free by construction rather than by comment.
//!
//! # Why an observation, and not a claim or an assessment
//!
//! A trace is a **report of what happened** — a routing decision, a fallback, a completion. It is
//! not a judgement (an assessment has an author who *judged*), and it is not a self-description (a
//! claim). §1 split the record types so that a report cannot be counted as evidence, and
//! `knowledge::resolution` enforces that split: an observation never counts toward a verdict.
//!
//! # Determinism
//!
//! A record's id is a digest over its canonical bytes, so the body must serialise identically on
//! every node. `serde_json::Value::Object` is a `BTreeMap` in this workspace — no crate enables
//! `preserve_order` — so `to_vec` is key-sorted. Cargo unions features across the graph, so a
//! dependency enabling it later would silently break this; [the test that pins
//! it](self#tests) exists for that reason.

use mycelium::hlc::physical_ms;
use mycelium::knowledge::{IssuerId, KnowledgeRecord, Link, LinkKind, RecordError, RecordId, RecordKind};
use serde_json::json;

use crate::TraceEvent;

/// Why a trace event could not become a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceAdapterError {
    /// The event's `node` is empty, so there is no issuer to attribute the observation to.
    EmptyNode,
    /// The record itself was malformed.
    Record(RecordError),
}

impl std::fmt::Display for TraceAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyNode => f.write_str("trace event has an empty node; no issuer to attribute it to"),
            Self::Record(e) => write!(f, "record: {e:?}"),
        }
    }
}

impl From<RecordError> for TraceAdapterError {
    fn from(e: RecordError) -> Self {
        Self::Record(e)
    }
}

/// The record body: the event, whole. The full packed HLC is kept (not only its millisecond) so two
/// events in the same millisecond remain distinct records.
fn body(event: &TraceEvent) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "hlc":    event.hlc,
        "kind":   event.kind,
        "detail": event.detail,
    }))
    .unwrap_or_default()
}

/// **One trace event as an observation**, with derivation **as the caller asserts it**.
///
/// - issuer: the node that recorded the event;
/// - `at_ms`: the HLC's physical millisecond;
/// - `derived_from`: exactly the records the caller says this was derived from — no more.
///
/// Pass an empty slice and the record has no links. Nothing here consults the HLC to decide what
/// this event came from.
pub fn observation(
    event: &TraceEvent,
    subject: &str,
    derived_from: &[RecordId],
) -> Result<KnowledgeRecord, TraceAdapterError> {
    let issuer = IssuerId::new(&event.node).ok_or(TraceAdapterError::EmptyNode)?;
    let links = derived_from
        .iter()
        .map(|target| Link { kind: LinkKind::DerivedFrom, target: target.clone() })
        .collect();
    Ok(KnowledgeRecord::new(
        issuer,
        RecordKind::Observation,
        physical_ms(event.hlc),
        subject,
        body(event),
        links,
    )?)
}

/// **A whole run as observations — with no links.**
///
/// This is the batch form of [`observation`] with derivation deliberately absent: there is no
/// argument through which links could be supplied, so adjacent events come out unrelated. If two
/// of them *are* related, the caller says so with [`observation`], record by record.
///
/// Events that cannot be converted (an empty `node`) are skipped; the count of skipped events is
/// returned beside the records so a caller can tell a short list from a lossless one.
pub fn observations(events: &[TraceEvent], subject: &str) -> (Vec<KnowledgeRecord>, usize) {
    let mut out = Vec::with_capacity(events.len());
    let mut skipped = 0usize;
    for event in events {
        match observation(event, subject, &[]) {
            Ok(r) => out.push(r),
            Err(_) => skipped += 1,
        }
    }
    (out, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycelium::hlc::pack;
    use serde_json::{Map, Value};

    fn ev(hlc: u64, node: &str, kind: &str, detail: Value) -> TraceEvent {
        TraceEvent { hlc, node: node.into(), kind: kind.into(), detail }
    }

    /// **§6, the decisive test.** Two events one HLC tick apart — as adjacent as events get —
    /// produce two observations with **no link between them**. Adjacency is not derivation.
    #[test]
    fn adjacent_events_produce_no_link() {
        let first = ev(pack(1_000, 0), "node-a", "route", json!({"model": "m"}));
        let second = ev(pack(1_000, 1), "node-a", "complete", json!({"ok": true}));

        let (records, skipped) = observations(&[first, second], "run/1");
        assert_eq!(skipped, 0);
        assert_eq!(records.len(), 2);
        for r in &records {
            assert!(r.links().is_empty(), "no link may be manufactured from HLC order: {r:?}");
        }
    }

    /// The **only** way a link appears is that the caller asserted it, and then it is exactly the
    /// asserted one.
    #[test]
    fn a_link_exists_only_because_the_caller_asserted_it() {
        let first = ev(pack(1_000, 0), "node-a", "route", json!({}));
        let base = observation(&first, "run/1", &[]).expect("converts");

        let second = ev(pack(1_000, 1), "node-a", "complete", json!({}));
        let derived = observation(&second, "run/1", &[base.id().clone()]).expect("converts");

        assert_eq!(derived.links().len(), 1);
        assert_eq!(derived.links()[0].kind, LinkKind::DerivedFrom);
        assert_eq!(&derived.links()[0].target, base.id());
    }

    /// A trace is a **report**, so it is an observation — the kind that can never be evidence.
    #[test]
    fn a_trace_event_is_an_observation_not_an_assessment() {
        let r = observation(&ev(pack(5, 0), "node-a", "route", json!({})), "s", &[]).expect("converts");
        assert_eq!(r.kind(), RecordKind::Observation);
        assert_eq!(r.issuer().as_str(), "node-a", "attributed to the node that recorded it");
        assert_eq!(r.at_ms(), 5, "the HLC's physical millisecond");
    }

    /// Two events in the same millisecond are distinct records: the full HLC is in the body.
    #[test]
    fn same_millisecond_events_are_distinct_records() {
        let a = observation(&ev(pack(7, 0), "n", "k", json!({})), "s", &[]).expect("converts");
        let b = observation(&ev(pack(7, 1), "n", "k", json!({})), "s", &[]).expect("converts");
        assert_ne!(a.id(), b.id());
        assert_eq!(a.at_ms(), b.at_ms());
    }

    /// **Pins body determinism.** The same detail built in two insertion orders yields one id. If any
    /// crate in the graph ever enables `serde_json/preserve_order`, this is the test that fails.
    #[test]
    fn the_record_id_does_not_depend_on_key_insertion_order() {
        let mut ab = Map::new();
        ab.insert("a".into(), json!(1));
        ab.insert("b".into(), json!(2));
        let mut ba = Map::new();
        ba.insert("b".into(), json!(2));
        ba.insert("a".into(), json!(1));

        let x = observation(&ev(pack(1, 0), "n", "k", Value::Object(ab)), "s", &[]).expect("converts");
        let y = observation(&ev(pack(1, 0), "n", "k", Value::Object(ba)), "s", &[]).expect("converts");
        assert_eq!(x.id(), y.id(), "canonical bytes must not depend on insertion order");
    }

    /// An event with no node has no issuer, and is refused rather than attributed to nobody.
    #[test]
    fn an_event_with_an_empty_node_is_refused() {
        let r = observation(&ev(pack(1, 0), "", "k", json!({})), "s", &[]);
        assert_eq!(r.unwrap_err(), TraceAdapterError::EmptyNode);

        let (records, skipped) = observations(&[ev(pack(1, 0), "", "k", json!({}))], "s");
        assert!(records.is_empty());
        assert_eq!(skipped, 1, "the batch form counts what it dropped");
    }
}
