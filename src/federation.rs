//! Federation identity and policy objects (v3 contracts axis, item 2 PR 2).
//!
//! The record is [`docs/design/federated-domains.md`](../../docs/design/federated-domains.md); this
//! module is its identity half — `DomainId`, the signed [`DomainDescriptor`], the revisioned
//! [`DomainPolicy`], and the bilateral [`TrustBundle`] — plus the canonical byte encoding a
//! signature is taken over.
//!
//! # What is here and what is deliberately not
//!
//! **The contract is types and bytes.** This file and `catalog`/`call`/`gateway`/`session` decide;
//! none of them touches the wire. The transport's first arm (PR 8) is `edge` (provider side, `tls`)
//! and `client` (consumer side, `gateway` + `tls`), which carry those decisions over HTTP and add
//! none of their own. There is no `federation/` KV prefix (§8 of the record forbids one, and
//! `scripts/check-kv-namespaces.sh` enforces the absence). The invocation edge is A2A (§5).
//!
//! # Why a length-prefixed canonical form and not canonical JSON
//!
//! A signature is only meaningful if two implementations agree, byte for byte, on what was signed.
//! Canonical JSON *can* provide that and is a known foot-gun while doing it: key ordering,
//! non-ASCII escaping, surrogate pairs above the BMP, and number formatting all have to match
//! exactly, and each is a place where two correct-looking implementations differ. (This repository
//! has a canonical-JSON encoder precisely because a consumer required one, and it needed a
//! character-by-character reimplementation of another language's escaping rules to be correct.)
//!
//! Here we own both ends, so the encoding is chosen to have no such freedom: every field is
//! length-prefixed, every integer is little-endian fixed width, and every structure begins with a
//! **domain-separation tag**. There is exactly one way to encode a given value and no way to encode
//! two different values identically.
//!
//! # Domain separation is a security property, not tidiness
//!
//! Each object type's canonical bytes begin with a distinct tag, so a signature over a descriptor
//! can never verify as a signature over a policy. This is the same rule the record states for
//! reusing the OIDC verifier (D6): *reuse the code, never the trust* — an object valid for one
//! purpose must not be valid for another merely because the same key signed it.

pub mod call;
pub mod catalog;
pub mod gateway;
pub mod session;
/// The transport's provider side (item 2 PR 8): the credential's wire form, the edge that
/// authenticates and authorises it, the catalogue reply.
#[cfg(feature = "tls")]
pub mod edge;
/// The transport's consumer side (item 2 PR 8): link, resolver and pool driven by HTTP.
#[cfg(all(feature = "gateway", feature = "tls"))]
pub mod client;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Domain-separation tag for [`DomainDescriptor`] signatures.
pub const TAG_DESCRIPTOR: &str = "mycelium.federation/descriptor/1";
/// Domain-separation tag for [`DomainPolicy`] signatures.
pub const TAG_POLICY: &str = "mycelium.federation/policy/1";

/// The stable name of a domain.
///
/// A domain is one independently admitted gossip mesh (§1 of the record). `DomainId` names it;
/// `cluster_name` remains a cosmetic label with no isolation meaning and is **not** this.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DomainId(Arc<str>);

/// Why a candidate string is not a usable `DomainId`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainIdError {
    /// Empty, or longer than 253 bytes.
    Length(usize),
    /// Contains a byte outside `[a-z0-9.-]`.
    ///
    /// Lowercase-only by construction rather than by normalisation: two ids differing only in case
    /// would be one domain to a human and two to a `HashMap`, and the place that difference would
    /// surface is a trust decision.
    Charset(char),
}

impl std::fmt::Display for DomainIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Length(n) => write!(f, "domain id must be 1..=253 bytes, got {n}"),
            Self::Charset(c) => write!(f, "domain id may contain only [a-z0-9.-], found {c:?}"),
        }
    }
}

impl std::error::Error for DomainIdError {}

impl DomainId {
    /// Validate and construct.
    pub fn new(s: impl AsRef<str>) -> Result<Self, DomainIdError> {
        let s = s.as_ref();
        if s.is_empty() || s.len() > 253 {
            return Err(DomainIdError::Length(s.len()));
        }
        if let Some(c) = s.chars().find(|c| !matches!(c, 'a'..='z' | '0'..='9' | '.' | '-')) {
            return Err(DomainIdError::Charset(c));
        }
        Ok(Self(Arc::from(s)))
    }

    /// The id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DomainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a domain says about itself and what it exports.
///
/// Signed by the domain's key. Its **public subset** is an AgentFacts profile through the existing
/// serializer (§7, D25) — there is no second well-known document, and this type is not one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainDescriptor {
    /// Which domain this describes.
    pub domain: DomainId,
    /// The domain's Ed25519 verifying key.
    pub public_key: [u8; 32],
    /// When it was issued, in epoch milliseconds.
    pub issued_at_ms: u64,
    /// The policy revision in force when it was issued — so "which rules applied" is answerable
    /// after the fact rather than inferred.
    pub policy_revision: u64,
    /// The **explicitly exported** service names. Nothing not listed here is offered, and the list
    /// is part of the signed bytes so it cannot be widened in transit.
    pub exports: Vec<String>,
}

/// A revisioned statement of who may invoke what.
///
/// Revisioned rather than versionless because a policy revision is the unit an evidence record
/// names: *"under which rules was this allowed"* has to have an answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainPolicy {
    /// Whose policy this is.
    pub domain: DomainId,
    /// Monotonic revision. A consumer that has seen a higher revision must not accept a lower one.
    pub revision: u64,
    /// `(partner, export)` pairs this revision permits. Absence is denial — there is no wildcard,
    /// because a wildcard is how an export nobody reviewed becomes reachable.
    pub grants: Vec<(DomainId, String)>,
}

impl DomainPolicy {
    /// Does this revision permit `partner` to invoke `export`?
    ///
    /// Absence is denial. This is the whole evaluation: there is no inheritance, no wildcard and no
    /// default-allow, because each of those is a way for a grant to exist that nobody wrote down.
    pub fn permits(&self, partner: &DomainId, export: &str) -> bool {
        self.grants.iter().any(|(p, e)| p == partner && e == export)
    }
}

/// One partner, its current key, and anything in transition (item 2 PR 6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartnerTrust {
    /// Whose entry this is.
    pub domain: DomainId,
    /// The key currently in force.
    pub key: [u8; 32],
    /// A key being **retired**, still accepted until the given epoch-millisecond deadline.
    ///
    /// Rotation without a flag day: for a bounded window both keys verify, so a partner can start
    /// signing with the new one before every counterparty has finished updating. **Bounded**
    /// because an unbounded overlap is not a rotation, it is two keys — and the compromised one is
    /// still among them.
    pub retiring: Option<([u8; 32], u64)>,
    /// **Revoked.** Nothing from this partner is accepted, under any key, ignoring every other
    /// field. Kept as a tombstone rather than deleted, so *"we used to trust them and stopped"* is
    /// distinguishable from *"we never heard of them"* — the two mean different things to an
    /// operator reading a refusal.
    pub revoked: bool,
}

/// The partners this operator has chosen to trust, and their keys.
///
/// **Bilateral operator configuration** — no registry, no trust-registry service (D25). A bundle is
/// a local decision about who this domain will talk to, and there is deliberately no mechanism by
/// which a third party can add an entry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustBundle {
    /// One entry per partner.
    pub partners: Vec<PartnerTrust>,
}

impl TrustBundle {
    /// A bundle trusting each `(domain, key)` outright — the common case, and what tests want.
    pub fn trusting(entries: impl IntoIterator<Item = (DomainId, [u8; 32])>) -> Self {
        Self {
            partners: entries
                .into_iter()
                .map(|(domain, key)| PartnerTrust { domain, key, retiring: None, revoked: false })
                .collect(),
        }
    }

    fn entry(&self, domain: &DomainId) -> Option<&PartnerTrust> {
        self.partners.iter().find(|p| &p.domain == domain)
    }

    /// The key currently in force for `domain`, if it is trusted at all.
    ///
    /// Returns `None` for a revoked partner: a revoked entry is not a key that happens to fail, it
    /// is an absence of trust.
    pub fn key_for(&self, domain: &DomainId) -> Option<&[u8; 32]> {
        self.entry(domain).filter(|p| !p.revoked).map(|p| &p.key)
    }

    /// Every key acceptable for `domain` at `now_ms` — current first, then a retiring key whose
    /// window has not closed.
    ///
    /// Callers try them in order. Current-first matters: after a rotation the overwhelming majority
    /// of traffic is signed with the new key, and checking the old one first would spend a
    /// signature verification on the unlikely case for the whole overlap window.
    pub fn acceptable_keys(&self, domain: &DomainId, now_ms: u64) -> Vec<[u8; 32]> {
        let Some(p) = self.entry(domain) else { return Vec::new() };
        if p.revoked {
            return Vec::new();
        }
        let mut keys = vec![p.key];
        if let Some((old, until_ms)) = p.retiring
            && now_ms <= until_ms
        {
            keys.push(old);
        }
        keys
    }

    /// Is this partner known but revoked? Distinct from unknown — see [`PartnerTrust::revoked`].
    pub fn is_revoked(&self, domain: &DomainId) -> bool {
        self.entry(domain).is_some_and(|p| p.revoked)
    }

    /// Stop accepting anything from `domain`, keeping the tombstone.
    pub fn revoke(&mut self, domain: &DomainId) {
        if let Some(p) = self.partners.iter_mut().find(|p| &p.domain == domain) {
            p.revoked = true;
        }
    }

    /// Rotate `domain` to `new_key`, accepting the previous one until `until_ms`.
    pub fn rotate(&mut self, domain: &DomainId, new_key: [u8; 32], until_ms: u64) {
        if let Some(p) = self.partners.iter_mut().find(|p| &p.domain == domain) {
            p.retiring = Some((p.key, until_ms));
            p.key = new_key;
        }
    }
}

// ── Canonical bytes ───────────────────────────────────────────────────────────────────────────

/// Append a length-prefixed byte string: `u32` little-endian length, then the bytes.
fn put_lp(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl DomainDescriptor {
    /// The exact bytes a descriptor signature is taken over.
    ///
    /// Begins with [`TAG_DESCRIPTOR`], so this can never collide with a policy's bytes.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_lp(&mut out, TAG_DESCRIPTOR.as_bytes());
        put_lp(&mut out, self.domain.as_str().as_bytes());
        put_lp(&mut out, &self.public_key);
        out.extend_from_slice(&self.issued_at_ms.to_le_bytes());
        out.extend_from_slice(&self.policy_revision.to_le_bytes());
        out.extend_from_slice(&(self.exports.len() as u32).to_le_bytes());
        // Order is the field's order, not sorted: `exports` is a list the issuer wrote, and
        // re-ordering it here would mean two different documents signed identical bytes.
        for e in &self.exports {
            put_lp(&mut out, e.as_bytes());
        }
        out
    }
}

impl DomainPolicy {
    /// The exact bytes a policy signature is taken over. Begins with [`TAG_POLICY`].
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_lp(&mut out, TAG_POLICY.as_bytes());
        put_lp(&mut out, self.domain.as_str().as_bytes());
        out.extend_from_slice(&self.revision.to_le_bytes());
        out.extend_from_slice(&(self.grants.len() as u32).to_le_bytes());
        for (p, e) in &self.grants {
            put_lp(&mut out, p.as_str().as_bytes());
            put_lp(&mut out, e.as_bytes());
        }
        out
    }
}

// ── Verification ──────────────────────────────────────────────────────────────────────────────

/// Verify a descriptor against a key the operator already trusts for that domain.
///
/// **The bundle decides which key, not the descriptor.** A descriptor carries a `public_key`, and
/// trusting *that* would make every self-signed document self-authorising — the signature would
/// prove only that whoever wrote it had a key. The key comes from the [`TrustBundle`], which is a
/// decision this operator made; the descriptor's own field is checked to match, and a mismatch is a
/// refusal.
#[cfg(feature = "tls")]
pub fn verify_descriptor(
    descriptor: &DomainDescriptor,
    signature: &[u8],
    bundle: &TrustBundle,
) -> bool {
    let Some(trusted) = bundle.key_for(&descriptor.domain) else { return false };
    if *trusted != descriptor.public_key {
        return false;
    }
    mycelium_core::tls::verify_bytes(trusted, &descriptor.canonical_bytes(), signature)
}

/// Verify a descriptor during a key rotation: any key the bundle still accepts at `now_ms` will do.
///
/// Separate from [`verify_descriptor`] rather than replacing it, because a descriptor also pins its
/// own `public_key`, and during an overlap that field legitimately lags the bundle's current key.
#[cfg(feature = "tls")]
pub fn verify_descriptor_rotating(
    descriptor: &DomainDescriptor,
    signature: &[u8],
    bundle: &TrustBundle,
    now_ms: u64,
) -> bool {
    let bytes = descriptor.canonical_bytes();
    bundle
        .acceptable_keys(&descriptor.domain, now_ms)
        .iter()
        .any(|k| *k == descriptor.public_key && mycelium_core::tls::verify_bytes(k, &bytes, signature))
}

/// Verify a policy the same way, against the key the bundle trusts for its domain.
#[cfg(feature = "tls")]
pub fn verify_policy(policy: &DomainPolicy, signature: &[u8], bundle: &TrustBundle) -> bool {
    let Some(trusted) = bundle.key_for(&policy.domain) else { return false };
    mycelium_core::tls::verify_bytes(trusted, &policy.canonical_bytes(), signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did(s: &str) -> DomainId {
        DomainId::new(s).unwrap()
    }

    fn descriptor() -> DomainDescriptor {
        DomainDescriptor {
            domain: did("alpha.example"),
            public_key: [7u8; 32],
            issued_at_ms: 1_789_000_000_000,
            policy_revision: 3,
            exports: vec!["invoice.submit".into(), "invoice.status".into()],
        }
    }

    fn policy() -> DomainPolicy {
        DomainPolicy {
            domain: did("alpha.example"),
            revision: 3,
            grants: vec![(did("beta.example"), "invoice.submit".into())],
        }
    }

    // ── identity ─────────────────────────────────────────────────────────────────────────────

    #[test]
    fn domain_ids_are_validated_not_normalised() {
        assert!(DomainId::new("alpha.example").is_ok());
        assert!(DomainId::new("a-b.c-d").is_ok());
        assert_eq!(DomainId::new(""), Err(DomainIdError::Length(0)));
        assert_eq!(DomainId::new("x".repeat(254)), Err(DomainIdError::Length(254)));
        // Uppercase is REJECTED rather than lowercased. Two ids differing only in case would be one
        // domain to a human and two to a map, and the place that difference surfaces is a trust
        // decision — so it is refused where it is written, not repaired where it is read.
        assert_eq!(DomainId::new("Alpha.example"), Err(DomainIdError::Charset('A')));
        assert_eq!(DomainId::new("alpha_example"), Err(DomainIdError::Charset('_')));
    }

    // ── canonical bytes ──────────────────────────────────────────────────────────────────────

    /// **The test vector.** Pinned bytes, so another implementation can check itself against this
    /// one rather than against a description of it — and so a change to the encoding shows up here
    /// as a diff instead of as a signature that stops verifying in the field.
    #[test]
    fn descriptor_canonical_bytes_are_pinned() {
        let got = descriptor().canonical_bytes();
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "20000000\
6d7963656c69756d2e66656465726174696f6e2f64657363726970746f722f31\
0d000000616c7068612e6578616d706c65\
20000000\
0707070707070707070707070707070707070707070707070707070707070707\
00a2b588a0010000\
0300000000000000\
02000000\
0e000000696e766f6963652e7375626d6974\
0e000000696e766f6963652e737461747573",
            "descriptor canonical bytes changed — this is a wire-visible break for any signature \
             already issued, so it must be a deliberate, versioned change (bump the tag)"
        );
    }

    #[test]
    fn policy_canonical_bytes_are_pinned() {
        let got = policy().canonical_bytes();
        let hex: String = got.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "1c000000\
6d7963656c69756d2e66656465726174696f6e2f706f6c6963792f31\
0d000000616c7068612e6578616d706c65\
0300000000000000\
01000000\
0c000000626574612e6578616d706c65\
0e000000696e766f6963652e7375626d6974"
        );
    }

    /// **The pin is not the only guard.**
    ///
    /// A hex vector generated from the implementation and then asserted against the implementation
    /// proves only that it is deterministic — it would happily pin a wrong encoding. (It nearly did:
    /// the first version of the vector above was hand-written and had the timestamp's little-endian
    /// bytes wrong, and the *implementation* was correct.)
    ///
    /// So the encoding is also checked **structurally**: decode the bytes back with an independent
    /// reader and require every field to reappear, with nothing left over. Together the two say
    /// "this is the right encoding" and "it has not silently changed"; neither says both alone.
    #[test]
    fn canonical_bytes_decode_back_to_exactly_the_fields_that_went_in() {
        struct Reader<'a> { b: &'a [u8], i: usize }
        impl<'a> Reader<'a> {
            fn lp(&mut self) -> &'a [u8] {
                let n = u32::from_le_bytes(self.b[self.i..self.i + 4].try_into().unwrap()) as usize;
                self.i += 4;
                let v = &self.b[self.i..self.i + n];
                self.i += n;
                v
            }
            fn u32(&mut self) -> u32 {
                let v = u32::from_le_bytes(self.b[self.i..self.i + 4].try_into().unwrap());
                self.i += 4;
                v
            }
            fn u64(&mut self) -> u64 {
                let v = u64::from_le_bytes(self.b[self.i..self.i + 8].try_into().unwrap());
                self.i += 8;
                v
            }
        }

        let d = descriptor();
        let bytes = d.canonical_bytes();
        let mut r = Reader { b: &bytes, i: 0 };

        assert_eq!(r.lp(), TAG_DESCRIPTOR.as_bytes(), "tag first, for domain separation");
        assert_eq!(r.lp(), d.domain.as_str().as_bytes());
        assert_eq!(r.lp(), &d.public_key[..]);
        assert_eq!(r.u64(), d.issued_at_ms);
        assert_eq!(r.u64(), d.policy_revision);
        let n = r.u32() as usize;
        assert_eq!(n, d.exports.len());
        for want in &d.exports {
            assert_eq!(r.lp(), want.as_bytes(), "exports keep the issuer's order");
        }
        assert_eq!(r.i, bytes.len(), "no trailing bytes — the encoding is exactly the fields");
    }

    /// **Domain separation.** A descriptor and a policy that agree on every shared field still
    /// produce different bytes, so a signature over one can never verify as the other.
    #[test]
    fn a_descriptor_signature_can_never_verify_as_a_policy() {
        let d = descriptor().canonical_bytes();
        let p = policy().canonical_bytes();
        assert_ne!(d, p);
        assert!(d.starts_with(&(TAG_DESCRIPTOR.len() as u32).to_le_bytes()));
        assert!(p.starts_with(&(TAG_POLICY.len() as u32).to_le_bytes()));
        assert_ne!(TAG_DESCRIPTOR, TAG_POLICY);
    }

    /// Length prefixes exist so no two distinct values can encode identically. Without them,
    /// `["ab", "c"]` and `["a", "bc"]` would be the same bytes — and an attacker choosing export
    /// names would be choosing which signature to forge.
    #[test]
    fn length_prefixes_make_the_encoding_unambiguous() {
        let mut a = descriptor();
        a.exports = vec!["ab".into(), "c".into()];
        let mut b = descriptor();
        b.exports = vec!["a".into(), "bc".into()];
        assert_ne!(
            a.canonical_bytes(),
            b.canonical_bytes(),
            "a concatenation-ambiguous encoding would let one signature cover both"
        );
    }

    /// A field change moves the bytes — otherwise the signature would not cover it.
    #[test]
    fn every_signed_field_is_actually_covered() {
        let base = descriptor().canonical_bytes();
        let mut changed = descriptor();
        changed.policy_revision += 1;
        assert_ne!(base, changed.canonical_bytes(), "policy_revision must be covered");
        let mut changed = descriptor();
        changed.exports.push("invoice.void".into());
        assert_ne!(base, changed.canonical_bytes(), "exports must be covered");
        let mut changed = descriptor();
        changed.public_key = [8u8; 32];
        assert_ne!(base, changed.canonical_bytes(), "public_key must be covered");
        let mut changed = descriptor();
        changed.issued_at_ms += 1;
        assert_ne!(base, changed.canonical_bytes(), "issued_at_ms must be covered");
    }

    // ── policy evaluation ────────────────────────────────────────────────────────────────────

    #[test]
    fn absence_is_denial_and_there_is_no_wildcard() {
        let p = policy();
        assert!(p.permits(&did("beta.example"), "invoice.submit"));
        assert!(!p.permits(&did("beta.example"), "invoice.status"), "an unlisted export is denied");
        assert!(!p.permits(&did("gamma.example"), "invoice.submit"), "an unlisted partner is denied");
        assert!(!p.permits(&did("beta.example"), "*"), "there is no wildcard to match");
    }

    // ── trust bundles ────────────────────────────────────────────────────────────────────────

    #[test]
    fn a_bundle_only_knows_partners_the_operator_put_in_it() {
        let b = TrustBundle::trusting([(did("beta.example"), [9u8; 32])]);
        assert_eq!(b.key_for(&did("beta.example")), Some(&[9u8; 32]));
        assert_eq!(b.key_for(&did("gamma.example")), None, "no implicit trust");
        assert_eq!(TrustBundle::default().key_for(&did("beta.example")), None);
    }

    // ── rotation and revocation (item 2 PR 6) ────────────────────────────────────────────────

    #[test]
    fn a_revoked_partner_has_no_acceptable_keys_and_is_distinguishable_from_an_unknown_one() {
        let mut b = TrustBundle::trusting([(did("beta.example"), [9u8; 32])]);
        b.revoke(&did("beta.example"));

        assert_eq!(b.key_for(&did("beta.example")), None, "revoked is an absence of trust");
        assert!(b.acceptable_keys(&did("beta.example"), 0).is_empty());

        // The distinction the tombstone exists for: "we used to trust them and stopped" is not the
        // same fact as "we never heard of them", and an operator reading a refusal needs to know
        // which.
        assert!(b.is_revoked(&did("beta.example")));
        assert!(!b.is_revoked(&did("gamma.example")), "never-known is not revoked");
    }

    /// **Rotation without a flag day.** For a bounded window both keys verify, so a partner can
    /// start signing with the new one before every counterparty has finished updating.
    #[test]
    fn rotation_accepts_both_keys_until_the_window_closes() {
        let old = [1u8; 32];
        let new = [2u8; 32];
        let mut b = TrustBundle::trusting([(did("beta.example"), old)]);
        b.rotate(&did("beta.example"), new, 1_000);

        let during = b.acceptable_keys(&did("beta.example"), 500);
        assert_eq!(during, vec![new, old], "current first — most traffic is signed with the new key");

        let at_deadline = b.acceptable_keys(&did("beta.example"), 1_000);
        assert_eq!(at_deadline.len(), 2, "the window is inclusive of its stated deadline");

        let after = b.acceptable_keys(&did("beta.example"), 1_001);
        assert_eq!(after, vec![new], "past the window the old key is simply gone");
    }

    /// An unbounded overlap is not a rotation — it is two keys, and the compromised one is still
    /// among them. The window closing is what makes it a rotation.
    #[test]
    fn a_retired_key_stops_being_accepted() {
        let old = [1u8; 32];
        let mut b = TrustBundle::trusting([(did("beta.example"), old)]);
        b.rotate(&did("beta.example"), [2u8; 32], 1_000);
        assert!(!b.acceptable_keys(&did("beta.example"), 9_999).contains(&old));
    }

    /// Revocation beats rotation: a revoked partner has no keys, mid-rotation or not.
    #[test]
    fn revoking_mid_rotation_accepts_neither_key() {
        let mut b = TrustBundle::trusting([(did("beta.example"), [1u8; 32])]);
        b.rotate(&did("beta.example"), [2u8; 32], 10_000);
        b.revoke(&did("beta.example"));
        assert!(b.acceptable_keys(&did("beta.example"), 500).is_empty());
    }

    // ── verification ─────────────────────────────────────────────────────────────────────────

    #[cfg(feature = "tls")]
    mod verify {
        use super::*;
        use ed25519_dalek::SigningKey;

        fn keypair() -> (SigningKey, [u8; 32]) {
            let sk = SigningKey::from_bytes(&[42u8; 32]);
            let pk = sk.verifying_key().to_bytes();
            (sk, pk)
        }

        #[test]
        fn a_descriptor_signed_by_a_trusted_key_verifies_and_a_tampered_one_does_not() {
            let (sk, pk) = keypair();
            let mut d = descriptor();
            d.public_key = pk;
            let sig = mycelium_core::tls::sign_bytes(&sk, &d.canonical_bytes());
            let bundle = TrustBundle::trusting([(d.domain.clone(), pk)]);

            assert!(verify_descriptor(&d, &sig, &bundle), "a correctly signed descriptor verifies");

            let mut tampered = d.clone();
            tampered.exports.push("invoice.void".into());
            assert!(
                !verify_descriptor(&tampered, &sig, &bundle),
                "widening the export list must invalidate the signature — that list is the whole \
                 point of signing a descriptor"
            );
        }

        /// **The bundle decides which key.** A self-signed descriptor carrying its own key is not
        /// authorised by being internally consistent; it is authorised by the operator having put
        /// that key in the bundle.
        #[test]
        fn a_self_consistent_descriptor_from_an_untrusted_domain_is_refused() {
            let (sk, pk) = keypair();
            let mut d = descriptor();
            d.public_key = pk;
            let sig = mycelium_core::tls::sign_bytes(&sk, &d.canonical_bytes());

            // Internally perfect, and from a domain this operator never listed.
            let empty = TrustBundle::default();
            assert!(
                !verify_descriptor(&d, &sig, &empty),
                "an unknown domain must be refused however well-formed its document is"
            );

            // Listed, but with a different key than the descriptor claims.
            let wrong = TrustBundle::trusting([(d.domain.clone(), [1u8; 32])]);
            assert!(
                !verify_descriptor(&d, &sig, &wrong),
                "a descriptor whose key disagrees with the bundle's must be refused, not preferred"
            );
        }

        #[test]
        fn a_policy_signature_does_not_verify_a_descriptor() {
            let (sk, pk) = keypair();
            let p = policy();
            let policy_sig = mycelium_core::tls::sign_bytes(&sk, &p.canonical_bytes());

            let mut d = descriptor();
            d.public_key = pk;
            let bundle = TrustBundle::trusting([(d.domain.clone(), pk)]);
            assert!(
                !verify_descriptor(&d, &policy_sig, &bundle),
                "domain separation: a policy signature must not authenticate a descriptor"
            );
        }
    }
}
