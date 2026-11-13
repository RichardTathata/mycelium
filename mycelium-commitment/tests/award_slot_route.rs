//! A committed award, asked for through the node's own routes (the slot decision of 2026-11-12).
//!
//! `commit_award_linearizable` decides the consensus slot `cn/{requirement}/award`. The gateway has two routes
//! that take a slot name from the caller: `GET /consensus/{slot}`, which until this decision answered the award —
//! requirement, awardee and declarer — to any `consensus:read` token, and
//! `POST /gateway/consensus/cross_group_propose`, through which a `consensus:write` token could decide an
//! award's slot before the declarer did. Both refuse the slot now, and the declarer's own path is untouched.

use std::sync::Arc;
use std::time::Duration;

use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_commitment::{AwardRule, CommitmentRefusal, ContractNet};

const FAR_DEADLINE: u64 = 1_000;

async fn node() -> (Arc<GossipAgent>, String, reqwest::Client) {
    let (gossip_port, http_port) = (mycelium::test_util::alloc_port(), mycelium::test_util::alloc_port());
    let cfg = GossipConfig { bind_port: gossip_port, http_port: Some(http_port), ..Default::default() };
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.expect("the node starts");
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while client.get(format!("{base}/health")).send().await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "the gateway did not come up");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (agent, base, client)
}

fn announced_and_closed(hub: &ContractNet, driver: &ContractNet, requirement: &str) {
    hub.announce(requirement, "2 crates, 14:00", "delivered before 15:00", FAR_DEADLINE, 0);
    assert!(driver.offer(requirement, "5 km", 1).is_some());
    hub.close(requirement, None, FAR_DEADLINE).expect("the declarer closes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_award_is_not_read_or_proposed_through_the_slot_routes() {
    let (agent, base, client) = node().await;
    let hub = ContractNet::new(Arc::clone(&agent), "hub");
    let driver = ContractNet::new(Arc::clone(&agent), "driver-a");

    // Read: the award is committed, the component reads it, the slot route does not.
    announced_and_closed(&hub, &driver, "pickup-1");
    let awarded = hub.award_linearizable("pickup-1", AwardRule::LowestParticipant, FAR_DEADLINE).await.expect("awarded");
    assert_eq!(awarded.award.participant, "driver-a");
    assert_eq!(hub.award_of("pickup-1").map(|a| a.participant), Some("driver-a".to_string()), "the component reads its award");
    assert!(agent.consensus().consensus_get("cn/pickup-1/award").is_some(), "the award is the slot's decision");

    for url in [format!("{base}/consensus/cn/pickup-1/award"), format!("{base}/consensus/cn%2Fpickup-1%2Faward")] {
        let r = client.get(&url).send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        let body: serde_json::Value = serde_json::from_str(&text).expect("a JSON refusal");
        assert_eq!((status, body["error"].as_str()), (403, Some("protected_slot")), "{url}: {text}");
        assert!(body.get("committed").is_none() && !text.contains("driver-a"), "{url}: nothing of the award: {text}");
        assert!(body["message"].as_str().unwrap_or("").contains("mycelium-commitment"), "{url}: {text}");
    }
    // The same answer for a requirement that has no award.
    let decided = client.get(format!("{base}/consensus/cn/pickup-1/award")).send().await.unwrap().text().await.unwrap();
    let undecided = client.get(format!("{base}/consensus/cn/never-announced/award")).send().await.unwrap().text().await.unwrap();
    assert_eq!(decided, undecided, "a refusal does not say whether a requirement was awarded");

    // Propose: an open requirement's award slot is not decided through the cross-group route…
    announced_and_closed(&hub, &driver, "pickup-2");
    let _listener = agent.consensus().start_consensus_listener(mycelium::ConsensusConfig::default());
    agent.mesh().join_group("g");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while agent.mesh().group_members("g").is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the node never saw itself in g");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let propose = |slot: &'static str| {
        let (client, base) = (client.clone(), base.clone());
        async move {
            let body = serde_json::json!({ "slot": slot, "value_b64": "Zm9yZ2Vk", "groups": [{ "group": "g", "quorum": 0.5, "veto": false }] });
            let r = client.post(format!("{base}/gateway/consensus/cross_group_propose")).json(&body).send().await.unwrap();
            (r.status().as_u16(), r.json::<serde_json::Value>().await.unwrap_or(serde_json::Value::Null))
        }
    };
    // (the control: the route does decide an application slot on this node, so the refusal is the guard's)
    let (status, body) = propose("donation/control").await;
    assert_eq!((status, &body["ok"]), (200, &serde_json::json!(true)), "{body}");
    let (status, body) = propose("cn/pickup-2/award").await;
    assert_eq!((status, body["error"].as_str()), (403, Some("protected_slot")), "{body}");
    assert!(agent.consensus().consensus_get("cn/pickup-2/award").is_none(), "nothing was decided on the award's slot");
    // …so the declarer's award is the one that commits.
    match hub.award_linearizable("pickup-2", AwardRule::LowestParticipant, FAR_DEADLINE).await {
        Ok(awarded) => assert_eq!(awarded.award.participant, "driver-a"),
        Err(CommitmentRefusal::SlotHoldsAnotherRecord { .. }) | Err(CommitmentRefusal::Encoding(_)) =>
            panic!("the award's slot was taken through the cross-group route"),
        Err(other) => panic!("the declarer's award did not commit: {other:?}"),
    }
    agent.shutdown().await;
}
