//! Pinned TLS for the federation edge — the trust anchor is the bundle, not a CA (item 2 row 11).
//!
//! Everything a federated call carries is already signed: [`super::call::FederatedCaller`] binds
//! the origin domain, the principal, the export, the validity window and — since 2.12.0 — the
//! request body. What that signature does *not* provide is **confidentiality**. An observer on the
//! path between two domains reads every call and every reply in the clear.
//!
//! TLS fixes that, and TLS needs a trust anchor. This module is the decision about which one.
//!
//! # Why the bundle and not a CA
//!
//! Partner trust in this design is **one Ed25519 key per partner, chosen bilaterally by an
//! operator** ([`super::PartnerTrust`], D25: no registry, no trust-registry service). There is no
//! X.509 material anywhere in it. So a TLS anchor had three candidates:
//!
//! | Anchor | What it would mean |
//! |---|---|
//! | The public Web PKI | *Anyone* a public CA will issue for becomes a possible partner endpoint — a trust root far larger than the bilateral one, installed underneath a bilateral design. It also requires every federated gateway to hold a publicly-resolvable name and a renewed public certificate, which a private partner deployment often cannot. |
//! | Exchange private CAs | Workable, and strictly more machinery: a CA per domain, a second rotation story, and a second document whose compromise is total (a CA signs *any* name). It buys delegation — the partner may re-issue endpoint certs freely — which is the one property a two-party link does not need. |
//! | **Pin the key in the bundle** | The anchor is the decision the operator already made. Nothing new to distribute, and a compromise costs one endpoint rather than a namespace. |
//!
//! The third is what this module implements, and the reasoning is the parent module's own rule for
//! descriptors: **the bundle decides which key, never the document.** A certificate that vouches
//! for itself is not evidence.
//!
//! # What is pinned
//!
//! The **sha256 of the end-entity certificate's `SubjectPublicKeyInfo`**, DER-encoded, outer
//! `SEQUENCE` included — the same value as an RFC 7469 pin, and the same value that
//! `openssl x509 -pubkey | openssl pkey -pubin -outform der | sha256sum` prints. The key, not the
//! certificate: a partner may re-issue, extend or re-name a certificate around the same key
//! without every counterparty editing a bundle.
//!
//! [`super::PartnerTrust::tls_spki_sha256`] holds a **list**, for the reason
//! [`super::PartnerTrust::retiring`] exists: a single pin makes key rotation a flag day. The
//! partner publishes the next pin, every counterparty adds it, the partner swaps, the old one goes.
//!
//! # What this proves, and what it does not
//!
//! A completed handshake proves the endpoint **holds the private key** for a pinned SPKI — the
//! handshake signature is verified with the provider's own algorithms, not asserted. So it proves
//! that endpoint is the partner, and it encrypts the call.
//!
//! It does **not** authenticate the caller — that stays the credential, deliberately. A federated
//! call authenticated by its transport would be a second authentication model beside the one the
//! record already chose (the ADR's divergence note: *two invocation edges with different auth
//! models is the drift the gateway-auth fixes just cleaned up*). There is no client certificate
//! here, and a pinned link does not make an unsigned call acceptable.
//!
//! It also does not check the certificate's name, chain or expiry — **there is nothing to check
//! them against**, and a name checked against no anchor is decoration. Stated rather than left
//! silently true: see [`PinnedServerVerifier::verify_server_cert`].
//!
//! # Why this does not reuse `ed25519_key_from_cert_der`
//!
//! `mycelium_core::tls::ed25519_key_from_cert_der` finds a key by **scanning the DER for the
//! Ed25519 SPKI byte prefix**, and its doc states the safety argument exactly: the input has
//! already been validated against the cluster CA, so the bytes are a well-formed cert this
//! codebase generated.
//!
//! Here there is no CA, and the input is **whatever an attacker put on the wire**. A scan matches
//! that prefix anywhere in the certificate — in a subject, an extension, an issuer name an
//! attacker chooses freely — so an attacker could carry the partner's public key as *data* inside
//! a certificate whose actual key is their own, and the pin would match a key that is not signing
//! the handshake. [`spki_sha256`] therefore reads the SPKI from its **structural position**, using
//! the X.509 parser rustls itself uses. The plant in this module's tests builds that certificate
//! and asserts the two disagree.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error as TlsError, OtherError, SignatureScheme};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The sha256 of a certificate's `SubjectPublicKeyInfo`, or `None` if the DER is not a certificate
/// this parser will read.
///
/// The SPKI is taken from its structural position by rustls's own X.509 parser — never by
/// searching the encoding for a key-shaped byte pattern. See the module docs for the attack that
/// distinction refuses.
pub fn spki_sha256(cert_der: &[u8]) -> Option<[u8; 32]> {
    let der = CertificateDer::from(cert_der);
    let cert = webpki::EndEntityCert::try_from(&der).ok()?;
    let spki = cert.subject_public_key_info();
    Some(Sha256::digest(spki.as_ref()).into())
}

/// The pin for an endpoint that terminates TLS with an **Ed25519 identity key** — the node-cert
/// reuse path (`GatewayTlsConfig::default()`), where the certificate is regenerated at every
/// startup and never written to disk.
///
/// A partner on that path has no certificate file to hand a counterparty, but it does have a stable
/// public key, and the Ed25519 `SubjectPublicKeyInfo` is a fixed 12-byte header followed by the key
/// — RFC 8410 admits exactly one encoding, with no parameters and no choices. So the pin can be
/// computed from the key alone.
///
/// This constructs an encoding by hand, which is what the module docs warn against everywhere else.
/// The distinction is the direction: [`spki_sha256`] reads **attacker-supplied** bytes, where a
/// guess about layout is a vulnerability; this writes bytes for a key already trusted, and
/// `the_two_ways_to_compute_a_pin_agree` asserts the result equals the structural read of a real
/// certificate carrying that key. If the two ever disagree, that test fails rather than a partner
/// silently becoming unreachable.
pub fn ed25519_spki_sha256(public_key: &[u8; 32]) -> [u8; 32] {
    // SEQUENCE(42) { SEQUENCE(5) { OID 1.3.101.112 }, BIT STRING(33) { 0 unused bits, key } }
    const HEADER: [u8; 12] = [0x30, 0x2A, 0x30, 0x05, 0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];
    let mut hasher = Sha256::new();
    hasher.update(HEADER);
    hasher.update(public_key);
    hasher.finalize().into()
}

/// The digest an endpoint presented, when it was not one this domain pins.
///
/// Carried as the source of the rustls error so the client can distinguish a **pin mismatch** — an
/// endpoint that is not the partner — from a silent gateway, which means something entirely
/// different to a caller. See `ClientError::Tls`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinMismatch {
    /// sha256 of the SPKI the endpoint actually presented.
    pub presented: [u8; 32],
    /// How many pins this domain would have accepted.
    pub pins_configured: usize,
}

impl std::fmt::Display for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the endpoint presented an SPKI this domain does not pin (sha256 {}, {} pin(s) configured)",
            hex32(&self.presented),
            self.pins_configured
        )
    }
}

impl std::error::Error for PinMismatch {}

/// Lower-case hex, for an operator comparing a refusal against a bundle.
pub(crate) fn hex32(bytes: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(64);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// A rustls server-certificate verifier whose entire trust decision is the pin list.
#[derive(Debug)]
pub struct PinnedServerVerifier {
    pins: Vec<[u8; 32]>,
    provider: Arc<CryptoProvider>,
}

impl PinnedServerVerifier {
    /// Accept exactly the endpoints whose SPKI digest is in `pins`.
    ///
    /// An **empty** list accepts nothing. That is deliberate: this verifier exists only because a
    /// pin was configured, and an empty pin set is a configuration mistake rather than a request to
    /// fall back to some other anchor. `FederationClient` never installs it without pins.
    pub fn new(pins: Vec<[u8; 32]>, provider: Arc<CryptoProvider>) -> Self {
        Self { pins, provider }
    }
}

impl ServerCertVerifier for PinnedServerVerifier {
    /// The whole trust decision: does the end-entity certificate's SPKI hash to a pin?
    ///
    /// Intermediates, the server name, OCSP staples and validity dates are all ignored, and that is
    /// honest behaviour rather than a shortcut. Each of them is a check *relative to an anchor* — a
    /// chain needs a root, a name needs an authority that attests names, an expiry is the issuer's
    /// statement about its own document. This verifier has no such authority; it has one key an
    /// operator wrote down. Enforcing a self-asserted expiry would add an outage mode and no
    /// security, and checking a name against nothing would read like validation while proving
    /// nothing.
    ///
    /// The comparison is a plain `==`. A constant-time compare protects a *secret*, and a pin is
    /// the hash of a public key the partner publishes — an attacker who learns it has learned
    /// nothing they could not fetch.
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        let Some(digest) = spki_sha256(end_entity) else {
            return Err(TlsError::InvalidCertificate(CertificateError::BadEncoding));
        };
        if self.pins.contains(&digest) {
            return Ok(ServerCertVerified::assertion());
        }
        Err(TlsError::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(PinMismatch {
            presented: digest,
            pins_configured: self.pins.len(),
        })))))
    }

    /// Delegated to the provider, never asserted.
    ///
    /// This is the half that makes a pin mean anything. `verify_server_cert` says *we would accept
    /// this key*; these two say *the peer proved it holds the matching private key for this
    /// handshake*. A verifier returning `assertion()` here — the shape every "skip TLS
    /// verification" snippet has — would accept any endpoint that could copy the partner's
    /// certificate, which is public by construction.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A rustls client config that trusts exactly these pins and no certificate authority.
pub fn pinned_client_config(pins: Vec<[u8; 32]>) -> rustls::ClientConfig {
    // Pin `ring` explicitly rather than depending on whichever provider a host installed first —
    // the same reasoning as `mycelium_core::tls`, which installs it process-wide. Taking the
    // provider by value here means this config is correct even in a process that installed none.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ClientConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServerVerifier::new(pins, provider)))
        .with_no_client_auth()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};

    /// A self-signed certificate for `name`, signed by `key`.
    fn cert_with(key: &KeyPair, name: &str) -> Vec<u8> {
        let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        params.self_signed(key).unwrap().der().to_vec()
    }

    fn ed25519_key() -> KeyPair {
        KeyPair::generate_for(&PKCS_ED25519).unwrap()
    }

    /// The pin follows the **key**, not the document: re-issuing a certificate around the same key
    /// — a new name, new dates, new serial — leaves every counterparty's bundle correct.
    ///
    /// This is the property that decides SPKI-pinning over certificate-pinning, so it is asserted
    /// rather than assumed, and the negative half is asserted beside it: a *different* key is a
    /// different pin, or "the pin follows the key" would be satisfied by a constant.
    #[test]
    fn the_pin_follows_the_key_and_not_the_certificate() {
        let key = ed25519_key();
        let first = cert_with(&key, "gateway-a.partner.example");
        let reissued = cert_with(&key, "gateway-b.partner.example");
        assert_ne!(first, reissued, "two certificates, or this proves nothing");

        let pin = spki_sha256(&first).expect("a generated certificate parses");
        assert_eq!(Some(pin), spki_sha256(&reissued), "same key, same pin");

        let other = cert_with(&ed25519_key(), "gateway-a.partner.example");
        assert_ne!(Some(pin), spki_sha256(&other), "a different key must be a different pin");
    }

    /// The hand-written Ed25519 SPKI encoding and the structural read of a real certificate must
    /// produce the same pin — otherwise a partner that published its pin from its identity key
    /// would be refused by every counterparty, with a message about an unpinned key and no hint
    /// that the two sides computed the value differently.
    #[test]
    fn the_two_ways_to_compute_a_pin_agree() {
        let key = ed25519_key();
        let cert = cert_with(&key, "partner.example");
        let public = mycelium_core::tls::ed25519_key_from_cert_der(&cert).expect("an Ed25519 certificate");
        assert_eq!(Some(ed25519_spki_sha256(&public)), spki_sha256(&cert));
    }

    /// Not a certificate is `None`, never a digest of whatever arrived.
    #[test]
    fn input_that_is_not_a_certificate_has_no_pin() {
        assert_eq!(spki_sha256(b""), None);
        assert_eq!(spki_sha256(b"-----BEGIN CERTIFICATE-----"), None);
        assert_eq!(spki_sha256(&[0x30, 0x82, 0xff, 0xff]), None, "a plausible DER header is not a certificate");
    }

    /// **The plant, and the reason this module does not reuse `ed25519_key_from_cert_der`.**
    ///
    /// An attacker puts the *victim's* Ed25519 SPKI bytes somewhere they choose inside a
    /// certificate — here the serial number, which precedes the real `SubjectPublicKeyInfo` in the
    /// encoding and whose content is theirs to pick — and signs the certificate with their own key.
    ///
    /// The byte-scan extraction returns the **victim's** key, because it finds the first occurrence
    /// of the prefix and has no idea where it is in the structure. Pinning on that would accept an
    /// endpoint holding the attacker's private key as though it were the partner. The structural
    /// extraction returns the attacker's key, so the pin does not match and the handshake is
    /// refused.
    ///
    /// What this does **not** say: `ed25519_key_from_cert_der` is not wrong where it is used. Its
    /// input has already been validated against the cluster CA, which is exactly the premise that
    /// is absent here.
    #[test]
    fn a_planted_key_pattern_fools_the_byte_scan_and_not_the_structural_read() {
        let victim = ed25519_key();
        let victim_cert = cert_with(&victim, "partner.example");
        let victim_key = mycelium_core::tls::ed25519_key_from_cert_der(&victim_cert)
            .expect("the victim's own certificate carries an Ed25519 key");

        // `06 03 2B 65 70 03 21 00` — the Ed25519 SPKI prefix the scan looks for — then the key.
        let mut planted = vec![0x06, 0x03, 0x2B, 0x65, 0x70, 0x03, 0x21, 0x00];
        planted.extend_from_slice(&victim_key);

        let attacker = ed25519_key();
        let mut params = CertificateParams::new(vec!["partner.example".to_string()]).unwrap();
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        params.serial_number = Some(rcgen::SerialNumber::from(planted));
        let forgery = params.self_signed(&attacker).unwrap().der().to_vec();

        // The scan is fooled: it reports the victim's key from a certificate the victim never saw.
        assert_eq!(
            mycelium_core::tls::ed25519_key_from_cert_der(&forgery),
            Some(victim_key),
            "the plant is only decisive if the scan actually falls for it",
        );

        // The structural read is not: the pin is the attacker's key, so it does not match the
        // victim's pin and the endpoint is refused.
        let victim_pin = spki_sha256(&victim_cert).unwrap();
        let forged_pin = spki_sha256(&forgery).expect("the forgery is a well-formed certificate");
        assert_ne!(victim_pin, forged_pin, "the structural read must not be fooled by planted bytes");
        let attacker_cert = cert_with(&attacker, "attacker.example");
        assert_eq!(forged_pin, spki_sha256(&attacker_cert).unwrap(), "and it reads the key that actually signs");
    }

    /// The verifier accepts a pinned key and refuses an unpinned one, and the refusal carries the
    /// digest that arrived so an operator can compare it with the bundle by eye.
    #[test]
    fn the_verifier_accepts_only_a_pinned_key() {
        use rustls::client::danger::ServerCertVerifier;

        let partner = ed25519_key();
        let partner_cert = cert_with(&partner, "partner.example");
        let partner_pin = spki_sha256(&partner_cert).unwrap();
        let impostor_cert = cert_with(&ed25519_key(), "partner.example");
        let impostor_pin = spki_sha256(&impostor_cert).unwrap();

        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let verifier = PinnedServerVerifier::new(vec![partner_pin], provider);
        let name = rustls::pki_types::ServerName::try_from("partner.example").unwrap();
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_800_000_000));

        assert!(verifier
            .verify_server_cert(&CertificateDer::from(partner_cert.clone()), &[], &name, &[], now)
            .is_ok());

        // The same name, a valid certificate, a different key: refused.
        let err = verifier
            .verify_server_cert(&CertificateDer::from(impostor_cert), &[], &name, &[], now)
            .expect_err("an unpinned key must be refused");
        let TlsError::InvalidCertificate(CertificateError::Other(other)) = err else {
            panic!("expected a certificate refusal carrying the mismatch, got {err:?}");
        };
        let inner: &(dyn std::error::Error + 'static) = &*other.0;
        let mismatch = inner.downcast_ref::<PinMismatch>().expect("the refusal names what arrived");
        assert_eq!(mismatch.presented, impostor_pin);
        assert_eq!(mismatch.pins_configured, 1);
    }

    /// Two pins accept either key — the rotation window, and the reason the field is a list. Once
    /// the old pin is dropped the old key is refused, which is what makes the window *close*.
    #[test]
    fn two_pins_are_a_rotation_window_and_dropping_one_closes_it() {
        use rustls::client::danger::ServerCertVerifier;

        let old_cert = cert_with(&ed25519_key(), "partner.example");
        let new_cert = cert_with(&ed25519_key(), "partner.example");
        let (old_pin, new_pin) = (spki_sha256(&old_cert).unwrap(), spki_sha256(&new_cert).unwrap());

        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let name = rustls::pki_types::ServerName::try_from("partner.example").unwrap();
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_800_000_000));

        let during = PinnedServerVerifier::new(vec![old_pin, new_pin], std::sync::Arc::clone(&provider));
        for cert in [&old_cert, &new_cert] {
            assert!(during
                .verify_server_cert(&CertificateDer::from(cert.clone()), &[], &name, &[], now)
                .is_ok(), "both keys verify while the window is open");
        }

        let after = PinnedServerVerifier::new(vec![new_pin], provider);
        assert!(after
            .verify_server_cert(&CertificateDer::from(new_cert), &[], &name, &[], now)
            .is_ok());
        assert!(after
            .verify_server_cert(&CertificateDer::from(old_cert), &[], &name, &[], now)
            .is_err(), "the retired key stops working, or the window never closed");
    }

    /// An empty pin list accepts nothing. A verifier that fell back to "trust anything" when its
    /// configuration was empty would be the most dangerous possible reading of an operator's
    /// mistake.
    #[test]
    fn no_pins_accepts_nothing() {
        use rustls::client::danger::ServerCertVerifier;

        let cert = cert_with(&ed25519_key(), "partner.example");
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let verifier = PinnedServerVerifier::new(Vec::new(), provider);
        let name = rustls::pki_types::ServerName::try_from("partner.example").unwrap();
        let now = rustls::pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(1_800_000_000));
        assert!(verifier
            .verify_server_cert(&CertificateDer::from(cert), &[], &name, &[], now)
            .is_err());
    }
}
