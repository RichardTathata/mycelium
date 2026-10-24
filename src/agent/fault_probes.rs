//! The probes `mycelium test --faults` drives against a built fleet for its `revoke` class
//! (docs/plans/adopter-surface.md, X6). They reach a node's internals — its identity material, the
//! frame builders, the key view every verifier reads — which is why they sit beside those internals
//! rather than in [`crate::faults`]. Each is the same move row Q's own tests make
//! (`src/lib_tests.rs`, "revocation reaches every verifier"), lifted so the command can make it against
//! *an adopter's* fleet. Nothing here runs unless the command runs it, and nothing here is reachable
//! from outside the crate.

use std::sync::Arc;

use bytes::Bytes;

use super::GossipAgent;
use crate::node_id::NodeId;

/// What a thief who copied a node's material before a rotation holds: the TLS client configuration
/// (its certificate and key) and the identity signing key.
pub(crate) struct Stolen {
    pub(crate) client: Arc<rustls::ClientConfig>,
    pub(crate) signing_key: Arc<ed25519_dalek::SigningKey>,
    pub(crate) verifying_key: [u8; 32],
}

/// Copy `c`'s current material. `None` when the node has no TLS identity.
pub(crate) fn steal(c: &GossipAgent) -> Option<Stolen> {
    let tls = c.task_ctx.tls.get()?;
    Some(Stolen { client: tls.client_config(), signing_key: tls.signing_key(), verifying_key: tls.verifying_key_bytes() })
}

/// Whether `x` trusts `key` for `node` — the answer every verifier of a node's key reads
/// (`helpers::known_verifying_keys`: the anchored set minus the revoked).
pub(crate) fn trusts(x: &GossipAgent, node: &NodeId, key: &[u8; 32]) -> bool {
    super::helpers::known_verifying_keys(&x.task_ctx, node).contains(key)
}

/// Whether `x` holds any key for `node` yet — first contact.
pub(crate) fn knows(x: &GossipAgent, node: &NodeId) -> bool {
    x.task_ctx.peer_keys.pin().get(node).is_some_and(|k| !k.is_empty())
}

/// Drive one in-memory TLS handshake from `client` to `server`'s accept side. `Err` names the side
/// that refused — a revoked certificate is refused by the server with a reason that says `revoked`.
pub(crate) fn handshake(client: Arc<rustls::ClientConfig>, server: &GossipAgent) -> Result<(), String> {
    let Some(tls) = server.task_ctx.tls.get() else { return Err("the server has no TLS identity".into()) };
    let server_config = tls.server_config();
    let name = rustls::pki_types::ServerName::try_from("127.0.0.1").map_err(|e| e.to_string())?.to_owned();
    let mut c = rustls::ClientConnection::new(client, name).map_err(|e| format!("client: {e}"))?;
    let mut s = rustls::ServerConnection::new(server_config).map_err(|e| format!("server: {e}"))?;
    for _ in 0..16 {
        let mut buf = Vec::new();
        while c.wants_write() {
            c.write_tls(&mut buf).map_err(|e| format!("client write: {e}"))?;
        }
        if !buf.is_empty() {
            s.read_tls(&mut buf.as_slice()).map_err(|e| format!("server read: {e}"))?;
            s.process_new_packets().map_err(|e| format!("server refused: {e}"))?;
        }
        let mut buf = Vec::new();
        while s.wants_write() {
            s.write_tls(&mut buf).map_err(|e| format!("server write: {e}"))?;
        }
        if !buf.is_empty() {
            c.read_tls(&mut buf.as_slice()).map_err(|e| format!("client read: {e}"))?;
            c.process_new_packets().map_err(|e| format!("client refused: {e}"))?;
        }
        if !c.is_handshaking() && !s.is_handshaking() {
            return Ok(());
        }
    }
    Err("the handshake did not finish".into())
}

/// Put a `SignedData` frame on the wire from `a` in `as_node`'s name, signed by `sk` — what a holder of
/// one of `as_node`'s keys can send. Lands at a peer only if the peer still trusts `sk`'s key for
/// `as_node`.
pub(crate) fn send_signed_as(a: &GossipAgent, as_node: &NodeId, sk: &ed25519_dalek::SigningKey, key: &str, value: Vec<u8>) {
    use crate::framing::{canonical_sign_bytes, dispatch_gossip_try_send, kv_signing_message, make_gossip_update, ForwardHint, WireMessage};
    let mut u = make_gossip_update(&a.node_id, a.task_ctx.default_ttl, Arc::from(key), Bytes::from(value), false, &a.task_ctx.hlc);
    u.sender = as_node.id_hash();
    let canonical = canonical_sign_bytes(&u);
    let sig = mycelium_core::tls::sign_bytes(sk, &kv_signing_message(&canonical));
    let lo: [u8; 32] = sig[..32].try_into().expect("32 bytes");
    let hi: [u8; 32] = sig[32..].try_into().expect("32 bytes");
    let msg = WireMessage::SignedData { update: u, signer: as_node.id_hash(), signature: (lo, hi) };
    dispatch_gossip_try_send(&a.task_ctx.gossip_txs, msg, a.node_id.id_hash(), ForwardHint::All, &a.task_ctx.kv_state.dropped_frames);
}

/// A sealed identity record whose history is `history_keys` and whose proof `signer` signs — what a
/// holder of a node's key can publish for that node.
pub(crate) fn sealed_record(history_keys: &[[u8; 32]], signer: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use ed25519_dalek::Signer;
    let history: Vec<u8> = history_keys.iter().flatten().copied().collect();
    let sig = signer.sign(&super::helpers::identity_proof_message(&history)).to_bytes();
    let proof = super::helpers::encode_identity_proof(&signer.verifying_key().to_bytes(), &sig);
    super::helpers::encode_sealed_identity(&history, &proof)
}

/// The key the sealed identity record of `node` is written under.
pub(crate) fn sealed_identity_key(node: &NodeId) -> String {
    format!("sys/identity-signed/{node}")
}
