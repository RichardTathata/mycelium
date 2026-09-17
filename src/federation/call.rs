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
}

impl Default for CallPolicy {
    fn default() -> Self {
        Self { max_lifetime: Duration::from_secs(300), skew_tolerance: Duration::from_secs(30) }
    }
}

/// Why a federated call was refused.
///
/// Each variant is a different thing for an operator to do, which is why they are not one
/// `Refused`. In particular **`BadSignature` and `NotPermitted` must never merge**: the first says
/// someone is forging, the second says a partner asked for something we chose not to grant. Reading
/// a forgery as a policy gap sends the operator to edit the wrong file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallRefusal {
    /// The origin domain is not in this operator's trust bundle.
    UnknownDomain,
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
}

impl std::fmt::Display for CallRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownDomain => write!(f, "origin domain is not in the trust bundle"),
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
        out
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
/// Authentication strictly before authorisation, and both strictly before the call. A partner that
/// authenticates has proved who it is and nothing else; step 5 is where this domain decides.
#[cfg(feature = "tls")]
#[allow(clippy::too_many_arguments)]
pub fn verify_federated_call(
    credential: &FederatedCaller,
    signature: &[u8],
    requested_export: &str,
    bundle: &TrustBundle,
    policy: &DomainPolicy,
    call_policy: &CallPolicy,
    now_ms: u64,
) -> Result<AcceptedCall, CallRefusal> {
    // 1. who — the bundle decides which key, never the credential.
    let Some(key) = bundle.key_for(&credential.origin_domain) else {
        return Err(CallRefusal::UnknownDomain);
    };

    // 2. authentic
    if !mycelium_core::tls::verify_bytes(key, &credential.canonical_bytes(), signature) {
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
            TrustBundle { partners: vec![(domain.clone(), key)] }
        }

        fn check(
            c: &FederatedCaller,
            sig: &[u8],
            requested: &str,
            bundle: &TrustBundle,
            now_ms: u64,
        ) -> Result<AcceptedCall, CallRefusal> {
            verify_federated_call(c, sig, requested, bundle, &policy(), &CallPolicy::default(), now_ms)
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
