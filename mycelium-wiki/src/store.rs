//! The **data-plane interface** — the pluggable backing store a group's wiki lives in. Deliberately
//! substrate-agnostic: an `FsStore` (this crate), an `S3Store`, or a doc-store all implement it, and
//! the Mycelium control plane (Phase 2) drives whichever the group is configured with.
//!
//! **Concurrency contract.** The store is **airtight under concurrent writers** — it does not *assume*
//! a single writer, it *enforces* one per object via compare-and-swap. Two writers (e.g. a transient
//! split-brain where the ring briefly elects two curators) can no longer clobber each other:
//!
//! - **Section body edits** go through [`write_section`](WikiStore::write_section), a CAS keyed on the
//!   section's own version — an independent slot per section, so two curators editing *different*
//!   sections of one page never contend, and two editing the *same* section serialise (the stale one
//!   gets [`WikiError::Conflict`] and must re-read + re-reconcile).
//! - **Membership / order / page attributes** go through [`update_manifest`](WikiStore::update_manifest),
//!   a CAS keyed on the manifest version.
//! - A CAS write returns [`WikiError::Conflict`] instead of silently overwriting; the curator treats
//!   that as "re-read the committed state and re-apply".
//!
//! The CAS is **at-least-once, never-lose**: a version can be consumed by a follower (who reads it and
//! builds the next version on top) and then re-applied on the original writer's retry, so a `Conflict`
//! caller may apply its edit more than once — but no committed edit is ever *lost*. Exactly-once
//! *effect* therefore comes from the caller's reconcile being **idempotent** (the append-merge skips an
//! already-contained edit) — the same at-least-once + idempotent-merge = exactly-once-effect contract
//! the tuple space and blackboard use (see `docs/design/exactly-once-effect.md`). Bounded disk (old
//! versions are GC'd) is why the store cannot promise exactly-once at its own layer.
//!
//! Readers are still torn-read-safe: a store publishes each object atomically and a reader entering via
//! the manifest never observes a half-applied edit. [`write_page`](WikiStore::write_page) remains as a
//! non-CAS full-replace convenience (page bootstrap / tests); the concurrent write path is the CAS pair.

use std::collections::BTreeMap;

use crate::model::{Page, Predicate, Section, SectionId, SectionRef, WikiError};

/// One whole page in a [`write_pages`](WikiStore::write_pages) batch (council-substrate P6.1).
#[derive(Debug, Clone)]
pub struct PageWrite {
    pub path:       String,
    pub attributes: BTreeMap<String, String>,
    pub sections:   Vec<Section>,
}

/// A page read together with the compare-and-swap version tokens a curator needs to write it back
/// safely. The curator reads this, reconciles, then writes each section (and, on a membership change,
/// the manifest) against the versions it saw — a mismatch means someone else committed in between.
pub struct VersionedPage {
    /// The manifest's section render order (membership + sequence), as committed.
    pub order: Vec<SectionId>,
    /// Page-level attributes, as committed.
    pub attributes: BTreeMap<String, String>,
    /// The manifest's current version — the CAS token for [`update_manifest`](WikiStore::update_manifest).
    pub manifest_version: u64,
    /// **Every** section that has an object on the store — `id -> (version, content)` — including
    /// sections not (yet) referenced by `order` (an in-flight membership add). The `version` is the CAS
    /// token for [`write_section`](WikiStore::write_section); the content is the reconcile base.
    pub sections: BTreeMap<SectionId, (u64, Section)>,
}

/// A group's wiki backing store. All methods are `&self` — a store handle is shared; a curator holds
/// the write side, readers hold read handles to the same underlying store.
pub trait WikiStore: Send + Sync {
    /// A stable locator for this store (a path / bucket-prefix / URI) — what the curator advertises so
    /// group agents can reach it directly for reads.
    fn location(&self) -> String;

    /// Read a page: its manifest joined with the live section bodies it references, in render order.
    /// `None` if the page has no manifest. Sections present on the store but **not** referenced by the
    /// manifest are invisible here (that is what makes a torn multi-section write unobservable).
    fn read(&self, page: &str) -> Result<Option<Page>, WikiError>;

    /// The **curator's** richer read: the page plus the CAS version tokens for its manifest and each
    /// section (including in-flight orphans). `None` if the page has no manifest. Used to drive
    /// [`write_section`] / [`update_manifest`].
    ///
    /// [`write_section`]: WikiStore::write_section
    /// [`update_manifest`]: WikiStore::update_manifest
    fn read_versioned(&self, page: &str) -> Result<Option<VersionedPage>, WikiError>;

    /// Find sections matching `predicate` across all pages (structured attribute filter — the
    /// scope/id query, not similarity search). Only manifest-referenced sections are considered.
    fn query(&self, predicate: &Predicate) -> Result<Vec<SectionRef>, WikiError>;

    /// **Compare-and-swap write of one section body** (the concurrent write path). `expected` is the
    /// section's current version as read via [`read_versioned`](WikiStore::read_versioned) — `None`
    /// means "this section must not yet exist" (a create). Returns the new version on success, or
    /// [`WikiError::Conflict`] if the on-store version has moved since `expected` (the caller re-reads
    /// and retries). Does **not** touch the manifest — membership is a separate CAS.
    fn write_section(
        &self, page: &str, section: &Section, expected: Option<u64>,
    ) -> Result<u64, WikiError>;

    /// **Compare-and-swap update of a page's manifest** — its section `order` and page `attributes`.
    /// Used only when section *membership*/order or page attributes change, not for body edits.
    /// `expected` is the current manifest version (`None` = the page must not yet exist). Returns the
    /// new manifest version, or [`WikiError::Conflict`] if it moved since `expected`.
    fn update_manifest(
        &self, page: &str, order: &[SectionId], attributes: &BTreeMap<String, String>,
        expected: Option<u64>,
    ) -> Result<u64, WikiError>;

    /// **Non-CAS full-replace convenience** (page bootstrap, tests, one-shot imports): replace a page
    /// with `sections` + page `attributes`, dropping sections no longer referenced. Force-writes over
    /// whatever is there; it does **not** guard against a concurrent writer, so it is *not* the curator
    /// path — use [`write_section`](WikiStore::write_section) + [`update_manifest`](WikiStore::update_manifest)
    /// there. Still torn-read-safe (each object published atomically, manifest last).
    fn write_page(
        &self, page: &str, sections: &[Section], attributes: &BTreeMap<String, String>,
    ) -> Result<(), WikiError>;

    /// List the paths of all pages that currently have a manifest.
    fn list_pages(&self) -> Result<Vec<String>, WikiError>;

    /// **Batch write of whole pages** (council-substrate P6.1), applied in slice order. `label` is
    /// provenance for stores that record it (a commit message fragment); others ignore it.
    ///
    /// The default is the plain per-page loop — `FsStore`/`S3Store` semantics unchanged, and an
    /// error may leave a prefix applied (each page that did apply is a complete page; a retry
    /// re-applies idempotently). A store with a cheaper or atomic batch primitive overrides it:
    /// `GitStore` lands the whole batch as **one commit** (the deployment's per-meeting boundary
    /// commit), runs its write gate **once over the full batch**, and makes a gate refusal
    /// **whole-batch atomic** — nothing commits, so the repository only ever holds whole batches.
    fn write_pages(&self, pages: &[PageWrite], label: &str) -> Result<(), WikiError> {
        let _ = label;
        for p in pages {
            self.write_page(&p.path, &p.sections, &p.attributes)?;
        }
        Ok(())
    }

    /// **Remove a page from the store** — the right-to-erasure verb (GDPR Art. 17 / the
    /// `docs/operations/data-erasure.md` procedure). Removes the page's manifest and every section
    /// object so no read or query path can serve it; sub-pages nested under the path are **not**
    /// touched. Returns `Ok(true)` if the page existed, `Ok(false)` if there was nothing to remove
    /// (idempotent — a retry after a partial failure completes the erasure). `label` is provenance
    /// for stores that record it.
    ///
    /// What "removed" means is the store's honest ceiling, and differs by store:
    /// `FsStore` deletes the objects — the bytes are gone (modulo filesystem forensics — full
    /// erasure of a corpus additionally follows the crypto-shred procedure). `GitStore` commits a
    /// removal **at tip**: the page stops being served, but git *history retains it by design* —
    /// that is redaction, not erasure, which is exactly why the git-as-truth envelope (E1) is
    /// limited to public-record corpora. An erasable corpus belongs on an erasable store with
    /// the `GitMirror` projection sink (erase here, then rebuild the mirror per its documented
    /// erasure procedure — the sink is feature-gated behind `git-mirror`).
    ///
    /// The default **fails closed** (`Err`, "not implemented"): a store that has not implemented
    /// erasure must never silently pretend it erased.
    fn remove_page(&self, page: &str, label: &str) -> Result<bool, WikiError> {
        let _ = (page, label);
        Err(WikiError::Io(std::io::Error::other(
            "this WikiStore does not implement remove_page (right-to-erasure)",
        )))
    }

    /// **Bring this store's local view up to date with its shared backing** (council-substrate
    /// P6.3). Default: no-op — `FsStore`/`S3Store` are inherently shared, so a promoted curator
    /// already sees the truth. A store with node-local state (`GitStore`: a per-node clone)
    /// overrides it (fetch + adopt the remote head). The curator calls this **at promotion, before
    /// serving** — and refuses the curatorship on failure, because a knowingly-stale curator is
    /// the data-loss path.
    fn refresh(&self) -> Result<(), WikiError> {
        Ok(())
    }

    /// **Make this store's local writes visible to other nodes' views** (P6.3). Default: no-op —
    /// inherently-shared stores have nothing to publish. `GitStore` pushes to its remote (with a
    /// plumbing merge retry and a post-push divergence tripwire). The curator calls this
    /// best-effort after each applied round — a publish failure is logged, never fails the apply
    /// (the writes are committed locally; the next round retries).
    fn publish(&self) -> Result<(), WikiError> {
        Ok(())
    }
}
