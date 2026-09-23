//! The authenticated federation call, and the provider adapter (item 2 PR 4).
//!
//! §5 of `docs/design/federated-domains.md` decided D5: **the federation call *is* A2A**, with
//! domain-bound origin credentials. This module is that credential and its verification — the part
//! that has to be right before any transport carries it.
//!
//! # The confused deputy, one boundary further out
//!
//! Item 7 fixed this inside a domain: a gateway used to dispatch under the *node's* identity, so a
//! provider's `authorized_callers` saw the gateway and never the client. [`FederatedCaller`] is the
//! same fix across a domain boundary, and it is needed more here, because the caller is by
//! construction not ours.
//!
//! The credential therefore **names the export it authorises**. A credential minted for
//! `invoice.status` cannot invoke `invoice.submit`, even from the same partner, in the same second,
//! over the same connection. Binding only the *caller* would leave the deputy confused about
//! *what*, having fixed *who*.
//!
//! # Why expiry here is wall-clock, when everything else in this crate is monotonic
//!
//! The replay seams deliberately push intervals onto a monotonic clock, because a monotonic clock
//! cannot jump backwards. **That reasoning does not cross a domain boundary**: two domains share no
//! monotonic origin, so "expires at monotonic 41s" is meaningless to a reader in another process on
//! another machine. A cross-domain deadline has to be stated on the one clock both sides can name,
//! which is the wall clock — and the cost of that is skew, handled explicitly by
//! [`CallPolicy::skew_tolerance`] rather than assumed away.
//!
//! # The verifier bounds the lifetime, not the issuer
//!
//! A credential says when it expires. If the verifier simply believed that, a partner could mint a
//! credential valid for a century and §10's *"issued authority lasts only to its expiry"* would be a
//! promise the issuer makes to itself. [`CallPolicy::max_lifetime`] is the receiving domain's own
//! ceiling, applied to every credential regardless of what it claims.

use super::DomainId;
// Only the verifier uses these, and the verifier needs the signature machinery — so without `tls`
// they would be dead imports. The `--no-default-features` clippy in `make check` is the gate that
// catches this class (CLAUDE.md's feature-gated dead-code trap), and it caught exactly this.
#[cfg(feature = "tls")]
use super::{DomainPolicy, TrustBundle};
use std::time::Duration;

/// Domain-separation tag for [`FederatedCaller`] signatures.
///
/// Distinct from the descriptor and policy tags, so a signature over one object type can never
/// authenticate another — see the module docs in the parent.
pub const TAG_CALL: &str = "mycelium.federation/call/1";

/// A credential binding *this call* to the domain and principal making it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FederatedCaller {
    /// The domain vouching for the caller.
    pub origin_domain: DomainId,
    /// Who, within that domain. Opaque to us — their namespace, not ours.
    pub principal: String,
    /// **The export this credential authorises, and only this one.**
    pub export: String,
    /// Epoch milliseconds when it was minted.
    pub issued_at_ms: u64,
    /// Epoch milliseconds after which it is worthless.
    pub expires_at_ms: u64,
    /// **sha256 of the request body this credential authorises.**
    ///
    /// Without it the signature proves *who is asking and for which export* and says nothing about
    /// **what they asked**. An on-path attacker between two domains could rewrite the payload,
    /// leave the credential header untouched, and the receiving gateway would accept the altered
    /// call as authentic — then make and record an authorisation decision about the attacker's
    /// text. The same reasoning as `ActionEnvelope::arguments_digest` one layer up, which binds
    /// arguments to a decision; this binds the body to the caller.
    ///
    /// `None` is a credential from a partner that predates the binding. It is **not** the same as
    /// a match, and a verifier that treats it as one gets no protection at all — an attacker would
    /// simply strip the field. See [`CallPolicy::require_body_binding`].
    pub body_sha256: Option<[u8; 32]>,
}

/// What the **receiving** domain will accept, independent of what a credential claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallPolicy {
    /// The longest validity window this domain honours, however long the credential claims.
    pub max_lifetime: Duration,
    /// How far into the future an `issued_at_ms` may sit before it is treated as wrong rather than
    /// skewed. Cross-domain deadlines are wall-clock (see the module docs), so some tolerance is
    /// required; unbounded tolerance would make `issued_at` decorative.
    pub skew_tolerance: Duration,
    /// Refuse a credential that does not bind the request body.
    ///
    /// **Default `false`, and that default is a rolling-upgrade window, not a recommendation.**
    /// A partner that predates the binding sends no digest; refusing it outright would break every
    /// existing federation on upgrade, so the receiving domain chooses when to require it.
    ///
    /// What `false` costs, stated plainly: an attacker on the path between two domains can rewrite
    /// a call's payload and **strip** the binding, and this verifier will accept it. Absence is
    /// indistinguishable from a partner that has not upgraded, which is exactly what makes a
    /// downgrade free. So `false` provides **no integrity guarantee against an active attacker** —
    /// it is worth having only against a partner that has upgraded and is not being tampered with,
    /// and it is worth turning on as soon as every partner has.
    pub require_body_binding: bool,
    /// **The most calls from one partner this gateway will carry at once.** `0` (the default) is
    /// unlimited, which is what every deployment had before this field existed.
    ///
    /// The consumer side has metered per-partner slots since PR 5 ([`GatewayPool`]): it bounds what
    /// *we send* a partner. Nothing bounded what a partner sends *us* — the Phase-C audit's last
    /// open finding — so this is that counter, on the other side of the same edge, and deliberately
    /// the same shape: fixed slots per partner, refused rather than queued, so one partner
    /// saturating its allowance cannot consume another's.
    ///
    /// **What it does not do, and cannot.** The count is *this gateway's*. A domain that runs N
    /// gateways admits up to **N × this** from one partner in aggregate, because there is no
    /// cross-gateway counter — by design: a shared one would mean either a coordinator or partner
    /// identifiers gossiped through `sys/`, and D7 says foreign state does not enter the medium.
    /// If you need an aggregate bound, divide it by the number of gateways you run and accept that
    /// a gateway which is down leaves its share unused.
    ///
    /// It also bounds *concurrency*, not rate: a partner making brief calls in a tight loop stays
    /// under any in-flight cap. Rate is the operator's ingress, and `mycelium-core`'s `rate` module
    /// is the intra-mesh answer to the same question.
    pub max_in_flight_per_partner: usize,
}

impl Default for CallPolicy {
    fn default() -> Self {
        Self {
            max_lifetime: Duration::from_secs(300),
            skew_tolerance: Duration::from_secs(30),
            require_body_binding: false,
            max_in_flight_per_partner: 0,
        }
    }
}

/// Why a federated call was refused.
///
/// Each variant is a different thing for an operator to do, which is why they are not one
/// `Refused`. In particular **`BadSignature` and `NotPermitted` must never merge**: the first says
/// someone is forging, the second says a partner asked for something we chose not to grant. Reading
/// a forgery as a policy gap sends the operator to edit the wrong file.
/// `#[non_exhaustive]` since the release that added [`CallRefusal::AtCapacity`]. A new refusal here
/// is, as with [`ClientError`](crate::federation::client::ClientError), a refusal that already
/// existed and was being reported as something less precise — so more are expected, and a `_` arm
/// now means the next one is not a breaking change. **That arm must fail closed**: an unrecognised
/// refusal is *this call was not authorised for a reason this code does not know*, never *it is
/// fine*.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallRefusal {
    /// The origin domain is not in this operator's trust bundle.
    UnknownDomain,
    /// The credential carries no body binding and this domain requires one
    /// ([`CallPolicy::require_body_binding`]).
    ///
    /// Distinct from [`CallRefusal::BodyMismatch`] on purpose: *not bound* is a partner that has
    /// not upgraded (or an attacker who stripped the field), while *mismatch* is a body that was
    /// changed under a binding. The operator response differs — upgrade the partner, versus
    /// someone is on the path.
    BodyNotBound,
    /// The credential binds a body digest and the body received is not that body.
    ///
    /// There is no benign reading of this. The credential is authentic — it verified under a key
    /// this domain chose to trust — and it authorises a different payload than the one that
    /// arrived, so the payload was changed after the partner signed for it.
    BodyMismatch,
    /// The signature does not verify under the key the bundle trusts for that domain.
    BadSignature,
    /// The credential authorises a different export than the one being invoked.
    WrongExport { authorised: String, requested: String },
    /// Past its stated expiry.
    Expired { expires_at_ms: u64, now_ms: u64 },
    /// Minted further in the future than skew tolerance allows.
    NotYetValid { issued_at_ms: u64, now_ms: u64 },
    /// Its validity window is longer than this domain will honour, whatever it claims.
    LifetimeTooLong { claimed: Duration, max: Duration },
    /// Authentic, current, correctly bound — and not something our policy grants.
    NotPermitted,
    /// This gateway is already carrying [`CallPolicy::max_in_flight_per_partner`] calls for that
    /// partner. **Refused, not queued** — the same choice the consumer side's `NoCapacity` makes.
    ///
    /// Distinct from [`CallRefusal::NotPermitted`] and it must stay so: *not permitted* is a
    /// standing answer about authority that a retry will not change, while this is a transient
    /// answer about load that a retry probably will. Merging them would tell a partner to go and
    /// ask their operator about a grant they already have.
    AtCapacity { partner: DomainId, limit: usize },
}

impl std::fmt::Display for CallRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDomain => write!(f, "origin domain is not in the trust bundle"),
            Self::BodyNotBound => {
                write!(f, "the credential does not bind the request body and this domain requires it")
            }
            Self::BodyMismatch => write!(
                f,
                "the credential binds a different request body than the one received — the payload \
                 was changed after it was signed"
            ),
            Self::BadSignature => write!(f, "signature does not verify under the trusted key"),
            Self::WrongExport { authorised, requested } =>
                write!(f, "credential authorises {authorised:?}, not {requested:?}"),
            Self::Expired { expires_at_ms, now_ms } =>
                write!(f, "expired at {expires_at_ms} (now {now_ms})"),
            Self::NotYetValid { issued_at_ms, now_ms } =>
                write!(f, "issued at {issued_at_ms}, which is ahead of now ({now_ms}) beyond tolerance"),
            Self::LifetimeTooLong { claimed, max } =>
                write!(f, "claims a {claimed:?} lifetime; this domain honours at most {max:?}"),
            Self::NotPermitted => write!(f, "authenticated, but policy does not grant this export"),
            Self::AtCapacity { partner, limit } =>
                write!(f, "this gateway is already carrying {limit} calls for {partner}; refused, not queued"),
        }
    }
}

impl std::error::Error for CallRefusal {}

/// A call that passed every check — **with its origin preserved**.
///
/// This is the provider adapter's whole point. The provider is handed *who actually asked*, across
/// the boundary, rather than "the gateway called me". A provider that only ever sees the gateway
/// cannot make an authorisation decision of its own, and cannot say afterwards who it acted for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedCall {
    /// The domain that vouched for the caller.
    pub origin_domain: DomainId,
    /// The principal within that domain, preserved verbatim.
    pub principal: String,
    /// The export being invoked.
    pub export: String,
    /// Which policy revision permitted it — so *"under which rules"* has an answer afterwards.
    pub policy_revision: u64,
}

impl FederatedCaller {
    /// The exact bytes a credential signature is taken over.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn put_lp(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        put_lp(&mut out, TAG_CALL.as_bytes());
        put_lp(&mut out, self.origin_domain.as_str().as_bytes());
        put_lp(&mut out, self.principal.as_bytes());
        put_lp(&mut out, self.export.as_bytes());
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&self.expires_at_ms.to_le_bytes());
        // A presence byte. **It is not load-bearing today, and saying so is the point** — a plant
        // that removed it broke no test, which is how the original note here was found to be
        // wrong. That note claimed an attacker could otherwise strip `Some([0; 32])` down to
        // `None`; they cannot, because the digest is fixed-length and last, so removing it changes
        // the byte length and the signature fails either way.
        //
        // It is kept because that argument depends on *being last*. The day any field is appended
        // after this one, an absent digest and a present one would no longer be distinguishable by
        // length alone, and the ambiguity would be silent. One byte now is cheaper than noticing
        // then.
        match &self.body_sha256 {
            None => out.push(0),
            Some(d) => {
                out.push(1);
                out.extend_from_slice(d);
            }
        }
        out
    }

    /// The digest this credential should carry for `body`.
    ///
    /// Over the **bytes as received**, never a re-serialisation: `{"a":1,"b":2}` and
    /// `{"b":2,"a":1}` are the same JSON and different bytes, so digesting a parsed-then-re-encoded
    /// body would compare our serialiser against theirs and fail for honest partners while an
    /// attacker who matched our encoder would pass. Same rule as the exporter's *verify the
    /// received bytes, then parse*.
    pub fn digest_of(body: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(body).into()
    }

    /// The window this credential claims. Used by the verifier, which needs `tls`.
    #[cfg(feature = "tls")]
    fn claimed_lifetime(&self) -> Duration {
        Duration::from_millis(self.expires_at_ms.saturating_sub(self.issued_at_ms))
    }
}

/// Verify a federated call and, if it passes, hand back the origin for the provider.
///
/// **The order is the design.** Each step answers one question, and each is refused separately:
///
/// 1. **who** — is this domain one we chose to trust? (the bundle, not the credential)
/// 2. **authentic** — does it verify under *that* key?
/// 3. **what** — does the credential authorise the export actually being invoked?
/// 4. **when** — is it inside a window we will honour, by our clock and our ceiling?
/// 5. **may they** — does our policy grant it?
///
/// Authentication only: the credential is signed by the partner's trusted key and is inside its
/// lifetime (item 2 PR 8). **This proves who asked and nothing else** — it binds no export and
/// consults no policy. It exists because the transport learns *who* before it learns *what*: the
/// gateway's auth layer reads the credential from a header while the export is still in the
/// request body, so it authenticates here and defers to [`verify_federated_call`] once it has the
/// requested export in hand. An identity that passes this and is then used for an export it does
/// not name is refused there, not admitted here.
///
/// The checks and their order are the same ones [`verify_federated_call`] runs, minus the export
/// binding and the policy grant; the refusals mean the same thing.
#[cfg(feature = "tls")]
pub fn verify_federated_credential(
    credential: &FederatedCaller,
    signature: &[u8],
    bundle: &TrustBundle,
    call_policy: &CallPolicy,
    now_ms: u64,
) -> Result<(), CallRefusal> {
    // Every key acceptable *now*, not just the current one. A rotation is deliberately overlapping —
    // both keys verify for a bounded window so a partner can move to the new one at its own pace —
    // and consulting only `key_for` made that window unreachable on the call path: a partner still
    // signing with the retiring key was refused as `BadSignature`, which this module's own rule says
    // must mean *someone is forging*. Accusing a partner of forgery for doing exactly what the
    // rotation design tells it to do sends the operator to the wrong file.
    // Found by the Phase-C adversarial audit (items 1+2+7).
    let keys = bundle.acceptable_keys(&credential.origin_domain, now_ms);
    if keys.is_empty() {
        return Err(CallRefusal::UnknownDomain);
    }
    let bytes = credential.canonical_bytes();
    if !keys.iter().any(|k| mycelium_core::tls::verify_bytes(k, &bytes, signature)) {
        return Err(CallRefusal::BadSignature);
    }
    let claimed = credential.claimed_lifetime();
    if claimed > call_policy.max_lifetime {
        return Err(CallRefusal::LifetimeTooLong { claimed, max: call_policy.max_lifetime });
    }
    if now_ms > credential.expires_at_ms {
        return Err(CallRefusal::Expired { expires_at_ms: credential.expires_at_ms, now_ms });
    }
    let skew = call_policy.skew_tolerance.as_millis() as u64;
    if credential.issued_at_ms > now_ms.saturating_add(skew) {
        return Err(CallRefusal::NotYetValid { issued_at_ms: credential.issued_at_ms, now_ms });
    }
    Ok(())
}

/// Authentication strictly before authorisation, and both strictly before the call. A partner that
/// authenticates has proved who it is and nothing else; step 5 is where this domain decides.
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)]
pub fn verify_federated_call(
    credential: &FederatedCaller,
    signature: &[u8],
    requested_export: &str,
    body: &[u8],
    bundle: &TrustBundle,
    policy: &DomainPolicy,
    call_policy: &CallPolicy,
    now_ms: u64,
) -> Result<AcceptedCall, CallRefusal> {
    // 1. who — the bundle decides which key, never the credential.
    // See `verify_federated_credential`: the rotation overlap is honoured here too, so the two
    // entry points cannot disagree about which keys are acceptable.
    let keys = bundle.acceptable_keys(&credential.origin_domain, now_ms);
    if keys.is_empty() {
        return Err(CallRefusal::UnknownDomain);
    }

    // 2. authentic
    let bytes = credential.canonical_bytes();
    if !keys.iter().any(|k| mycelium_core::tls::verify_bytes(k, &bytes, signature)) {
        return Err(CallRefusal::BadSignature);
    }

    // 3. what — checked before time, so a mis-bound credential is reported as mis-bound rather
    //    than as whatever its clock happens to say.
    if credential.export != requested_export {
        return Err(CallRefusal::WrongExport {
            authorised: credential.export.clone(),
            requested: requested_export.to_string(),
        });
    }

    // 3b. what, continued — **which body**. The export says which door; this says what was carried
    //     through it. Checked here, after the signature and beside the export, because it is the
    //     same question: a credential authorises *one* call, and a call is an export and a payload.
    //
    //     Order matters: the signature is verified first, so a digest from an unauthenticated
    //     credential is never compared against anything. Comparing first would let an attacker
    //     learn whether a guessed body matched, under a credential we had not yet trusted.
    match &credential.body_sha256 {
        Some(bound) => {
            let actual = FederatedCaller::digest_of(body);
            // Constant-time is not required: both sides are public once the call is made, and the
            // comparison reveals nothing an attacker who holds the body does not already have.
            if *bound != actual {
                return Err(CallRefusal::BodyMismatch);
            }
        }
        None if call_policy.require_body_binding => return Err(CallRefusal::BodyNotBound),
        // Absent, and this domain has not yet required it — the rolling-upgrade window. See
        // `CallPolicy::require_body_binding` for exactly what is and is not guaranteed here.
        None => {}
    }

    // 4. when — our ceiling first, because a credential claiming a century should be refused for
    //    that reason and not merely happen to be unexpired.
    let claimed = credential.claimed_lifetime();
    if claimed > call_policy.max_lifetime {
        return Err(CallRefusal::LifetimeTooLong { claimed, max: call_policy.max_lifetime });
    }
    if now_ms > credential.expires_at_ms {
        return Err(CallRefusal::Expired {
            expires_at_ms: credential.expires_at_ms,
            now_ms,
        });
    }
    let skew = call_policy.skew_tolerance.as_millis() as u64;
    if credential.issued_at_ms > now_ms.saturating_add(skew) {
        return Err(CallRefusal::NotYetValid { issued_at_ms: credential.issued_at_ms, now_ms });
    }

    // 5. may they — authentication proved who, this decides whether.
    if !policy.permits(&credential.origin_domain, requested_export) {
        return Err(CallRefusal::NotPermitted);
    }

    Ok(AcceptedCall {
        origin_domain: credential.origin_domain.clone(),
        principal: credential.principal.clone(),
        export: requested_export.to_string(),
        policy_revision: policy.revision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did(s: &str) -> DomainId {
        DomainId::new(s).unwrap()
    }

    const NOW: u64 = 1_789_000_000_000;

    fn cred() -> FederatedCaller {
        FederatedCaller {
            origin_domain: did("beta.example"),
            principal: "svc/billing".into(),
            export: "invoice.submit".into(),
            issued_at_ms: NOW,
            expires_at_ms: NOW + 60_000,
            body_sha256: None,
        }
    }


    #[test]
    fn the_call_tag_is_distinct_from_the_other_object_tags() {
        assert_ne!(TAG_CALL, super::super::TAG_DESCRIPTOR);
        assert_ne!(TAG_CALL, super::super::TAG_POLICY);
        assert!(cred().canonical_bytes().starts_with(&(TAG_CALL.len() as u32).to_le_bytes()));
    }

    #[test]
    fn every_credential_field_is_covered_by_the_signature() {
        let base = cred().canonical_bytes();
        for mutate in [
            (|c: &mut FederatedCaller| c.principal = "svc/other".into()) as fn(&mut FederatedCaller),
            |c: &mut FederatedCaller| c.export = "invoice.status".into(),
            |c: &mut FederatedCaller| c.issued_at_ms += 1,
            |c: &mut FederatedCaller| c.expires_at_ms += 1,
            |c: &mut FederatedCaller| c.origin_domain = did("gamma.example"),
        ] {
            let mut c = cred();
            mutate(&mut c);
            assert_ne!(base, c.canonical_bytes(), "a signed field must move the bytes");
        }
    }

    #[cfg(feature = "tls")]
    mod verify {
        use super::*;
        use crate::federation::DomainPolicy;
        use ed25519_dalek::SigningKey;

        /// The body the bound fixtures below authorise. Declared here rather than beside `cred()`
        /// because only this `tls`-gated module uses it, and a constant that is dead in a
        /// feature-minimal build fails `make check`'s `--no-default-features` clippy.
        const BODY: &[u8] = br#"{"jsonrpc":"2.0","method":"tasks/send","params":{"skillId":"invoice.submit"}}"#;

        fn policy() -> DomainPolicy {
            DomainPolicy {
                domain: did("alpha.example"),
                revision: 7,
                grants: vec![(did("beta.example"), "invoice.submit".into())],
            }
        }

        fn signed(c: &FederatedCaller) -> ([u8; 32], Vec<u8>) {
            let sk = SigningKey::from_bytes(&[11u8; 32]);
            let pk = sk.verifying_key().to_bytes();
            (pk, mycelium_core::tls::sign_bytes(&sk, &c.canonical_bytes()).to_vec())
        }

        fn bundle_for(domain: &DomainId, key: [u8; 32]) -> TrustBundle {
            TrustBundle::trusting([(domain.clone(), key)])
        }

        fn check(
            c: &FederatedCaller,
            sig: &[u8],
            requested: &str,
            bundle: &TrustBundle,
            now_ms: u64,
        ) -> Result<AcceptedCall, CallRefusal> {
            check_body(c, sig, requested, BODY, bundle, &CallPolicy::default(), now_ms)
        }

        fn check_body(
            c: &FederatedCaller,
            sig: &[u8],
            requested: &str,
            body: &[u8],
            bundle: &TrustBundle,
            call_policy: &CallPolicy,
            now_ms: u64,
        ) -> Result<AcceptedCall, CallRefusal> {
            verify_federated_call(c, sig, requested, body, bundle, &policy(), call_policy, now_ms)
        }

        /// A credential for [`BODY`], signed.
        fn bound() -> (FederatedCaller, [u8; 32], Vec<u8>) {
            let c = FederatedCaller {
                body_sha256: Some(FederatedCaller::digest_of(BODY)),
                ..cred()
            };
            let (pk, sig) = signed(&c);
            (c, pk, sig)
        }

        // ── item 2 row 11: the credential binds the body ──────────────────────────────────────

        /// **A credential authenticates who is asking and, now, what they asked.**
        ///
        /// Before this the signature covered the origin, the principal, the export and the window —
        /// and nothing about the payload. An attacker on the path between two domains could rewrite
        /// the body, leave the header untouched, and the receiving gateway would accept the altered
        /// call as authentic, then make and record an authorisation decision about the attacker's
        /// text. The credential said *this principal may call `invoice.submit`* and stayed true
        /// while the invoice became a different invoice.
        #[test]
        fn a_body_changed_in_flight_is_refused() {
            let (c, pk, sig) = bound();
            let bundle = bundle_for(&c.origin_domain, pk);

            assert!(
                check_body(&c, &sig, "invoice.submit", BODY, &bundle, &CallPolicy::default(), NOW + 1_000)
                    .is_ok(),
                "the body it was signed for must be accepted",
            );

            let tampered = br#"{"jsonrpc":"2.0","method":"tasks/send","params":{"skillId":"invoice.submit","amount":999999}}"#;
            assert_eq!(
                check_body(&c, &sig, "invoice.submit", tampered, &bundle, &CallPolicy::default(), NOW + 1_000),
                Err(CallRefusal::BodyMismatch),
                "a payload changed after signing has no benign reading",
            );
        }

        /// **Stripping the binding is a forgery, not a downgrade.**
        ///
        /// Whether a credential binds a body is *inside* the signed bytes, so one minted with a
        /// binding and arriving without fails as `BadSignature` — the attacker is caught forging
        /// rather than quietly granted the weaker check. That is what the presence byte in
        /// `canonical_bytes` is for: without it, an absent binding and an all-zero digest would
        /// sign identically.
        #[test]
        fn stripping_the_binding_is_a_forgery_not_a_downgrade() {
            let (c, pk, sig) = bound();
            let bundle = bundle_for(&c.origin_domain, pk);

            let stripped = FederatedCaller { body_sha256: None, ..c.clone() };
            assert_eq!(
                check_body(&stripped, &sig, "invoice.submit", BODY, &bundle, &CallPolicy::default(), NOW + 1_000),
                Err(CallRefusal::BadSignature),
                "removing the binding must not verify, or the protection is opt-out by anyone on \
                 the path",
            );

            // Absent and all-zero must sign differently. **What this does not prove:** removing
            // the presence byte still passes it, because the digest is fixed-length and last, so
            // the two differ by 32 bytes anyway. The assertion pins the property; the byte is
            // insurance for the day a field is appended after this one. Measured, not assumed — a
            // plant that deleted the byte broke nothing.
            let zeroed = FederatedCaller { body_sha256: Some([0u8; 32]), ..c.clone() };
            assert_ne!(
                stripped.canonical_bytes(),
                zeroed.canonical_bytes(),
                "absent and all-zero must be distinguishable in the signature",
            );
        }

        /// **An unbound credential is accepted only while this domain allows it.**
        ///
        /// A partner that predates the binding sends no digest, and refusing it outright would
        /// break every existing federation on upgrade — so the default is a rolling-upgrade window
        /// and the receiving domain closes it when it is ready.
        #[test]
        fn an_unbound_credential_is_a_choice_the_receiving_domain_makes() {
            let c = cred();
            let (pk, sig) = signed(&c);
            let bundle = bundle_for(&c.origin_domain, pk);

            let permissive = CallPolicy::default();
            assert!(!permissive.require_body_binding, "the default is the upgrade window");
            assert!(
                check_body(&c, &sig, "invoice.submit", BODY, &bundle, &permissive, NOW + 1_000).is_ok(),
                "a partner that has not upgraded still works",
            );

            let strict = CallPolicy { require_body_binding: true, ..CallPolicy::default() };
            assert_eq!(
                check_body(&c, &sig, "invoice.submit", BODY, &bundle, &strict, NOW + 1_000),
                Err(CallRefusal::BodyNotBound),
                "and a domain that has finished upgrading can say so",
            );
        }

        /// The two refusals are **not** the same event.
        ///
        /// *Not bound* sends an operator to upgrade a partner. *Mismatch* sends them to look for
        /// someone on the path. Collapsing them sends them to the wrong place on the worse day.
        #[test]
        fn not_bound_and_mismatch_are_different_answers() {
            assert_ne!(CallRefusal::BodyNotBound, CallRefusal::BodyMismatch);
            assert!(format!("{}", CallRefusal::BodyNotBound).contains("does not bind"));
            assert!(format!("{}", CallRefusal::BodyMismatch).contains("changed after it was signed"));
        }

        /// The happy path, and the point of the provider adapter: **origin survives**.
        #[test]
        fn an_accepted_call_hands_the_provider_the_origin_not_the_gateway() {
            let c = cred();
            let (pk, sig) = signed(&c);
            let accepted = check(&c, &sig, "invoice.submit", &bundle_for(&c.origin_domain, pk), NOW + 1_000)
                .expect("a well-formed, permitted call");
            assert_eq!(accepted.origin_domain, did("beta.example"));
            assert_eq!(accepted.principal, "svc/billing", "the partner's principal, verbatim");
            assert_eq!(accepted.policy_revision, 7, "which rules permitted it, answerable afterwards");
        }

        /// **A rotation's overlap actually reaches the call path, and closes when it should.**
        ///
        /// `TrustBundle::rotate` is deliberately overlapping: both keys verify for a bounded window,
        /// so a partner can move to the new key at its own pace. The verifiers consulted only
        /// `key_for` — the *current* key — so the window was unreachable and a partner still signing
        /// with the retiring key was refused as `BadSignature`, which this module's own rule reserves
        /// for *someone is forging*. Accusing a correct partner of forgery sends the operator to the
        /// wrong file. Phase-C audit finding.
        #[test]
        fn a_retiring_key_verifies_inside_its_window_and_not_after() {
            let c = cred();
            let (old_pk, sig_with_old) = signed(&c);

            // The operator rotates to a new key, with the old one acceptable until NOW + 5_000.
            let new_pk = SigningKey::from_bytes(&[12u8; 32]).verifying_key().to_bytes();
            let mut bundle = bundle_for(&c.origin_domain, old_pk);
            bundle.rotate(&c.origin_domain, new_pk, NOW + 5_000);

            // Inside the window: the partner has not moved yet, and is accepted.
            check(&c, &sig_with_old, "invoice.submit", &bundle, NOW + 1_000)
                .expect("a retiring key must verify inside its own overlap window");

            // After it: refused — the window is a bound, not a suggestion.
            let after = check(&c, &sig_with_old, "invoice.submit", &bundle, NOW + 6_000);
            assert!(
                matches!(after, Err(CallRefusal::BadSignature)),
                "past the window the retiring key is no longer acceptable, got {after:?}"
            );

            // And the new key works throughout — the rotation's whole point.
            let new_sk = SigningKey::from_bytes(&[12u8; 32]);
            let sig_with_new = mycelium_core::tls::sign_bytes(&new_sk, &c.canonical_bytes()).to_vec();
            check(&c, &sig_with_new, "invoice.submit", &bundle, NOW + 6_000)
                .expect("the new key verifies after the overlap closes");
        }

        /// **The confused deputy, one boundary out.** A credential for one export must not invoke
        /// another — same partner, same key, same second.
        #[test]
        fn a_credential_for_one_export_cannot_invoke_another() {
            let c = cred();
            let (pk, sig) = signed(&c);
            let r = check(&c, &sig, "invoice.status", &bundle_for(&c.origin_domain, pk), NOW + 1_000);
            assert_eq!(
                r,
                Err(CallRefusal::WrongExport {
                    authorised: "invoice.submit".into(),
                    requested: "invoice.status".into(),
                }),
                "binding the caller without binding the call leaves the deputy confused about what"
            );
        }

        /// **Authentication is not authorisation**, and the two refusals must stay distinct: one
        /// says someone is forging, the other says we chose not to grant it.
        #[test]
        fn authenticating_is_not_being_permitted() {
            let mut c = cred();
            c.export = "ledger.audit".into();
            let (pk, sig) = signed(&c);
            let r = check(&c, &sig, "ledger.audit", &bundle_for(&c.origin_domain, pk), NOW + 1_000);
            assert_eq!(r, Err(CallRefusal::NotPermitted), "a perfectly authentic, ungranted call");

            let (_, wrong_sig) = signed(&cred());
            let forged = check(&c, &wrong_sig, "ledger.audit", &bundle_for(&c.origin_domain, pk), NOW + 1_000);
            assert_eq!(forged, Err(CallRefusal::BadSignature), "and a forgery is a different thing");
        }

        #[test]
        fn an_untrusted_domain_is_refused_however_well_formed_the_credential_is() {
            let c = cred();
            let (_, sig) = signed(&c);
            assert_eq!(
                check(&c, &sig, "invoice.submit", &TrustBundle::default(), NOW + 1_000),
                Err(CallRefusal::UnknownDomain)
            );
        }

        /// §10: issued authority lasts only to its expiry — **nothing is silently extended**.
        #[test]
        fn an_expired_credential_is_refused_with_both_times() {
            let c = cred();
            let (pk, sig) = signed(&c);
            match check(&c, &sig, "invoice.submit", &bundle_for(&c.origin_domain, pk), NOW + 61_000) {
                Err(CallRefusal::Expired { expires_at_ms, now_ms }) => {
                    assert_eq!(expires_at_ms, NOW + 60_000);
                    assert_eq!(now_ms, NOW + 61_000);
                }
                other => panic!("expected Expired, got {other:?}"),
            }
        }

        /// **The verifier bounds the lifetime, not the issuer.** Otherwise a partner mints a
        /// century-long credential and §10's promise is one the issuer makes to itself.
        #[test]
        fn a_credential_claiming_more_lifetime_than_we_honour_is_refused_for_that_reason() {
            let mut c = cred();
            c.expires_at_ms = c.issued_at_ms + 100 * 365 * 24 * 3_600_000; // a century
            let (pk, sig) = signed(&c);
            match check(&c, &sig, "invoice.submit", &bundle_for(&c.origin_domain, pk), NOW + 1_000) {
                Err(CallRefusal::LifetimeTooLong { max, .. }) => {
                    assert_eq!(max, CallPolicy::default().max_lifetime);
                }
                other => panic!(
                    "a century-long credential must be refused for its lifetime, not merely \
                     happen to be unexpired: got {other:?}"
                ),
            }
        }

        /// Cross-domain deadlines are wall-clock, so skew is real — tolerated, but bounded.
        #[test]
        fn clock_skew_is_tolerated_up_to_a_stated_bound_and_no_further() {
            let mut c = cred();
            c.issued_at_ms = NOW + 10_000; // 10s ahead; default tolerance is 30s
            c.expires_at_ms = c.issued_at_ms + 60_000;
            let (pk, sig) = signed(&c);
            let bundle = bundle_for(&c.origin_domain, pk);
            assert!(
                check(&c, &sig, "invoice.submit", &bundle, NOW).is_ok(),
                "modest skew is tolerated — two domains have no shared clock"
            );

            let mut far = cred();
            far.issued_at_ms = NOW + 120_000; // 2 minutes ahead
            far.expires_at_ms = far.issued_at_ms + 60_000;
            let (pk2, sig2) = signed(&far);
            match check(&far, &sig2, "invoice.submit", &bundle_for(&far.origin_domain, pk2), NOW) {
                Err(CallRefusal::NotYetValid { .. }) => {}
                other => panic!("beyond tolerance is wrong, not skewed: {other:?}"),
            }
        }
    }
}
