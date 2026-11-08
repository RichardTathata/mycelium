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
//! (`identity/client-certs.journal`), one record per supersession per principal — the revocation
//! ledger's pattern, with its own line in the budget: [`JOURNAL_PER_PRINCIPAL`] per principal and
//! [`JOURNAL_TOTAL`] in all. A principal past its budget is **saturated**: its mark still rises in this
//! process, but after a restart it trusts only the newest record it can see (stated, as the ledger's
//! saturated node trusts only its newest anchored key). Without `[persistence]` the mark lives only in
//! this process, so after a restart a hidden newer record lets an older key back until the newer one is
//! read again — the exit is stated *under `[persistence]`*.
//!
//! Lock-free (papaya and atomics): no lock-order row.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::TaskCtx;

/// KV namespace of client certificate records: `sys/client-cert/{principal-hex}/{key-hex}`.
pub const CLIENT_CERT_PREFIX: &str = "sys/client-cert/";
/// Supersessions journaled per principal before the principal is saturated.
pub(crate) const JOURNAL_PER_PRINCIPAL: usize = 64;
/// Supersessions journaled in all before every further principal is saturated.
pub(crate) const JOURNAL_TOTAL: usize = 65_536;
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

/// One journal record: a principal's mark rose to this, or the principal was saturated.
#[derive(Serialize, Deserialize)]
enum JournalEntry {
    Raised { principal: String, mark: Mark },
    Saturated { principal: String },
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
    marks: papaya::HashMap<String, Mark>,
    journal: arc_swap::ArcSwapOption<super::journal::Journal>,
    journaled: papaya::HashMap<String, usize>,
    journaled_total: AtomicUsize,
    saturated: papaya::HashSet<String>,
    /// Records ignored because their issuance was past the wall clock plus the skew bound.
    pub(crate) future: AtomicU64,
    /// Supersessions the journal could not take (full, or this node without `[persistence]` never counts).
    pub(crate) journal_refused: AtomicU64,
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
                    self.journaled.pin().update_or_insert(principal, |n| n + 1, 1);
                    self.journaled_total.fetch_add(1, Ordering::Relaxed);
                }
                Ok(JournalEntry::Saturated { principal }) => {
                    self.saturated.pin().insert(principal);
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

    /// Raise `principal`'s mark to `to`, journaling the rise when there is room (and a journal).
    fn raise(&self, principal: &str, to: Mark) {
        if !self.raise_in_memory(principal, to) {
            return;
        }
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
        self.journaled.pin().update_or_insert(principal.to_string(), |n| n + 1, 1);
        self.journaled_total.fetch_add(1, Ordering::Relaxed);
        self.append(JournalEntry::Raised { principal: principal.to_string(), mark: to });
    }

    /// `principal`'s current mark, if any record of it has been read.
    pub(crate) fn mark(&self, principal: &str) -> Option<Mark> {
        self.marks.pin().get(principal).copied()
    }
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
    let id = mycelium_core::tls::client_identity(der)?;
    if id.principal() != principal || expect.is_some_and(|k| k != id.key) {
        return None;
    }
    // The chain first, so only a certificate the fleet CA issued is counted as issued in the future.
    if !tls.verify_member_cert(der) {
        return None;
    }
    let issued = id.issued_ms.unwrap_or(0);
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        ctx.client_certs.future.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(%principal, issued_ms = issued, "client certificate issued in the future beyond the skew bound: ignored");
        return None;
    }
    Some(Mark { issued, key: id.key })
}

/// Fold one principal's records: judge each, raise the mark to the newest that counts, and return the
/// verified issuance of every key seen this fold.
fn fold(ctx: &TaskCtx, tls: &mycelium_core::tls::NodeTls, principal: &str) -> HashMap<[u8; 32], u64> {
    let prefix = format!("{CLIENT_CERT_PREFIX}{}/", hex(principal.as_bytes()));
    let mut seen: HashMap<[u8; 32], u64> = HashMap::new();
    // Every record is judged: a cap here would let a member's junk records, sorted ahead of the newer genuine one,
    // keep it from ever raising the mark. Junk is cheap to refuse — `client_identity` and the path check come before
    // the chain — and a genuine record exists only once per certificate the CA issued, its path fixed by its key.
    for (path, der) in mycelium_core::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
        let Some(expect) = path.rsplit('/').next().and_then(parse_hex32) else { continue };
        if let Some(m) = judge(ctx, tls, principal, &der, Some(expect)) {
            let e = seen.entry(m.key).or_insert(m.issued);
            *e = (*e).max(m.issued);
        }
    }
    if let Some(newest) = seen.iter().map(|(k, at)| Mark { issued: *at, key: *k }).max() {
        ctx.client_certs.raise(principal, newest);
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

/// Re-fold every principal with a record in the store — run when a record under [`CLIENT_CERT_PREFIX`]
/// changes, so a newer certificate raises the mark when it arrives rather than when its key is next
/// asked about (a newer record overwritten before anyone asked would otherwise never raise it).
pub(crate) fn refresh(ctx: &TaskCtx) {
    let Some(tls) = ctx.tls.get() else { return };
    let mut principals: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (path, _) in mycelium_core::store::scan_kv_prefix(&ctx.kv_state, CLIENT_CERT_PREFIX) {
        let Some(rest) = path.strip_prefix(CLIENT_CERT_PREFIX) else { continue };
        let Some((phex, _)) = rest.split_once('/') else { continue };
        let Some(principal) = decode_principal_hex(phex) else { continue };
        principals.insert(principal);
    }
    for p in principals {
        fold(ctx, tls, &p);
    }
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
    let issued = id.issued_ms.unwrap_or(0);
    if issued > mycelium_core::sim_seam::wall_now_ms().saturating_add(skew_ms(ctx)) {
        return Err(PublishRefusal::IssuedInTheFuture);
    }
    if !tls.verify_member_cert(der) {
        return Err(PublishRefusal::NotIssuedByTheFleetCa);
    }
    fold(ctx, tls, &principal);
    if ctx.client_certs.mark(&principal).is_some_and(|m| m > Mark { issued, key: id.key }) {
        return Err(PublishRefusal::Superseded);
    }
    let path = client_cert_key(&principal, &id.key);
    let _ = super::helpers::kv_set(ctx, Arc::from(path.as_str()), bytes::Bytes::copy_from_slice(der));
    ctx.client_certs.raise(&principal, Mark { issued, key: id.key });
    Ok(Published { principal, key: id.key, issued_ms: issued })
}
