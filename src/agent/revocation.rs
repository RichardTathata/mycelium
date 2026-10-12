//! Key revocation (WS-D / Quilt-DD#2, track 1 · D1) — closing the WS5 compromise caveat.
//!
//! WS5's retained-key-set rotation keeps a *retired* key trusted for verification forever (so
//! historical signatures stay valid across a rotation). That is right for *hygiene* rotation but
//! wrong for *compromise*: once a key is known-compromised, nothing it ever signed should be
//! trusted. This module adds explicit revocation — gossip-replicated, per-node, **coordinator-free**
//! (a new owned KV namespace, no central CT operator).
//!
//! ## Model
//!
//! A node revokes one of its own verifying keys by writing a **signed** [`RevocationEvent`] to
//! `sys/revocation/{node}/{revoked-key-hex}` (idempotent — keyed by the revoked key). It gossips
//! like any KV value. Each node validates what it receives ([`refresh`]) into
//! `CoreCtx::revoked_keys`, and **every verifier of a node's key** reads its trusted set through
//! `CoreCtx::trusted_keys` / `retain_trusted` (post-360 plan row Q): the KV `SignedData` arm, the
//! unsigned identity exemption, identity-proof chaining (`helpers::validate_and_merge_identity`),
//! consensus, [`super::helpers::known_verifying_keys`] (role claims, the audit chain, capability
//! authorization, gateway caller envelopes) and the knowledge layer's member keys. So anything signed
//! by a revoked key fails verification at every node once the revocation has reached it ("sub-second
//! revocation", bounded by gossip latency).
//!
//! **Order.** A revocation seen *after* a write was verified and applied does not un-apply it —
//! detection, not prevention: the write is history, and the revocation is what makes the next one
//! fail. Every verification after the revocation is seen refuses. Once validated here a revocation is
//! never undone (`RevokedKeys` is monotonic): a later tombstone or overwrite of the record does not
//! restore the key. After a restart the store's identity records are validated first and the
//! revocations against what they established ([`refresh`] runs after the identity prewarm).
//!
//! ## Validation rule (who may revoke what)
//!
//! A revocation of key `R` for node `N` counts **iff** it is signed by a key this node **holds** for
//! `N` and `R` is a key this node holds for `N` — *held* meaning authenticated here: in `peer_keys`
//! (proved, chained by rotation, or first-sighted) or CA-anchored by a direct connection, plus this
//! node's own current key when `N` is self. And it removes `R` from **`N`'s** trusted set only.
//!
//! - **Not the raw `sys/identity/{N}` value.** Until row Q the rule read "signed by `N`'s *current*
//!   key", taken from the bare `sys/identity/{N}` KV entry — which any member can write. A member
//!   could write `sys/identity/{V}` = *its key ‖ V's key* and revoke V's key under its own, and once
//!   revocation reached the KV verifier that would have silenced V fleet-wide.
//! - **Per node.** A member can put another's key in *its own* identity record (signed by its own
//!   key, which chains); its revocation of that key then strips it from the member's set, not the
//!   owner's. (The revoked set used to be one fleet-wide set of keys.)
//! - **Who can revoke is exactly who can sign as `N`**: a holder of any key `N` still has. With
//!   `require_identity_proofs` off (the default), a bare identity write can put a member's key in
//!   another's set — the documented identity-poisoning residual — and a member that can do that can
//!   already sign as that node; revocation adds nothing to it.
//! - **A compromised key can revoke too.** Whoever holds `N`'s compromised key can revoke `N`'s new
//!   one, as the owner revokes the old one with the new. Both count — a contested identity fails
//!   closed (none of the contested keys verify), and convergently (the result does not depend on
//!   arrival order). Recovery is operator work: member removal and a fresh identity. A revocation
//!   signed by a key that is itself revoked still counts, for the same reason: no rule here can tell
//!   the owner from the thief, and refusing one of them by arrival order would let nodes disagree.
//! - **A node cannot revoke the key it signs with** ([`revoke_key`] refuses): that would leave it with
//!   no key its peers trust. The compromise flow rotates first, publishing the new key, then revokes
//!   the old one with it (`rotate_identity_on_compromise`).
//!
//! A forged `sys/revocation/` write is LWW-accepted by the substrate (detection, not prevention) but
//! fails this validation at read, so it has no effect.
//!
//! D2 adds Merkle inclusion proofs over these events (`/gateway/transparency`); D1 is the
//! exclusion semantics that make revocation *work*.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
#[cfg(feature = "compliance")]
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::node_id::NodeId;
#[cfg(feature = "compliance")]
use super::TaskCtx;

/// KV namespace owned by the revocation log: `sys/revocation/{node}/{revoked-key-hex}`.
pub const REVOCATION_PREFIX: &str = "sys/revocation/";

fn hex32(k: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in k {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// KV key for a node's revocation of one verifying key.
#[cfg_attr(not(feature = "compliance"), allow(dead_code))]
pub fn revocation_key(node: &NodeId, revoked_key: &[u8; 32]) -> String {
    format!("{REVOCATION_PREFIX}{node}/{}", hex32(revoked_key))
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

/// A [`RevocationEvent`] plus the writer's Ed25519 signature over its canonical bytes. Stored at
/// `sys/revocation/{node}/{revoked-key-hex}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRevocation {
    pub event: RevocationEvent,
    pub sig:   Vec<u8>,
}

impl SignedRevocation {
    #[cfg(feature = "compliance")]
    fn sign(event: RevocationEvent, signing_key: &SigningKey) -> Self {
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

    /// Canonical wire encoding (the bytes a transparency leaf commits to via `leaf_hash`).
    #[cfg_attr(not(feature = "compliance"), allow(dead_code))]
    pub fn encode(&self) -> bytes::Bytes {
        bytes::Bytes::from(mycelium_core::serde_fixint::to_vec(self).unwrap_or_default())
    }

    /// Decode from the wire encoding. `None` on malformed bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        mycelium_core::serde_fixint::from_slice(bytes).ok()
    }
}

/// Append a signed revocation of `revoked_key` for *this* node. Requires the `tls` identity (the
/// revocation is signed by the current key). Returns `Err` without a tls identity, for the key this
/// node signs with (revoking it would leave the node with no key its peers trust — rotate first), and
/// for a key that is not one of this node's. The write gossips; once a peer has validated it
/// ([`refresh`]), **every verifier of this node's key at that peer** excludes `revoked_key` — KV
/// writes, identity records, consensus, role claims, the audit chain, gateway caller envelopes and
/// knowledge records (post-360 plan row Q).
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
    if revoked_key == tls.verifying_key_bytes() {
        return Err(crate::error::GossipError::InvalidField {
            field:  "revoked_key",
            reason: "this is the key this node signs with; revoking it would leave the node with no key its peers \
                     trust — rotate first (rotate_identity), then revoke the old key, or use rotate_identity_on_compromise".into(),
        });
    }
    let own_history = mycelium_core::kv_handle::KvHandle::from_core(std::sync::Arc::clone(&ctx.core))
        .get(&format!("sys/identity/{}", ctx.node_id))
        .map(|b| super::helpers::parse_identity_keys(&b))
        .unwrap_or_default();
    if !held_keys(ctx, &ctx.node_id).contains(&revoked_key) && !own_history.contains(&revoked_key) {
        return Err(crate::error::GossipError::InvalidField {
            field:  "revoked_key",
            reason: "not one of this node's keys; a node revokes only its own".into(),
        });
    }
    let event = RevocationEvent {
        node_id:     ctx.node_id.clone(),
        hlc:         ctx.hlc.tick(),
        revoked_key,
        reason,
    };
    let signed = SignedRevocation::sign(event, &tls.signing_key());
    let key = revocation_key(&ctx.node_id, &revoked_key);
    let _ = mycelium_core::kv_handle::KvHandle::from_core(std::sync::Arc::clone(&ctx.core))
        .set(key, signed.encode());
    // Our own verifiers stop at once, not when the watcher next runs.
    refresh(ctx);
    Ok(())
}

/// The keys this node **holds** for `node` — authenticated here, revoked or not: `peer_keys`
/// (proved, chained, or first-sighted), the CA anchors of a direct connection, and this node's own
/// current key when `node` is self. Never the raw `sys/identity/{node}` value, which any member can
/// write. What a revocation's signer and its revoked key must both be in.
pub(crate) fn held_keys(ctx: &mycelium_core::CoreCtx, node: &NodeId) -> Vec<[u8; 32]> {
    let mut held: Vec<[u8; 32]> = ctx.peer_keys.pin().get(node).cloned().unwrap_or_default();
    if let Some(anchored) = ctx.peer_anchor_keys.pin().get(node) {
        for k in anchored {
            if !held.contains(k) {
                held.push(*k);
            }
        }
    }
    if node == &ctx.node_id
        && let Some(t) = ctx.tls.get()
    {
        let cur = t.verifying_key_bytes();
        if !held.contains(&cur) {
            held.push(cur);
        }
    }
    held
}

/// The **validated** signed revocations published by one `node`, read from the local gossip view.
/// A revocation counts only if it names `node` (as its path does), is signed by a key this node
/// holds for `node`, and revokes a key this node holds for `node` (see the module-level validation
/// rule). Forged or foreign-signed revocations are dropped. Order is the KV scan order (callers that
/// need determinism — e.g. the Merkle root — sort).
pub(crate) fn validated_signed_revocations(ctx: &mycelium_core::CoreCtx, node: &NodeId) -> Vec<SignedRevocation> {
    let held = held_keys(ctx, node);
    if held.is_empty() {
        return Vec::new();
    }
    let prefix = format!("{REVOCATION_PREFIX}{node}/");
    let mut out = Vec::new();
    for (_key, bytes) in crate::store::scan_kv_prefix(&ctx.kv_state, &prefix) {
        let Some(signed) = SignedRevocation::decode(&bytes) else { continue };
        if &signed.event.node_id == node
            && held.contains(&signed.event.revoked_key)
            && held.iter().any(|k| signed.verify(k))
        {
            out.push(signed);
        }
    }
    out
}

/// Distinct nodes that have published at least one revocation in the local gossip view.
pub(crate) fn revocation_nodes(ctx: &mycelium_core::CoreCtx) -> Vec<NodeId> {
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

/// Validate every revocation in the local gossip view and record each that counts in
/// `CoreCtx::revoked_keys` (monotonic: nothing recorded is ever withdrawn). Run by the identity
/// watcher whenever a `sys/identity*` or `sys/revocation/` record changes — a revocation can arrive
/// before the identity record that makes its signer a held key, and counts once that record has been
/// validated — after the startup identity prewarm, and by [`revoke_key`] for this node's own.
pub(crate) fn refresh(ctx: &mycelium_core::CoreCtx) {
    for node in revocation_nodes(ctx) {
        for signed in validated_signed_revocations(ctx, &node) {
            if ctx.revoked_keys.revoke(&node, signed.event.revoked_key) {
                tracing::warn!(%node, "identity key revoked: {} no longer verifies for this node", hex32(&signed.event.revoked_key));
                // A node every one of whose held keys is revoked can no longer be verified at all:
                // a contested identity, failing closed. Say so — it is operator work (removal and a
                // fresh identity), not something this node can resolve.
                if held_keys(ctx, &node).iter().all(|k| ctx.revoked_keys.is_revoked(&node, k)) {
                    tracing::warn!(%node, "every key held for this node is revoked: nothing it signs verifies here");
                }
            }
        }
    }
}

#[cfg(all(test, feature = "compliance"))]
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
