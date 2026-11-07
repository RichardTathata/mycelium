//! Spike S-presence verifier (throwaway). Reads `pub_x963_hex`, `sig_der_hex` and `msg_hex` lines
//! from a probe's output and checks them three ways.
use std::collections::HashMap;

fn hex(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }

struct SePublic(Vec<u8>);
impl rcgen::PublicKeyData for SePublic {
    fn der_bytes(&self) -> &[u8] { &self.0 }
    fn algorithm(&self) -> &rcgen::SignatureAlgorithm { &rcgen::PKCS_ECDSA_P256_SHA256 }
}

fn main() {
    let path = std::env::args().nth(1).expect("probe output");
    let prefix = std::env::args().nth(2).unwrap_or_default();
    let text = std::fs::read_to_string(path).unwrap();
    let m: HashMap<String, String> = text.lines().filter_map(|l| l.split_once('\t'))
        .filter(|(k, _)| k.starts_with(&prefix)).map(|(k, v)| (k.trim_start_matches(&prefix).to_string(), v.to_string())).collect();
    let (pubk, sig, msg) = (hex(&m["pub_x963_hex"]), hex(&m["sig_der_hex"]), hex(&m["msg_hex"]));

    // 1. ring, the verifier already in the tree under `tls`.
    let pk = ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_ASN1, &pubk);
    println!("ring.verify\t{:?}", pk.verify(&msg, &sig).map(|_| "ok"));
    let mut bad = msg.clone(); bad[0] ^= 1;
    println!("ring.verify_tampered_msg\t{:?}", pk.verify(&bad, &sig).map(|_| "ACCEPTED (defect)"));

    // 2. An Ed25519 fleet CA (as `mycelium tls` makes) issues a certificate whose key is the SE key.
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).unwrap();
    let mut ca_p = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_p.distinguished_name.push(rcgen::DnType::CommonName, "spike fleet CA");
    let ca = ca_p.self_signed(&ca_key).unwrap();
    let mut leaf_p = rcgen::CertificateParams::new(vec!["dev.spike.invalid".to_string()]).unwrap();
    leaf_p.distinguished_name.push(rcgen::DnType::CommonName, "mycelium-client://dev");
    leaf_p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
    let leaf = leaf_p.signed_by(&SePublic(pubk.clone()), &ca, &ca_key).unwrap();
    println!("rcgen.issue_over_se_key\tok der_len={}", leaf.der().len());

    // 3. webpki (rustls's own verifier): the chain Ed25519 CA -> P-256 leaf, then the SE signature
    //    verified against the certificate's key.
    use rustls_pki_types::{CertificateDer, UnixTime};
    let ca_der = CertificateDer::from(ca.der().to_vec());
    let anchor = webpki::anchor_from_trusted_cert(&ca_der).unwrap().to_owned();
    let leaf_der = CertificateDer::from(leaf.der().to_vec());
    let ee = webpki::EndEntityCert::try_from(&leaf_der).unwrap();
    let anchors = [anchor];
    let chain = ee.verify_for_usage(
        &[webpki::ring::ED25519, webpki::ring::ECDSA_P256_SHA256],
        &anchors, &[], UnixTime::now(), webpki::KeyUsage::client_auth(), None, None);
    println!("webpki.chain_ed25519_ca_to_p256_leaf\t{:?}", chain.map(|_| "ok"));
    println!("webpki.verify_se_signature\t{:?}", ee.verify_signature(webpki::ring::ECDSA_P256_SHA256, &msg, &sig).map(|_| "ok"));
    println!("webpki.verify_se_signature_as_ed25519\t{:?}", ee.verify_signature(webpki::ring::ED25519, &msg, &sig).map(|_| "ACCEPTED (defect)"));
}
