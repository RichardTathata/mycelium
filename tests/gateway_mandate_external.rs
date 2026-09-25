//! **An execution authority built from outside the crate**, as every operator's will be.
//!
//! A separate crate linking `mycelium`: it compiles only if a deployment can actually construct an
//! `ExecutionAuthority`, attach it with `with_execution_authority`, and have a presented grant
//! established — through public paths alone. The first cut of the gateway wiring could not: the type
//! lived in a crate-private module with no re-export, so the runbook's own snippet would not have
//! compiled. This test is the gate that keeps it constructible.
#![cfg(all(feature = "gateway", feature = "tls"))]

use base64::Engine;
use ed25519_dalek::SigningKey;
use mycelium::knowledge::issuer::{MemberKeys, TrustedExternalIssuers};
use mycelium::knowledge::IssuerId;
use mycelium::mandate::authority::{
    ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier, RevocationCheckpoint, SignedRevocationCheckpoint,
};
use mycelium::mandate::grant::{possession_message, EntitlementTable, GrantVerifier, SignedMandateGrant};
use mycelium::mandate::{Mandate, PrincipalId, ResourceAuthority, TermId};
use mycelium::{
    mandate_operation, possession_request, ExecutionAuthority, GossipAgent, GossipConfig, MandateState, NodeId,
    PresentedMandate,
};
use std::collections::HashMap;
use std::sync::Arc;

#[test]
fn an_operator_can_build_attach_and_use_an_execution_authority() {
    const CALLER: &str = "token:gw/agent-1";
    let (authority_key, holder_key) = (SigningKey::from_bytes(&[61u8; 32]), SigningKey::from_bytes(&[62u8; 32]));
    let (op, res) = ("skill.invoke", "skill:depot/dispatch@10.0.0.1:57000");

    let mut entitled = EntitlementTable::new();
    entitled.entitle("depot", PrincipalId::new("operator:acme").unwrap());
    let mut external = TrustedExternalIssuers::new();
    external.trust(IssuerId::new("operator:acme").unwrap(), authority_key.verifying_key().to_bytes()).unwrap();
    external.trust(IssuerId::new(CALLER).unwrap(), holder_key.verifying_key().to_bytes()).unwrap();
    let gate = ExecutionGate::strict(
        ResourceAuthority::new("depot", 1),
        ResourceTier::Serialised,
        ClockModel { skew_ms: 100 },
        FreshnessPolicy { freshness_ms: 60_000, interval_ms: 20_000, delivery_ms: 5_000 },
    )
    .unwrap();
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitled), external));

    // Attachable to a real agent.
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", 47_999).unwrap(), GossipConfig::default());
    agent.with_execution_authority(Arc::clone(&authority));

    let no_members: HashMap<NodeId, MemberKeys> = HashMap::new();
    let c = RevocationCheckpoint {
        authority: PrincipalId::new("operator:acme").unwrap(),
        scope: "depot".into(),
        seq: 1,
        issued_at_ms: 1_000,
        revoked: Default::default(),
    };
    let signed = SignedRevocationCheckpoint { signature: sign(&authority_key, &c.canonical_bytes()), checkpoint: c };
    authority.offer_checkpoint(&signed, 1_000, &no_members);

    let mandate = Mandate {
        holder: PrincipalId::new(CALLER).unwrap(),
        established_by: PrincipalId::new("operator:acme").unwrap(),
        purpose: "dispatch".into(),
        scope: "depot".into(),
        operations: vec![mandate_operation(op, res)],
        epoch: 1,
        term: TermId::new("t1").unwrap(),
        valid_from_ms: 0,
        valid_until_ms: 50_000,
    };
    let grant = SignedMandateGrant { signature: sign(&authority_key, &mandate.canonical_bytes()), mandate };
    let digest = [5u8; 32];
    let proof = sign(&holder_key, &possession_message(&grant.mandate, &possession_request(op, res, &digest)));
    let presented = PresentedMandate { grant, possession: base64::engine::general_purpose::STANDARD.encode(proof) };

    let a = authority.assess(CALLER, op, res, &digest, Some(&presented), 2_000, &no_members);
    assert_eq!(a.binding.map(|b| b.state), Some(MandateState::Established));
}

fn sign(key: &SigningKey, msg: &[u8]) -> Vec<u8> {
    use ed25519_dalek::Signer;
    key.sign(msg).to_bytes().to_vec()
}
