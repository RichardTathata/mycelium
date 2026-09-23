//! Two federated domains, end to end — the item 2 contract as a runnable narrative.
//!
//! ```text
//! cargo run --example federated_domains --features tls
//! ```
//!
//! # What this demonstrates, and what it does not
//!
//! **Does:** every decision item 2 defines — a signed descriptor, a revisioned policy, a filtered
//! catalog, an authenticated call bound to one export, per-partner budgets, failover that respects
//! repeatability, a partition/reconnect cycle, a key rotation, and a revocation.
//!
//! **Does not:** move a byte over a network. PRs 1–6 built the contract and this example exercises
//! it in one process; the transport's first arm (PR 8, `federation::edge` + `federation::client`)
//! is exercised by the two-mesh test in `src/lib_tests.rs`, not here. The record's release gate
//! (§13) is a *two-mesh demonstration* proving from membership tables, consensus state and traces
//! that the meshes never merged; that gate is **not** claimed here, and saying so is the point.
//! What is claimed is narrower and checkable: given these inputs, these are the decisions.
//!
//! Every step prints what it decided and why, so the output reads as the argument rather than as a
//! log of a thing that worked.

use mycelium::federation::{
    call::{verify_federated_call, CallPolicy, CallRefusal, FederatedCaller},
    catalog::{filtered_catalog, CatalogObservation, RemoteResolver, ResolveFailure},
    gateway::{on_gateway_silent, CallOutcome, GatewayPool, Repeatability},
    pinning::spki_sha256,
    session::{LinkRefusal, PartnerLink},
    DomainDescriptor, DomainId, DomainPolicy, TrustBundle,
};
use std::time::{Duration, Instant};

/// A sub-step of the one before it — same subject, sharper question.
fn substep(label: &str, title: &str) {
    println!("\n\x1b[1m{label}. {title}\x1b[0m");
}

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}

fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

fn main() {
    let alpha = DomainId::new("alpha.example").expect("valid domain id");
    let beta = DomainId::new("beta.example").expect("valid domain id");

    println!("\x1b[1mTwo federated domains — alpha (provider) and beta (consumer)\x1b[0m");
    println!("Alpha exports three services. Beta is granted exactly one of them.");

    // ── 1. identity ───────────────────────────────────────────────────────────────────────────
    step(1, "Alpha's descriptor: what it is, and what it exports");

    let (signing, verifying) = keypair();
    let descriptor = DomainDescriptor {
        domain: alpha.clone(),
        public_key: verifying,
        issued_at_ms: 1_789_000_000_000,
        policy_revision: 3,
        exports: vec![
            "invoice.submit".into(),
            "invoice.status".into(),
            "ledger.audit".into(),
        ],
    };
    note(format!("exports: {:?}", descriptor.exports));
    note(format!(
        "signed over {} canonical bytes, tagged so a policy signature can never authenticate it",
        descriptor.canonical_bytes().len()
    ));

    // ── 2. the filtered catalog ───────────────────────────────────────────────────────────────
    step(2, "What beta is allowed to SEE");

    let policy = DomainPolicy {
        domain: alpha.clone(),
        revision: 3,
        grants: vec![(beta.clone(), "invoice.submit".into())],
    };
    let visible = filtered_catalog(&descriptor.exports, &policy, &beta);
    note(format!("beta sees: {visible:?}"));
    note("`ledger.audit` is ABSENT, not refused — a catalog is a disclosure, and telling a partner");
    note("that a capability exists has already told it something about this domain.");

    // ── 3. discovery ──────────────────────────────────────────────────────────────────────────
    step(3, "Beta's gateway observes that catalog");

    let mut resolver = RemoteResolver::new(Duration::from_secs(60));
    resolver.observe(CatalogObservation {
        partner: alpha.clone(),
        observed_by: "beta-gw-1".into(),
        observed_at: Instant::now(),
        exports: visible.clone(),
    });
    match resolver.resolve(&alpha, "invoice.submit") {
        Ok(cap) => note(format!("resolved {}/{}", cap.domain, cap.export)),
        Err(e) => note(format!("unexpected: {e}")),
    }
    match resolver.resolve(&alpha, "ledger.audit") {
        Err(ResolveFailure::NotExported) => {
            note("`ledger.audit` does not resolve — beta never saw it, because it was never shown")
        }
        other => note(format!("unexpected: {other:?}")),
    }

    // ── 4. the authenticated call ─────────────────────────────────────────────────────────────
    step(4, "Beta calls — and the credential names the call, not just the caller");

    let now_ms = 1_789_000_010_000;
    // The request beta is actually making. The credential binds *these bytes* — see step 4b.
    let body = br#"{"jsonrpc":"2.0","method":"tasks/send","params":{"invoice":"INV-4471","amount_pence":21900}}"#;
    let credential = FederatedCaller {
        origin_domain: beta.clone(),
        principal: "svc/billing".into(),
        export: "invoice.submit".into(),
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + 60_000,
        body_sha256: Some(FederatedCaller::digest_of(body)),
    };
    let signature = sign(&signing, &credential.canonical_bytes());
    let bundle = TrustBundle::trusting([(beta.clone(), verifying)]);
    let call_policy = CallPolicy::default();

    match verify_federated_call(
        &credential, &signature, "invoice.submit", body, &bundle, &policy, &call_policy, now_ms + 1_000,
    ) {
        Ok(accepted) => {
            note(format!(
                "accepted — the provider is handed origin={} principal={:?}, not \"the gateway\"",
                accepted.origin_domain, accepted.principal
            ));
            note(format!("and policy revision {} is recorded as what permitted it", accepted.policy_revision));
        }
        Err(e) => note(format!("unexpected refusal: {e}")),
    }

    note("");
    note("The same credential, used for a different export:");
    match verify_federated_call(
        &credential, &signature, "invoice.status", body, &bundle, &policy, &call_policy, now_ms + 1_000,
    ) {
        Err(CallRefusal::WrongExport { authorised, requested }) => note(format!(
            "refused — authorises {authorised:?}, asked for {requested:?}. Binding only the caller",
        )),
        other => note(format!("unexpected: {other:?}")),
    }
    note("would leave the deputy confused about WHAT, having fixed WHO.");

    // ── 4b. the payload, changed in flight ────────────────────────────────────────────────────
    substep("4b", "…and the credential names the PAYLOAD, or the deputy is still confused");

    note("The credential above authorises invoice INV-4471 for £219.00.");
    note("Someone on the path between the two domains rewrites the amount and forwards it:");
    let tampered = br#"{"jsonrpc":"2.0","method":"tasks/send","params":{"invoice":"INV-4471","amount_pence":2190000}}"#;
    match verify_federated_call(
        &credential, &signature, "invoice.submit", tampered, &bundle, &policy, &call_policy, now_ms + 1_000,
    ) {
        Err(CallRefusal::BodyMismatch) => {
            note("refused — the credential binds a different body than the one that arrived.");
            note("The header is untouched and still verifies: the signature is genuine, the");
            note("principal is real, the export is right. Only the payload moved — which until");
            note("this binding existed was enough, and the £21,900 invoice would have been");
            note("accepted as authentically beta's, then recorded as such in the evidence.");
        }
        other => note(format!("unexpected: {other:?}")),
    }
    note("");
    note("A partner that predates the binding sends no digest at all. That is accepted while");
    note("`CallPolicy::require_body_binding` is false — a rolling-upgrade window, and one that");
    note("gives no protection against an attacker, who would simply strip the field. Turn it on");
    note("once every partner has upgraded.");

    // ── 5. budgets and failover ───────────────────────────────────────────────────────────────
    step(5, "Two gateways, per-partner budgets, and what a silence means");

    let mut pool = GatewayPool::new(["gw-1", "gw-2"], 1);
    let gw = pool.admit(&beta, &[]).expect("a free slot");
    note(format!("admitted on {gw} (fixed local order — no election, no leader)"));

    let mut attempted = vec![gw];
    note("gw-1 goes silent mid-call…");
    match on_gateway_silent(&mut pool, &beta, Repeatability::AtMostOnce, &mut attempted, "timeout") {
        Err(CallOutcome::DeliveryUnknown { attempted_via, .. }) => {
            note(format!("at-most-once: NOT failed over. attempted={attempted_via:?}"));
            note("The far side may have run it, so the caller is told UNKNOWN — which is true —");
            note("rather than FAILED, which would be a guess.");
        }
        other => note(format!("unexpected: {other:?}")),
    }

    let mut pool2 = GatewayPool::new(["gw-1", "gw-2"], 1);
    let g = pool2.admit(&beta, &[]).unwrap();
    let mut tried = vec![g];
    if let Ok(next) = on_gateway_silent(&mut pool2, &beta, Repeatability::Repeatable, &mut tried, "timeout") {
        note(format!("repeatable: failed over to {next} — doing it twice is, by declaration, harmless"));
    }

    // ── 6. partition and reconnect ────────────────────────────────────────────────────────────
    step(6, "The link drops, and comes back");

    let mut link = PartnerLink::new(alpha.clone());
    link.connected();
    link.discovery_refreshed();
    note(format!("ready: admit() -> {:?}", link.admit().is_ok()));

    link.disconnected();
    resolver.forget(&alpha);
    note("disconnected — discovery is DISCARDED, not left to age out");

    link.connected();
    match link.admit() {
        Err(LinkRefusal::Refreshing { .. }) => {
            note("reconnected, and work is still REFUSED: the catalogue predates the partition,");
            note("and whatever changed while we were down is exactly what we'd be acting on.");
        }
        other => note(format!("unexpected: {other:?}")),
    }
    link.discovery_refreshed();
    note(format!("discovery refreshed: admit() -> {:?}", link.admit().is_ok()));

    // ── 7. rotation, then revocation ──────────────────────────────────────────────────────────
    step(7, "Beta rotates its key, then alpha revokes it");

    let mut bundle = TrustBundle::trusting([(beta.clone(), verifying)]);
    let new_key = [7u8; 32];
    bundle.rotate(&beta, new_key, now_ms + 30_000);
    note(format!(
        "during the window both keys verify ({} accepted); past it, only the new one ({})",
        bundle.acceptable_keys(&beta, now_ms).len(),
        bundle.acceptable_keys(&beta, now_ms + 30_001).len()
    ));
    note("Bounded on purpose: an unbounded overlap is not a rotation, it is two keys —");
    note("and the compromised one is still among them.");

    bundle.revoke(&beta);
    note(format!(
        "revoked: acceptable keys = {}, is_revoked = {}",
        bundle.acceptable_keys(&beta, now_ms).len(),
        bundle.is_revoked(&beta)
    ));
    note("A tombstone, not a deletion: \"we used to trust them and stopped\" is a different fact");
    note("from \"we never heard of them\", and an operator reading a refusal needs to know which.");

    step(8, "The endpoint is pinned: who answers, not just who signs");
    note("Everything above is signed and none of it is encrypted. The credential makes a call");
    note("unforgeable; it does nothing to stop an on-path observer READING it. TLS closes that,");
    note("and TLS needs a trust anchor — of which this design has none, because partner trust");
    note("here is an Ed25519 key an operator chose, with no X.509 material anywhere.");
    note("So the anchor is the bundle: pin the sha256 of the endpoint's SubjectPublicKeyInfo.");
    // Beta's own bundle this time: the side that *dials* is the side that pins. (The demo reuses
    // one keypair throughout, so `verifying` stands in for alpha's signing key here.)
    let mut beta_bundle = TrustBundle::trusting([(alpha.clone(), verifying)]);
    let partner_cert = self_signed_cert("gw-1.alpha.example");
    let impostor_cert = self_signed_cert("gw-1.alpha.example");
    let partner_pin = spki_sha256(&partner_cert).expect("a certificate we just generated");
    beta_bundle.pin_tls(&alpha, partner_pin);
    note(format!(
        "alpha's pin recorded beside its key: {} pin(s), first bytes {:02x?}",
        beta_bundle.tls_pins_for(&alpha).len(),
        &partner_pin[..4],
    ));
    let impostor_pin = spki_sha256(&impostor_cert).expect("also a certificate");
    note(format!(
        "an endpoint with the SAME NAME and a different key: pinned={} — refused, and nothing is sent",
        beta_bundle.tls_pins_for(&alpha).contains(&impostor_pin),
    ));
    note("The name matched and it was refused anyway, which is the point: a name is checked");
    note("against an authority, and there is no authority here — there is one key, written down.");

    println!("\n\x1b[1mWhat was NOT demonstrated\x1b[0m");
    println!("   No bytes crossed a network *in this example*. The transport exists — the provider");
    println!("   side is `federation::edge`, the consumer side `federation::client` — and it is");
    println!("   exercised by the two-mesh tests in `src/lib_tests.rs` and the Docker suite");
    println!("   (`make test-federation`), which is where the record's release gate is met.");
    println!("   Step 8 compares pins in process; it completes no handshake. The gate that does is");
    println!("   `a_pinned_federation_link_talks_only_to_the_key_the_bundle_names`, two real HTTPS");
    println!("   gateways differing only in their TLS key.");
}

/// A self-signed certificate for `name`, as a partner's gateway endpoint would present.
#[cfg(feature = "tls")]
fn self_signed_cert(name: &str) -> Vec<u8> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519).expect("keypair");
    let mut params = rcgen::CertificateParams::new(vec![name.to_string()]).expect("params");
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after = rcgen::date_time_ymd(2099, 1, 1);
    params.self_signed(&key).expect("self-signed").der().to_vec()
}

// ── signing helpers ───────────────────────────────────────────────────────────────────────────

#[cfg(feature = "tls")]
fn keypair() -> (ed25519_dalek::SigningKey, [u8; 32]) {
    let sk = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let pk = sk.verifying_key().to_bytes();
    (sk, pk)
}

#[cfg(feature = "tls")]
fn sign(sk: &ed25519_dalek::SigningKey, msg: &[u8]) -> Vec<u8> {
    mycelium_core::tls::sign_bytes(sk, msg).to_vec()
}
