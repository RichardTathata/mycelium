//! **Trust does not compose** — three domains, and the grants that do *not* follow.
//!
//! ```text
//! cargo run --example federation_trust_is_not_transitive --features tls
//! ```
//!
//! # The claim this exists to make checkable
//!
//! [`federated_domains`](federated_domains.rs) tells the two-domain story: identity, a filtered
//! catalogue, an authenticated call, budgets, revocation. Two domains cannot ask the question that
//! matters most once a federation is real, because it needs a **third**:
//!
//! > Alpha trusts Beta. Beta trusts Gamma. **Does Alpha trust Gamma?**
//!
//! The intuitive answer is the expensive one. Everything below is the substrate answering *no*, in
//! three different ways, each of which would be a breach if it answered otherwise.
//!
//! | # | The question | The answer, and why |
//! |---|---|---|
//! | 1 | Does a trust bundle compose? | **No** — a bundle is a list of keys an operator chose. Gamma is not in Alpha's, so Gamma's signature verifies under nothing |
//! | 2 | Can Beta re-export a grant it holds? | **No** — the credential names the *origin domain*, and Beta cannot mint one naming Gamma without Gamma's key |
//! | 3 | Is a per-partner budget shared? | **No** — it is counted per partner, so Gamma cannot exhaust Beta's allowance |
//!
//! # Why this is the right default, not a limitation
//!
//! Transitive trust is how one compromised partner becomes everybody's compromise. If Alpha
//! inherited Beta's trust list, then Beta adding a partner would silently grant that partner access
//! to Alpha — an authority decision Alpha's operator never made and cannot see.
//!
//! So a trust relationship here is **pairwise and explicit**: it holds between two domains because
//! two operators each said so. Non-merger is the property — joining a federation must not merge the
//! meshes, and must not merge the trust either.
//!
//! The cost is real and worth stating: Alpha *cannot* reach Gamma without its own relationship, so
//! federations grow by O(pairs) rather than by transitivity. That is the price of an authority
//! surface an operator can enumerate.

use mycelium::federation::{
    call::{verify_federated_call, CallPolicy, CallRefusal, FederatedCaller},
    edge::FederationEdge,
    DomainId, DomainPolicy, TrustBundle,
};
use ed25519_dalek::{Signer, SigningKey};

fn step(n: u8, title: &str) {
    println!("\n\x1b[1m{n}. {title}\x1b[0m");
}
fn note(s: impl AsRef<str>) {
    println!("   {}", s.as_ref());
}

fn keypair(seed: u8) -> (SigningKey, [u8; 32]) {
    let sk = SigningKey::from_bytes(&[seed; 32]);
    let vk = sk.verifying_key().to_bytes();
    (sk, vk)
}

fn main() {
    let alpha = DomainId::new("alpha.example").expect("domain id");
    let beta  = DomainId::new("beta.example").expect("domain id");
    let gamma = DomainId::new("gamma.example").expect("domain id");

    let (beta_sk,  beta_vk)  = keypair(2);
    let (gamma_sk, gamma_vk) = keypair(3);

    println!("\x1b[1mThree domains\x1b[0m");
    println!("   alpha  — the provider. Trusts beta, and only beta.");
    println!("   beta   — trusted by alpha. Also trusts gamma, for its own reasons.");
    println!("   gamma  — trusted by beta. A stranger to alpha.");

    // Alpha's bundle is the whole of Alpha's trust: one operator's explicit list.
    let alpha_bundle = TrustBundle::trusting([(beta.clone(), beta_vk)]);
    // Alpha grants beta exactly one export — and grants gamma nothing, because alpha has never
    // heard of gamma.
    let policy = DomainPolicy {
        domain: alpha.clone(),
        revision: 3,
        grants: vec![(beta.clone(), "delivery.book".into())],
    };
    let call_policy = CallPolicy::default();
    let now_ms = 1_789_000_000_000;

    let body = br#"{"jsonrpc":"2.0","method":"tasks/send","params":{"pallets":6}}"#;

    // ── 1. a bundle does not compose ────────────────────────────────────────────────────────
    step(1, "Gamma calls alpha directly, with a perfectly good credential");
    note("Gamma signs correctly, names itself honestly, and binds the body. Nothing is forged.");

    let gamma_cred = FederatedCaller {
        origin_domain: gamma.clone(),
        principal: "svc/logistics".into(),
        export: "delivery.book".into(),
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + 60_000,
        body_sha256: Some(FederatedCaller::digest_of(body)),
    };
    let gamma_sig = gamma_sk.sign(&gamma_cred.canonical_bytes()).to_bytes();

    match verify_federated_call(
        &gamma_cred, &gamma_sig, "delivery.book", body,
        &alpha_bundle, &policy, &call_policy, now_ms + 1_000,
    ) {
        Err(CallRefusal::UnknownDomain) =>
            note("refused: UnknownDomain — gamma is not in alpha's bundle, so its key means nothing here"),
        Ok(_) => panic!("BREACH: alpha accepted a domain its operator never trusted"),
        Err(other) => panic!("expected UnknownDomain, got {other}"),
    }
    note("✓ trust did not arrive through beta. Alpha's authority list is alpha's own.");

    // ── 2. a grant you hold is not a grant you can re-export ────────────────────────────────
    step(2, "Beta tries to pass its access along — the favour that would be a breach");
    note("Beta genuinely is trusted by alpha. It now vouches for gamma, meaning well.");

    // Beta can only sign as itself. Claiming gamma's origin means signing gamma's credential —
    // which needs gamma's key, which beta does not have.
    let forwarded = FederatedCaller {
        origin_domain: gamma.clone(),          // "this is really from gamma"
        principal: "svc/logistics".into(),
        export: "delivery.book".into(),
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + 60_000,
        body_sha256: Some(FederatedCaller::digest_of(body)),
    };
    let beta_signature_on_gammas_claim = beta_sk.sign(&forwarded.canonical_bytes()).to_bytes();

    match verify_federated_call(
        &forwarded, &beta_signature_on_gammas_claim, "delivery.book", body,
        &alpha_bundle, &policy, &call_policy, now_ms + 1_000,
    ) {
        Err(CallRefusal::UnknownDomain) =>
            note("refused: the credential says gamma, so alpha looks up gamma — and finds nothing"),
        Err(CallRefusal::BadSignature) =>
            note("refused: BadSignature — beta signed a claim only gamma's key could make"),
        Ok(_) => panic!("BREACH: a domain re-exported a grant it merely held"),
        Err(other) => panic!("unexpected: {other}"),
    }
    note("✓ the credential names its *origin*, and only that origin's key can mint one.");
    note("  Beta may call alpha all it likes. It cannot lend that to anybody.");

    // ── 3. and the budget is pairwise too ───────────────────────────────────────────────────
    step(3, "Budgets are counted per partner, so a stranger cannot spend beta's allowance");

    // An edge that will carry at most ONE call per partner at a time — small enough to exhaust
    // in a demonstration, and the same mechanism at any limit.
    let metered = CallPolicy { max_in_flight_per_partner: 1, ..CallPolicy::default() };
    let edge = std::sync::Arc::new(FederationEdge::new(
        alpha.clone(),
        ["delivery.book"],
        DomainPolicy { domain: alpha.clone(), revision: 3, grants: vec![] },
        TrustBundle::trusting([(beta.clone(), beta_vk), (gamma.clone(), gamma_vk)]),
        metered,
    ));
    note("edge carries max_in_flight_per_partner = 1");

    // Gamma takes its one slot and holds it.
    let gamma_slot = edge.admit(&gamma).expect("gamma's first call is admitted");
    note("gamma holds its slot");

    // Gamma asking again is refused — transiently, and it says so.
    match edge.admit(&gamma) {
        Err(CallRefusal::AtCapacity { partner, limit }) =>
            note(format!("gamma refused: AtCapacity {{ partner: {partner}, limit: {limit} }}")),
        Ok(_)      => panic!("REGRESSION: the per-partner limit did not hold"),
        Err(other) => panic!("expected AtCapacity, got {other}"),
    }

    // And here is the property: beta is entirely unaffected by gamma's saturation.
    let _beta_slot = edge.admit(&beta)
        .expect("BREACH: one partner exhausting its budget must not refuse another");
    note("beta admitted anyway — the meter is keyed by partner, not shared");

    drop(gamma_slot);
    note("gamma's slot released on drop (RAII), so its next call is admitted again");
    edge.admit(&gamma).expect("released slot is reusable");

    note("✓ AtCapacity is transient load, never NotPermitted — which is a standing answer about");
    note("  authority that a retry will not change. Telling a partner to go and ask their");
    note("  operator about a grant they already hold is its own kind of outage.");

    // ── what it costs ───────────────────────────────────────────────────────────────────────
    println!("\n\x1b[1mWhat this costs, stated plainly\x1b[0m");
    note("Alpha cannot reach gamma at all without its own relationship. Federations grow by");
    note("pairs, not by transitivity — and that is the price of an authority surface an operator");
    note("can enumerate. If alpha inherited beta's trust list, beta adding a partner would");
    note("silently grant that partner access to alpha: an authority decision alpha never made,");
    note("and could not see.");

    println!("\n\x1b[1mAlpha trusts beta. Beta trusts gamma. Alpha does not trust gamma.\x1b[0m");
}
