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
use mycelium_core::tls::{CaCommitment, CommitmentPin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// KV namespace of the commitments nodes publish for detection: `sys/ca-commit/{node}`.
pub const CA_COMMIT_PREFIX: &str = "sys/ca-commit/";

/// Distinct refused or unverified copies remembered, so each is counted and warned once; past it the set is cleared.
const NOTED_MAX: usize = 1_024;

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
    /// Commitments refused because they conflict with the pin: the operator's file at a start, and distinct KV copies.
    pub(crate) conflicts: AtomicU64,
    /// Distinct KV copies read as absent: malformed, or not signed by a root this node trusts.
    pub(crate) unverified: AtomicU64,
    /// Digests (of a commitment, or of a copy's bytes) already counted.
    noted: papaya::HashSet<[u8; 32]>,
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

    fn note_once(&self, digest: [u8; 32]) -> bool {
        let noted = self.noted.pin();
        if noted.len() >= NOTED_MAX {
            noted.clear();
        }
        noted.insert(digest)
    }
}

/// **At start, after the TLS identity is loaded and before it is installed:** pin the operator's commitment, or read
/// the pin back ([`mycelium_core::tls::pin_ca_commitment`]). Refuses the start, by name, when this node holds a
/// commitment — a pin, or an operator's file — and no `[persistence]`. A conflicting operator's file is refused,
/// counted and named; the pin stands. Nothing is published here: every refusal comes before this node says anything.
pub(crate) fn at_start(ctx: &TaskCtx, tls_cfg: &mycelium_core::config::TlsConfig, tls: &mycelium_core::tls::NodeTls) -> Result<(), crate::error::GossipError> {
    let holds = [
        tls_cfg.auto_cert_dir.join(mycelium_core::tls::CA_COMMIT_FILE),
        tls_cfg.auto_cert_dir.join(mycelium_core::tls::SUCCESSION_DIR).join(mycelium_core::tls::CA_COMMIT_PIN_FILE),
    ];
    if ctx.config.persistence.is_none()
        && let Some(found) = holds.iter().find(|p| p.exists())
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
        CommitmentPin::Pinned { commitment, first, active, refused } => {
            let digest = hex(&commitment.digest());
            if first {
                tracing::info!(commitment = %digest, "CA succession: pinned the commitment the operator placed (ca-commit.pem); this node refuses any other from this CA");
            }
            if let Some(other) = refused {
                state.conflicts.fetch_add(1, Ordering::Relaxed);
                state.note_once(other);
                tracing::error!(
                    pinned = %digest, refused = %hex(&other),
                    "CA succession: ca-commit.pem holds a commitment that conflicts with the one this node pinned; it is refused and the pin stands. \
                     Two commitments from one CA mean a mistake or a stolen CA key: compare the file with the issuer's"
                );
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
/// node's pin: the same commitment is nothing; one that does not verify against this node's roots — malformed, signed
/// by another key, a successor it cannot read — is read as absent and counted once (`ca_commitment_unverified`); a
/// verified one that differs from the pin is refused and counted once (`ca_commitment_conflict`), naming the node
/// whose path carries it. Nothing read here is ever pinned: a node with no pin compares nothing and adopts nothing.
pub(crate) fn scan(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let state = &ctx.ca_commitment;
    let pinned = state.pinned();
    let roots = tls.roots_der();
    for (path, bytes) in mycelium_core::store::scan_kv_prefix(&ctx.kv_state, CA_COMMIT_PREFIX) {
        let copy = std::str::from_utf8(&bytes).ok().and_then(|pem| CaCommitment::from_pem(pem).ok());
        if let (Some(c), Some(p)) = (&copy, &pinned)
            && c.digest() == p.digest()
        {
            continue;
        }
        match copy.filter(|c| c.verify(&roots).is_ok()) {
            None => {
                if state.note_once(mycelium_core::tls::sha256(&bytes)) {
                    state.unverified.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(key = %path, "CA succession: a commitment in the KV is not one a root this node trusts signed; read as absent");
                }
            }
            Some(c) => {
                let Some(p) = &pinned else { continue };
                if state.note_once(c.digest()) {
                    state.conflicts.fetch_add(1, Ordering::Relaxed);
                    tracing::error!(
                        key = %path, pinned = %hex(&p.digest()), refused = %hex(&c.digest()),
                        "CA succession: the KV carries a commitment, signed by the CA's key, that conflicts with the one this node pinned; it is refused and the pin stands. \
                         Two commitments from one CA mean a mistake or a stolen CA key"
                    );
                }
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
