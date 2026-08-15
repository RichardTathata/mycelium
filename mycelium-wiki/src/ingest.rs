//! Bulk ingest — the **claim-check** path (council-substrate Phase 4,
//! `docs/design/transparency-council-substrate.md` §6.4). A pipeline worker that extracted a
//! meeting's leaves does **not** push the payload through the mesh: it stages the batch in a
//! bulk-capable store (S3 in the council deployment) and submits only a **reference** to the
//! curator, who fetches it from the configured [`BatchSource`] and applies it — in batch order,
//! through the same write path as every other apply (the Phase-3 gate runs, the change sink is
//! notified, the lint re-arms). The payload rule is structural: the RPC carries the reference
//! string, never the leaves.
//!
//! **Determinism + resubmission.** [`apply_batch`] writes each page with
//! [`write_page`](crate::WikiStore::write_page) in the batch's order, so the resulting store state
//! is byte-identical to a serial in-process writer applying the same pages (the Phase-4 gate
//! asserts identical git trees). Because `write_page` is a full replace and an unchanged rewrite
//! records nothing, **re-submitting a batch after a partial failure is naturally idempotent** —
//! already-applied pages are no-ops, only the remainder lands.
//!
//! The types here are data-plane (no Mycelium dependency): [`IngestBatch`]/[`IngestPage`] are the
//! staged payload format, [`BatchSource`] the pluggable fetch (an `S3BatchSource` is a deployment
//! impl of the same trait; [`FsBatchSource`] is the reference impl standing in for it). The curator
//! RPC surface (`wiki.{group}.ingest`) lives on [`Wiki`](crate::Wiki) under `control-plane`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::Section;
use crate::store::WikiStore;

/// One page in an ingest batch — applied via `write_page` (full replace) in batch order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestPage {
    /// The page path inside the group's store scope.
    pub path: String,
    #[serde(default)]
    pub attributes: BTreeMap<String, String>,
    pub sections: Vec<Section>,
}

/// A staged batch of extracted pages — what the claim-check reference points at.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestBatch {
    /// Provenance label for logs and the change-sink round (e.g. `pipeline/run-42`).
    pub source: String,
    pub pages:  Vec<IngestPage>,
}

/// The outcome of applying a batch: how many pages landed, how many the write gate refused, and
/// the gate's findings (one entry per refused page, prefixed with its path).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestSummary {
    pub applied:  usize,
    pub refused:  usize,
    pub findings: Vec<String>,
}

/// Where the curator fetches a staged batch, given a submitted reference. The claim-check source —
/// an object store in production; the trait keeps it pluggable exactly like
/// [`WikiStore`](crate::WikiStore) keeps the corpus store pluggable.
pub trait BatchSource: Send + Sync {
    fn fetch(&self, reference: &str) -> Result<IngestBatch, String>;
}

/// The filesystem reference implementation: a reference is a path **relative to the root** naming a
/// JSON-serialized [`IngestBatch`] (the shape an `S3BatchSource` reads from a bucket key).
pub struct FsBatchSource {
    root: PathBuf,
}

impl FsBatchSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl BatchSource for FsBatchSource {
    fn fetch(&self, reference: &str) -> Result<IngestBatch, String> {
        for comp in reference.split('/') {
            if comp.is_empty() || comp == "." || comp == ".." || comp.contains('\\') {
                return Err(format!("unsafe batch reference {reference:?}"));
            }
        }
        let bytes = std::fs::read(self.root.join(reference))
            .map_err(|e| format!("reading batch {reference:?}: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("decoding batch {reference:?}: {e}"))
    }
}

/// Apply a batch to `store` **in batch order** via the store's batch primitive
/// ([`write_pages`](crate::WikiStore::write_pages)) — the deterministic serial write phase,
/// factored pure so the byte-identical gate can prove it against a serial writer without an agent.
/// On `GitStore` the whole batch is **one commit** (the deployment's per-meeting boundary commit).
///
/// **Refusal semantics (P6.1, a recorded change from Phase 4):** a write-gate refusal refuses
/// the **whole batch** — the summary reports every page refused with the gate's findings, and on
/// an atomic store (`GitStore`) *nothing* was committed, matching the deployment's
/// whole-meetings-only crash invariant. On a non-atomic store (the default per-page loop) a
/// prefix may have applied before the refusal — either way, **resubmission converges**: applied
/// pages re-apply as no-ops. Any non-gate store error aborts with the error; the batch is
/// resubmittable.
pub fn apply_batch<S: WikiStore>(store: &S, batch: &IngestBatch) -> Result<IngestSummary, crate::WikiError> {
    let pages: Vec<crate::store::PageWrite> = batch
        .pages
        .iter()
        .map(|p| crate::store::PageWrite {
            path:       p.path.clone(),
            attributes: p.attributes.clone(),
            sections:   p.sections.clone(),
        })
        .collect();
    match store.write_pages(&pages, &batch.source) {
        Ok(()) => Ok(IngestSummary { applied: batch.pages.len(), refused: 0, findings: Vec::new() }),
        Err(e) => match e.as_gate_refusal() {
            Some(findings) => Ok(IngestSummary {
                applied:  0,
                refused:  batch.pages.len(),
                findings: vec![format!("batch refused by the write gate: {findings}")],
            }),
            None => Err(e),
        },
    }
}
