//! RPC wire encoding and primary-side handlers (WS-G / G3 · Phase 3).
//!
//! Kinds are namespaced per board — `blackboard.{ns}.{op}` — so two boards on one agent never share
//! a handler channel. Payloads are compact length-prefixed binary; base64/JSON appears only at the
//! HTTP gateway boundary (Phase 4).
//!
//! **Replication is `Post`/`Ack`-only.** A `Claim` (and `Release`) does not change a mirror's
//! *liveness*: a claimed-but-unacked fact stays claimable in the mirror, which is exactly the
//! at-least-once re-queue a promotion wants. So the primary replicates only `Post` (a new fact) and
//! `Ack` (a consumed fact); the mirror is always a complete live view with no byte-offset replay.

use bytes::Bytes;
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{AttrMatch, Blackboard, Fact, Predicate};

pub(crate) const ST_OK: u8 = 0;
/// A `post` refused at admission (item 4 PR 5): `[ST_BACKPRESSURE][available u64][high_watermark u64]`.
/// Code 2 was unused; a pre-2.8 client decodes it through `status_err` as a generic `Rpc` error,
/// which is the same refusal with less detail, never a false success.
pub(crate) const ST_BACKPRESSURE: u8 = 2;
pub(crate) const ST_NOT_FOUND: u8 = 3;
pub(crate) const ST_ERR: u8 = 4;

const REP_POST: u8 = 1;
const REP_ACK: u8 = 2;

pub(crate) fn rpc_kind(ns: &str, op: &str) -> Arc<str> {
    Arc::from(format!("blackboard.{ns}.{op}"))
}

// ── Primitive codecs ─────────────────────────────────────────────────────────

fn put_str(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u16).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
}
fn get_str(p: &[u8], off: &mut usize) -> Option<String> {
    let len = u16::from_le_bytes(p.get(*off..*off + 2)?.try_into().ok()?) as usize;
    *off += 2;
    let s = std::str::from_utf8(p.get(*off..*off + len)?).ok()?.to_string();
    *off += len;
    Some(s)
}
fn put_payload(buf: &mut Vec<u8>, b: &[u8]) {
    buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
    buf.extend_from_slice(b);
}
fn get_payload(p: &[u8], off: &mut usize) -> Option<Bytes> {
    let len = u32::from_le_bytes(p.get(*off..*off + 4)?.try_into().ok()?) as usize;
    *off += 4;
    let b = Bytes::copy_from_slice(p.get(*off..*off + len)?);
    *off += len;
    Some(b)
}
fn put_attrs(buf: &mut Vec<u8>, attrs: &BTreeMap<String, String>) {
    buf.extend_from_slice(&(attrs.len() as u16).to_le_bytes());
    for (k, v) in attrs {
        put_str(buf, k);
        put_str(buf, v);
    }
}
fn get_attrs(p: &[u8], off: &mut usize) -> Option<BTreeMap<String, String>> {
    let n = u16::from_le_bytes(p.get(*off..*off + 2)?.try_into().ok()?) as usize;
    *off += 2;
    let mut m = BTreeMap::new();
    for _ in 0..n {
        let k = get_str(p, off)?;
        let v = get_str(p, off)?;
        m.insert(k, v);
    }
    Some(m)
}
fn put_predicate(buf: &mut Vec<u8>, pred: &Predicate) {
    buf.extend_from_slice(&(pred.attrs.len() as u16).to_le_bytes());
    for (k, m) in &pred.attrs {
        put_str(buf, k);
        match m {
            AttrMatch::Present => buf.push(0),
            AttrMatch::Equals(v) => {
                buf.push(1);
                put_str(buf, v);
            }
        }
    }
}
fn get_predicate(p: &[u8], off: &mut usize) -> Option<Predicate> {
    let n = u16::from_le_bytes(p.get(*off..*off + 2)?.try_into().ok()?) as usize;
    *off += 2;
    let mut pred = Predicate::new();
    for _ in 0..n {
        let k = get_str(p, off)?;
        let kind = *p.get(*off)?;
        *off += 1;
        match kind {
            0 => pred = pred.present(k),
            1 => {
                let v = get_str(p, off)?;
                pred = pred.eq(k, v);
            }
            _ => return None,
        }
    }
    Some(pred)
}
fn put_fact(buf: &mut Vec<u8>, f: &Fact) {
    buf.extend_from_slice(&f.id.to_le_bytes());
    put_attrs(buf, &f.attributes);
    put_payload(buf, &f.payload);
}
fn get_fact(p: &[u8], off: &mut usize) -> Option<Fact> {
    let id = u64::from_le_bytes(p.get(*off..*off + 8)?.try_into().ok()?);
    *off += 8;
    let attributes = get_attrs(p, off)?;
    let payload = get_payload(p, off)?;
    Some(Fact { id, attributes, payload })
}

// ── Request encoders (client side) ───────────────────────────────────────────

pub(crate) fn enc_post_req(attrs: &BTreeMap<String, String>, payload: &Bytes) -> Bytes {
    let mut buf = Vec::new();
    put_attrs(&mut buf, attrs);
    put_payload(&mut buf, payload);
    Bytes::from(buf)
}
pub(crate) fn enc_predicate_req(pred: &Predicate) -> Bytes {
    let mut buf = Vec::new();
    put_predicate(&mut buf, pred);
    Bytes::from(buf)
}
pub(crate) fn enc_id_req(id: u64) -> Bytes {
    Bytes::copy_from_slice(&id.to_le_bytes())
}
pub(crate) fn enc_replicate_post(f: &Fact) -> Bytes {
    let mut buf = vec![REP_POST];
    put_fact(&mut buf, f);
    Bytes::from(buf)
}
pub(crate) fn enc_replicate_ack(id: u64) -> Bytes {
    let mut buf = vec![REP_ACK];
    buf.extend_from_slice(&id.to_le_bytes());
    Bytes::from(buf)
}

// ── Response decoders (client side) ──────────────────────────────────────────

pub(crate) fn dec_id_resp(resp: &Bytes) -> Result<u64, crate::BlackboardError> {
    match resp.first() {
        Some(&ST_OK) => {
            let b: [u8; 8] = resp.get(1..9).and_then(|s| s.try_into().ok())
                .ok_or_else(|| crate::BlackboardError::Rpc("malformed id response".into()))?;
            Ok(u64::from_le_bytes(b))
        }
        _ => Err(status_err(resp)),
    }
}
pub(crate) fn dec_unit_resp(resp: &Bytes) -> Result<(), crate::BlackboardError> {
    match resp.first() {
        Some(&ST_OK) => Ok(()),
        Some(&ST_NOT_FOUND) => Err(crate::BlackboardError::NotFound),
        _ => Err(status_err(resp)),
    }
}
pub(crate) fn dec_facts_resp(resp: &Bytes) -> Result<Vec<Fact>, crate::BlackboardError> {
    dec_facts_at(resp).map(|(facts, _)| facts)
}

/// The facts of a facts response and the offset after them. Bytes after the facts are not read
/// here — a pre-ER6 decoder stopped at the same place, which is why the snapshot trailer is safe.
fn dec_facts_at(resp: &Bytes) -> Result<(Vec<Fact>, usize), crate::BlackboardError> {
    if resp.first() != Some(&ST_OK) {
        return Err(status_err(resp));
    }
    let mut off = 1;
    let n = u32::from_le_bytes(resp.get(off..off + 4)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| crate::BlackboardError::Rpc("malformed facts response".into()))?) as usize;
    off += 4;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(get_fact(resp, &mut off).ok_or_else(|| crate::BlackboardError::Rpc("malformed fact".into()))?);
    }
    Ok((out, off))
}

/// The snapshot trailer's extension tag (ER6): `[u8 1][u64 next_id][u32 n] n × ([u64 start][u32 len])`.
const SNAP_EXT_FENCE: u8 = 1;
/// The largest `next_id` a trailer may carry: ids stay below 2^53 (ER3, `fresh_id_base`).
const MAX_TRAILER_NEXT_ID: u64 = 1 << 53;
/// The digest extension (ER6 PR B), after the trailer of a digest-only answer:
/// `[u8 3][u64 live count][u64 xor of the live ids' mixes]`. Tag 3, not 2: a development build of
/// PR B sent tag 2 with a replication sequence first, and a mirror must not read that as this.
const SNAP_EXT_DIGEST: u8 = 3;
/// A `snapshot` request body asking for the digest only (ER6 PR B). A primary older than PR B
/// ignores the body and answers a full snapshot, which the mirror applies — still correct.
pub(crate) const SNAP_REQ_DIGEST: u8 = 1;

/// A primary's live-set digest (ER6 PR B).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotDigest {
    pub(crate) count: u64,
    pub(crate) mix: u64,
}

/// A decoded `snapshot` response: the facts, the trailer from an ER6 primary, the digest from a
/// digest-only answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecodedSnapshot {
    pub(crate) facts: Vec<Fact>,
    pub(crate) trailer: Option<SnapshotTrailer>,
    pub(crate) digest: Option<SnapshotDigest>,
}

/// The digest-only answer: no facts, the trailer with no acked runs (`next_id` refreshes the
/// mirror's fence on every probe), then the digest.
pub(crate) fn digest_to_bytes(next_id: u64, digest: SnapshotDigest) -> Vec<u8> {
    let mut buf = snapshot_to_bytes(&crate::store::MirrorSnapshot { facts: Vec::new(), next_id, acked: Vec::new() });
    buf.push(SNAP_EXT_DIGEST);
    buf.extend_from_slice(&digest.count.to_le_bytes());
    buf.extend_from_slice(&digest.mix.to_le_bytes());
    buf
}

/// What a primary's snapshot says beyond its live facts (ER6): the id it issues next, and the acked
/// ids it remembers, as sorted runs. `None` from a primary older than ER6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotTrailer {
    pub(crate) next_id: u64,
    pub(crate) acked: Vec<(u64, u32)>,
}

impl SnapshotTrailer {
    /// Every acked id the runs name.
    pub(crate) fn acked_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.acked.iter().flat_map(|(start, len)| (0..u64::from(*len)).filter_map(move |i| start.checked_add(i)))
    }
}

/// The `snapshot` response: the facts as `read` answers them, then the trailer — on the snapshot
/// only; `read` shares `facts_to_bytes` and stays as it was.
pub(crate) fn snapshot_to_bytes(snap: &crate::store::MirrorSnapshot) -> Vec<u8> {
    let mut buf = facts_to_bytes(&snap.facts);
    buf.push(SNAP_EXT_FENCE);
    buf.extend_from_slice(&snap.next_id.to_le_bytes());
    buf.extend_from_slice(&(snap.acked.len() as u32).to_le_bytes());
    for (start, len) in &snap.acked {
        buf.extend_from_slice(&start.to_le_bytes());
        buf.extend_from_slice(&len.to_le_bytes());
    }
    buf
}

/// Decode a `snapshot` response: the facts, and the trailer when the primary sent one. No bytes
/// after the facts (a primary older than ER6), or an extension this build does not know, is `None`
/// — the mirror then adds facts as it always did. A trailer that starts and is cut short is an error.
pub(crate) fn dec_snapshot_resp(resp: &Bytes) -> Result<DecodedSnapshot, crate::BlackboardError> {
    let (facts, mut off) = dec_facts_at(resp)?;
    if resp.get(off) != Some(&SNAP_EXT_FENCE) {
        return Ok(DecodedSnapshot { facts, trailer: None, digest: None });
    }
    off += 1;
    let bad = || crate::BlackboardError::Rpc("malformed snapshot trailer".into());
    let u64_at = |at: usize| resp.get(at..at + 8).and_then(|s| s.try_into().ok()).map(u64::from_le_bytes);
    let u32_at = |at: usize| resp.get(at..at + 4).and_then(|s| s.try_into().ok()).map(u32::from_le_bytes);
    let next_id = u64_at(off).ok_or_else(bad)?;
    // Ids stay below 2^53 (ER3); a larger fence would wrap the mirror's `post` (review I2).
    if next_id > MAX_TRAILER_NEXT_ID {
        return Err(bad());
    }
    let n = u32_at(off + 8).ok_or_else(bad)? as usize;
    off += 12;
    if resp.len() < off.saturating_add(n.saturating_mul(12)) {
        return Err(bad());
    }
    // No honest primary names more ids than a board remembers; a run's `u32` length would
    // otherwise turn 12 bytes into billions of discards and WAL appends (review I2).
    let mut named = 0u64;
    let mut acked = Vec::with_capacity(n);
    for _ in 0..n {
        let (start, len) = (u64_at(off).ok_or_else(bad)?, u32_at(off + 8).ok_or_else(bad)?);
        named += u64::from(len);
        if named > crate::wal::TOMBSTONE_MEMORY as u64 {
            return Err(bad());
        }
        acked.push((start, len));
        off += 12;
    }
    let trailer = Some(SnapshotTrailer { next_id, acked });
    if resp.get(off) != Some(&SNAP_EXT_DIGEST) {
        return Ok(DecodedSnapshot { facts, trailer, digest: None });
    }
    off += 1;
    let digest = SnapshotDigest { count: u64_at(off).ok_or_else(bad)?, mix: u64_at(off + 8).ok_or_else(bad)? };
    Ok(DecodedSnapshot { facts, trailer, digest: Some(digest) })
}
/// `[ST_OK][u8 present][fact?]` → `Option<Fact>`.
pub(crate) fn dec_opt_fact_resp(resp: &Bytes) -> Result<Option<Fact>, crate::BlackboardError> {
    if resp.first() != Some(&ST_OK) {
        return Err(status_err(resp));
    }
    match resp.get(1) {
        Some(0) => Ok(None),
        Some(1) => {
            let mut off = 2;
            Ok(Some(get_fact(resp, &mut off).ok_or_else(|| crate::BlackboardError::Rpc("malformed claim".into()))?))
        }
        _ => Err(crate::BlackboardError::Rpc("malformed claim response".into())),
    }
}

fn status_err(resp: &Bytes) -> crate::BlackboardError {
    match resp.first() {
        Some(&ST_NOT_FOUND) => crate::BlackboardError::NotFound,
        Some(&ST_BACKPRESSURE) => {
            let word = |at: usize| resp.get(at..at + 8).and_then(|s| s.try_into().ok()).map(u64::from_le_bytes);
            match (word(1), word(9)) {
                (Some(available), Some(high_watermark)) => {
                    crate::BlackboardError::Backpressure { available, high_watermark }
                }
                _ => crate::BlackboardError::Rpc("malformed backpressure response".into()),
            }
        }
        _ => crate::BlackboardError::Rpc("board primary returned error".into()),
    }
}

fn facts_to_bytes(facts: &[Fact]) -> Vec<u8> {
    let mut buf = vec![ST_OK];
    buf.extend_from_slice(&(facts.len() as u32).to_le_bytes());
    for f in facts {
        put_fact(&mut buf, f);
    }
    buf
}

// ── Primary handlers ─────────────────────────────────────────────────────────

pub(crate) fn spawn_primary_handlers(bb: &Arc<Blackboard>) -> Vec<tokio::task::JoinHandle<()>> {
    let ns = &bb.cfg().namespace;
    let mut handles = Vec::with_capacity(6);

    macro_rules! handler {
        ($op:literal, $req:ident, $body:expr) => {{
            let me = Arc::clone(bb);
            let mut rx = bb.agent().service().rpc_rx(rpc_kind(ns, $op));
            handles.push(tokio::spawn(async move {
                while let Some($req) = rx.recv().await {
                    let reply: Vec<u8> = $body(&me, &$req);
                    me.agent().service().rpc_respond(&$req, Bytes::from(reply));
                }
            }));
        }};
    }

    handler!("post", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        let p = req.payload();
        let mut off = 0;
        match (get_attrs(&p, &mut off), get_payload(&p, &mut off)) {
            (Some(attrs), Some(payload)) => match me.serve_post(attrs, payload) {
                Ok(id) => { let mut b = vec![ST_OK]; b.extend_from_slice(&id.to_le_bytes()); b }
                Err(crate::BlackboardError::Backpressure { available, high_watermark }) => {
                    let mut b = vec![ST_BACKPRESSURE];
                    b.extend_from_slice(&available.to_le_bytes());
                    b.extend_from_slice(&high_watermark.to_le_bytes());
                    b
                }
                Err(_) => vec![ST_ERR],
            },
            _ => vec![ST_ERR],
        }
    });

    handler!("read", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        let p = req.payload();
        let mut off = 0;
        match get_predicate(&p, &mut off) {
            Some(pred) => facts_to_bytes(&me.serve_read(&pred)),
            None => vec![ST_ERR],
        }
    });

    handler!("claim", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        let p = req.payload();
        let mut off = 0;
        match get_predicate(&p, &mut off) {
            Some(pred) => match me.serve_claim(&pred) {
                Ok(Some(f)) => { let mut b = vec![ST_OK, 1]; put_fact(&mut b, &f); b }
                Ok(None) => vec![ST_OK, 0],
                Err(_) => vec![ST_ERR],
            },
            None => vec![ST_ERR],
        }
    });

    handler!("ack", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        let p = req.payload();
        match p.get(..8).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
            Some(b) => match me.serve_ack(u64::from_le_bytes(b)) {
                Ok(()) => vec![ST_OK],
                Err(crate::BlackboardError::NotFound) => vec![ST_NOT_FOUND],
                Err(_) => vec![ST_ERR],
            },
            None => vec![ST_ERR],
        }
    });

    handler!("release", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        let p = req.payload();
        match p.get(..8).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
            Some(b) => match me.serve_release(u64::from_le_bytes(b)) {
                Ok(()) => vec![ST_OK],
                Err(crate::BlackboardError::NotFound) => vec![ST_NOT_FOUND],
                Err(_) => vec![ST_ERR],
            },
            None => vec![ST_ERR],
        }
    });

    handler!("snapshot", req, |me: &Arc<Blackboard>, req: &mycelium::RpcRequest| {
        match me.store() {
            // A digest probe (ER6 PR B).
            Some(store) if req.payload().as_ref() == [SNAP_REQ_DIGEST] => {
                let (count, mix) = store.live_digest(&std::collections::HashSet::new());
                digest_to_bytes(store.mirror_snapshot_next_id(), SnapshotDigest { count, mix })
            }
            Some(store) => snapshot_to_bytes(&store.mirror_snapshot()),
            None => vec![ST_ERR],
        }
    });

    handler!("depth", req, |me: &Arc<Blackboard>, _req: &mycelium::RpcRequest| {
        match me.store() {
            Some(store) => {
                let d = store.depth();
                let mut b = vec![ST_OK];
                b.extend_from_slice(&d.available.to_le_bytes());
                b.extend_from_slice(&d.inflight.to_le_bytes());
                b
            }
            None => vec![ST_ERR],
        }
    });

    handles
}

pub(crate) fn dec_depth_resp(resp: &Bytes) -> Result<crate::BoardDepth, crate::BlackboardError> {
    if resp.first() != Some(&ST_OK) {
        return Err(status_err(resp));
    }
    let available = u64::from_le_bytes(resp.get(1..9).and_then(|s| s.try_into().ok())
        .ok_or_else(|| crate::BlackboardError::Rpc("malformed depth".into()))?);
    let inflight = u64::from_le_bytes(resp.get(9..17).and_then(|s| s.try_into().ok())
        .ok_or_else(|| crate::BlackboardError::Rpc("malformed depth".into()))?);
    Ok(crate::BoardDepth { available, inflight })
}

/// The mirror's `replicate` handler: applies `Post`/`Ack` shipped by the live primary.
pub(crate) fn spawn_mirror_handlers(bb: &Arc<Blackboard>) -> Vec<tokio::task::JoinHandle<()>> {
    let ns = &bb.cfg().namespace;
    let me = Arc::clone(bb);
    let mut rx = bb.agent().service().rpc_rx(rpc_kind(ns, "replicate"));
    vec![tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let p = req.payload();
            me.note_primary_seen(req.sender());
            me.apply_replicated(&p);
            me.agent().service().rpc_respond(&req, Bytes::from_static(&[ST_OK]));
        }
    })]
}

impl Blackboard {
    /// Apply one replicated record (`Post` or `Ack`) into the mirror store.
    pub(crate) fn apply_replicated(&self, p: &[u8]) {
        let Some(store) = self.store() else { return };
        match p.first() {
            Some(&REP_POST) => {
                let mut off = 1;
                // The store dedupes a re-delivery and refuses a remembered acked id (ER6).
                if let Some(f) = get_fact(p, &mut off) {
                    let id = f.id;
                    if let Err(e) = store.post_with_id(id, f.attributes, f.payload) {
                        self.note_mirror_gap(id, &e);
                    }
                }
            }
            Some(&REP_ACK) => {
                if let Some(b) = p.get(1..9).and_then(|s| <[u8; 8]>::try_from(s).ok()) {
                    let id = u64::from_le_bytes(b);
                    // The primary acked it too: no longer a refused-but-live id (review I1).
                    self.forget_refused_live(id);
                    // Remembered as acked whether or not the fact is here, so a `Post` arriving after
                    // it is refused (ER6). A refused `Ack` record leaves the fact live in the log (not
                    // counted): a restart re-queues it — at-least-once, never a loss.
                    if let Err(e) = store.discard(id) {
                        tracing::error!(id, error = %e, "blackboard: mirror ack not logged");
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;

    /// Item 4 PR 5: the refusal crosses the RPC as itself — available and watermark intact — and a
    /// truncated frame is a malformed response, never a false success.
    #[test]
    fn a_backpressure_status_round_trips_with_its_numbers() {
        let mut b = vec![ST_BACKPRESSURE];
        b.extend_from_slice(&7u64.to_le_bytes());
        b.extend_from_slice(&5u64.to_le_bytes());
        match dec_id_resp(&Bytes::from(b.clone())) {
            Err(crate::BlackboardError::Backpressure { available, high_watermark }) => {
                assert_eq!((available, high_watermark), (7, 5))
            }
            other => panic!("expected the refusal with its numbers, got {other:?}"),
        }
        b.truncate(9);
        assert!(matches!(dec_id_resp(&Bytes::from(b)), Err(crate::BlackboardError::Rpc(_))), "truncated is malformed");
    }
}

#[cfg(test)]
mod snapshot_trailer_tests {
    use super::*;
    use crate::store::MirrorSnapshot;

    fn facts() -> Vec<Fact> {
        vec![Fact { id: 7, attributes: BTreeMap::from([("k".to_string(), "v".to_string())]), payload: Bytes::from("p") }]
    }

    /// ER6, a mixed pair: a pre-ER6 mirror decodes a new primary's snapshot with `dec_facts_resp`,
    /// which stops after the facts — the trailer is ignored, the facts are the same.
    #[test]
    fn an_old_mirror_ignores_the_trailer() {
        let snap = MirrorSnapshot { facts: facts(), next_id: 99, acked: vec![(8, 3)] };
        let bytes = Bytes::from(snapshot_to_bytes(&snap));
        assert_eq!(dec_facts_resp(&bytes).unwrap(), facts());
        let got = dec_snapshot_resp(&bytes).unwrap();
        assert_eq!(got.facts, facts());
        assert_eq!(got.digest, None);
        let trailer = got.trailer.expect("the trailer");
        assert_eq!(trailer.next_id, 99);
        assert_eq!(trailer.acked_ids().collect::<Vec<_>>(), vec![8, 9, 10]);
    }

    /// ER6, the other direction: a pre-ER6 primary's snapshot has no trailer; the new mirror reads
    /// the facts and no trailer (it then only adds, as before). An unknown extension is ignored; a
    /// trailer cut short is malformed, never a short memory.
    #[test]
    fn an_old_snapshot_without_a_trailer_still_applies() {
        let old = Bytes::from(facts_to_bytes(&facts()));
        let plain = DecodedSnapshot { facts: facts(), trailer: None, digest: None };
        assert_eq!(dec_snapshot_resp(&old).unwrap(), plain);
        let mut unknown = facts_to_bytes(&facts());
        unknown.extend_from_slice(&[9, 1, 2, 3]);
        assert_eq!(dec_snapshot_resp(&Bytes::from(unknown)).unwrap(), plain);
        let mut cut = snapshot_to_bytes(&MirrorSnapshot { facts: facts(), next_id: 99, acked: vec![(8, 3)] });
        cut.truncate(cut.len() - 1);
        assert!(dec_snapshot_resp(&Bytes::from(cut)).is_err());
    }

    /// Review I2: one 12-byte run `(0, u32::MAX)` named about 4.3 billion ids for the mirror to
    /// discard and log. A trailer naming more ids than any board remembers is refused.
    #[test]
    fn a_trailer_naming_more_than_the_memory_is_refused() {
        let forged = snapshot_to_bytes(&MirrorSnapshot { facts: Vec::new(), next_id: 10, acked: vec![(0, u32::MAX)] });
        assert!(dec_snapshot_resp(&Bytes::from(forged)).is_err());
        let over = (crate::wal::TOMBSTONE_MEMORY as u32) + 1;
        let forged = snapshot_to_bytes(&MirrorSnapshot { facts: Vec::new(), next_id: 10, acked: vec![(0, over - 1), (1 << 40, 1)] });
        assert!(dec_snapshot_resp(&Bytes::from(forged)).is_err(), "summed across runs");
        let honest = snapshot_to_bytes(&MirrorSnapshot { facts: Vec::new(), next_id: 10, acked: vec![(0, over - 1)] });
        assert!(dec_snapshot_resp(&Bytes::from(honest)).is_ok());
    }

    /// Review I2: a trailer's `next_id` of `u64::MAX` made the mirror persist `Claim{u64::MAX-1}`
    /// and its next `post` wrap to 0. Ids stay below 2^53 (ER3); a larger fence is refused.
    #[test]
    fn a_trailer_next_id_beyond_two_to_the_53_is_refused() {
        for next_id in [u64::MAX, (1u64 << 53) + 1] {
            let forged = snapshot_to_bytes(&MirrorSnapshot { facts: Vec::new(), next_id, acked: Vec::new() });
            assert!(dec_snapshot_resp(&Bytes::from(forged)).is_err(), "{next_id}");
        }
        let edge = snapshot_to_bytes(&MirrorSnapshot { facts: Vec::new(), next_id: 1u64 << 53, acked: Vec::new() });
        assert!(dec_snapshot_resp(&Bytes::from(edge)).is_ok());
    }

    /// ER6 PR B: the digest-only answer round-trips — no facts, the fence, the digest — and a
    /// pre-ER6 mirror reading it sees an empty facts list (it never asks for one).
    #[test]
    fn a_digest_answer_round_trips() {
        let d = SnapshotDigest { count: 2, mix: 0xdead_beef };
        let bytes = Bytes::from(digest_to_bytes(123, d));
        let got = dec_snapshot_resp(&bytes).unwrap();
        assert!(got.facts.is_empty());
        assert_eq!(got.trailer, Some(SnapshotTrailer { next_id: 123, acked: Vec::new() }));
        assert_eq!(got.digest, Some(d));
        assert!(dec_facts_resp(&bytes).unwrap().is_empty());
        // The first build's tag 2 (with a sequence) is not read as a digest.
        let mut first_build = digest_to_bytes(123, d);
        let tag_at = first_build.len() - 17;
        first_build[tag_at] = 2;
        assert_eq!(dec_snapshot_resp(&Bytes::from(first_build)).unwrap().digest, None);
    }
}

/// External review 2026-10-31, F1 (plan row R1): every operation this board serves as an RPC kind is mapped by the
/// gateway's raw-route gate (`mycelium::companion_kind_scope`) to the scope the board's own route requires. The
/// operations are read from this crate's source — the literal in each `rpc_kind(…, "op")` call and in each
/// `handler!("op", …)` registration — rather than listed a second time, so a verb registered or called here is
/// checked against the table without anyone remembering to. Which verbs are reads (`read`, `depth`, and `snapshot`,
/// the replica-sync read that answers every fact) is this test's own statement, matching the board's route table in
/// `http.rs`; a new read verb needs adding here and in the core table both. Doc-comment lines are skipped so the
/// scan reads code, not prose (#714's review, round 2).
#[cfg(test)]
mod raw_route_scope_tests {
    fn ops_in(src: &str) -> Vec<String> {
        let src = src.lines().filter(|l| !l.trim_start().starts_with("///")).collect::<Vec<_>>().join("\n");
        let src = src.as_str();
        let registered = src.match_indices("handler!(\"").filter_map(|(i, m)| {
            let rest = &src[i + m.len()..];
            Some(rest[..rest.find('"')?].to_string())
        });
        registered.chain(src.match_indices("rpc_kind(")
            .filter_map(|(i, _)| {
                let rest = &src[i..];
                let close = rest.find(')')?;
                let args = &rest[..close];
                let q1 = args.rfind('"')?;
                let q0 = args[..q1].rfind('"')?;
                Some(args[q0 + 1..q1].to_string())
            }))
            .collect()
    }

    #[test]
    fn every_served_operation_maps_to_the_boards_own_scope() {
        let mut ops = ops_in(include_str!("rpc.rs"));
        ops.extend(ops_in(include_str!("lib.rs")));
        ops.sort();
        ops.dedup();
        assert!(ops.len() >= 8, "the scan found too few operations: {ops:?}");
        for op in &ops {
            let kind = super::rpc_kind("ns", op);
            let expected = if matches!(op.as_str(), "read" | "depth" | "snapshot") { "board:read" } else { "board:write" };
            assert_eq!(mycelium::companion_kind_scope(&kind), Some(expected), "{kind}");
        }
    }
}
