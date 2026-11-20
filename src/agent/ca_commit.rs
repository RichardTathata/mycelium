//! **The CA's commitment to its successor, on a node** (identity-and-lifecycle I2, PR 2b).
//!
//! `mycelium tls commit` has the old CA sign `ca-commit.pem`, naming its successor
//! ([`CaCommitment`](mycelium_core::tls::CaCommitment)): the old CA's own promise. This module is what a node does
//! about it, and each step is the node's own:
//!
//! - **It pins a commitment only from the file its operator placed** (`{auto_cert_dir}/ca-commit.pem`; the
//!   maintainer's decisions (a) of 2026-11-15 and of 2026-11-16), after assessing it against a root it already
//!   trusts, and persists the pin under its own `succession/` before it goes on. It refuses to start, by name, when
//!   it holds a commitment and no `[persistence]`: a node can only promise what it can remember (#805's review, F1).
//! - **A pin is per committing CA** (the decision of 2026-11-18): the node's own promise "I accept no other successor
//!   from this CA". Once it trusts the successor, it may pin one commitment from the successor the same way — a
//!   different promiser, speaking of its own succession; the first pin is kept, spent.
//! - **A second commitment from a CA it holds a pin for is refused and counted**, wherever it is seen: in the
//!   operator's file at a later start, or in the KV. There is no in-band way to replace a pin.
//! - **The KV carries commitments for detection only.** A node publishes each commitment it pinned, while it trusts
//!   that commitment's CA, at `sys/ca-commit/{node}/{predecessor}` — the statement "this is what I pinned", in its own
//!   path. A reader verifies a copy against its own roots (a CA it trusts signed it, or it is read as absent and
//!   counted), compares it with the pin it holds **for that copy's CA**, and counts a conflict: two commitments one
//!   CA's key signed, which is what a stolen key produces. A copy from a CA the reader holds no pin for is not a
//!   conflict. **A KV copy never becomes the source of a pin.**
//!
//! Nothing here changes what a node trusts. The record the successor signs, and the phases that act on it, are the
//! row's next PR; they read the successor from [`CommitmentState::pinned_for`], and from nowhere else.

use super::TaskCtx;
use mycelium_core::node_id::NodeId;
use mycelium_core::tls::{CaCommitment, OfferedCommitment};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// KV namespace of the commitments nodes publish for detection: `sys/ca-commit/{node}/{predecessor-hex}`.
pub const CA_COMMIT_PREFIX: &str = "sys/ca-commit/";

/// A copy larger than this is not a commitment (one is under 2 KiB) and is skipped unread: any admitted member can
/// write a value of megabytes under the prefix, and the scan must not hash it.
const MAX_COPY_BYTES: usize = 4_096;
/// Copies read in one scan; the rest are left for the next scans and the scan says how many
/// (`ca_commitment_scan_skipped`). Each scan starts where the last one stopped, in key order and wrapping, so with `n`
/// readable copies every one is read within `ceil(n / MAX_COPIES_PER_SCAN)` consecutive scans.
const MAX_COPIES_PER_SCAN: usize = 1_024;
/// Distinct unverified copies counted in a process's life. Past it a further distinct one is **not counted** (and
/// nothing is ever counted twice): the counter then reads "at least this many".
const NOTED_UNVERIFIED_MAX: usize = 1_024;
/// Distinct conflicting commitments counted in a process's life; past it, as above. Each needs the CA's own key.
const NOTED_CONFLICTS_MAX: usize = 64;

/// The key a node publishes the commitment it pinned from one CA at: `predecessor` is the SHA-256 of that CA's
/// certificate (the commitment's `predecessor`), so a node holding two pins publishes two keys.
pub fn ca_commit_key(node: &NodeId, predecessor: &[u8; 32]) -> String {
    format!("{CA_COMMIT_PREFIX}{node}/{}", hex(predecessor))
}

/// This node's commitment: its pin, and what it has refused. Lock-free (`ArcSwap`, papaya, atomics): no lock-order row.
#[derive(Default)]
pub(crate) struct CommitmentState {
    /// The commitments this node pinned — at most one per committing CA — each with whether that CA is still a root
    /// this node trusts. Set once at start from the operator's file and the persisted pins — never from the KV.
    pins: arc_swap::ArcSwap<Vec<(Arc<CaCommitment>, bool)>>,
    /// Second commitments the pinned CA's key signed, refused: distinct ones, from the operator's file at a start
    /// and from KV copies.
    pub(crate) conflicts: AtomicU64,
    /// Distinct KV copies read as absent: malformed, or not signed by a root this node trusts. The KV's only: the
    /// operator's file has its own signal.
    pub(crate) unverified: AtomicU64,
    /// `1` when, at this start, the operator's file held a commitment from a CA this node does not trust (yet) and
    /// the node, holding a pin, started without pinning it. Every other file it cannot assess refuses the start.
    pub(crate) file_untrusted_ca: AtomicU64,
    /// Where the next bounded scan starts among the copies, so that every copy is read within a bounded number of
    /// scans whatever sorts ahead of it (#812's confirm round C3). A cursor, not a random draw: deterministic.
    scan_cursor: std::sync::atomic::AtomicUsize,
    /// KV copies the last scan did not read: larger than a commitment can be, or past the per-scan bound. A gauge,
    /// not a count: above 0, detection through the KV is degraded for as long as it stays there.
    pub(crate) scan_skipped: AtomicU64,
    /// Digests of the copies' bytes already counted as unverified (bounded, never cleared).
    unverified_noted: papaya::HashSet<[u8; 32]>,
    /// Digests of the conflicting commitments already counted (bounded, never cleared).
    conflict_noted: papaya::HashSet<[u8; 32]>,
}

impl CommitmentState {
    /// Every commitment this node holds a pin for.
    pub(crate) fn pinned(&self) -> Vec<Arc<CaCommitment>> {
        self.pins.load().iter().map(|(c, _)| Arc::clone(c)).collect()
    }

    /// The commitment this node pinned from the CA whose certificate digest is `predecessor`, if it holds one — what
    /// the succession's phases read the successor from (PR 2b-2; until then only tests ask).
    #[allow(dead_code)] // read by this module's tests, which are built only with `provisional-ca-generation`
    pub(crate) fn pinned_for(&self, predecessor: &[u8; 32]) -> Option<Arc<CaCommitment>> {
        self.pins.load().iter().find(|(c, _)| &c.predecessor == predecessor).map(|(c, _)| Arc::clone(c))
    }

    /// Whether this node holds a commitment whose committing CA it still trusts. **Not** the definition of
    /// `id.ca_succession_committed` (PR 2b-4): this is `false` after narrowing and on a node first started on the
    /// successor's files, and that guarantee must not fail a completed succession.
    #[allow(dead_code)] // read by this module's tests, which are built only with `provisional-ca-generation`
    pub(crate) fn committed(&self) -> bool {
        self.pins.load().iter().any(|(_, active)| *active)
    }

    /// Count a conflicting commitment once per digest, wherever it was seen. `false` when it was counted before, or
    /// when the bound is met (then it is not counted at all — never twice).
    fn conflict_once(&self, digest: [u8; 32]) -> bool {
        note_once(&self.conflict_noted, NOTED_CONFLICTS_MAX, digest) && {
            self.conflicts.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    fn unverified_once(&self, digest: [u8; 32]) -> bool {
        note_once(&self.unverified_noted, NOTED_UNVERIFIED_MAX, digest) && {
            self.unverified.fetch_add(1, Ordering::Relaxed);
            true
        }
    }
}

/// Whether `digest` is new to `set` and was taken: never when it is already there, and never past `max` — the set
/// is not cleared to make room, so nothing noted is ever counted again (#812's review F4).
fn note_once(set: &papaya::HashSet<[u8; 32]>, max: usize, digest: [u8; 32]) -> bool {
    let set = set.pin();
    !set.contains(&digest) && set.len() < max && set.insert(digest)
}

/// **At start, after the TLS identity is loaded and before it is installed:** pin the operator's commitment, or read
/// the pin back ([`mycelium_core::tls::pin_ca_commitment`]). Refuses the start, by name, when this node holds a
/// commitment — a pin, or an operator's file — and no `[persistence]`. A conflicting operator's file is refused,
/// counted and named; the pin stands. Nothing is published here: every refusal comes before this node says anything.
pub(crate) fn at_start(ctx: &TaskCtx, tls_cfg: &mycelium_core::config::TlsConfig, tls: &mycelium_core::tls::NodeTls) -> Result<(), crate::error::GossipError> {
    if ctx.config.persistence.is_none()
        && let Some(found) = mycelium_core::tls::held_ca_commitment(tls_cfg)
    {
        return Err(crate::error::GossipError::InvalidField {
            field: "persistence",
            reason: format!(
                "this node holds a CA commitment ({}) and has no `[persistence]`: a succession's commitments and phases \
                 need a node that remembers across a restart what it no longer trusts. Configure `[persistence]`, or \
                 remove the commitment if this node takes no part in a succession (identity-and-lifecycle I2)",
                found.display()
            ),
        });
    }
    let state = &ctx.ca_commitment;
    let found = mycelium_core::tls::pin_ca_commitment(tls_cfg, &tls.roots_der())?;
    let file = tls_cfg.auto_cert_dir.join(mycelium_core::tls::CA_COMMIT_FILE);
    for pin in found.pins.iter().filter(|p| p.first) {
        tracing::info!(commitment = %hex(&pin.commitment.digest()), ca = %hex(&pin.commitment.predecessor), "CA succession: pinned the commitment the operator placed (ca-commit.pem); this node refuses any other from that CA");
    }
    match found.offered {
        OfferedCommitment::Nothing => {}
        // The operator's file, from a CA this node does not trust (yet): the one unassessable file a node with a pin
        // starts with (`EARLY_COMMITMENT_BESIDE_A_PIN_STARTS`). Its own signal — the KV's unverified copies are a
        // different thing, which any member can raise.
        OfferedCommitment::UntrustedCa => {
            state.file_untrusted_ca.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                file = %file.display(),
                "CA succession: ca-commit.pem holds a commitment from a CA this node does not trust (placed before this node cut over to that CA?); \
                 nothing is pinned from it, this node's pins stand, and the file is read again at the next start"
            );
        }
        OfferedCommitment::Conflict(other) => {
            state.conflict_once(other);
            tracing::error!(
                file = %file.display(), refused = %hex(&other),
                "CA succession: ca-commit.pem holds a second commitment from a CA this node already holds a pin for, signed by that CA's key and naming another successor; it is refused and the pin stands. \
                 Two commitments from one CA mean a mistake or a stolen CA key: compare the file with the issuer's"
            );
        }
    }
    state.pins.store(Arc::new(found.pins.into_iter().map(|p| (Arc::new(p.commitment), p.active)).collect()));
    Ok(())
}

/// **Once the TLS identity is installed** (so the writes are signed as this node's): publish each commitment this
/// node pinned whose committing CA is a root it trusts, at `sys/ca-commit/{self}/{predecessor}`, and register it to be
/// republished when it is overwritten or removed. The statement is this node's own — "this is what I pinned" — and
/// the copy proves itself to a reader that trusts the same CA.
pub(crate) fn publish(ctx: &TaskCtx) {
    for (commitment, active) in ctx.ca_commitment.pins.load().iter() {
        if !*active {
            continue;
        }
        let path = ca_commit_key(&ctx.node_id, &commitment.predecessor);
        let bytes = commitment.to_pem().into_bytes();
        if mycelium_core::ops::kv_get(ctx, &path).as_deref() != Some(bytes.as_slice()) {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(bytes.clone()));
        }
        ctx.identity_ledger.own(path, bytes);
    }
}

/// **Read the commitments in the KV, for detection only.** Each copy under `sys/ca-commit/` is compared with the pin
/// this node holds **for that copy's committing CA**:
/// - a commitment this node pinned is nothing;
/// - a copy that does not verify against this node's roots — malformed, signed by another key, a successor it cannot
///   read — is read as absent and counted once (`ca_commitment_unverified`);
/// - a copy that verifies and differs from the pin this node holds for its CA is a second commitment that CA's key
///   signed: refused and counted once (`ca_commitment_conflict`), naming the key that carries it;
/// - a copy that verifies, from a CA this node holds **no pin for**, is not a conflict and is not counted.
///
/// Nothing read here is ever pinned: a node pins from its operator's file, and a copy — forged or genuine — adds no
/// pin and changes none.
///
/// **Bounded, because any admitted member can write any key here** (#812's review F4, its confirm round C3). A value
/// larger than [`MAX_COPY_BYTES`] is skipped before it is hashed or parsed. Of the rest, a scan reads at most
/// [`MAX_COPIES_PER_SCAN`], in key order **from where the last scan stopped**, wrapping — so keys that sort ahead of a
/// genuine copy delay its reading by a bounded number of scans and never hide it; what a scan left is reported as a
/// gauge (`ca_commitment_scan_skipped`). "Counted once" holds without limit of time: a copy already counted is never
/// counted again, and past [`NOTED_UNVERIFIED_MAX`] distinct unverified copies (or [`NOTED_CONFLICTS_MAX`] distinct
/// conflicts) a further distinct one is not counted at all. Conflicts have their own set, so junk cannot use up the
/// room a conflict is counted in.
pub(crate) fn scan(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let state = &ctx.ca_commitment;
    let pins = state.pinned();
    let roots = tls.roots_der();
    // In key order (`scan_kv_prefix` sorts), so the cursor means the same copies from scan to scan.
    let all = mycelium_core::store::scan_kv_prefix(&ctx.kv_state, CA_COMMIT_PREFIX);
    let oversize = all.iter().filter(|(_, bytes)| bytes.len() > MAX_COPY_BYTES).count();
    let mut copies: Vec<_> = all.into_iter().filter(|(_, bytes)| bytes.len() <= MAX_COPY_BYTES).collect();
    let mut skipped = oversize as u64;
    if copies.len() > MAX_COPIES_PER_SCAN {
        let n = copies.len();
        let start = state.scan_cursor.fetch_add(MAX_COPIES_PER_SCAN, Ordering::Relaxed) % n;
        copies.rotate_left(start);
        copies.truncate(MAX_COPIES_PER_SCAN);
        skipped += (n - MAX_COPIES_PER_SCAN) as u64;
    }
    for (path, bytes) in copies {
        let copy = std::str::from_utf8(&bytes).ok().and_then(|pem| CaCommitment::from_pem(pem).ok());
        if let Some(c) = &copy
            && pins.iter().any(|p| p.digest() == c.digest())
        {
            continue;
        }
        match copy.filter(|c| c.verify(&roots).is_ok()) {
            None => {
                if state.unverified_once(mycelium_core::tls::sha256(&bytes)) {
                    tracing::warn!(key = %path, "CA succession: a commitment in the KV is not one a root this node trusts signed; read as absent");
                }
            }
            Some(c) => {
                // Compared with the pin held for this copy's CA; with none, there is nothing it contradicts.
                let Some(p) = pins.iter().find(|p| p.predecessor == c.predecessor) else { continue };
                if state.conflict_once(c.digest()) {
                    tracing::error!(
                        key = %path, pinned = %hex(&p.digest()), refused = %hex(&c.digest()),
                        "CA succession: the KV carries a second commitment from a CA this node holds a pin for, signed by that CA's key and naming another successor; it is refused and the pin stands. \
                         Two commitments from one CA mean a mistake or a stolen CA key"
                    );
                }
            }
        }
    }
    let before = state.scan_skipped.swap(skipped, Ordering::AcqRel);
    if skipped > 0 && before == 0 {
        tracing::warn!(skipped, prefix = CA_COMMIT_PREFIX, "CA succession: copies under the commitment prefix were not read (too large to be a commitment, or past the per-scan bound); detection through the KV is degraded while this lasts");
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
