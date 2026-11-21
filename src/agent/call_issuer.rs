//! **The gateway's half of a `call:` mandate** (identity-and-lifecycle I4, PR 2): the mint, the records it writes,
//! the revoke, the re-assertion of a checkpoint another member wrote over, and the collector of the `call/` and
//! `grant/` key families.
//!
//! # The mint
//!
//! With `gateway_mint_call_mandates` on, every protected call the gateway dispatches (`gateway_rpc_call_with_mandate`,
//! the one dispatch path of every gateway door) carries, in its envelope's second mandate slot (`cm`), a mandate the
//! gateway issues **to itself** for that one call (D-6): scope `call:c{n}` where `n` is the envelope's id, issuer and
//! holder `node:{self}`, the one operation of the call, a window of `call_mandate_ttl_secs`. Beside it go its
//! possession proof (over the call's operation, resource and RFC 8785 arguments digest — derived by the provider's
//! own [`derive_call`](super::provider_enforcement::derive_call), so the two cannot disagree) and its first
//! revocation checkpoint (sequence 1, nothing revoked). The provider establishes it from what the envelope carries,
//! with no round trip.
//!
//! The order is the plan's: the gateway's preflight has decided (a refused call never reaches the dispatch); the mint
//! is drawn inside framing, once the envelope's id is known; then **the first checkpoint and the `window` record are
//! written durably** (`call/{gateway}/c{n}/checkpoint` and `…/window`); only then is the envelope sent. A write the
//! disk will not take refuses the dispatch by name (`call_mandate_not_durable`): nothing is sent.
//!
//! A gateway mints for every protected kind, whatever its own preflight said about `cancellable` (the provider's rule
//! decides, Q5). It mints nothing it cannot: without a TLS identity, for a call whose operation cannot be derived
//! (the provider will refuse it the same way), or with [`MAX_ISSUED_CALLS`] held — each counted.
//!
//! # The records (owner-signed, verified at read)
//!
//! Any admitted member can write any key, so neither record is trusted for where it sits:
//! - `checkpoint` is a `SignedRevocationCheckpoint`, used only when its authority and scope are its key's and it is
//!   signed by that gateway's node key (`take_kv_copy` at a provider; [`verified_checkpoint`] here);
//! - `window` is a [`CallWindow`] signed by the issuer's node key under the domain tag `mycelium:call-window:v1\n`
//!   ([`window_message`], pinned in a golden), used only when its `scope` and `term` are its key's and the signature
//!   is under the node key of the gateway its key's segment names ([`verified_window`]).
//!
//! A record that fails reads as **absent**. The only reader of `window` is the collector, and its absence moves the
//! floor later, never earlier: a forged record can only delay collection, never hasten it.
//!
//! # The revoke and the re-assertion
//!
//! [`revoke_call`] writes the call's next checkpoint (sequence 2, its term revoked) durably and offers it to this node's
//! own authority. A provider learns it from the KV (its feeder reads the key only for a window it holds) and its sweep
//! stops the work — bound 1. The cancel doors (A2A `tasks/cancel`, MCP `notifications/cancelled`,
//! `GossipAgent::cancel_call`) take the revoke in its two steps: [`begin_revoke`] signs the revoking checkpoint and
//! applies it to this node's own authority, the door **forwards it to the provider at once** (`call_cancel`, the row's
//! PR 3 — the primary path; the KV is the fallback), and [`finish_revoke`] writes it durably, the caller's
//! acknowledgement waiting for that alone (§9 O10, the maintainer's decision of 2026-11-20). The revocation is the
//! gateway's own signed act, true when it is signed; only the gateway's promise that it is *on disk* waits for the
//! disk.
//!
//! **The gateway re-asserts its checkpoint** (#775's review, H1): it watches its own `call/{self}/` keys, and when one
//! holds anything but what it wrote — another member's value, a tombstone, its own first checkpoint written back over
//! the revoking one — it writes its newest again, at a later stamp. What it holds is in memory ([`Issued`], at most
//! [`MAX_ISSUED_CALLS`]); after a restart it holds nothing and re-asserts nothing (D-4: the KV is the record), which is
//! stated, not closed.
//!
//! # The collector (D-5), owner-only
//!
//! Off unless `collect_expired_records` is on. **Each node deletes only records its own agent wrote** (the
//! maintainer's decision of 2026-11-15): a gateway its own `call/` pairs and its own grant issue records (`issued`,
//! `checkpoint`), a door the use markers and releases it signed — each checked against this node's key before it is
//! tombstoned, never earlier than D-5's floor. Another agent's record, and any record that does not verify, is left
//! alone; one at a path naming this node is counted. See [`collect`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::gateway_authority::{arguments_digest, call_checkpoint_key, grant_holder_from_segment, grant_holder_segment, mandate_operation, possession_request, PresentedCallMandate, CALL_KEY_PREFIX};
use super::gateway_caller::node_principal;
use super::TaskCtx;
use crate::knowledge::issuer::{verify_signed_by, Authenticity, MemberKeySource, MemberKeys, TrustedExternalIssuers};
use crate::knowledge::IssuerId;
use crate::mandate::authority::{RevocationCheckpoint, SignedRevocationCheckpoint};
use crate::mandate::grant::{possession_message, SignedMandateGrant, CALL_SCOPE_PREFIX, GRANT_TTL_CAP_SECS};
use crate::mandate::{Mandate, PrincipalId, TermId};
use crate::node_id::NodeId;

/// The domain tag a `window` record's signature is made under.
pub(crate) const CALL_WINDOW_DOMAIN: &[u8] = b"mycelium:call-window:v1\n";

/// The most calls a gateway holds in memory as issued — for the re-assertion and the revoke — at once: the
/// provider's own cap on the windows it holds ([`MAX_CALL_WINDOWS`](super::gateway_authority::MAX_CALL_WINDOWS)).
/// At it, the gateway mints nothing (counted, `call_mints_refused`) until windows end.
pub const MAX_ISSUED_CALLS: usize = super::gateway_authority::MAX_CALL_WINDOWS;

/// How often the collector runs.
const COLLECT_INTERVAL_MS: u64 = 60_000;

/// How many written keys the re-assertion queues before it falls back to re-reading every call it holds.
const REASSERT_QUEUE: usize = 4_096;

/// **A `call:` mandate's window, as its issuer signs it** (`call/{gateway}/{term}/window`): what any node reads D-5's
/// floor from, since a checkpoint carries no window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CallWindow {
    /// `1`.
    pub(crate) v: u8,
    /// `call:{term}`.
    pub(crate) scope: String,
    pub(crate) term: String,
    pub(crate) valid_until_ms: u64,
}

/// A [`CallWindow`] and the issuer's signature over [`window_message`], base64.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SignedCallWindow {
    pub(crate) window: CallWindow,
    pub(crate) signature: String,
}

/// The bytes a `window` record's signature covers: the domain tag, then `v`, `scope`, `term` and `valid_until_ms`,
/// each a little-endian `u32` length and its bytes (`valid_until_ms` as eight little-endian bytes). Pinned in
/// `the_window_records_signed_message_is_pinned`.
pub(crate) fn window_message(w: &CallWindow) -> Vec<u8> {
    fn lp(out: &mut Vec<u8>, b: &[u8]) {
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(b);
    }
    let mut out = CALL_WINDOW_DOMAIN.to_vec();
    out.push(w.v);
    lp(&mut out, w.scope.as_bytes());
    lp(&mut out, w.term.as_bytes());
    out.extend_from_slice(&w.valid_until_ms.to_le_bytes());
    out
}

/// The key of a `call:` mandate's window record: `call/{gateway segment}/{term}/window`.
pub(crate) fn call_window_key(gateway: &str, term: &str) -> String {
    format!("{CALL_KEY_PREFIX}{}/{term}/window", grant_holder_segment(gateway))
}

/// What a `call/{segment}/{term}/{leaf}` key names: `(gateway principal, term)`. `None` for any other shape, or a
/// segment that names no node.
pub(crate) fn call_key_names(key: &str) -> Option<(String, String)> {
    let mut parts = key.strip_prefix(CALL_KEY_PREFIX)?.split('/');
    let (segment, term, _leaf) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || term.is_empty() {
        return None;
    }
    let gateway = grant_holder_from_segment(segment)?;
    gateway.starts_with("node:").then(|| (gateway, term.to_string()))
}

/// The member keys a record is verified against: this node's view of every member's keys, and its own current key
/// for itself — so a build without `compliance`, whose view is empty, still reads back what it wrote.
pub(crate) fn record_keys(ctx: &TaskCtx) -> std::collections::HashMap<NodeId, MemberKeys> {
    let mut members = super::gateway_member_keys(ctx);
    if let Some(tls) = ctx.tls.get() {
        let own = members.entry(ctx.node_id.clone()).or_default();
        let key = tls.verifying_key_bytes();
        if !own.retained.contains(&key) {
            own.retained.push(key);
        }
    }
    members
}

/// Is `signature` over `message` the current key of the node `gateway` names?
fn signed_by(gateway: &str, message: &[u8], signature: &[u8], members: &impl MemberKeySource) -> bool {
    IssuerId::new(gateway).is_some_and(|issuer| {
        matches!(verify_signed_by(&issuer, message, signature, members, &TrustedExternalIssuers::new()), Authenticity::Current { .. })
    })
}

/// **A `window` record, verified at read**: its body's `scope` and `term` are the key's and it is signed by the node
/// key of the gateway its key names. `None` for anything else — absent.
pub(crate) fn verified_window(key: &str, bytes: &[u8], members: &impl MemberKeySource) -> Option<CallWindow> {
    let (gateway, term) = call_key_names(key)?;
    let signed: SignedCallWindow = serde_json::from_slice(bytes).ok()?;
    let w = signed.window;
    if w.v != 1 || w.term != term || w.scope != format!("{CALL_SCOPE_PREFIX}{term}") {
        return None;
    }
    let signature = base64::engine::general_purpose::STANDARD.decode(signed.signature.as_bytes()).ok()?;
    signed_by(&gateway, &window_message(&w), &signature, members).then_some(w)
}

/// **A `checkpoint` record, verified at read**: its authority and scope are the key's and it is signed by that
/// gateway's node key. `None` for anything else.
pub(crate) fn verified_checkpoint(key: &str, bytes: &[u8], members: &impl MemberKeySource) -> Option<RevocationCheckpoint> {
    let (gateway, term) = call_key_names(key)?;
    let signed: SignedRevocationCheckpoint = serde_json::from_slice(bytes).ok()?;
    let c = &signed.checkpoint;
    if c.authority.as_str() != gateway || c.scope != format!("{CALL_SCOPE_PREFIX}{term}") {
        return None;
    }
    let verified = signed_by(&gateway, &c.canonical_bytes(), &signed.signature, members);
    verified.then_some(signed.checkpoint)
}

/// One call this gateway issued, as it holds it: what its newest checkpoint is, so it can be signed again — the
/// signature is deterministic, so the same bytes — and the end of its window.
#[derive(Clone, Debug)]
pub(crate) struct Issued {
    seq: u64,
    at_ms: u64,
    revoked: bool,
    until_ms: u64,
    /// The highest sequence **this process** wrote durably (#794's review, K1): the re-assertion writes without an
    /// fsync, so a checkpoint in the KV is not, by itself, a checkpoint on disk.
    durable_seq: u64,
    /// The provider the call was sent to — where the cancel-forward goes (I4, PR 3). `None` for a term this process
    /// learned from the KV after a restart (D-4: nothing is kept), so its cancel reaches the provider only by the KV.
    target: Option<NodeId>,
}

/// The gateway's state for the calls it mints, on `TaskCtx`. A papaya map, so it adds no lock.
#[derive(Default)]
pub(crate) struct CallIssuer {
    /// Each term this gateway minted and holds, until its window's end + 2·skew.
    issued: papaya::HashMap<String, Issued>,
    /// The collector's and the re-assertion's tasks are started once.
    collector: AtomicBool,
    watcher: AtomicBool,
    pub(crate) mints: AtomicU64,
    pub(crate) mints_refused: AtomicU64,
    pub(crate) reasserted: AtomicU64,
    pub(crate) call_records_collected: AtomicU64,
    pub(crate) call_records_unverifiable: AtomicU64,
    pub(crate) grant_records_collected: AtomicU64,
    /// This door's own markers with no grant window in their signed body — from a build before the field, never
    /// released — kept, not collected (#794's round-3 review, D3).
    pub(crate) markers_without_window: AtomicU64,
    /// I4, PR 3: this gateway's cancel receipts, by class (`CancelClass::label`).
    pub(crate) cancel_receipts: papaya::HashMap<&'static str, u64>,
    /// I4, PR 3: cancel-forwards this node refused as a provider, by reason (`cancel_not_from_issuer`, …).
    pub(crate) cancel_forwards_refused: papaya::HashMap<&'static str, u64>,
    /// I4, PR 3: the `/mcp` door's id → term table for `notifications/cancelled`, and its counters.
    pub(crate) mcp_cancels: super::call_cancel::McpCancels,
}

impl CallIssuer {
    /// Is this node's collector running?
    #[cfg_attr(not(all(test, feature = "compliance")), allow(dead_code))]
    pub(crate) fn collecting(&self) -> bool {
        self.collector.load(Ordering::Acquire)
    }

    /// The terms this gateway holds as issued (the re-assertion's fallback walk; tests).
    pub(crate) fn issued_terms(&self) -> Vec<String> {
        self.issued.pin().keys().cloned().collect()
    }
}

/// Does this gateway mint for a `kind` call? Only with `gateway_mint_call_mandates`, a TLS identity, and for protected
/// work (the provider's own predicate, so a hosted capability's kind is minted for too).
pub(crate) fn mints_for(ctx: &TaskCtx, kind: &str) -> bool {
    ctx.config.gateway_mint_call_mandates && ctx.tls.get().is_some() && super::provider_enforcement::protected_here(&ctx.config, kind)
}

/// What one mint produced: `cm` for the envelope, and the two records to write before it is sent.
pub(crate) struct Minted {
    pub(crate) cm: serde_json::Value,
    pub(crate) term: String,
    target: NodeId,
    checkpoint: Vec<u8>,
    window: Vec<u8>,
    at_ms: u64,
    until_ms: u64,
}

/// The signed checkpoint `seq` of `term`'s scope, as this gateway issues it.
fn sign_checkpoint(ctx: &TaskCtx, term: &str, seq: u64, at_ms: u64, revoked: bool) -> Option<SignedRevocationCheckpoint> {
    let tls = ctx.tls.get()?;
    let c = RevocationCheckpoint {
        authority: PrincipalId::new(node_principal(&ctx.node_id))?,
        scope: format!("{CALL_SCOPE_PREFIX}{term}"),
        seq,
        issued_at_ms: at_ms,
        revoked: if revoked { std::iter::once(TermId::new(term)?).collect() } else { Default::default() },
    };
    Some(SignedRevocationCheckpoint { signature: mycelium_core::tls::sign_bytes(&tls.signing_key(), &c.canonical_bytes()).to_vec(), checkpoint: c })
}

/// Test hook: hold `term` as issued until `until_ms`, as a mint does, with no record written.
#[cfg(all(test, feature = "compliance"))]
pub(crate) fn test_hold_issued(ctx: &TaskCtx, term: &str, until_ms: u64) {
    ctx.call_issuer.issued.pin().insert(term.to_string(), Issued { seq: 1, at_ms: 0, revoked: false, until_ms, durable_seq: 1, target: None });
}

/// Test hook: the checkpoint `seq` of `term` this gateway would sign, written nowhere.
#[cfg(all(test, feature = "compliance"))]
pub(crate) fn test_sign_checkpoint(ctx: &TaskCtx, term: &str, seq: u64, at_ms: u64, revoked: bool) -> SignedRevocationCheckpoint {
    sign_checkpoint(ctx, term, seq, at_ms, revoked).expect("an identity")
}

/// **Mint the `call:` mandate for envelope `n`** of a `kind` call to `target` — `None`, minting nothing, where it
/// cannot (see the module doc).
#[allow(clippy::too_many_arguments)] // one call's facts
pub(crate) fn mint(ctx: &TaskCtx, kind: &str, payload: &[u8], claim: Option<&str>, target: &NodeId, n: u64, now_ms: u64) -> Option<Minted> {
    let refused = || {
        ctx.call_issuer.mints_refused.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("mycelium_gateway_call_mints_refused_total").increment(1);
        None
    };
    let tls = ctx.tls.get()?;
    if ctx.call_issuer.issued.len() >= MAX_ISSUED_CALLS {
        return refused();
    }
    let Ok((operation, resource, arguments)) = super::provider_enforcement::derive_call(kind, payload, claim, &target.to_string()) else {
        return refused();
    };
    let gateway = PrincipalId::new(node_principal(&ctx.node_id))?;
    let term = format!("c{n}");
    let until_ms = now_ms.saturating_add(ctx.config.call_mandate_ttl_secs.min(GRANT_TTL_CAP_SECS) * 1_000);
    let m = Mandate {
        holder: gateway.clone(),
        established_by: gateway.clone(),
        purpose: format!("call {term} dispatched by {}", gateway.as_str()),
        scope: format!("{CALL_SCOPE_PREFIX}{term}"),
        operations: vec![mandate_operation(&operation, &resource)],
        epoch: 1,
        term: TermId::new(&term)?,
        valid_from_ms: now_ms,
        valid_until_ms: until_ms,
    };
    let sk = tls.signing_key();
    let first = sign_checkpoint(ctx, &term, 1, now_ms, false)?;
    let proof = mycelium_core::tls::sign_bytes(&sk, &possession_message(&m, &possession_request(&operation, &resource, &arguments_digest(&arguments))));
    let grant = SignedMandateGrant { signature: mycelium_core::tls::sign_bytes(&sk, &m.canonical_bytes()).to_vec(), mandate: m };
    let checkpoint = serde_json::to_vec(&first).ok()?;
    let window = CallWindow { v: 1, scope: format!("{CALL_SCOPE_PREFIX}{term}"), term: term.clone(), valid_until_ms: until_ms };
    let window = serde_json::to_vec(&SignedCallWindow {
        signature: base64::engine::general_purpose::STANDARD.encode(mycelium_core::tls::sign_bytes(&sk, &window_message(&window))),
        window,
    })
    .ok()?;
    let cm = serde_json::to_value(PresentedCallMandate::new(grant, &proof, first)).ok()?;
    Some(Minted { cm, term, target: target.clone(), checkpoint, window, at_ms: now_ms, until_ms })
}

/// A record on disk — the WAL's required-sync write, fsynced before it is applied or gossiped — or why not. A node
/// with no `[persistence]` writes in memory, as every owned record there is written.
async fn write_durable(ctx: &TaskCtx, key: String, value: Vec<u8>) -> Result<(), String> {
    let op = mycelium_core::receipt::OperationId::generate(&ctx.node_id);
    let attempt = mycelium_core::receipt::AttemptId::fresh(&op);
    let key: Arc<str> = Arc::from(key.as_str());
    let written = if ctx.wal.get().is_some() {
        mycelium_core::ops::kv_set_requiring_sync(ctx, op, attempt, key, Bytes::from(value), None).await
    } else {
        mycelium_core::ops::kv_set_with_receipt(ctx, op, attempt, key, Bytes::from(value), None).await
    };
    written.map(|_| ()).map_err(|e| e.to_string())
}

/// **Write a mint's two records durably and hold the call as issued** — before its envelope is sent. `Err`: a record
/// is not on disk, and the call must not be sent.
pub(crate) async fn record(ctx: &TaskCtx, minted: Minted) -> Result<(), String> {
    let gateway = node_principal(&ctx.node_id);
    #[cfg(test)]
    let started = mycelium_core::sim_seam::mono_instant();
    write_durable(ctx, call_checkpoint_key(&gateway, &minted.term), minted.checkpoint).await?;
    write_durable(ctx, call_window_key(&gateway, &minted.term), minted.window).await?;
    #[cfg(test)]
    test_hooks::note_record_latency(&ctx.node_id, started.elapsed());
    ctx.call_issuer.issued.pin().insert(
        minted.term,
        Issued { seq: 1, at_ms: minted.at_ms, revoked: false, until_ms: minted.until_ms, durable_seq: 1, target: Some(minted.target) },
    );
    ctx.call_issuer.mints.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_call_mints_total").increment(1);
    Ok(())
}

/// Why a revoke did not stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RevokeRefusal {
    /// This node has no TLS identity to sign with.
    NoIdentity,
    /// This gateway holds no call of that term, in memory or as a verified window record of its own.
    UnknownTerm,
    /// The revoking checkpoint is not on disk. It is signed and stands in this process — offered to its own
    /// authority, and forwarded to the provider where a cancel door sent it (§9 O10) — but no restart keeps it.
    NotDurable(String),
}

/// **Cancel the call of `term`** (I4): its next checkpoint, its term revoked, offered to this node's own authority
/// (and its sweep run, so work admitted here stops now) and written durably where a provider reads it. `Ok(true)`:
/// this call wrote the revoking checkpoint to disk; `Ok(false)`: this process had already written it durably. A revoke
/// whose write the disk refused is answered `NotDurable` and stands in this process only; called again it writes the
/// same checkpoint (its signature is deterministic) and answers `Ok(true)` once the disk takes it — whatever the KV
/// holds meanwhile, since the re-assertion writes without an fsync (#794's review, K1). After a restart the term is
/// written again for the same reason.
///
/// The term is known from memory, or — after a restart, which keeps nothing in memory (D-4) — from this gateway's
/// own verified `window` record and checkpoint in the KV. Two revokes of one term at once make one checkpoint: the
/// transition to revoked is one papaya `compute` (retry-safe: it reads only its input), and the loser writes the
/// winner's.
#[cfg_attr(not(all(test, feature = "compliance")), allow(dead_code))] // the tests' entry; the doors go through `begin_revoke`
pub(crate) async fn revoke_call(ctx: &TaskCtx, term: &str) -> Result<bool, RevokeRefusal> {
    revoke(ctx, term).await
}

/// [`revoke_call`]: both steps, with nothing between them — for a call that was never sent, which has nothing to
/// forward (`call_cancel::cancelled_before_sent`).
pub(crate) async fn revoke(ctx: &TaskCtx, term: &str) -> Result<bool, RevokeRefusal> {
    let pending = begin_revoke(ctx, term)?;
    finish_revoke(ctx, &pending).await
}

/// **A revocation signed and applied here, not yet on disk** (§9 O10): what [`begin_revoke`] did, what a cancel door
/// forwards, and what [`finish_revoke`] writes.
pub(crate) struct PendingRevoke {
    term: String,
    /// The signed revoking checkpoint — what the cancel-forward carries.
    pub(crate) checkpoint: SignedRevocationCheckpoint,
    /// The provider the call was sent to, if this process knows it.
    pub(crate) target: Option<NodeId>,
    /// Its sequence, and whether this process has written it durably already (then [`finish_revoke`] writes nothing).
    seq: u64,
    on_disk: bool,
}

/// **The revoke's first step** — no await in it: the call's next checkpoint, its term revoked, held as issued, signed,
/// and offered to this node's own authority (its sweep run, so work admitted here stops now). Nothing is written to the
/// KV or the disk: that is [`finish_revoke`]. A door may forward [`PendingRevoke::checkpoint`] in between — the
/// gateway's revocation is its own signed act, true once signed (the maintainer's decision of 2026-11-20, §9 O10).
pub(crate) fn begin_revoke(ctx: &TaskCtx, term: &str) -> Result<PendingRevoke, RevokeRefusal> {
    if ctx.tls.get().is_none() {
        return Err(RevokeRefusal::NoIdentity);
    }
    let gateway = node_principal(&ctx.node_id);
    let cp_key = call_checkpoint_key(&gateway, term);
    let members = record_keys(ctx);
    let held = ctx.call_issuer.issued.pin().get(term).cloned();
    let issued = match held {
        Some(i) => i,
        None => {
            let window_key = call_window_key(&gateway, term);
            let Some(w) = mycelium_core::ops::kv_get(ctx, &window_key).and_then(|b| verified_window(&window_key, &b, &members)) else {
                return Err(RevokeRefusal::UnknownTerm);
            };
            let held = mycelium_core::ops::kv_get(ctx, &cp_key).and_then(|b| verified_checkpoint(&cp_key, &b, &members));
            let issued = Issued {
                seq: held.as_ref().map_or(1, |c| c.seq),
                at_ms: held.as_ref().map_or(0, |c| c.issued_at_ms),
                revoked: held.as_ref().is_some_and(|c| c.revoked.iter().any(|t| t.as_str() == term)),
                until_ms: w.valid_until_ms,
                // What the KV holds after a restart may have come back by gossip from a copy this gateway never
                // fsynced: nothing is taken as on disk until this process writes it.
                durable_seq: 0,
                // Not kept across a restart (D-4): this cancel reaches the provider by the KV only.
                target: None,
            };
            // Held from here, so what this process writes durably is remembered (and re-asserted).
            let _ = ctx.call_issuer.issued.pin().try_insert(term.to_string(), issued.clone());
            issued
        }
    };
    let now = ctx.hlc.decision_now_ms();
    let next = Issued { seq: issued.seq + 1, at_ms: now, revoked: true, until_ms: issued.until_ms, durable_seq: issued.durable_seq, target: issued.target.clone() };
    // Held as issued **before** the write: the re-assertion compares the KV with what it holds, so it must never
    // hold the first checkpoint while the revoking one is in the KV — it would write the first back over it.
    let won = !issued.revoked
        && matches!(
            ctx.call_issuer.issued.pin().compute(term.to_string(), |entry| match entry {
                Some((_, cur)) if cur.revoked => papaya::Operation::Abort(()),
                _ => papaya::Operation::Insert(next.clone()),
            }),
            papaya::Compute::Inserted(..) | papaya::Compute::Updated { .. }
        );
    let revoking = if won { next } else { ctx.call_issuer.issued.pin().get(term).cloned().filter(|i| i.revoked).unwrap_or(issued) };
    let Some(signed) = sign_checkpoint(ctx, term, revoking.seq, revoking.at_ms, true) else { return Err(RevokeRefusal::NoIdentity) };
    if won && let Some(authority) = ctx.execution_authority.get() {
        let _ = authority.offer_checkpoint(&signed, now, &super::gateway_member_keys(ctx));
        authority.sweep(now);
    }
    // Already on disk — written durably by this process at or past this sequence: nothing to write. Only this
    // process's own record of its durable writes decides it (#794's review, K1): the KV may hold the revoking
    // checkpoint because the re-assertion wrote it without an fsync.
    let on_disk = revoking.durable_seq >= revoking.seq;
    Ok(PendingRevoke { term: term.to_string(), checkpoint: signed, target: revoking.target, seq: revoking.seq, on_disk })
}

/// **The revoke's second step: the revoking checkpoint on disk** — the WAL's required-sync write, which applies it to
/// the KV and gossips it only once its sync returns. `Ok(true)`: written now; `Ok(false)`: this process had written it
/// durably already. `Err(NotDurable)`: not on disk — it stands in memory, here and wherever it was forwarded.
pub(crate) async fn finish_revoke(ctx: &TaskCtx, pending: &PendingRevoke) -> Result<bool, RevokeRefusal> {
    if pending.on_disk {
        return Ok(false);
    }
    #[cfg(test)]
    test_hooks::before_revoke_write(&ctx.node_id).await;
    let bytes = serde_json::to_vec(&pending.checkpoint).map_err(|e| RevokeRefusal::NotDurable(e.to_string()))?;
    let cp_key = call_checkpoint_key(&node_principal(&ctx.node_id), &pending.term);
    write_durable(ctx, cp_key, bytes).await.map_err(RevokeRefusal::NotDurable)?;
    let written = pending.seq;
    ctx.call_issuer.issued.pin().update(pending.term.clone(), move |i| Issued { durable_seq: i.durable_seq.max(written), ..i.clone() });
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_calls_cancelled_total").increment(1);
    Ok(true)
}

/// **Write this gateway's newest checkpoint for `term` again if the KV holds anything else** (#775's H1): another
/// member's value, a tombstone, or an older checkpoint of this gateway's written back over a newer one. Nothing for a
/// term it does not hold, or one past its floor (the collector's). `true`: re-asserted.
pub(crate) fn reassert(ctx: &Arc<TaskCtx>, term: &str) -> bool {
    let Some(issued) = ctx.call_issuer.issued.pin().get(term).cloned() else { return false };
    if ctx.hlc.decision_now_ms() > issued.until_ms.saturating_add(2 * skew_ms(ctx)) {
        return false;
    }
    let Some(ours) = sign_checkpoint(ctx, term, issued.seq, issued.at_ms, issued.revoked).and_then(|s| serde_json::to_vec(&s).ok()) else { return false };
    let key = call_checkpoint_key(&node_principal(&ctx.node_id), term);
    let held = mycelium_core::ops::kv_get(ctx, &key);
    if held.as_deref() == Some(ours.as_slice()) {
        return false;
    }
    // A checkpoint of this gateway's own at or past the sequence it holds is never written over.
    if let Some(c) = held.as_deref().and_then(|b| verified_checkpoint(&key, b, &record_keys(ctx)))
        && c.seq >= issued.seq
    {
        return false;
    }
    tracing::warn!(key = %key, "a call: checkpoint this gateway issued was written over by another value; re-asserting it");
    // Written at a stamp past the value it replaces, whatever clock wrote that one (within the HLC's drift bound):
    // last-writer-wins must keep the re-assertion, here and wherever it gossips.
    if let Some(stamp) = ctx.kv_state.store.pin().get(key.as_str()).map(|e| e.timestamp) {
        ctx.hlc.observe(stamp);
    }
    super::state_machine::write_owned_kv(ctx, &key, ours);
    ctx.call_issuer.reasserted.fetch_add(1, Ordering::Relaxed);
    #[cfg(feature = "metrics")]
    metrics::counter!("mycelium_gateway_call_checkpoints_reasserted_total").increment(1);
    true
}

/// The skew D-5's floor counts twice: this node's execution authority's, else `[authority]`'s, else its default.
fn skew_ms(ctx: &TaskCtx) -> u64 {
    if let Some(a) = ctx.execution_authority.get() {
        return a.skew_ms();
    }
    ctx.config.authority.as_ref().map_or(1_000, |a| a.skew_ms)
}

/// The records of a KV key family, grouped: each group's keys and their values.
type Grouped<K> = std::collections::BTreeMap<K, Vec<(Arc<str>, Bytes)>>;

/// What one collection removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Collected {
    pub(crate) call_records: usize,
    pub(crate) grant_records: usize,
}

/// **Collect what D-5 lets go at `now_ms` — only records this node's own agent wrote** (the maintainer's decision of
/// 2026-11-15: each agent's retention is a promise about its own records). This node deletes, with a tombstone:
/// - **as a gateway**, its own `call/{self}/{term}/…` records — the pair once its window record (or, with none, its
///   checkpoint) verifies under this node's key — past the window's end + 2·skew + `max_drain_secs`; and its own grant
///   issue records, `grant/…/issued` and `grant/…/checkpoint` signed under its key, past the later of the grant's
///   window + 2·skew and its issue + `grant_retention_secs`;
/// - **as a door**, the use markers and releases it signed (critique plan CR2), past their `at_ms` +
///   `grant_retention_secs` + 2·skew — never before the grant's window ends, since the retention is at least the
///   longest window.
///
/// **No other record is deleted.** Another agent's record — a member's `call/` pair, another issuer's grant, another
/// door's marker, an agent's request, an unsigned `revoked` or `refused` — is left alone; a record at a path that
/// names this node and does not verify under its key is left alone and counted (`call_records_unverifiable`). A
/// request record is unsigned, so it is never collected: after its grant's records go, the holder's status route
/// still answers it `expired` from it.
pub(crate) fn collect(ctx: &TaskCtx, now_ms: u64) -> Collected {
    let members = record_keys(ctx);
    let me = node_principal(&ctx.node_id);
    let skew2 = 2 * skew_ms(ctx);
    let drain_ms = ctx.config.max_drain_secs * 1_000;
    let retention_ms = ctx.config.grant_retention_secs.max(GRANT_TTL_CAP_SECS) * 1_000;
    let mut out = Collected::default();
    let unverifiable = || {
        ctx.call_issuer.call_records_unverifiable.fetch_add(1, Ordering::Relaxed);
    };
    let delete = |key: &Arc<str>| mycelium_core::ops::kv_delete(ctx, Arc::clone(key));

    // `call/{self}/…`: this gateway's own pairs only.
    let own = format!("{CALL_KEY_PREFIX}{}/", grant_holder_segment(&me));
    let mut calls: Grouped<String> = Default::default();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &own) {
        if let Some((_, term)) = call_key_names(&key) {
            calls.entry(term).or_default().push((key, bytes));
        }
    }
    for (term, records) in calls {
        let leaf = |name: &str| records.iter().find(|(k, _)| k.ends_with(&format!("/{name}")));
        let floor_of = |until_ms: u64| until_ms.saturating_add(skew2).saturating_add(drain_ms);
        let floor = match leaf("window").and_then(|(k, b)| verified_window(k, b, &members)) {
            Some(w) => Some(floor_of(w.valid_until_ms)),
            None => leaf("checkpoint")
                .and_then(|(k, b)| verified_checkpoint(k, b, &members))
                .map(|c| floor_of(c.issued_at_ms.saturating_add(GRANT_TTL_CAP_SECS * 1_000))),
        };
        let Some(floor) = floor else {
            unverifiable();
            continue;
        };
        if now_ms <= floor {
            continue;
        }
        // Forgotten before the delete, so the re-assertion does not write back what is being collected.
        ctx.call_issuer.issued.pin().remove(&term);
        // Only the two records this gateway writes, and each only if it is this gateway's: a value another member
        // wrote at one of the keys is left alone.
        for (key, bytes) in &records {
            let ours = (key.ends_with("/window") && verified_window(key, bytes, &members).is_some())
                || (key.ends_with("/checkpoint") && verified_checkpoint(key, bytes, &members).is_some());
            if ours {
                delete(key);
                out.call_records += 1;
            } else {
                unverifiable();
            }
        }
    }
    // Held until its window could no longer be presented anywhere (its end + 2·skew): past it a provider refuses the
    // mandate out of window whatever its checkpoint says, so there is nothing left to re-assert or revoke from memory.
    // Not `papaya::HashMap::retain`: on papaya 0.1.x it reads each entry through its tagged pointer, so after an
    // incremental resize the closure is handed bytes 4 off the entry (#799's review, H1; fixed upstream in 0.2.x).
    // `iter` reads the masked pointer: collect the dead terms under one guard, remove them one by one.
    let dead: Vec<String> = ctx.call_issuer.issued.pin().iter().filter(|(_, i)| now_ms > i.until_ms.saturating_add(skew2)).map(|(t, _)| t.clone()).collect();
    for term in dead {
        ctx.call_issuer.issued.pin().remove(&term);
    }

    // `grant/`: this node's own issue records and its own door's markers and releases — only with
    // `collect_expired_records` on. The `call/` half above runs on a minting gateway too (#794's round-2 review, C3).
    if !ctx.config.collect_expired_records {
        ctx.call_issuer.call_records_collected.fetch_add(out.call_records as u64, Ordering::Relaxed);
        return out;
    }
    let mut grants: Grouped<String> = Default::default();
    for (key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, "grant/") {
        let mut parts = key.splitn(4, '/');
        let (Some(_), Some(segment), Some(request)) = (parts.next(), parts.next(), parts.next()) else { continue };
        grants.entry(format!("grant/{segment}/{request}")).or_default().push((key, bytes));
    }
    for (base, records) in grants {
        // As the issuer: `issued` and `checkpoint`, each signed under this node's key.
        let issued_key = format!("{base}/issued");
        if let Some((key, bytes)) = records.iter().find(|(k, _)| k.as_ref() == issued_key)
            && let Some((issuer, (from, until))) = verified_grant(&base, bytes, &members)
            && issuer == me
        {
            let floor = until.saturating_add(skew2).max(from.saturating_add(retention_ms));
            if now_ms > floor {
                delete(key);
                out.grant_records += 1;
                let cp_key = format!("{base}/checkpoint");
                if let Some((k, b)) = records.iter().find(|(k, _)| k.as_ref() == cp_key) {
                    if own_grant_checkpoint(&base, b, &me, &members) {
                        delete(k);
                        out.grant_records += 1;
                    } else {
                        unverifiable();
                    }
                }
            }
        }
        // As a door: the use markers and releases this node signed — kept at least until the grant's own window
        // ends, whatever the retention (#794's round-2 review, C2): `GrantVerifier` caps no `grant:` window, so a
        // grant can outlive the retention, and a marker gone inside the window would let the grant be used again.
        for (key, bytes) in records.iter().filter(|(k, _)| k.contains("/used/")) {
            let Some((at_ms, until_ms)) = super::grant_use::own_record_at(ctx, key, bytes) else { continue };
            // The window from the door's own signed body (#794's round-3 review, D3): no ledger or journal needed.
            // A body without it comes only from a build before the field, never released: kept, and counted.
            if until_ms == 0 {
                ctx.call_issuer.markers_without_window.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if now_ms > at_ms.saturating_add(retention_ms).max(until_ms).saturating_add(skew2) {
                delete(key);
                out.grant_records += 1;
            }
        }
    }
    ctx.call_issuer.call_records_collected.fetch_add(out.call_records as u64, Ordering::Relaxed);
    ctx.call_issuer.grant_records_collected.fetch_add(out.grant_records as u64, Ordering::Relaxed);
    out
}

/// A grant's checkpoint at `{base}/checkpoint`, signed by this node (`me`) for the grant's scope.
fn own_grant_checkpoint(base: &str, bytes: &[u8], me: &str, members: &impl MemberKeySource) -> bool {
    let Some(request) = base.rsplit('/').next() else { return false };
    let Ok(signed) = serde_json::from_slice::<SignedRevocationCheckpoint>(bytes) else { return false };
    let c = &signed.checkpoint;
    c.authority.as_str() == me && c.scope == format!("grant:{request}") && signed_by(me, &c.canonical_bytes(), &signed.signature, members)
}

/// A grant's `issued` record at `grant/{segment}/{request}/issued`, verified at read: a grant whose holder's segment
/// and term are its key's, issued by a node, under that node's current key. `(issuer, (valid_from, valid_until))`.
/// A grant's `issued` record at `grant/{segment}/{request}/issued`, verified at read: a grant whose holder's segment
/// and term are its key's, issued by a node, under that node's current key. `(issuer, (valid_from, valid_until))`.
fn verified_grant(base: &str, bytes: &[u8], members: &impl MemberKeySource) -> Option<(String, (u64, u64))> {
    let g: SignedMandateGrant = serde_json::from_slice(bytes).ok()?;
    let m = &g.mandate;
    let (segment, request) = base.strip_prefix("grant/")?.split_once('/')?;
    if !m.is_grant() || m.term.as_str() != request || grant_holder_segment(m.holder.as_str()) != segment {
        return None;
    }
    let issuer = m.established_by.as_str();
    (issuer.starts_with("node:") && signed_by(issuer, &m.canonical_bytes(), &g.signature, members))
        .then(|| (issuer.to_string(), (m.valid_from_ms, m.valid_until_ms)))
}

/// Start the collector, once per node, if `collect_expired_records` or `gateway_mint_call_mandates` is on: each
/// minute, through the timer seam. With only the mint on it collects the gateway's own `call/` records.
/// `false`: not started (off, or already running).
pub(crate) fn start_collector(ctx: &Arc<TaskCtx>) -> bool {
    // The mint implies collecting the gateway's own `call/` records: that is the minting gateway's promise about its
    // own records (#794's round-2 review, C3). Everything else waits for `collect_expired_records`.
    if !ctx.config.collect_expired_records && !ctx.config.gateway_mint_call_mandates {
        return false;
    }
    if ctx.call_issuer.collector.swap(true, Ordering::AcqRel) {
        return false;
    }
    let ctx2 = Arc::clone(ctx);
    let mut shutdown = ctx.shutdown_tx.subscribe();
    ctx.spawn_task(async move {
        loop {
            tokio::select! {
                _ = super::capability_ops::await_shutdown(&mut shutdown) => break,
                _ = mycelium_core::sim_seam::sleep_ms("authority/collect", COLLECT_INTERVAL_MS) => {}
            }
            let _ = collect(&ctx2, ctx2.hlc.decision_now_ms());
        }
    });
    true
}

/// Start the re-assertion, once per minting gateway: a write under its own `call/{self}/…/checkpoint` keys is taken
/// where the KV's prefix watch sees it, in a bounded queue; when the queue overflows, every held call is re-read.
pub(crate) fn start_reassertion(ctx: &Arc<TaskCtx>) {
    if ctx.call_issuer.watcher.swap(true, Ordering::AcqRel) {
        return;
    }
    let (tx, mut keys) = tokio::sync::mpsc::channel::<Arc<str>>(REASSERT_QUEUE);
    let lagged = Arc::new(AtomicBool::new(false));
    let raise = Arc::clone(&lagged);
    let own = format!("{CALL_KEY_PREFIX}{}/", grant_holder_segment(&node_principal(&ctx.node_id)));
    let mut wake = mycelium_core::kv_handle::KvHandle::from_core(Arc::clone(&ctx.core)).subscribe_prefix_with_predicate(CALL_KEY_PREFIX, move |k: &str| {
        if !k.starts_with(own.as_str()) || !k.ends_with("/checkpoint") {
            return false;
        }
        if mycelium_core::sim_seam::chan_try_send("authority/call-reassert", &tx, Arc::from(k)) != mycelium_core::sim_seam::ChanVerdict::Sent {
            raise.store(true, Ordering::Release);
        }
        true
    });
    let ctx2 = Arc::clone(ctx);
    let mut shutdown = ctx.shutdown_tx.subscribe();
    ctx.spawn_task(async move {
        loop {
            tokio::select! {
                _ = super::capability_ops::await_shutdown(&mut shutdown) => break,
                changed = wake.changed() => { if changed.is_err() { break; } }
            }
            if lagged.swap(false, Ordering::AcqRel) {
                while keys.try_recv().is_ok() {}
                for term in ctx2.call_issuer.issued_terms() {
                    reassert(&ctx2, &term);
                }
            }
            while let Ok(key) = keys.try_recv() {
                if let Some((_, term)) = call_key_names(&key) {
                    reassert(&ctx2, &term);
                }
            }
        }
    });
}

/// Test hooks: the mint's durable writes, timed; a revoke's durable write held.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::sync::Mutex;
    /// Per gateway node: a revoke's durable write waits at [`super::finish_revoke`]'s start until its gate lets it
    /// through (§9 O10's tests: the forward goes out while the write is held; a crash between the two).
    pub(crate) static HOLD_REVOKE_WRITE: std::sync::LazyLock<papaya::HashMap<String, std::sync::Arc<RevokeGate>>> = std::sync::LazyLock::new(papaya::HashMap::new);
    /// One node's gate: the writes that reached it, and the permits that let them through.
    pub(crate) struct RevokeGate {
        permits: tokio::sync::Semaphore,
        reached: std::sync::atomic::AtomicUsize,
    }
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))] // read by `call_cancel_tests`
    impl RevokeGate {
        /// How many revoke writes have reached the gate.
        pub(crate) fn reached(&self) -> usize {
            self.reached.load(std::sync::atomic::Ordering::SeqCst)
        }
        /// Let `n` writes through.
        pub(crate) fn release(&self, n: usize) {
            self.permits.add_permits(n);
        }
    }
    /// `node`'s revoke writes held at a gate while this lives; dropped, the gate is removed and lets every write by.
    pub(crate) struct HeldRevokeWrites {
        node: String,
        gate: std::sync::Arc<RevokeGate>,
    }
    impl std::ops::Deref for HeldRevokeWrites {
        type Target = RevokeGate;
        fn deref(&self) -> &RevokeGate {
            &self.gate
        }
    }
    impl Drop for HeldRevokeWrites {
        fn drop(&mut self) {
            HOLD_REVOKE_WRITE.pin().remove(&self.node);
            self.gate.permits.close();
        }
    }
    /// Hold `node`'s revoke writes at a gate until it is released or dropped.
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))] // read by `call_cancel_tests`
    pub(crate) fn hold_revoke_writes(node: &crate::node_id::NodeId) -> HeldRevokeWrites {
        let gate = std::sync::Arc::new(RevokeGate { permits: tokio::sync::Semaphore::new(0), reached: std::sync::atomic::AtomicUsize::new(0) });
        HOLD_REVOKE_WRITE.pin().insert(node.to_string(), std::sync::Arc::clone(&gate));
        HeldRevokeWrites { node: node.to_string(), gate }
    }
    pub(super) async fn before_revoke_write(node: &crate::node_id::NodeId) {
        let gate = HOLD_REVOKE_WRITE.pin().get(&node.to_string()).cloned();
        if let Some(gate) = gate {
            gate.reached.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Ok(permit) = gate.permits.acquire().await {
                permit.forget();
            }
        }
    }
    /// Per node: how long each mint's two durable writes took.
    pub(crate) static RECORD_LATENCIES: Mutex<Vec<(String, std::time::Duration)>> = Mutex::new(Vec::new());
    pub(super) fn note_record_latency(node: &crate::node_id::NodeId, d: std::time::Duration) {
        RECORD_LATENCIES.lock().unwrap_or_else(|e| e.into_inner()).push((node.to_string(), d));
    }
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))] // read by `call_mint_tests`
    pub(crate) fn record_latencies(node: &crate::node_id::NodeId) -> Vec<std::time::Duration> {
        let node = node.to_string();
        RECORD_LATENCIES.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(n, _)| *n == node).map(|(_, d)| *d).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The `window` record's signed message is pinned** (the plan's golden): the domain tag, `v`, then each field
    /// length-prefixed. A change to the encoding breaks every window record a gateway has written.
    #[test]
    fn the_window_records_signed_message_is_pinned() {
        let w = CallWindow { v: 1, scope: "call:c7".into(), term: "c7".into(), valid_until_ms: 0x0102_0304_0506_0708 };
        let hex: String = window_message(&w).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "6d7963656c69756d3a63616c6c2d77696e646f773a76310a\
             01\
             0700000063616c6c3a6337\
             020000006337\
             0807060504030201"
        );
    }

    /// The key shapes the collector and the re-assertion read: only `call/{node segment}/{term}/{leaf}`.
    #[test]
    fn a_call_key_names_its_gateway_and_term_and_nothing_else_does() {
        assert_eq!(call_key_names("call/node:127.0.0.1:7000/c9/window"), Some(("node:127.0.0.1:7000".into(), "c9".into())));
        assert_eq!(call_key_names(&call_window_key("node:127.0.0.1:7000", "c9")), Some(("node:127.0.0.1:7000".into(), "c9".into())));
        for bad in ["call/node:127.0.0.1:7000/c9", "call/node:127.0.0.1:7000//window", "call/node:a/c9/window/x", "grant/node:a/c9/window", "call/746f6b656e3a67772f61/c9/window"] {
            assert_eq!(call_key_names(bad), None, "{bad}");
        }
    }
}
