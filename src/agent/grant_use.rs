//! A door's own record of the grants it admitted — the use marker a member cannot re-arm (critique plan row CR2,
//! first PR; the owner-signed rule, `docs/wiki/dev/owner-signed-records.md`).
//!
//! A grant admits one call at each enforcement point. The use is kept in three places, read in this order:
//!
//! 1. **This node's journal** ([`GrantUseLedger`], stream `grant/uses`, never gossiped). A use is appended and
//!    fsynced before the call is admitted; a restart folds it back; a member's KV write cannot touch it. A use the
//!    journal holds and does not show released reads **used**, whatever the KV says.
//! 2. **The marker key** `grant/{holder}/{term}/used/{point}` in the gossip KV — one key for every door of a class
//!    (`{point}` is `gateway` or `provider`, not a node), so it is how a door learns of a use at *another* door.
//!    Any member can write or tombstone it, so **anything other than *never written* reads used**: a verified
//!    marker, a value that does not verify, a tombstone, a release made elsewhere. Each odd reading is counted.
//! 3. The execution authority's in-memory claim (`claim_grant_use`), where two concurrent presentations at this
//!    node meet.
//!
//! **A release is honoured only by the door that made it** (decided 2026-11-13). A door that claimed a use and then
//! knows nothing ran appends the release to its journal and writes a signed release over its marker. At that door
//! the key then reads unused when it holds exactly that release — this node's, signed, naming the use the journal
//! shows released — or was never written. At every other door a release is a value like any other and reads used:
//! a reader that did not record the use cannot tell a genuine release of the newest use from a replayed older one,
//! from a member's own marker-and-release pair written over another door's use, or from a release that overlapped
//! another door's admission. The signed release in the KV is therefore **never the reason** a grant reads unused —
//! the journal is; the KV copy says to the other doors and to an auditor who released what, and is what a collector
//! reads.
//!
//! Nothing in Layer I learns of `grant/`: the store applies a forged marker, a garbage value and a tombstone as it
//! applies anything. The reading is made here, at the door.
//!
//! **Not built, stated** (`docs/features/grants.md`): two doors' concurrent admission of one grant; a door that
//! never recorded the use reads *never written* once a planted tombstone is collected; a member can spend a grant
//! by writing the key first; the journal is not compacted.

use super::journal::Journal;
use super::TaskCtx;
use crate::node_id::NodeId;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Domain separator of a use marker's and a release's signature.
const USE_RECORD_TAG: &[u8] = b"mycelium.grant/use/1\n";
/// The journal's stream identity (one per destination: a replay of another journal cannot answer for this one).
const STREAM: &str = "grant/uses";
/// How long this door remembers a use: a grant's window is at most `GRANT_TTL_CAP_SECS`, so a grant that was
/// current when it was used has expired once this has passed.
const RETENTION_MS: u64 = crate::mandate::grant::GRANT_TTL_CAP_SECS * 1_000 + 120_000;

/// The stable name of the refusal a door answers when its own marker write lost to a newer value at the key.
pub(crate) const GRANT_USE_SUPERSEDED: &str = "grant_use_superseded";
/// The stable name of the refusal a door answers when its store applied nothing of its marker write (the live-entry
/// cap, `max_store_entries`): the key holds no marker here, the call does not run, and the use stands in the journal.
pub(crate) const GRANT_USE_NOT_STORED: &str = "grant_use_not_stored";

/// What a door's marker write established, read from its receipt. Only [`MarkerWrite::Current`] admits the call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MarkerWrite {
    /// The marker is the key's value (`Applied`, or `AlreadyCurrent`).
    Current,
    /// A newer value held the key and LWW kept it (`Superseded`) — or an outcome this code does not know, which is
    /// not *current* either. The use stands in this door's journal: something is at the key that it did not read.
    Superseded,
    /// The store refused the write and applied nothing (`Refused`: the live-entry cap). No marker is at the key
    /// here for a reader to find, so the call does not run. The use stands in the journal, as for `Superseded`: the
    /// write path syncs the record to the WAL and gossips it whatever the local apply did, so the marker may be on
    /// this node's disk (a restart replays it) and at its peers — a give-back here would disagree with both.
    NotStored,
}

/// What a use record says: a use, or the release of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum UseKind {
    Use,
    Release,
}

/// The value at a marker key since CR2: a door's signed statement that it admitted a call under a grant (`use`), or
/// that the use it names ran nothing (`release`). JSON, as the other `grant/` records are. The body names the grant
/// (its holder segment and term — the key's), the door (the key's `{point}`), the door's node and the use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UseRecord {
    /// Format version: 1.
    pub(crate) v: u8,
    pub(crate) kind: UseKind,
    /// `{holder segment}/{term}` — the grant, as the key names it.
    pub(crate) grant: String,
    /// The enforcement point's tag, as the key names it (`gateway`, `provider`).
    pub(crate) door: String,
    /// The node whose door this was.
    pub(crate) node: String,
    /// The use: unique per admission at that node. A release names the use it releases.
    #[serde(rename = "use")]
    pub(crate) use_id: String,
    pub(crate) at_ms: u64,
    /// The signing key (base64) and the Ed25519 signature (base64) over [`UseRecord::message`]. Empty on a node with
    /// no identity, which signs nothing anywhere.
    #[serde(default)]
    pub(crate) signer: String,
    #[serde(default)]
    pub(crate) sig: String,
}

impl UseRecord {
    fn message(&self) -> Vec<u8> {
        let mut m = USE_RECORD_TAG.to_vec();
        m.push(match self.kind {
            UseKind::Use => b'u',
            UseKind::Release => b'r',
        });
        for field in [&self.grant, &self.door, &self.node, &self.use_id] {
            m.extend_from_slice(&(field.len() as u64).to_le_bytes());
            m.extend_from_slice(field.as_bytes());
        }
        m.extend_from_slice(&self.at_ms.to_le_bytes());
        m
    }

    /// A record of `kind` for the use `use_id` at this node's door, signed with the node's key when it has one.
    pub(crate) fn signed(ctx: &TaskCtx, kind: UseKind, marker_key: &str, use_id: &str) -> Option<Self> {
        let (grant, door) = split_marker_key(marker_key)?;
        let mut r = Self {
            v: 1,
            kind,
            grant: grant.to_string(),
            door: door.to_string(),
            node: ctx.node_id.to_string(),
            use_id: use_id.to_string(),
            at_ms: ctx.hlc.decision_now_ms(),
            signer: String::new(),
            sig: String::new(),
        };
        if let Some(tls) = ctx.tls.get() {
            let sk = tls.signing_key();
            r.sign_with(&sk);
        }
        Some(r)
    }

    pub(crate) fn sign_with(&mut self, sk: &ed25519_dalek::SigningKey) {
        let b64 = base64::engine::general_purpose::STANDARD;
        self.signer = b64.encode(sk.verifying_key().to_bytes());
        self.sig = b64.encode(crate::tls::sign_bytes(sk, &self.message()));
    }

    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Does this record name the grant and the door `marker_key` names, under a signature by a key this node trusts
    /// for the node the body names? A node with no identity trusts no signature and signs none: there, its **own**
    /// unsigned record passes, and nothing else does.
    fn verifies_at(&self, ctx: &TaskCtx, marker_key: &str) -> bool {
        if self.v != 1 || split_marker_key(marker_key) != Some((self.grant.as_str(), self.door.as_str())) {
            return false;
        }
        let Ok(node) = self.node.parse::<NodeId>() else { return false };
        if ctx.tls.get().is_none() {
            return node == ctx.node_id && self.signer.is_empty() && self.sig.is_empty();
        }
        let b64 = base64::engine::general_purpose::STANDARD;
        let (Ok(signer), Ok(sig)) = (b64.decode(&self.signer), b64.decode(&self.sig)) else { return false };
        let Ok(signer) = <[u8; 32]>::try_from(signer.as_slice()) else { return false };
        super::gateway_caller::verifying_keys_for(ctx, &node).contains(&signer) && crate::tls::verify_bytes(&signer, &self.message(), &sig)
    }
}

/// `grant/{holder}/{term}/used/{point}` → (`{holder}/{term}`, `{point}`).
fn split_marker_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("grant/")?;
    let (grant, door) = rest.rsplit_once("/used/")?;
    (!grant.is_empty() && !door.is_empty() && !door.contains('/')).then_some((grant, door))
}

/// A marker as every release before CR2 wrote it: the decimal milliseconds of the use.
fn is_legacy_marker(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.len() <= 20 && bytes.iter().all(u8::is_ascii_digit)
}

/// One use this door made of a grant, as its journal holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OwnUse {
    pub(crate) use_id: String,
    pub(crate) released: bool,
    pub(crate) at_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "e", rename_all = "lowercase")]
enum Entry {
    Used { key: String, #[serde(rename = "use")] use_id: String, at_ms: u64 },
    Released { key: String, #[serde(rename = "use")] use_id: String, at_ms: u64 },
}

/// Why a read at a marker key found the grant used, or odd. Each is a counter on `GET /stats` (`grant_use`) and a
/// label of `mycelium_grant_use_reads_total{reading}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Odd {
    /// A tombstone at the key: read as used.
    Tombstone,
    /// A value that is neither form, or a new-form marker that does not verify: read as used.
    Unverifiable,
    /// A marker in the form before CR2 (a timestamp): honoured as used, for one release.
    Legacy,
    /// A verified release made by another node's door: read as used here.
    ReleaseElsewhere,
    /// A release that does not verify — signed by a key the reader does not trust for the node it names, or naming
    /// another grant or door: read as used.
    ReleaseForged,
    /// A genuine release by this node that is not the release of the use its journal shows released — an earlier
    /// release written back over a later use: read as used.
    ReleaseReplayed,
    /// This door's own marker or release write lost to a newer value at the key — or its release was not written
    /// because the key no longer held this door's marker for that use.
    Superseded,
    /// This door's own marker write was refused by the store (the live-entry cap): nothing applied, the call
    /// refused, the use standing at this door.
    NotStored,
}

impl Odd {
    pub(crate) const ALL: [Odd; 8] = [Odd::Tombstone, Odd::Unverifiable, Odd::Legacy, Odd::ReleaseElsewhere, Odd::ReleaseForged, Odd::ReleaseReplayed, Odd::Superseded, Odd::NotStored];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Odd::Tombstone => "tombstone",
            Odd::Unverifiable => "unverifiable",
            Odd::Legacy => "legacy",
            Odd::ReleaseElsewhere => "release_elsewhere",
            Odd::ReleaseForged => "release_forged",
            Odd::ReleaseReplayed => "release_replayed",
            Odd::Superseded => "superseded",
            Odd::NotStored => "not_stored",
        }
    }
}

/// This node's record of the grant uses its doors made: the journal, and its fold in memory.
#[derive(Default)]
pub(crate) struct GrantUseLedger {
    journal: arc_swap::ArcSwapOption<Journal>,
    /// Lock-order row 79: the newest use this node made at each marker key, from the journal. Leaf; never held
    /// across an `await` (the journal append is awaited with it released).
    uses: Mutex<HashMap<String, OwnUse>>,
    /// The map's size after its last prune, doubled: the next size at which it is pruned.
    prune_at: AtomicU64,
    odd: [AtomicU64; 8],
    /// Journal entries that did not decode at open; skipped.
    pub(crate) undecoded: AtomicU64,
    /// Test seam: run once between a use's journal append and its marker write — where another member's write to
    /// the key can land after the door's read.
    #[cfg(test)]
    pub(crate) before_marker_write: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl std::fmt::Debug for GrantUseLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrantUseLedger").finish_non_exhaustive()
    }
}

impl GrantUseLedger {
    /// Fold the journal at `path` and open it for appends. Run once at start, before any door admits a call. Uses
    /// older than the retention are not kept in memory: their grants have expired.
    pub(crate) fn open_journal(&self, path: &std::path::Path, now_ms: u64) -> std::io::Result<()> {
        let existing = if path.exists() { super::journal::read_journal(path)? } else { Vec::new() };
        {
            let mut uses = self.uses.lock().unwrap_or_else(|e| e.into_inner());
            for bytes in existing {
                match serde_json::from_slice::<Entry>(&bytes) {
                    Ok(Entry::Used { key, use_id, at_ms }) => {
                        uses.insert(key, OwnUse { use_id, released: false, at_ms });
                    }
                    Ok(Entry::Released { key, use_id, .. }) => {
                        if let Some(u) = uses.get_mut(&key).filter(|u| u.use_id == use_id) {
                            u.released = true;
                        }
                    }
                    Err(_) => {
                        self.undecoded.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(journal = %path.display(), "grant-use journal: an entry did not decode; skipped");
                    }
                }
            }
            uses.retain(|_, u| u.at_ms.saturating_add(RETENTION_MS) > now_ms);
        }
        self.journal.store(Some(Journal::open(path, STREAM)?));
        Ok(())
    }

    /// Release the journal (shutdown): its ownership lock goes with the last handle.
    pub(crate) fn close_journal(&self) {
        self.journal.store(None);
    }

    /// The newest use this node made at `key`, if it still remembers one.
    pub(crate) fn own(&self, key: &str) -> Option<OwnUse> {
        self.uses.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
    }

    async fn append(&self, entry: &Entry) -> Result<(), String> {
        let Some(j) = self.journal.load_full() else { return Ok(()) };
        let bytes = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
        j.append(bytes).await.map(|_| ()).map_err(|e| e.to_string())
    }

    /// Record that this node's door admits a call at `key` as use `use_id`: **on disk, fsynced, before this
    /// returns** on a node that persists, then in memory. `Err` names why it is not on disk; nothing is remembered
    /// then, and the caller refuses the call. A node with no `[persistence]` has no journal: the use is in memory,
    /// as everything there is.
    pub(crate) async fn record_use(&self, key: &str, use_id: &str, at_ms: u64) -> Result<(), String> {
        self.append(&Entry::Used { key: key.to_string(), use_id: use_id.to_string(), at_ms }).await?;
        let mut uses = self.uses.lock().unwrap_or_else(|e| e.into_inner());
        if uses.len() as u64 >= self.prune_at.load(Ordering::Relaxed) {
            uses.retain(|_, u| u.at_ms.saturating_add(RETENTION_MS) > at_ms);
            self.prune_at.store((uses.len() as u64).saturating_mul(2).max(1_024), Ordering::Relaxed);
        }
        uses.insert(key.to_string(), OwnUse { use_id: use_id.to_string(), released: false, at_ms });
        Ok(())
    }

    /// Record that the use `use_id` at `key` ran nothing: on disk first, then in memory. `Err` (the journal did not
    /// take it) leaves the use standing — the safe side.
    pub(crate) async fn record_release(&self, key: &str, use_id: &str, at_ms: u64) -> Result<(), String> {
        self.append(&Entry::Released { key: key.to_string(), use_id: use_id.to_string(), at_ms }).await?;
        if let Some(u) = self.uses.lock().unwrap_or_else(|e| e.into_inner()).get_mut(key).filter(|u| u.use_id == use_id) {
            u.released = true;
        }
        Ok(())
    }

    pub(crate) fn count(&self, odd: Odd) {
        self.odd[Odd::ALL.iter().position(|o| *o == odd).unwrap_or(0)].fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_grant_use_reads_total", "reading" => odd.label()).increment(1);
    }

    /// How many reads or writes met `odd` since this process started.
    pub(crate) fn counted(&self, odd: Odd) -> u64 {
        self.odd[Odd::ALL.iter().position(|o| *o == odd).unwrap_or(0)].load(Ordering::Relaxed)
    }
}

/// **Is the grant at `marker_key` used, as this node's door reads it?** The journal first; then the key, where
/// anything other than *never written* reads used — except this node's own signed release of the use its journal
/// shows released. Odd readings are counted on `ctx.grant_uses`.
pub(crate) fn reads_used(ctx: &TaskCtx, marker_key: &str) -> bool {
    let ledger = &ctx.grant_uses;
    let own = ledger.own(marker_key);
    if own.as_ref().is_some_and(|u| !u.released) {
        return true;
    }
    // The store's entry, not `kv_get`: a tombstone is an entry with no data, and it is not *never written*.
    let held: Option<Option<bytes::Bytes>> = ctx.kv_state.store.pin().get(marker_key).map(|e| e.data.clone());
    let odd = match held {
        None => return false,
        Some(None) => Odd::Tombstone,
        Some(Some(bytes)) if is_legacy_marker(&bytes) => Odd::Legacy,
        Some(Some(bytes)) => match serde_json::from_slice::<UseRecord>(&bytes) {
            Err(_) => Odd::Unverifiable,
            Ok(rec) => {
                let verified = rec.verifies_at(ctx, marker_key);
                match rec.kind {
                    // A verified marker is the ordinary reading of a grant used at another door, or at this one
                    // before its journal's retention: used, and nothing to count.
                    UseKind::Use if verified => return true,
                    UseKind::Use => Odd::Unverifiable,
                    UseKind::Release if !verified => Odd::ReleaseForged,
                    UseKind::Release if rec.node != ctx.node_id.to_string() => Odd::ReleaseElsewhere,
                    UseKind::Release => {
                        if own.is_some_and(|u| u.released && u.use_id == rec.use_id) {
                            return false;
                        }
                        Odd::ReleaseReplayed
                    }
                }
            }
        },
    };
    ledger.count(odd);
    true
}

/// A fresh use id for an admission at this node: unique per call (the HLC ticks once for it).
pub(crate) fn new_use_id(ctx: &TaskCtx, marker_key: &str, attempt_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(ctx.node_id.to_string().as_bytes());
    h.update([0]);
    h.update(marker_key.as_bytes());
    h.update([0]);
    h.update(attempt_id.as_bytes());
    h.update(ctx.hlc.tick().to_le_bytes());
    h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// **Record a use this door is about to admit**: the journal (fsynced), then the signed marker in the KV with its
/// receipt read. Every outcome of the write is named, and only one admits:
///
/// - `Ok(Current)` — `Applied` or `AlreadyCurrent`: the marker is the key's value. The call may run.
/// - `Ok(Superseded)` — `Superseded` (or an outcome this code does not know): a newer value holds the key, written
///   after this door's read. The call is refused [`GRANT_USE_SUPERSEDED`] and **the use stands** in the journal: the
///   grant reads used at this door.
/// - `Ok(NotStored)` — `Refused`: the store's live-entry cap applied nothing, so this node's key is as it was read,
///   *never written*, and a call admitted now would leave no marker here. The call is refused
///   [`GRANT_USE_NOT_STORED`] and **the use stands** in the journal — spent at this door, though nothing ran: the
///   record was synced to the WAL and gossiped before the apply was refused, so a restart or a peer may hold it.
/// - `Err(reason)` — the use or the marker is not on disk. Nothing is remembered, the caller gives its in-memory
///   claim back, and nothing is spent.
pub(crate) async fn record_use(ctx: &Arc<TaskCtx>, marker_key: &str, use_id: &str) -> Result<MarkerWrite, String> {
    use mycelium_core::receipt::LocalApplication;
    let now_ms = ctx.hlc.decision_now_ms();
    ctx.grant_uses.record_use(marker_key, use_id, now_ms).await?;
    let Some(rec) = UseRecord::signed(ctx, UseKind::Use, marker_key, use_id) else { return Err(format!("{marker_key} is not a marker key")) };
    #[cfg(test)]
    {
        let hook = ctx.grant_uses.before_marker_write.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(hook) = hook {
            hook();
        }
    }
    match super::state_machine::write_owned_kv_reporting(ctx, marker_key, rec.to_bytes()).await {
        Ok(LocalApplication::Applied | LocalApplication::AlreadyCurrent) => Ok(MarkerWrite::Current),
        Ok(LocalApplication::Refused) => {
            ctx.grant_uses.count(Odd::NotStored);
            Ok(MarkerWrite::NotStored)
        }
        // `Superseded`, and — the enum is `#[non_exhaustive]` — anything added later: not *current*, so not admitted.
        Ok(_) => {
            ctx.grant_uses.count(Odd::Superseded);
            Ok(MarkerWrite::Superseded)
        }
        Err(reason) => {
            // The marker is not on disk, so the call does not run and the use is given back — in the journal too,
            // or a restart would read a use that admitted nothing.
            if let Err(e) = ctx.grant_uses.record_release(marker_key, use_id, now_ms).await {
                tracing::warn!(key = marker_key, error = %e, "grant use: the marker was not written and the journal did not take the release; the grant stays spent at this door");
            }
            Err(reason)
        }
    }
}

/// Does `marker_key` hold **this node's own marker for `use_id`** — the record this door wrote when it admitted the
/// call, still the key's value?
fn holds_own_marker(ctx: &TaskCtx, marker_key: &str, use_id: &str) -> bool {
    let held = ctx.kv_state.store.pin().get(marker_key).and_then(|e| e.data.clone());
    held.and_then(|bytes| serde_json::from_slice::<UseRecord>(&bytes).ok())
        .is_some_and(|rec| rec.kind == UseKind::Use && rec.use_id == use_id && rec.node == ctx.node_id.to_string() && rec.verifies_at(ctx, marker_key))
}

/// **Release the use `use_id` this door made at `marker_key`**, for a call this node knows did not run: the journal
/// first, then the signed release — written **only over this door's own marker for that use**. `true` when the
/// release is the key's value, so the grant reads unused at this door. `false` leaves it spent:
///
/// - the journal did not take the release;
/// - the key no longer holds this door's marker for the use — another door's marker (two doors admitted at once),
///   a tombstone, any other value. Nothing is written over it, it reads used, and it is counted (`superseded`). A
///   value that lands between this check and the write still loses to the release under LWW: stated, D12's window;
/// - the release's own write was not *current*: `Superseded`, or `Refused` (which cannot happen over a live marker —
///   an overwrite of a live key is outside the cap — and is read the same way if it does).
pub(crate) async fn release_use(ctx: &Arc<TaskCtx>, marker_key: &str, use_id: &str) -> bool {
    let now_ms = ctx.hlc.decision_now_ms();
    if let Err(e) = ctx.grant_uses.record_release(marker_key, use_id, now_ms).await {
        tracing::warn!(key = marker_key, error = %e, "grant use: the release was not journaled; the grant stays spent at this door");
        return false;
    }
    if !holds_own_marker(ctx, marker_key, use_id) {
        ctx.grant_uses.count(Odd::Superseded);
        return false;
    }
    let Some(rec) = UseRecord::signed(ctx, UseKind::Release, marker_key, use_id) else { return false };
    match super::state_machine::write_owned_kv_reporting(ctx, marker_key, rec.to_bytes()).await {
        Ok(application) if application.is_current() => true,
        Ok(_) => {
            ctx.grant_uses.count(Odd::Superseded);
            false
        }
        Err(e) => {
            tracing::warn!(key = marker_key, error = %e, "grant use: the signed release was not written; the marker stands and the grant stays spent at this door");
            false
        }
    }
}
