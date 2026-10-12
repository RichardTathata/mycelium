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
//! is not anchored, or S's certificate was issued strictly after P's. Authority flows only to the
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
//! trusts only its newest anchored key — failing closed, never open. Anchored keys this process has
//! judged are kept for its life, so hiding a certificate record does not demote a node in-process.
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
}

/// What this node keeps between folds (post-360 plan row Q). Lock-free (papaya and atomics), so no
/// lock-order row; the fold itself is single-flight (`running` / `again`).
#[derive(Default)]
pub(crate) struct IdentityLedger {
    /// Every anchored key this process has judged, per node, with its certificate's issuance — kept
    /// for the process's life, so hiding a certificate record does not demote a node in-process.
    anchors_seen: papaya::HashMap<NodeId, HashMap<[u8; 32], u64>>,
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
        let already = self.journaled.pin().get(node).is_some_and(|v| v.iter().any(|(p, b)| p == path && b == bytes));
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

/// Judge one certificate for `node`: issued by the fleet CA and valid now, naming `node` by URI,
/// for `expect` when a path names the key, issued no later than the wall clock plus the skew bound.
/// `Some((key, issued_ms))` for a certificate that anchors.
fn judge_cert(ctx: &CoreCtx, ledger: &IdentityLedger, node: &NodeId, der: &[u8], expect: Option<[u8; 32]>) -> Option<([u8; 32], u64)> {
    let tls = ctx.tls.get()?;
    let id = mycelium_core::tls::cert_identity(der)?;
    if expect.is_some_and(|k| k != id.key) || !id.node_uris.contains(&mycelium_core::tls::node_uri(node)) {
        return None;
    }
    let issued = id.issued_ms.unwrap_or(0);
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        ledger.cert_future.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%node, issued_ms = issued, "certificate record issued in the future beyond the skew bound: ignored (identity_cert_future)");
        return None;
    }
    if !tls.verify_member_cert(der) {
        return None;
    }
    Some((id.key, issued))
}

/// A revocation record found for a node: where, its bytes, and what it says.
struct Candidate {
    path: String,
    bytes: Vec<u8>,
    parsed: ParsedRevocation,
    /// The revoker the path names (v2), if any.
    named: Option<[u8; 32]>,
}

/// Parse a revocation record found at `path` for `node`, refusing it — before any signature check —
/// unless its path agrees with its body.
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

/// One node's fold: its anchored keys (with issuance) and the revocations that count.
struct Folded {
    anchors: HashMap<[u8; 32], u64>,
    counted: Vec<Candidate>,
}

fn fold_node(ctx: &TaskCtx, node: &NodeId) -> Folded {
    let ledger = &ctx.identity_ledger;
    // Revocation records: the store, then the journal.
    let mut found: Vec<Candidate> = Vec::new();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{REVOCATION_PREFIX}{node}/")) {
        if let Some(c) = candidate(node, &key, &bytes) {
            found.push(c);
        }
    }
    if let Some(j) = ledger.journaled.pin().get(node) {
        for (path, bytes) in j {
            if !found.iter().any(|c| &c.path == path && &c.bytes == bytes)
                && let Some(c) = candidate(node, path, bytes)
            {
                found.push(c);
            }
        }
    }
    // Stratum 1 — anchored keys: certificate records, and the certificates revocations carry.
    let mut anchors: HashMap<[u8; 32], u64> = ledger.anchors_seen.pin().get(node).cloned().unwrap_or_default();
    let mut offer = |key: [u8; 32], issued: u64| {
        let e = anchors.entry(key).or_insert(issued);
        *e = (*e).max(issued);
    };
    for (path, der) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{CERT_PREFIX}{node}/")) {
        let expect = path.rsplit('/').next().and_then(parse_hex32);
        if expect.is_none() { continue; }
        if let Some((k, at)) = judge_cert(ctx, ledger, node, &der, expect) {
            offer(k, at);
        }
    }
    for c in &found {
        for der in c.parsed.trailer.iter().flat_map(|t| t.certs.iter()) {
            if let Some((k, at)) = judge_cert(ctx, ledger, node, der, None) {
                offer(k, at);
            }
        }
    }
    // The cap, by issuance (newest first) — never by key bytes an attacker chooses.
    if anchors.len() > MAX_CERTS_PER_NODE {
        let mut v: Vec<([u8; 32], u64)> = anchors.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        ledger.over_cap.fetch_add((v.len() - MAX_CERTS_PER_NODE) as u64, Ordering::Relaxed);
        v.truncate(MAX_CERTS_PER_NODE);
        anchors = v.into_iter().collect();
    }
    if !anchors.is_empty() {
        ledger.anchors_seen.pin().insert(node.clone(), anchors.clone());
    }
    // Stratum 2 — revocations, judged against the anchors. A legacy node (none) has none that count.
    let mut counted = Vec::new();
    if !anchors.is_empty() {
        let mut by_signers: Vec<[u8; 32]> = anchors.keys().copied().collect();
        by_signers.sort_by(|a, b| anchors[b].cmp(&anchors[a]).then(a.cmp(b)));
        // Per revoked key, at most MAX_REVOCATIONS_PER_KEY records, those naming the newest-issued
        // anchored revoker first; v1 records (no named revoker) after them.
        let mut per_key: HashMap<[u8; 32], Vec<Candidate>> = HashMap::new();
        for c in found {
            per_key.entry(c.parsed.signed.event.revoked_key).or_default().push(c);
        }
        for (p, mut cs) in per_key {
            cs.sort_by_key(|c| std::cmp::Reverse(c.named.and_then(|s| anchors.get(&s).copied()).unwrap_or(0)));
            if cs.len() > MAX_REVOCATIONS_PER_KEY {
                ledger.over_cap.fetch_add((cs.len() - MAX_REVOCATIONS_PER_KEY) as u64, Ordering::Relaxed);
                cs.truncate(MAX_REVOCATIONS_PER_KEY);
            }
            for c in cs {
                let signers: Vec<[u8; 32]> = match c.named {
                    Some(s) => vec![s],
                    None => by_signers.clone(),
                };
                let counts = signers.iter().any(|s| {
                    let Some(&s_at) = anchors.get(s) else { return false };
                    let outranks = anchors.get(&p).is_none_or(|&p_at| s_at > p_at);
                    outranks && c.parsed.signed_by(s)
                });
                if counts {
                    counted.push(c);
                }
            }
        }
    }
    Folded { anchors, counted }
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
/// while one runs makes that one run again. Run by the identity watcher on any `sys/identity-cert/`
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
            refresh_once(ctx);
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

fn refresh_once(ctx: &TaskCtx) {
    let ledger = &ctx.identity_ledger;
    for node in nodes_with_records(ctx) {
        let folded = fold_node(ctx, &node);
        let mut revoked = HashSet::new();
        for c in &folded.counted {
            revoked.insert(c.parsed.signed.event.revoked_key);
            if ledger.record_counted(&node, &c.path, &c.bytes) {
                tracing::warn!(%node, key = %hex32(&c.parsed.signed.event.revoked_key), "identity key revoked: it no longer verifies for this node");
            }
        }
        let anchored = if folded.anchors.is_empty() {
            None
        } else if ledger.saturated.pin().contains(&node) {
            // Fail closed: the newest anchored key alone.
            folded.anchors.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(k, _)| [*k].into_iter().collect())
        } else {
            Some(folded.anchors.keys().copied().collect::<HashSet<_>>())
        };
        let standing = NodeStanding { anchored, revoked };
        if ctx.key_standing.get(&node).as_deref() != Some(&standing) {
            ctx.key_standing.set(&node, standing);
        }
    }
}

/// The revocations of `node`'s keys that count at this node (the transparency log's leaves).
#[cfg(feature = "compliance")]
pub(crate) fn validated_signed_revocations(ctx: &TaskCtx, node: &NodeId) -> Vec<SignedRevocation> {
    let mut out: Vec<SignedRevocation> = Vec::new();
    for c in fold_node(ctx, node).counted {
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

/// The keys this node knows it has held: its identity history in the local store, its retained set,
/// and its anchored keys. Used only to decide which of its own records it may republish or write.
fn own_history(ctx: &TaskCtx) -> Vec<[u8; 32]> {
    let mut keys: Vec<[u8; 32]> = mycelium_core::ops::kv_get(ctx, &format!("sys/identity/{}", ctx.node_id))
        .map(|b| super::helpers::parse_identity_keys(&b))
        .unwrap_or_default();
    for k in ctx.peer_keys.pin().get(&ctx.node_id).cloned().unwrap_or_default() {
        if !keys.contains(&k) { keys.push(k); }
    }
    for k in ctx.identity_ledger.anchors_seen.pin().get(&ctx.node_id).map(|m| m.keys().copied().collect::<Vec<_>>()).unwrap_or_default() {
        if !keys.contains(&k) { keys.push(k); }
    }
    keys
}

/// Publish this node's certificate record for its current key, and keep it for republishing.
pub(crate) fn publish_own_certificate(ctx: &CoreCtx, ledger: &IdentityLedger, tls: &mycelium_core::tls::NodeTls) {
    let der = tls.node_cert_der();
    let path = cert_record_key(&ctx.node_id, &tls.verifying_key_bytes());
    if mycelium_core::tls::cert_identity(&der).is_none_or(|id| !id.node_uris.contains(&mycelium_core::tls::node_uri(&ctx.node_id))) {
        tracing::warn!(
            node = %ctx.node_id,
            "this node's certificate does not name it by URI (mycelium://<node>): it has no standing, so no revocation of its keys counts at upgraded peers — re-issue it (docs/guide/deprecations.md §39)"
        );
    }
    let _ = mycelium_core::ops::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(der.clone()));
    ledger.own(path, der);
}

/// At start: take up this node's own revocations — its v2 records, and each v1 record (at the old
/// path) it can show it wrote, re-issued as v2 under its current, anchored key — so they count at
/// upgraded peers. Never one revoking the key it signs with.
pub(crate) fn adopt_own_revocations(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let current = tls.verifying_key_bytes();
    let history = own_history(ctx);
    let own_cert = tls.node_cert_der();
    for (path, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &format!("{REVOCATION_PREFIX}{}/", ctx.node_id)) {
        let Some(c) = candidate(&ctx.node_id, &path, &bytes) else { continue };
        let p = c.parsed.signed.event.revoked_key;
        if p == current || !history.contains(&p) {
            continue;
        }
        match c.named {
            Some(s) if history.contains(&s) && c.parsed.signed_by(&s) => ctx.identity_ledger.own(path.to_string(), bytes.to_vec()),
            Some(_) => {}
            None => {
                if !history.iter().any(|k| c.parsed.signed.verify(k)) {
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
/// this node signs with (rotate first) and a key that is not this node's. It counts at a peer only
/// if S is anchored there and was issued after P (or P has no standing) — `rotate_identity` mints the
/// new key's certificate, so `rotate_identity_on_compromise` satisfies it.
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
    let mut certs = vec![tls.node_cert_der()];
    if let Some(p_cert) = mycelium_core::ops::kv_get(ctx, &cert_record_key(&ctx.node_id, &revoked_key)) {
        certs.push(p_cert.to_vec());
    }
    let event = RevocationEvent { node_id: ctx.node_id.clone(), hlc: ctx.hlc.tick(), revoked_key, reason };
    let bytes = encode_v2(event, &tls.signing_key(), certs);
    let path = revocation_key_v2(&ctx.node_id, &revoked_key, &current);
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
