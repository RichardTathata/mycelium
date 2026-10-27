//! The gateway's by-hash index of decision records (adopter-surface Y4): what `GET /gateway/records/decision/{sha256}`
//! answers from, fed by each registered sink's [`TapSlot::RecordIndex`] tap — so the route **never takes a sink's ring
//! lock** (lock-order row 53) and never makes a decision point drop a record.
//!
//! **Fed lazily, off the decision path.** A sink pushes a copy of each record it keeps into its [`TapRing`] after
//! releasing its own lock (rows 72, 74); nothing here runs a loop. Each lookup first takes what the rings hold into the
//! index — hashing each line as `Corpus::add_trace` does (`content_hash(to_string_line())`) — then searches it. Called
//! off the async workers.
//!
//! **Bounded, the newest kept, every loss counted.** Between lookups a sink's ring holds the newest [`INDEX_RECORDS`]
//! copies and at most [`INDEX_BYTES`] of them by the sink's own estimate, the oldest let go; the index holds at most
//! [`INDEX_RECORDS`] records and [`INDEX_BYTES`] of their lines, the oldest let go (`evicted`). So the memory held for
//! one registered sink is bounded by twice that, whether or not anyone reads. A source's kept records the index never
//! received — kept before it was registered, let go by its ring, or dropped by the ring on contention — are `missed`,
//! from the sink's own counters: `last_seq` less the copies received.
//!
//! **A slot for the agent's life.** A registered sink's record-index slot is held until the agent is dropped; labels are
//! unique (a second source under a label already registered is refused).
//!
//! **Only the sinks registered here.** A sink the agent does not hold — a stem provisioner's, an activation hook's —
//! is read only once registered ([`GossipAgent::with_decision_record_source`](crate::GossipAgent::with_decision_record_source)).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mycelium_core::decision::{DecisionSink, DropCounts, TapRing};

/// The most records the index keeps.
pub const INDEX_RECORDS: usize = 4096;
/// The most bytes of record lines the index keeps.
pub const INDEX_BYTES: usize = 4 << 20;

/// Record lines with their content hashes, `(hash, line)`, oldest first.
type Hashed = Vec<(String, String)>;

struct Source {
    label: Arc<str>,
    sink: Arc<DecisionSink>,
    ring: Arc<TapRing>,
    received: u64,
}

#[derive(Default)]
struct State {
    sources: Vec<Source>,
    /// `(content hash, source label, line)`, oldest first.
    ring: VecDeque<(String, Arc<str>, String)>,
    bytes: usize,
    evicted: u64,
}

/// One registered source, as a lookup saw it.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SourceReport {
    pub label: String,
    /// Copies the index received from this sink.
    pub received: u64,
    /// Records the sink kept that the index never received: kept before it was registered, or dropped by its tap.
    pub missed: u64,
    /// Records the sink itself dropped (never kept), by reason — cumulative.
    pub dropped: DropCounts,
}

/// What one lookup found and searched.
#[derive(Clone, Debug, Default)]
pub struct Lookup {
    pub line: Option<String>,
    pub label: Option<String>,
    pub sources: Vec<SourceReport>,
    pub held: usize,
    pub evicted: u64,
}

/// The index: one lock (lock-order row 73), a leaf — held for a map read or update only, never across a sink's locks
/// or a ring's (row 74): a sink's tap is taken, and each ring emptied, with this lock released.
#[derive(Default)]
pub struct RecordIndex {
    state: Mutex<State>,
}

impl RecordIndex {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Register a sink under `label`: take its record-index tap. Refused, naming why, when the label is already
    /// registered or the sink's record-index tap already has a live reader.
    pub fn add_source(&self, label: &str, sink: Arc<DecisionSink>) -> Result<(), &'static str> {
        if self.lock().sources.iter().any(|s| &*s.label == label) {
            return Err("a source is already registered under this label");
        }
        let ring = sink.tap_ring(INDEX_RECORDS, INDEX_BYTES).map_err(|_| "the sink's record-index tap is already held")?;
        let mut st = self.lock();
        // Checked again: a concurrent registration under the same label may have landed meanwhile. The ring dropped
        // here frees the sink's slot.
        if st.sources.iter().any(|s| &*s.label == label) {
            return Err("a source is already registered under this label");
        }
        st.sources.push(Source { label: Arc::from(label), sink, ring, received: 0 });
        Ok(())
    }

    /// Take what each sink's ring holds into the index, then find the record whose line hashes to `hash`.
    pub fn lookup(&self, hash: &str) -> Lookup {
        let rings: Vec<(Arc<str>, Arc<TapRing>)> =
            self.lock().sources.iter().map(|s| (Arc::clone(&s.label), Arc::clone(&s.ring))).collect();
        // Emptied and hashed with no lock of the index held.
        let batches: Vec<(Arc<str>, Hashed)> = rings
            .into_iter()
            .map(|(label, ring)| {
                let lines = ring
                    .take()
                    .into_iter()
                    .map(|r| {
                        let line = r.to_string_line();
                        (crate::causal::content_hash(line.as_bytes()), line)
                    })
                    .collect();
                (label, lines)
            })
            .collect();
        let mut guard = self.lock();
        let st = &mut *guard;
        for (label, lines) in batches {
            if let Some(src) = st.sources.iter_mut().find(|s| s.label == label) {
                src.received += lines.len() as u64;
            }
            for (h, line) in lines {
                st.bytes += line.len();
                st.ring.push_back((h, Arc::clone(&label), line));
                while st.ring.len() > INDEX_RECORDS || st.bytes > INDEX_BYTES {
                    let Some((_, _, old)) = st.ring.pop_front() else { break };
                    st.bytes = st.bytes.saturating_sub(old.len());
                    st.evicted += 1;
                }
            }
        }
        let mut out = Lookup::default();
        for src in &st.sources {
            out.sources.push(SourceReport {
                label: src.label.to_string(),
                received: src.received,
                missed: src.sink.last_seq().saturating_sub(src.received),
                dropped: src.sink.drop_counts(),
            });
        }
        if let Some((_, label, line)) = st.ring.iter().rev().find(|(h, _, _)| h == hash) {
            out.label = Some(label.to_string());
            out.line = Some(line.clone());
        }
        out.held = st.ring.len();
        out.evicted = st.evicted;
        out
    }
}
