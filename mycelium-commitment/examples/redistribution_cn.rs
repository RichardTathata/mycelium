//! Worked example — the surplus-food redistribution workload, run as a **contract net**.
//!
//! The same donations `mycelium-tuple-space`'s `redistribution` moves through a competitive `take`
//! pipeline, and `mycelium-blackboard`'s examples share as facts, are here coordinated by
//! **commitment**: the hub *announces* each pickup; drivers *offer* against the pickups they can
//! reach; the hub *awards* each pickup once, by the deterministic lowest-participant rule, and the
//! award is a **receipt-bearing write**; the awardee *reports* against the award's operation; a
//! food-bank auditor — not the hub — *assesses*, and signs. Five records, a mechanism each, no
//! dispatcher and no planner. The three coordination models are thereby compared on one workload.
//!
//! What is shown, and asserted:
//! - every pickup is awarded **exactly once**, and a second award attempt is refused, not overwritten;
//! - a contested pickup goes to the lowest participant, the uncontested ones to their only offerer;
//! - every award carries a receipt whose `application` is `Applied` under `cn/{pickup}/award`;
//! - every award has a report tied to its operation id, and a signed assessment that verifies under
//!   the auditor's key and under no other; an unsigned assessment never verifies.
//!
//! Single node, several participants — the model, not the topology, is what this example is about.
//! Run: `cargo run -p mycelium-commitment --example redistribution_cn`. Exits 0 on success.

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use mycelium::{GossipAgent, GossipConfig, LocalApplication, NodeId};
use mycelium_commitment::{verify_assessment, verify_award, verify_close, AwardRule, CommitmentRefusal, ContractNet, Offer, Outcome};

const PICKUPS: usize = 6;
/// How long offers stay open, on the hub's wall clock. Short, so the example runs in well under a second
/// while still closing *after* its deadline rather than claiming to.
const WINDOW_MS: u64 = 300;

/// Sleep until the hub's clock reads `deadline_ms`, so the close is honestly at or after it.
async fn wait_for(deadline_ms: u64) {
    let now = now_ms();
    if deadline_ms > now {
        tokio::time::sleep(std::time::Duration::from_millis(deadline_ms - now)).await;
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[tokio::main]
async fn main() {
    let port = mycelium::test_util::alloc_port();
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).unwrap(),
        GossipConfig { bind_port: port, ..Default::default() },
    ));
    agent.start().await.unwrap();

    let hub = ContractNet::new(Arc::clone(&agent), "hub");
    let drivers: Vec<ContractNet> =
        ["driver-a", "driver-b", "driver-c"].iter().map(|d| ContractNet::new(Arc::clone(&agent), *d)).collect();
    let auditor = ContractNet::new(Arc::clone(&agent), "food-bank-auditor");
    let auditor_key = SigningKey::from_bytes(&[11u8; 32]);
    // Each driver holds its own key, and the hub knows which key belongs to which driver. In a
    // deployment this directory is the operator's — `sys/identity/{node}` for nodes, whatever the
    // co-op already uses for people and vehicles. The companion does not mint identity.
    let driver_keys: Vec<SigningKey> = (0..drivers.len()).map(|i| SigningKey::from_bytes(&[20 + i as u8; 32])).collect();
    let directory: std::collections::HashMap<String, [u8; 32]> = drivers
        .iter()
        .zip(&driver_keys)
        .map(|(d, k)| (d.me().to_string(), k.verifying_key().to_bytes()))
        .collect();
    let resolve = |participant: &str| directory.get(participant).copied();
    // The hub signs its closes and awards; a participant checking them pins "hub" and this key.
    let hub_key = SigningKey::from_bytes(&[12u8; 32]);
    let resolve_with_hub = |name: &str| if name == "hub" { Some(hub_key.verifying_key().to_bytes()) } else { resolve(name) };

    // ── Announce: the hub declares each pickup — terms, a deadline for offers, what satisfies it.
    let t0 = now_ms();
    let pickups: Vec<String> = (0..PICKUPS).map(|i| format!("pickup-{i}")).collect();
    for (i, p) in pickups.iter().enumerate() {
        hub.announce(p, format!("{} crates from donor {i}", 1 + i % 3), "delivered to the pantry before close", t0 + WINDOW_MS, t0);
    }
    println!("hub: announced {PICKUPS} pickups");

    // ── Offer: each driver offers on the pickups it can reach. Pickup 0 is contested by every driver;
    //    the rest go to one driver each — nobody is assigned anything.
    let mut offers = 0;
    for (d, driver) in drivers.iter().enumerate() {
        for (i, p) in pickups.iter().enumerate() {
            if i == 0 || i % drivers.len() == d {
                driver
                    .offer_signed(p, format!("{} km", 2 + (i + d) % 5), Some(&driver_keys[d]), now_ms())
                    .expect("announced");
                offers += 1;
            }
        }
    }
    println!("drivers: {offers} offers made");

    // ── Close: once its own clock passes the deadline, the hub says which offers arrived in time — every
    //    offer it holds, by log position and digest. No stamp a driver writes decides lateness.
    wait_for(t0 + WINDOW_MS).await;
    for p in &pickups {
        let close = hub.close(p, None, now_ms()).expect("the hub closes at or after the deadline");
        assert!(!close.offers.is_empty(), "every pickup had an offer by its close");
    }
    println!("hub: closed {PICKUPS} pickups after the deadline");

    // ── Award: one per pickup, by the rule over the offers the close names, each a receipt-bearing write.
    let mut awards = Vec::new();
    for p in &pickups {
        let awarded = hub.award(p, AwardRule::LowestParticipant, now_ms()).await.expect("every pickup had an offer");
        assert_eq!(awarded.receipt.application, LocalApplication::Applied, "the award is a receipt, not a hopeful write");
        assert_eq!(awarded.receipt.operation_id.as_str(), format!("cn/{p}/award"));
        println!("hub: awarded {p} → {} (receipt {}, durability {:?})",
            awarded.award.participant, awarded.receipt.attempt_id, awarded.receipt.local_durability);
        awards.push(awarded.award);
    }
    assert_eq!(awards[0].participant, "driver-a", "the contested pickup goes to the lowest participant");
    for (i, a) in awards.iter().enumerate().skip(1) {
        assert_eq!(a.participant, drivers[i % drivers.len()].me(), "an uncontested pickup goes to its only offerer");
    }
    match hub.award(&pickups[0], AwardRule::LowestParticipant, now_ms()).await {
        Err(CommitmentRefusal::AlreadyAwarded(existing)) => {
            println!("hub: a second award of {} refused — it stands with {}", pickups[0], existing.participant);
        }
        other => panic!("a second award must be refused, got {other:?}"),
    }

    // ── Report: each awardee reports against its award's operation id.
    for a in &awards {
        let driver = drivers.iter().find(|d| d.me() == a.participant).unwrap();
        driver.report(&a.requirement, Outcome::Fulfilled, a.operation_id.clone(), now_ms());
    }
    // ── Assess: the auditor, not the hub, signs off — and one unsigned view for contrast.
    for a in &awards {
        auditor.assess(&a.requirement, true, "pantry confirmed receipt", Some(&auditor_key), now_ms());
    }
    hub.assess(&pickups[0], true, "the hub's own, unsigned view", None, now_ms());

    let public = auditor_key.verifying_key().to_bytes();
    for a in &awards {
        let reports = hub.reports(&a.requirement);
        assert_eq!(reports.len(), 1, "one report per award");
        assert_eq!(reports[0].operation_id, a.operation_id, "tied to this acceptance and no other");
        let assessments = hub.assessments(&a.requirement);
        let signed = assessments.iter().find(|x| x.assessor == "food-bank-auditor").expect("the auditor assessed");
        assert!(verify_assessment(signed, &public), "verifies under the auditor's key");
        assert!(!verify_assessment(signed, &[0u8; 32]), "and under no other");
    }
    let unsigned = hub.assessments(&pickups[0]).into_iter().find(|x| x.assessor == "hub").unwrap();
    assert!(!unsigned.is_signed() && !verify_assessment(&unsigned, &public), "unsigned means unproven");

    // ── Provenance: an offer nobody made.
    //
    // A member of the co-op's mesh posts an offer on a fresh pickup NAMING driver-a, signed with
    // its own key. `driver-a` sorts lowest, so the deterministic rule would hand driver-a a job
    // they never offered to do — and every reader checking the award against the offers would
    // agree it was correct. This is the crate's first rule (*no component assigns another
    // participant's obligation*) being defeated by a field anyone can write.
    let contested = "pickup-forged";
    let t1 = now_ms();
    hub.announce(contested, "4 crates from the market", "delivered to the pantry before close", t1 + WINDOW_MS, t1);
    drivers[1].offer_signed(contested, "6 km", Some(&driver_keys[1]), now_ms()).expect("announced");
    let forged = {
        use ed25519_dalek::Signer;
        let stranger = SigningKey::from_bytes(&[99u8; 32]);
        let o = Offer {
            requirement: contested.to_string(),
            participant: "driver-a".to_string(),
            bid: "0 km".to_string(),
            offered_at_ms: now_ms(),
            signature: Vec::new(),
        };
        let sig = stranger.sign(&o.canonical_bytes()).to_bytes().to_vec();
        Offer { signature: sig, ..o }
    };
    agent.kv().append(&format!("cn/{contested}/offers"), serde_json::to_vec(&forged).unwrap());
    // The hub closes this one **signed**, so a checker can hold the close to the hub's key.
    wait_for(t1 + WINDOW_MS).await;
    let close = hub.close(contested, Some(&hub_key), now_ms()).expect("the hub closes; the forgery arrived in time, so it is named");
    assert!(verify_close(&close, &hub_key.verifying_key().to_bytes()), "a signed close proves who decided what was in time");

    let unchecked = hub.plan_award(contested, AwardRule::LowestParticipant, now_ms()).expect("plans");
    assert_eq!(unchecked.participant, "driver-a", "unchecked, the forged offer wins");
    // The checked path: the close must verify under the hub's key, and only offers their participants
    // signed are candidates.
    let checked = hub
        .plan_award_verified(contested, AwardRule::LowestParticipant, now_ms(), "hub", resolve_with_hub)
        .expect("plans on the checked path");
    assert_eq!(checked.participant, "driver-b", "checked, only an offer its participant signed is a candidate");
    assert_eq!(checked.close_sha256, close.sha256(), "and the award names the close it was chosen from");
    let awarded = hub.commit_award(checked.signed(&hub_key)).await.expect("commits");
    assert!(verify_award(&awarded.award, &hub_key.verifying_key().to_bytes()), "and the award proves which declarer made it");
    println!("hub: a forged offer naming driver-a was not a candidate — {contested} → {}", awarded.award.participant);

    println!("{PICKUPS} pickups: announced, offered, awarded once each with a receipt, reported, and assessed by a third party.");
    agent.shutdown().await;
    println!("All assertions passed");
}
