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
//! - **A second commitment is refused and counted**, wherever it is seen: in the operator's file at a later start,
//!   or in the KV.
//! - **The KV carries commitments for detection only.** A node publishes the commitment it pinned at
//!   `sys/ca-commit/{node}` — the statement "this is what I pinned", in its own path. A reader verifies a copy against
//!   its own roots (the old CA signed it, or it is read as absent and counted), compares it with its pin, and counts
//!   a conflict: two commitments the old CA's key signed, which is what a stolen key produces. **A KV copy never
//!   becomes the source of a pin**: a node with no pin reads every copy and pins nothing.
//!
//! Nothing here changes what a node trusts. The record the successor signs, and the phases that act on it, are the
//! row's next PR; they read the successor from [`CommitmentState::pinned`], and from nowhere else.

use super::TaskCtx;
use mycelium_core::node_id::NodeId;
use mycelium_core::tls::{CaCommitment, CommitmentPin, OfferedCommitment};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// KV namespace of the commitments nodes publish for detection: `sys/ca-commit/{node}`.
pub const CA_COMMIT_PREFIX: &str = "sys/ca-commit/";

/// A copy larger than this is not a commitment (one is under 2 KiB) and is skipped unread: any admitted member can
/// write a value of megabytes under the prefix, and the scan must not hash it.
const MAX_COPY_BYTES: usize = 4_096;
/// Copies read in one scan, in key order; the rest are skipped and the scan says how many
/// (`ca_commitment_scan_skipped`).
const MAX_COPIES_PER_SCAN: usize = 1_024;
/// Distinct unverified copies counted in a process's life. Past it a further distinct one is **not counted** (and
/// nothing is ever counted twice): the counter then reads "at least this many".
const NOTED_UNVERIFIED_MAX: usize = 1_024;
/// Distinct conflicting commitments counted in a process's life; past it, as above. Each needs the CA's own key.
const NOTED_CONFLICTS_MAX: usize = 64;

/// The key a node publishes its pinned commitment at.
pub fn ca_commit_key(node: &NodeId) -> String {
    format!("{CA_COMMIT_PREFIX}{node}")
}

/// This node's commitment: its pin, and what it has refused. Lock-free (`ArcSwap`, papaya, atomics): no lock-order row.
#[derive(Default)]
pub(crate) struct CommitmentState {
    /// The commitment this node pinned, set once at start from the operator's file or the persisted pin — never from
    /// the KV.
    pinned: arc_swap::ArcSwapOption<CaCommitment>,
    /// Whether the pin's committing CA is still a root this node trusts (see [`CommitmentPin::Pinned`]).
    active: std::sync::atomic::AtomicBool,
    /// Second commitments the pinned CA's key signed, refused: distinct ones, from the operator's file at a start
    /// and from KV copies.
    pub(crate) conflicts: AtomicU64,
    /// Documents read as absent — malformed, or not signed by a root this node trusts: distinct KV copies, and the
    /// operator's file beside a pin at a start.
    pub(crate) unverified: AtomicU64,
    /// KV copies the last scan did not read: larger than a commitment can be, or past the per-scan bound. A gauge,
    /// not a count: above 0, detection through the KV is degraded for as long as it stays there.
    pub(crate) scan_skipped: AtomicU64,
    /// Digests of the copies' bytes already counted as unverified (bounded, never cleared).
    unverified_noted: papaya::HashSet<[u8; 32]>,
    /// Digests of the conflicting commitments already counted (bounded, never cleared).
    conflict_noted: papaya::HashSet<[u8; 32]>,
}

impl CommitmentState {
    /// The pinned commitment, if this node holds one.
    pub(crate) fn pinned(&self) -> Option<Arc<CaCommitment>> {
        self.pinned.load_full()
    }

    /// Whether this node holds a commitment whose committing CA it still trusts — `id.ca_succession_committed`.
    pub(crate) fn committed(&self) -> bool {
        self.pinned.load().is_some() && self.active.load(Ordering::Acquire)
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
    match mycelium_core::tls::pin_ca_commitment(tls_cfg, &tls.roots_der())? {
        CommitmentPin::None => {}
        CommitmentPin::Pinned { commitment, first, active, offered } => {
            let digest = hex(&commitment.digest());
            if first {
                tracing::info!(commitment = %digest, "CA succession: pinned the commitment the operator placed (ca-commit.pem); this node refuses any other from this CA");
            }
            let file = tls_cfg.auto_cert_dir.join(mycelium_core::tls::CA_COMMIT_FILE);
            match offered {
                OfferedCommitment::Nothing => {}
                // Not a second commitment from the CA: nothing shows the CA's key signed it (#812's review F3).
                OfferedCommitment::Unverified(why) => {
                    state.unverified.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        file = %file.display(), pinned = %digest, %why,
                        "CA succession: ca-commit.pem holds a document this node cannot assess against its roots; it is read as absent and the pin stands"
                    );
                }
                OfferedCommitment::Conflict(other) => {
                    state.conflict_once(other);
                    tracing::error!(
                        file = %file.display(), pinned = %digest, refused = %hex(&other),
                        "CA succession: ca-commit.pem holds a second commitment the pinned CA's key signed, naming another successor; it is refused and the pin stands. \
                         Two commitments from one CA mean a mistake or a stolen CA key: compare the file with the issuer's"
                    );
                }
                // A different promiser. Whether a node pins one commitment per committing CA is not decided (plan row
                // I2, §8): it is not pinned, and it is not a conflict of the pinned CA's.
                OfferedCommitment::AnotherCa(other) => tracing::warn!(
                    file = %file.display(), pinned = %digest, offered = %hex(&other),
                    "CA succession: ca-commit.pem holds a commitment made by another CA than the one this node's pin names; it is not pinned and not counted as a conflict"
                ),
            }
            state.active.store(active, Ordering::Release);
            state.pinned.store(Some(Arc::new(commitment)));
        }
    }
    Ok(())
}

/// **Once the TLS identity is installed** (so the write is signed as this node's): publish the commitment this node
/// pinned at `sys/ca-commit/{self}`, while its committing CA is a root this node trusts, and register it to be
/// republished when it is overwritten or removed. The statement is this node's own — "this is what I pinned" — and
/// the copy proves itself to a reader that trusts the same CA.
pub(crate) fn publish(ctx: &TaskCtx) {
    let state = &ctx.ca_commitment;
    let Some(commitment) = state.pinned().filter(|_| state.committed()) else { return };
    let path = ca_commit_key(&ctx.node_id);
    let bytes = commitment.to_pem().into_bytes();
    if mycelium_core::ops::kv_get(ctx, &path).as_deref() != Some(bytes.as_slice()) {
        let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(bytes.clone()));
    }
    ctx.identity_ledger.own(path, bytes);
}

/// **Read the commitments in the KV, for detection only.** Each copy under `sys/ca-commit/` is compared with this
/// node's pin:
/// - the pinned commitment itself is nothing;
/// - a copy that does not verify against this node's roots — malformed, signed by another key, a successor it cannot
///   read — is read as absent and counted once (`ca_commitment_unverified`);
/// - a copy that verifies, **names the pinned commitment's CA** and differs from the pin is a second commitment that
///   CA's key signed: refused and counted once (`ca_commitment_conflict`), naming the key that carries it;
/// - a copy that verifies and was made by another CA is not a conflict of the pinned CA's, and is not counted (the
///   open question in [`OfferedCommitment::AnotherCa`]).
///
/// Nothing read here is ever pinned: a node with no pin compares nothing and adopts nothing.
///
/// **Bounded, because any admitted member can write any key here** (#812's review F4). A value larger than
/// [`MAX_COPY_BYTES`] is skipped before it is hashed or parsed; at most [`MAX_COPIES_PER_SCAN`] copies are read, in
/// key order; what was skipped is reported as a gauge (`ca_commitment_scan_skipped`). "Counted once" holds without
/// limit of time: a copy already counted is never counted again, and past [`NOTED_UNVERIFIED_MAX`] distinct unverified
/// copies (or [`NOTED_CONFLICTS_MAX`] distinct conflicts) a further distinct one is not counted at all. **What a flood
/// can do, stated:** more than `MAX_COPIES_PER_SCAN` keys under the prefix can keep a genuine conflicting copy outside
/// what a scan reads; the gauge above 0 is how that shows, and the operator's file path is not affected.
pub(crate) fn scan(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let state = &ctx.ca_commitment;
    let pinned = state.pinned();
    let roots = tls.roots_der();
    // In key order (`scan_kv_prefix` sorts), so which copies a bounded scan reads does not change from scan to scan.
    let copies = mycelium_core::store::scan_kv_prefix(&ctx.kv_state, CA_COMMIT_PREFIX);
    let (mut read, mut skipped) = (0usize, 0u64);
    for (path, bytes) in copies {
        if bytes.len() > MAX_COPY_BYTES || read >= MAX_COPIES_PER_SCAN {
            skipped += 1;
            continue;
        }
        read += 1;
        let copy = std::str::from_utf8(&bytes).ok().and_then(|pem| CaCommitment::from_pem(pem).ok());
        if let (Some(c), Some(p)) = (&copy, &pinned)
            && c.digest() == p.digest()
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
                let Some(p) = &pinned else { continue };
                if c.predecessor != p.predecessor {
                    continue;
                }
                if state.conflict_once(c.digest()) {
                    tracing::error!(
                        key = %path, pinned = %hex(&p.digest()), refused = %hex(&c.digest()),
                        "CA succession: the KV carries a second commitment the pinned CA's key signed, naming another successor; it is refused and the pin stands. \
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
