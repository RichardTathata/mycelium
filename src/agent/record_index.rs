//! The gateway's by-hash index of decision records (adopter-surface Y4): what `GET /gateway/records/decision/{sha256}`
//! answers from, fed by each registered sink's [`TapSlot::RecordIndex`] tap — so the route **never takes a sink's ring
//! lock** (lock-order row 53) and never makes a decision point drop a record.
//!
//! **Fed lazily, off the decision path.** A sink sends a copy of each record it keeps to the tap after releasing its own
//! lock (row 72); nothing here runs a loop. Each lookup first moves what the taps hold into the index — hashing each
//! line as `Corpus::add_trace` does (`content_hash(to_string_line())`) — then searches it. Called off the async workers.
//!
//! **Bounded, and every loss counted.** At most [`INDEX_RECORDS`] records and [`INDEX_BYTES`] of their lines; past
//! either the oldest go (`evicted`). A tap holds at most [`INDEX_RECORDS`] copies waiting between lookups; a copy it
//! could not hold is the sink's `tap_dropped_for(RecordIndex)`. A source's kept records the index never received —
//! those kept before it was registered, and those its tap dropped — are `missed`, from the sink's own counters:
//! `last_seq` less the copies received.
//!
//! **Only the sinks registered here.** A sink the agent does not hold — a stem provisioner's, an activation hook's —
//! is read only once registered ([`GossipAgent::with_decision_record_source`](crate::GossipAgent::with_decision_record_source)).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mycelium_core::decision::{AlreadyTapped, DecisionRecord, DecisionSink, DropCounts, TapSlot};
use tokio::sync::mpsc::Receiver;

/// The most records the index keeps.
pub const INDEX_RECORDS: usize = 4096;
/// The most bytes of record lines the index keeps.
pub const INDEX_BYTES: usize = 4 << 20;

struct Source {
    label: Arc<str>,
    sink: Arc<DecisionSink>,
    rx: Receiver<DecisionRecord>,
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

/// The index: one lock (lock-order row 73), a leaf — held to move tapped copies in and search, never across a sink's
/// locks (`try_recv` takes none of them) or anything else.
#[derive(Default)]
pub struct RecordIndex {
    state: Mutex<State>,
}

impl RecordIndex {
    /// Register a sink: tap its [`TapSlot::RecordIndex`] slot. Refused when that slot already has a live reader.
    pub fn add_source(&self, label: impl Into<String>, sink: Arc<DecisionSink>) -> Result<(), AlreadyTapped> {
        let rx = sink.tap_slot(TapSlot::RecordIndex, INDEX_RECORDS)?;
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        st.sources.push(Source { label: Arc::from(label.into()), sink, rx, received: 0 });
        Ok(())
    }

    /// Move every waiting copy in, then find the record whose line hashes to `hash`.
    pub fn lookup(&self, hash: &str) -> Lookup {
        let mut guard = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let st = &mut *guard;
        let mut out = Lookup::default();
        for src in &mut st.sources {
            while let Ok(r) = src.rx.try_recv() {
                src.received += 1;
                let line = r.to_string_line();
                let h = crate::causal::content_hash(line.as_bytes());
                st.bytes += line.len();
                st.ring.push_back((h, Arc::clone(&src.label), line));
                while st.ring.len() > INDEX_RECORDS || st.bytes > INDEX_BYTES {
                    let Some((_, _, old)) = st.ring.pop_front() else { break };
                    st.bytes = st.bytes.saturating_sub(old.len());
                    st.evicted += 1;
                }
            }
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
