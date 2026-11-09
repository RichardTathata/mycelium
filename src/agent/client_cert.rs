//! Client certificates for services (identity-and-lifecycle I1, D1) — the fold that says which of a client
//! principal's keys is **current**.
//!
//! ## What a client certificate is
//!
//! A certificate the fleet CA issues to a service's process (`mycelium tls issue --client <name> --issuer
//! <issuer>`), naming `mycelium-client://{issuer}/{name}` and nothing else, for the gateway principal
//! `token:{issuer}/{name}` (`mycelium_core::tls::client_identity`). Both mesh verifiers refuse it, so a
//! client key never joins the transport; `NodeTls::verify_member_cert`, the chain alone, still accepts it,
//! and this fold uses that to judge its issuance.
//!
//! ## Records
//!
//! `sys/client-cert/{principal-hex}/{key-hex}` = the certificate's DER, published by
//! `POST /gateway/identity/client-cert`. The namespace is not self-owned (absent from
//! `SELF_OWNED_SYS_PREFIXES`, so the `sys/` tripwire never counts a write to it) and any member's own
//! `kv().set` can write it — the raw KV route's `sys/` refusal binds gateway clients, not members. So a
//! record is judged by **the certificate it holds**, never by its KV write time: the fleet CA issued it,
//! it names exactly the principal and the key its path names, and its issuance (the serial, in ms) is no
//! later than the wall clock plus the skew bound. A record that fails any of these is ignored.
//!
//! ## Supersession, not revocation
//!
//! A client has no entry in the revocation ledger (`sys/revocation/{node}/` is node-keyed). A client key
//! is retired by **issuing the principal a newer certificate**. Per principal this fold keeps a
//! **high-water mark** — the newest verified issuance it has read, ties broken on the greater key hex —
//! and only the key at the mark is current. The mark only rises: once a newer record is read, a later
//! overwrite of it (by LWW, by anti-entropy carrying an older genuine copy, or by a member writing
//! garbage) cannot lower it, so a superseded key stays refused.
//!
//! ## Restart and budget
//!
//! The mark is kept in memory and, under `[persistence]`, appended to a node-local journal
//! (`identity/client-certs.journal`), one record per supersession per principal — two at the gateway that
//! published it, which also journals the record it republishes, so about 32 supersessions there — the revocation
//! ledger's pattern, with its own line in the budget: [`JOURNAL_PER_PRINCIPAL`] records per principal and
//! [`JOURNAL_TOTAL`] in all. A principal past its budget is **saturated**: its mark still rises in this
//! process, but after a restart it trusts only the newest record it can see (stated, as the ledger's
//! saturated node trusts only its newest anchored key). Without `[persistence]` the mark lives only in
//! this process, so after a restart a hidden newer record lets an older key back until the newer one is
//! read again — the exit is stated *under `[persistence]`*.
//!
//! ## Republishing
//!
//! The gateway that published a record keeps it (in memory, and journaled under `[persistence]` only when it changes,
//! inside the same budget as the marks) and writes it again when it is missing or overwritten — but not over a valid
//! certificate for the same key issued at or after its own — unless the principal's mark has risen past it — so a node that joins later, or
//! restarts without `[persistence]`, still reads the newer certificate. A record whose publisher is gone for good and
//! which every remaining node has lost is not restored.
//!
//! ## Cost
//!
//! `refresh` scans the prefix once, grouped by principal, and checks each distinct certificate's chain once while it
//! is cached by digest; it is single-flight. A certificate whose serial carries no readable issuance is never counted.
//!
//! Lock-free (papaya and atomics): no lock-order row.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::TaskCtx;

/// KV namespace of client certificate records: `sys/client-cert/{principal-hex}/{key-hex}`.
pub const CLIENT_CERT_PREFIX: &str = "sys/client-cert/";
/// Journal records (`Raised` and `Published`) per principal before the principal is saturated: 64 supersessions at
/// a node that only reads records, about 32 at the gateway that publishes them.
pub(crate) const JOURNAL_PER_PRINCIPAL: usize = 64;
/// Journal records in all before every further principal is saturated.
pub(crate) const JOURNAL_TOTAL: usize = 65_536;
/// Distinct certificates whose parse and chain verdict are cached; past it the cache is cleared, never grown.
const CERT_CACHE_MAX: usize = 4_096;
/// The skew a certificate's issuance may run ahead of this node's wall clock when `max_clock_drift_ms` is 0.
const DEFAULT_SKEW_MS: u64 = 300_000;

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
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

/// The record path of `principal`'s certificate for `key`.
pub fn client_cert_key(principal: &str, key: &[u8; 32]) -> String {
    format!("{CLIENT_CERT_PREFIX}{}/{}", hex(principal.as_bytes()), hex(key))
}

/// A principal's newest verified issuance: the certificate's serial in ms, then its key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Mark {
    pub(crate) issued: u64,
    pub(crate) key: [u8; 32],
}

/// One journal record: a principal's mark rose to this, the principal was saturated, or this gateway published a
/// record it republishes.
#[derive(Serialize, Deserialize)]
enum JournalEntry {
    Raised { principal: String, mark: Mark },
    Saturated { principal: String },
    Published { principal: String, path: String, der: Vec<u8> },
}

/// What one certificate says, once its chain is checked: cached by its digest.
#[derive(Clone)]
struct Facts {
    principal: String,
    key: [u8; 32],
    issued: Option<u64>,
}

/// Where a client key stands for a principal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKeyStanding {
    /// The principal's newest verified certificate is for this key.
    Current,
    /// A verified certificate for this key exists, but a newer one for the principal has been read.
    Superseded,
    /// No verified certificate for this key under this principal (none on record, not the fleet CA's,
    /// another principal's, or issued in the future beyond the skew bound).
    Unknown,
}

/// What this node keeps between client-cert folds. Lock-free (papaya and atomics).
#[derive(Default)]
pub(crate) struct ClientCertLedger {
    pub(super) marks: papaya::HashMap<String, Mark>,
    /// The parse and chain verdict per distinct certificate (by SHA-256), bounded by [`CERT_CACHE_MAX`].
    cert_cache: papaya::HashMap<[u8; 32], Option<Facts>>,
    /// The records this gateway published, newest per principal, `(path, der)`: republished when missing or
    /// overwritten, as `revocation.rs` republishes a node's own records.
    published: papaya::HashMap<String, (String, Vec<u8>)>,
    running: AtomicBool,
    again: AtomicBool,
    journal: arc_swap::ArcSwapOption<super::journal::Journal>,
    journaled: papaya::HashMap<String, usize>,
    journaled_total: AtomicUsize,
    saturated: papaya::HashSet<String>,
    /// Records ignored because their issuance was past the wall clock plus the skew bound.
    pub(crate) future: AtomicU64,
    /// Supersessions the journal could not take (full, or this node without `[persistence]` never counts).
    pub(crate) journal_refused: AtomicU64,
    /// Store scans the folds made (one per `refresh`, one per single-principal fold).
    pub(crate) scans: AtomicU64,
    /// CA chain checks run (once per distinct certificate while it is cached).
    pub(crate) chain_checks: AtomicU64,
    /// Records ignored because their certificate carried no readable issuance.
    pub(crate) no_issuance: AtomicU64,
}

impl std::fmt::Debug for ClientCertLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientCertLedger").field("principals", &self.marks.len()).finish()
    }
}

impl ClientCertLedger {
    /// Load the journal at `path` and open it for appends. Run once at start, before the first fold.
    pub(crate) fn open_journal(&self, path: &std::path::Path) -> std::io::Result<()> {
        let existing = if path.exists() { super::journal::read_journal(path)? } else { Vec::new() };
        for bytes in existing {
            match mycelium_core::serde_fixint::from_slice::<JournalEntry>(&bytes) {
                Ok(JournalEntry::Raised { principal, mark }) => {
                    self.raise_in_memory(&principal, mark);
                    self.count_journaled(&principal);
                }
                Ok(JournalEntry::Saturated { principal }) => {
                    self.saturated.pin().insert(principal);
                }
                // Spawned appends can land out of order, so the newest by (issuance, key) is kept, not the last.
                Ok(JournalEntry::Published { principal, path, der }) => {
                    self.count_journaled(&principal);
                    let Some(m) = published_mark(&der) else { continue };
                    self.published.pin().compute(principal, |existing| match existing {
                        Some((_, (_, held))) if published_mark(held).is_some_and(|h| h >= m) => papaya::Operation::Abort(()),
                        _ => papaya::Operation::Insert((path.clone(), der.clone())),
                    });
                }
                Err(_) => tracing::warn!(journal = %path.display(), "client-cert journal: an entry did not decode; skipped"),
            }
        }
        let j = super::journal::Journal::open(path, "identity/client-certs")?;
        self.journal.store(Some(j));
        Ok(())
    }

    /// Release the journal (shutdown): its ownership lock goes with the last handle.
    pub(crate) fn close_journal(&self) {
        self.journal.store(None);
    }

    /// Raise `principal`'s mark to `to` if `to` is newer, in memory only. Returns whether it rose.
    /// Retry-safe `compute`: two folds never lower each other's mark.
    fn raise_in_memory(&self, principal: &str, to: Mark) -> bool {
        let mut rose = false;
        self.marks.pin().compute(principal.to_string(), |existing| match existing {
            Some((_, m)) if *m >= to => {
                rose = false;
                papaya::Operation::Abort(())
            }
            _ => {
                rose = true;
                papaya::Operation::Insert(to)
            }
        });
        rose
    }

    fn append(&self, entry: JournalEntry) {
        let Some(j) = self.journal.load_full() else { return };
        let Ok(bytes) = mycelium_core::serde_fixint::to_vec(&entry) else { return };
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            if let Err(e) = j.append(bytes).await {
                tracing::warn!(error = %e, "client-cert journal: append failed; the supersession still holds in this process");
            }
        });
    }

    fn count_journaled(&self, principal: &str) {
        self.journaled.pin().update_or_insert(principal.to_string(), |n| n + 1, 1);
        self.journaled_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Journal `entry` for `principal` within the budget — [`JOURNAL_PER_PRINCIPAL`] records per principal,
    /// [`JOURNAL_TOTAL`] in all, `Raised` and `Published` alike. Past it the principal is saturated (recorded once).
    fn journal_within_budget(&self, principal: &str, entry: JournalEntry) {
        if self.journal.load().is_none() {
            return;
        }
        let per = self.journaled.pin().get(principal).copied().unwrap_or(0);
        if per >= JOURNAL_PER_PRINCIPAL || self.journaled_total.load(Ordering::Relaxed) >= JOURNAL_TOTAL {
            self.journal_refused.fetch_add(1, Ordering::Relaxed);
            if self.saturated.pin().insert(principal.to_string()) {
                tracing::warn!(%principal, "client-cert journal full for this principal: after a restart it trusts only its newest visible certificate");
                self.append(JournalEntry::Saturated { principal: principal.to_string() });
            }
            return;
        }
        self.count_journaled(principal);
        self.append(entry);
    }

    /// Raise `principal`'s mark to `to`, journaling the rise when there is room (and a journal).
    fn raise(&self, principal: &str, to: Mark) {
        if self.raise_in_memory(principal, to) {
            self.journal_within_budget(principal, JournalEntry::Raised { principal: principal.to_string(), mark: to });
        }
    }

    /// Hold `(path, der)` as `principal`'s published record and journal it — only when it is newer than the one held
    /// (by issuance, then key), so publishing the same certificate again changes nothing and appends nothing.
    fn hold_published(&self, principal: &str, path: String, der: Vec<u8>) {
        let Some(m) = published_mark(&der) else { return };
        let mut changed = false;
        self.published.pin().compute(principal.to_string(), |existing| match existing {
            Some((_, (_, held))) if published_mark(held).is_some_and(|h| h >= m) => {
                changed = false;
                papaya::Operation::Abort(())
            }
            _ => {
                changed = true;
                papaya::Operation::Insert((path.clone(), der.clone()))
            }
        });
        if changed {
            self.journal_within_budget(principal, JournalEntry::Published { principal: principal.to_string(), path, der });
        }
    }

    /// `principal`'s current mark, if any record of it has been read.
    pub(crate) fn mark(&self, principal: &str) -> Option<Mark> {
        self.marks.pin().get(principal).copied()
    }
}

/// A held record's order: its certificate's issuance and key, read without the chain (the door checked it).
fn published_mark(der: &[u8]) -> Option<Mark> {
    let id = mycelium_core::tls::client_identity(der)?;
    Some(Mark { issued: id.issued_ms?, key: id.key })
}

fn skew_ms(ctx: &TaskCtx) -> u64 {
    match ctx.config.max_clock_drift_ms {
        0 => DEFAULT_SKEW_MS,
        d => d,
    }
}

/// Judge one certificate: issued by the fleet CA (the chain alone), a client certificate for exactly
/// `principal` and `expect` (the key its path names), issued no later than the wall clock plus the skew
/// bound. `Some(mark)` for one that counts.
pub(crate) fn judge(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls, principal: &str, der: &[u8], expect: Option<[u8; 32]>) -> Option<Mark> {
    let f = facts(&ctx.client_certs, tls, der)?;
    if f.principal != principal || expect.is_some_and(|k| k != f.key) {
        return None;
    }
    // No readable issuance (a serial longer than 8 bytes, or zero): it cannot be ordered, so it never counts.
    let Some(issued) = f.issued else {
        ctx.client_certs.no_issuance.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%principal, "client certificate with no readable issuance in its serial: ignored");
        return None;
    };
    // Time-dependent, so checked on every judgment rather than cached.
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        ctx.client_certs.future.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%principal, issued_ms = issued, "client certificate issued in the future beyond the skew bound: ignored");
        return None;
    }
    Some(Mark { issued, key: f.key })
}

/// The parse and the CA chain check of `der`, once per distinct certificate (a member can write one certificate under
/// many paths, or many principals' worth of foreign ones): cached by digest, the cache cleared when full. Only a
/// client certificate the fleet CA issued has facts.
fn facts(ledger: &ClientCertLedger, tls: &mycelium_core::tls::NodeTls, der: &[u8]) -> Option<Facts> {
    let digest: [u8; 32] = Sha256::digest(der).into();
    if let Some(hit) = ledger.cert_cache.pin().get(&digest) {
        return hit.clone();
    }
    let found = mycelium_core::tls::client_identity(der)
        .filter(|_| {
            ledger.chain_checks.fetch_add(1, Ordering::Relaxed);
            tls.verify_member_cert(der)
        })
        .map(|id| Facts { principal: id.principal(), key: id.key, issued: id.issued_ms });
    let cache = ledger.cert_cache.pin();
    if cache.len() >= CERT_CACHE_MAX {
        cache.clear();
    }
    cache.insert(digest, found.clone());
    found
}

/// Fold one principal's records: judge each, raise the mark to the newest that counts, and return the
/// verified issuance of every key seen this fold.
fn fold(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls, principal: &str) -> HashMap<[u8; 32], u64> {
    let prefix = format!("{CLIENT_CERT_PREFIX}{}/", hex(principal.as_bytes()));
    let mut seen: HashMap<[u8; 32], u64> = HashMap::new();
    // Every record is judged: a cap here would let a member's junk records, sorted ahead of the newer genuine one,
    // keep it from ever raising the mark. Each distinct certificate's chain is checked once (`facts`, by digest), so
    // a fold costs one scan of the principal's records and a chain check per certificate not yet seen.
    ctx.client_certs.scans.fetch_add(1, Ordering::Relaxed);
    for (path, der) in mycelium_core::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
        let Some(expect) = path.rsplit('/').next().and_then(parse_hex32) else { continue };
        if let Some(m) = judge(ctx, tls, principal, &der, Some(expect)) {
            let e = seen.entry(m.key).or_insert(m.issued);
            *e = (*e).max(m.issued);
        }
    }
    if let Some(newest) = seen.iter().map(|(k, at)| Mark { issued: *at, key: *k }).max() {
        ctx.client_certs.raise(principal, newest);
        feed_client_keys(ctx);
    }
    seen
}

/// Where `key` stands for `principal`: folds the principal's records first, so a newer record read now
/// supersedes at once. After a restart the mark is the newest the journal held and the store shows; for a
/// saturated principal the journal stopped at its budget, so a newer mark reached later lives in this process only.
pub fn client_key_standing(ctx: &TaskCtx, principal: &str, key: &[u8; 32]) -> ClientKeyStanding {
    let Some(tls) = ctx.tls.get() else { return ClientKeyStanding::Unknown };
    let seen = fold(ctx, tls, principal);
    let Some(mark) = ctx.client_certs.mark(principal) else { return ClientKeyStanding::Unknown };
    if mark.key == *key {
        return ClientKeyStanding::Current;
    }
    // Below the mark: superseded while its verified record is visible, unknown once it is gone — never current.
    if seen.contains_key(key) {
        ClientKeyStanding::Superseded
    } else {
        ClientKeyStanding::Unknown
    }
}

/// Re-fold every principal with a record in the store, then republish this gateway's own records — run when a
/// record under [`CLIENT_CERT_PREFIX`] changes, at start and periodically, so a newer certificate raises the mark
/// when it arrives rather than when its key is next asked about. **One** scan of the prefix, grouped by principal,
/// whatever the number of principals; each distinct certificate's chain checked once while cached. Single-flight: a
/// call while one runs makes that one run again (the revocation fold's `running` / `again`).
pub(crate) fn refresh(ctx: &TaskCtx) {
    let ledger = &ctx.client_certs;
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
    let Some(tls) = ctx.tls.get() else { return };
    ctx.client_certs.scans.fetch_add(1, Ordering::Relaxed);
    let mut newest: std::collections::BTreeMap<String, Mark> = std::collections::BTreeMap::new();
    for (path, der) in mycelium_core::store::scan_kv_prefix(&ctx.kv_state, CLIENT_CERT_PREFIX) {
        let Some(rest) = path.strip_prefix(CLIENT_CERT_PREFIX) else { continue };
        let Some((phex, khex)) = rest.split_once('/') else { continue };
        let (Some(principal), Some(expect)) = (decode_principal_hex(phex), parse_hex32(khex)) else { continue };
        if let Some(m) = judge(ctx, tls, &principal, &der, Some(expect)) {
            let e = newest.entry(principal).or_insert(m);
            *e = (*e).max(m);
        }
    }
    for (principal, mark) in newest {
        ctx.client_certs.raise(&principal, mark);
    }
    republish_own(ctx);
    feed_client_keys(ctx);
}

/// Republish each record this gateway published that is missing from the store or holds other bytes, unless the
/// principal's mark has since risen past it (another gateway published a newer one). Returns how many.
pub(crate) fn republish_own(ctx: &TaskCtx) -> usize {
    let Some(tls) = ctx.tls.get() else { return 0 };
    let mut n = 0;
    for (principal, (path, der)) in ctx.client_certs.published.pin().iter() {
        let key = path.rsplit('/').next().and_then(parse_hex32);
        if ctx.client_certs.mark(principal).is_some_and(|m| Some(m.key) != key) {
            continue;
        }
        let Some(own) = published_mark(der) else { continue };
        let stored = mycelium_core::ops::kv_get(ctx, path);
        if stored.as_deref() == Some(der.as_slice()) {
            continue;
        }
        // Another certificate for the same key that the fold would count, issued at or after ours (the CA certified the
        // key twice), is left alone: comparing bytes would have two gateways rewrite each other forever. The fold's own
        // judgment, skew bound included, so a far-future certificate planted at the path does not stop the restore.
        if let Some(stored) = stored
            && judge(ctx, tls, principal, &stored, key).is_some_and(|m| m.issued >= own.issued)
        {
            continue;
        }
        {
            let _ = super::helpers::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::from(der.clone()));
            n += 1;
        }
    }
    if n > 0 {
        tracing::warn!(records = n, "republished client certificate records this gateway published (missing or overwritten)");
    }
    n
}

fn decode_principal_hex(s: &str) -> Option<String> {
    if s.is_empty() || !s.len().is_multiple_of(2) || s.len() > 2 * 300 || !s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

/// Why a client certificate was not published.
#[derive(Debug, PartialEq, Eq)]
pub enum PublishRefusal {
    /// This node has no `[tls]`, so it holds no CA anchor to judge the chain by.
    NoTls,
    /// Not a client certificate: no `mycelium-client://{issuer}/{name}` name, or a node's name beside it.
    NotAClientCertificate,
    /// The fleet CA did not issue it, or it is not valid now.
    NotIssuedByTheFleetCa,
    /// Its issuance is past the wall clock plus the skew bound.
    IssuedInTheFuture,
    /// Its serial carries no issuance this tree can read (longer than 8 bytes, or zero), as a certificate from
    /// general PKI tooling has: it could not be ordered against the principal's other certificates.
    NoReadableIssuance,
    /// A newer certificate for the principal has already been read: publishing this one would change nothing.
    Superseded,
}

impl std::fmt::Display for PublishRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoTls => "no_tls",
            Self::NotAClientCertificate => "not_a_client_certificate",
            Self::NotIssuedByTheFleetCa => "not_issued_by_the_fleet_ca",
            Self::IssuedInTheFuture => "issued_in_the_future",
            Self::NoReadableIssuance => "no_readable_issuance",
            Self::Superseded => "superseded",
        })
    }
}

/// What a published client certificate says.
#[derive(Debug)]
pub struct Published {
    pub principal: String,
    pub key: [u8; 32],
    pub issued_ms: u64,
}

/// Judge `der` and publish it as its principal's record: the door `POST /gateway/identity/client-cert` calls
/// this. The record is the certificate itself, so any node judges it the same way this one did.
pub fn publish(ctx: &Arc<TaskCtx>, der: &[u8]) -> Result<Published, PublishRefusal> {
    let Some(tls) = ctx.tls.get() else { return Err(PublishRefusal::NoTls) };
    let id = mycelium_core::tls::client_identity(der).ok_or(PublishRefusal::NotAClientCertificate)?;
    let principal = id.principal();
    if !tls.verify_member_cert(der) {
        return Err(PublishRefusal::NotIssuedByTheFleetCa);
    }
    let issued = id.issued_ms.ok_or(PublishRefusal::NoReadableIssuance)?;
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        return Err(PublishRefusal::IssuedInTheFuture);
    }
    fold(ctx, tls, &principal);
    if ctx.client_certs.mark(&principal).is_some_and(|m| m > Mark { issued, key: id.key }) {
        return Err(PublishRefusal::Superseded);
    }
    let path = client_cert_key(&principal, &id.key);
    let _ = super::helpers::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::copy_from_slice(der));
    ctx.client_certs.raise(&principal, Mark { issued, key: id.key });
    feed_client_keys(ctx);
    ctx.client_certs.hold_published(&principal, path, der.to_vec());
    Ok(Published { principal, key: id.key, issued_ms: issued })
}

/// A `Published` journal entry, encoded as the ledger writes it (tests build a journal by hand).
#[cfg(test)]
pub(crate) fn published_entry_for_test(principal: &str, path: &str, der: &[u8]) -> Vec<u8> {
    mycelium_core::serde_fixint::to_vec(&JournalEntry::Published { principal: principal.into(), path: path.into(), der: der.to_vec() }).unwrap()
}

#[cfg(test)]
impl ClientCertLedger {
    /// The record path this gateway holds as `principal`'s published record.
    pub(crate) fn published_path_for_test(&self, principal: &str) -> Option<String> {
        self.published.pin().get(principal).map(|(p, _)| p.clone())
    }
}

/// Feed every client principal's current key — the fold's marks — to each registered external-issuer set
/// (identity-and-lifecycle I1): an execution authority then verifies a client holder's possession proof by it. A
/// superseded key is replaced, never kept beside its successor. Run after each fold and when an authority attaches.
pub(crate) fn feed_client_keys(ctx: &TaskCtx) {
    let sinks: Vec<crate::knowledge::issuer::SharedExternalIssuers> = ctx.client_key_sinks.pin().values().cloned().collect();
    if sinks.is_empty() {
        return;
    }
    let marks: Vec<(String, Mark)> = ctx.client_certs.marks.pin().iter().map(|(p, m)| (p.clone(), *m)).collect();
    for (principal, mark) in marks {
        let Some(issuer) = crate::knowledge::IssuerId::new(&principal) else { continue };
        for sink in &sinks {
            let _ = sink.set_client_key(issuer.clone(), mark.key);
        }
    }
}
