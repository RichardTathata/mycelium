//! Post-360 hardening row E — the gateway remainder. One test per promise, each seen failing on the
//! code before its fix (the failing lines are quoted in the PR and the row's §3.5 block).

use crate::{GossipAgent, GossipConfig, NodeId};
use std::{sync::Arc, time::Duration};
use tokio_stream::StreamExt as _;

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

async fn gateway_agent() -> (Arc<GossipAgent>, String, reqwest::Client) {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();
    // The listener is bound inside `start()`; wait for the first answer rather than sleeping.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while client.get(format!("{base}/health")).send().await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "gateway never answered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (agent, base, client)
}

async fn post_status(client: &reqwest::Client, url: &str, body: serde_json::Value) -> u16 {
    client.post(url).json(&body).send().await.unwrap().status().as_u16()
}

// ── Promise 1: client-supplied timer periods are bounded ─────────────────────

/// `interval_secs` and `lease_secs` reached a timer unbounded: any `u64` was a period (a refresh "every
/// 30 years" is an advert that never refreshes and a requirement that never evaporates), and a value that
/// was present but not a whole number fell back to the 30 s default without a word. Both routes refuse
/// each now, by name; the bound is `MAX_CLIENT_TIMER_SECS`.
#[tokio::test]
async fn client_timer_periods_are_bounded_at_both_routes() {
    let (agent, base, client) = gateway_agent().await;
    let adv = format!("{base}/gateway/capability/advertise");
    let units = format!("{base}/gateway/units/declare");
    let unit = "[[capability]]\nns = \"plan\"\nname = \"route\"\nttl_secs = 30\n";

    for (field, value, why) in [
        ("interval_secs", serde_json::json!(1_000_000_000u64), "past the bound"),
        ("interval_secs", serde_json::json!(0), "zero is no period"),
        ("interval_secs", serde_json::json!(0.5), "not a whole number"),
        ("interval_secs", serde_json::json!("30"), "not a number"),
        ("lease_secs",    serde_json::json!(1_000_000_000u64), "past the bound"),
        ("lease_secs",    serde_json::json!(0), "zero is no lease"),
        ("lease_secs",    serde_json::json!(2.5), "not a whole number"),
    ] {
        let mut a = serde_json::json!({"ns": "plan", "name": "route"});
        a[field] = value.clone();
        assert_eq!(post_status(&client, &adv, a).await, 400, "capability/advertise {field}={value}: {why}");
        let mut u = serde_json::json!({"toml": unit});
        u[field] = value.clone();
        assert_eq!(post_status(&client, &units, u).await, 400, "units/declare {field}={value}: {why}");
    }
    // The bound itself and the defaults are accepted.
    assert_eq!(post_status(&client, &adv, serde_json::json!({"ns": "plan", "name": "a", "interval_secs": 3600, "lease_secs": 3600})).await, 200);
    assert_eq!(post_status(&client, &adv, serde_json::json!({"ns": "plan", "name": "b"})).await, 200);
    assert_eq!(post_status(&client, &adv, serde_json::json!({"ns": "plan", "name": "c", "interval_secs": 30.0})).await, 200,
        "a whole number written as a float");
    assert_eq!(post_status(&client, &units, serde_json::json!({"toml": unit, "interval_secs": 1, "lease_secs": 1})).await, 200);
    agent.shutdown().await;
}

/// A gateway advert gossiped a bare `Capability`, which every reader times with the legacy 60 s
/// default — so an advert refreshed every 600 s read as stale for 420 s of every 600, and one refreshed
/// every second lingered three minutes after its refresher died. It now carries the period it refreshes
/// at, as an in-process advert does.
#[tokio::test]
async fn a_gateway_advert_declares_the_period_it_refreshes_at() {
    let (agent, base, client) = gateway_agent().await;
    let status = post_status(&client, &format!("{base}/gateway/capability/advertise"),
        serde_json::json!({"ns": "plan", "name": "slow", "interval_secs": 600})).await;
    assert_eq!(status, 200);
    let key = format!("cap/{}/plan/slow", agent.node_id());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let bytes = loop {
        if let Some(b) = agent.kv().get(&key) { break b; }
        assert!(tokio::time::Instant::now() < deadline, "the advert never landed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let entry = crate::capability::CapEntry::decode(&bytes).expect("a CapEntry, not a bare Capability");
    assert_eq!(entry.refresh_interval_ms, 600_000);
    assert_eq!(&*entry.capability.name, "slow");
    agent.shutdown().await;
}

/// A unit file's `ttl_secs` is the period its capability re-asserts at; `0` made a 1 ms gossip loop
/// (the timer seam floors a zero period at 1 ms) and nothing capped it from above. `validate()` refuses
/// both, so a stem and `units/declare` refuse the file by name.
#[test]
fn a_unit_files_ttl_is_bounded_by_validate() {
    let file = |ttl: u64| format!("[[capability]]\nns = \"plan\"\nname = \"route\"\nttl_secs = {ttl}\n");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file(0)).is_err(), "ttl_secs = 0");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file(3_601)).is_err(), "ttl_secs past the bound");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file(3_600)).is_ok(), "the bound itself");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file(30)).is_ok());
}

// ── Promise 3: the mailbox deletes an entry only after the client has it ──────

fn mailbox_left(agent: &GossipAgent, kind: &str) -> usize {
    crate::store::scan_kv_prefix(&agent.task_ctx.kv_state, &format!("mailbox/{}/{kind}/", agent.node_id())).len()
}

/// The gateway mailbox tombstoned each event as it entered the stream's 256-slot channel, so a stream
/// dropped after the client read one event had already deleted every other queued event. Now an event is
/// deleted when the stream hands it on — the rest stay, and the next stream delivers them.
#[tokio::test]
async fn a_dropped_mailbox_stream_loses_only_what_it_handed_on() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    for i in 0u8..10 {
        assert!(agent.service().deliver_event(&id, "row-e.mail", bytes::Bytes::from(vec![i])));
    }
    assert_eq!(mailbox_left(&agent, "row-e.mail"), 10);

    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut stream = Box::pin(super::mailbox_events(Arc::clone(&agent.task_ctx), Arc::from("row-e.mail"), shutdown_rx.clone()));
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().expect("one event");
    assert_eq!(first["payload_b64"], "AA==", "the oldest first");
    // Give the drain every chance to queue (and, before the fix, delete) the other nine.
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(mailbox_left(&agent, "row-e.mail"), 9, "only the event the stream handed on is deleted");

    // The next stream delivers the nine, in order, and deletes each as it hands it on.
    let mut again = Box::pin(super::mailbox_events(Arc::clone(&agent.task_ctx), Arc::from("row-e.mail"), shutdown_rx));
    for i in 1u8..10 {
        let ev = tokio::time::timeout(Duration::from_secs(5), again.next()).await.unwrap().expect("an event");
        use base64::Engine as _;
        assert_eq!(ev["payload_b64"], base64::engine::general_purpose::STANDARD.encode([i]));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(mailbox_left(&agent, "row-e.mail"), 0);
    drop(again);
    agent.shutdown().await;
}

// ── Promises 2 and 5: the log-group subscribe ─────────────────────────────────

#[cfg(feature = "consensus")]
fn put_log(agent: &GossipAgent, stream: &str, hlc: u64, node: &str, value: &str) {
    assert!(agent.kv().set(format!("log/{stream}/{hlc:016x}/{node}"), value.as_bytes().to_vec()));
}

#[cfg(feature = "consensus")]
async fn single_node() -> Arc<GossipAgent> {
    let port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
    agent.start().await.unwrap();
    agent
}

#[cfg(feature = "consensus")]
fn values(events: &[serde_json::Value]) -> Vec<String> {
    use base64::Engine as _;
    events.iter().map(|e| {
        String::from_utf8(base64::engine::general_purpose::STANDARD.decode(e["value_b64"].as_str().unwrap()).unwrap()).unwrap()
    }).collect()
}

/// Two entries can share an HLC (two nodes appending in one millisecond — the reason the key carries a
/// node salt). The core orders a log by `(hlc, key)`; the gateway's log-group consumer sorted by HLC
/// alone and kept an HLC-only offset, so of two same-HLC entries it delivered one, in scan order, and
/// skipped the other forever. The in-process `subscribe_log_group` skipped it the same way (`offset + 1`).
#[cfg(feature = "consensus")]
#[tokio::test]
async fn the_log_group_keeps_the_cores_hlc_key_tiebreak() {
    let agent = single_node().await;
    put_log(&agent, "row-e.tie", 0x100, "10.0.0.2:1", "b");
    put_log(&agent, "row-e.tie", 0x100, "10.0.0.1:1", "a");
    put_log(&agent, "row-e.tie", 0x200, "10.0.0.1:1", "c");

    let mut stream = Box::pin(super::log_group_events(Arc::clone(&agent.task_ctx), "row-e.tie".into(), "g".into()));
    let mut got = Vec::new();
    while got.len() < 3 {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(ev)) => got.push(ev),
            _ => break,
        }
    }
    assert_eq!(values(&got), ["a", "b", "c"], "every entry once, in (hlc, key) order");
    drop(stream);
    agent.shutdown().await;
}

/// The in-process `subscribe_log_group` sorted by `(hlc, key)` but resumed from `offset + 1`, so it
/// skipped the same-HLC sibling the same way.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn the_in_process_log_group_keeps_the_tiebreak() {
    let agent = single_node().await;
    put_log(&agent, "row-e.tie2", 0x100, "10.0.0.2:1", "b");
    put_log(&agent, "row-e.tie2", 0x100, "10.0.0.1:1", "a");
    put_log(&agent, "row-e.tie2", 0x200, "10.0.0.1:1", "c");
    let mut rx = agent.kv().subscribe_log_group("row-e.tie2", "g").await;
    let mut core = Vec::new();
    while core.len() < 3 {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(e)) => core.push(String::from_utf8(e.value.to_vec()).unwrap()),
            _ => break,
        }
    }
    assert_eq!(core, ["a", "b", "c"], "the in-process consumer keeps the same order");
    agent.shutdown().await;
}

/// The gateway's log-group consumer saved the group offset *before* queueing the entry for the stream,
/// so every entry in its 64-slot channel was already "consumed" when the stream dropped and a successor
/// resumed past them. The offset now moves when the stream hands the entry on.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn a_dropped_log_group_stream_saves_only_what_it_handed_on() {
    let agent = single_node().await;
    for (i, v) in ["e1", "e2", "e3", "e4", "e5"].iter().enumerate() {
        put_log(&agent, "row-e.off", 0x100 + i as u64, "10.0.0.1:1", v);
    }
    let mut stream = Box::pin(super::log_group_events(Arc::clone(&agent.task_ctx), "row-e.off".into(), "g".into()));
    let first = tokio::time::timeout(Duration::from_secs(10), stream.next()).await.unwrap().expect("one entry");
    assert_eq!(values(&[first]), ["e1"]);
    tokio::time::sleep(Duration::from_millis(300)).await; // the drain queues the other four
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let offset = agent.kv().get("clog/row-e.off/g/offset").map(|b| String::from_utf8(b.to_vec()).unwrap());
    assert_eq!(offset.as_deref(), Some("0000000000000100"), "the offset names the one entry handed on");
    agent.shutdown().await;
}

/// In process, the offset was saved before the send, so an entry "sent" to a receiver that was already
/// gone counted as consumed. A receiver that is gone is not sent to — the position does not move for it.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn the_in_process_log_group_records_only_after_the_send() {
    let agent = single_node().await;
    let rx = agent.kv().subscribe_log_group("row-e.off2", "g").await;
    drop(rx);
    put_log(&agent, "row-e.off2", 0x100, "10.0.0.1:1", "lost?");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(agent.kv().get("clog/row-e.off2/g/offset"), None, "nothing was handed to a dropped receiver");
    agent.shutdown().await;
}

// ── Promise 4: prompt keys cannot alias across ns/name ─────────────────────────

/// `prompts/{ns}/{name}` with a `/` in either: `(a/b, c)` and `(a, b/c)` were one key, so a template
/// written under one pair answered for the other — and the evidence resource `prompt:{ns}/{name}` named
/// both. A `/` (or an empty segment) is refused at every door that builds the key.
#[cfg(feature = "llm")]
#[tokio::test]
async fn prompt_keys_cannot_alias_across_ns_and_name() {
    let (agent, base, client) = gateway_agent().await;
    let template = serde_json::json!({
        "system": "s", "user_template": "{{input}}", "max_tokens": 8, "temperature": 0.0, "metadata": {}
    });
    let put = client.put(format!("{base}/gateway/prompts/a%2Fb/c")).json(&template).send().await.unwrap().status();
    let get = client.get(format!("{base}/gateway/prompts/a/b%2Fc")).send().await.unwrap().status();
    assert_eq!(get, 400, "the other pair never reads the first pair's template (PUT answered {put})");
    assert_eq!(put, 400, "a `/` in ns is refused");
    assert_eq!(client.delete(format!("{base}/gateway/prompts/a/b%2Fc")).send().await.unwrap().status(), 400);
    assert_eq!(post_status(&client, &format!("{base}/gateway/llm/call"),
        serde_json::json!({"ns": "a/b", "name": "c", "input": "x"})).await, 400, "llm/call");
    assert!(agent.kv().get("prompts/a/b/c").is_none(), "nothing was written");
    agent.shutdown().await;
}

/// The in-process doors build the same key and refuse the same names.
#[cfg(feature = "llm")]
#[tokio::test]
async fn in_process_prompt_names_cannot_alias() {
    let (agent, _base, _client) = gateway_agent().await;
    let t: crate::agent::prompt::PromptTemplate = serde_json::from_value(serde_json::json!({
        "system": "s", "user_template": "{{input}}", "max_tokens": 8, "temperature": 0.0, "metadata": {}
    })).unwrap();
    assert!(agent.llm().update_prompt("a/b", "c", t.clone()).is_err(), "update_prompt");
    assert!(agent.llm().update_prompt("a", "", t.clone()).is_err(), "an empty name");
    assert!(agent.kv().get("prompts/a/b/c").is_none() && agent.kv().get("prompts/a/").is_none());
    assert!(agent.llm().update_prompt("a", "b", t).is_ok(), "a plain pair still works");
    agent.shutdown().await;
}
