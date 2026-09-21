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

/// Domain-separation tag for a signed [`CatalogReply`]. Distinct from the descriptor, policy and
/// call tags, so a catalogue signature can never authenticate another object type.
pub const TAG_CATALOG: &str = "mycelium.federation/catalog/1";

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
        let call = serde_json::from_str::<Self>(raw)
            .map_err(|e| format!("federation credential header is malformed: {e}"))?;

        // **The ASCII rule is about the credential's content, not its encoding.**
        //
        // The check above reads the raw header, and JSON can spell any character in pure ASCII:
        // `"\u0809"` is an ASCII header carrying a non-ASCII principal. So the gate passed an
        // escaped field straight through, and [`Self::to_header_value`]'s promise that "a principal
        // that is not [ASCII] is refused at parse time" was not kept by anything.
        //
        // Two consequences, and the second is why it is a defect rather than a nicety. An identity
        // string that may hold arbitrary Unicode admits confusables — a `principal` that renders
        // like another one. And the accept-set stopped matching the emit-set: `to_header_value`
        // re-emits the character **unescaped**, so a credential this node accepted it could not
        // itself re-parse, and a peer forwarding it would refuse what we had just allowed.
        //
        // `origin` needs no check: `DomainId` is already restricted to `[a-z0-9.-]` at
        // construction, and its `Deserialize` routes through that constructor.
        //
        // Found by §12.6's `presented_call` fuzz target — the round-trip assertion, not the parse.
        for (field, text) in [
            ("principal", call.principal.as_str()),
            ("export", call.export.as_str()),
            ("signature", call.signature.as_str()),
        ] {
            if !text.is_ascii() {
                return Err(format!("federation credential {field} is not ASCII"));
            }
        }

        Ok(Some(call))
    }
}

/// What the catalogue route answers: this domain's identity, **whom the list was filtered for**,
/// the policy revision that filtered it, when it was issued, and the exports that partner has
/// been granted — signed under the domain's key when the edge has one (item 2 PR 10a).
///
/// The signature covers `for_partner`, so a reply issued to one partner cannot be replayed to
/// another as its catalogue; and it covers `domain`, so a gateway cannot answer for a domain it
/// does not hold the key of. Freshness is not the signature's job — the resolver's observation
/// window (PR 3) decides how long a catalogue may be relied on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogReply {
    pub domain: DomainId,
    pub for_partner: DomainId,
    pub policy_revision: u64,
    pub issued_at_ms: u64,
    pub exports: Vec<String>,
    /// Standard base64 of the Ed25519 signature over [`canonical_bytes`](Self::canonical_bytes),
    /// or `None` from an edge with no signing key.
    #[serde(default)]
    pub signature: Option<String>,
}

/// Why a catalogue reply was not accepted by a client that requires a signed one.
///
/// `#[non_exhaustive]`: a later rung may find another way a reply is unacceptable, and a consumer
/// that matched exhaustively would then fail to compile on an upgrade that only *added* a refusal.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogRefusal {
    /// The reply carries no signature and the client requires one.
    Unsigned,
    /// The signature does not verify under the partner's key (or is not decodable).
    BadSignature,
    /// The reply names a domain other than the partner asked.
    WrongDomain { expected: DomainId, got: DomainId },
    /// The reply was filtered for someone else.
    NotForUs { expected: DomainId, got: DomainId },
    /// The reply carries an **older** policy revision than one this client has already seen.
    ///
    /// `DomainPolicy::revision` is documented as monotonic — *a consumer that has seen a higher
    /// revision must not accept a lower one* — and that rule was stated and implemented nowhere. A
    /// signed reply carries no expiry and no nonce, so a reply captured under revision 3 (which
    /// granted an export) still verifies after the operator withdraws that grant at revision 4.
    /// Replayed, it became a fresh observation and the withdrawn export reappeared in the client's
    /// view. Found by the Phase-C adversarial audit (items 1+2+7).
    ///
    /// **Bounded, and worth saying:** the catalogue confers *visibility*, not authority. The
    /// provider re-authorises at call time against its live policy, so a replay ended in a refusal
    /// at the edge rather than an unauthorised invocation. The damage was a client acting on a view
    /// of the partner it was told it would never have.
    StaleRevision {
        /// The highest revision this client has accepted from this partner.
        seen: u64,
        /// What the replayed reply offered.
        offered: u64,
    },
}

impl std::fmt::Display for CatalogRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleRevision { seen, offered } => write!(
                f,
                "catalogue offers policy revision {offered} but revision {seen} has already been \
                 accepted; a revision never goes backwards"
            ),
            Self::Unsigned => write!(f, "catalogue is unsigned and a signed one is required"),
            Self::BadSignature => write!(f, "catalogue signature does not verify under the partner's key"),
            Self::WrongDomain { expected, got } => write!(f, "catalogue is for domain {got}, expected {expected}"),
            Self::NotForUs { expected, got } => write!(f, "catalogue was filtered for {got}, not for {expected}"),
        }
    }
}

impl std::error::Error for CatalogRefusal {}

impl CatalogReply {
    /// Length-prefixed, tagged, in field order — the same shape as the descriptor and policy
    /// forms in the parent module, and for the same reason: we own both ends.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn put_lp(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        put_lp(&mut out, TAG_CATALOG.as_bytes());
        put_lp(&mut out, self.domain.as_str().as_bytes());
        put_lp(&mut out, self.for_partner.as_str().as_bytes());
        out.extend_from_slice(&self.policy_revision.to_le_bytes());
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&(self.exports.len() as u32).to_le_bytes());
        for e in &self.exports {
            put_lp(&mut out, e.as_bytes());
        }
        out
    }

    /// Sign under the domain's key.
    pub fn signed(mut self, key: &ed25519_dalek::SigningKey) -> Self {
        let sig = mycelium_core::tls::sign_bytes(key, &self.canonical_bytes());
        self.signature = Some(base64::engine::general_purpose::STANDARD.encode(sig));
        self
    }

    /// Is this the catalogue `partner` issued to `us`, signed under `partner_key`? Checks the
    /// bindings before the signature so a refusal names the cheaper reason first.
    pub fn verify(&self, partner: &DomainId, us: &DomainId, partner_key: &[u8; 32]) -> Result<(), CatalogRefusal> {
        if &self.domain != partner {
            return Err(CatalogRefusal::WrongDomain { expected: partner.clone(), got: self.domain.clone() });
        }
        if &self.for_partner != us {
            return Err(CatalogRefusal::NotForUs { expected: us.clone(), got: self.for_partner.clone() });
        }
        let Some(sig) = &self.signature else { return Err(CatalogRefusal::Unsigned) };
        let sig = base64::engine::general_purpose::STANDARD.decode(sig).map_err(|_| CatalogRefusal::BadSignature)?;
        if !mycelium_core::tls::verify_bytes(partner_key, &self.canonical_bytes(), &sig) {
            return Err(CatalogRefusal::BadSignature);
        }
        Ok(())
    }
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
    /// The domain's signing key — the one whose public half partners hold in their trust bundles
    /// (the descriptor's `public_key`). Present: catalogue replies are signed. Absent: unsigned,
    /// which a partner that requires signatures refuses.
    signing_key: Option<ed25519_dalek::SigningKey>,
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
            signing_key: None,
            trust: RwLock::new(EdgeTrust { policy, bundle }),
        }
    }

    /// Sign catalogue replies under the domain's key (item 2 PR 10a). Partners verify against the
    /// public key their trust bundle holds for this domain.
    pub fn with_signing_key(mut self, key: ed25519_dalek::SigningKey) -> Self {
        self.signing_key = Some(key);
        self
    }

    /// Does this edge sign its catalogue replies?
    pub fn signs_catalogue(&self) -> bool {
        self.signing_key.is_some()
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
        let reply = CatalogReply {
            domain: self.domain.clone(),
            for_partner: credential.origin_domain.clone(),
            policy_revision: trust.policy.revision,
            issued_at_ms: now_ms,
            exports: filtered_catalog(&self.exports, &trust.policy, &credential.origin_domain),
            signature: None,
        };
        Ok(match &self.signing_key {
            Some(key) => reply.signed(key),
            None => reply,
        })
    }
}

#[cfg(test)]
mod tests {
    /// **A credential this node accepts, it can re-parse.** Regression for the defect §12.6's
    /// `presented_call` fuzz target found on main, 2026-09-21.
    ///
    /// The header gate checked `raw.is_ascii()` on the *encoding*. JSON spells any character in
    /// pure ASCII, so `"\u0809"` is an ASCII header carrying a non-ASCII field — it passed, and
    /// `to_header_value` then re-emitted the character **unescaped**, producing a header this same
    /// parser rejects. The accept-set and the emit-set had come apart.
    ///
    /// Both directions matter, so both are asserted: the escaped credential is refused by name,
    /// and a well-formed one still round-trips.
    #[test]
    fn a_credential_is_ascii_in_its_content_not_merely_its_encoding() {
        use super::PresentedCall;

        // `\u0809` is written here as an ASCII escape, exactly as a partner would send it.
        let escaped = r#"{"origin":"partner.example","principal":"oidc:idp/al\u0809ice","export":"depot.read","issued_at_ms":0,"expires_at_ms":1,"signature":"AAAA"}"#;
        assert!(escaped.is_ascii(), "the header itself is ASCII — that was the whole trap");

        let refused = PresentedCall::from_header_value(Some(escaped));
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("principal") && e.contains("not ASCII")),
            "a non-ASCII principal must be refused by name, got {refused:?}",
        );

        // And the honest case still works, encoding and content both ASCII.
        let plain = r#"{"origin":"partner.example","principal":"oidc:idp/alice","export":"depot.read","issued_at_ms":0,"expires_at_ms":1,"signature":"AAAA"}"#;
        let call = PresentedCall::from_header_value(Some(plain))
            .expect("a well-formed credential parses")
            .expect("present");
        let again = PresentedCall::from_header_value(Some(&call.to_header_value()));
        assert_eq!(
            Ok(Some(call)),
            again,
            "what this node emits, it must be able to read back",
        );
    }

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

    /// A signed catalogue verifies under the domain's key and is bound to the domain and the
    /// partner it was filtered for; a changed export, another partner, another domain and the
    /// wrong key are each refused for their own reason; an unkeyed edge answers unsigned, which a
    /// requiring client refuses as `Unsigned` (item 2 PR 10a).
    #[test]
    fn a_signed_catalogue_is_bound_to_the_domain_and_the_asker() {
        let (beta_sk, beta_vk) = keypair(7);
        let (alpha_sk, alpha_vk) = keypair(8);
        let (_, other_vk) = keypair(9);
        let now = 1_789_000_000_000;
        let alpha = DomainId::new("alpha.example").unwrap();
        let beta = DomainId::new("beta.example").unwrap();
        let gamma = DomainId::new("gamma.example").unwrap();

        let unkeyed = edge(beta_vk);
        let reply = unkeyed.catalog_for(&presented(&beta_sk, CATALOG_EXPORT, now), now).unwrap();
        assert_eq!(reply.signature, None);
        assert_eq!(reply.for_partner, beta);
        assert_eq!(reply.issued_at_ms, now);
        assert_eq!(reply.verify(&alpha, &beta, &alpha_vk), Err(CatalogRefusal::Unsigned));

        let keyed = edge(beta_vk).with_signing_key(alpha_sk);
        assert!(keyed.signs_catalogue());
        let reply = keyed.catalog_for(&presented(&beta_sk, CATALOG_EXPORT, now), now).unwrap();
        assert!(reply.signature.is_some());
        assert_eq!(reply.verify(&alpha, &beta, &alpha_vk), Ok(()));
        assert_eq!(reply.verify(&alpha, &beta, &other_vk), Err(CatalogRefusal::BadSignature));
        assert!(matches!(reply.verify(&gamma, &beta, &alpha_vk), Err(CatalogRefusal::WrongDomain { .. })));
        assert!(matches!(reply.verify(&alpha, &gamma, &alpha_vk), Err(CatalogRefusal::NotForUs { .. })));

        let mut tampered = reply.clone();
        tampered.exports.push("ledger.audit".into());
        assert_eq!(tampered.verify(&alpha, &beta, &alpha_vk), Err(CatalogRefusal::BadSignature));
        let mut readdressed = reply.clone();
        readdressed.for_partner = gamma.clone();
        assert!(matches!(readdressed.verify(&alpha, &gamma, &alpha_vk), Err(CatalogRefusal::BadSignature)), "re-addressing a reply breaks its signature");

        // The tag keeps a catalogue signature from authenticating anything else.
        assert_ne!(TAG_CATALOG, super::super::TAG_DESCRIPTOR);
        assert_ne!(TAG_CATALOG, super::super::TAG_POLICY);
        assert_ne!(TAG_CATALOG, super::super::call::TAG_CALL);
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
