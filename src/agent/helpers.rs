use std::sync::Arc;

use super::{GossipAgent, TaskCtx};

// ── Private impl helpers ──────────────────────────────────────────────────────

impl GossipAgent {
    /// This node's `LocalityPath`, derived from `config.locality_path`. Returns
    /// `None` when locality is unconfigured. Shared helper used by the
    /// consensus engine builder, the gossip-shard start path, and the
    /// Phase 5 locality-aware resolution methods.
    pub(crate) fn self_locality(&self) -> Option<crate::locality::LocalityPath> {
        if self.config.locality_path.is_empty() {
            None
        } else {
            Some(crate::locality::LocalityPath::new(
                self.config.locality_path.iter().cloned(),
            ))
        }
    }
}

// ── Layer-I/II ops behind the typed handles ───────────────────────────────────
// Moved to `mycelium-core::ops` (v2 M3, over `&CoreCtx`) so the substrate handles
// can live in core. Re-exported so existing `helpers::emit_signal` / `kv_*` /
// `group_members_ctx` call sites resolve unchanged — they pass `&TaskCtx`, which
// Deref-coerces to the `&CoreCtx` these take.
pub(crate) use mycelium_core::ops::{
    emit_signal, emit_signal_async,
    group_members_ctx, kv_delete, kv_get, kv_scan_prefix,
    kv_set, kv_subscribe, kv_subscribe_prefix,
    kv_subscribe_prefix_with_predicate,
};

/// How long a `MembershipIntent` is honoured as a declared electorate floor. Mirrors the
/// membership governor's own TTL: an intent nobody is refreshing has evaporated, and an evaporated
/// floor must not block elections forever (posture rule 5, *roles evaporate*).
#[cfg(any(feature = "consensus", feature = "gateway"))]
pub(crate) const ELECTORATE_INTENT_TTL_MS: u64 = 30_000;

/// The electorate a group proposal is allowed to decide with.
#[cfg(feature = "consensus")]
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Electorate {
    /// `n` members may vote; the quorum is computed from this.
    Established(usize),
    /// No decision may be attempted — the roster is empty, or smaller than the group declares.
    Unavailable { observed: usize, declared_min: usize },
}

/// Decide whether an election may proceed, given what this node can **see** and what the group
/// **declares**.
///
/// Three cases, and the middle one is the whole point:
///
/// - `observed == 0` — unknown or unjoined group. **Refuse.** This used to be counted as one
///   member with a quorum of one, which the proposer's own self-vote satisfied, so every node
///   committed its own candidate unopposed. *"I cannot see members"* must not mean *"I may decide
///   alone."*
/// - `observed < declared_min` — a fresh `MembershipIntent` says the group should hold at least
///   `declared_min`, and this node sees fewer. Its view is **partial**, and a partial view confers
///   a *smaller* quorum, which is the same defect wearing a plausible number. **Refuse.**
/// - otherwise — decide with `observed`. A genuine one-member group elects normally: an explicit
///   singleton is legitimate, an inferred one is not.
///
/// `declared_min == 0` means no fresh intent exists, so there is nothing to be partial against and
/// only the empty case refuses. That is deliberate: the floor binds while it is asserted, not
/// after it evaporates.
#[cfg(feature = "consensus")]
pub(crate) fn resolve_electorate(observed: usize, declared_min: usize) -> Electorate {
    if observed == 0 || (declared_min > 0 && observed < declared_min) {
        return Electorate::Unavailable { observed, declared_min };
    }
    Electorate::Established(observed)
}

/// The electorate floor a group declares through a **fresh** `MembershipIntent`, or `0`.
///
/// Read from the same key the membership governor writes (`sys/govern/membership/{group}`) and
/// subject to the same freshness rule **on the same clock**, so a stale intent cannot wedge a
/// group shut. `written_at_ms` is Unix ms (`FleetIntent::stamp`), so the comparison is against
/// the wall-clock seam, not the monotonic one: the monotonic seam's origin is `MONO_ORIGIN_NS`
/// (a year in ns, ~3.15e10 ms), three orders below any Unix timestamp, so `now - written_at`
/// saturated to `0` for every intent and the TTL never fired (external review F7, 2026-09-26;
/// shipped in #377). `electorate_intent_tests` below crosses the TTL and failed before this.
#[cfg(any(feature = "consensus", feature = "gateway"))]
pub(crate) fn declared_electorate_min(ctx: &crate::agent::TaskCtx, group: &str) -> usize {
    let key = format!("{}{}", crate::agent::membership_governor::MEMBERSHIP_PREFIX, group);
    let Some(bytes) = ctx.kv_state.store.pin().get(key.as_str()).and_then(|e| e.data.clone())
    else { return 0 };
    let Ok(intent) = mycelium_core::serde_fixint::from_slice::<
        crate::agent::membership_governor::MembershipIntent>(&bytes)
    else { return 0 };
    let now = mycelium_core::sim_seam::wall_now_ms();
    if now.saturating_sub(intent.written_at_ms) > ELECTORATE_INTENT_TTL_MS { return 0; }
    intent.min
}

#[cfg(feature = "consensus")]
pub(crate) fn compute_quorum_size(config_size: usize, member_count: usize) -> usize {
    if config_size > 0 { config_size } else { member_count / 2 + 1 }
}

// Re-exported for the consensus-layer call sites (the M2-gated `agent::make_gossip_update`
// alias); core KV writes use `mycelium_core::framing::make_gossip_update` directly.
#[cfg(feature = "consensus")]
pub(crate) use crate::framing::make_gossip_update;


/// Cached variant of `group_members_ctx`. Returns a cached roster when the
/// `grp_generation` counter is unchanged and the entry is within `ttl`.
#[cfg(feature = "consensus")]
pub(crate) fn cached_group_members_ctx(
    ctx:   &TaskCtx,
    group: &str,
    ttl:   std::time::Duration,
) -> Arc<super::RosterEntry> {
    use std::sync::atomic::Ordering;
    let group_key: Arc<str> = Arc::from(group);
    // Acquire (not Relaxed): pairs with the Release bump in store::apply_and_notify so observing
    // a new generation guarantees the prefix_index membership it advertises is visible (audit 2026-07-15).
    let current_gen = ctx.kv_state.grp_generation.load(Ordering::Acquire);
    let guard = ctx.group_roster_cache.pin();
    if let Some(entry) = guard.get(&group_key)
        && entry.grp_gen == current_gen && entry.fetched_at.elapsed() < ttl {
            return Arc::clone(entry);
        }
    let members = group_members_ctx(ctx, group);
    let fresh = Arc::new(super::RosterEntry {
        members,
        fetched_at: std::time::Instant::now(),
        grp_gen: current_gen,
    });
    guard.insert(group_key, Arc::clone(&fresh));
    fresh
}

/// Returns this node's `LocalityPath` from `ctx.config.locality_path`.
/// Returns `None` when locality is unconfigured.
pub(super) fn self_locality_ctx(ctx: &TaskCtx) -> Option<crate::locality::LocalityPath> {
    if ctx.config.locality_path.is_empty() {
        None
    } else {
        Some(crate::locality::LocalityPath::new(
            ctx.config.locality_path.iter().cloned(),
        ))
    }
}

/// Constructs a [`ConsensusEngine`] from a `TaskCtx` reference.
/// Used by `ConsensusHandle` methods.
#[cfg(feature = "consensus")]
pub(super) fn make_consensus_engine_ctx(
    ctx:                 &Arc<TaskCtx>,
    abstain_when_opaque: bool,
    use_trust_slices:    bool,
    max_abstain_ballots: u32,
    topology_policy:     Option<crate::config::GroupTopologyPolicy>,
) -> crate::consensus::ConsensusEngine {
    crate::consensus::ConsensusEngine {
        task_ctx: Arc::clone(ctx),
        abstain_when_opaque,
        use_trust_slices,
        max_abstain_ballots,
        self_locality: self_locality_ctx(ctx),
        topology_policy,
    }
}

/// Returns the group member with the lowest observed load for `kind`,
/// operating on a `TaskCtx` reference.
pub(super) fn suggest_leader_ctx(
    ctx:     &TaskCtx,
    group:   &str,
    kind:    &str,
    max_age: std::time::Duration,
) -> crate::node_id::NodeId {
    use super::opacity::peer_load_ctx;
    let members = group_members_ctx(ctx, group);
    if members.is_empty() {
        return ctx.node_id.clone();
    }
    // Trust-weighting reads consensus `trust/` slices; without the `consensus`
    // feature there are none, so leader choice degrades to pure load (trust = 0
    // everywhere → `fill / (1.0 + 0)`). Graceful degradation, not a hard dependency.
    #[allow(unused_mut)]
    let mut trust_counts: ahash::AHashMap<u64, usize> = ahash::AHashMap::new();
    #[cfg(feature = "consensus")]
    {
        let trust_prefix = format!("{}{}/", crate::consensus::consensus_ns::TRUST, group);
        for (_, bytes) in kv_scan_prefix(ctx, &trust_prefix) {
            let Ok(peers) = mycelium_core::serde_fixint::from_slice::<Vec<crate::node_id::NodeId>>(&bytes)
                else { continue };
            for p in peers {
                *trust_counts.entry(p.id_hash()).or_insert(0) += 1;
            }
        }
    }
    let load_by_node: ahash::AHashMap<Arc<str>, f32> = peer_load_ctx(ctx, max_age)
        .into_iter()
        .filter(|(_, k, _)| k.as_ref() == kind)
        .map(|(n, _, s)| (n, s.fill_ratio))
        .collect();
    let best = members.iter().min_by(|a, b| {
        let score = |n: &crate::node_id::NodeId| -> f32 {
            let fill = load_by_node.get(n.to_string().as_str()).copied().unwrap_or(0.0);
            let trust = *trust_counts.get(&n.id_hash()).unwrap_or(&0) as f32;
            fill / (1.0 + trust)
        };
        score(a).partial_cmp(&score(b))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id_hash().cmp(&b.id_hash()))
    });
    best.cloned().unwrap_or_else(|| ctx.node_id.clone())
}


// ── WS5: retained verifying-key set (hot cert rotation, option B) ──────────────
//
// Identity keys rotate, but historical signatures (audit chain, committed
// consensus values, role claims) must still verify. So `peer_keys` holds a
// *retained set* per node — every key a node has published — and verification
// tries them all. `sys/identity/{node}` carries one key (32 bytes) normally, or
// `current ‖ previous` (64 bytes) during a rotation window. Population
// *accumulates* (never drops a key except on tombstone), so a key is verifiable
// for the life of the records it signed.
//
// Tradeoff (documented): a retired key stays trusted for verification, so
// rotating away from a *compromised* key needs explicit revocation on top —
// it is not automatic. Hygiene rotation is fully covered.

/// Parse a `sys/identity/{node}` value into verifying keys: the value is a
/// concatenation of 32-byte keys (`32 × N`) — the first is the **current** key,
/// the rest are retained priors (full rotation history, WS5 multi-key archival).
/// A 32-byte value is the common single-key case; an empty or non-multiple-of-32
/// value yields no keys.
#[cfg(feature = "tls")]
pub(crate) fn parse_identity_keys(bytes: &[u8]) -> Vec<[u8; 32]> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(32) {
        return Vec::new();
    }
    bytes
        .chunks_exact(32)
        .map(|c| {
            let mut a = [0u8; 32];
            a.copy_from_slice(c);
            a
        })
        .collect()
}

/// Build the durable `sys/identity/{node}` value: `current` first, then every
/// other previously-published key (deduped, order otherwise preserved) — so the
/// **full** rotation history is retained on disk and historical signatures stay
/// verifiable across any number of rotations and restarts (WS5 multi-key
/// archival). Grows 32 bytes per rotation; rotations are rare operational events.
#[cfg(feature = "tls")]
pub(crate) fn encode_identity_history(current: [u8; 32], existing: &[[u8; 32]]) -> Vec<u8> {
    let mut keys = vec![current];
    for k in existing {
        if !keys.contains(k) {
            keys.push(*k);
        }
    }
    let mut out = Vec::with_capacity(keys.len() * 32);
    for k in &keys {
        out.extend_from_slice(k);
    }
    out
}

/// Union `new_keys` into `node`'s retained key set in `peer_keys` (accumulate;
/// existing keys are never dropped — historical signatures stay verifiable).
///
/// The union is computed inside a papaya `compute` closure so the read →
/// merge → write is a single atomic CAS, retried if the entry changes
/// concurrently (the "lock-free op + unserialised derived effect" family — see
/// CLAUDE.md §Lock-free mutation rules). A prior get-clone-modify-insert could
/// lose a key when two rotations for the same node merged concurrently: each
/// read the same base set, each inserted its own superset, and the later insert
/// clobbered the earlier — silently dropping a still-needed historical verifying
/// key. The closure is retry-safe: it derives `merged` afresh from the *current*
/// stored value on every invocation and never mutates captured state.
#[cfg(feature = "tls")]
pub(crate) fn merge_peer_keys(
    peer_keys: &papaya::HashMap<crate::node_id::NodeId, Vec<[u8; 32]>>,
    node: &crate::node_id::NodeId,
    new_keys: &[[u8; 32]],
) {
    let guard = peer_keys.pin();
    guard.compute(node.clone(), |existing| {
        // Recompute the union from the current stored set every invocation;
        // papaya re-runs this if the entry changed between read and CAS.
        let base: &[[u8; 32]] = existing.map(|(_, set)| set.as_slice()).unwrap_or(&[]);
        let mut merged = base.to_vec();
        for k in new_keys {
            if !merged.contains(k) {
                merged.push(*k);
            }
        }
        // No-op if nothing new and the entry already exists (avoid a needless CAS);
        // otherwise upsert (papaya `Insert` both creates and replaces).
        if existing.is_some() && merged.len() == base.len() {
            papaya::Operation::Abort(())
        } else {
            papaya::Operation::Insert(merged)
        }
    });
}

/// Record a directly-connected peer's **CA-validated** key (identity-auth Phase 1b): add it to
/// the peer's anchor set, and merge it into `peer_keys` so it is usable for verification even if
/// the peer's `sys/identity` KV entry is absent or poisoned to omit it. Both updates use
/// retry-safe papaya `compute` closures (the lock-free-op-then-effect family). Idempotent.
#[cfg(feature = "tls")]
pub(crate) fn record_peer_anchor(
    anchor_keys: &papaya::HashMap<crate::node_id::NodeId, std::collections::HashSet<[u8; 32]>>,
    peer_keys: &papaya::HashMap<crate::node_id::NodeId, Vec<[u8; 32]>>,
    peer: &crate::node_id::NodeId,
    key: [u8; 32],
) {
    anchor_keys.pin().compute(peer.clone(), |existing| {
        let mut set = existing.map(|(_, s)| s.clone()).unwrap_or_default();
        let was_new = set.insert(key);
        if existing.is_some() && !was_new {
            papaya::Operation::Abort(())
        } else {
            papaya::Operation::Insert(set)
        }
    });
    merge_peer_keys(peer_keys, peer, &[key]);
}

/// Detection tripwire (identity-auth Phase 1b): given the KV-asserted keys for `node` about to be
/// merged, warn + count if `node` has a **CA-anchored** key and the KV set introduces one that is
/// not in the anchor set — the poisoning signal. Detection-only (accumulate still happens in 1b);
/// may briefly trip on a legitimate rotation until the new key is re-anchored on reconnect.
#[cfg(feature = "tls")]
pub(crate) fn flag_identity_anchor_conflict(
    anchor_keys: &papaya::HashMap<crate::node_id::NodeId, std::collections::HashSet<[u8; 32]>>,
    conflict_counter: &std::sync::atomic::AtomicU64,
    node: &crate::node_id::NodeId,
    kv_keys: &[[u8; 32]],
) {
    let Some(anchors) = anchor_keys.pin().get(node).cloned() else { return };
    if anchors.is_empty() {
        return;
    }
    if kv_keys.iter().any(|k| !anchors.contains(k)) {
        conflict_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tracing::warn!(
            node = %node,
            "sys/identity anchor conflict: a KV identity key differs from this peer's \
             CA-anchored key (see SystemStats::identity_anchor_conflicts) — poisoning signal, \
             or a rotation not yet re-anchored"
        );
    }
}

// ── Identity Phase 2: signed proofs (prevention) ────────────────────────────
//
// `sys/identity-proof/{V}` = signer_key(32) ‖ signature(64) over the `sys/identity/{V}` history
// bytes. A node accepts an identity entry only if its proof is signed by a key already trusted
// for V (CA anchor from Phase 1b, or a prior valid key — rotation chaining), or, for a
// never-before-seen V, TOFU-accepts a self-signed first entry. No proof ⇒ rollout tolerance
// (old unsigned nodes still establish; Phase 3 tightens that). Signed by an *untrusted* key ⇒
// rejected — the poisoning case.

/// Encode a proof value: `signer_key(32) ‖ signature(64)` = 96 bytes.
#[cfg(feature = "tls")]
pub(crate) fn encode_identity_proof(signer_key: &[u8; 32], sig: &[u8; 64]) -> Vec<u8> {
    let mut v = Vec::with_capacity(96);
    v.extend_from_slice(signer_key);
    v.extend_from_slice(sig);
    v
}

/// Parse a 96-byte proof value into `(signer_key, signature)`.
#[cfg(feature = "tls")]
pub(crate) fn parse_identity_proof(bytes: &[u8]) -> Option<([u8; 32], [u8; 64])> {
    if bytes.len() != 96 {
        return None;
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes[..32]);
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bytes[32..96]);
    Some((key, sig))
}

/// Sign a node's own identity `history` bytes with its current key, producing the proof value
/// to publish at `sys/identity-proof/{self}`. At startup the current key is the node's initial
/// key (self-signed → TOFU/anchor); during rotation this runs *before* cutover so it signs with
/// the **prior** key (which peers already trust → chains the new key in).
#[cfg(feature = "tls")]
pub(crate) fn sign_identity_proof(tls: &mycelium_core::tls::NodeTls, history: &[u8]) -> Vec<u8> {
    let sk = tls.signing_key();
    let signer = sk.verifying_key().to_bytes();
    let sig = crate::tls::sign_bytes(&sk, history);
    encode_identity_proof(&signer, &sig)
}

/// The sealed-identity record's version byte. Bumping it is a format change old readers must
/// refuse rather than misparse, which is why the version leads the value.
#[cfg(feature = "tls")]
pub(crate) const SEALED_IDENTITY_V1: u8 = 1;

/// Build the `sys/identity-signed/{node}` value: `version(1) ‖ history ‖ proof(96)`.
///
/// One record, so a reader can never see the keys without the proof that authenticates them. The
/// proof is the *same* bytes published at `sys/identity-proof/{node}` and signs the *same* history
/// bytes published at `sys/identity/{node}` — this is a re-packaging, not a second credential, so
/// the two forms can never disagree about what was signed.
#[cfg(feature = "tls")]
pub(crate) fn encode_sealed_identity(history: &[u8], proof: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + history.len() + proof.len());
    v.push(SEALED_IDENTITY_V1);
    v.extend_from_slice(history);
    v.extend_from_slice(proof);
    v
}

/// Split a sealed record into `(history, proof)`, or `None` if it is not one.
///
/// The proof is fixed-width (96 bytes), so the split is unambiguous without a length prefix: a
/// well-formed value is the version byte, a whole number of 32-byte keys, and the proof. Anything
/// else — wrong version, short value, a history that is not key-aligned — returns `None` and is
/// treated as *no record*, never as a partial one.
#[cfg(feature = "tls")]
pub(crate) fn parse_sealed_identity(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    const PROOF: usize = 96;
    if bytes.len() < 1 + 32 + PROOF || bytes[0] != SEALED_IDENTITY_V1 {
        return None;
    }
    let (history, proof) = bytes[1..].split_at(bytes.len() - 1 - PROOF);
    if history.is_empty() || !history.len().is_multiple_of(32) {
        return None;
    }
    Some((history, proof))
}

/// Choose which identity record to validate for a peer: the **sealed** one when it is present and
/// well-formed, else the legacy `sys/identity/` + `sys/identity-proof/` pair.
///
/// The `require_proofs` arm is the point of the whole mechanism. A legacy pair is two independent
/// gossip messages; accepting it under `require_identity_proofs` would reopen exactly the window
/// the flag exists to close, so the pair is surfaced **without** its proof and the validator
/// rejects it — the same answer the flag already gave a node that published no proof at all, for
/// the same reason: that node predates the mechanism being required.
///
/// With the flag off (the default) nothing changes for an existing deployment: the sealed record is
/// preferred when present, and the legacy pair still works when it is not.
#[cfg(feature = "tls")]
pub(crate) fn resolve_identity_record<'a>(
    sealed: Option<&'a [u8]>,
    legacy_history: &'a [u8],
    legacy_proof: Option<&'a [u8]>,
    require_proofs: bool,
) -> (&'a [u8], Option<&'a [u8]>) {
    if let Some((history, proof)) = sealed.and_then(parse_sealed_identity) {
        return (history, Some(proof));
    }
    if require_proofs {
        return (legacy_history, None);
    }
    (legacy_history, legacy_proof)
}

/// Validate a peer `node`'s identity entry against its (optional) proof, and merge its keys into
/// `peer_keys` only if authenticated (identity-auth Phase 2). See the module comment for the rule
/// table. Rejection increments `conflict_counter` + warns; no proof falls back to the Phase-1b
/// accept-and-flag path (rollout tolerance).
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_and_merge_identity(
    peer_keys: &papaya::HashMap<crate::node_id::NodeId, Vec<[u8; 32]>>,
    anchor_keys: &papaya::HashMap<crate::node_id::NodeId, std::collections::HashSet<[u8; 32]>>,
    conflict_counter: &std::sync::atomic::AtomicU64,
    node: &crate::node_id::NodeId,
    history_bytes: &[u8],
    kv_keys: &[[u8; 32]],
    proof: Option<&[u8]>,
    require_proofs: bool,
) {
    use std::sync::atomic::Ordering;
    let Some((signer, sig)) = proof.and_then(parse_identity_proof) else {
        // No proof.
        if require_proofs {
            // Phase 3 (require_identity_proofs): reject an unsigned entry outright — closes the
            // "mimic a pre-Phase-2 node" residual. Safe only once the whole fleet writes proofs;
            // a late-arriving proof re-validates via the identity watcher (which also watches the
            // proof prefix). The key is NOT merged.
            conflict_counter.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(
                node = %node,
                "rejected sys/identity: no proof and require_identity_proofs is set (Phase 3)"
            );
            return;
        }
        // Rollout tolerance (default): keep the Phase-1b detection tripwire, then accept.
        flag_identity_anchor_conflict(anchor_keys, conflict_counter, node, kv_keys);
        merge_peer_keys(peer_keys, node, kv_keys);
        return;
    };

    let sig_ok = crate::tls::verify_bytes(&signer, history_bytes, &sig);
    let anchor = anchor_keys.pin().get(node).cloned().unwrap_or_default();
    let current = peer_keys.pin().get(node).cloned().unwrap_or_default();
    let established = !anchor.is_empty() || !current.is_empty();
    let signer_trusted = anchor.contains(&signer) || current.contains(&signer);

    let accept = if established {
        // A key may enter an established/anchored set only via a proof signed by a key already
        // trusted for V — a legitimate rotation chains through the prior key. An overwrite signed
        // by any other key (the poisoning case) fails here.
        sig_ok && signer_trusted
    } else {
        // TOFU first-sighting (V unknown, never connected): accept a self-signed entry — the
        // signer must be one of the keys in its own published history.
        sig_ok && kv_keys.contains(&signer)
    };

    if accept {
        merge_peer_keys(peer_keys, node, kv_keys);
    } else {
        conflict_counter.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(
            node = %node,
            "rejected sys/identity: proof not chained to a trusted key for this peer \
             (identity poisoning signal — key NOT added to peer_keys)"
        );
    }
}

/// All verifying keys known for `node`: the retained set in `peer_keys`, plus
/// this node's own current key when `node` is self (covers the gap before the
/// node's own `sys/identity` write has cycled back through the watcher). Used by
/// the `compliance` verify paths (role claims, audit chain).
#[cfg(feature = "compliance")]
pub(crate) fn known_verifying_keys(ctx: &TaskCtx, node: &crate::node_id::NodeId) -> Vec<[u8; 32]> {
    let mut keys: Vec<[u8; 32]> = ctx.peer_keys.pin().get(node).cloned().unwrap_or_default();
    if node == &ctx.node_id
        && let Some(t) = ctx.tls.get()
    {
        let cur = t.verifying_key_bytes();
        if !keys.contains(&cur) {
            keys.push(cur);
        }
    }
    // WS-D / D1: exclude validly-revoked keys. A key the node has explicitly revoked (signed by its
    // current identity) is no longer trusted for *any* signature — closing the WS5 compromise
    // caveat. Retained-key verification (role claims, audit chain) consults this set here, so a
    // revoked key fails verification everywhere it is read.
    let revoked = super::revocation::revoked_key_set(ctx);
    if !revoked.is_empty() {
        keys.retain(|k| !revoked.contains(k));
    }
    keys
}

#[cfg(all(test, feature = "tls"))]
mod ws5_identity_key_tests {
    use super::{encode_identity_history, parse_identity_keys};

    #[test]
    fn parse_handles_n_keys_and_rejects_bad_lengths() {
        let a = [1u8; 32];
        assert_eq!(parse_identity_keys(&a), vec![a]);
        // empty / non-multiple-of-32 → no keys
        assert!(parse_identity_keys(&[]).is_empty());
        assert!(parse_identity_keys(&[0u8; 31]).is_empty());
        assert!(parse_identity_keys(&[0u8; 65]).is_empty());
        // 96 bytes → three keys, in order
        let mut v = Vec::new();
        v.extend_from_slice(&[1u8; 32]);
        v.extend_from_slice(&[2u8; 32]);
        v.extend_from_slice(&[3u8; 32]);
        assert_eq!(parse_identity_keys(&v), vec![[1u8; 32], [2u8; 32], [3u8; 32]]);
    }

    #[test]
    fn encode_puts_current_first_and_dedups() {
        let (a, b, c) = ([1u8; 32], [2u8; 32], [3u8; 32]);
        // c is the new current; existing already contains a dup of c plus b, a.
        let parsed = parse_identity_keys(&encode_identity_history(c, &[b, a, c]));
        assert_eq!(parsed, vec![c, b, a], "current first, priors retained, no dup");
    }

    #[test]
    fn full_history_retained_across_multiple_rotations() {
        // k1 → rotate to k2 → rotate to k3; every key must persist (WS5 archival).
        let (k1, k2, k3) = ([10u8; 32], [20u8; 32], [30u8; 32]);
        let h1 = encode_identity_history(k1, &[]);
        assert_eq!(parse_identity_keys(&h1), vec![k1]);
        let h2 = encode_identity_history(k2, &parse_identity_keys(&h1));
        assert_eq!(parse_identity_keys(&h2), vec![k2, k1]);
        let h3 = encode_identity_history(k3, &parse_identity_keys(&h2));
        assert_eq!(parse_identity_keys(&h3), vec![k3, k2, k1],
            "all three keys retained across two rotations");
    }

    // Regression for the long-standing ratings watch-item (Runs 23–25):
    // `merge_peer_keys` was a get-clone-modify-insert, not a papaya `compute`.
    // Many threads each merging a distinct key for the SAME node would read the
    // same base set and clobber one another on insert — dropping retained keys
    // and making historical signatures unverifiable. With the atomic `compute`
    // fix every distinct key must survive. Pre-fix this loses keys on most runs.
    #[test]
    fn concurrent_merges_for_one_node_never_drop_a_key() {
        use crate::node_id::NodeId;
        use std::sync::Arc;

        const THREADS: usize = 16;
        const ITERS: usize = 64; // 16 × 64 = 1024 distinct keys contended per round

        for _round in 0..32 {
            let peer_keys: Arc<papaya::HashMap<NodeId, Vec<[u8; 32]>>> =
                Arc::new(papaya::HashMap::new());
            let node = NodeId::new("127.0.0.1", 9000).unwrap();

            std::thread::scope(|s| {
                for t in 0..THREADS {
                    let pk = Arc::clone(&peer_keys);
                    let node = node.clone();
                    s.spawn(move || {
                        for i in 0..ITERS {
                            // Globally-unique key per (thread, iter) so any clobber
                            // is detectable as a missing entry in the final union.
                            let mut k = [0u8; 32];
                            let id = (t * ITERS + i) as u32;
                            k[..4].copy_from_slice(&id.to_le_bytes());
                            super::merge_peer_keys(&pk, &node, &[k]);
                        }
                    });
                }
            });

            let stored = peer_keys.pin().get(&node).cloned().unwrap_or_default();
            assert_eq!(
                stored.len(),
                THREADS * ITERS,
                "all {} concurrently-merged keys must survive; lost {} — \
                 merge_peer_keys clobbered a concurrent writer",
                THREADS * ITERS,
                THREADS * ITERS - stored.len(),
            );
        }
    }
}

#[cfg(all(test, any(feature = "consensus", feature = "gateway")))]
mod electorate_intent_tests {
    //! **F7 (external review, 2026-09-26): the electorate floor never expired.** The reader
    //! compared a Unix-ms stamp against the *monotonic* seam, whose origin is a year of
    //! nanoseconds — three orders of magnitude below any Unix timestamp — so the saturating
    //! subtraction was always `0` and the TTL never fired. A stale intent could wedge a group
    //! shut indefinitely while the membership governor, reading the same key with wall time,
    //! had long since let it evaporate. This test failed before the fix.
    use super::{declared_electorate_min, ELECTORATE_INTENT_TTL_MS};
    use crate::agent::membership_governor::{MembershipIntent, MEMBERSHIP_PREFIX};
    use crate::{GossipAgent, GossipConfig, NodeId};
    use std::sync::Arc;

    fn seed(agent: &GossipAgent, group: &str, min: usize, written_at_ms: u64) {
        let mut intent = MembershipIntent::new(group, min, None);
        intent.written_at_ms = written_at_ms;
        agent.task_ctx.kv_state.store.pin().insert(
            Arc::from(format!("{MEMBERSHIP_PREFIX}{group}").as_str()),
            crate::store::StoreEntry {
                data:      Some(bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(&intent).unwrap())),
                timestamp: 1,
            },
        );
    }

    #[test]
    fn a_stale_membership_intent_stops_declaring_a_floor() {
        let agent = GossipAgent::new(NodeId::new("127.0.0.1", 0).unwrap(), GossipConfig::default());
        let now = mycelium_core::sim_seam::wall_now_ms();

        seed(&agent, "fresh", 5, now);
        assert_eq!(declared_electorate_min(&agent.task_ctx, "fresh"), 5, "a fresh intent binds");

        seed(&agent, "stale", 5, now - 4 * ELECTORATE_INTENT_TTL_MS);
        assert_eq!(
            declared_electorate_min(&agent.task_ctx, "stale"), 0,
            "an intent four TTLs old must have evaporated — the floor binds while asserted, not after"
        );

        assert_eq!(declared_electorate_min(&agent.task_ctx, "absent"), 0, "no intent, no floor");
    }
}
