//! **Capability advertisements bound to authority** (Boundary H item H3) —
//! [`docs/design/knowledge-issuer-binding.md`](../../../docs/design/knowledge-issuer-binding.md) §6.
//!
//! # The gap this closes
//!
//! Any member may advertise any capability name under its own `cap/{node}/…`, and
//! `advertise_capability` checks no role or mandate. A member holding a power no mandate gave it — a
//! stolen credential — can therefore offer that power to others as a service, and the credential
//! itself never travels: a power **laundered as a capability** (threat model, Boundary H).
//!
//! # A reader-side check, so Layer I is never taught a higher law
//!
//! The advertisement still gossips — detection, not prevention. What changes is what a reader that
//! **protects a namespace** will resolve. A capability in a protected namespace resolves only if its
//! advertiser presents, in the capability's own attributes:
//!
//! - a [`SignedMandateGrant`] (attribute [`GRANT_ATTR`]) that is issued, entitled, current and
//!   unsuperseded (P2's four checks), **naming the advertiser as holder**, and **permitting**
//!   `serve:{namespace}/{name}`;
//! - a **possession proof** (attribute [`POSSESSION_ATTR`]): the holder's signature over this grant and
//!   *this* advertisement ([`advertisement_request`]), so a grant cannot be lifted onto another node's
//!   advertisement or another capability.
//!
//! Everything else in a protected namespace is filtered out **and reported** as an
//! [`UnbackedCapability`] with its reason — the observation a monitor or NovusLens turns into a
//! Capability/Authority/Responsibility finding. Namespaces the reader does not protect pass through
//! untouched, and the router's order is never changed.
//!
//! # Honest limit
//!
//! Colluders resolve with their own policy. This keeps a laundered power away from **honest** readers
//! and makes it visible; it does not stop a cohort using the power among its own members.

use std::collections::BTreeSet;

use base64::Engine;

use super::grant::{GrantVerdict, GrantVerifier, SignedMandateGrant};
use crate::capability::{CapValue, Capability};
use crate::knowledge::issuer::{MemberKeySource, TrustedExternalIssuers};
use crate::knowledge::IssuerId;
use crate::node_id::NodeId;

/// The capability attribute carrying the advertiser's grant, as JSON.
pub const GRANT_ATTR: &str = "mycelium.grant";
/// The capability attribute carrying the advertiser's possession proof, as base64.
pub const POSSESSION_ATTR: &str = "mycelium.grant.possession";

/// The operation a grant must enumerate for its holder to serve `cap`: `serve:{namespace}/{name}`.
pub fn serve_operation(cap: &Capability) -> String {
    format!("serve:{}/{}", cap.namespace, cap.name)
}

/// The bytes a possession proof binds to: this node advertising this capability. Tagged and
/// length-prefixed, so it cannot be confused with any other signed message.
pub fn advertisement_request(node: &NodeId, cap: &Capability) -> Vec<u8> {
    let mut out = b"mycelium.cap/advertise/1".to_vec();
    for part in [node.to_string().as_bytes(), cap.namespace.as_bytes(), cap.name.as_bytes()] {
        out.extend_from_slice(&(part.len() as u32).to_le_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// **For an advertiser:** attach `grant` and a possession proof to `cap`. `sign` is the holder's
/// signing function (a node's `sign_with_identity`).
pub fn attach_grant(
    mut cap: Capability,
    node: &NodeId,
    grant: &SignedMandateGrant,
    sign: impl FnOnce(&[u8]) -> Option<[u8; 64]>,
) -> Option<Capability> {
    let request = advertisement_request(node, &cap);
    let proof = sign(&super::grant::possession_message(&grant.mandate, &request))?;
    let json = serde_json::to_string(grant).ok()?;
    cap.attributes.insert(GRANT_ATTR.into(), CapValue::Text(json.into()));
    cap.attributes.insert(
        POSSESSION_ATTR.into(),
        CapValue::Text(base64::engine::general_purpose::STANDARD.encode(proof).into()),
    );
    Some(cap)
}

/// The namespaces a reader protects.
#[derive(Clone, Debug, Default)]
pub struct ProtectedNamespaces(BTreeSet<String>);

impl ProtectedNamespaces {
    /// Protect `namespaces`.
    pub fn new(namespaces: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self(namespaces.into_iter().map(Into::into).collect())
    }

    /// Is `namespace` protected?
    pub fn protects(&self, namespace: &str) -> bool {
        self.0.contains(namespace)
    }
}

/// Why a capability in a protected namespace did not resolve.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unbacked {
    /// No grant attribute.
    NoGrant,
    /// A grant or possession attribute that does not decode.
    Malformed,
    /// The grant names someone other than the advertiser as holder.
    HolderIsNotAdvertiser,
    /// The grant does not enumerate `serve:{namespace}/{name}`.
    OperationNotPermitted,
    /// P2's checks failed.
    Grant(GrantVerdict),
}

/// A capability a reader filtered out, and why — the observation to surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnbackedCapability {
    /// The advertiser.
    pub node: NodeId,
    /// The capability's namespace.
    pub namespace: String,
    /// The capability's name.
    pub name: String,
    /// Why it did not resolve.
    pub why: Unbacked,
}

/// **Filter resolved capabilities by authority** (Boundary H item H3).
///
/// Candidates in unprotected namespaces pass unchanged. Candidates in protected namespaces pass only
/// if backed; the rest are returned as observations. The router's order is preserved.
pub fn filter_protected(
    candidates: Vec<(NodeId, Capability)>,
    protected: &ProtectedNamespaces,
    verifier: &mut GrantVerifier,
    now_ms: u64,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> (Vec<(NodeId, Capability)>, Vec<UnbackedCapability>) {
    let mut kept = Vec::new();
    let mut unbacked = Vec::new();
    for (node, cap) in candidates {
        if !protected.protects(&cap.namespace) {
            kept.push((node, cap));
            continue;
        }
        match backing(&node, &cap, verifier, now_ms, members, external) {
            None => kept.push((node, cap)),
            Some(why) => unbacked.push(UnbackedCapability {
                namespace: cap.namespace.to_string(),
                name: cap.name.to_string(),
                node,
                why,
            }),
        }
    }
    (kept, unbacked)
}

/// `None` if backed; otherwise why not.
fn backing(
    node: &NodeId,
    cap: &Capability,
    verifier: &mut GrantVerifier,
    now_ms: u64,
    members: &impl MemberKeySource,
    external: &TrustedExternalIssuers,
) -> Option<Unbacked> {
    let text = |key: &str| match cap.attributes.get(key) {
        Some(CapValue::Text(t)) => Some(std::sync::Arc::clone(t)),
        _ => None,
    };
    let Some(grant_json) = text(GRANT_ATTR) else { return Some(Unbacked::NoGrant) };
    let Ok(grant) = serde_json::from_str::<SignedMandateGrant>(&grant_json) else {
        return Some(Unbacked::Malformed);
    };
    if grant.mandate.holder.as_str() != IssuerId::for_node(node).as_str() {
        return Some(Unbacked::HolderIsNotAdvertiser);
    }
    if !grant.mandate.permits(&serve_operation(cap)) {
        return Some(Unbacked::OperationNotPermitted);
    }
    let proof = match text(POSSESSION_ATTR)
        .map(|p| base64::engine::general_purpose::STANDARD.decode(p.as_bytes()))
    {
        Some(Ok(bytes)) => bytes,
        Some(Err(_)) => return Some(Unbacked::Malformed),
        None => return Some(Unbacked::Grant(GrantVerdict::WrongHolder)),
    };
    let verdict = verifier.check(
        &grant,
        &advertisement_request(node, cap),
        Some(&proof),
        now_ms,
        members,
        external,
    );
    (!verdict.is_valid()).then_some(Unbacked::Grant(verdict))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mandate::grant::tests::{grant, mandate, verifier, world, World};
    use ed25519_dalek::SigningKey;

    fn sign_with(key: &SigningKey) -> impl FnOnce(&[u8]) -> Option<[u8; 64]> + '_ {
        move |msg| Some(mycelium_core::tls::sign_bytes(key, msg))
    }

    fn backed(w: &World, ns: &str, name: &str, ops: &[&str]) -> (NodeId, Capability) {
        let g = grant(w, mandate(w, "cap/fleet", ops, 1));
        let cap = attach_grant(Capability::new(ns, name), &w.holder_node, &g, sign_with(&w.holder)).unwrap();
        (w.holder_node.clone(), cap)
    }

    fn run(w: &World, candidates: Vec<(NodeId, Capability)>) -> (Vec<(NodeId, Capability)>, Vec<UnbackedCapability>) {
        filter_protected(candidates, &ProtectedNamespaces::new(["tools"]), &mut verifier("cap/fleet"), 1_000, &w.members, &w.external)
    }

    /// A backed capability resolves; an unbacked one in the same protected namespace is filtered out
    /// and **reported** — the laundered power made visible.
    #[test]
    fn a_backed_capability_resolves_and_an_unbacked_one_is_filtered_and_reported() {
        let w = world();
        let good = backed(&w, "tools", "search", &["serve:tools/search"]);
        let laundered = (w.holder_node.clone(), Capability::new("tools", "artifactory-admin"));
        let (kept, unbacked) = run(&w, vec![good.clone(), laundered]);
        assert_eq!(kept, vec![good]);
        assert_eq!(unbacked.len(), 1);
        assert_eq!((unbacked[0].name.as_str(), &unbacked[0].why), ("artifactory-admin", &Unbacked::NoGrant));
    }

    /// Unprotected namespaces pass untouched, and the router's order is kept.
    #[test]
    fn unprotected_namespaces_pass_and_order_is_preserved() {
        let w = world();
        let a = (w.holder_node.clone(), Capability::new("misc", "a"));
        let b = backed(&w, "tools", "search", &["serve:tools/search"]);
        let c = (w.holder_node.clone(), Capability::new("misc", "c"));
        let (kept, unbacked) = run(&w, vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(kept, vec![a, b, c]);
        assert!(unbacked.is_empty());
    }

    /// **A grant for one capability does not back another.**
    #[test]
    fn a_grant_for_one_capability_does_not_back_another() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:tools/search"], 1));
        let other = attach_grant(Capability::new("tools", "admin"), &w.holder_node, &g, sign_with(&w.holder)).unwrap();
        let (_, unbacked) = run(&w, vec![(w.holder_node.clone(), other)]);
        assert_eq!(unbacked[0].why, Unbacked::OperationNotPermitted);
    }

    /// **A grant lifted onto another node's advertisement does not back it**: the holder is not the
    /// advertiser.
    #[test]
    fn a_grant_on_another_nodes_advertisement_does_not_back_it() {
        let w = world();
        let (_, cap) = backed(&w, "tools", "search", &["serve:tools/search"]);
        let thief = NodeId::new("127.0.0.1", 7999).unwrap();
        let (_, unbacked) = run(&w, vec![(thief, cap)]);
        assert_eq!(unbacked[0].why, Unbacked::HolderIsNotAdvertiser);
    }

    /// A possession proof by someone other than the holder is refused.
    #[test]
    fn a_proof_not_made_by_the_holder_is_refused() {
        let w = world();
        let g = grant(&w, mandate(&w, "cap/fleet", &["serve:tools/search"], 1));
        let impostor = SigningKey::from_bytes(&[77u8; 32]);
        let cap = attach_grant(Capability::new("tools", "search"), &w.holder_node, &g, sign_with(&impostor)).unwrap();
        let (_, unbacked) = run(&w, vec![(w.holder_node.clone(), cap)]);
        assert_eq!(unbacked[0].why, Unbacked::Grant(GrantVerdict::WrongHolder));
    }

    /// Expired, and non-entitled, grants are filtered with P2's own reason.
    #[test]
    fn expired_and_non_entitled_grants_are_filtered_with_their_reason() {
        let w = world();
        let (n, cap) = backed(&w, "tools", "search", &["serve:tools/search"]);
        let (_, late) = filter_protected(vec![(n.clone(), cap.clone())], &ProtectedNamespaces::new(["tools"]), &mut verifier("cap/fleet"), 50_000, &w.members, &w.external);
        assert_eq!(late[0].why, Unbacked::Grant(GrantVerdict::OutOfWindow));
        let (_, wrong) = filter_protected(vec![(n, cap)], &ProtectedNamespaces::new(["tools"]), &mut verifier("cap/other"), 1_000, &w.members, &w.external);
        assert_eq!(wrong[0].why, Unbacked::Grant(GrantVerdict::NotEntitled));
    }
}
