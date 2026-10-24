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
/// each now, by name; the bound is `MAX_TIMER_PERIOD_SECS`.
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
    // Adopter-surface X8: a unit's `runtime` is read by wire-check and accepted, not read, here — the declaration says
    // what will run the unit, the node does nothing with it; with a stem section the file is still refused (422).
    let with_runtime = "principal = \"p\"\nruntime = \"python\"\n[[capability]]\nns = \"plan\"\nname = \"route\"\nttl_secs = 30\n";
    assert_eq!(post_status(&client, &units, serde_json::json!({"toml": with_runtime})).await, 200, "a runtime key is accepted and ignored");
    let hosting = "principal = \"p\"\nruntime = \"python\"\n[hosts]\nkinds = [\"blob\"]\ntrusted_publishers = [\"ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12\"]\nplacement_root = \"/var/lib/mycelium/artifacts\"\n";
    assert_eq!(post_status(&client, &units, serde_json::json!({"toml": hosting})).await, 422, "a stem section is a stem's, whatever the runtime");
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
    let cursor = agent.kv().get("clog/row-e.off/g/cursor").map(|b| String::from_utf8(b.to_vec()).unwrap());
    assert_eq!(cursor.as_deref(), Some("0000000000000100/10.0.0.1:1"), "the cursor names the one entry handed on");
    // The legacy offset sits one below it (a reader with only the offset re-delivers, never skips).
    let offset = agent.kv().get("clog/row-e.off/g/offset").map(|b| String::from_utf8(b.to_vec()).unwrap());
    assert_eq!(offset.as_deref(), Some("00000000000000ff"));
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

// ── #613's review round ─────────────────────────────────────────────────────────

/// Review finding 2: each mailbox stream kept its own in-flight set, so a second stream for the same kind
/// found the first's handed-out, not-yet-acknowledged events live and delivered every one of them again.
/// The set is shared per `(node, kind)`; what a dropped stream held un-acknowledged is released to the other.
#[tokio::test]
async fn two_mailbox_streams_for_one_kind_never_hand_out_one_event_twice() {
    let port = alloc_port();
    let id = NodeId::new("127.0.0.1", port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    for i in 0u8..10 {
        assert!(agent.service().deliver_event(&id, "row-e.two", bytes::Bytes::from(vec![i])));
    }
    let (_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut a = Box::pin(super::mailbox_events(Arc::clone(&agent.task_ctx), Arc::from("row-e.two"), shutdown_rx.clone()));
    let first = tokio::time::timeout(Duration::from_secs(5), a.next()).await.unwrap().expect("one event");
    assert_eq!(first["payload_b64"], "AA==");
    tokio::time::sleep(Duration::from_millis(300)).await; // A's drain hands the other nine to its channel

    let mut b = Box::pin(super::mailbox_events(Arc::clone(&agent.task_ctx), Arc::from("row-e.two"), shutdown_rx));
    let dup = tokio::time::timeout(Duration::from_millis(700), b.next()).await;
    assert!(dup.is_err(), "stream B was handed an event stream A holds: {:?}", dup.ok().flatten());

    // A drops with nine handed out and not acknowledged: they are B's now, in order.
    drop(a);
    for i in 1u8..10 {
        let ev = tokio::time::timeout(Duration::from_secs(5), b.next()).await.expect("released to B").expect("an event");
        use base64::Engine as _;
        assert_eq!(ev["payload_b64"], base64::engine::general_purpose::STANDARD.encode([i]));
    }
    drop(b);
    agent.shutdown().await;
}

/// Review finding 3: the cursor and the legacy offset are two KV writes and may arrive apart. A reader that
/// saw the new offset before the new cursor read "everything through this HLC" and skipped a same-HLC
/// sibling for ever. The offset is written one below the entry's HLC, so a reader that has only it
/// re-delivers rather than skips.
#[test]
fn an_offset_that_arrives_before_its_cursor_never_skips_a_sibling() {
    use mycelium_core::log_cursor::{self, LogRow};
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.unwrap();
        let row = |hlc: u64, node: &str| LogRow { hlc, suffix: Arc::from(format!("{hlc:016x}/{node}").as_str()), value: bytes::Bytes::new() };
        let (a, b, c) = (row(0x100, "n1"), row(0x200, "n1"), row(0x200, "n2"));
        let core = &agent.task_ctx.core;
        log_cursor::record(core, "row-e.reorder", "g", &a);
        log_cursor::record(core, "row-e.reorder", "g", &b);
        // The new cursor has not arrived here: this node still holds the one before it.
        assert!(agent.kv().set(log_cursor::cursor_key("row-e.reorder", "g"), a.suffix.as_bytes().to_vec()));
        let pos = log_cursor::read_position(&agent.task_ctx.kv_state, "row-e.reorder", "g");
        assert!(pos.precedes(&c), "the same-HLC sibling is still ahead (position {pos:?})");
        agent.shutdown().await;
    });
}

/// Review finding 5: a `[[serve]]` whose `ns` or `name` holds a `/` (or is empty) loaded, and then
/// `register_prompt_skill` refused it as `InvalidName` on every reconcile tick, warning for ever.
/// `validate()` refuses it by name at load.
#[test]
fn a_serve_name_that_cannot_be_a_prompt_skill_is_refused_at_load() {
    let file = |ns: &str, name: &str| format!(
        "[[serve]]\nns = \"{ns}\"\nname = \"{name}\"\nendpoint = \"http://127.0.0.1:1\"\nmodel = \"m\"\n");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file("llm", "qwen/3")).is_err(), "a / in name");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file("ll/m", "qwen3")).is_err(), "a / in ns");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file("llm", "")).is_err(), "an empty name");
    // A served skill advertises `cap/{node}/{ns}/{name}`, so the reserved locality pair is refused too
    // (#629's review, finding 2: it loaded, then served a skill whose advert was refused).
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file("locality", "self")).is_err(), "the locality pair");
    assert!(crate::NodeCapabilityConfig::from_toml_str(&file("llm", "qwen3")).is_ok());
}

/// `register_prompt_skill("locality", "self")` wrote the template and returned `Ok` while its advert was
/// refused — a skill nobody can resolve (#629's review, finding 2). It refuses the pair as `InvalidName`
/// before writing anything, as it refuses a `/`.
#[cfg(feature = "llm")]
#[tokio::test]
async fn a_prompt_skill_named_like_the_locality_entry_is_refused() {
    let (agent, _base, _client) = gateway_agent().await;
    let template = crate::PromptTemplate {
        system: "s".into(), user_template: "{{input}}".into(), max_tokens: 16, temperature: 0.0,
        metadata: std::collections::HashMap::new(),
    };
    let r = agent.llm().register_prompt_skill("locality", "self", template, Arc::new(crate::agent::llm::EchoBackend)).await;
    assert!(matches!(r, Err(crate::PromptSkillError::InvalidName { .. })), "the locality pair is refused by name");
    assert!(agent.kv().get("prompts/locality/self").is_none(), "and no template is written");
    agent.shutdown().await;
}

// ── #613's review, finding 6: a capability name that cannot be a key segment ─────

/// A capability's namespace and name are two segments of `cap/{node}/{ns}/{name}` (and of `req/…`,
/// `gcap/{group}/{ns}/{name}/{node}`), so an empty one or one holding a `/` builds a key no reader parses
/// (`parse_cap_key` refuses it) — and `ns = "locality", name = "self"` *is* the node's own locality entry,
/// `cap/{node}/locality/self`, which an advert overwrote with a `Capability` where a `LocalityPath` lives.
/// `POST /gateway/capability/advertise` answered 200 to all of them (#613's review, finding 6). Every door
/// refuses them now: the route (400 `invalid_capability_name`), the unit-file loader behind
/// `units/declare` (400), and in process `advertise_capability`, `declare_requirement` and
/// `define_capability_group`, which write nothing.
#[tokio::test]
async fn a_capability_name_that_is_not_a_key_segment_is_refused_at_every_door() {
    use crate::capability::{CapFilter, Capability, CapabilityGroupDef};
    let (agent, base, client) = gateway_agent().await;
    let adv = format!("{base}/gateway/capability/advertise");
    let units = format!("{base}/gateway/units/declare");
    let node = agent.node_id().to_string();
    for (ns, name, why) in [
        ("plan", "a/b", "a '/' in the name"),
        ("pl/an", "b", "a '/' in the namespace"),
        ("", "b", "an empty namespace"),
        ("plan", "", "an empty name"),
        ("locality", "self", "the node's own locality entry"),
    ] {
        let resp = client.post(&adv).json(&serde_json::json!({"ns": ns, "name": name})).send().await.unwrap();
        assert_eq!(resp.status().as_u16(), 400, "capability/advertise {ns:?}/{name:?}: {why}");
        let body: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(body["error"], "invalid_capability_name", "{why}: {body}");
        let cap_unit = format!("[[capability]]\nns = {ns:?}\nname = {name:?}\nttl_secs = 30\n");
        assert_eq!(post_status(&client, &units, serde_json::json!({"toml": cap_unit})).await, 400, "units/declare [[capability]] {why}");
        let req_unit = format!("[[requirement]]\nns = {ns:?}\nname = {name:?}\n");
        assert_eq!(post_status(&client, &units, serde_json::json!({"toml": req_unit})).await, 400, "units/declare [[requirement]] {why}");
        let grp_unit = format!("[[group]]\nname = \"g\"\n  [group.filter]\n  ns = \"plan\"\n  name = \"route\"\n  [[group.provides]]\n  ns = {ns:?}\n  name = {name:?}\n");
        assert_eq!(post_status(&client, &units, serde_json::json!({"toml": grp_unit})).await, 400, "units/declare [[group.provides]] {why}");
    }
    let bad_group = "[[group]]\nname = \"g/h\"\n  [group.filter]\n  ns = \"plan\"\n  name = \"route\"\n";
    assert_eq!(post_status(&client, &units, serde_json::json!({"toml": bad_group})).await, 400, "units/declare a group name holding '/'");

    // In process: each door writes nothing.
    let caps = agent.capabilities();
    let _a = caps.advertise_capability(Capability::new("locality", "self"), Duration::from_millis(50));
    let _b = caps.advertise_capability(Capability::new("plan", "x/y"), Duration::from_millis(50));
    let _c = caps.declare_requirement(CapFilter::new("plan", "x/y"), Duration::from_millis(50));
    let _d = caps.define_capability_group("plan-g", CapabilityGroupDef {
        filter: CapFilter::new("plan", "route"), topology_policy: None,
        provides: vec![Capability::new("plan", "x/y")], requires: vec![],
    }, Duration::from_millis(50));
    let _e = caps.define_capability_group("g/h", CapabilityGroupDef {
        filter: CapFilter::new("plan", "route"), topology_policy: None, provides: vec![], requires: vec![],
    }, Duration::from_millis(50));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let written: Vec<String> = [format!("cap/{node}/"), format!("req/{node}/"), "cap-group/".to_string()].iter()
        .flat_map(|p| agent.kv().scan_prefix(p).into_iter().map(|(k, _)| k.to_string()))
        .collect();
    assert!(written.is_empty(), "an in-process door wrote a key no reader can parse, or the locality entry: {written:?}");

    // A well-formed name still advertises.
    assert_eq!(post_status(&client, &adv, serde_json::json!({"ns": "plan", "name": "fine"})).await, 200);
    agent.shutdown().await;
}

/// The emergent-group projector writes `gcap/{group}/{ns}/{name}/{self}` and `grp/{group}/{self}` from
/// every `cap-group/` definition in its view — a peer's included — so a definition no upgraded door would
/// accept (a group name holding a `/`, or a `provides` name that is not a key segment), gossiped from a
/// 2.33 peer or replayed from state persisted before the upgrade, still produced keys no reader parses
/// (#629's review, finding 1). The projector now skips such a definition. A well-named group beside them is
/// the control: it is joined and projected.
#[tokio::test]
async fn the_group_projector_skips_a_definition_whose_names_are_not_key_segments() {
    use crate::capability::{CapFilter, Capability, CapabilityGroupDef};
    let (agent, _base, _client) = gateway_agent().await;
    let node = agent.node_id().to_string();
    let _cap = agent.capabilities().advertise_capability(Capability::new("plan", "route"), Duration::from_secs(30));
    let def = |provides: Capability| CapabilityGroupDef {
        filter: CapFilter::new("plan", "route"), topology_policy: None, provides: vec![provides], requires: vec![],
    }.encode();
    // Written raw, as a 2.33 peer's definitions would arrive: no in-process door would write them now.
    let _ = agent.kv().set("cap-group/g/h", def(Capability::new("plan", "fine")));
    let _ = agent.kv().set("cap-group/bad-provides", def(Capability::new("plan", "x/y")));
    let _ = agent.kv().set("cap-group/good", def(Capability::new("plan", "fine")));
    let good = format!("gcap/good/plan/fine/{node}");
    for _ in 0..200 { if agent.kv().get(&good).is_some() { break; } tokio::time::sleep(Duration::from_millis(25)).await; }
    assert!(agent.kv().get(&good).is_some(), "the well-named group is projected (the projector ran)");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let bad: Vec<String> = ["gcap/g/h/", "gcap/bad-provides/", "grp/g/h/", "grp/bad-provides/"].iter()
        .flat_map(|p| agent.kv().scan_prefix(p).into_iter().map(|(k, _)| k.to_string()))
        .collect();
    assert!(bad.is_empty(), "the projector wrote keys for a definition whose names are not key segments: {bad:?}");
    agent.shutdown().await;
}
