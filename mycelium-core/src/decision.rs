//! The **decision trace** core — `docs/plans/guarantees-and-rule-catalogue.md`, increment I4 (G7, G8).
//!
//! A [`DecisionRecord`] says what one evaluation of a catalogued rule ([`crate::rule`]) decided, from
//! values the code **already produced**: the rule and its revision, the build and configuration the node
//! ran under, the node and its incarnation, a local sequence number, the target, the trigger and the
//! record it followed from *when known*, the inputs it read (bounded, with age and provenance), how
//! confident its view was, the outcome and its typed reason, a reference to the effect, and what this
//! record could not say. A [`DecisionSink`] keeps a bounded number of them without ever blocking the
//! decision that produced one.
//!
//! # The trace changes no decision (G7)
//!
//! This module is a leaf: it reads no clock, draws no random number, touches no network, advances no
//! HLC and makes no authority decision. A record's `at_ms` is the time the decision point already held
//! (the HLC or decision clock it used); the sink assigns only its own sequence number from a local
//! counter. The sink's one lock is its own, taken with `try_lock` — a contended sink **drops** the
//! record and counts the drop rather than waiting, so a decision point never serialises under it, and a
//! decision point must hold **no subsystem lock** while it records (the record is built from values
//! copied out of the decision, after the lock is released). The sink is bounded by count **and** by
//! bytes; at either bound the newest record is dropped and counted, never the oldest evicted, so a trace
//! is a prefix of what happened plus the number of records it did not keep.
//!
//! # Coverage is stated, never implied (G8)
//!
//! A rule is instrumented, catalogue-only or unsupported (`rule::TracePolicy`); a sink that was never
//! attached is not an empty trace. A replay bundle carries the trace as [`DECISION_ATTACHMENT`]; a
//! bundle without it means *trace unavailable*, never *no decisions*. The [`SinkStats`] travel with the
//! records so a reader knows how many were dropped and how many inputs were cut.
//!
//! # Off by default
//!
//! Nothing here runs unless a decision point is handed an `Arc<DecisionSink>`; the pilot (I5) is the
//! first to take one. Bytes are an **estimate** — the retained text plus a fixed overhead per record and
//! per input ([`RECORD_OVERHEAD_BYTES`], [`INPUT_OVERHEAD_BYTES`]) — so the bound is on what the sink
//! holds, not on the size of a serialised export, which adds its own framing.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;

pub use crate::rule::OutcomeKind;

/// The file name a replay bundle gives the decision trace (one JSON record per line). Absent from a
/// bundle ⇒ *trace unavailable*, never *no decisions* (G8).
pub const DECISION_ATTACHMENT: &str = "decisions.jsonl";

/// The schema id of a serialised record; a change to the record's shape is a new one.
pub const RECORD_SCHEMA: &str = "mycelium.decision/1";

/// Fixed per-record cost in the byte estimate, beside the text it carries.
pub const RECORD_OVERHEAD_BYTES: usize = 96;
/// Fixed per-input cost in the byte estimate.
pub const INPUT_OVERHEAD_BYTES: usize = 32;

/// How confident the view a decision read was — stated by the decision point from what it already
/// knew (a governor's `ViewConfidence`, a gossiped read's staleness), never computed here.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewStatus {
    /// The decision point judged its view fresh enough to act on.
    Confident,
    /// The view was older than the decision point's own bound.
    Stale,
    /// The view was known to be missing peers or entries.
    Partial,
    /// The decision point does not assess its view.
    Unknown,
}

/// Where an input's value came from, as the decision point knows it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// This node's own state, read atomically.
    Local,
    /// A gossiped value: another node wrote it, anti-entropy carried it, it may be stale.
    Gossiped,
    /// Configuration, read at start.
    Configured,
    /// A value a caller supplied with the trigger (a request body, a frame).
    Supplied,
    /// Sampled from the host (a resource probe) at the decision.
    Sampled,
}

/// One input a decision read, as the decision point already had it. `value` is a bounded rendering,
/// cut at [`SinkConfig::max_input_bytes`] and flagged when it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InputSnapshot {
    /// The source named in the rule's descriptor (`Input::source`), so a reader can join them.
    pub source: String,
    /// A rendering of what was read — a key, a count, a hash, a short literal; never a payload.
    pub value: String,
    /// How old the value was at the decision, if the decision point knows (from the HLC it already read).
    pub age_ms: Option<u64>,
    pub provenance: Provenance,
    /// `true` when `value` was cut to the sink's bound.
    pub truncated: bool,
}

impl InputSnapshot {
    pub fn new(source: impl Into<String>, value: impl Into<String>, provenance: Provenance) -> Self {
        InputSnapshot { source: source.into(), value: value.into(), age_ms: None, provenance, truncated: false }
    }
    pub fn aged(mut self, age_ms: u64) -> Self { self.age_ms = Some(age_ms); self }
}

/// What a record could not say. A reader treats every `true` as *unknown*, never as *none*.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Completeness {
    /// At least one input's value was cut to the bound.
    pub inputs_truncated: bool,
    /// Inputs the decision point chose not to render (`inputs_dropped` of them), for size.
    pub inputs_dropped: u32,
    /// The decision followed from a record this node did not keep or could not identify.
    pub parent_unknown: bool,
    /// The decision's effect could not be referenced (it had not happened yet, or has no id).
    pub effect_unreferenced: bool,
}

/// The profile the node ran under when it decided (G12): both the name and the revision, so a reader
/// knows which guarantee set was required of the node that decided.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProfileStamp {
    pub name: String,
    pub revision: u32,
}

/// One evaluation of a catalogued rule, as it already happened.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DecisionRecord {
    pub schema: &'static str,
    /// The rule's stable id and semantic revision (`rule::RuleDescriptor::{id, revision}`).
    pub rule: String,
    pub rule_revision: u32,
    /// A digest or version of the build that decided (the caller's `CARGO_PKG_VERSION`, a git commit).
    pub build: String,
    /// The configuration digest the startup report was computed over (G13), when the node has one.
    pub config_digest: Option<String>,
    pub profile: Option<ProfileStamp>,
    /// The deciding node, as it names itself.
    pub node: String,
    /// The node's incarnation: a start counter or a start time the node already holds.
    pub incarnation: u64,
    /// Assigned by the sink: strictly increasing per sink, gaps where records were dropped.
    pub seq: u64,
    /// The time the decision point already held when it decided (its HLC or decision clock, ms).
    pub at_ms: Option<u64>,
    /// What the decision was about: a capability, a key, a peer, a request id.
    pub target: Option<String>,
    /// What caused the evaluation — the descriptor's `trigger`, instantiated (a round number, a frame kind).
    pub trigger: String,
    /// The `seq` of the record this one followed from, when the decision point knows it.
    pub parent_seq: Option<u64>,
    pub inputs: Vec<InputSnapshot>,
    pub view: ViewStatus,
    pub outcome: OutcomeKind,
    /// One of the descriptor's typed reason codes for `outcome` — never free text.
    pub reason: String,
    /// A reference to what the decision caused: a key written, an install token, a frame id.
    pub effect: Option<String>,
    pub completeness: Completeness,
}

impl DecisionRecord {
    /// A record with every field a decision point must supply; the rest default to *unknown* and
    /// are set by the builder methods. `seq` is assigned when the sink keeps it.
    pub fn new(rule: &str, rule_revision: u32, trigger: impl Into<String>, outcome: OutcomeKind, reason: &str) -> Self {
        DecisionRecord {
            schema: RECORD_SCHEMA,
            rule: rule.into(),
            rule_revision,
            build: String::new(),
            config_digest: None,
            profile: None,
            node: String::new(),
            incarnation: 0,
            seq: 0,
            at_ms: None,
            target: None,
            trigger: trigger.into(),
            parent_seq: None,
            inputs: Vec::new(),
            view: ViewStatus::Unknown,
            outcome,
            reason: reason.into(),
            effect: None,
            completeness: Completeness { parent_unknown: true, effect_unreferenced: true, ..Default::default() },
        }
    }

    pub fn node(mut self, node: impl Into<String>, incarnation: u64) -> Self { self.node = node.into(); self.incarnation = incarnation; self }
    pub fn build(mut self, build: impl Into<String>) -> Self { self.build = build.into(); self }
    pub fn config_digest(mut self, digest: impl Into<String>) -> Self { self.config_digest = Some(digest.into()); self }
    pub fn profile(mut self, name: impl Into<String>, revision: u32) -> Self { self.profile = Some(ProfileStamp { name: name.into(), revision }); self }
    /// The time the decision point already held — never read here.
    pub fn at(mut self, at_ms: u64) -> Self { self.at_ms = Some(at_ms); self }
    pub fn target(mut self, target: impl Into<String>) -> Self { self.target = Some(target.into()); self }
    pub fn parent(mut self, seq: u64) -> Self { self.parent_seq = Some(seq); self.completeness.parent_unknown = false; self }
    /// A decision with no parent by nature (a tick, a first trigger) — distinct from one not kept.
    pub fn no_parent(mut self) -> Self { self.completeness.parent_unknown = false; self }
    pub fn input(mut self, input: InputSnapshot) -> Self { self.inputs.push(input); self }
    pub fn inputs_dropped(mut self, n: u32) -> Self { self.completeness.inputs_dropped = n; self }
    pub fn view(mut self, view: ViewStatus) -> Self { self.view = view; self }
    pub fn effect(mut self, effect: impl Into<String>) -> Self { self.effect = Some(effect.into()); self.completeness.effect_unreferenced = false; self }
    /// A decision with no effect by nature (a refusal, no action) — distinct from one unreferenced.
    pub fn no_effect(mut self) -> Self { self.completeness.effect_unreferenced = false; self }

    /// The sink's byte estimate for this record: its text plus the fixed overheads.
    pub fn approx_bytes(&self) -> usize {
        RECORD_OVERHEAD_BYTES
            + self.rule.len() + self.build.len() + self.node.len() + self.trigger.len() + self.reason.len()
            + self.config_digest.as_ref().map_or(0, String::len)
            + self.profile.as_ref().map_or(0, |p| p.name.len())
            + self.target.as_ref().map_or(0, String::len)
            + self.effect.as_ref().map_or(0, String::len)
            + self.inputs.iter().map(|i| INPUT_OVERHEAD_BYTES + i.source.len() + i.value.len()).sum::<usize>()
    }
}

/// The sink's bounds. Defaults: 4096 records, 1 MiB, 256 bytes per input value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinkConfig {
    pub max_records: usize,
    pub max_bytes: usize,
    pub max_input_bytes: usize,
}

impl Default for SinkConfig {
    fn default() -> Self { SinkConfig { max_records: 4096, max_bytes: 1 << 20, max_input_bytes: 256 } }
}

/// Why the sink did not keep a record. Every variant is counted in [`SinkStats`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The record count bound was reached.
    Full,
    /// The byte bound would be exceeded.
    Bytes,
    /// Another holder had the sink's lock; the record was not waited for.
    Contended,
    /// One record larger than the whole byte bound.
    Oversized,
}

/// The sink's counters — they travel with an export so a reader knows what the trace is missing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SinkStats {
    pub recorded: u64,
    pub dropped_full: u64,
    pub dropped_bytes: u64,
    pub dropped_contended: u64,
    pub dropped_oversized: u64,
    /// Input values cut to the bound, across all kept records.
    pub inputs_truncated: u64,
    /// Records currently held.
    pub held: usize,
    pub held_bytes: usize,
}

struct Ring {
    records: VecDeque<DecisionRecord>,
    bytes: usize,
}

/// A bounded, nonblocking store of decision records (lock-order row 53). One per node, or one per
/// recording; shared as `Arc<DecisionSink>` with every decision point that is instrumented.
pub struct DecisionSink {
    cfg: SinkConfig,
    ring: Mutex<Ring>,
    seq: AtomicU64,
    recorded: AtomicU64,
    dropped_full: AtomicU64,
    dropped_bytes: AtomicU64,
    dropped_contended: AtomicU64,
    dropped_oversized: AtomicU64,
    inputs_truncated: AtomicU64,
}

impl std::fmt::Debug for DecisionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecisionSink").field("cfg", &self.cfg).field("stats", &self.stats()).finish()
    }
}

impl Default for DecisionSink {
    fn default() -> Self { Self::new(SinkConfig::default()) }
}

impl DecisionSink {
    pub fn new(cfg: SinkConfig) -> Self {
        DecisionSink {
            cfg,
            ring: Mutex::new(Ring { records: VecDeque::new(), bytes: 0 }),
            seq: AtomicU64::new(0),
            recorded: AtomicU64::new(0),
            dropped_full: AtomicU64::new(0),
            dropped_bytes: AtomicU64::new(0),
            dropped_contended: AtomicU64::new(0),
            dropped_oversized: AtomicU64::new(0),
            inputs_truncated: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> SinkConfig { self.cfg }

    /// Keep a record, or say why not — without waiting. Input values are cut to the bound here, so a
    /// decision point hands over what it has and the sink keeps it affordable. Returns the sequence
    /// number the record was kept under, which a later record may cite as its parent.
    pub fn record(&self, mut rec: DecisionRecord) -> Result<u64, Dropped> {
        let mut cut = 0u64;
        for i in &mut rec.inputs {
            if i.value.len() > self.cfg.max_input_bytes {
                let mut end = self.cfg.max_input_bytes;
                while !i.value.is_char_boundary(end) { end -= 1; }
                i.value.truncate(end);
                i.truncated = true;
                cut += 1;
            }
        }
        if cut > 0 { rec.completeness.inputs_truncated = true; }
        let size = rec.approx_bytes();
        if size > self.cfg.max_bytes {
            self.dropped_oversized.fetch_add(1, Ordering::Relaxed);
            return Err(Dropped::Oversized);
        }
        let Ok(mut ring) = self.ring.try_lock() else {
            self.dropped_contended.fetch_add(1, Ordering::Relaxed);
            return Err(Dropped::Contended);
        };
        if ring.records.len() >= self.cfg.max_records {
            self.dropped_full.fetch_add(1, Ordering::Relaxed);
            return Err(Dropped::Full);
        }
        if ring.bytes + size > self.cfg.max_bytes {
            self.dropped_bytes.fetch_add(1, Ordering::Relaxed);
            return Err(Dropped::Bytes);
        }
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        rec.seq = seq;
        ring.bytes += size;
        ring.records.push_back(rec);
        drop(ring);
        self.recorded.fetch_add(1, Ordering::Relaxed);
        self.inputs_truncated.fetch_add(cut, Ordering::Relaxed);
        Ok(seq)
    }

    /// Take every held record, oldest first, leaving the sink empty; the counters are kept so an
    /// export can say what the trace is missing. This one waits for the lock — an exporter is not a
    /// decision point.
    pub fn drain(&self) -> Vec<DecisionRecord> {
        let mut ring = self.ring.lock().unwrap_or_else(|p| p.into_inner());
        ring.bytes = 0;
        ring.records.drain(..).collect()
    }

    /// A copy of every held record, oldest first.
    pub fn snapshot(&self) -> Vec<DecisionRecord> {
        let ring = self.ring.lock().unwrap_or_else(|p| p.into_inner());
        ring.records.iter().cloned().collect()
    }

    pub fn stats(&self) -> SinkStats {
        let (held, held_bytes) = match self.ring.try_lock() {
            Ok(r) => (r.records.len(), r.bytes),
            Err(_) => (0, 0),
        };
        SinkStats {
            recorded: self.recorded.load(Ordering::Relaxed),
            dropped_full: self.dropped_full.load(Ordering::Relaxed),
            dropped_bytes: self.dropped_bytes.load(Ordering::Relaxed),
            dropped_contended: self.dropped_contended.load(Ordering::Relaxed),
            dropped_oversized: self.dropped_oversized.load(Ordering::Relaxed),
            inputs_truncated: self.inputs_truncated.load(Ordering::Relaxed),
            held,
            held_bytes,
        }
    }

    /// The counters as a JSON object, to write beside an export.
    pub fn stats_json(&self) -> String {
        serde_json::to_string_pretty(&self.stats()).unwrap_or_else(|_| "{}".into())
    }

    /// Every record dropped, whatever the reason.
    pub fn dropped(&self) -> u64 {
        let s = self.stats();
        s.dropped_full + s.dropped_bytes + s.dropped_contended + s.dropped_oversized
    }

    /// The held records as one JSON record per line, the shape [`DECISION_ATTACHMENT`] carries; the
    /// sink is left as it was. The stats are not in the lines — an exporter writes them beside.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for r in self.snapshot() {
            if let Ok(line) = serde_json::to_string(&r) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }
}

/// The canonical decision tuple (plan I6): what a replay must reproduce — the rule and revision, the
/// trigger, the target, the outcome and its reason. Not the sequence number, the stamps or the inputs'
/// rendering, which may legitimately differ between a recording and its replay.
pub fn canonical(r: &DecisionRecord) -> String {
    format!("{}@{} | {} | {} | {:?} | {}", r.rule, r.rule_revision, r.trigger, r.target.as_deref().unwrap_or("-"), r.outcome, r.reason)
}

/// Can a replay be expected to reproduce this decision? Only when every input it read is one the
/// seams cover (local state, configuration, a supplied value). A gossiped or sampled input arrives
/// from outside the recording's seams, so a difference there is **unattributable** — the trace cannot
/// say whether the decision or its world changed.
pub fn attributable(r: &DecisionRecord) -> bool {
    r.inputs.iter().all(|i| !matches!(i.provenance, Provenance::Gossiped | Provenance::Sampled))
}

/// One position where two traces differ.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Divergence {
    /// Index into both traces (the first trace's record is `left`, the second's `right`; `None` where
    /// one trace has ended).
    pub index: usize,
    pub left: Option<String>,
    pub right: Option<String>,
    /// `true` when both records read only seam-covered inputs, so the difference is the decision's.
    pub attributable: bool,
}

/// The result of comparing two traces position by position on their canonical tuples.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub compared: usize,
    pub divergences: Vec<Divergence>,
    /// Divergences whose records read inputs outside the seams — reported, never counted as a
    /// reproduction failure.
    pub unattributable: usize,
}

impl Comparison {
    /// The traces agree on every attributable decision (an unattributable divergence is stated, not
    /// counted).
    pub fn reproduces(&self) -> bool {
        self.divergences.iter().all(|d| !d.attributable)
    }
}

/// Compare two traces deterministically: position by position on [`canonical`] tuples, each divergence
/// marked attributable or not by [`attributable`].
pub fn compare(left: &[DecisionRecord], right: &[DecisionRecord]) -> Comparison {
    let n = left.len().max(right.len());
    let mut c = Comparison { compared: n, ..Default::default() };
    for i in 0..n {
        let (l, r) = (left.get(i), right.get(i));
        let (lc, rc) = (l.map(canonical), r.map(canonical));
        if lc != rc {
            let attributable = l.is_none_or(attributable) && r.is_none_or(attributable);
            if !attributable { c.unattributable += 1; }
            c.divergences.push(Divergence { index: i, left: lc, right: rc, attributable });
        }
    }
    c
}

/// The coverage manifest (plan I6, G8): which rules a trace could have recorded, by trace policy, so a
/// reader of a bundle knows that an absent rule is *not recorded here*, never *did not decide*.
pub fn coverage_manifest(rules: &[&crate::rule::RuleDescriptor]) -> String {
    #[derive(Serialize)]
    struct Row<'a> { id: &'a str, revision: u32, trace: crate::rule::TracePolicy }
    #[derive(Serialize)]
    struct Manifest<'a> { schema: &'static str, record_schema: &'static str, rules: Vec<Row<'a>> }
    let rows = rules.iter().map(|r| Row { id: r.id, revision: r.revision, trace: r.trace }).collect();
    serde_json::to_string_pretty(&Manifest { schema: "mycelium.coverage/1", record_schema: RECORD_SCHEMA, rules: rows }).unwrap_or_else(|_| "{}".into())
}

/// Render a decision trace (one JSON record per line, as [`DecisionSink::to_jsonl`] writes it) as what
/// each target went through, in order: the round, the rule, how it ended and why, what it read and
/// what it caused. With a catalogue document (`docs/reference/rule-catalogue.json`), each rule used is
/// summarised once at the end. The footer says what the trace cannot: a dropped record is the sink's
/// counter, not a line here; a decision that read gossiped or sampled inputs is unattributable under
/// replay. Lines that do not parse are counted, never silently skipped. `target` narrows the output.
pub fn explain(jsonl: &str, catalogue_json: Option<&str>, target: Option<&str>) -> String {
    use std::collections::BTreeMap;
    let mut records: Vec<serde_json::Value> = Vec::new();
    let mut unparsed = 0usize;
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => records.push(v),
            Err(_) => unparsed += 1,
        }
    }
    let summaries: BTreeMap<String, String> = catalogue_json
        .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok())
        .and_then(|c| c.get("rules").and_then(|r| r.as_array()).cloned())
        .map(|rules| rules.iter().filter_map(|r| Some((r.get("id")?.as_str()?.to_string(), r.get("summary")?.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let str_of = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("-").to_string();

    let mut order: Vec<String> = Vec::new();
    let mut by_target: BTreeMap<String, Vec<&serde_json::Value>> = BTreeMap::new();
    for r in &records {
        let t = str_of(r, "target");
        if let Some(want) = target && t != want { continue; }
        if !by_target.contains_key(&t) { order.push(t.clone()); }
        by_target.entry(t).or_default().push(r);
    }
    let mut out = String::new();
    let mut rules_used: BTreeMap<String, ()> = BTreeMap::new();
    let (mut unattributable, mut truncated) = (0usize, 0usize);
    for t in &order {
        out.push_str(&format!("{t}\n"));
        for r in &by_target[t] {
            let rule = str_of(r, "rule");
            rules_used.insert(rule.clone(), ());
            let outcome = str_of(r, "outcome");
            let reason = str_of(r, "reason");
            let trigger = str_of(r, "trigger");
            let mut line = format!("  {trigger} · {rule} → {outcome} {reason}");
            if let Some(effect) = r.get("effect").and_then(|e| e.as_str()) {
                line.push_str(&format!(" ⇒ {effect}"));
            }
            if let Some(inputs) = r.get("inputs").and_then(|i| i.as_array()) && !inputs.is_empty() {
                let reads: Vec<String> = inputs.iter().map(|i| {
                    let v = i.get("value").and_then(|x| x.as_str()).unwrap_or("-");
                    let cut = if i.get("truncated").and_then(|x| x.as_bool()).unwrap_or(false) { "…" } else { "" };
                    format!("{v}{cut}")
                }).collect();
                line.push_str(&format!(" (reads: {})", reads.join("; ")));
                if inputs.iter().any(|i| matches!(i.get("provenance").and_then(|p| p.as_str()), Some("gossiped" | "sampled"))) {
                    unattributable += 1;
                }
            }
            if r.pointer("/completeness/inputs_truncated").and_then(|x| x.as_bool()).unwrap_or(false) {
                truncated += 1;
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    if !summaries.is_empty() && !rules_used.is_empty() {
        out.push_str("\nRules:\n");
        for rule in rules_used.keys() {
            if let Some(sum) = summaries.get(rule) {
                out.push_str(&format!("  {rule} — {sum}\n"));
            }
        }
    }
    let shown: usize = by_target.values().map(Vec::len).sum();
    out.push_str(&format!(
        "\n{shown} record(s) over {} target(s); {unattributable} read gossiped or sampled inputs (unattributable under replay); {truncated} with truncated inputs; {unparsed} line(s) did not parse. \
Records the sink dropped are in its counters (decisions.stats.json), not here.\n",
        order.len()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn rec(reason: &str) -> DecisionRecord {
        DecisionRecord::new("prov.self_election", 1, "round 7", OutcomeKind::Deferral, reason)
            .node("n1", 3).build("2.19.0").at(1_700_000_000_000).target("data/pack").no_parent().no_effect()
    }

    /// Records are kept in order with strictly increasing sequence numbers; a drained sink is empty and
    /// its counters survive the drain.
    #[test]
    fn records_are_sequenced_and_drained_in_order() {
        let s = DecisionSink::default();
        assert_eq!(s.record(rec("declined")), Ok(1));
        assert_eq!(s.record(rec("declined").parent(1)), Ok(2));
        let got = s.drain();
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(got[1].parent_seq, Some(1));
        assert!(!got[1].completeness.parent_unknown && !got[0].completeness.parent_unknown);
        assert!(s.drain().is_empty());
        assert_eq!(s.stats().recorded, 2);
        assert_eq!(s.record(rec("declined")), Ok(3), "the sequence continues across a drain");
    }

    /// At the count bound the newest record is dropped and counted; the kept ones are a prefix.
    #[test]
    fn the_count_bound_drops_the_newest_and_counts_it() {
        let s = DecisionSink::new(SinkConfig { max_records: 2, ..Default::default() });
        assert!(s.record(rec("a")).is_ok() && s.record(rec("b")).is_ok());
        assert_eq!(s.record(rec("c")), Err(Dropped::Full));
        let st = s.stats();
        assert_eq!((st.recorded, st.dropped_full, st.held), (2, 1, 2));
        assert_eq!(s.snapshot().iter().map(|r| r.reason.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(s.dropped(), 1);
    }

    /// The byte bound holds on the sink's own estimate; an oversized single record is refused by name.
    #[test]
    fn the_byte_bound_holds_and_an_oversized_record_is_named() {
        let one = rec("x").approx_bytes();
        let s = DecisionSink::new(SinkConfig { max_bytes: one * 2 + 1, max_records: 100, ..Default::default() });
        assert!(s.record(rec("x")).is_ok() && s.record(rec("x")).is_ok());
        assert_eq!(s.record(rec("x")), Err(Dropped::Bytes));
        assert!(s.stats().held_bytes <= one * 2 + 1);
        let big = DecisionSink::new(SinkConfig { max_bytes: 8, ..Default::default() });
        assert_eq!(big.record(rec("x")), Err(Dropped::Oversized));
        assert_eq!(big.stats().dropped_oversized, 1);
    }

    /// An input value is cut at a character boundary, flagged on the input and the record, and counted.
    #[test]
    fn an_input_is_cut_at_a_char_boundary_and_flagged() {
        let s = DecisionSink::new(SinkConfig { max_input_bytes: 5, ..Default::default() });
        let r = rec("x").input(InputSnapshot::new("demand/", "ab€cd€", Provenance::Gossiped).aged(40));
        s.record(r).unwrap();
        let got = s.drain().remove(0);
        assert_eq!(got.inputs[0].value, "ab€", "5 bytes would split the second € (bytes 5..8)");
        assert!(got.inputs[0].truncated && got.completeness.inputs_truncated);
        assert_eq!(got.inputs[0].age_ms, Some(40));
        assert_eq!(s.stats().inputs_truncated, 1);
    }

    /// A contended sink drops rather than waits: with the lock held elsewhere, `record` returns at once.
    #[test]
    fn a_contended_sink_drops_instead_of_blocking() {
        let s = Arc::new(DecisionSink::default());
        let guard = s.ring.lock().unwrap();
        let s2 = Arc::clone(&s);
        let t = std::thread::spawn(move || s2.record(rec("x")));
        let r = t.join().unwrap();
        drop(guard);
        assert_eq!(r, Err(Dropped::Contended));
        assert_eq!(s.stats().dropped_contended, 1);
        assert!(s.record(rec("x")).is_ok(), "and the next one is kept");
    }

    /// The export is one JSON object per line carrying the schema, and a new record states what it does
    /// not know rather than implying none.
    #[test]
    fn the_export_is_jsonl_and_unknowns_are_stated() {
        let s = DecisionSink::default();
        s.record(DecisionRecord::new("prov.install", 1, "admitted", OutcomeKind::Action, "completed")).unwrap();
        let line = s.to_jsonl();
        assert_eq!(line.lines().count(), 1);
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["schema"], RECORD_SCHEMA);
        assert_eq!(v["completeness"]["parent_unknown"], true);
        assert_eq!(v["completeness"]["effect_unreferenced"], true);
        assert_eq!(v["view"], "unknown");
        assert_eq!(v["outcome"], "action");
    }

    /// Two traces that decided the same compare equal whatever their sequence numbers and stamps; a
    /// different reason is an attributable divergence when the inputs were seam-covered, and
    /// unattributable when one was gossiped — stated, and not counted against reproduction.
    #[test]
    fn comparison_is_on_the_canonical_tuple_and_names_what_it_cannot_attribute() {
        let a = rec("declined");
        let mut b = rec("declined");
        b.seq = 99; b.node = "other".into();
        assert_eq!(canonical(&a), canonical(&b));
        assert!(compare(std::slice::from_ref(&a), std::slice::from_ref(&b)).reproduces());

        let c = rec("elected");
        let cmp = compare(std::slice::from_ref(&a), std::slice::from_ref(&c));
        assert_eq!(cmp.divergences.len(), 1);
        assert!(cmp.divergences[0].attributable && !cmp.reproduces());

        let g = rec("elected").input(InputSnapshot::new("demand/", "providers=0", Provenance::Gossiped));
        let cmp = compare(&[a], &[g]);
        assert_eq!((cmp.unattributable, cmp.divergences[0].attributable), (1, false));
        assert!(cmp.reproduces(), "an unattributable divergence is reported, not counted");
        let cmp = compare(&[c], &[]);
        assert_eq!(cmp.divergences[0].right, None);
    }

    /// The coverage manifest lists every rule with its trace policy.
    #[test]
    fn the_coverage_manifest_names_each_rule_and_its_policy() {
        use crate::rule::{Outcome, OutcomeKind, Responsibility, RuleDescriptor, TracePolicy};
        static R: RuleDescriptor = RuleDescriptor {
            id: "t.one", revision: 2, subsystem: "t", responsibilities: &[Responsibility::Response], trigger: "t", inputs: &[],
            outcomes: &[Outcome { kind: OutcomeKind::Action, reasons: &["done"] }], effects: &[], guards: &[], may_trigger: &[],
            may_inhibit: &[], depends_on: &[], symbol: "t::t", docs: "d", tests: &["t"], trace: TracePolicy::Instrumented, summary: "s",
        };
        let m: serde_json::Value = serde_json::from_str(&coverage_manifest(&[&R])).unwrap();
        assert_eq!(m["schema"], "mycelium.coverage/1");
        assert_eq!(m["rules"][0]["id"], "t.one");
        assert_eq!(m["rules"][0]["revision"], 2);
        assert_eq!(m["rules"][0]["trace"], "instrumented");
    }

    /// The explanation groups by target in first-seen order, says each step's rule, outcome, reason,
    /// reads and effect, summarises the rules used from a catalogue, and counts what it cannot say.
    #[test]
    fn the_explanation_reads_as_what_each_target_went_through() {
        let s = DecisionSink::default();
        s.record(DecisionRecord::new("prov.eligible", 1, "round 1", OutcomeKind::Action, "eligible").target("text/echo").no_parent().no_effect()).unwrap();
        s.record(DecisionRecord::new("prov.demand_response", 1, "round 1", OutcomeKind::Action, "unmet_demand_live").target("text/echo")
            .input(InputSnapshot::new("demand/", "providers=0 demanding=1", Provenance::Gossiped)).effect("start_install")).unwrap();
        s.record(DecisionRecord::new("prov.presence_floor", 1, "round 1", OutcomeKind::NoAction, "floor_met").target("route/optimize").no_parent().no_effect()).unwrap();
        let jsonl = s.to_jsonl() + "not json\n";
        let catalogue = r#"{"schema":"mycelium.rules/1","rules":[{"id":"prov.eligible","summary":"Can host it."}]}"#;
        let text = explain(&jsonl, Some(catalogue), None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "text/echo");
        assert_eq!(lines[1], "  round 1 · prov.eligible → action eligible");
        assert_eq!(lines[2], "  round 1 · prov.demand_response → action unmet_demand_live ⇒ start_install (reads: providers=0 demanding=1)");
        assert_eq!(lines[3], "route/optimize");
        assert!(text.contains("  prov.eligible — Can host it."), "{text}");
        assert!(text.contains("3 record(s) over 2 target(s); 1 read gossiped or sampled inputs") && text.contains("1 line(s) did not parse"), "{text}");
        let only = explain(&jsonl, None, Some("route/optimize"));
        assert!(only.starts_with("route/optimize\n") && !only.contains("text/echo"));
    }
}
