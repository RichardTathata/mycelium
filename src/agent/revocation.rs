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

/// The kept-anchor appends in flight (I2, R4).
#[derive(Default)]
struct AppendTally {
    started: AtomicU64,
    done: AtomicU64,
    failed: AtomicU64,
    notify: tokio::sync::Notify,
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
    anchors_seen: papaya::HashMap<NodeId, HashMap<[u8; 32], KeptAnchor>>,
    /// Kept certificates journaled (read at start, and appended since), per node, by digest.
    anchors_journaled: papaya::HashMap<NodeId, HashSet<[u8; 32]>>,
    anchors_journaled_total: AtomicUsize,
    /// Anchor appends handed to the journal, and those finished — what [`IdentityLedger::anchors_durable`] waits on.
    anchor_tally: Arc<AppendTally>,
    /// Kept certificates the journal could not take (budget full, or a failed append): kept in memory only.
    pub(crate) anchor_journal_refused: AtomicU64,
    /// Kept certificates dropped because they no longer verify against this node's roots (I2, R4).
    pub(crate) anchors_dropped: AtomicU64,
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
    cert_cache: papaya::HashMap<[u8; 32], (u64, Option<CertFacts>)>,
    running: AtomicBool,
    again: AtomicBool,
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
        let j = super::journal::Journal::open(path, "identity/revocations")?;
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
        let mut added = false;
        self.anchors_journaled.pin().compute(node.clone(), |existing| {
            let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
            added = set.insert(digest);
            if added { papaya::Operation::Insert(set) } else { papaya::Operation::Abort(()) }
        });
        if added {
            self.anchors_journaled_total.fetch_add(1, Ordering::Relaxed);
        }
        let cert = KeptCert { der: Arc::from(der), issuer, issued: id.issued_ms.unwrap_or(0), judged: UNJUDGED };
        self.anchors_seen.pin().compute(node.clone(), |existing| {
            let mut m = existing.map(|(_, m)| m.clone()).unwrap_or_default();
            m.entry(id.key).or_default().keep(cert.clone());
            papaya::Operation::Insert::<_, ()>(m)
        });
    }

    /// Wait until every kept-anchor append handed to the journal before this call has finished (identity-and-lifecycle
    /// I2, R4: the append is awaited before the Narrowed phase is persisted). `true` when no anchor append has failed in this process; with no
    /// journal there is nothing to wait for and the anchors are in memory only, which is `true` too — durability across
    /// a restart is stated under `[persistence]`.
    pub(crate) async fn anchors_durable(&self) -> bool {
        let t = Arc::clone(&self.anchor_tally);
        let target = t.started.load(Ordering::Acquire);
        loop {
            let notified = t.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if t.done.load(Ordering::Acquire) >= target {
                break;
            }
            notified.await;
        }
        t.failed.load(Ordering::Acquire) == 0
    }

    /// Journal a kept certificate of `node`'s, within the anchors' own budget. Held in memory first (the caller has
    /// merged it), then handed to the journal: the store-then-WAL order.
    fn journal_anchor(&self, node: &NodeId, der: &[u8]) {
        if self.journal.load().is_none() {
            return;
        }
        let digest: [u8; 32] = Sha256::digest(der).into();
        if self.anchors_journaled.pin().get(node).is_some_and(|s| s.contains(&digest)) {
            return;
        }
        let per = self.anchors_journaled.pin().get(node).map_or(0, |s| s.len());
        if per >= ANCHOR_JOURNAL_PER_NODE || self.anchors_journaled_total.load(Ordering::Relaxed) >= ANCHOR_JOURNAL_TOTAL {
            self.anchor_journal_refused.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%node, "identity journal: the kept-anchor budget is full; this anchor is kept in this process only");
            return;
        }
        let mut added = false;
        self.anchors_journaled.pin().compute(node.clone(), |existing| {
            let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
            added = set.insert(digest);
            if added { papaya::Operation::Insert(set) } else { papaya::Operation::Abort(()) }
        });
        if !added {
            return;
        }
        self.anchors_journaled_total.fetch_add(1, Ordering::Relaxed);
        self.append_anchor(JournalEntry::Anchor { node: node.to_string(), der: der.to_vec() });
    }

    /// Append a kept-anchor entry, counted so [`IdentityLedger::anchors_durable`] can wait for it.
    fn append_anchor(&self, entry: JournalEntry) {
        let Some(j) = self.journal.load_full() else { return };
        let Ok(bytes) = mycelium_core::serde_fixint::to_vec(&entry) else { return };
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        let tally = Arc::clone(&self.anchor_tally);
        tally.started.fetch_add(1, Ordering::AcqRel);
        rt.spawn(async move {
            if let Err(e) = j.append(bytes).await {
                tally.failed.fetch_add(1, Ordering::AcqRel);
                tracing::warn!(error = %e, "identity journal: an anchor append failed; it is kept in this process only");
            }
            tally.done.fetch_add(1, Ordering::AcqRel);
            tally.notify.notify_waiters();
        });
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
    if let Some((generation, hit)) = ledger.cert_cache.pin().get(&digest)
        && *generation == tls.roots_generation()
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
    cache.insert(digest, (generation, facts.clone()));
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
    Some(Offered { key, cert: KeptCert { der: Arc::from(der), issuer, issued, judged } })
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
    let stale: Vec<([u8; 32], Arc<[u8]>)> = ledger.anchors_seen.pin().get(node).map(|m| {
        m.iter().flat_map(|(k, a)| a.certs.iter().filter(|c| c.judged != g0).map(move |c| (*k, Arc::clone(&c.der)))).collect()
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
    let mut dropped = 0u64;
    let mut capped = 0u64;
    ledger.anchors_seen.pin().compute(node.clone(), |existing| {
        fresh.clear();
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
                if c.judged == g0 {
                    return true;
                }
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
        let held = |der: &Arc<[u8]>| existing.is_some_and(|(_, old)| old.values().any(|a| a.certs.iter().any(|c| &c.der == der)));
        fresh = m.values().flat_map(|a| a.certs.iter()).filter(|c| !held(&c.der)).map(|c| Arc::clone(&c.der)).collect();
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
    let _ = refresh_now(ctx);
}

/// [`refresh`], saying whether **this call** ran a fold to completion (`true`) or found one running and asked it to run
/// again (`false`). A caller that must see the fold's result before it goes on — `GossipAgent::change_roots`, after the
/// roots changed — calls this until it is `true` (identity-and-lifecycle I2): a fold that ran under the old roots and
/// re-runs on its own would otherwise finish after the caller had returned.
pub(crate) fn refresh_now(ctx: &TaskCtx) -> bool {
    let ledger = &ctx.identity_ledger;
    let mut ran = false;
    loop {
        if ledger.running.swap(true, Ordering::AcqRel) {
            ledger.again.store(true, Ordering::Release);
            return ran;
        }
        loop {
            ledger.again.store(false, Ordering::Release);
            refresh_once(ctx);
            if !ledger.again.load(Ordering::Acquire) {
                break;
            }
        }
        ran = true;
        ledger.running.store(false, Ordering::Release);
        if !ledger.again.swap(false, Ordering::AcqRel) {
            return ran;
        }
    }
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

fn refresh_once(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let ledger = &ctx.identity_ledger;
    // I2, R4 (#768's M3): the whole fold is judged under one roots generation. If the roots change while it runs, the
    // node being folded merges nothing, no standing judged under the old roots is published, and the fold runs again.
    let g0 = tls.roots_generation();
    for node in nodes_with_records(ctx) {
        let Some(folded) = fold_node(ctx, tls, &node, g0) else {
            ledger.again.store(true, Ordering::Release);
            return;
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
            return;
        }
        if ctx.key_standing.get(&node).as_deref() != Some(&standing) {
            ctx.key_standing.set(&node, standing);
        }
    }
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
