//! Persisted-by-peer replica sync — contracts axis item 1 PR 4b.
//!
//! PR 4a measured that the origin of a write **cannot observe that write's propagation**: an
//! update's `sender` is its originating node across every hop, fan-out excludes the origin, and
//! anti-entropy re-attributes what it delivers to the receiver, so no inbound frame says *a peer
//! holds your write* (`docs/design/contracts-receipts.md` §1a). Watching the gossip stream for
//! evidence therefore cannot work, however the predicate is written.
//!
//! So this asks instead. The origin sends each peer the operation's identity; the peer answers
//! about its **own** state, and the answer is the evidence. Three consequences follow, and each is
//! the reason a simpler design was rejected:
//!
//! - **No wire change.** The exchange rides the existing RPC layer (Layer II individual-scope
//!   signals), so `WIRE_VERSION` is untouched and a mixed-version cluster keeps working — a peer
//!   that does not serve this kind simply never answers, which reads as *unknown*, not *did not
//!   persist*.
//! - **No retained operation status.** The peer answers from live state: does my store hold this
//!   exact stamp and content, and is my WAL synced past it? Nothing is remembered per operation,
//!   so there is no table to size, expire, or explain when a query arrives late. That is the same
//!   conclusion §9a reached for the prepared write, for the same reason.
//! - **No per-entry durability tracking.** Records append in order to one file, so a single
//!   `fdatasync` ([`WalHandle::sync`](mycelium_core::persistence::WalHandle::sync)) establishes
//!   durability for **everything already appended**. Checking the store, then syncing, then
//!   answering is sound without the node knowing when any particular record reached disk.
//!
//! **What an answer establishes, exactly.** [`Answer::Persisted`] from peer P means: at the moment
//! P answered, P's store held this key at this exact HLC stamp with this exact content, and P's WAL
//! `fdatasync` returned `Ok` — so P holds the record across its own crash and restart, because
//! replay restores it. It does **not** mean P will hold it forever: a later write supersedes it
//! like any other value. That is the *replica sync* rung and nothing above it — no destination
//! commit, no claim about any other peer (`docs/design/contracts-receipts.md` §2.1).
//!
//! **What a non-answer establishes.** Nothing. A peer that is unreachable, slow, mid-restart, or
//! running a version without this handler lands in [`ReplicaSync::missing`], which the record
//! defines as *asked and did not report — unknown, not "did not persist"*. A timeout is never a
//! negative anywhere on this axis, and it is not one here.

use bytes::{BufMut, Bytes, BytesMut};
use mycelium_core::receipt::{content_hash, ReplicaSync};
use mycelium_core::NodeId;
use std::{sync::Arc, time::Duration};

use super::TaskCtx;

/// RPC kind for the persisted-by-peer query. Versioned in the name: a future incompatible
/// question is a new kind, never a reinterpreted payload, so an old peer answers the old question
/// or not at all.
pub(crate) const RPC_KIND: &str = "sys.replica-sync.v1";

/// How long the origin waits for one peer. Bounded independently of the caller's overall deadline
/// so a single unreachable peer cannot consume it — the others are asked concurrently.
const PER_PEER_TIMEOUT: Duration = Duration::from_secs(5);

/// Gap between rounds when some peer has not yet answered `Persisted`.
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// What a peer says about one operation. Encoded as a single leading byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The store holds this exact stamp and content, and the WAL sync returned `Ok`.
    Persisted,
    /// The store does not hold this operation — absent, or superseded by a different value.
    /// Says nothing about whether it was ever persisted here.
    NotHeld,
    /// This node has no persistence configured, so it never promised durability for anything.
    /// Distinct from [`Failed`](Self::Failed): nothing was attempted, nothing broke.
    NotConfigured,
    /// The operation is held, but durability could not be established now (the writer is gone, or
    /// the `fdatasync` failed). Denies nothing — the record may well be on disk.
    Failed,
}

impl Answer {
    fn tag(self) -> u8 {
        match self {
            Answer::Persisted => 0,
            Answer::NotHeld => 1,
            Answer::NotConfigured => 2,
            Answer::Failed => 3,
        }
    }
    fn from_tag(b: u8) -> Option<Self> {
        Some(match b {
            0 => Answer::Persisted,
            1 => Answer::NotHeld,
            2 => Answer::NotConfigured,
            3 => Answer::Failed,
            _ => return None,
        })
    }
}

/// The operation's identity as it travels to a peer: stamp, content, key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Query {
    pub(crate) stamp: u64,
    pub(crate) content_hash: u64,
    pub(crate) key: Arc<str>,
}

pub(crate) fn encode_query(q: &Query) -> Bytes {
    let mut b = BytesMut::with_capacity(16 + q.key.len());
    b.put_u64_le(q.stamp);
    b.put_u64_le(q.content_hash);
    b.extend_from_slice(q.key.as_bytes());
    b.freeze()
}

/// Decodes a query. Returns `None` for anything malformed — a short frame or a non-UTF-8 key is
/// refused, never guessed at, so a corrupted question cannot be answered as though it were a
/// different one.
pub(crate) fn decode_query(b: &[u8]) -> Option<Query> {
    if b.len() < 16 {
        return None;
    }
    let stamp = u64::from_le_bytes(b[0..8].try_into().ok()?);
    let content_hash = u64::from_le_bytes(b[8..16].try_into().ok()?);
    let key = std::str::from_utf8(&b[16..]).ok()?;
    if key.is_empty() {
        return None;
    }
    Some(Query { stamp, content_hash, key: Arc::from(key) })
}

pub(crate) fn encode_answer(a: Answer) -> Bytes {
    Bytes::from(vec![a.tag()])
}

pub(crate) fn decode_answer(b: &[u8]) -> Option<Answer> {
    Answer::from_tag(*b.first()?)
}

/// Answers one query from this node's own state.
///
/// Ordering matters and is the point: the store is checked **before** the sync, so a value that
/// arrives between the two cannot be reported as this operation; and the sync happens **after** the
/// match, so `Ok` covers the append that carried it. A `NotHeld` answer short-circuits without
/// touching the disk, which keeps the common case (a superseded key) cheap.
pub(crate) async fn answer(ctx: &TaskCtx, q: &Query) -> Answer {
    let held = {
        let guard = ctx.kv_state.store.pin();
        match guard.get(q.key.as_ref()) {
            None => false,
            Some(entry) => {
                let is_tombstone = entry.data.is_none();
                let value: &[u8] = entry.data.as_deref().unwrap_or(&[]);
                entry.timestamp == q.stamp
                    && content_hash(q.key.as_ref(), value, is_tombstone) == q.content_hash
            }
        }
    };
    if !held {
        return Answer::NotHeld;
    }
    match ctx.wal.get() {
        None => Answer::NotConfigured,
        Some(wal) => match wal.sync().await {
            Ok(()) => Answer::Persisted,
            Err(_) => Answer::Failed,
        },
    }
}

/// Asks every current peer whether it holds `query`, concurrently, and returns the receipt.
///
/// `persisted_by` names only peers that answered [`Answer::Persisted`]; **everyone else is
/// `missing`** — including peers that answered `NotHeld`, `NotConfigured` or `Failed`. That
/// grouping is deliberate: the receipt's two lists are *established* and *unknown*, and none of
/// those three answers establishes that the peer does not hold the operation. `Failed` in
/// particular denies nothing at all.
///
/// The origin is never queried and never counted. Peers that have not answered `Persisted` are
/// re-asked until `deadline`, because the commonest reason for a `NotHeld` immediately after a
/// write is that gossip has not delivered it yet.
pub(crate) async fn collect(ctx: &Arc<TaskCtx>, query: Query, deadline: Duration) -> ReplicaSync {
    let started = tokio::time::Instant::now();
    let payload = encode_query(&query);
    let mut persisted_by: Vec<NodeId> = Vec::new();

    loop {
        // Re-read the peer set each round: a peer that joins mid-window is a peer we should ask,
        // and one that left should not be waited on.
        let outstanding: Vec<NodeId> = ctx
            .peers
            .pin()
            .keys()
            .filter(|p| !persisted_by.contains(p))
            .cloned()
            .collect();
        if outstanding.is_empty() {
            // Either every peer has answered, or there are none. Both are answers.
            return ReplicaSync::new(persisted_by, Vec::new());
        }

        let remaining = deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return ReplicaSync::new(persisted_by, outstanding);
        }
        let per_peer = PER_PEER_TIMEOUT.min(remaining);

        let mut tasks = Vec::with_capacity(outstanding.len());
        for peer in outstanding.clone() {
            let ctx = Arc::clone(ctx);
            let payload = payload.clone();
            tasks.push(tokio::spawn(async move {
                let reply = super::rpc::rpc_call_ctx(
                    &ctx, peer.clone(), Arc::from(RPC_KIND), payload, per_peer,
                )
                .await;
                (peer, reply.ok().and_then(|b| decode_answer(&b)))
            }));
        }
        for t in tasks {
            if let Ok((peer, Some(Answer::Persisted))) = t.await
                && !persisted_by.contains(&peer)
            {
                persisted_by.push(peer);
            }
        }

        let still_out: Vec<NodeId> =
            outstanding.into_iter().filter(|p| !persisted_by.contains(p)).collect();
        if still_out.is_empty() {
            return ReplicaSync::new(persisted_by, Vec::new());
        }
        if started.elapsed() >= deadline {
            return ReplicaSync::new(persisted_by, still_out);
        }
        // A peer that has not answered `Persisted` is very often one the write has not reached
        // yet — gossip is in flight, not broken. Asking again until the deadline is what makes
        // this verb usable immediately after a write, rather than only after some unstated
        // settling time the caller would have to guess at.
        tokio::time::sleep(RETRY_INTERVAL.min(deadline.saturating_sub(started.elapsed()))).await;
    }
}

/// Serves the persisted-by-peer query for the life of the agent.
///
/// Registered by **every** node at start, gateway or not: any node can hold a replica, so any node
/// may be asked. The receiver is [`ServiceHandle::rpc_rx`], which verifies the caller-context
/// envelope at the receive boundary (item 7), so an unverified or forged query is refused there and
/// never reaches this loop.
pub(crate) fn serve(agent: &crate::GossipAgent) {
    let mut rx = agent.service().rpc_rx(RPC_KIND);
    let ctx = Arc::clone(&agent.service().ctx);
    // Detached on purpose: the loop ends when `rpc_rx` yields `None` at shutdown, so there is
    // nothing to join and no handle worth keeping.
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let payload = req.payload();
            let Some(q) = decode_query(&payload) else {
                // A malformed question is refused, not guessed at. Answering `NotHeld` would be a
                // claim about a key we could not read.
                continue;
            };
            let a = answer(&ctx, &q).await;
            super::rpc::rpc_respond_ctx(&ctx, &req, encode_answer(a));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_round_trips_and_binds_all_three_fields() {
        let q = Query { stamp: 0x0102_0304_0506_0708, content_hash: 0xdead_beef_cafe_f00d,
                        key: Arc::from("k/one") };
        let decoded = decode_query(&encode_query(&q)).expect("round trip");
        assert_eq!(decoded, q);
    }

    #[test]
    fn a_malformed_query_is_refused_never_guessed() {
        assert!(decode_query(&[]).is_none(), "empty");
        assert!(decode_query(&[0u8; 15]).is_none(), "short of the fixed header");
        assert!(decode_query(&[0u8; 16]).is_none(), "header but no key");
        let mut bad = vec![0u8; 16];
        bad.extend_from_slice(&[0xff, 0xfe]);       // not UTF-8
        assert!(decode_query(&bad).is_none(), "non-UTF-8 key");
    }

    #[test]
    fn every_answer_round_trips() {
        for a in [Answer::Persisted, Answer::NotHeld, Answer::NotConfigured, Answer::Failed] {
            assert_eq!(decode_answer(&encode_answer(a)), Some(a));
        }
        assert_eq!(decode_answer(&[]), None, "an empty reply is not an answer");
        assert_eq!(decode_answer(&[9]), None, "an unknown tag is not an answer");
    }
}
