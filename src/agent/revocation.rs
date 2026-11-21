//! Key revocation and key standing (WS-D / D1; post-360 plan row Q) — who may revoke a node's key,
//! and which of its keys verify. The decision record is `docs/design/revocation-authority.md` (rev 3);
//! this module is its fold.
//!
//! ## Standing
//!
//! A key has **standing** for node N only through a **certificate record**:
//! `sys/identity-cert/{N}/{key-hex}` = a certificate the fleet CA issued for that key, naming N by a
//! `mycelium://{N}` URI (`mycelium_core::tls::cert_identity`, judged by
//! `NodeTls::verify_member_cert`), with an issuance time (its serial, in ms) no later than
//! `wall_now_ms` plus the skew bound — a later one is counted (`identity_cert_future`) and ignored.
//! A node with such a record is **anchored**, and its trusted keys are **exactly its anchored keys
//! it has not revoked**: a bare or first-sighted identity key, a legacy IP-only handshake anchor, a
//! key from a v1 sealed record alone — none verifies as an anchored node. A node without one is a
//! **legacy** node, judged from `peer_keys` as before row Q, and **no revocation of its keys counts**
//! until it is re-anchored (the window `deprecations.md` §39 states).
//!
//! ## Who may revoke whom
//!
//! A revocation of key P for node N, signed by S, counts iff S is an anchored key of N and either P
//! is not anchored, or S was **first** issued strictly after P was first issued (a later re-issue of a
//! key never raises its rank). A revocation this node has counted stays counted; a node whose own
//! records show its key revoked refuses to start. History (the audit chain) verifies under a separate,
//! verify-only set: every key held for a node minus the keys it revoked (`CoreCtx::history_keys`). Authority flows only to the
//! past: a thief holding an old key cannot revoke a newer one, whatever else it holds, and a node that
//! rotates (`rotate_identity_on_compromise` mints the new key's certificate) or is re-issued a
//! certificate by the operator outranks every older key. Two keys issued in the same millisecond are
//! siblings: neither revokes the other. There are no succession links and no voting among
//! successors — every successor is a certificate, so nothing needs designating.
//!
//! ## Records
//!
//! A revocation lives at `sys/revocation/{N}/{P-hex}/{S-hex}` — keyed `(P, S)`, so a thief's later
//! write or a tombstone never replaces the owner's — as the **v1 bytes unchanged**
//! ([`SignedRevocation`], signed by S) followed by a trailer ([`RevocationTrailer`]: the revoker, the
//! certificates of S and P, a signature by S over both). A 2.32 node decodes the prefix
//! (`serde_fixint::from_slice` ignores trailing bytes) and judges it by its old rule; this node judges
//! it from its own contents, so hiding N's certificate records cannot stop it. A v1 record at the old
//! path `sys/revocation/{N}/{P-hex}` is judged by the same rule, its signer being whichever of N's
//! anchored keys its signature verifies under.
//!
//! ## Order, restart, hiding
//!
//! A revocation seen after a write was verified does not un-apply it (detection, not prevention);
//! every verification after it refuses. The fold recomputes a node's standing as a whole from the
//! store, the certificates carried in revocations and the **journal**: every revocation this node
//! counts is appended to a node-local journal (`src/agent/journal.rs`, when persistence is
//! configured), so a revocation hidden while the node was down still counts after its restart. The
//! journal is bounded (64 per node, 65 536 in all); a node whose budget is full is **saturated** and
//! trusts only its newest anchored key — failing closed, never open. **Kept anchors** (identity-and-lifecycle
//! I2, R4): an anchored key keeps its certificates (at most two, one per issuer), journaled under
//! `[persistence]`, so hiding a certificate record does not demote a node, in-process or across a restart. A
//! kept certificate is removed only when it fails the roots this node trusts now — re-judged at every change of
//! roots and at start — and a node left with none trusts nothing, never reverting to legacy.
//! **The journal of kept anchors is compacted** when its budget is met: a record leaves when its certificate is no
//! longer kept, except the last record of a peer with nothing kept, which is the memory that keeps such a peer from
//! reading as legacy after a restart. Throughout, the journaled set never names a certificate the file does not
//! hold — `anchors_durable_for` rests on it — and [`compact`] argues that against every interleaving of an append,
//! its settle, a rewrite and a change of what is kept.
//! The owner republishes its certificate and its revocations at start, periodically, and whenever its
//! records under these prefixes change (the `sys/` tripwire counts the foreign write).

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::node_id::NodeId;
use mycelium_core::removal::NodeStanding;
use mycelium_core::CoreCtx;
use super::TaskCtx;

/// KV namespace owned by the revocation log: `sys/revocation/{node}/{revoked-key-hex}[/{revoker-hex}]`.
pub const REVOCATION_PREFIX: &str = "sys/revocation/";
/// KV namespace of certificate records: `sys/identity-cert/{node}/{key-hex}`.
pub const CERT_PREFIX: &str = mycelium_core::signal::kv_ns::IDENTITY_CERT;

/// At most this many anchored keys per node are considered, newest first.
pub(crate) const MAX_CERTS_PER_NODE: usize = 16;
/// At most this many revocations of one key are considered, by their signer's issuance, newest first.
pub(crate) const MAX_REVOCATIONS_PER_KEY: usize = 8;
/// Counted revocations journaled per node before the node is saturated.
pub(crate) const JOURNAL_PER_NODE: usize = 64;
/// Counted revocations journaled in all before every further node is saturated.
pub(crate) const JOURNAL_TOTAL: usize = 65_536;
/// Certificates a revocation record may carry (the revoker's and the revoked key's); a record with
/// more is refused before any check.
pub(crate) const MAX_CERTS_PER_RECORD: usize = 2;
/// Distinct revoked keys considered per node, anchored ones first.
pub(crate) const MAX_REVOKED_KEYS_PER_NODE: usize = 32;
/// Distinct certificates whose CA check is cached before the cache is cleared.
const CERT_CACHE_MAX: usize = 4_096;
/// The skew a certificate's issuance may run ahead of this node's wall clock when
/// `max_clock_drift_ms` is 0 (no drift bound).
const DEFAULT_SKEW_MS: u64 = 300_000;
const V2_MAGIC: &[u8] = b"MYCREV2\0";
const V2_DOMAIN: &[u8] = b"mycelium.identity/revoke/2";

fn hex32(k: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in k {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn parse_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The v1 path of a node's revocation of one key (what a 2.32 node writes; tests write one).
#[cfg(test)]
#[allow(dead_code)]
pub fn revocation_key(node: &NodeId, revoked_key: &[u8; 32]) -> String {
    format!("{REVOCATION_PREFIX}{node}/{}", hex32(revoked_key))
}

/// The path of a revocation of `revoked_key` by `revoker`: keyed `(P, S)`, so two revocations of one
/// key never share a slot.
pub fn revocation_key_v2(node: &NodeId, revoked_key: &[u8; 32], revoker: &[u8; 32]) -> String {
    format!("{REVOCATION_PREFIX}{node}/{}/{}", hex32(revoked_key), hex32(revoker))
}

/// The path of `node`'s certificate record for `key`.
pub fn cert_record_key(node: &NodeId, key: &[u8; 32]) -> String {
    format!("{CERT_PREFIX}{node}/{}", hex32(key))
}

/// One revocation event: node `node_id` retires `revoked_key` as compromised.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationEvent {
    pub node_id:     NodeId,
    /// HLC timestamp at seal time (packed `u64`).
    pub hlc:         u64,
    /// The verifying key being revoked (one of `node_id`'s own keys).
    pub revoked_key: [u8; 32],
    pub reason:      Option<String>,
}

impl RevocationEvent {
    fn canonical(&self) -> Vec<u8> {
        mycelium_core::serde_fixint::to_vec(self).unwrap_or_default()
    }
}

/// A [`RevocationEvent`] plus the writer's Ed25519 signature over its canonical bytes — the v1
/// record, and the unchanged prefix of a v2 one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRevocation {
    pub event: RevocationEvent,
    pub sig:   Vec<u8>,
}

impl SignedRevocation {
    fn sign(event: RevocationEvent, signing_key: &ed25519_dalek::SigningKey) -> Self {
        let sig = crate::tls::sign_bytes(signing_key, &event.canonical()).to_vec();
        Self { event, sig }
    }

    /// Verify the event signature against `verifying_key`. Public so an external auditor can
    /// independently confirm a revocation is signed by the claimed key before trusting it.
    pub fn verify(&self, verifying_key: &[u8; 32]) -> bool {
        let Ok(sig) = <[u8; 64]>::try_from(self.sig.as_slice()) else { return false };
        let Ok(vk) = VerifyingKey::from_bytes(verifying_key) else { return false };
        vk.verify(&self.event.canonical(), &Signature::from_bytes(&sig)).is_ok()
    }

    /// Canonical wire encoding of the v1 record (the bytes a transparency leaf commits to via `leaf_hash`).
    pub fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    /// Decode the v1 record (or a v2 record's v1 prefix). `None` on malformed bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// What a v2 revocation carries after its v1 prefix: the revoker, the certificates that establish
/// its standing and the revoked key's, and the revoker's signature over the event and all of these.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevocationTrailer {
    pub revoker: [u8; 32],
    pub certs:   Vec<Vec<u8>>,
    pub sig:     Vec<u8>,
}

/// A decoded revocation record: the v1 part, and the v2 trailer when there is one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRevocation {
    pub signed:  SignedRevocation,
    pub trailer: Option<RevocationTrailer>,
}

fn v2_message(event: &RevocationEvent, revoker: &[u8; 32], certs: &[Vec<u8>]) -> Vec<u8> {
    let mut h = Sha256::new();
    let canonical = event.canonical();
    h.update((canonical.len() as u32).to_le_bytes());
    h.update(&canonical);
    h.update(revoker);
    for c in certs {
        h.update((c.len() as u32).to_le_bytes());
        h.update(c);
    }
    let mut out = V2_DOMAIN.to_vec();
    out.extend_from_slice(&h.finalize());
    out
}

/// Decode a revocation record: the v1 prefix, then a v2 trailer behind its marker if one follows.
pub fn parse_revocation(bytes: &[u8]) -> Option<ParsedRevocation> {
    let signed = SignedRevocation::decode(bytes)?;
    let prefix_len = signed.encode().len();
    let rest = bytes.get(prefix_len..)?;
    let trailer = match rest.strip_prefix(V2_MAGIC) {
        Some(t) => Some(mycelium_core::serde_fixint::from_slice::<RevocationTrailer>(t).ok()?),
        None if rest.is_empty() => None,
        None => return None,
    };
    Some(ParsedRevocation { signed, trailer })
}

/// Build a v2 record: `event` signed by `signing_key` (the v1 prefix), then the trailer.
pub(crate) fn encode_v2(event: RevocationEvent, signing_key: &ed25519_dalek::SigningKey, certs: Vec<Vec<u8>>) -> Vec<u8> {
    let revoker = signing_key.verifying_key().to_bytes();
    let sig = crate::tls::sign_bytes(signing_key, &v2_message(&event, &revoker, &certs)).to_vec();
    let signed = SignedRevocation::sign(event, signing_key);
    let mut out = signed.encode().to_vec();
    out.extend_from_slice(V2_MAGIC);
    out.extend_from_slice(&mycelium_core::serde_fixint::to_vec(&RevocationTrailer { revoker, certs, sig }).unwrap_or_default());
    out
}

impl ParsedRevocation {
    /// Whether `signer` signed this record — the v1 signature, and the trailer's when there is one
    /// (which must then name `signer`).
    fn signed_by(&self, signer: &[u8; 32]) -> bool {
        if !self.signed.verify(signer) {
            return false;
        }
        match &self.trailer {
            None => true,
            Some(t) => {
                &t.revoker == signer
                    && crate::tls::verify_bytes(signer, &v2_message(&self.signed.event, &t.revoker, &t.certs), &t.sig)
            }
        }
    }
}

// ── The ledger: this node's memory of standing across a fold ───────────────────────────────────

#[derive(Serialize, Deserialize)]
enum JournalEntry {
    Counted { node: String, path: String, bytes: Vec<u8> },
    Saturated { node: String },
    /// A certificate this node keeps as one of `node`'s anchors (identity-and-lifecycle I2, R4): appended after it is
    /// held in memory (store, then WAL), on its own budget line. Last, so the variants before it keep their encoding;
    /// an older binary skips it with its "did not decode" warning.
    Anchor { node: String, der: Vec<u8> },
}

/// Kept anchor certificates journaled per node before further ones live in memory only.
pub(crate) const ANCHOR_JOURNAL_PER_NODE: usize = 2 * MAX_CERTS_PER_NODE;
/// Kept anchor certificates journaled in all.
pub(crate) const ANCHOR_JOURNAL_TOTAL: usize = 65_536;
/// Certificates an anchor keeps: one per issuer, and two issuers at once is a succession's widened phase.
pub(crate) const KEPT_CERTS_PER_ANCHOR: usize = 2;
/// The roots generation a kept certificate restored from the journal carries: no generation's, so the first fold
/// re-judges it against the roots this process has.
const UNJUDGED: u64 = u64::MAX;

/// One certificate an anchor keeps (identity-and-lifecycle I2, R4): the certificate itself, never its digest, so it
/// can be re-judged against new roots after its record has left the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeptCert {
    der: Arc<[u8]>,
    /// The certificate's issuer name (DER) — one kept certificate per issuer.
    issuer: Vec<u8>,
    /// Its issuance (the serial, in ms): the anchor's rank is the lowest among its kept certificates.
    issued: u64,
    /// The roots generation it was last judged under ([`UNJUDGED`] when restored from the journal).
    judged: u64,
    /// Its `notAfter` (ms): once the wall clock reaches it the certificate is judged again, in-process as at a restart
    /// (#805's review F3), and an expired one is dropped.
    expires: u64,
}

/// What an anchor keeps: its certificates, at most [`KEPT_CERTS_PER_ANCHOR`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeptAnchor {
    certs: Vec<KeptCert>,
}

impl KeptAnchor {
    /// The anchor's rank: the lowest issuance among its kept certificates (a later re-issue never raises it).
    fn rank(&self) -> Option<u64> {
        self.certs.iter().map(|c| c.issued).min()
    }

    /// Keep `cert`: it replaces the kept certificate of the same issuer when it was issued earlier (the lower rank
    /// stands), joins when there is room, and otherwise replaces the latest-issued kept certificate if it is earlier.
    /// Returns whether it is now kept and was not before.
    fn keep(&mut self, cert: KeptCert) -> bool {
        if let Some(same) = self.certs.iter_mut().find(|c| c.issuer == cert.issuer) {
            if cert.issued < same.issued || (cert.der == same.der && same.judged != cert.judged) {
                let new = cert.der != same.der;
                *same = cert;
                return new;
            }
            return false;
        }
        if self.certs.len() < KEPT_CERTS_PER_ANCHOR {
            self.certs.push(cert);
            return true;
        }
        if let Some(latest) = self.certs.iter_mut().max_by_key(|c| c.issued)
            && cert.issued < latest.issued
        {
            *latest = cert;
            return true;
        }
        false
    }
}

/// The kept-anchor journal's bookkeeping (I2, R4; #805's review F2 and its confirm round N1–N3), shared with the
/// spawned appends. Durability is tracked **per certificate**: a certificate is journaled (its append durable), in flight,
/// or neither — never offered, refused by the budget, or failed and waiting out its backoff. A caller waits for the
/// certificates it cares about ([`IdentityLedger::anchors_durable_for`]), whichever fold started their appends, so an
/// append in flight before the caller began is waited for too, and a later append finishing first satisfies nothing.
/// Bounded: one entry per kept certificate in each map, and kept certificates are bounded by the anchors' caps.
#[derive(Default)]
struct AppendTally {
    /// The next sequence number an append takes (the test hooks name appends by it).
    next: AtomicU64,
    /// Failed certificates: consecutive failures and the wall-clock ms before which no retry is made (N3's backoff).
    retry: papaya::HashMap<(NodeId, [u8; 32]), (u32, u64)>,
    /// Certificates whose refusal (budget, or not handed over) was already warned and counted: once per certificate.
    refused_noted: papaya::HashSet<(NodeId, [u8; 32])>,
    /// Kept certificates whose append is in flight, per node, by digest.
    inflight: papaya::HashMap<NodeId, HashSet<[u8; 32]>>,
    /// Kept certificates journaled durably (read at start, and appended since), per node, by digest.
    journaled: papaya::HashMap<NodeId, HashSet<[u8; 32]>>,
    journaled_total: AtomicUsize,
    notify: tokio::sync::Notify,
    /// Test hooks (#805's review F2): hold the append numbered `hold_plus1 - 1` until `gate` is notified, and fail the
    /// one numbered `fail_plus1 - 1`. `0` holds and fails nothing.
    #[cfg(test)]
    hold_plus1: AtomicU64,
    #[cfg(test)]
    gate: tokio::sync::Notify,
    #[cfg(test)]
    fail_plus1: AtomicU64,
    /// Panic the append task numbered `panic_plus1 - 1` before it appends.
    #[cfg(test)]
    panic_plus1: AtomicU64,
}

/// One kept-anchor append, settled exactly once: by [`SettleAppend::finish`] with its result, or — if the task panics
/// or is dropped before that — as **failed** on drop. Either way the certificate leaves the in-flight set and the
/// waiters are woken, so `anchors_durable_for` always answers and a later fold retries a failure.
struct SettleAppend {
    tally: Arc<AppendTally>,
    node: NodeId,
    digest: [u8; 32],
    failed: Arc<AtomicU64>,
    done: bool,
}

impl SettleAppend {
    fn finish(&mut self, ok: bool) {
        if self.done {
            return;
        }
        self.done = true;
        let key = (self.node.clone(), self.digest);
        if ok {
            if set_edit(&self.tally.journaled, &self.node, self.digest, true) {
                self.tally.journaled_total.fetch_add(1, Ordering::Relaxed);
            }
            self.tally.retry.pin().remove(&key);
            self.tally.refused_noted.pin().remove(&key);
        } else {
            self.failed.fetch_add(1, Ordering::Relaxed);
            let at = mycelium_core::sim_seam::wall_now_ms();
            self.tally.retry.pin().compute(key, |existing| {
                let n = existing.map_or(0, |(_, (n, _))| *n).saturating_add(1);
                let backoff = 1_000u64 << (n - 1).min(6);
                papaya::Operation::Insert::<_, ()>((n, at.saturating_add(backoff)))
            });
        }
        set_edit(&self.tally.inflight, &self.node, self.digest, false);
        self.tally.notify.notify_waiters();
    }
}

impl Drop for SettleAppend {
    fn drop(&mut self) {
        self.finish(false);
    }
}

fn set_edit(map: &papaya::HashMap<NodeId, HashSet<[u8; 32]>>, node: &NodeId, digest: [u8; 32], insert: bool) -> bool {
    let mut changed = false;
    map.pin().compute(node.clone(), |existing| {
        let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
        changed = if insert { set.insert(digest) } else { set.remove(&digest) };
        if changed { papaya::Operation::Insert(set) } else { papaya::Operation::Abort(()) }
    });
    changed
}

/// The kept-anchor journal's compaction state ([`IdentityLedger::compact_anchor_journal`]).
#[derive(Default)]
pub(crate) struct Compaction {
    /// Journaled anchor records compactions removed because this node no longer kept their certificates.
    pub(crate) removed: AtomicU64,
    /// Compactions that failed (the journal stood as it was; a later one tries again).
    pub(crate) failed: AtomicU64,
    /// A compaction the budget asked for is running or queued: one at a time from that path.
    spawned: AtomicBool,
    /// No budget-triggered compaction before this wall-clock ms.
    not_before: AtomicU64,
    /// A test's plant, run by the compaction between its first look at a dead record and taking it off the
    /// journaled set — where a fold may keep the certificate again.
    #[cfg(test)]
    pub(super) between_looks: arc_swap::ArcSwapOption<BetweenLooks>,
}

#[cfg(test)]
pub(super) type BetweenLooks = Box<dyn Fn(&NodeId, &[u8]) + Send + Sync>;

type AnchorsSeen = papaya::HashMap<NodeId, HashMap<[u8; 32], KeptAnchor>>;

/// Whether [`IdentityLedger::restore_anchor`] would take `der` back at a start. A record it would not take is not a
/// compaction's to judge (a newer binary may have written it): it is kept as it is.
fn restorable(der: &[u8]) -> bool {
    // The journal's unit tests keep eight-byte stand-ins for certificates.
    #[cfg(test)]
    if der.len() == 8 {
        return true;
    }
    mycelium_core::tls::cert_identity(der).is_some() && mycelium_core::tls::cert_issuer_der(der).is_some()
}

fn in_set(map: &papaya::HashMap<NodeId, HashSet<[u8; 32]>>, n: &NodeId, d: &[u8; 32]) -> bool {
    map.pin().get(n).is_some_and(|s| s.contains(d))
}

/// Whether the journal holds, durably, a certificate `n`'s anchors keep now: then a restart re-creates `n`'s entry
/// from a record that will still be there, and `n`'s dead records are not needed as its memory.
fn anchored_on_disk(seen: &AnchorsSeen, tally: &AppendTally, n: &NodeId) -> bool {
    let journaled = tally.journaled.pin();
    let Some(on_disk) = journaled.get(n) else { return false };
    seen.pin().get(n).is_some_and(|m| m.values().flat_map(|a| a.certs.iter()).any(|c| on_disk.contains(&<[u8; 32]>::from(Sha256::digest(&c.der)))))
}

/// One compaction of the identity journal `j`; how many records it removed.
///
/// # What a record is for, and so which may go
///
/// An `Anchor` record is read once, at a start ([`IdentityLedger::restore_anchor`]), and three things are rebuilt
/// from it and from nothing else: (1) **the peer's entry** in `anchors_seen` — the memory that this node once anchored
/// the peer, which is what makes a peer with no anchor left trust nothing instead of reading as legacy
/// (`refresh_once`, `nodes_with_records`); (2) **the kept certificate**, with its rank; (3) **the journaled set**, which
/// the budget counts and `anchors_durable_for` answers from. A dead record — one whose certificate this node no
/// longer keeps — carries nothing of (2), and removing it is what (3) is compacted for. It may still carry (1): when
/// it is the last record of a peer. So, per record, in the journal's order:
///
/// - not an `Anchor`, undecodable, its node unparseable, or its certificate one this binary would not restore: **kept**;
/// - its certificate kept now, or its append still in flight: **kept** (once: a duplicate is left out);
/// - dead, and the journal holds no kept certificate of that peer: **the first such record is kept** as the peer's
///   memory (the others go); it leaves at a later compaction, once a kept certificate of the peer is journaled;
/// - dead otherwise: **removed**.
///
/// # "Never believes one durable that is not"
///
/// `journaled` must never name a certificate the file under the journal's name does not hold. Four things move:
/// an append (handed to the writer, then **settled**: put on `journaled`, then taken off `inflight`), a rewrite (this
/// closure, run by the writer between appends), and the fold changing what is kept. The closure looks, takes the
/// record off `journaled`, **looks again**, and only then leaves the frame out:
///
/// - *Appended, not yet settled, and no longer kept* (the frame is in the file, `journaled` does not have it yet): the
///   record is in flight at the first look, so the frame is kept, and the settle that follows is true.
/// - *Settled between the two halves of the first look*: either `inflight` still has it (kept), or the settle is
///   complete and `journaled` has it, and it is taken off before its frame is left out.
/// - *Kept again between the first look and the take-off* (the fold sees it on `journaled` and appends nothing): the
///   second look sees it kept, puts it back and keeps the frame.
/// - *Kept again after the take-off*: `journaled` no longer has it, so the fold appends it; the append is in flight or
///   the certificate is kept at the second look (the frame is kept, the append adds a duplicate a later compaction
///   leaves out), or it comes after the second look and lands in the new file, behind the rewrite.
/// - *The rewrite fails before its rename*: the file is unchanged and what was taken off is put back.
/// - *The rewrite ends in an error after its rename, or its answer is lost*: what was taken off stays off; the writer
///   refuses appends until a reopen, and a restart rebuilds `journaled` from the file.
///
/// Between the take-off and the second look the node may believe un-journaled a record that is on disk; that answer
/// is `false`, and a caller that waits for durability waits or retries. No interleaving leaves it believing the reverse.
/// The same direction holds for a duplicate of the record kept as a peer's memory: the second copy is dead, is taken
/// off `journaled` and left out, while the first stays on disk — un-journaled by the node's count, and on disk.
async fn compact(j: Arc<super::journal::Journal>, seen: Arc<AnchorsSeen>, tally: Arc<AppendTally>, state: Arc<Compaction>) -> u64 {
    let mut written: HashSet<(String, [u8; 32])> = HashSet::new();
    // Peers whose one dead record this pass kept as their memory.
    let mut remembered: HashSet<NodeId> = HashSet::new();
    // What the choice took off the journaled set, so a rewrite that changed nothing can put it back.
    let taken: Arc<papaya::HashSet<(NodeId, [u8; 32])>> = Arc::default();
    let (taken_in, tally_in) = (Arc::clone(&taken), Arc::clone(&tally));
    #[cfg(test)]
    let state_in = Arc::clone(&state);
    let keep: super::journal::KeepRecord = Box::new(move |bytes: &[u8]| {
        let Ok(JournalEntry::Anchor { node, der }) = mycelium_core::serde_fixint::from_slice::<JournalEntry>(bytes) else {
            return true;
        };
        let Ok(n) = node.parse::<NodeId>() else { return true };
        if !restorable(&der) {
            return true;
        }
        let digest: [u8; 32] = Sha256::digest(&der).into();
        let needed = || {
            seen.pin().get(&n).is_some_and(|m| m.values().any(|a| a.certs.iter().any(|c| *c.der == *der))) || in_set(&tally_in.inflight, &n, &digest)
        };
        if needed() {
            // One record per kept certificate: a duplicate (a retried append whose first copy did land) is left out.
            return written.insert((node, digest));
        }
        // The last memory that this peer was anchored: a restart must find it, or the peer reads as legacy.
        if !anchored_on_disk(&seen, &tally_in, &n) && !remembered.contains(&n) {
            remembered.insert(n);
            return true;
        }
        #[cfg(test)]
        if let Some(plant) = state_in.between_looks.load_full() {
            plant(&n, &der);
        }
        let was_journaled = set_edit(&tally_in.journaled, &n, digest, false);
        if was_journaled {
            tally_in.journaled_total.fetch_sub(1, Ordering::Relaxed);
        }
        if needed() {
            if was_journaled && set_edit(&tally_in.journaled, &n, digest, true) {
                tally_in.journaled_total.fetch_add(1, Ordering::Relaxed);
            }
            return written.insert((node, digest));
        }
        if was_journaled {
            taken_in.pin().insert((n, digest));
        }
        false
    });
    use super::journal::RewriteOutcome;
    match j.rewrite(keep).await {
        Ok(RewriteOutcome::Unchanged(why)) => {
            // The journal is exactly what it was, so every record taken off the journaled set is still on disk: put
            // them back, or the budget would stop counting records the file still holds and nothing would compact them.
            for (n, digest) in taken.pin().iter() {
                if set_edit(&tally.journaled, n, *digest, true) {
                    tally.journaled_total.fetch_add(1, Ordering::Relaxed);
                }
            }
            state.failed.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(error = %why, "identity journal: a compaction failed before its rename; the journal is as it was and a later one tries again");
            0
        }
        Ok(RewriteOutcome::Done(done)) => {
            if done.dropped > 0 {
                state.removed.fetch_add(done.dropped, Ordering::Relaxed);
                tracing::info!(removed = done.dropped, kept = done.kept, "identity journal: compacted; anchors this node no longer keeps stop counting against the kept-anchor budget");
            }
            done.dropped
        }
        Err(e) => {
            state.failed.fetch_add(1, Ordering::Relaxed);
            // Which file the journal is now is not known here (the rename may have happened; the writer refuses
            // appends until a reopen). What was taken off the journaled set stays off: this node may believe a record
            // un-journaled that is on disk, never one durable that is not.
            tracing::warn!(error = %e, "identity journal: a compaction did not finish; appends are refused until the journal is reopened, and a restart reads what it holds");
            0
        }
    }
}

/// A certificate offered to `node`'s anchors by a fold: judged under the roots generation `judged`.
pub(crate) struct Offered {
    key: [u8; 32],
    cert: KeptCert,
}

/// What this node keeps between folds (post-360 plan row Q). Lock-free (papaya and atomics), so no
/// lock-order row; the fold itself is single-flight (`running` / `again`).
#[derive(Default)]
pub(crate) struct IdentityLedger {
    /// Every anchored key this node keeps, per node, **with its certificates** (identity-and-lifecycle I2, R4): an
    /// anchor is removed only when its kept certificates fail the current roots, never because its record left the
    /// store — so hiding a record demotes no node, in-process or (journaled, under `[persistence]`) across a restart.
    /// A node's entry, once made, stays when it is empty: a node that had standing and lost it trusts nothing
    /// (fails closed; #796's review, must-address 7), never reverting to legacy.
    /// Shared (`Arc`) with the journal's compaction, which asks it record by record what this node still keeps.
    anchors_seen: Arc<AnchorsSeen>,
    /// Anchor appends handed to the journal, and those finished — what [`IdentityLedger::anchors_durable`] waits on.
    anchor_tally: Arc<AppendTally>,
    /// Kept certificates the journal could not take (budget full, or a failed append): kept in memory only.
    pub(crate) anchor_journal_refused: AtomicU64,
    /// Kept certificates dropped because they no longer verify against this node's roots (I2, R4).
    pub(crate) anchors_dropped: AtomicU64,
    /// Kept-anchor appends that failed (each is tried again by a later fold).
    pub(crate) anchor_append_failed: Arc<AtomicU64>,
    /// The kept-anchor journal's compaction: its counters and its one-at-a-time flag, shared with the task the budget
    /// spawns (the maintainer's decision of 2026-11-17).
    pub(crate) compaction: Arc<Compaction>,
    /// **The one owner of this node's identity and roots** (identity-and-lifecycle I2, #796's review must-address 8):
    /// `rotate_identity` holds it from building its material to activating it — across its propagation wait — and a
    /// change of roots (`GossipAgent::change_roots`) holds it for its whole run, so the roots never change between a
    /// rotation's publish and its cutover. Before, a change of roots in that wait made `activate` refuse the rotation
    /// after its new key had been published and saved. A `tokio` mutex, held across `.await`; it is taken before
    /// `NodeTls::activate_lock` and never while holding another lock (lock-order table, `IdentityLedger::identity_owner`).
    pub(crate) identity_owner: tokio::sync::Mutex<()>,
    /// The journal of counted revocations, when persistence is configured. Swappable so shutdown can
    /// release it (and its ownership lock) for an agent restarted over the same directory.
    journal: arc_swap::ArcSwapOption<super::journal::Journal>,
    /// Counted revocations journaled (read at start, and appended since), per node: `(path, bytes)`.
    journaled: papaya::HashMap<NodeId, Vec<(String, Vec<u8>)>>,
    journaled_total: AtomicUsize,
    /// Nodes whose journal budget is full: they trust only their newest anchored key.
    saturated: papaya::HashSet<NodeId>,
    /// This node's own records, which it republishes when they are missing or overwritten.
    own_records: papaya::HashMap<String, Vec<u8>>,
    /// Every key each node has revoked that this node ever counted: never un-counted.
    counted_ever: papaya::HashMap<NodeId, HashSet<[u8; 32]>>,
    /// The CA check and parse per distinct certificate (by SHA-256), bounded by [`CERT_CACHE_MAX`],
    /// each verdict tagged with the roots generation it was judged under (identity-and-lifecycle I2,
    /// R4): a verdict of an older generation is a miss.
    /// Each verdict also carries the certificate's `notAfter` (ms): a verdict is a miss once the wall clock reaches
    /// it, so a certificate's expiry is judged in-process as a restart judges it (I2, #805's review F3).
    cert_cache: papaya::HashMap<[u8; 32], (u64, u64, Option<CertFacts>)>,
    running: AtomicBool,
    again: AtomicBool,
    /// Tickets handed out by [`fold_ticket`]: a caller that must see a fold that started after its call takes one.
    fold_asked: AtomicU64,
    /// The newest ticket a fold has answered: a fold that read `fold_asked` at its start and ran to the end under one
    /// roots generation answers every ticket up to what it read, whichever thread ran it.
    fold_done: AtomicU64,
    /// A test's plant, run at the start of every fold (the livelock test slows the fold with it).
    #[cfg(test)]
    pub(super) fold_plant: arc_swap::ArcSwapOption<Box<dyn Fn() + Send + Sync>>,
    /// Certificate records whose issuance was past `wall_now_ms` plus the skew bound when judged.
    pub(crate) cert_future: AtomicU64,
    /// Records skipped because a per-node or per-key cap was full.
    pub(crate) over_cap: AtomicU64,
    /// Counted revocations the journal could not take (full, or a failed append).
    pub(crate) journal_refused: AtomicU64,
}

impl std::fmt::Debug for IdentityLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdentityLedger").field("nodes", &self.anchors_seen.len()).finish()
    }
}

impl IdentityLedger {
    /// Load the journal at `path` and open it for appends. Run once at start, before the first fold.
    pub(crate) fn open_journal(&self, path: &std::path::Path) -> std::io::Result<()> {
        self.open_journal_with(path, |p| super::journal::Journal::open(p, "identity/revocations"))
    }

    /// [`Self::open_journal`] over a journal whose next compaction stops as a crash would (the crash-window tests).
    #[cfg(test)]
    pub(super) fn open_journal_crashing_for_test(&self, path: &std::path::Path, at: u8) -> std::io::Result<()> {
        let crash = Arc::new(std::sync::atomic::AtomicU8::new(at));
        self.open_journal_with(path, |p| super::journal::Journal::open_with_crash(p, "identity/revocations", crash, None))
    }

    fn open_journal_with(
        &self,
        path: &std::path::Path,
        open: impl FnOnce(&std::path::Path) -> std::io::Result<Arc<super::journal::Journal>>,
    ) -> std::io::Result<()> {
        let existing = if path.exists() { super::journal::read_journal(path)? } else { Vec::new() };
        for bytes in existing {
            match mycelium_core::serde_fixint::from_slice::<JournalEntry>(&bytes) {
                Ok(JournalEntry::Counted { node, path, bytes }) => {
                    if let Ok(n) = node.parse::<NodeId>() {
                        self.remember_journaled(&n, path, bytes);
                    }
                }
                Ok(JournalEntry::Saturated { node }) => {
                    if let Ok(n) = node.parse::<NodeId>() {
                        self.saturated.pin().insert(n);
                    }
                }
                Ok(JournalEntry::Anchor { node, der }) => {
                    if let Ok(n) = node.parse::<NodeId>() {
                        self.restore_anchor(&n, der);
                    }
                }
                Err(_) => tracing::warn!(journal = %path.display(), "revocation journal: an entry did not decode; skipped"),
            }
        }
        let j = open(path)?;
        self.journal.store(Some(j));
        Ok(())
    }

    /// Release the journal (shutdown): its ownership lock goes with the last handle.
    pub(crate) fn close_journal(&self) {
        self.journal.store(None);
    }

    /// Take a journaled kept certificate back at start, **unjudged**: the first fold judges it against the roots this
    /// process has, so a restart in a later phase than the journal's drops what no longer verifies.
    fn restore_anchor(&self, node: &NodeId, der: Vec<u8>) {
        let Some(id) = mycelium_core::tls::cert_identity(&der) else { return };
        let Some(issuer) = mycelium_core::tls::cert_issuer_der(&der) else { return };
        let digest: [u8; 32] = Sha256::digest(&der).into();
        if set_edit(&self.anchor_tally.journaled, node, digest, true) {
            self.anchor_tally.journaled_total.fetch_add(1, Ordering::Relaxed);
        }
        let expires = mycelium_core::tls::cert_not_after_ms(&der).unwrap_or(u64::MAX);
        let cert = KeptCert { der: Arc::from(der), issuer, issued: id.issued_ms.unwrap_or(0), judged: UNJUDGED, expires };
        self.anchors_seen.pin().compute(node.clone(), |existing| {
            let mut m = existing.map(|(_, m)| m.clone()).unwrap_or_default();
            m.entry(id.key).or_default().keep(cert.clone());
            papaya::Operation::Insert::<_, ()>(m)
        });
    }

    /// The sequence number the next kept-anchor append will take (the test hooks name appends by it).
    #[cfg(test)]
    pub(super) fn anchor_mark(&self) -> u64 {
        self.anchor_tally.next.load(Ordering::Acquire)
    }

    /// Every certificate this node keeps as an anchor, by node and digest — what `change_roots` waits on.
    #[cfg_attr(not(test), allow(dead_code))] // its caller, `change_roots`, has none outside the tests until PR 2b
    pub(crate) fn kept_certs(&self) -> Vec<(NodeId, [u8; 32])> {
        self.anchors_seen.pin().iter().flat_map(|(n, m)| {
            m.values().flat_map(|a| a.certs.iter()).map(|c| (n.clone(), Sha256::digest(&c.der).into())).collect::<Vec<_>>()
        }).collect()
    }

    /// Wait until each of `certs` is **settled** — journaled durably, or not in flight (refused by the budget, failed and
    /// backing off, or never offered) — and answer whether every one is journaled (identity-and-lifecycle I2, R4: the
    /// kept certificates are durable before the Narrowed phase is persisted; #805's review F2 and N1). Per certificate,
    /// whichever fold started its append: one in flight before the caller began is waited for, and a later append
    /// finishing first satisfies nothing. With no journal there is nothing to wait for and the anchors are in memory
    /// only, which is `true`: durability across a restart is stated under `[persistence]`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn anchors_durable_for(&self, certs: &[(NodeId, [u8; 32])]) -> bool {
        if self.journal.load().is_none() {
            return true;
        }
        let t = Arc::clone(&self.anchor_tally);
        let has = |m: &papaya::HashMap<NodeId, HashSet<[u8; 32]>>, n: &NodeId, d: &[u8; 32]| m.pin().get(n).is_some_and(|s| s.contains(d));
        loop {
            let notified = t.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !certs.iter().any(|(n, d)| has(&t.inflight, n, d)) {
                return certs.iter().all(|(n, d)| has(&t.journaled, n, d));
            }
            notified.await;
        }
    }

    /// How many of `node`'s journaled anchor records a compaction would remove: those naming a certificate this node
    /// no longer keeps and has no append in flight for, less the one a peer with nothing kept on disk retains as its
    /// memory. With `node` `None`, over every node.
    fn dead_journaled(&self, node: Option<&NodeId>) -> usize {
        let dead_of = |n: &NodeId, set: &HashSet<[u8; 32]>| {
            let kept: HashSet<[u8; 32]> = self.anchors_seen.pin().get(n).map(|m| m.values().flat_map(|a| a.certs.iter()).map(|c| Sha256::digest(&c.der).into()).collect()).unwrap_or_default();
            let dead = set.iter().filter(|d| !kept.contains(*d) && !in_set(&self.anchor_tally.inflight, n, d)).count();
            // A peer with no kept certificate journaled keeps one dead record as its memory ([`compact`]).
            if set.iter().any(|d| kept.contains(d)) { dead } else { dead.saturating_sub(1) }
        };
        let journaled = self.anchor_tally.journaled.pin();
        match node {
            Some(n) => journaled.get(n).map_or(0, |set| dead_of(n, set)),
            None => journaled.iter().map(|(n, set)| dead_of(n, set)).sum(),
        }
    }

    /// **Compact the kept-anchor journal** (identity-and-lifecycle I2; the maintainer's decision of 2026-11-17: the
    /// budget of [`ANCHOR_JOURNAL_PER_NODE`] applies to the anchors a node keeps **now**, not to every one it ever
    /// kept). Every journaled `Anchor` record whose certificate this node no longer keeps is left out of a rewritten
    /// journal ([`Journal::rewrite`](super::journal::Journal::rewrite): new file, fsync, rename, directory fsync), and
    /// stops counting against the budget; counted revocations, saturation marks, records this binary cannot decode
    /// or would not restore, and **the last record of a peer this node keeps nothing of** are kept as they are
    /// ([`compact`] states which record may go and why). Returns how many records it removed; `0` with no journal, nothing to remove, or a failure
    /// (counted, the journal as it was).
    ///
    /// The choice is made inside the journal's writer, against what this node keeps at that moment. A record is taken
    /// off the journaled set **before** its frame is left out, never after: between the two this node may believe a
    /// certificate un-journaled that is still on disk (it is appended again, and a duplicate is read once), but never
    /// believes one durable that is not.
    pub(crate) async fn compact_anchor_journal(&self) -> u64 {
        let Some(j) = self.journal.load_full() else { return 0 };
        if self.dead_journaled(None) == 0 {
            return 0;
        }
        compact(j, Arc::clone(&self.anchors_seen), Arc::clone(&self.anchor_tally), Arc::clone(&self.compaction)).await
    }

    /// The budget met a journal holding records of anchors this node no longer keeps: compact it off the fold's
    /// thread, one at a time, and let the next fold offer the certificate again. A failed compaction backs off a minute.
    fn spawn_compaction(&self, j: Arc<super::journal::Journal>) {
        let c = Arc::clone(&self.compaction);
        if mycelium_core::sim_seam::wall_now_ms() < c.not_before.load(Ordering::Acquire) {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        if c.spawned.swap(true, Ordering::AcqRel) {
            return;
        }
        let (seen, tally) = (Arc::clone(&self.anchors_seen), Arc::clone(&self.anchor_tally));
        rt.spawn(async move {
            let failed_before = c.failed.load(Ordering::Relaxed);
            compact(j, seen, tally, Arc::clone(&c)).await;
            if c.failed.load(Ordering::Relaxed) != failed_before {
                c.not_before.store(mycelium_core::sim_seam::wall_now_ms().saturating_add(60_000), Ordering::Release);
            }
            c.spawned.store(false, Ordering::Release);
        });
    }

    /// Count and warn a refusal once per certificate (N3).
    fn refuse_once(&self, node: &NodeId, digest: [u8; 32], why: &str) {
        if self.anchor_tally.refused_noted.pin().insert((node.clone(), digest)) {
            self.anchor_journal_refused.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%node, "identity journal: {why}; this anchor is kept in this process only");
        }
    }

    /// Journal a kept certificate of `node`'s, within the anchors' own budget. Held in memory first (the caller has
    /// merged it), then handed to the journal: the store-then-WAL order. A certificate already journaled or in flight
    /// is left; one this cannot hand to the journal (no runtime, an encode failure) is counted, and the next fold tries
    /// it again (#805's review F2).
    fn journal_anchor(&self, node: &NodeId, der: &[u8]) {
        let Some(j) = self.journal.load_full() else { return };
        let t = &self.anchor_tally;
        let digest: [u8; 32] = Sha256::digest(der).into();
        let held = |m: &papaya::HashMap<NodeId, HashSet<[u8; 32]>>| m.pin().get(node).map_or(0, |s| s.len());
        if t.journaled.pin().get(node).is_some_and(|s| s.contains(&digest)) || t.inflight.pin().get(node).is_some_and(|s| s.contains(&digest)) {
            return;
        }
        let (node_full, all_full) = (held(&t.journaled) + held(&t.inflight) >= ANCHOR_JOURNAL_PER_NODE, t.journaled_total.load(Ordering::Relaxed) >= ANCHOR_JOURNAL_TOTAL);
        if node_full || all_full {
            // The budget is of the anchors kept **now** (the maintainer's decision of 2026-11-17): while the journal
            // holds records of anchors this node dropped, it is compacted and the next fold offers this one again.
            // Only a node keeping more than the budget at once is refused, by name.
            if (node_full && self.dead_journaled(Some(node)) > 0) || (all_full && self.dead_journaled(None) > 0) {
                self.spawn_compaction(j);
                return;
            }
            self.refuse_once(node, digest, &format!("this node keeps more than the kept-anchor budget at once ({ANCHOR_JOURNAL_PER_NODE} per node, {ANCHOR_JOURNAL_TOTAL} in all)"));
            return;
        }
        // N3: a certificate whose append failed is retried only after its backoff (1 s, doubling, at most 64 s).
        let now = mycelium_core::sim_seam::wall_now_ms();
        if t.retry.pin().get(&(node.clone(), digest)).is_some_and(|(_, after)| now < *after) {
            return;
        }
        let entry = JournalEntry::Anchor { node: node.to_string(), der: der.to_vec() };
        let (Ok(bytes), Ok(rt)) = (mycelium_core::serde_fixint::to_vec(&entry), tokio::runtime::Handle::try_current()) else {
            self.refuse_once(node, digest, "a kept anchor could not be handed to the journal (the next fold tries again)");
            return;
        };
        if !set_edit(&t.inflight, node, digest, true) {
            return;
        }
        #[cfg_attr(not(test), allow(unused_variables))]
        let seq = t.next.fetch_add(1, Ordering::AcqRel);
        let tally = Arc::clone(t);
        let node = node.clone();
        let failed_counter = Arc::clone(&self.anchor_append_failed);
        // Settled on every exit: a task that panics, or is dropped at shutdown, would otherwise leave the certificate in
        // flight for good — the wait would never answer and no fold would retry it (#805's confirm round, the last item).
        let mut settle = SettleAppend { tally, node: node.clone(), digest, failed: failed_counter, done: false };
        rt.spawn(async move {
            #[cfg(test)]
            if settle.tally.hold_plus1.load(Ordering::Acquire) == seq + 1 {
                settle.tally.gate.notified().await;
            }
            #[cfg(test)]
            if settle.tally.panic_plus1.load(Ordering::Acquire) == seq + 1 {
                panic!("the test's hook: this append task panics");
            }
            let ok = j.append(bytes).await.map_err(|e| tracing::warn!(error = %e, "identity journal: an anchor append failed; the next fold tries again")).is_ok();
            #[cfg(test)]
            let ok = ok && settle.tally.fail_plus1.load(Ordering::Acquire) != seq + 1;
            settle.finish(ok);
        });
    }

    /// Test hooks on the next appends (#805's review F2): hold append `hold` until [`Self::release_held_for_test`], and
    /// fail append `fail`.
    #[cfg(test)]
    pub(super) fn hook_appends_for_test(&self, hold: Option<u64>, fail: Option<u64>) {
        self.anchor_tally.hold_plus1.store(hold.map_or(0, |h| h + 1), Ordering::Release);
        self.anchor_tally.fail_plus1.store(fail.map_or(0, |f| f + 1), Ordering::Release);
    }

    /// Panic the append task numbered `seq` (the drop guard's test).
    #[cfg(test)]
    pub(super) fn panic_append_for_test(&self, seq: u64) {
        self.anchor_tally.panic_plus1.store(seq + 1, Ordering::Release);
    }

    /// Whether this node holds retry or refusal bookkeeping for `node`'s certificate `der` (the pruning test).
    #[cfg(test)]
    pub(super) fn anchor_bookkeeping_for_test(&self, node: &NodeId, der: &[u8]) -> bool {
        let key = (node.clone(), <[u8; 32]>::from(Sha256::digest(der)));
        self.anchor_tally.retry.pin().contains_key(&key) || self.anchor_tally.refused_noted.pin().contains(&key)
    }

    /// Forget the retry and refusal bookkeeping of certificates `node` no longer keeps, so each map holds one entry per
    /// **kept** certificate at most (#805's confirm round: §117's bound, exact).
    fn prune_anchor_bookkeeping(&self, node: &NodeId, gone: &[[u8; 32]]) {
        for d in gone {
            let key = (node.clone(), *d);
            self.anchor_tally.retry.pin().remove(&key);
            self.anchor_tally.refused_noted.pin().remove(&key);
        }
    }

    #[cfg(test)]
    pub(super) fn release_held_for_test(&self) {
        self.anchor_tally.gate.notify_waiters();
    }

    /// Whether `node`'s certificate `der` is journaled durably (I2's tests).
    #[cfg(test)]
    pub(super) fn anchor_journaled_for_test(&self, node: &NodeId, der: &[u8]) -> bool {
        let digest: [u8; 32] = Sha256::digest(der).into();
        self.anchor_tally.journaled.pin().get(node).is_some_and(|s| s.contains(&digest))
    }

    /// Keep `der` as `node`'s anchor under `key` without judging it, or stop keeping it (the journal tests, whose
    /// certificates are plain bytes).
    #[cfg(test)]
    pub(super) fn keep_raw_for_test(&self, node: &NodeId, key: [u8; 32], der: &[u8], keep: bool) {
        self.anchors_seen.pin().compute(node.clone(), |existing| {
            let mut m = existing.map(|(_, m)| m.clone()).unwrap_or_default();
            if keep {
                m.entry(key).or_default().certs.push(KeptCert { der: Arc::from(der), issuer: key.to_vec(), issued: 1, judged: UNJUDGED, expires: u64::MAX });
            } else {
                m.remove(&key);
            }
            papaya::Operation::Insert::<_, ()>(m)
        });
    }

    /// Let no budget-triggered compaction start (a test that shows another path compacts).
    #[cfg(all(test, feature = "provisional-ca-generation"))]
    pub(super) fn block_spawned_compaction_for_test(&self) {
        self.compaction.spawned.store(true, Ordering::Release);
    }

    /// How many anchor records of `node` the journal holds, by this node's bookkeeping.
    #[cfg(test)]
    pub(super) fn anchors_journaled_count_for_test(&self, node: &NodeId) -> usize {
        self.anchor_tally.journaled.pin().get(node).map_or(0, |s| s.len())
    }

    fn remember_journaled(&self, node: &NodeId, path: String, bytes: Vec<u8>) -> bool {
        let mut added = false;
        self.journaled.pin().compute(node.clone(), |existing| {
            let mut v = existing.map(|(_, v)| v.clone()).unwrap_or_default();
            if v.iter().any(|(p, b)| *p == path && *b == bytes) {
                added = false;
                papaya::Operation::Abort(())
            } else {
                v.push((path.clone(), bytes.clone()));
                added = true;
                papaya::Operation::Insert(v)
            }
        });
        if added {
            self.journaled_total.fetch_add(1, Ordering::Relaxed);
        }
        added
    }

    fn append(&self, entry: JournalEntry) {
        let Some(j) = self.journal.load_full() else { return };
        let Ok(bytes) = mycelium_core::serde_fixint::to_vec(&entry) else { return };
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            if let Err(e) = j.append(bytes).await {
                tracing::warn!(error = %e, "revocation journal: append failed; the revocation still counts in this process");
            }
        });
    }

    /// Record that this node counted `bytes` (at `path`) for `node`. Journals it when there is room;
    /// otherwise saturates `node`. Returns whether it was new.
    fn record_counted(&self, node: &NodeId, path: &str, bytes: &[u8]) -> bool {
        let per_node = self.journaled.pin().get(node).map_or(0, |v| v.len());
        // One `(P, S)` is one fact: a record at a path already journaled is not journaled again, whatever
        // its bytes (a re-signed or republished copy) — so repeated revocations cannot fill the budget.
        let already = self.journaled.pin().get(node).is_some_and(|v| v.iter().any(|(p, _)| p == path));
        if already {
            return false;
        }
        if per_node >= JOURNAL_PER_NODE || self.journaled_total.load(Ordering::Relaxed) >= JOURNAL_TOTAL {
            self.journal_refused.fetch_add(1, Ordering::Relaxed);
            if self.saturated.pin().insert(node.clone()) {
                tracing::warn!(%node, "revocation journal full for this node: it now trusts only its newest anchored key (fails closed)");
                self.append(JournalEntry::Saturated { node: node.to_string() });
            }
            return false;
        }
        if self.remember_journaled(node, path.to_string(), bytes.to_vec()) {
            self.append(JournalEntry::Counted { node: node.to_string(), path: path.to_string(), bytes: bytes.to_vec() });
            return true;
        }
        false
    }

    /// Register one of this node's own records for republishing.
    pub(crate) fn own(&self, path: String, bytes: Vec<u8>) {
        self.own_records.pin().insert(path, bytes);
    }
}

// ── The fold ───────────────────────────────────────────────────────────────────────────────────

fn skew_ms(ctx: &CoreCtx) -> u64 {
    match ctx.config.max_clock_drift_ms {
        0 => DEFAULT_SKEW_MS,
        d => d,
    }
}

/// What the CA check and the parse say about one certificate, cached by the DER's hash: `None` when
/// the CA did not issue it, it is not valid now, or it carries no Ed25519 key.
#[derive(Clone)]
struct CertFacts {
    key: [u8; 32],
    issued: u64,
}

/// The CA check and the parse of `der`, once per distinct certificate (the final review's finding
/// 3: a record can carry the same certificate thousands of times). The cache is bounded: past
/// [`CERT_CACHE_MAX`] entries it is cleared, never grown.
///
/// **Re-judged when the roots change** (identity-and-lifecycle I2, R4): each verdict carries the roots
/// generation [`NodeTls::verify_member_cert_tagged`](mycelium_core::tls::NodeTls::verify_member_cert_tagged)
/// read before it loaded the chain check, and a verdict whose generation is not the node's current one is
/// a miss. Clearing the cache when the roots change would not do: a fold judging across `activate` would
/// insert its old-roots verdict after the clear.
fn cert_facts(ledger: &IdentityLedger, tls: &mycelium_core::tls::NodeTls, der: &[u8]) -> (u64, Option<CertFacts>) {
    cert_facts_with(ledger, tls, der, || {})
}

/// [`cert_facts`], running `between` after the judgment and before its verdict is cached — the plant
/// the roots-generation test (I2, exit 18) changes the roots in.
/// Empty the certificate cache — what clearing it at a change of roots would do (I2's roots-generation tests).
#[cfg(test)]
pub(super) fn clear_cert_cache_for_test(ledger: &IdentityLedger) {
    ledger.cert_cache.pin().clear();
}

/// How many certificates `node`'s anchor for `key` keeps, and their issuer names (I2, R4's tests).
#[cfg(all(test, feature = "provisional-ca-generation"))]
pub(super) fn kept_for_test(ledger: &IdentityLedger, node: &NodeId, key: &[u8; 32]) -> Vec<Vec<u8>> {
    ledger.anchors_seen.pin().get(node).and_then(|m| m.get(key).map(|a| a.certs.iter().map(|c| c.issuer.clone()).collect())).unwrap_or_default()
}

/// Whether this node holds an entry for `node`'s kept anchors at all (I2, must-address 7's tests).
#[cfg(test)]
pub(super) fn has_anchor_entry_for_test(ledger: &IdentityLedger, node: &NodeId) -> bool {
    ledger.anchors_seen.pin().contains_key(node)
}

/// One node's fold under the generation it reads now; whether it completed (I2, #768's M3 test).
#[cfg(all(test, feature = "provisional-ca-generation"))]
pub(super) fn fold_node_for_test(ctx: &TaskCtx, node: &NodeId) -> bool {
    let Some(tls) = ctx.tls.get() else { return false };
    fold_node(ctx, tls, node, tls.roots_generation()).is_some()
}

/// Whether [`cert_facts_with`] gives `der` facts (I2's roots-generation tests).
#[cfg(test)]
pub(super) fn cert_verdict_for_test(ledger: &IdentityLedger, tls: &mycelium_core::tls::NodeTls, der: &[u8], between: impl FnOnce()) -> bool {
    cert_facts_with(ledger, tls, der, between).1.is_some()
}

/// The verdict and the roots generation it was reached under (the cached one's tag on a hit).
fn cert_facts_with(ledger: &IdentityLedger, tls: &mycelium_core::tls::NodeTls, der: &[u8], between: impl FnOnce()) -> (u64, Option<CertFacts>) {
    let digest: [u8; 32] = Sha256::digest(der).into();
    if let Some((generation, expires, hit)) = ledger.cert_cache.pin().get(&digest)
        && *generation == tls.roots_generation()
        && mycelium_core::sim_seam::wall_now_ms() < *expires
    {
        return (*generation, hit.clone());
    }
    let (generation, facts) = match mycelium_core::tls::cert_identity(der) {
        Some(id) => {
            let (generation, verified) = tls.verify_member_cert_tagged(der);
            (generation, verified.then_some(CertFacts { key: id.key, issued: id.issued_ms.unwrap_or(0) }))
        }
        None => (tls.roots_generation(), None),
    };
    between();
    let cache = ledger.cert_cache.pin();
    if cache.len() >= CERT_CACHE_MAX {
        cache.clear();
    }
    cache.insert(digest, (generation, mycelium_core::tls::cert_not_after_ms(der).unwrap_or(u64::MAX), facts.clone()));
    (generation, facts)
}

/// Judge one certificate for `node`: issued by the fleet CA and valid now, naming `node` by URI,
/// for `expect` when a path names the key, issued no later than the wall clock plus the skew bound.
/// `Some((key, issued_ms))` for a certificate that anchors.
fn judge_cert(ctx: &CoreCtx, tls: &mycelium_core::tls::NodeTls, ledger: &IdentityLedger, node: &NodeId, der: &[u8], expect: Option<[u8; 32]>) -> Option<([u8; 32], u64)> {
    judge_cert_tagged(ctx, tls, ledger, node, der, expect).1
}

/// [`judge_cert`], with the roots generation the chain verdict was reached under (identity-and-lifecycle I2, R4): a
/// fold merges a verdict into the kept anchors only when it was reached under the generation the fold started at.
fn judge_cert_tagged(ctx: &CoreCtx, tls: &mycelium_core::tls::NodeTls, ledger: &IdentityLedger, node: &NodeId, der: &[u8], expect: Option<[u8; 32]>) -> (u64, Option<([u8; 32], u64)>) {
    // The parse is free; refuse on it before the CA check.
    let Some(id) = mycelium_core::tls::cert_identity(der) else { return (tls.roots_generation(), None) };
    if expect.is_some_and(|k| k != id.key) || !id.node_uris.contains(&mycelium_core::tls::node_uri(node)) {
        return (tls.roots_generation(), None);
    }
    let issued = id.issued_ms.unwrap_or(0);
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        ledger.cert_future.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%node, issued_ms = issued, "certificate record issued in the future beyond the skew bound: ignored (identity_cert_future)");
        return (tls.roots_generation(), None);
    }
    let (generation, facts) = cert_facts(ledger, tls, der);
    (generation, facts.map(|f| (f.key, f.issued)))
}

/// A certificate `judge_cert_tagged` passed, as an [`Offered`] kept certificate.
fn offered(der: &[u8], key: [u8; 32], issued: u64, judged: u64) -> Option<Offered> {
    let issuer = mycelium_core::tls::cert_issuer_der(der)?;
    let expires = mycelium_core::tls::cert_not_after_ms(der).unwrap_or(u64::MAX);
    Some(Offered { key, cert: KeptCert { der: Arc::from(der), issuer, issued, judged, expires } })
}

/// A revocation record found for a node: where, its bytes, and what it says.
struct Candidate {
    path: String,
    bytes: Vec<u8>,
    parsed: ParsedRevocation,
    /// The revoker the path names (v2), if any.
    named: Option<[u8; 32]>,
}

/// Parse a revocation record found at `path` for `node`, refusing it — before any signature or CA
/// check — unless its path agrees with its body and it carries at most
/// [`MAX_CERTS_PER_RECORD`] certificates.
fn candidate(node: &NodeId, path: &str, bytes: &[u8]) -> Option<Candidate> {
    let rest = path.strip_prefix(REVOCATION_PREFIX)?.strip_prefix(&format!("{node}/"))?;
    let (p_hex, s_hex) = match rest.split_once('/') {
        Some((p, s)) => (p, Some(s)),
        None => (rest, None),
    };
    let p = parse_hex32(p_hex)?;
    let parsed = parse_revocation(bytes)?;
    if &parsed.signed.event.node_id != node || parsed.signed.event.revoked_key != p {
        return None;
    }
    if parsed.trailer.as_ref().is_some_and(|t| t.certs.len() > MAX_CERTS_PER_RECORD) {
        return None;
    }
    let named = match s_hex {
        Some(s) => {
            let s = parse_hex32(s)?;
            if parsed.trailer.as_ref().map(|t| t.revoker) != Some(s) {
                return None;
            }
            Some(s)
        }
        None => None,
    };
    Some(Candidate { path: path.to_string(), bytes: bytes.to_vec(), parsed, named })
}

/// One node's fold: its anchored keys (with first issuance) and the revocations that count.
struct Folded {
    anchors: HashMap<[u8; 32], u64>,
    /// Revocations of anchored keys that count: recorded, journaled, never un-counted.
    counted: Vec<Candidate>,
    /// Revocations of keys not (yet) anchored that count this fold only: re-judged every fold.
    provisional: Vec<Candidate>,
}

/// Re-judge, under the roots generation `g0`, every certificate `node`'s anchors keep that was judged under another
/// (identity-and-lifecycle I2, R4): `digest → verdict`, only for verdicts reached under `g0` itself — one reached under
/// a later generation is left out, and the next fold judges it.
fn rejudge_kept(ctx: &CoreCtx, tls: &mycelium_core::tls::NodeTls, ledger: &IdentityLedger, node: &NodeId, g0: u64) -> HashMap<[u8; 32], bool> {
    let now = mycelium_core::sim_seam::wall_now_ms();
    let stale: Vec<([u8; 32], Arc<[u8]>)> = ledger.anchors_seen.pin().get(node).map(|m| {
        m.iter().flat_map(|(k, a)| a.certs.iter().filter(|c| c.judged != g0 || now >= c.expires).map(move |c| (*k, Arc::clone(&c.der)))).collect()
    }).unwrap_or_default();
    let mut out = HashMap::new();
    for (key, der) in stale {
        let (tag, verdict) = judge_cert_tagged(ctx, tls, ledger, node, &der, Some(key));
        if tag == g0 {
            out.insert(Sha256::digest(&der).into(), verdict.is_some());
        }
    }
    out
}

/// Merge `offered` into `node`'s kept anchors, and apply `rejudged` to the certificates they keep (identity-and-lifecycle
/// I2, R4). Each key's rank is its kept certificates' **lowest** issuance — a later re-issue of a key never raises it
/// (the final review's finding 2). A kept certificate `rejudged` refuses is dropped, a key with none left is dropped,
/// and the node's entry stays when it is empty (fails closed). Only verdicts reached under `g0` are merged, and when the
/// roots generation has moved from `g0` **nothing** is merged: `None`, and the caller folds again. Retry-safe `compute`
/// (the closure is a pure function of what it reads and its inputs), so two folds never drop each other's anchors
/// (finding 5). Newly kept certificates are journaled after the merge (memory, then the journal).
fn merge_anchors_seen(
    ledger: &IdentityLedger,
    tls: &mycelium_core::tls::NodeTls,
    node: &NodeId,
    offered: &[Offered],
    rejudged: &HashMap<[u8; 32], bool>,
    g0: u64,
) -> Option<HashMap<[u8; 32], u64>> {
    let mut merged_out: Option<HashMap<[u8; 32], u64>> = None;
    let mut fresh: Vec<Arc<[u8]>> = Vec::new();
    let mut gone: Vec<[u8; 32]> = Vec::new();
    let mut dropped = 0u64;
    let mut capped = 0u64;
    ledger.anchors_seen.pin().compute(node.clone(), |existing| {
        fresh.clear();
        gone.clear();
        dropped = 0;
        capped = 0;
        merged_out = None;
        #[cfg(test)]
        if let Some(plant) = MERGE_PLANT.with(|p| p.borrow_mut().take()) {
            plant();
        }
        if tls.roots_generation() != g0 {
            return papaya::Operation::Abort(());
        }
        let mut m: HashMap<[u8; 32], KeptAnchor> = existing.map(|(_, m)| m.clone()).unwrap_or_default();
        for a in m.values_mut() {
            a.certs.retain_mut(|c| {
                let digest: [u8; 32] = Sha256::digest(&c.der).into();
                match rejudged.get(&digest) {
                    Some(true) => {
                        c.judged = g0;
                        true
                    }
                    Some(false) => {
                        dropped += 1;
                        false
                    }
                    None => true,
                }
            });
        }
        m.retain(|_, a| !a.certs.is_empty());
        for o in offered {
            if o.cert.judged == g0 {
                m.entry(o.key).or_default().keep(o.cert.clone());
            }
        }
        if m.len() > MAX_CERTS_PER_NODE {
            let mut v: Vec<([u8; 32], u64)> = m.iter().map(|(k, a)| (*k, a.rank().unwrap_or(0))).collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            for (k, _) in v.into_iter().skip(MAX_CERTS_PER_NODE) {
                m.remove(&k);
                capped += 1;
            }
        }
        // Every kept certificate is offered to the journal, which skips what is journaled or in flight: a failed or
        // never-handed append is tried again here (#805's review F2).
        fresh = m.values().flat_map(|a| a.certs.iter()).map(|c| Arc::clone(&c.der)).collect();
        // Certificates this merge stops keeping (dropped, replaced or capped): their retry bookkeeping goes with them.
        gone = existing.map(|(_, old)| {
            old.values().flat_map(|a| a.certs.iter()).filter(|c| !fresh.contains(&c.der)).map(|c| Sha256::digest(&c.der).into()).collect()
        }).unwrap_or_default();
        merged_out = Some(m.iter().filter_map(|(k, a)| a.rank().map(|r| (*k, r))).collect());
        if existing.is_none() && m.is_empty() {
            return papaya::Operation::Abort(());
        }
        if existing.is_some_and(|(_, old)| *old == m) {
            papaya::Operation::Abort(())
        } else {
            papaya::Operation::Insert(m)
        }
    });
    if dropped > 0 {
        ledger.anchors_dropped.fetch_add(dropped, Ordering::Relaxed);
        tracing::warn!(%node, certificates = dropped, "kept anchor certificates no longer verify against this node's roots: dropped");
    }
    if capped > 0 {
        ledger.over_cap.fetch_add(capped, Ordering::Relaxed);
    }
    for der in &fresh {
        ledger.journal_anchor(node, der);
    }
    ledger.prune_anchor_bookkeeping(node, &gone);
    merged_out
}

#[cfg(test)]
thread_local! {
    /// A plant for exit 18's merge race (#768's M3): run inside the next `merge_anchors_seen` closure on this thread,
    /// before its generation check — where a test changes the roots between a fold's judging and its merge.
    pub(super) static MERGE_PLANT: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// One node's fold under the roots generation `g0` the whole fold started at; `None` when the roots changed during it
/// (nothing was merged — the caller folds again).
fn fold_node(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls, node: &NodeId, g0: u64) -> Option<Folded> {
    let ledger = &ctx.identity_ledger;
    // Stratum 0 (I2, R4) — the certificates the anchors keep, re-judged when the roots changed since they were judged.
    let rejudged = rejudge_kept(ctx, tls, ledger, node, g0);
    // Stratum 1a — the store's certificate records: parse first, CA-check what survives.
    let mut offers: Vec<Offered> = Vec::new();
    for (path, der) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{CERT_PREFIX}{node}/")) {
        let Some(expect) = path.rsplit('/').next().and_then(parse_hex32) else { continue };
        if let (tag, Some((k, at))) = judge_cert_tagged(ctx, tls, ledger, node, &der, Some(expect))
            && let Some(o) = offered(&der, k, at, tag)
        {
            offers.push(o);
        }
    }
    let known_anchors = merge_anchors_seen(ledger, tls, node, &offers, &rejudged, g0)?;
    // Revocation records: the store, then the journal — parsed (path against body, at most two
    // certificates) before any check.
    let mut found: Vec<Candidate> = Vec::new();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{REVOCATION_PREFIX}{node}/")) {
        if let Some(c) = candidate(node, &key, &bytes) {
            found.push(c);
        }
    }
    let journaled: Vec<(String, Vec<u8>)> = ledger.journaled.pin().get(node).cloned().unwrap_or_default();
    for (path, bytes) in &journaled {
        if !found.iter().any(|c| &c.path == path && &c.bytes == bytes)
            && let Some(c) = candidate(node, path, bytes)
        {
            found.push(c);
        }
    }
    // The caps, before any CA or signature check (finding 3). A revoked key must be one held for the
    // node; per revoked key, records naming an already-anchored revoker come first, then by the
    // issuance their carried certificate *claims* (parsed, unchecked) — never by key bytes.
    let held: HashSet<[u8; 32]> = ctx.peer_keys.pin().get(node).cloned().unwrap_or_default().into_iter()
        .chain(known_anchors.keys().copied()).collect();
    let claimed = |c: &Candidate| -> u64 {
        let s = c.named;
        c.parsed.trailer.iter().flat_map(|t| t.certs.iter())
            .filter_map(|d| mycelium_core::tls::cert_identity(d))
            .filter(|id| Some(id.key) == s)
            .filter_map(|id| id.issued_ms)
            .min()
            .unwrap_or(0)
    };
    let mut per_key: HashMap<[u8; 32], Vec<Candidate>> = HashMap::new();
    for c in found {
        let p = c.parsed.signed.event.revoked_key;
        if held.contains(&p) {
            per_key.entry(p).or_default().push(c);
        }
    }
    if per_key.len() > MAX_REVOKED_KEYS_PER_NODE {
        let mut ps: Vec<[u8; 32]> = per_key.keys().copied().collect();
        ps.sort_by_key(|p| (std::cmp::Reverse(known_anchors.contains_key(p)), *p));
        for p in ps.into_iter().skip(MAX_REVOKED_KEYS_PER_NODE) {
            if let Some(v) = per_key.remove(&p) {
                ledger.over_cap.fetch_add(v.len() as u64, Ordering::Relaxed);
            }
        }
    }
    let mut selected: Vec<Candidate> = Vec::new();
    for (_, mut cs) in per_key {
        cs.sort_by_key(|c| {
            let anchored_at = c.named.and_then(|s| known_anchors.get(&s).copied());
            (std::cmp::Reverse(anchored_at.is_some()), std::cmp::Reverse(anchored_at.unwrap_or_else(|| claimed(c))))
        });
        if cs.len() > MAX_REVOCATIONS_PER_KEY {
            ledger.over_cap.fetch_add((cs.len() - MAX_REVOCATIONS_PER_KEY) as u64, Ordering::Relaxed);
            cs.truncate(MAX_REVOCATIONS_PER_KEY);
        }
        selected.extend(cs);
    }
    // Stratum 1b — the certificates the selected records carry.
    let mut carried: Vec<Offered> = Vec::new();
    for c in &selected {
        for der in c.parsed.trailer.iter().flat_map(|t| t.certs.iter()) {
            if let (tag, Some((k, at))) = judge_cert_tagged(ctx, tls, ledger, node, der, None)
                && let Some(o) = offered(der, k, at, tag)
            {
                carried.push(o);
            }
        }
    }
    let mut anchors = merge_anchors_seen(ledger, tls, node, &carried, &HashMap::new(), g0)?;
    if anchors.len() > MAX_CERTS_PER_NODE {
        let mut v: Vec<([u8; 32], u64)> = anchors.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ledger.over_cap.fetch_add((v.len() - MAX_CERTS_PER_NODE) as u64, Ordering::Relaxed);
        v.truncate(MAX_CERTS_PER_NODE);
        anchors = v.into_iter().collect();
    }
    // Stratum 2 — revocations, judged against the anchors. A legacy node (none) has none that count.
    // A revocation of a key that is not (yet) anchored counts only **provisionally** (the confirming
    // review's N1): it is re-judged on every fold and never recorded as counted, because the revoked
    // key's certificate may still arrive and outrank the signer — a thief's revocation that arrived
    // first must not lock in. A signer being revoked itself does not void what it revoked earlier:
    // authority flows only to the past, and refusing it made two compromise rotations learned out of
    // order leave the oldest key trusted (the N1/N2 check's NEW-2).
    let mut counted = Vec::new();
    let mut provisional = Vec::new();
    if !anchors.is_empty() {
        let mut by_signers: Vec<[u8; 32]> = anchors.keys().copied().collect();
        by_signers.sort_by(|a, b| anchors[b].cmp(&anchors[a]).then(a.cmp(b)));
        for c in selected {
            let p = c.parsed.signed.event.revoked_key;
            let signers: Vec<[u8; 32]> = match c.named {
                Some(s) => vec![s],
                None => by_signers.clone(),
            };
            let counts = signers.iter().any(|s| {
                let Some(&s_at) = anchors.get(s) else { return false };
                let outranks = anchors.get(&p).is_none_or(|&p_at| s_at > p_at);
                outranks && c.parsed.signed_by(s)
            });
            if counts && anchors.contains_key(&p) {
                counted.push(c);
            } else if counts {
                provisional.push(c);
            }
        }
    }
    Some(Folded { anchors, counted, provisional })
}

/// Every node with a certificate record, a revocation, or a journaled revocation.
fn nodes_with_records(ctx: &TaskCtx) -> Vec<NodeId> {
    let mut nodes: HashSet<NodeId> = HashSet::new();
    for prefix in [CERT_PREFIX, REVOCATION_PREFIX] {
        for (key, _) in crate::store::scan_kv_prefix(&ctx.kv_state, prefix) {
            if let Some((n, _)) = key[prefix.len()..].split_once('/')
                && let Ok(n) = n.parse::<NodeId>()
            {
                nodes.insert(n);
            }
        }
    }
    for (n, _) in ctx.identity_ledger.journaled.pin().iter() {
        nodes.insert(n.clone());
    }
    for (n, _) in ctx.identity_ledger.anchors_seen.pin().iter() {
        nodes.insert(n.clone());
    }
    let mut v: Vec<NodeId> = nodes.into_iter().collect();
    v.sort_by_key(|n| n.to_string());
    v
}

/// Recompute every node's standing from the store, the carried certificates and the journal, and
/// publish it to `CoreCtx::key_standing`; journal newly counted revocations. Single-flight: a call
/// while one runs makes that one run again. Run by the identity watcher on any `sys/identity*`
/// or `sys/revocation/` change, after the startup identity prewarm, periodically (to re-judge a
/// certificate issued in the future), and by [`revoke_key`].
pub(crate) fn refresh(ctx: &TaskCtx) {
    let ledger = &ctx.identity_ledger;
    loop {
        if ledger.running.swap(true, Ordering::AcqRel) {
            ledger.again.store(true, Ordering::Release);
            return;
        }
        loop {
            ledger.again.store(false, Ordering::Release);
            // Acquire pairs with the ticket's AcqRel: a fold that reads a ticket sees what its taker wrote before taking it
            // (a record put, the roots changed). Release on `fold_done` pairs with the waiter's Acquire: it sees the fold.
            let asked = ledger.fold_asked.load(Ordering::Acquire);
            if refresh_once(ctx) {
                ledger.fold_done.fetch_max(asked, Ordering::Release);
            }
            if !ledger.again.load(Ordering::Acquire) {
                break;
            }
        }
        ledger.running.store(false, Ordering::Release);
        if !ledger.again.swap(false, Ordering::AcqRel) {
            return;
        }
    }
}

/// A ticket for a fold that **starts after** this call: a caller that must see the fold's result before it goes on —
/// `GossipAgent::change_roots`, after the roots changed (identity-and-lifecycle I2) — takes one, then calls
/// [`refresh_answers`] until it is `true`. A fold that ran under the old roots and re-runs on its own would otherwise
/// finish after the caller had returned.
pub(crate) fn fold_ticket(ctx: &TaskCtx) -> u64 {
    ctx.identity_ledger.fold_asked.fetch_add(1, Ordering::AcqRel) + 1
}

/// [`refresh`], then whether `ticket` is answered: a fold that started after it was taken has run to the end, **this
/// call's or one another thread is running**, which re-runs for it. Until 2026-11-21 the waiter accepted only a fold it
/// ran itself: while another thread's fold ran, each of its retries set `again`, so that fold re-ran for ever and the
/// waiter never ran one — a livelock that hung `an_often_rotated_peer_does_not_exhaust_the_kept_anchor_budget` (and
/// CI) for six hours whenever the identity watcher's fold had started first.
pub(crate) fn refresh_answers(ctx: &TaskCtx, ticket: u64) -> bool {
    refresh(ctx);
    ctx.identity_ledger.fold_done.load(Ordering::Acquire) >= ticket
}

/// The keys a node has revoked that this node has ever counted — in this process, or journaled
/// before a restart. A counted revocation stays counted: no later fold un-counts it (the final
/// review's finding 2).
fn remember_counted(ledger: &IdentityLedger, node: &NodeId, now: &HashSet<[u8; 32]>) -> HashSet<[u8; 32]> {
    let mut out = HashSet::new();
    ledger.counted_ever.pin().compute(node.clone(), |existing| {
        let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
        let before = set.len();
        set.extend(now.iter().copied());
        out = set.clone();
        if existing.is_some() && set.len() == before {
            papaya::Operation::Abort(())
        } else {
            papaya::Operation::Insert(set)
        }
    });
    out
}

/// One fold of every node; whether it ran to the end under one roots generation, and so answers the tickets it read
/// (`false`: the roots changed during it — mid-fold `again` is set and the caller folds again).
fn refresh_once(ctx: &TaskCtx) -> bool {
    #[cfg(test)]
    if let Some(plant) = ctx.identity_ledger.fold_plant.load_full() {
        plant();
    }
    let Some(tls) = ctx.tls.get() else { return true };
    let ledger = &ctx.identity_ledger;
    // I2, R4 (#768's M3): the whole fold is judged under one roots generation. If the roots change while it runs, the
    // node being folded merges nothing, no standing judged under the old roots is published, and the fold runs again.
    let g0 = tls.roots_generation();
    for node in nodes_with_records(ctx) {
        let Some(folded) = fold_node(ctx, tls, &node, g0) else {
            ledger.again.store(true, Ordering::Release);
            return false;
        };
        let mut now = HashSet::new();
        for c in &folded.counted {
            now.insert(c.parsed.signed.event.revoked_key);
            if ledger.record_counted(&node, &c.path, &c.bytes) {
                tracing::warn!(%node, key = %hex32(&c.parsed.signed.event.revoked_key), "identity key revoked: it no longer verifies for this node");
            }
        }
        // Journaled revocations were counted by this node before: they stay counted.
        for (path, bytes) in ledger.journaled.pin().get(&node).cloned().unwrap_or_default() {
            if let Some(c) = candidate(&node, &path, &bytes) {
                now.insert(c.parsed.signed.event.revoked_key);
            }
        }
        let counted = remember_counted(ledger, &node, &now);
        let mut revoked = counted.clone();
        revoked.extend(folded.provisional.iter().map(|c| c.parsed.signed.event.revoked_key));
        // A node whose kept anchors were all dropped keeps its (empty) entry, and has no trusted key: removing anchors
        // never takes a node back to legacy (#796's review, must-address 7). Only a node never anchored here is legacy.
        let anchored = if folded.anchors.is_empty() {
            ledger.anchors_seen.pin().contains_key(&node).then(HashSet::new)
        } else if ledger.saturated.pin().contains(&node) {
            // Fail closed: the newest anchored key alone.
            folded.anchors.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(k, _)| [*k].into_iter().collect())
        } else {
            Some(folded.anchors.keys().copied().collect::<HashSet<_>>())
        };
        let standing = NodeStanding { anchored, revoked, counted };
        if tls.roots_generation() != g0 {
            ledger.again.store(true, Ordering::Release);
            return false;
        }
        if ctx.key_standing.get(&node).as_deref() != Some(&standing) {
            ctx.key_standing.set(&node, standing);
        }
    }
    // The roots changed after the last node: nothing is marked judged, and the fold did not answer a ticket (a waiter
    // asks again; the change of roots that moved them asks for its own fold).
    if tls.roots_generation() != g0 {
        return false;
    }
    ctx.key_standing.set_judged_under(g0);
    true
}

/// The revocations of `node`'s keys that count at this node (the transparency log's leaves).
#[cfg(feature = "compliance")]
pub(crate) fn validated_signed_revocations(ctx: &TaskCtx, node: &NodeId) -> Vec<SignedRevocation> {
    let Some(tls) = ctx.tls.get() else { return Vec::new() };
    let mut out: Vec<SignedRevocation> = Vec::new();
    for c in fold_node(ctx, tls, node, tls.roots_generation()).map(|f| f.counted).unwrap_or_default() {
        if !out.contains(&c.parsed.signed) {
            out.push(c.parsed.signed);
        }
    }
    out
}

/// Distinct nodes that have published at least one revocation in the local gossip view.
#[cfg(feature = "compliance")]
pub(crate) fn revocation_nodes(ctx: &CoreCtx) -> Vec<NodeId> {
    let mut nodes = HashSet::new();
    for (key, _) in crate::store::scan_kv_prefix(&ctx.kv_state, REVOCATION_PREFIX) {
        if let Some(rest) = key.strip_prefix(REVOCATION_PREFIX)
            && let Some((node_seg, _)) = rest.split_once('/')
            && let Ok(node) = node_seg.parse::<NodeId>()
        {
            nodes.insert(node);
        }
    }
    let mut v: Vec<NodeId> = nodes.into_iter().collect();
    v.sort_by_key(|n| n.to_string());
    v
}

// ── The owner's side ───────────────────────────────────────────────────────────────────────────

/// The keys this node can show it has held, without trusting a value any member can write: its
/// current key, its anchored keys (each a CA-issued certificate naming it), and the keys in its own
/// sealed identity record when that record's proof is signed by one of those (the final review's
/// finding 6 — never the raw `sys/identity/{self}`).
pub(crate) fn own_history(ctx: &TaskCtx) -> Vec<[u8; 32]> {
    let mut keys: Vec<[u8; 32]> = Vec::new();
    if let Some(t) = ctx.tls.get() {
        keys.push(t.verifying_key_bytes());
    }
    for k in ctx.identity_ledger.anchors_seen.pin().get(&ctx.node_id).map(|m| m.keys().copied().collect::<Vec<_>>()).unwrap_or_default() {
        if !keys.contains(&k) { keys.push(k); }
    }
    let sealed = mycelium_core::ops::kv_get(ctx, &format!("{}{}", mycelium_core::signal::kv_ns::IDENTITY_SIGNED, ctx.node_id));
    if let Some(bytes) = sealed
        && let Some((history, proof)) = super::helpers::parse_sealed_identity(&bytes)
        && let Some((signer, sig)) = super::helpers::parse_identity_proof(proof)
        && keys.contains(&signer)
        && crate::tls::verify_bytes(&signer, &super::helpers::identity_proof_message(history), &sig)
    {
        for k in super::helpers::parse_identity_keys(history) {
            if !keys.contains(&k) { keys.push(k); }
        }
    }
    keys
}

/// The key history this node republishes at start (the confirming review's N2): its current key, the
/// keys its own certificate records anchor (CA-checked), and the keys of its own sealed record when that
/// record's proof is signed by one of those — never the raw `sys/identity/{self}` value, which any
/// member can write. Runs before the identity is installed, so it takes the loaded `tls`.
pub(crate) fn own_history_at_start(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls) -> Vec<[u8; 32]> {
    let current = tls.verifying_key_bytes();
    let mut keys = vec![current];
    for (p, der) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{CERT_PREFIX}{}/", ctx.node_id)) {
        if let Some(k) = p.rsplit('/').next().and_then(parse_hex32)
            && let Some((key, _)) = judge_cert(ctx, tls, &ctx.identity_ledger, &ctx.node_id, &der, Some(k))
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }
    let sealed = mycelium_core::ops::kv_get(ctx, &format!("{}{}", mycelium_core::signal::kv_ns::IDENTITY_SIGNED, ctx.node_id));
    if let Some(bytes) = sealed
        && let Some((history, proof)) = super::helpers::parse_sealed_identity(&bytes)
        && let Some((signer, sig)) = super::helpers::parse_identity_proof(proof)
        && keys.contains(&signer)
        && crate::tls::verify_bytes(&signer, &super::helpers::identity_proof_message(history), &sig)
    {
        for k in super::helpers::parse_identity_keys(history) {
            if !keys.contains(&k) { keys.push(k); }
        }
    }
    keys
}

/// The newest issuance among this node's own certificates — its records in the store and the one
/// it runs with. A rotation issues strictly after it (`generate_rotation_after`).
pub(crate) fn own_newest_issuance(ctx: &CoreCtx) -> u64 {
    let mut newest = ctx.tls.get().and_then(|t| mycelium_core::tls::cert_identity(&t.node_cert_der())).and_then(|id| id.issued_ms).unwrap_or(0);
    for (_, der) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{CERT_PREFIX}{}/", ctx.node_id)) {
        if let Some(at) = mycelium_core::tls::cert_identity(&der).and_then(|id| id.issued_ms) {
            newest = newest.max(at);
        }
    }
    newest
}

/// Whether this node's own records show the key it is starting with revoked (the final review's
/// finding 2): a key file restored from a backup, or `key_pem` reloading a key a rotation retired.
/// Judged by the same fold as any peer's, over the store and the journal.
pub(crate) fn own_key_revoked(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls) -> bool {
    let current = tls.verifying_key_bytes();
    // The node's own current certificate is an anchor before anything is judged (the confirming
    // review's N1), so a missing own-certificate record cannot make a planted record a refusal.
    let own = tls.node_cert_der();
    if let (tag, Some((k, at))) = judge_cert_tagged(ctx, tls, &ctx.identity_ledger, &ctx.node_id, &own, Some(current))
        && let Some(o) = offered(&own, k, at, tag)
    {
        merge_anchors_seen(&ctx.identity_ledger, tls, &ctx.node_id, &[o], &HashMap::new(), tag);
    }
    let journaled = ctx.identity_ledger.journaled.pin().get(&ctx.node_id).cloned().unwrap_or_default();
    if journaled.iter().filter_map(|(p, b)| candidate(&ctx.node_id, p, b)).any(|c| c.parsed.signed.event.revoked_key == current) {
        return true;
    }
    fold_node(ctx, tls, &ctx.node_id, tls.roots_generation())
        .is_some_and(|f| f.counted.iter().any(|c| c.parsed.signed.event.revoked_key == current))
}

/// Publish this node's certificate record for its current key — keeping a record already in the
/// store for that key, so the key's first issuance stands — and take up for republishing every
/// certificate record of a key it has held (the final review's finding 1).
pub(crate) fn publish_own_certificate(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls) {
    let ledger = &ctx.identity_ledger;
    let current = tls.verifying_key_bytes();
    let path = cert_record_key(&ctx.node_id, &current);
    let uri = mycelium_core::tls::node_uri(&ctx.node_id);
    let valid = |der: &[u8], key: [u8; 32]| {
        mycelium_core::tls::cert_identity(der).is_some_and(|id| id.key == key && id.node_uris.contains(&uri)) && tls.verify_member_cert(der)
    };
    let der = match mycelium_core::ops::kv_get(ctx, &path) {
        Some(stored) if valid(&stored, current) => stored.to_vec(),
        _ => {
            let der = tls.node_cert_der();
            if !valid(&der, current) {
                tracing::warn!(
                    node = %ctx.node_id,
                    "this node's certificate does not name it by URI (mycelium://<node>): it has no standing, so no revocation of its keys counts at upgraded peers — re-issue it (docs/guide/deprecations.md §39)"
                );
            }
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(der.clone()));
            der
        }
    };
    ledger.own(path, der);
    for (p, stored) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{CERT_PREFIX}{}/", ctx.node_id)) {
        if let Some(k) = p.rsplit('/').next().and_then(parse_hex32)
            && valid(&stored, k)
        {
            ledger.own(p.to_string(), stored.to_vec());
        }
    }
}

/// At start: take up this node's own revocations — its v2 records, and each v1 record (at the old
/// path) it can show it wrote, re-issued as v2 under its current, anchored key — so they count at
/// upgraded peers. Never one revoking the key it signs with.
pub(crate) fn adopt_own_revocations(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let current = tls.verifying_key_bytes();
    let history = own_history(ctx);
    // Who may have written a record this node adopts: its current key or one a certificate anchors
    // for it — never a key that reached its identity history some other way.
    let signers: Vec<[u8; 32]> = std::iter::once(current)
        .chain(ctx.identity_ledger.anchors_seen.pin().get(&ctx.node_id).map(|m| m.keys().copied().collect::<Vec<_>>()).unwrap_or_default())
        .collect();
    let own_cert = tls.node_cert_der();
    for (path, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{REVOCATION_PREFIX}{}/", ctx.node_id)) {
        let Some(c) = candidate(&ctx.node_id, &path, &bytes) else { continue };
        let p = c.parsed.signed.event.revoked_key;
        if p == current || !history.contains(&p) {
            continue;
        }
        match c.named {
            Some(s) if signers.contains(&s) && c.parsed.signed_by(&s) => ctx.identity_ledger.own(path.to_string(), bytes.to_vec()),
            Some(_) => {}
            None => {
                if !signers.iter().any(|k| c.parsed.signed.verify(k)) {
                    continue;
                }
                let new_path = revocation_key_v2(&ctx.node_id, &p, &current);
                if mycelium_core::ops::kv_get(ctx, &new_path).is_some() {
                    continue;
                }
                let event = RevocationEvent { node_id: ctx.node_id.clone(), hlc: ctx.hlc.tick(), revoked_key: p, reason: c.parsed.signed.event.reason.clone() };
                let v2 = encode_v2(event, &tls.signing_key(), vec![own_cert.clone()]);
                let _ = mycelium_core::ops::kv_set(ctx, Arc::from(new_path.as_str()), bytes::Bytes::from(v2.clone()));
                ctx.identity_ledger.own(new_path, v2);
            }
        }
    }
}

/// Republish each of this node's own records that is missing from the store or holds other bytes.
pub(crate) fn republish_own(ctx: &CoreCtx, ledger: &IdentityLedger) -> usize {
    let mut n = 0;
    for (path, bytes) in ledger.own_records.pin().iter() {
        if mycelium_core::ops::kv_get(ctx, path).as_deref() != Some(bytes.as_slice()) {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(bytes.clone()));
            n += 1;
        }
    }
    if n > 0 {
        tracing::warn!(records = n, "republished this node's own certificate or revocation records (missing or overwritten)");
    }
    n
}

/// Revoke one of this node's own keys: a v2 record at `sys/revocation/{self}/{P}/{S}`, signed by the
/// current key S and carrying its certificate (and P's, when this node holds one). Refuses the key
/// this node signs with (rotate first) and a key that is not this node's. Idempotent per `(P, S)`: a
/// record this node already wrote for the pair is kept, not re-signed. It counts at a peer only if
/// S is anchored there and was first issued after P (or P has no standing) — `rotate_identity`
/// mints the new key's certificate, so `rotate_identity_on_compromise` satisfies it.
#[cfg(feature = "compliance")]
pub(crate) fn revoke_key(
    ctx: &TaskCtx,
    revoked_key: [u8; 32],
    reason: Option<String>,
) -> Result<(), crate::error::GossipError> {
    let tls = ctx.tls.get().ok_or(crate::error::GossipError::InvalidField {
        field:  "tls",
        reason: "revocation requires the tls identity (set GossipConfig::tls)".into(),
    })?;
    let current = tls.verifying_key_bytes();
    if revoked_key == current {
        return Err(crate::error::GossipError::InvalidField {
            field:  "revoked_key",
            reason: "this is the key this node signs with; revoking it would leave the node with no key its peers \
                     trust — rotate first (rotate_identity), then revoke the old key, or use rotate_identity_on_compromise".into(),
        });
    }
    if !own_history(ctx).contains(&revoked_key) {
        return Err(crate::error::GossipError::InvalidField {
            field:  "revoked_key",
            reason: "not one of this node's keys; a node revokes only its own".into(),
        });
    }
    let path = revocation_key_v2(&ctx.node_id, &revoked_key, &current);
    let existing = ctx.identity_ledger.own_records.pin().get(&path).cloned()
        .or_else(|| mycelium_core::ops::kv_get(ctx, &path).map(|b| b.to_vec()));
    if let Some(bytes) = existing
        && candidate(&ctx.node_id, &path, &bytes).is_some_and(|c| c.parsed.signed_by(&current))
    {
        ctx.identity_ledger.own(path.clone(), bytes.clone());
        if mycelium_core::ops::kv_get(ctx, &path).as_deref() != Some(bytes.as_slice()) {
            let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(bytes));
        }
        refresh(ctx);
        return Ok(());
    }
    let mut certs = vec![mycelium_core::ops::kv_get(ctx, &cert_record_key(&ctx.node_id, &current)).map(|b| b.to_vec()).unwrap_or_else(|| tls.node_cert_der())];
    if let Some(p_cert) = mycelium_core::ops::kv_get(ctx, &cert_record_key(&ctx.node_id, &revoked_key)) {
        certs.push(p_cert.to_vec());
    }
    let event = RevocationEvent { node_id: ctx.node_id.clone(), hlc: ctx.hlc.tick(), revoked_key, reason };
    let bytes = encode_v2(event, &tls.signing_key(), certs);
    let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(bytes.clone()));
    ctx.identity_ledger.own(path, bytes);
    // Our own verifiers stop at once, not when the watcher next runs.
    refresh(ctx);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn key(seed: u8) -> ([u8; 32], SigningKey) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        (sk.verifying_key().to_bytes(), sk)
    }

    #[test]
    fn signed_revocation_round_trips_and_verifies() {
        let (pub_a, sk_a) = key(1);
        let event = RevocationEvent {
            node_id: "127.0.0.1:9000".parse().unwrap(),
            hlc: 42,
            revoked_key: [9u8; 32],
            reason: Some("compromise".into()),
        };
        let signed = SignedRevocation::sign(event, &sk_a);
        assert!(signed.verify(&pub_a), "verifies against the signer");
        let (pub_b, _) = key(2);
        assert!(!signed.verify(&pub_b), "rejects a different key");
        let round = SignedRevocation::decode(&signed.encode()).unwrap();
        assert_eq!(round, signed, "encode/decode round-trips");
    }

    #[test]
    fn a_v2_record_is_a_v1_record_followed_by_a_trailer_both_readers_decode() {
        let (pub_a, sk_a) = key(1);
        let event = RevocationEvent { node_id: "127.0.0.1:9000".parse().unwrap(), hlc: 7, revoked_key: [9u8; 32], reason: None };
        let v2 = encode_v2(event.clone(), &sk_a, vec![vec![1, 2, 3]]);
        // A 2.32 reader decodes the prefix and verifies it.
        let v1 = SignedRevocation::decode(&v2).expect("the v1 prefix decodes");
        assert_eq!(v1.event, event);
        assert!(v1.verify(&pub_a));
        let parsed = parse_revocation(&v2).expect("v2 parses");
        assert!(parsed.signed_by(&pub_a));
        assert_eq!(parsed.trailer.as_ref().unwrap().revoker, pub_a);
        let (pub_b, _) = key(2);
        assert!(!parsed.signed_by(&pub_b));
        // A trailer whose certificates were swapped no longer verifies.
        let mut tampered = parsed.clone();
        tampered.trailer.as_mut().unwrap().certs = vec![vec![4]];
        assert!(!tampered.signed_by(&pub_a));
        // Garbage after the prefix that is not a trailer is refused.
        let mut junk = v1.encode().to_vec();
        junk.extend_from_slice(b"junk");
        assert!(parse_revocation(&junk).is_none());
    }

    #[test]
    fn a_path_must_agree_with_its_body_before_any_signature_is_checked() {
        let (pub_a, sk_a) = key(1);
        let node: NodeId = "127.0.0.1:9000".parse().unwrap();
        let event = RevocationEvent { node_id: node.clone(), hlc: 7, revoked_key: [9u8; 32], reason: None };
        let v2 = encode_v2(event, &sk_a, vec![]);
        assert!(candidate(&node, &revocation_key_v2(&node, &[9u8; 32], &pub_a), &v2).is_some());
        assert!(candidate(&node, &revocation_key_v2(&node, &[8u8; 32], &pub_a), &v2).is_none(), "another revoked key in the path");
        assert!(candidate(&node, &revocation_key_v2(&node, &[9u8; 32], &[5u8; 32]), &v2).is_none(), "another revoker in the path");
        let other: NodeId = "127.0.0.1:9001".parse().unwrap();
        assert!(candidate(&other, &revocation_key_v2(&other, &[9u8; 32], &pub_a), &v2).is_none(), "another node");
    }

    fn fixture(file: &str) -> Vec<(String, Vec<u8>)> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/persistence/identity-journal-v1").join(file);
        std::fs::read_to_string(path).unwrap().lines().map(|l| {
            let (name, hex) = l.split_once(' ').unwrap();
            (name.to_string(), (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect())
        }).collect()
    }

    /// **#805's review F6: the revocation journal's entries are pinned.** Each variant encodes to the golden bytes and the
    /// golden bytes decode to it; a reader with only the variants before I2's PR 2a (an older binary) refuses `Anchor` —
    /// which it skips, with its warning — and reads the others. A variant reordered or inserted in the middle fails here.
    /// Seen failing first with `Anchor` moved before `Saturated`.
    #[test]
    fn the_identity_journal_entries_are_pinned() {
        #[derive(serde::Deserialize)]
        #[allow(dead_code)]
        enum Before { Counted { node: String, path: String, bytes: Vec<u8> }, Saturated { node: String } }
        let node = "127.0.0.1:9000".to_string();
        let entries = [
            ("Counted", JournalEntry::Counted { node: node.clone(), path: "sys/revocation/127.0.0.1:9000/aa/bb".into(), bytes: vec![1, 2, 3] }),
            ("Saturated", JournalEntry::Saturated { node: node.clone() }),
            ("Anchor", JournalEntry::Anchor { node: node.clone(), der: vec![4, 5, 6] }),
        ];
        let golden = fixture("revocations.hex");
        assert_eq!(golden.len(), entries.len(), "one line per variant");
        for ((name, entry), (gname, bytes)) in entries.iter().zip(&golden) {
            assert_eq!(name, gname);
            assert_eq!(&mycelium_core::serde_fixint::to_vec(entry).unwrap(), bytes, "{name} encodes to the golden bytes");
            let back: JournalEntry = mycelium_core::serde_fixint::from_slice(bytes).unwrap();
            assert_eq!(mycelium_core::serde_fixint::to_vec(&back).unwrap(), *bytes, "{name} decodes back");
            let old = mycelium_core::serde_fixint::from_slice::<Before>(bytes);
            assert_eq!(old.is_ok(), *name != "Anchor", "an older reader reads {name}: {}", old.is_ok());
        }
    }

    fn temp_journal(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("myc-i2-anchor-journal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("revocations.journal")
    }

    fn digest(der: &[u8]) -> [u8; 32] {
        Sha256::digest(der).into()
    }

    /// **#805's confirm round N1 (and F2): a caller waits per certificate, whichever fold started its append.** The
    /// append of X is held, and fails, as an append started before the caller began; the caller's re-fold offers X again,
    /// which starts nothing (it is in flight), and the wait for X does not answer while X is in flight — not even once a
    /// later append finishes — and then answers `false`, X not journaled. The reviewer's probe. Seen failing first with
    /// the wait counting only the appends numbered after its mark: it answered `true` at once.
    #[tokio::test]
    async fn a_kept_anchor_in_flight_before_the_wait_is_waited_for() {
        let ledger = IdentityLedger::default();
        ledger.open_journal(&temp_journal("inflight")).unwrap();
        let n: NodeId = "127.0.0.1:9100".parse().unwrap();
        let (x, y) = ([1u8; 8], [2u8; 8]);
        let before = ledger.anchor_mark();
        ledger.hook_appends_for_test(Some(before), Some(before));
        ledger.journal_anchor(&n, &x);
        ledger.journal_anchor(&n, &x);
        assert_eq!(ledger.anchor_mark(), before + 1, "re-offered while in flight: no second append");
        let only_x = [(n.clone(), digest(&x))];
        let wait = ledger.anchors_durable_for(&only_x);
        tokio::pin!(wait);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), &mut wait).await.is_err(), "X in flight: no answer");
        ledger.journal_anchor(&n, &y);
        assert!(ledger.anchors_durable_for(&[(n.clone(), digest(&y))]).await, "Y durable on its own");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(100), &mut wait).await.is_err(), "a later append finishing first satisfies nothing");
        let answered = loop {
            ledger.release_held_for_test();
            if let Ok(a) = tokio::time::timeout(std::time::Duration::from_millis(20), &mut wait).await {
                break a;
            }
        };
        assert!(!answered, "X's append failed: not durable");
        assert!(!ledger.anchor_journaled_for_test(&n, &x));
        assert!(ledger.anchors_durable_for(&[(n.clone(), digest(&y))]).await, "X's failure is X's alone");
        assert_eq!(ledger.anchor_append_failed.load(Ordering::Relaxed), 1, "counted");
    }

    /// **#805's last item: an append task that panics settles its certificate as failed.** The task panics before it
    /// appends; the drop guard takes the certificate out of the in-flight set, counts the failure and wakes the waiters,
    /// so the wait answers `false` and a later offer, after the backoff, journals it. Seen failing first with the guard's
    /// `Drop` removed: the certificate stayed in flight and the wait never answered.
    #[tokio::test]
    async fn a_panicking_anchor_append_settles_as_failed() {
        let ledger = IdentityLedger::default();
        ledger.open_journal(&temp_journal("panic")).unwrap();
        let n: NodeId = "127.0.0.1:9103".parse().unwrap();
        let x = [9u8; 8];
        let first = ledger.anchor_mark();
        ledger.panic_append_for_test(first);
        ledger.journal_anchor(&n, &x);
        let only_x = [(n.clone(), digest(&x))];
        let answered = tokio::time::timeout(std::time::Duration::from_secs(3), ledger.anchors_durable_for(&only_x)).await;
        assert_eq!(answered.ok(), Some(false), "the wait answers, and says not durable");
        assert_eq!(ledger.anchor_append_failed.load(Ordering::Relaxed), 1, "counted as a failure");
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        ledger.journal_anchor(&n, &x);
        assert!(ledger.anchors_durable_for(&only_x).await, "retried after the backoff, durable");
    }

    /// **#805's confirm round N3: a failed append backs off.** Right after X's append failed, offering it again starts no
    /// append; once its backoff (1 s for a first failure) has passed, it does, and X is journaled. Seen failing first with
    /// the backoff check removed: the retry started at once.
    #[tokio::test]
    async fn a_failed_anchor_append_is_retried_only_after_its_backoff() {
        let ledger = IdentityLedger::default();
        ledger.open_journal(&temp_journal("backoff")).unwrap();
        let n: NodeId = "127.0.0.1:9101".parse().unwrap();
        let x = [3u8; 8];
        let first = ledger.anchor_mark();
        ledger.hook_appends_for_test(None, Some(first));
        ledger.journal_anchor(&n, &x);
        assert!(!ledger.anchors_durable_for(&[(n.clone(), digest(&x))]).await, "failed");
        ledger.journal_anchor(&n, &x);
        assert_eq!(ledger.anchor_mark(), first + 1, "inside the backoff: no retry");
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        ledger.journal_anchor(&n, &x);
        assert_eq!(ledger.anchor_mark(), first + 2, "after the backoff: retried");
        assert!(ledger.anchors_durable_for(&[(n.clone(), digest(&x))]).await, "durable");
    }

    fn key_of(i: usize) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..8].copy_from_slice(&(i as u64).to_le_bytes());
        k
    }

    fn cert_of(i: usize) -> [u8; 8] {
        let mut c = [0xABu8; 8];
        c[..2].copy_from_slice(&(i as u16).to_le_bytes());
        c
    }

    /// Keep and journal certificates `range` of `n`, and wait until each is durable.
    async fn keep_and_journal(ledger: &IdentityLedger, n: &NodeId, range: std::ops::Range<usize>) {
        for i in range.clone() {
            ledger.keep_raw_for_test(n, key_of(i), &cert_of(i), true);
            ledger.journal_anchor(n, &cert_of(i));
        }
        let all: Vec<(NodeId, [u8; 32])> = range.map(|i| (n.clone(), digest(&cert_of(i)))).collect();
        assert!(ledger.anchors_durable_for(&all).await);
    }

    async fn until_true(what: &str, f: impl Fn() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("timed out: {what}");
    }

    /// **A caller waiting for a fold is answered by the fold another thread is running** (the CI hang of 2026-11-21:
    /// `an_often_rotated_peer_does_not_exhaust_the_kept_anchor_budget` ran until the six-hour limit, twice). A fold is
    /// running on one thread, slowed by a plant, when a caller on another asks for a fold that starts after its call —
    /// as `change_roots` and the succession tests' `refold` do while the identity watcher folds. The caller is answered,
    /// and the running fold stops after the one re-run it owes. Seen failing first with the waiter accepting only a
    /// fold it ran itself (`while !refresh_now(ctx) {}`): each retry set `again`, the running fold re-ran on every
    /// pass, and neither thread returned (a livelock, the answer timing out here with hundreds of folds run).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_waiter_is_answered_by_a_fold_another_thread_is_running() {
        let agent = crate::GossipAgent::new(NodeId::new("127.0.0.1", crate::test_util::alloc_port()).unwrap(), crate::config::GossipConfig::default());
        let ctx = Arc::clone(agent.task_ctx_for_tests());
        let folds = Arc::new(AtomicU64::new(0));
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let started_tx = std::sync::Mutex::new(Some(started_tx));
        let f = Arc::clone(&folds);
        ctx.identity_ledger.fold_plant.store(Some(Arc::new(Box::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
            if let Some(tx) = started_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            // Long beside a waiter's retry, as a fold over forty certificates is.
            std::thread::sleep(std::time::Duration::from_millis(20));
        }))));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<&'static str>();
        let (c1, d1) = (Arc::clone(&ctx), done_tx.clone());
        std::thread::spawn(move || {
            refresh(&c1);
            let _ = d1.send("the running fold");
        });
        started_rx.recv_timeout(std::time::Duration::from_secs(10)).expect("the first fold started");
        let (c2, d2) = (Arc::clone(&ctx), done_tx);
        std::thread::spawn(move || {
            let ticket = fold_ticket(&c2);
            while !refresh_answers(&c2, ticket) {
                std::thread::yield_now();
            }
            let _ = d2.send("the waiter");
        });
        for _ in 0..2 {
            let who = done_rx.recv_timeout(std::time::Duration::from_secs(10));
            assert!(who.is_ok(), "both returned within 10 s (folds run: {}): a waiter and a running fold livelocked", folds.load(Ordering::SeqCst));
        }
        ctx.identity_ledger.fold_plant.store(None);
        assert!(folds.load(Ordering::SeqCst) <= 4, "the running fold re-ran for the waiter a bounded number of times, not for ever ({} folds)", folds.load(Ordering::SeqCst));
    }

    /// **#805's confirm round N3, under the decision of 2026-11-17: a node keeping more than the budget at once is still
    /// refused by name, once per certificate.** A node's 32 kept certificates are journaled; a 33rd kept beside them,
    /// offered on fold after fold, is refused, warned and counted once, and no compaction runs: nothing in the journal
    /// is dead. Seen failing first with the refusal counted on every offer (N3), and with the budget treating every
    /// full journal as compactable (the refusal never counted).
    #[tokio::test]
    async fn a_certificate_past_the_anchor_budget_is_counted_once() {
        let ledger = IdentityLedger::default();
        ledger.open_journal(&temp_journal("budget")).unwrap();
        let n: NodeId = "127.0.0.1:9102".parse().unwrap();
        keep_and_journal(&ledger, &n, 0..ANCHOR_JOURNAL_PER_NODE).await;
        let extra = cert_of(900);
        ledger.keep_raw_for_test(&n, key_of(900), &extra, true);
        for _ in 0..3 {
            ledger.journal_anchor(&n, &extra);
        }
        assert_eq!(ledger.anchor_journal_refused.load(Ordering::Relaxed), 1, "once per certificate");
        assert!(!ledger.anchors_durable_for(&[(n.clone(), digest(&extra))]).await, "not durable: in memory only");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(ledger.compaction.removed.load(Ordering::Relaxed), 0, "every journaled anchor is kept: nothing to compact");
        assert_eq!(ledger.anchors_journaled_count_for_test(&n), ANCHOR_JOURNAL_PER_NODE);
    }

    /// **The decision of 2026-11-17: the budget counts the anchors a node keeps now, not every one it ever kept.** A
    /// node's 32 kept certificates are journaled beside a counted revocation; it stops keeping five of them and keeps a
    /// new one. The new one meets a full budget, the journal is compacted — the five dropped anchors' records leave it
    /// — and the next offer journals it: every kept certificate is durable again, nothing was refused, and a restart
    /// over the compacted journal reads back exactly the kept anchors and the counted revocation. Seen failing first
    /// with the budget refusing whenever the journal is full (no compaction): the new certificate was refused and
    /// `anchors_durable_for(kept_certs())` answered `false`.
    #[tokio::test]
    async fn the_anchor_budget_counts_the_anchors_kept_now() {
        let path = temp_journal("compact");
        let ledger = IdentityLedger::default();
        ledger.open_journal(&path).unwrap();
        let n: NodeId = "127.0.0.1:9103".parse().unwrap();
        assert!(ledger.record_counted(&n, "sys/revocation/127.0.0.1:9103/aa/bb", b"counted"));
        keep_and_journal(&ledger, &n, 0..ANCHOR_JOURNAL_PER_NODE).await;
        for i in 0..5 {
            ledger.keep_raw_for_test(&n, key_of(i), &cert_of(i), false);
        }
        let fresh = cert_of(700);
        ledger.keep_raw_for_test(&n, key_of(700), &fresh, true);
        ledger.journal_anchor(&n, &fresh);
        until_true("the compaction removed the five dead records", || ledger.compaction.removed.load(Ordering::Relaxed) == 5).await;
        assert_eq!(ledger.anchors_journaled_count_for_test(&n), ANCHOR_JOURNAL_PER_NODE - 5, "the dropped anchors stopped counting");
        ledger.journal_anchor(&n, &fresh);
        assert!(ledger.anchors_durable_for(&ledger.kept_certs()).await, "every kept certificate is journaled again");
        assert_eq!(ledger.anchor_journal_refused.load(Ordering::Relaxed), 0, "nothing was refused");
        ledger.close_journal();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let records = super::super::journal::read_journal(&path).unwrap();
        let (mut anchors, mut counted) = (HashSet::new(), 0);
        for r in &records {
            match mycelium_core::serde_fixint::from_slice::<JournalEntry>(r).unwrap() {
                JournalEntry::Anchor { der, .. } => assert!(anchors.insert(der), "one record per kept certificate"),
                JournalEntry::Counted { bytes, .. } => {
                    assert_eq!(bytes, b"counted");
                    counted += 1;
                }
                JournalEntry::Saturated { .. } => panic!("no node saturated"),
            }
        }
        let want: HashSet<Vec<u8>> = (5..ANCHOR_JOURNAL_PER_NODE).map(|i| cert_of(i).to_vec()).chain([fresh.to_vec()]).collect();
        assert_eq!(anchors, want, "exactly the anchors kept");
        assert_eq!(counted, 1, "the counted revocation is untouched");
    }

    /// Four kept anchors and a counted revocation journaled at `path`, two of the anchors then dropped: the state both
    /// crash windows start from. Returns the ledger and the node.
    async fn before_a_compaction(path: &std::path::Path, crash_at: u8) -> (IdentityLedger, NodeId) {
        let ledger = IdentityLedger::default();
        ledger.open_journal_crashing_for_test(path, crash_at).unwrap();
        let n: NodeId = "127.0.0.1:9104".parse().unwrap();
        assert!(ledger.record_counted(&n, "sys/revocation/127.0.0.1:9104/aa/bb", b"counted"));
        keep_and_journal(&ledger, &n, 0..4).await;
        until_true("the counted record is on disk", || super::super::journal::read_journal(path).unwrap().len() == 5).await;
        for i in 0..2 {
            ledger.keep_raw_for_test(&n, key_of(i), &cert_of(i), false);
        }
        (ledger, n)
    }

    fn anchor_ders(path: &std::path::Path) -> Vec<Vec<u8>> {
        super::super::journal::read_journal(path).unwrap().iter().filter_map(|r| match mycelium_core::serde_fixint::from_slice::<JournalEntry>(r) {
            Ok(JournalEntry::Anchor { der, .. }) => Some(der),
            _ => None,
        }).collect()
    }

    fn compact_tmp(path: &std::path::Path) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("{}.compact", path.display()))
    }

    /// **The compaction's first crash window: a crash before the rename.** The live set is written and synced under the
    /// temporary name and the process stops. The journal is byte for byte what it was — every anchor record, dropped
    /// or kept, and the counted revocation — the compaction is counted as failed, and the next open removes the
    /// temporary and reads the whole old journal. Seen failing first with the live set written over the journal's own
    /// name (no temporary, no rename): the dropped anchors' records were already gone at the crash.
    #[tokio::test]
    async fn a_crash_before_the_compactions_rename_leaves_the_journal_as_it_was() {
        let path = temp_journal("crash-before");
        let (ledger, n) = before_a_compaction(&path, 1).await;
        let before = std::fs::read(&path).unwrap();
        assert_eq!(ledger.compact_anchor_journal().await, 0, "the compaction did not finish");
        assert_eq!(ledger.compaction.failed.load(Ordering::Relaxed), 1);
        assert_eq!(std::fs::read(&path).unwrap(), before, "the journal is untouched");
        assert!(compact_tmp(&path).exists(), "the temporary a crash leaves");
        ledger.close_journal();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let again = IdentityLedger::default();
        again.open_journal(&path).unwrap();
        assert!(!compact_tmp(&path).exists(), "the next open removes the temporary");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(anchor_ders(&path).len(), 4, "every anchor record read back");
        assert_eq!(again.journaled.pin().get(&n).map_or(0, |v| v.len()), 1, "and the counted revocation");
    }

    /// **A compaction that fails before its rename leaves the bookkeeping true.** The temporary's name is taken, so
    /// the rewrite changes nothing: the failure is counted, and the two dropped anchors' records — still on disk — are
    /// still counted as journaled, so the next compaction, once the name is free, removes them. Seen failing first
    /// without the records put back: the bookkeeping said two while the file held four, nothing was dead any more by
    /// its count, and the second compaction removed nothing.
    #[tokio::test]
    async fn a_compaction_that_changes_nothing_leaves_the_journaled_set_as_it_was() {
        let path = temp_journal("unchanged");
        let (ledger, n) = before_a_compaction(&path, 0).await;
        std::fs::create_dir(compact_tmp(&path)).unwrap();
        assert_eq!(ledger.compact_anchor_journal().await, 0);
        assert_eq!(ledger.compaction.failed.load(Ordering::Relaxed), 1);
        assert_eq!(anchor_ders(&path).len(), 4, "the journal is as it was");
        assert_eq!(ledger.anchors_journaled_count_for_test(&n), 4, "and so is what this node counts as journaled");
        std::fs::remove_dir(compact_tmp(&path)).unwrap();
        assert_eq!(ledger.compact_anchor_journal().await, 2, "the next compaction removes the two dropped anchors");
        assert_eq!(anchor_ders(&path).len(), 2);
        assert_eq!(ledger.anchors_journaled_count_for_test(&n), 2);
    }

    fn on_disk(path: &std::path::Path, der: &[u8]) -> bool {
        anchor_ders(path).iter().any(|r| r.as_slice() == der)
    }

    /// **#812's review F2: an append that settles after a compaction is not believed durable unless it is on disk.**
    /// K is kept and journaled, D is journaled and dropped (so the compaction has a record to remove). X is kept and
    /// its append handed to the writer; before that append settles, X stops being kept and a compaction is queued
    /// behind it. The writer appends X, then rewrites: X is in flight, so its frame is kept, and the settle that
    /// follows — putting X on the journaled set — is true. Kept again, X is durable and on disk. (Current-thread
    /// runtime: the order is the scheduler's.) The durable claim is asserted before the count of removed records, so
    /// it is the claim that fails. Seen failing first with the compaction not consulting the in-flight set: X was on
    /// the journaled set and the journal did not hold it.
    #[tokio::test]
    async fn an_append_settling_after_a_compaction_is_on_disk_when_it_is_believed_durable() {
        let path = temp_journal("inflight-compact");
        let ledger = Arc::new(IdentityLedger::default());
        ledger.open_journal(&path).unwrap();
        let n: NodeId = "127.0.0.1:9105".parse().unwrap();
        keep_and_journal(&ledger, &n, 0..2).await; // K = 0, D = 1
        ledger.keep_raw_for_test(&n, key_of(1), &cert_of(1), false);
        let x = cert_of(2);
        ledger.keep_raw_for_test(&n, key_of(2), &x, true);
        ledger.journal_anchor(&n, &x);
        let (l2, n2) = (Arc::clone(&ledger), n.clone());
        let compaction = tokio::spawn(async move {
            l2.keep_raw_for_test(&n2, key_of(2), &cert_of(2), false);
            l2.compact_anchor_journal().await
        });
        let removed = tokio::time::timeout(std::time::Duration::from_secs(10), compaction).await.expect("bounded").unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), ledger.anchors_durable_for(&[(n.clone(), digest(&x))])).await.expect("bounded");
        // The claim itself, first (#812's confirm round): believed durable only when the journal holds it.
        assert!(!ledger.anchor_journaled_for_test(&n, &x) || on_disk(&path, &x), "X is on the journaled set and the journal does not hold it");
        ledger.keep_raw_for_test(&n, key_of(2), &x, true);
        ledger.journal_anchor(&n, &x);
        let durable = tokio::time::timeout(std::time::Duration::from_secs(10), ledger.anchors_durable_for(&ledger.kept_certs())).await.expect("bounded");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!durable || on_disk(&path, &x), "anchors_durable_for answered true for a kept certificate the journal does not hold");
        assert!(durable, "X kept again: durable");
        assert_eq!(removed, 1, "D's record, and only D's, left the journal");
        assert_eq!(ledger.dead_journaled(None), 0, "nothing is counted dead that no record stands for");
    }

    /// **#812's review F2, concurrently and with no plant (the confirm round's run, adopted): keeping, dropping and
    /// compacting at once never leaves the journaled set naming a record the journal does not hold.** On a
    /// multi-thread runtime one task keeps, offers and drops three certificates while another compacts; at each
    /// quiescent point the journaled set must be a subset of the file. 400 rounds, bounded at 40 s. It witnesses the
    /// in-flight rule; the second look's window is a few instructions wide and only the planted test above reaches
    /// it. Seen failing first with the compaction not consulting the in-flight set: a phantom within the first rounds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn keeping_dropping_and_compacting_at_once_never_believes_what_is_not_on_disk() {
        let path = temp_journal("stress");
        let ledger = Arc::new(IdentityLedger::default());
        ledger.open_journal(&path).unwrap();
        let n: NodeId = "127.0.0.1:9110".parse().unwrap();
        keep_and_journal(&ledger, &n, 0..1).await; // K stays kept, so no dead record is retained as the peer's memory
        let started = std::time::Instant::now();
        let (mut rounds, mut removed_total) = (0u32, 0u64);
        while rounds < 400 && started.elapsed() < std::time::Duration::from_secs(40) {
            rounds += 1;
            let (l1, n1) = (Arc::clone(&ledger), n.clone());
            let toggler = tokio::spawn(async move {
                for i in 0..40usize {
                    let c = 2 + (i % 3);
                    l1.keep_raw_for_test(&n1, key_of(c), &cert_of(c), true);
                    l1.journal_anchor(&n1, &cert_of(c));
                    tokio::task::yield_now().await;
                    l1.keep_raw_for_test(&n1, key_of(c), &cert_of(c), false);
                    tokio::task::yield_now().await;
                }
            });
            let l2 = Arc::clone(&ledger);
            let compactor = tokio::spawn(async move {
                let mut removed = 0;
                for _ in 0..40 {
                    removed += l2.compact_anchor_journal().await;
                    tokio::task::yield_now().await;
                }
                removed
            });
            tokio::time::timeout(std::time::Duration::from_secs(20), toggler).await.expect("bounded").unwrap();
            removed_total += tokio::time::timeout(std::time::Duration::from_secs(20), compactor).await.expect("bounded").unwrap();
            // Quiesce: no append in flight for this peer (bounded).
            let all: Vec<(NodeId, [u8; 32])> = (0..6).map(|i| (n.clone(), digest(&cert_of(i)))).collect();
            tokio::time::timeout(std::time::Duration::from_secs(10), ledger.anchors_durable_for(&all)).await.expect("bounded");
            let believed: HashSet<[u8; 32]> = ledger.anchor_tally.journaled.pin().get(&n).cloned().unwrap_or_default();
            let disk: HashSet<[u8; 32]> = anchor_ders(&path).iter().map(|d| digest(d)).collect();
            let phantom = believed.difference(&disk).count();
            assert_eq!(phantom, 0, "round {rounds}: certificates on the journaled set that the journal does not hold (compactions removed {removed_total} so far)");
        }
        assert!(removed_total > 0, "the compaction never had work: the run witnessed nothing");
    }

    /// **#812's review F2, the narrower window: a certificate kept again while the compaction is deciding about it.**
    /// D is journaled and dropped. Between the compaction's first look at D's record and its taking D off the
    /// journaled set (a plant), the fold keeps D again and offers it to the journal, which appends nothing: D is on
    /// the journaled set. The compaction looks again, sees D kept, puts it back and keeps the frame — D is believed
    /// durable and the journal holds it. Seen failing first without the second look: D's frame was left out while D
    /// was kept.
    #[tokio::test]
    async fn a_certificate_kept_again_while_the_compaction_decides_keeps_its_record() {
        let path = temp_journal("relook");
        let ledger = Arc::new(IdentityLedger::default());
        ledger.open_journal(&path).unwrap();
        let n: NodeId = "127.0.0.1:9106".parse().unwrap();
        keep_and_journal(&ledger, &n, 0..2).await; // K = 0, D = 1
        let d = cert_of(1);
        ledger.keep_raw_for_test(&n, key_of(1), &d, false);
        let l2 = Arc::clone(&ledger);
        let plant: BetweenLooks = Box::new(move |node, der| {
            l2.keep_raw_for_test(node, key_of(1), der, true);
            l2.journal_anchor(node, der);
        });
        ledger.compaction.between_looks.store(Some(Arc::new(plant)));
        tokio::time::timeout(std::time::Duration::from_secs(10), ledger.compact_anchor_journal()).await.expect("bounded");
        ledger.compaction.between_looks.store(None);
        assert!(on_disk(&path, &d), "D is kept: its record stays");
        assert!(ledger.anchor_journaled_for_test(&n, &d));
        assert!(tokio::time::timeout(std::time::Duration::from_secs(10), ledger.anchors_durable_for(&ledger.kept_certs())).await.expect("bounded"));
    }

    /// **#812's review F7: a record this binary would not restore is not the compaction's to remove.** An `Anchor`
    /// whose certificate does not parse here — a newer binary's, read after a downgrade — is kept through a compaction
    /// that removes a dead record beside it. Seen failing first with such a record judged like any other: it was not
    /// kept, so it was removed.
    #[tokio::test]
    async fn a_compaction_keeps_an_anchor_record_this_binary_would_not_restore() {
        let path = temp_journal("unrestorable");
        let ledger = IdentityLedger::default();
        ledger.open_journal(&path).unwrap();
        let n: NodeId = "127.0.0.1:9107".parse().unwrap();
        let foreign = b"a newer binary's certificate".to_vec();
        let entry = JournalEntry::Anchor { node: n.to_string(), der: foreign.clone() };
        ledger.journal.load_full().unwrap().append(mycelium_core::serde_fixint::to_vec(&entry).unwrap()).await.unwrap();
        keep_and_journal(&ledger, &n, 0..2).await;
        ledger.keep_raw_for_test(&n, key_of(1), &cert_of(1), false);
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), ledger.compact_anchor_journal()).await.expect("bounded"), 1);
        assert!(on_disk(&path, &foreign), "kept as it is");
        assert!(!on_disk(&path, &cert_of(1)), "the dead record beside it went");
    }

    /// **#812's review F1, at the journal: the last record of a peer this node keeps nothing of stays.** Three anchors
    /// of a peer are journaled and all three dropped: nothing is removable by the node's own count, and a compaction
    /// asked for all the same keeps one — the memory a restart rebuilds the peer's entry from. Once a certificate of
    /// the peer is kept and journaled again, the dead records go. Seen failing first with every dead record
    /// removable: the journal held no record of the peer.
    #[tokio::test]
    async fn a_compaction_keeps_the_last_record_of_a_peer_with_nothing_kept() {
        let path = temp_journal("last-record");
        let ledger = IdentityLedger::default();
        ledger.open_journal(&path).unwrap();
        let (n, other): (NodeId, NodeId) = ("127.0.0.1:9108".parse().unwrap(), "127.0.0.1:9109".parse().unwrap());
        keep_and_journal(&ledger, &n, 0..3).await;
        // Another peer with a dead record, so the compaction has work whatever it decides about N.
        keep_and_journal(&ledger, &other, 10..12).await;
        ledger.keep_raw_for_test(&other, key_of(11), &cert_of(11), false);
        for i in 0..3 {
            ledger.keep_raw_for_test(&n, key_of(i), &cert_of(i), false);
        }
        assert_eq!(ledger.dead_journaled(Some(&n)), 2, "one of N's three dead records is its memory");
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), ledger.compact_anchor_journal()).await.expect("bounded"), 3);
        let of_n = |path: &std::path::Path| (0..3).filter(|i| on_disk(path, &cert_of(*i))).count();
        assert_eq!(of_n(&path), 1, "exactly one record of N is left");
        assert_eq!(ledger.anchors_journaled_count_for_test(&n), 1, "and it is still counted as journaled");
        keep_and_journal(&ledger, &n, 5..6).await;
        assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(10), ledger.compact_anchor_journal()).await.expect("bounded"), 1);
        assert_eq!(of_n(&path), 0, "N is anchored on disk again: its dead record goes");
        assert!(on_disk(&path, &cert_of(5)));
    }

    /// **The compaction's second crash window: a crash after the rename**, before the directory is synced and the
    /// writer has reopened the file. The journal under its name is the compacted one, whole: the two kept anchors and
    /// the counted revocation, no temporary beside it; a restart reads exactly those. The stopped writer refuses the
    /// next append rather than write to the replaced file. Seen failing first with the writer left unpoisoned after
    /// the stop: the append was acknowledged, and absent from the journal at the restart.
    #[tokio::test]
    async fn a_crash_after_the_compactions_rename_leaves_the_compacted_journal_whole() {
        let path = temp_journal("crash-after");
        let (ledger, n) = before_a_compaction(&path, 2).await;
        assert_eq!(ledger.compact_anchor_journal().await, 0, "the compaction reported no success");
        assert!(!compact_tmp(&path).exists(), "renamed");
        let want: Vec<Vec<u8>> = (2..4).map(|i| cert_of(i).to_vec()).collect();
        assert_eq!(anchor_ders(&path), want, "the compacted journal, whole");
        // The writer still holds the replaced file: an append now must not be acknowledged.
        let late = cert_of(800);
        ledger.keep_raw_for_test(&n, key_of(800), &late, true);
        ledger.journal_anchor(&n, &late);
        assert!(!ledger.anchors_durable_for(&[(n.clone(), digest(&late))]).await, "an append after the stop is refused, not lost silently");
        ledger.close_journal();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let again = IdentityLedger::default();
        again.open_journal(&path).unwrap();
        assert_eq!(anchor_ders(&path), want);
        assert_eq!(again.journaled.pin().get(&n).map_or(0, |v| v.len()), 1, "and the counted revocation");
    }

    #[test]
    fn tampering_the_event_breaks_the_signature() {
        let (pub_a, sk_a) = key(1);
        let event = RevocationEvent {
            node_id: "127.0.0.1:9000".parse().unwrap(),
            hlc: 1, revoked_key: [9u8; 32], reason: None,
        };
        let mut signed = SignedRevocation::sign(event, &sk_a);
        signed.event.revoked_key = [7u8; 32]; // tamper
        assert!(!signed.verify(&pub_a), "a tampered event fails verification");
    }
}
