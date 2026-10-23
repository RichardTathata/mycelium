//! `mycelium explain --correlation <id> <files…>` and `mycelium explain --closure <hash> <files…>` (adopter-surface
//! X4): one run's records as one chain across the files given, and the dependency closure of one record.
//!
//! **Sources.** An evidence journal (`evidence.log`: the node-local, fsynced journal of `AeEvidence` records, read
//! with the journal module's `read_journal`) and a decision trace (`decisions.jsonl`: one `DecisionRecord` per line,
//! with `decisions.stats.json` beside it when the sink's counters were written). A file is read as a trace when its
//! first line parses as a JSON object; otherwise as a journal. Every record's identity is the content hash of its
//! bytes as written — a journal frame's body, a trace line without its newline — which is the hash an evidence
//! record's `journal_sha256` cites and the hash a `parent` names ([`crate::causal::content_hash`]).
//!
//! **Authoritative over the journal, best-effort over the trace.** The journal never drops a record; the decision
//! sink is lossy by design (`try_lock`, the newest dropped at either bound, counted). A `parent` that names a record
//! in none of the files is a **hole**, printed with the sink counters of every trace given, since a dropped decision
//! is one of the things it may be; a record the operator pruned or crypto-shredded is another, and a journal not
//! handed to the reader a third. The reader names the hole and never fills it.
//!
//! **Three words, the trace's own.** A line is *captured* when the record carries the field itself; *reconstructed*
//! when the reader joined it by `operation_id` to a record of the same attempt that carries it (an execution record
//! beside its decision); *unattributable* when the record carries no id the reader can use.
//!
//! **Order.** By event time where a record has one (`at_ms`), by file order where it has none; the output says
//! which. No clock is read here.
//!
//! **Bounds.** A closure stops at a federation edge (`gateway:federation/call` is the leaf and is printed as a
//! boundary: a partner's journals are private and trust does not compose) and visits each record once — a cycle
//! cannot arise from content hashes, and a hostile file's free-text parent is a hole, not a loop. A closure computed
//! over fewer journals than the records name nodes says so in its first line: it is incomplete.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::causal::content_hash;

/// The schema id printed at the head of each rendering.
pub const SCHEMA: &str = "mycelium.closure/1";

/// The enforcement point whose records are a closure's boundary.
const FEDERATION_EDGE: &str = "gateway:federation/call";

/// Where a record came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// An evidence journal: authoritative, never lossy.
    Journal,
    /// A decision trace: best-effort, the sink drops and counts.
    Trace,
}

impl SourceKind {
    fn word(self) -> &'static str {
        match self {
            SourceKind::Journal => "journal",
            SourceKind::Trace => "trace",
        }
    }
}

/// One record, as the reader joins it.
#[derive(Clone, Debug)]
pub struct Record {
    pub source: SourceKind,
    /// The file it was read from, as given.
    pub file: String,
    /// Its position in that file, from 0.
    pub index: usize,
    /// The content hash of its bytes as written.
    pub hash: String,
    /// `decided` / `execution` / `correction` for evidence; the rule id for a decision.
    pub kind: String,
    pub correlation: Option<String>,
    pub parent: Option<String>,
    pub operation_id: Option<String>,
    pub attempt_id: Option<String>,
    pub at_ms: Option<u64>,
    /// The node that wrote it: an evidence record's `via` (the enforcement point's node), a decision's `node`.
    pub node: Option<String>,
    /// The enforcement point, for evidence.
    pub enforcement_point: Option<String>,
    /// One line of what it says.
    pub summary: String,
}

/// A file the reader loaded.
#[derive(Clone, Debug)]
pub struct FileInfo {
    pub path: String,
    pub kind: SourceKind,
    pub records: usize,
    /// Lines that did not parse (a trace) or frames that were not evidence JSON (a journal) — counted, never skipped silently.
    pub unparsed: usize,
    /// The sink's counters from `decisions.stats.json` beside a trace, when present: records dropped, by reason.
    pub dropped: Option<BTreeMap<String, u64>>,
}

/// Every record the files given hold, joined.
#[derive(Clone, Debug, Default)]
pub struct Corpus {
    pub records: Vec<Record>,
    pub files: Vec<FileInfo>,
    by_hash: HashMap<String, usize>,
}

impl Corpus {
    /// Read every path. A path that cannot be read is an error naming it; a file whose contents do not parse
    /// contributes its parse count and nothing else.
    pub fn load(paths: &[PathBuf]) -> std::io::Result<Corpus> {
        let mut corpus = Corpus::default();
        for p in paths {
            let bytes = std::fs::read(p)?;
            let given = p.display().to_string();
            if looks_like_jsonl(&bytes) {
                let text = String::from_utf8_lossy(&bytes);
                let stats = sibling_stats(p);
                corpus.add_trace(&given, &text, stats);
            } else {
                // The journal mechanism is in every build (the rights ledger uses it); the evidence profile is not.
                let frames = crate::agent::journal::read_journal(p)?;
                corpus.add_journal(&given, &frames);
            }
        }
        Ok(corpus)
    }

    /// A trace from its text (one record per line); `dropped` is the sink's counters when known.
    pub fn add_trace(&mut self, file: &str, text: &str, dropped: Option<BTreeMap<String, u64>>) {
        let (mut n, mut unparsed) = (0usize, 0usize);
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) if v.is_object() => {
                    let hash = content_hash(line.as_bytes());
                    self.push(record_of(SourceKind::Trace, file, n, hash, &v));
                    n += 1;
                }
                _ => unparsed += 1,
            }
        }
        self.files.push(FileInfo { path: file.into(), kind: SourceKind::Trace, records: n, unparsed, dropped });
    }

    /// A journal from its frames (each the exact bytes one append wrote).
    pub fn add_journal(&mut self, file: &str, frames: &[Vec<u8>]) {
        let (mut n, mut unparsed) = (0usize, 0usize);
        for frame in frames {
            match serde_json::from_slice::<Value>(frame) {
                Ok(v) if v.is_object() => {
                    let hash = content_hash(frame);
                    self.push(record_of(SourceKind::Journal, file, n, hash, &v));
                    n += 1;
                }
                _ => unparsed += 1,
            }
        }
        self.files.push(FileInfo { path: file.into(), kind: SourceKind::Journal, records: n, unparsed, dropped: None });
    }

    fn push(&mut self, r: Record) {
        // Two files holding one record (a journal and its copy) are one record: the first wins, the second is skipped.
        if self.by_hash.contains_key(&r.hash) {
            return;
        }
        self.by_hash.insert(r.hash.clone(), self.records.len());
        self.records.push(r);
    }

    /// The record with this hash, if any file holds it.
    pub fn get(&self, hash: &str) -> Option<&Record> {
        self.by_hash.get(hash).map(|&i| &self.records[i])
    }

    fn journals(&self) -> usize {
        self.files.iter().filter(|f| f.kind == SourceKind::Journal).count()
    }

    fn traces(&self) -> usize {
        self.files.iter().filter(|f| f.kind == SourceKind::Trace).count()
    }

    /// The nodes the records name.
    fn nodes_named(&self) -> BTreeSet<String> {
        self.records.iter().filter_map(|r| r.node.clone()).collect()
    }

    /// Every record of one run, as one chain. Each line is marked *captured*, *reconstructed* or *unattributable*.
    pub fn chain(&self, correlation: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("{SCHEMA} chain for correlation {correlation}\n"));
        self.head(&mut out);
        // Captured: the record carries the id. Reconstructed: a record of the same operation that does not.
        let captured: Vec<usize> =
            (0..self.records.len()).filter(|&i| self.records[i].correlation.as_deref() == Some(correlation)).collect();
        let ops: HashSet<&str> = captured.iter().filter_map(|&i| self.records[i].operation_id.as_deref()).collect();
        let mut rows: Vec<(usize, &'static str)> = captured.iter().map(|&i| (i, "captured")).collect();
        for (i, r) in self.records.iter().enumerate() {
            if r.correlation.is_none() && r.operation_id.as_deref().is_some_and(|o| ops.contains(o)) {
                rows.push((i, "reconstructed"));
            }
        }
        if rows.is_empty() {
            out.push_str("  no record carries this correlation, and none joins to one by operation id\n");
            self.foot(&mut out);
            return out;
        }
        self.sort_rows(&mut rows);
        for (i, mark) in &rows {
            out.push_str(&self.line(&self.records[*i], mark));
            out.push('\n');
        }
        let unattributable = self.records.iter().filter(|r| r.correlation.is_none() && r.operation_id.is_none() && r.parent.is_none()).count();
        out.push_str(&format!(
            "\n{} record(s) in the chain ({} captured, {} reconstructed by operation id); {unattributable} record(s) in the files carry no id at all and are unattributable to any run.\n",
            rows.len(),
            rows.iter().filter(|(_, m)| *m == "captured").count(),
            rows.iter().filter(|(_, m)| *m == "reconstructed").count(),
        ));
        self.foot(&mut out);
        out
    }

    /// The dependency closure of one record: it, and every record whose parent chain reaches it, across the files
    /// given. Holes are named and never filled; a federation edge is a boundary; each record is visited once.
    pub fn closure(&self, hash: &str, fleet: Option<usize>) -> String {
        let mut out = String::new();
        out.push_str(&format!("{SCHEMA} closure of {hash}\n"));
        self.head(&mut out);
        let nodes = self.nodes_named();
        let journals = self.journals();
        let fleet_line = match fleet {
            Some(n) if journals < n => format!("INCOMPLETE: {journals} journal(s) given for a fleet of {n} nodes — a record on a node whose journal is not here is not in this closure.\n"),
            Some(n) => format!("{journals} journal(s) given for a fleet of {n} nodes.\n"),
            None if journals < nodes.len() => format!("INCOMPLETE: {journals} journal(s) given, and the records name {} node(s) ({}) — a record on a node whose journal is not here is not in this closure. Pass --fleet <n> to state the fleet's size.\n", nodes.len(), nodes.iter().cloned().collect::<Vec<_>>().join(", ")),
            None => format!("{journals} journal(s) given; the records name {} node(s). The fleet's size is not known to this reader: pass --fleet <n>, or a closure over fewer journals than nodes is incomplete without saying so.\n", nodes.len()),
        };
        out.push_str(&fleet_line);

        let Some(root) = self.get(hash) else {
            out.push_str(&format!("  the root {hash} is in none of the files given — "));
            out.push_str(&self.hole_reason());
            out.push('\n');
            self.cited_by(hash, &mut out);
            self.foot(&mut out);
            return out;
        };
        // Children by parent hash.
        let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, r) in self.records.iter().enumerate() {
            if let Some(p) = &r.parent {
                children.entry(p.as_str()).or_default().push(i);
            }
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut order: Vec<(usize, usize)> = Vec::new(); // (depth, index)
        let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
        let root_i = self.by_hash[hash];
        seen.insert(hash.to_string());
        queue.push_back((0, root_i));
        let mut boundaries = 0usize;
        while let Some((depth, i)) = queue.pop_front() {
            order.push((depth, i));
            let r = &self.records[i];
            if r.enforcement_point.as_deref() == Some(FEDERATION_EDGE) {
                boundaries += 1;
                continue;
            }
            if let Some(kids) = children.get(r.hash.as_str()) {
                let mut kids = kids.clone();
                self.sort_indices(&mut kids);
                for k in kids {
                    if seen.insert(self.records[k].hash.clone()) {
                        queue.push_back((depth + 1, k));
                    }
                }
            }
        }
        let root_parent = root.parent.clone();
        for (depth, i) in &order {
            let r = &self.records[*i];
            let mark = if *depth == 0 { if root_parent.is_some() { "captured" } else { "captured, a root" } } else { "captured" };
            out.push_str(&format!("{}{}", "  ".repeat(*depth), self.line(r, mark)));
            if r.enforcement_point.as_deref() == Some(FEDERATION_EDGE) {
                out.push_str("  [boundary: a federation edge — the partner's journals are private; this is the leaf, not the end of the chain]");
            }
            out.push('\n');
        }
        if let Some(p) = &root_parent && self.get(p).is_none() {
            out.push_str(&format!("  the root's own parent {p} is in none of the files given — "));
            out.push_str(&self.hole_reason());
            out.push('\n');
        }
        // Holes: a parent cited by a record in the closure's files that no file holds, beneath this root's tree.
        let mut holes: BTreeMap<String, usize> = BTreeMap::new();
        for r in &self.records {
            if let Some(p) = &r.parent && self.get(p).is_none() && p != hash {
                *holes.entry(p.clone()).or_default() += 1;
            }
        }
        out.push_str(&format!(
            "\n{} record(s) in the closure over {} file(s); {boundaries} federation boundary(ies); {} hole(s) cited elsewhere in these files.\n",
            order.len(),
            self.files.len(),
            holes.len()
        ));
        for (p, n) in &holes {
            out.push_str(&format!("  hole: {p} is cited by {n} record(s) and is in none of the files given — {}\n", self.hole_reason()));
        }
        self.foot(&mut out);
        out
    }

    fn cited_by(&self, hash: &str, out: &mut String) {
        let n = self.records.iter().filter(|r| r.parent.as_deref() == Some(hash)).count();
        if n > 0 {
            out.push_str(&format!("  {n} record(s) cite it as their parent.\n"));
        }
    }

    /// What a hole may be — stated, never resolved.
    fn hole_reason(&self) -> String {
        let drops: u64 = self.files.iter().filter_map(|f| f.dropped.as_ref()).map(|d| d.values().sum::<u64>()).sum();
        let traces = self.traces();
        let mut s = String::from("a journal not given, a record pruned by retention or crypto-shredded");
        if traces > 0 {
            s.push_str(&format!(", or a decision the sink dropped (the {traces} trace(s) given report {drops} dropped record(s)"));
            if self.files.iter().any(|f| f.kind == SourceKind::Trace && f.dropped.is_none()) {
                s.push_str("; a trace without decisions.stats.json beside it reports none, which is not zero");
            }
            s.push(')');
        }
        s
    }

    fn head(&self, out: &mut String) {
        for f in &self.files {
            let d = match &f.dropped {
                Some(d) => format!("; sink dropped {}", d.values().sum::<u64>()),
                None if f.kind == SourceKind::Trace => "; sink counters not given".to_string(),
                None => String::new(),
            };
            out.push_str(&format!("  {}: {} record(s), {} unparsed{d} ({})\n", f.kind.word(), f.records, f.unparsed, f.path));
        }
    }

    fn foot(&self, out: &mut String) {
        out.push_str("Order: event time where a record has one, file order where it has none. The journal is authoritative; the trace is best-effort (its sink drops and counts). A KV receipt is not journaled and joins only by operation id.\n");
    }

    fn sort_rows(&self, rows: &mut [(usize, &'static str)]) {
        rows.sort_by_key(|(i, _)| self.order_key(*i));
    }

    fn sort_indices(&self, idx: &mut [usize]) {
        idx.sort_by_key(|i| self.order_key(*i));
    }

    /// Event time first (a record without one sorts after those with one, by file order).
    fn order_key(&self, i: usize) -> (u8, u64, usize, usize) {
        let r = &self.records[i];
        let file_pos = self.files.iter().position(|f| f.path == r.file).unwrap_or(0);
        match r.at_ms {
            Some(t) => (0, t, file_pos, r.index),
            None => (1, 0, file_pos, r.index),
        }
    }

    fn line(&self, r: &Record, mark: &str) -> String {
        let at = r.at_ms.map(|t| format!("t={t}")).unwrap_or_else(|| "t=-".into());
        let short = &r.hash[..12.min(r.hash.len())];
        let parent = match &r.parent {
            Some(p) => format!(" parent={}{}", &p[..12.min(p.len())], if self.get(p).is_some() { "" } else { " (hole)" }),
            None => String::new(),
        };
        let corr = r.correlation.as_deref().map(|c| format!(" correlation={c}")).unwrap_or_default();
        let op = r.operation_id.as_deref().map(|o| format!(" op={o}")).unwrap_or_default();
        let node = r.node.as_deref().map(|n| format!(" @{n}")).unwrap_or_default();
        format!("[{mark}] {} {} {short} {at}{node}{corr}{op}{parent} — {}", r.source.word(), r.kind, r.summary)
    }
}

/// A trace is text whose first non-blank line is a JSON object; a journal is length-prefixed frames.
fn looks_like_jsonl(bytes: &[u8]) -> bool {
    let first = bytes.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
    bytes.get(first) == Some(&b'{')
        && std::str::from_utf8(bytes).ok().and_then(|s| s.lines().find(|l| !l.trim().is_empty()).map(|l| serde_json::from_str::<Value>(l).is_ok())).unwrap_or(false)
}

/// `decisions.stats.json` beside a trace, as the dropped counters it holds.
fn sibling_stats(trace: &Path) -> Option<BTreeMap<String, u64>> {
    let p = trace.parent()?.join("decisions.stats.json");
    let v: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    let mut m = BTreeMap::new();
    for (k, x) in v.as_object()? {
        if k.starts_with("dropped_") && let Some(n) = x.as_u64() {
            m.insert(k.clone(), n);
        }
    }
    Some(m)
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_owned)
}

/// One record from its JSON, whichever schema wrote it.
fn record_of(source: SourceKind, file: &str, index: usize, hash: String, v: &Value) -> Record {
    let schema = str_of(v, "schema").unwrap_or_default();
    let evidence = schema.starts_with("mycelium.ae-evidence") || v.get("enforcement_point").is_some();
    let (kind, node, summary, enforcement_point) = if evidence {
        let kind = str_of(v, "kind").unwrap_or_else(|| "decided".into());
        let decision = v.get("decision").map(|d| d.as_str().map(str::to_owned).unwrap_or_else(|| d.to_string())).unwrap_or_default();
        let execution = v.get("execution").map(|e| e.as_str().map(str::to_owned).unwrap_or_else(|| e.to_string())).unwrap_or_default();
        let summary = format!(
            "{} {} {} → {decision}, execution {execution}, policy {}",
            str_of(v, "subject").unwrap_or_default(),
            str_of(v, "operation").unwrap_or_default(),
            str_of(v, "resource").unwrap_or_default(),
            str_of(v, "policy_revision").unwrap_or_default()
        );
        (kind, str_of(v, "via"), summary, str_of(v, "enforcement_point"))
    } else {
        let rule = str_of(v, "rule").unwrap_or_else(|| "?".into());
        let summary = format!(
            "{} → {} {}{}",
            str_of(v, "trigger").unwrap_or_default(),
            v.get("outcome").map(|o| o.as_str().map(str::to_owned).unwrap_or_else(|| o.to_string())).unwrap_or_default(),
            str_of(v, "reason").unwrap_or_default(),
            str_of(v, "effect").map(|e| format!(" ⇒ {e}")).unwrap_or_default()
        );
        (rule, str_of(v, "node"), summary, None)
    };
    Record {
        source,
        file: file.into(),
        index,
        hash,
        kind,
        correlation: str_of(v, "correlation"),
        parent: str_of(v, "parent"),
        operation_id: str_of(v, "operation_id"),
        attempt_id: str_of(v, "attempt_id"),
        at_ms: v.get("at_ms").and_then(Value::as_u64),
        node,
        enforcement_point,
        summary,
    }
}

/// The fuzz hook (§12.6's registry): a trace's text parsed as a corpus, then every record's chain and closure
/// rendered. The invariant: rendering terminates, prints its own root, and visits each record at most once, whatever
/// a hostile file puts in `parent`. `true` when at least one record parsed, so the seed's reachability can be asserted.
#[cfg(feature = "fuzz-internals")]
pub fn fuzz_corpus_render(data: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(data) else { return false };
    let mut c = Corpus::default();
    c.add_trace("fuzz", text, None);
    for r in c.records.clone() {
        let out = c.closure(&r.hash, None);
        let own = out.matches(&r.hash[..12.min(r.hash.len())]).count();
        assert!(own >= 1, "a closure must print its own root");
        if let Some(id) = &r.correlation {
            let _ = c.chain(id);
        }
    }
    !c.records.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evidence(at: u64, op: &str, kind: &str, correlation: Option<&str>, parent: Option<&str>, point: &str) -> String {
        let mut v = json!({
            "schema": "mycelium.ae-evidence/1", "kind": kind, "at_ms": at, "subject": "oidc:idp/alice", "operation": "tools/call",
            "resource": "tool:post@n1", "decision": "permit", "execution": "attempted", "policy_revision": "rev-1",
            "enforcement_point": point, "operation_id": op, "attempt_id": format!("{op}/1"), "via": "127.0.0.1:7001"
        });
        if let Some(c) = correlation { v["correlation"] = json!(c); }
        if let Some(p) = parent { v["parent"] = json!(p); }
        v.to_string()
    }

    fn decision(seq: u64, correlation: Option<&str>, parent: Option<&str>) -> String {
        let mut v = json!({"schema": "mycelium.decision/1", "rule": "signal.admission", "rule_revision": 2, "node": "127.0.0.1:7002",
            "seq": seq, "trigger": "frame", "outcome": "Permitted", "reason": "under_fill"});
        if let Some(c) = correlation { v["correlation"] = json!(c); }
        if let Some(p) = parent { v["parent"] = json!(p); }
        v.to_string()
    }

    /// A chain across a journal and a trace: captured records carry the id, the execution record of a captured
    /// decision is reconstructed by operation id, and a record with no id is unattributable.
    #[test]
    fn a_chain_marks_captured_reconstructed_and_unattributable() {
        let mut c = Corpus::default();
        let decided = evidence(10, "op-1", "decided", Some("run-7"), None, "gateway:mcp/tools/call");
        let executed = evidence(10, "op-1", "execution", None, None, "gateway:mcp/tools/call");
        let stranger = json!({"schema": "mycelium.ae-evidence/1", "kind": "decided", "subject": "x", "enforcement_point": "p"}).to_string();
        c.add_journal("a.log", &[decided.clone().into_bytes(), executed.into_bytes(), stranger.into_bytes()]);
        let h = content_hash(decided.as_bytes());
        c.add_trace("d.jsonl", &format!("{}\n{}\n", decision(1, Some("run-7"), Some(&h)), decision(2, None, None)), None);
        let out = c.chain("run-7");
        assert!(out.contains("[captured] journal decided"), "{out}");
        assert!(out.contains("[reconstructed] journal execution"), "{out}");
        assert!(out.contains("[captured] trace signal.admission"), "{out}");
        assert!(out.contains("3 record(s) in the chain (2 captured, 1 reconstructed by operation id); 2 record(s) in the files carry no id at all"), "{out}");
        assert!(c.chain("run-8").contains("no record carries this correlation"));
    }

    /// A closure names every dependent record and no independent one; a hole is named with the sink's counters;
    /// a federation edge is a boundary; a cycle terminates.
    #[test]
    fn a_closure_follows_parents_names_holes_and_stops_at_a_federation_edge() {
        let mut c = Corpus::default();
        let root = evidence(1, "op-r", "decided", Some("run-1"), None, "gateway:mcp/tools/call");
        let rh = content_hash(root.as_bytes());
        let child = evidence(2, "op-c", "decided", Some("run-1"), Some(&rh), "provider");
        let ch = content_hash(child.as_bytes());
        let grandchild = decision(5, Some("run-1"), Some(&ch));
        let independent = evidence(3, "op-i", "decided", Some("run-2"), None, "gateway:mcp/tools/call");
        let edge = evidence(4, "op-e", "decided", Some("run-1"), Some(&rh), FEDERATION_EDGE);
        let eh = content_hash(edge.as_bytes());
        let beyond = evidence(6, "op-b", "decided", Some("run-1"), Some(&eh), "gateway:a2a");
        let orphan = evidence(7, "op-o", "decided", Some("run-1"), Some(&"f".repeat(64)), "provider");
        c.add_journal("n1.log", &[root.into_bytes(), child.into_bytes(), independent.into_bytes(), edge.into_bytes(), beyond.into_bytes(), orphan.into_bytes()]);
        let mut dropped = BTreeMap::new();
        dropped.insert("dropped_full".to_string(), 3u64);
        c.add_trace("d.jsonl", &format!("{grandchild}\n"), Some(dropped));
        let out = c.closure(&rh, Some(3));
        assert!(out.contains("INCOMPLETE: 1 journal(s) given for a fleet of 3 nodes"), "{out}");
        assert!(out.contains("[captured, a root] journal decided"), "{out}");
        assert!(out.contains("  [captured] journal decided") && out.contains("    [captured] trace signal.admission"), "{out}");
        assert!(out.contains("[boundary: a federation edge"), "{out}");
        assert!(!out.contains("op=op-b"), "a record beyond the edge is not reached through it: {out}");
        assert!(!out.contains("op=op-i"), "an independent record is not in the closure: {out}");
        assert!(out.contains("4 record(s) in the closure"), "{out}");
        assert!(out.contains(&format!("hole: {} is cited by 1 record(s)", "f".repeat(64))), "{out}");
        assert!(out.contains("report 3 dropped record(s)"), "{out}");
        // A root whose own parent is in no file says so; a hostile parent that is not a hash at all is a hole like any
        // other (a cycle cannot arise from content hashes — a record cannot contain the hash of a record that contains
        // its own — so the visited set guards only a repeated child).
        let mut h = Corpus::default();
        let a = decision(1, None, Some("not-even-a-hash"));
        h.add_trace("hostile.jsonl", &format!("{a}\n"), None);
        let out = h.closure(&content_hash(a.as_bytes()), None);
        assert!(out.contains("1 record(s) in the closure"), "{out}");
        assert!(out.contains("the root's own parent not-even-a-hash is in none of the files given"), "{out}");
    }

    /// The reader says when it has fewer journals than nodes, and names a root it does not hold.
    #[test]
    fn fewer_journals_than_nodes_is_said_first_and_a_missing_root_is_a_hole() {
        let mut c = Corpus::default();
        let one = evidence(1, "op-1", "decided", None, None, "gateway:mcp/tools/call");
        c.add_journal("n1.log", &[one.into_bytes()]);
        c.add_trace("d.jsonl", &format!("{}\n", decision(1, None, None)), None);
        let out = c.closure(&"0".repeat(64), None);
        assert!(out.contains("INCOMPLETE: 1 journal(s) given, and the records name 2 node(s)"), "{out}");
        assert!(out.contains("is in none of the files given"), "{out}");
        assert!(out.contains("a trace without decisions.stats.json beside it reports none, which is not zero"), "{out}");
    }

    /// A journal file and a trace file are told apart by content, and a trace's sibling counters are read.
    #[test]
    fn files_are_sniffed_and_sibling_stats_read() {
        let dir = std::env::temp_dir().join(format!("closure-sniff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("decisions.jsonl"), format!("{}\n{}\n", decision(1, Some("r"), None), "not json")).unwrap();
        std::fs::write(dir.join("decisions.stats.json"), r#"{"recorded":1,"dropped_full":2,"dropped_bytes":0,"dropped_contended":1,"dropped_oversized":0,"held":1}"#).unwrap();
        let c = Corpus::load(&[dir.join("decisions.jsonl")]).unwrap();
        assert_eq!((c.files[0].kind, c.files[0].records, c.files[0].unparsed), (SourceKind::Trace, 1, 1));
        assert_eq!(c.files[0].dropped.as_ref().unwrap().values().sum::<u64>(), 3);
        assert!(Corpus::load(&[dir.join("missing")]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
