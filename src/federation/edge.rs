//! The federation edge — the transport's provider side (item 2 PR 8).
//!
//! PRs 1–7 built the contract and left one sentence standing at the end of every page: *no bytes
//! cross a network*. This module is the first half of retiring it. It decides nothing new; it
//! carries the decisions [`call`](super::call), [`catalog`](super::catalog) and the trust objects
//! already make across an HTTP hop, and it is deliberately thin so that the arguments stay where
//! they were made.
//!
//! # What crosses, and where it is verified
//!
//! Per D5 (`docs/design/federated-domains.md` §5) the call **is** A2A. A federated call is an
//! ordinary `tasks/send` on the partner's `/a2a`, with one addition: the request carries the
//! [`FederatedCaller`] credential and its signature in the [`HEADER_FEDERATED_CALL`] header, as
//! JSON ([`PresentedCall`]). The gateway's auth layer authenticates that header
//! ([`FederationEdge::authenticate`] — signature, lifetime, expiry, skew) before the handler runs,
//! and the handler authorises the export the body names ([`FederationEdge::authorize`] — the
//! export binding and the policy grant) before it dispatches. Two steps because the header is read
//! before the body; one credential because binding *what* to *who* is the whole point of PR 4.
//!
//! Discovery is the same shape with a reserved export: `GET /federation/catalog` carries a
//! credential for [`CATALOG_EXPORT`], and the reply is the *filtered* catalogue for that partner —
//! what it has been granted, never the full export list. A call credential cannot fetch a
//! catalogue and a catalogue credential cannot make a call: both are refused as `WrongExport`.
//!
//! # What this arm does not do
//!
//! It carries no bytes into the gossip medium. Nothing here writes a KV key, joins a peer table,
//! or answers anti-entropy — the two-mesh test in `lib_tests.rs` re-runs the PR 1 harness's
//! `assert_never_merged` *after* a call has crossed, which is when those assertions stop being
//! trivially true. The catalogue reply is not itself signed in this arm (the partner attributes it
//! to the gateway it asked, per PR 3's *attributable observation*); the choreography of the release
//! gate — lose a gateway, sever every link, change permissions mid-partition, reconnect — is the
//! next arm, on top of this one.

use super::call::{
    verify_federated_call, verify_federated_credential, AcceptedCall, CallPolicy, CallRefusal,
    FederatedCaller,
};
use super::catalog::filtered_catalog;
use super::{DomainId, DomainPolicy, TrustBundle};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// The request header a federated call rides in. Its value is [`PresentedCall`] as JSON.
pub const HEADER_FEDERATED_CALL: &str = "x-mycelium-federation-call";

/// The reserved export a catalogue fetch is credentialed for. Never a real export: a domain that
/// exports a service under this name has made a naming mistake, and the edge refuses to serve it.
pub const CATALOG_EXPORT: &str = "federation.catalog";

/// The route the provider serves its filtered catalogue on.
pub const CATALOG_PATH: &str = "/federation/catalog";

/// The wall clock in milliseconds, through the replay seam. Federation deadlines are wall-clock by
/// design — see the module docs of [`call`](super::call) for why the monotonic reasoning stops at
/// the domain edge.
pub fn now_ms() -> u64 {
    mycelium_core::sim_seam::wall_now_ms()
}

/// A [`FederatedCaller`] and its signature, in the form that crosses the wire.
///
/// The signature is the 64-byte Ed25519 signature over the credential's canonical bytes, standard
/// base64. The credential fields are carried verbatim — the verifier rebuilds the canonical form
/// from them, so a field changed in transit is a `BadSignature`, not a different call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresentedCall {
    pub origin: DomainId,
    pub principal: String,
    pub export: String,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub signature: String,
}

impl PresentedCall {
    /// Sign `credential` with the origin domain's key.
    pub fn sign(credential: &FederatedCaller, key: &ed25519_dalek::SigningKey) -> Self {
        let sig = mycelium_core::tls::sign_bytes(key, &credential.canonical_bytes());
        Self {
            origin: credential.origin_domain.clone(),
            principal: credential.principal.clone(),
            export: credential.export.clone(),
            issued_at_ms: credential.issued_at_ms,
            expires_at_ms: credential.expires_at_ms,
            signature: base64::engine::general_purpose::STANDARD.encode(sig),
        }
    }

    /// The credential as the verifier sees it.
    pub fn credential(&self) -> FederatedCaller {
        FederatedCaller {
            origin_domain: self.origin.clone(),
            principal: self.principal.clone(),
            export: self.export.clone(),
            issued_at_ms: self.issued_at_ms,
            expires_at_ms: self.expires_at_ms,
        }
    }

    /// The signature bytes. Undecodable base64 is a signature that cannot verify.
    pub fn signature_bytes(&self) -> Result<Vec<u8>, CallRefusal> {
        base64::engine::general_purpose::STANDARD
            .decode(&self.signature)
            .map_err(|_| CallRefusal::BadSignature)
    }

    /// The header value: compact JSON. A domain id and a principal are ASCII by construction of
    /// the wire form; a principal that is not is refused at parse time on the other side.
    pub fn to_header_value(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Parse a header value. `None` means the header was absent; a present but malformed header
    /// is an error, never anonymised — a client that tried to present a credential and failed
    /// must not be treated as one that presented nothing.
    pub fn from_header_value(value: Option<&str>) -> Result<Option<Self>, String> {
        let Some(raw) = value else { return Ok(None) };
        if !raw.is_ascii() {
            return Err("federation credential header is not ASCII".to_string());
        }
        serde_json::from_str::<Self>(raw)
            .map(Some)
            .map_err(|e| format!("federation credential header is malformed: {e}"))
    }
}

/// What the catalogue route answers: this domain's identity, the policy revision that filtered
/// the list, and the exports the asking partner has been granted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogReply {
    pub domain: DomainId,
    pub policy_revision: u64,
    pub exports: Vec<String>,
}

struct EdgeTrust {
    policy: DomainPolicy,
    bundle: TrustBundle,
}

/// The provider side of the federation edge: this domain's identity, what it exports, whom it
/// trusts and what it grants. One per node that serves federated calls; attached with
/// `GossipAgent::with_federation_edge`.
///
/// Policy and trust are the two things an operator changes at runtime (a revocation mid-partition
/// is PR 6's first-class transition), so they sit behind one lock; the exports and the call policy
/// are fixed at attach time.
pub struct FederationEdge {
    domain: DomainId,
    exports: Vec<String>,
    call_policy: CallPolicy,
    /// Lock-order row 38: leaf, µs read on every federated request, never held across an await.
    trust: RwLock<EdgeTrust>,
}

impl FederationEdge {
    pub fn new(
        domain: DomainId,
        exports: impl IntoIterator<Item = impl Into<String>>,
        policy: DomainPolicy,
        bundle: TrustBundle,
        call_policy: CallPolicy,
    ) -> Self {
        Self {
            domain,
            exports: exports.into_iter().map(Into::into).collect(),
            call_policy,
            trust: RwLock::new(EdgeTrust { policy, bundle }),
        }
    }

    pub fn domain(&self) -> &DomainId {
        &self.domain
    }

    pub fn exports(&self) -> &[String] {
        &self.exports
    }

    pub fn call_policy(&self) -> &CallPolicy {
        &self.call_policy
    }

    /// The policy revision in force.
    pub fn policy_revision(&self) -> u64 {
        self.trust.read().unwrap_or_else(|e| e.into_inner()).policy.revision
    }

    /// Replace the policy. Takes effect on the next request; a call already authorised under the
    /// old revision carries that revision in its `AcceptedCall`.
    pub fn set_policy(&self, policy: DomainPolicy) {
        self.trust.write().unwrap_or_else(|e| e.into_inner()).policy = policy;
    }

    /// Revoke a partner's identity. Every subsequent credential from it is `UnknownDomain`.
    pub fn revoke(&self, partner: &DomainId) {
        self.trust.write().unwrap_or_else(|e| e.into_inner()).bundle.revoke(partner);
    }

    /// Step one, at the auth layer: is this a partner we trust, speaking within a lifetime we
    /// honour? Binds no export and consults no policy.
    pub fn authenticate(&self, presented: &PresentedCall, now_ms: u64) -> Result<FederatedCaller, CallRefusal> {
        let credential = presented.credential();
        let signature = presented.signature_bytes()?;
        let trust = self.trust.read().unwrap_or_else(|e| e.into_inner());
        verify_federated_credential(&credential, &signature, &trust.bundle, &self.call_policy, now_ms)?;
        Ok(credential)
    }

    /// Step two, at the handler: does the credential name `requested_export`, and does policy
    /// grant it? Re-runs the identity checks — they are cheap, and a handler must not have to
    /// trust that a layer ran before it.
    pub fn authorize(
        &self,
        presented: &PresentedCall,
        requested_export: &str,
        now_ms: u64,
    ) -> Result<AcceptedCall, CallRefusal> {
        if requested_export == CATALOG_EXPORT {
            return Err(CallRefusal::WrongExport {
                authorised: presented.export.clone(),
                requested: requested_export.to_string(),
            });
        }
        let credential = presented.credential();
        let signature = presented.signature_bytes()?;
        let trust = self.trust.read().unwrap_or_else(|e| e.into_inner());
        verify_federated_call(
            &credential,
            &signature,
            requested_export,
            &trust.bundle,
            &trust.policy,
            &self.call_policy,
            now_ms,
        )
    }

    /// The catalogue for the partner presenting `presented`, which must be credentialed for
    /// [`CATALOG_EXPORT`]. Identity is enough to ask; what comes back is only what policy grants.
    pub fn catalog_for(&self, presented: &PresentedCall, now_ms: u64) -> Result<CatalogReply, CallRefusal> {
        if presented.export != CATALOG_EXPORT {
            return Err(CallRefusal::WrongExport {
                authorised: presented.export.clone(),
                requested: CATALOG_EXPORT.to_string(),
            });
        }
        let credential = self.authenticate(presented, now_ms)?;
        let trust = self.trust.read().unwrap_or_else(|e| e.into_inner());
        Ok(CatalogReply {
            domain: self.domain.clone(),
            policy_revision: trust.policy.revision,
            exports: filtered_catalog(&self.exports, &trust.policy, &credential.origin_domain),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair(seed: u8) -> (ed25519_dalek::SigningKey, [u8; 32]) {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        let vk = sk.verifying_key().to_bytes();
        (sk, vk)
    }

    fn edge(beta_key: [u8; 32]) -> FederationEdge {
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        FederationEdge::new(
            alpha.clone(),
            ["invoice.submit", "invoice.status", "ledger.audit"],
            DomainPolicy { domain: alpha, revision: 3, grants: vec![(beta.clone(), "invoice.submit".into())] },
            TrustBundle::trusting([(beta, beta_key)]),
            CallPolicy::default(),
        )
    }

    fn presented(sk: &ed25519_dalek::SigningKey, export: &str, now: u64) -> PresentedCall {
        PresentedCall::sign(
            &FederatedCaller {
                origin_domain: DomainId::new("beta.example").unwrap(),
                principal: "svc/billing".into(),
                export: export.into(),
                issued_at_ms: now,
                expires_at_ms: now + 60_000,
            },
            sk,
        )
    }

    /// The header form round-trips byte-for-byte, and a field changed in transit is a
    /// `BadSignature` — the verifier rebuilds the canonical form from the carried fields.
    #[test]
    fn the_header_form_round_trips_and_a_changed_field_breaks_the_signature() {
        let (sk, vk) = keypair(7);
        let now = 1_789_000_000_000;
        let p = presented(&sk, "invoice.submit", now);
        let back = PresentedCall::from_header_value(Some(&p.to_header_value())).unwrap().unwrap();
        assert_eq!(back, p);
        assert!(edge(vk).authenticate(&back, now + 1).is_ok());

        let mut tampered = back.clone();
        tampered.principal = "svc/other".into();
        assert_eq!(edge(vk).authenticate(&tampered, now + 1), Err(CallRefusal::BadSignature));

        assert_eq!(PresentedCall::from_header_value(None).unwrap(), None);
        assert!(PresentedCall::from_header_value(Some("{not json")).is_err(), "malformed is an error, never anonymous");
    }

    /// Authentication and authorisation are two steps that agree: an identity that passes step
    /// one is refused at step two for an export it did not name or is not granted, and a
    /// catalogue credential cannot make a call.
    #[test]
    fn authenticate_binds_nothing_and_authorize_binds_export_and_grant() {
        let (sk, vk) = keypair(7);
        let e = edge(vk);
        let now = 1_789_000_000_000;

        let call = presented(&sk, "invoice.submit", now);
        assert!(e.authenticate(&call, now).is_ok());
        let accepted = e.authorize(&call, "invoice.submit", now).unwrap();
        assert_eq!(accepted.policy_revision, 3);
        assert_eq!(accepted.principal, "svc/billing");
        assert!(matches!(e.authorize(&call, "invoice.status", now), Err(CallRefusal::WrongExport { .. })));

        // Named but not granted: `ledger.audit` is exported, beta has no grant for it.
        let ungranted = presented(&sk, "ledger.audit", now);
        assert!(e.authenticate(&ungranted, now).is_ok(), "identity is not a grant");
        assert_eq!(e.authorize(&ungranted, "ledger.audit", now), Err(CallRefusal::NotPermitted));

        // The reserved export is not a call, whichever side names it.
        let cat = presented(&sk, CATALOG_EXPORT, now);
        assert!(matches!(e.authorize(&cat, CATALOG_EXPORT, now), Err(CallRefusal::WrongExport { .. })));
        assert!(matches!(e.authorize(&cat, "invoice.submit", now), Err(CallRefusal::WrongExport { .. })));
    }

    /// The catalogue is filtered to the grant, needs the reserved export, and follows a policy
    /// change and a revocation without a restart.
    #[test]
    fn the_catalogue_is_the_grant_and_tracks_policy_and_revocation() {
        let (sk, vk) = keypair(7);
        let e = edge(vk);
        let now = 1_789_000_000_000;
        let beta = DomainId::new("beta.example").unwrap();

        let cat = presented(&sk, CATALOG_EXPORT, now);
        let reply = e.catalog_for(&cat, now).unwrap();
        assert_eq!(reply.exports, vec!["invoice.submit".to_string()]);
        assert_eq!(reply.policy_revision, 3);

        // A call credential cannot ask for the catalogue.
        let call = presented(&sk, "invoice.submit", now);
        assert!(matches!(e.catalog_for(&call, now), Err(CallRefusal::WrongExport { .. })));

        // Policy revision 4 grants a second export; the next fetch sees it.
        e.set_policy(DomainPolicy {
            domain: e.domain().clone(),
            revision: 4,
            grants: vec![(beta.clone(), "invoice.submit".into()), (beta.clone(), "invoice.status".into())],
        });
        let reply = e.catalog_for(&cat, now).unwrap();
        assert_eq!(reply.exports, vec!["invoice.submit".to_string(), "invoice.status".to_string()]);
        assert_eq!(reply.policy_revision, 4);

        // Revoked: the partner is unknown to every path.
        e.revoke(&beta);
        assert_eq!(e.catalog_for(&cat, now), Err(CallRefusal::UnknownDomain));
        assert_eq!(e.authorize(&call, "invoice.submit", now), Err(CallRefusal::UnknownDomain));
    }

    /// A key the bundle does not trust signs a well-formed credential: refused as
    /// `UnknownDomain` for an unlisted domain, `BadSignature` for a listed domain under the wrong
    /// key. Both before any export or grant is consulted.
    #[test]
    fn an_untrusted_key_is_refused_before_anything_else() {
        let (_, vk) = keypair(7);
        let (wrong, _) = keypair(9);
        let e = edge(vk);
        let now = 1_789_000_000_000;
        let p = presented(&wrong, "invoice.submit", now);
        assert_eq!(e.authenticate(&p, now), Err(CallRefusal::BadSignature));

        let gamma = PresentedCall::sign(
            &FederatedCaller {
                origin_domain: DomainId::new("gamma.example").unwrap(),
                principal: "x".into(),
                export: "invoice.submit".into(),
                issued_at_ms: now,
                expires_at_ms: now + 1_000,
            },
            &wrong,
        );
        assert_eq!(e.authenticate(&gamma, now), Err(CallRefusal::UnknownDomain));
    }
}
