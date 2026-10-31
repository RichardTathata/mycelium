use crate::{GossipAgent, GossipConfig, NodeId};
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// Item 1 PR 7: the commit JSON carries the receipt's tag beside the old bool, and the error
/// field **only** for `failed` — the shape the live gateway test cannot induce.
#[cfg(feature = "consensus")]
#[test]
fn commit_json_renders_the_receipt_beside_persisted() {
    use crate::LocalDurability;
    let on_disk = super::commit_json(true, &LocalDurability::OnDisk);
    assert_eq!(on_disk["persisted"], true);
    assert_eq!(on_disk["local_durability"], "on_disk");
    assert!(on_disk.get("local_durability_error").is_none());

    // The collapse the field undoes: `persisted: true` with nothing promised.
    let none = super::commit_json(true, &LocalDurability::NotConfigured);
    assert_eq!(none["persisted"], true);
    assert_eq!(none["local_durability"], "not_configured");

    let failed = super::commit_json(false, &LocalDurability::Failed("no ack".into()));
    assert_eq!(failed["persisted"], false);
    assert_eq!(failed["local_durability"], "failed");
    assert_eq!(failed["local_durability_error"], "no ack");
    assert_eq!(failed["ok"], true, "a failed *durability* is still a committed value");
}

/// Realignment repairs R9's third door (verification policy, 2026-10-06): `POST /gateway/govern/timing`
/// published any value — every node's reconciler then ignored one outside `validate()`'s bounds, so
/// the route answered `published: true` for an intent that governs nothing. It refuses it now, and a
/// field that is present but not an integer is refused rather than read as `0` ("ungoverned").
#[tokio::test]
async fn govern_timing_refuses_what_no_node_would_apply() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let url = format!("http://127.0.0.1:{http_port}/gateway/govern/timing");
    let client = reqwest::Client::new();
    let post = |body: serde_json::Value| { let (c, u) = (client.clone(), url.clone()); async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match c.post(&u).json(&body).send().await {
                Ok(r) => break r.status().as_u16(),
                Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
            }
        }
    }};
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 99999})).await, 400, "above 3600");
    assert_eq!(post(serde_json::json!({"reconnect_backoff_secs": 301})).await, 400, "above 300");
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": "30"})).await, 400, "not an integer");
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "reconnect_backoff_secs": 5})).await, 200, "in range");
    // The adversarial review of #544: the route still published intents that govern nothing, or the
    // wrong thing — each was 200 and `published: true`.
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 0})).await, 400, "governs nothing");
    assert_eq!(post(serde_json::json!({})).await, 400, "an empty intent governs nothing");
    assert_eq!(post(serde_json::json!([30])).await, 400, "not an object");
    assert_eq!(post(serde_json::json!({"health_check_interval": 30})).await, 400, "a misspelled field");
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": "node-b"})).await, 400,
        "a target that is not a node id would have governed the whole fleet");
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": 5})).await, 400, "a target that is not a string");
    assert_eq!(post(serde_json::json!({"health_check_interval_secs": 30, "target": format!("127.0.0.1:{gossip_port}")})).await, 200, "a node id");
    agent.shutdown().await;
}

/// The raw KV routes' classification is the namespace table's: every top-level prefix `src/lib.rs`
/// documents is either owned (refused at the raw doors) or a named application namespace. A table row
/// added without a decision here fails this test, so the doors cannot fall behind the table.
#[test]
fn every_namespace_in_the_table_is_classified_for_the_raw_kv_routes() {
    let table = include_str!("../../lib.rs");
    let mut seen = 0;
    for line in table.lines().filter(|l| l.starts_with("//! | `")) {
        let cell = line.trim_start_matches("//! | `");
        let first = cell.split('`').next().unwrap_or("");
        let Some((top, _)) = first.split_once('/') else { continue };
        if top.is_empty() || top.chars().any(|c| c.is_ascii_uppercase()) || top.contains(' ') {
            continue;
        }
        let prefix = format!("{top}/");
        seen += 1;
        assert!(super::OWNED_KV_PREFIXES.contains(&prefix.as_str()) || super::APPLICATION_KV_PREFIXES.contains(&prefix.as_str()),
            "namespace {prefix} in src/lib.rs is neither owned nor an application namespace for the raw KV routes");
    }
    assert!(seen > 20, "the table was read ({seen} rows)");
}

/// The third review of #544: the timing route's strictness, at its two siblings. A `target` that was not
/// a string, a `min` that was not an integer, a `floor` that was a string — each read as absent.
#[tokio::test]
async fn the_governance_routes_parse_strictly() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let client = reqwest::Client::new();
    let post = |route: &'static str, body: serde_json::Value| { let c = client.clone(); async move {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match c.post(format!("http://127.0.0.1:{http_port}/gateway/govern/{route}")).json(&body).send().await {
                Ok(r) => break r.status().as_u16(),
                Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
            }
        }
    }};
    assert_eq!(post("tuning", serde_json::json!({"enabled": true, "target": 5})).await, 400, "a target that is not a string");
    assert_eq!(post("tuning", serde_json::json!({"enabled": true, "enabeld": false})).await, 400, "an unknown field");
    assert_eq!(post("tuning", serde_json::json!({"params": [{"param": "inbound_fps", "floor": "5"}]})).await, 400, "a floor that is not an integer");
    assert_eq!(post("tuning", serde_json::json!({"enabled": true, "params": {"param": "writer_depth"}})).await, 400, "params that is not an array");
    assert_eq!(post("tuning", serde_json::json!({"params": [{"param": "inbound_fps", "ratchet": 1}]})).await, 400, "a ratchet that is not a string");
    assert_eq!(post("tuning", serde_json::json!({"enabled": true})).await, 200);
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": "3"})).await, 400, "a min that is not an integer");
    assert_eq!(post("membership", serde_json::json!({"group": "w"})).await, 400, "min is required");
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "target": ["x"]})).await, 400, "a target that is not a string");
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "max": "10"})).await, 400, "a max that is not an integer");
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "mn": 2})).await, 400, "an unknown field");
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "drain": "10.0.0.5:9000"})).await, 400, "a drain that is not an array");
    assert_eq!(post("membership", serde_json::json!({"group": "w", "min": 1, "max": null})).await, 200);
    agent.shutdown().await;
}

/// The reviews of #544 (enumerating every door to the timing intent): the doors that take a KV key
/// wrote `sys/govern/…` under `kv:write`, publishing a governance intent without `govern:write`, and
/// reached every other key the substrate owns. They refuse `sys/` and `consensus/` now, naming the route.
#[tokio::test]
async fn the_raw_kv_routes_refuse_governance_intents() {
    use base64::Engine as _;
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    // No token table: the gateway is open, so the caller holds every scope — the refusal must not
    // depend on scopes (as for protected kinds), so a `kv:write` token is refused a fortiori.
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let base = format!("http://127.0.0.1:{http_port}/gateway/kv");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let intent = base64::engine::general_purpose::STANDARD.encode(br#"{"health_check_interval_secs":5}"#);
    let r = loop {
        match client.post(&base)
            .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent})).send().await {
            Ok(r) => break r,
            Err(e) => { assert!(tokio::time::Instant::now() < deadline, "{e}"); tokio::time::sleep(Duration::from_millis(50)).await; }
        }
    };
    assert_eq!(r.status(), 403, "a governance intent through the raw KV route");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "protected_key");
    assert!(agent.kv().get("sys/govern/timing").is_none(), "nothing was written");
    for key in ["sys/govern/fleet", "sys/govern/membership/workers"] {
        let r = client.delete(format!("{base}?key={key}")).send().await.unwrap();
        assert_eq!(r.status(), 403, "DELETE {key}");
    }
    let r = client.post(format!("{base}/quorum"))
        .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent, "min_acks": 0, "timeout_secs": 1})).send().await.unwrap();
    assert_eq!(r.status(), 403, "the quorum door");
    assert!(agent.kv().get("sys/govern/timing").is_none(), "nothing was written");
    // The third review of #544: the consensus door wrote the raw key too, and the doors reached every
    // other key the substrate owns — a planted caller-context marker fails a secure gateway's check
    // open; a forged commit or a deleted acceptor record undoes consensus.
    #[cfg(feature = "consensus")]
    {
        let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/overlay/consistent/set"))
            .json(&serde_json::json!({"key": "sys/govern/timing", "value_b64": intent})).send().await.unwrap();
        assert_eq!(r.status(), 403, "the consensus door");
    }
    for key in ["sys/caller-context/10.0.0.9:9000", "consensus/committed/slot-x", "sys/consensus-accepted/n/s", "sys/config/x", "sys/capauthz/ns/name"] {
        let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": ""})).send().await.unwrap();
        assert_eq!(r.status(), 403, "POST {key}");
    }
    // The kv:write decision (2026-10-07): the raw KV routes write application keys only. Every prefix
    // the namespace table assigns to the substrate or a companion is refused, naming its door — the
    // topology escape hatch included, which now has an audited governance route.
    for key in ["sys/topology-override/workers", "grp/workers/10.0.0.5:9000", "prompts/ns/name", "log/s/0001",
                "cap/10.0.0.5:9000/demo/echo", "req/n/demo/echo", "mailbox/n/kind/01", "installable/ns/n/ab",
                "tools/t/n", "skills/ns/n/node/input", "agent/n/task/1/turn", "lock/l", "svc/k/n",
                "wiki/g/proposal/1", "tuple/inflight/ns/1", "facts/n/f", "cn/r", "mandate/m",
                "knowledge/head/i/s", "rights/head/h", "clog/x", "comp/n/ns/k", "gcap/g/ns/n/c", "cap-group/g", "audit/1/n", "sched/close/last"] {
        let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": base64::engine::general_purpose::STANDARD.encode(b"true")})).send().await.unwrap();
        assert_eq!(r.status(), 403, "POST {key}");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["error"], "protected_key", "{key}");
        assert!(agent.kv().get(key).is_none(), "{key} was not written");
    }
    // The application namespaces the table names stay writable: the checkpointer's rows, and an
    // application's provisioning error report.
    // `manifest/` and `schemas/` are application-written by design (src/lib.rs; mesh_manifest.rs) and have
    // no gateway route of their own — the review of #549.
    for key in ["ckpt/t/ns/1", "ckptw/t/ns/1/task/0", "agent/n/provision/item/error", "orders/42",
                "manifest/current", "manifest/control/system", "schemas/s"] {
        let r = client.post(&base).json(&serde_json::json!({"key": key, "value_b64": ""})).send().await.unwrap();
        assert_eq!(r.status(), 200, "POST {key}");
    }
    // The log door names the stream; it must not reach a stream another owner keeps under `log/` — a
    // commitment's offers, a wiki's durable proposals, a reason trace (the review of #549).
    #[cfg(feature = "consensus")]
    {
        let ov = format!("http://127.0.0.1:{http_port}/gateway/overlay/log");
        for stream in ["cn/req-1/offers", "wiki/g/proposals", "reason/run-1/n"] {
            let r = client.post(format!("{ov}/append")).json(&serde_json::json!({"stream": stream, "value_b64": ""})).send().await.unwrap();
            assert_eq!(r.status(), 403, "append to {stream}");
            let r = client.post(format!("{ov}/compact")).json(&serde_json::json!({"stream": stream, "before_hlc": u64::MAX})).send().await.unwrap();
            assert_eq!(r.status(), 403, "compact {stream}");
        }
        let r = client.post(format!("{ov}/append")).json(&serde_json::json!({"stream": "events", "value_b64": ""})).send().await.unwrap();
        assert_eq!(r.status(), 200, "an application stream");
    }
    // The override's own door: govern:write, audited; `true` engages it, `false` tombstones it.
    let over = format!("http://127.0.0.1:{http_port}/gateway/govern/topology-override");
    let r = client.post(&over).json(&serde_json::json!({"group": "workers", "override": true})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(agent.kv().get("sys/topology-override/workers").as_deref(), Some(&b"true"[..]));
    let r = client.post(&over).json(&serde_json::json!({"group": "workers", "override": false})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(agent.kv().get("sys/topology-override/workers").is_none(), "released");
    for bad in [serde_json::json!({"group": "workers"}), serde_json::json!({"group": "workers", "override": "true"}),
                serde_json::json!({"override": true}), serde_json::json!({"group": "a/b", "override": true}),
                serde_json::json!({"group": "workers", "override": true, "x": 1})] {
        let r = client.post(&over).json(&bad).send().await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
    // An ordinary key is unaffected.
    let r = client.post(&base)
        .json(&serde_json::json!({"key": "app/x", "value_b64": ""})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    agent.shutdown().await;
}

/// Both signal SSE routes carry the kind in the data object as well as the SSE event name, so a
/// client that reads the body alone is not wrong (realignment repairs §3.2: `mycelium-ts`'s old
/// `raw.kind` was always undefined). Additive: the event name is unchanged.
#[tokio::test]
async fn signal_sse_data_carries_the_kind() {
    use futures_util::StreamExt as _;
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();

    for path in ["/gateway/signal/sse/repair.request", "/signals/repair.request"] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let resp = loop {
            match reqwest::get(format!("http://127.0.0.1:{http_port}{path}")).await {
                Ok(r) => break r,
                Err(e) => {
                    assert!(tokio::time::Instant::now() < deadline, "{path} never opened: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        assert_eq!(resp.status(), 200, "{path}");
        let mut body = resp.bytes_stream();
        let signal = crate::signal::Signal {
            kind:    Arc::from("repair.request"),
            scope:   crate::signal::SignalScope::Cluster,
            payload: bytes::Bytes::from_static(b"kettle"),
            sender:  id.clone(),
            nonce:   7,
        };
        // The subscription registers when the stream is polled; deliver until an event arrives.
        let mut seen = String::new();
        while !seen.contains("data:") {
            assert!(tokio::time::Instant::now() < deadline, "{path} delivered nothing");
            agent.task_ctx.signal_handlers.deliver(&signal);
            if let Ok(Some(Ok(chunk))) = tokio::time::timeout(Duration::from_millis(100), body.next()).await {
                seen.push_str(&String::from_utf8_lossy(&chunk));
            }
        }
        assert!(seen.contains("event: repair.request"), "{path}: the event name is unchanged: {seen}");
        let data = seen.lines().find_map(|l| l.strip_prefix("data:")).expect("a data line").trim();
        let v: serde_json::Value = serde_json::from_str(data).unwrap();
        assert_eq!(v["kind"], "repair.request", "{path}: the data object names its kind: {data}");
        // One shape on both routes (doc-coverage run 20): `payload_b64` and the `nonce` on the
        // node-level route too; its `payload` stays for existing readers.
        assert_eq!(v["payload_b64"], "a2V0dGxl", "{path}: payload_b64: {data}");
        assert_eq!(v["nonce"], 7, "{path}: nonce: {data}");
    }
    agent.shutdown().await;
}

#[tokio::test]
async fn test_http_health_responds() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();

    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port  = gossip_port;
    cfg.http_port  = Some(http_port);
    cfg.http_addr  = "127.0.0.1".to_string();

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    // Brief pause for the HTTP server to bind and accept.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://127.0.0.1:{http_port}/health");
    let resp = reqwest::get(&url).await.expect("HTTP request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert!(body["node_id"].as_str().unwrap().contains("127.0.0.1"));

    agent.shutdown().await;
}

#[tokio::test]
async fn test_http_stats_responds() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();

    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://127.0.0.1:{http_port}/stats");
    let resp = reqwest::get(&url).await.expect("stats request failed");
    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["store_entries"].is_number());
    assert!(body["dropped_frames"].is_number());

    agent.shutdown().await;
}

/// Providers currently resolvable for `(scrape, worker)` on this gateway.
async fn provider_count(client: &reqwest::Client, base: &str) -> usize {
    let v: serde_json::Value = client
        .get(format!("{base}/gateway/capability/resolve?ns=scrape&name=worker"))
        .send().await.unwrap().json().await.unwrap();
    v["providers"].as_array().map(|a| a.len()).unwrap_or(0)
}

async fn start_test_agent() -> (Arc<GossipAgent>, String, reqwest::Client) {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    (agent, format!("http://127.0.0.1:{http_port}"), reqwest::Client::new())
}

/// Lease mode (2026-07-20): a bridged advertiser's refresh loop runs in
/// THIS node's process, so without a lease the advert outlives a crashed
/// client — provider liveness decoupled from refresher liveness (the w15
/// stale-advert fingerprint). With `lease_secs`, a full window without a
/// heartbeat must retract the advert exactly as DELETE would.
#[tokio::test]
async fn test_gateway_cap_lease_expires_without_heartbeat() {
    let (agent, base, client) = start_test_agent().await;

    let resp = client.post(format!("{base}/gateway/capability/advertise"))
        .json(&serde_json::json!({ "ns": "scrape", "name": "worker",
                                   "interval_secs": 1, "lease_secs": 1 }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let handle_id = resp.json::<serde_json::Value>().await.unwrap()
        ["handle_id"].as_str().unwrap().to_string();

    // Advert appears (first persist tick is immediate).
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while provider_count(&client, &base).await == 0 {
        assert!(std::time::Instant::now() < deadline, "advert never appeared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // No heartbeats → the watchdog tombstones the advert.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while provider_count(&client, &base).await != 0 {
        assert!(std::time::Instant::now() < deadline, "leased advert was never retracted");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The handle died with the lease: DELETE now 404s.
    let del = client.delete(format!("{base}/gateway/capability/{handle_id}"))
        .send().await.unwrap();
    assert_eq!(del.status(), 404);

    agent.shutdown().await;
}

/// Q2: an SDK agent's unit file declared through the gateway — the node's own loader parses
/// it, capabilities, requirements and groups land under one handle, `DELETE` retracts them all,
/// and the hosting sections and a broken file are refused by name. Seen failing first: before
/// the route existed the POST answered 404.
#[tokio::test]
async fn test_gateway_units_declare_one_handle_for_a_whole_unit() {
    let (agent, base, client) = start_test_agent().await;
    let node = agent.node_id().to_string();
    let unit = "principal = \"sdk-planner\"\n\
        [[capability]]\nns = \"plan\"\nname = \"route\"\nttl_secs = 30\n  [capability.attrs]\n  region = \"north\"\n\
        [[requirement]]\nns = \"data\"\nname = \"realtime\"\n  [requirement.attrs]\n  hz = { gte = 10 }\n\
        [[group]]\nname = \"routers\"\n  [group.filter]\n  ns = \"plan\"\n  name = \"route\"\n\
        [[lane]]\nname = \"plans\"\nrole = \"produces\"\n";
    let resp = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": unit, "interval_secs": 1 }))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["principal"], "sdk-planner");
    assert_eq!(body["declared"], serde_json::json!({"capabilities": 1, "requirements": 1, "groups": 1}));
    assert_eq!(body["not_enforced"], serde_json::json!(["[[lane]]"]));
    let handle_id = body["handle_id"].as_str().unwrap().to_string();

    let cap_key = format!("cap/{node}/plan/route");
    let req_key = format!("req/{node}/data/realtime");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while agent.kv().get(&cap_key).is_none() || agent.kv().get(&req_key).is_none() || agent.kv().get("cap-group/routers").is_none() {
        assert!(std::time::Instant::now() < deadline, "the capability, the requirement and the group were declared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // One handle retracts the whole unit.
    let del = client.delete(format!("{base}/gateway/capability/{handle_id}")).send().await.unwrap();
    assert_eq!(del.status(), 200);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while agent.kv().get(&cap_key).is_some() || agent.kv().get(&req_key).is_some() {
        assert!(std::time::Instant::now() < deadline, "DELETE retracted the capability and the requirement");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Refusals, by name.
    let r = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": "[hosts]\nkinds = [\"blob\"]\naccept_unsigned = true\n" })).send().await.unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "hosting sections");
    let r = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": "[[presence]]\nns = \"a\"\nname = \"b\"\nmin_providers = 0\n" })).send().await.unwrap();
    assert_eq!(r.status(), 400, "validate() runs: a floor of zero is refused before the hosting check");
    let r = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": "[[schedule]]\nid = \"close\"\nperiod_secs = 60\n[schedule.holders]\nns = \"a\"\nname = \"b\"\n" })).send().await.unwrap();
    assert_eq!(r.status(), 422, "adopter-surface Y2: a unit carries no schedule; Y12's run stream runs one");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "schedule section");
    // The `schedule` capability namespace is the schedule loop's (#694's review, finding 1): refused at both doors.
    let r = client.post(format!("{base}/gateway/capability/advertise"))
        .json(&serde_json::json!({ "ns": "schedule", "name": "close" })).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "reserved_namespace");
    assert!(agent.capabilities().resolve(&crate::capability::CapFilter::new("schedule", "close")).is_empty());
    let r = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": "[[capability]]\nns = \"schedule\"\nname = \"close\"\n" })).send().await.unwrap();
    assert_eq!(r.status(), 400, "the loader refuses the reserved namespace");
    let r = client.post(format!("{base}/gateway/units/declare"))
        .json(&serde_json::json!({ "toml": "not = [toml" })).send().await.unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "invalid unit file");

    agent.shutdown().await;
}

/// The complement: heartbeats within the window keep a leased advert alive
/// past many lease periods, and a lease-less handle rejects heartbeats (409).
#[tokio::test]
async fn test_gateway_cap_lease_heartbeat_keeps_alive() {
    let (agent, base, client) = start_test_agent().await;

    let resp = client.post(format!("{base}/gateway/capability/advertise"))
        .json(&serde_json::json!({ "ns": "scrape", "name": "worker",
                                   "interval_secs": 1, "lease_secs": 1 }))
        .send().await.unwrap();
    let handle_id = resp.json::<serde_json::Value>().await.unwrap()
        ["handle_id"].as_str().unwrap().to_string();

    // Beat every 300 ms for 2.4 s — several full lease windows.
    for _ in 0..8 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let hb = client.post(format!("{base}/gateway/capability/{handle_id}/heartbeat"))
            .send().await.unwrap();
        assert_eq!(hb.status(), 200);
    }
    assert_eq!(provider_count(&client, &base).await, 1,
               "heartbeated advert must stay live across lease windows");

    // Heartbeats stop → retraction.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while provider_count(&client, &base).await != 0 {
        assert!(std::time::Instant::now() < deadline, "advert not retracted after heartbeats stopped");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // A handle advertised WITHOUT lease_secs has no lease to renew: 409.
    let resp = client.post(format!("{base}/gateway/capability/advertise"))
        .json(&serde_json::json!({ "ns": "scrape", "name": "worker", "interval_secs": 1 }))
        .send().await.unwrap();
    let durable_id = resp.json::<serde_json::Value>().await.unwrap()
        ["handle_id"].as_str().unwrap().to_string();
    let hb = client.post(format!("{base}/gateway/capability/{durable_id}/heartbeat"))
        .send().await.unwrap();
    assert_eq!(hb.status(), 409);

    agent.shutdown().await;
}

/// Native gateway TLS (SOC 2 WS-A, 2026-07-22): with `gateway_tls` set (node-cert
/// reuse), the gateway serves HTTPS — bearer tokens/JWTs never traverse cleartext.
/// A faithful client (rustls, trusting the generated cluster CA, IP-SAN match) must
/// complete a real handshake and get `/health` 200; a plaintext client must fail.
#[cfg(feature = "tls")]
#[tokio::test]
async fn test_gateway_serves_native_tls() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("gw-tls-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    cfg.gateway_tls = Some(crate::GatewayTlsConfig::default()); // reuse node cert

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Build a rustls client that trusts the cluster CA the node just generated.
    let ca_pem = std::fs::read(cert_dir.join("ca-cert.pem")).expect("ca-cert.pem written");
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut std::io::Cursor::new(&ca_pem)) {
        roots.add(c.unwrap()).unwrap();
    }
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(client_config));
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
    let server_name = rustls::pki_types::ServerName::IpAddress(
        std::net::Ipv4Addr::new(127, 0, 0, 1).into());
    let mut tls = connector.connect(server_name, tcp).await
        .expect("TLS handshake against the gateway (server must speak TLS)");
    tls.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await.unwrap();
    let mut buf = Vec::new();
    tls.read_to_end(&mut buf).await.unwrap();
    let resp = String::from_utf8_lossy(&buf);
    assert!(resp.contains("200 OK"), "expected 200 over HTTPS, got: {resp}");
    assert!(resp.contains("\"status\":\"ok\""), "health body over HTTPS: {resp}");

    // Negative: a plaintext HTTP client must NOT get a valid HTTP response — the
    // server now expects a TLS ClientHello, so a raw GET yields no "200 OK".
    let mut plain = tokio::net::TcpStream::connect(("127.0.0.1", http_port)).await.unwrap();
    plain.write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await.unwrap();
    let mut pbuf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(2), plain.read_to_end(&mut pbuf)).await;
    assert!(!String::from_utf8_lossy(&pbuf).contains("200 OK"),
        "plaintext HTTP must fail against a TLS gateway");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// Identity-auth Phase 2 (SOC 2 WS-E): signed-proof **prevention**. Poisoning — a
/// `sys/identity/{V}` overwrite whose proof is signed by an untrusted key — must be
/// **rejected** (the foreign key never enters `peer_keys[V]`), while a legitimate self-signed
/// entry is accepted. Single-node: drive the merge helper directly against a controlled store.
#[cfg(feature = "tls")]
#[tokio::test]
async fn test_identity_proof_rejects_poisoning_accepts_signed() {
    use ed25519_dalek::{Signer, SigningKey};
    let anchor_keys: papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> =
        papaya::HashMap::new();
    let peer_keys: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
    let conflicts = std::sync::atomic::AtomicU64::new(0);
    let victim = NodeId::new("127.0.0.1", 7001).unwrap();

    // V's real key. Anchor it (as Phase 1b would on a live connection).
    let v_sk = SigningKey::from_bytes(&[7u8; 32]);
    let v_key = v_sk.verifying_key().to_bytes();
    anchor_keys.pin().insert(victim.clone(), std::collections::HashSet::from([v_key]));

    // ── Poisoning: attacker M writes sys/identity/{V} = v_key‖m_key + a proof signed by M's
    //    own key (NOT trusted for V). Must be rejected — m_key must not enter peer_keys[V].
    let m_sk = SigningKey::from_bytes(&[66u8; 32]);
    let m_key = m_sk.verifying_key().to_bytes();
    let mut history = v_key.to_vec();
    history.extend_from_slice(&m_key);
    let m_sig = m_sk.sign(&crate::agent::helpers::identity_proof_message(&history)).to_bytes();
    let bad_proof = crate::agent::helpers::encode_identity_proof(&m_key, &m_sig);
    let kv_keys = [v_key, m_key];
    crate::agent::helpers::validate_and_merge_identity(
        &peer_keys, &anchor_keys, &conflicts, &victim, &history, &kv_keys, Some(&bad_proof), false);
    assert!(!peer_keys.pin().get(&victim).map(|v| v.contains(&m_key)).unwrap_or(false),
            "poisoning rejected: M's key must NOT be trusted for V");
    assert_eq!(conflicts.load(std::sync::atomic::Ordering::Relaxed), 1, "conflict counted");

    // ── Legitimate: V rotates, history = v2‖v_key, proof signed by the PRIOR (trusted) key.
    //    Must be accepted — v2 enters peer_keys[V].
    let v2_sk = SigningKey::from_bytes(&[8u8; 32]);
    let v2_key = v2_sk.verifying_key().to_bytes();
    let mut hist2 = v2_key.to_vec();
    hist2.extend_from_slice(&v_key);
    let good_sig = v_sk.sign(&crate::agent::helpers::identity_proof_message(&hist2)).to_bytes(); // signed by the prior key
    let good_proof = crate::agent::helpers::encode_identity_proof(&v_key, &good_sig);
    crate::agent::helpers::validate_and_merge_identity(
        &peer_keys, &anchor_keys, &conflicts, &victim, &hist2, &[v2_key, v_key], Some(&good_proof), false);
    assert!(peer_keys.pin().get(&victim).unwrap().contains(&v2_key),
            "legitimate rotation chained by the prior key is accepted");
}

/// Identity-auth Phase 3 (SOC 2 WS-E): with `require_identity_proofs`, an **unsigned**
/// `sys/identity` entry (mimicking a pre-Phase-2 node) is **rejected** — the last poisoning
/// residual — whereas the same entry is tolerated when the flag is explicitly off (the opt-out).
#[cfg(feature = "tls")]
#[tokio::test]
async fn test_require_identity_proofs_rejects_unsigned() {
    use ed25519_dalek::SigningKey;
    let anchor_keys: papaya::HashMap<NodeId, std::collections::HashSet<[u8; 32]>> =
        papaya::HashMap::new();
    let node = NodeId::new("127.0.0.1", 7002).unwrap();
    let k = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
    let history = k.to_vec();

    // Flag OFF (an explicit opt-out since post-360 row S, which made `true` the default; see
    // `config::tests::the_default_requires_identity_proofs`): unsigned accepted.
    let pk_off: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
    let c_off = std::sync::atomic::AtomicU64::new(0);
    crate::agent::helpers::validate_and_merge_identity(
        &pk_off, &anchor_keys, &c_off, &node, &history, &[k], None, false);
    assert!(pk_off.pin().get(&node).is_some_and(|v| v.contains(&k)),
            "flag off: unsigned entry accepted (rollout tolerance)");

    // Flag ON (Phase 3; the default since row S): the same unsigned entry rejected + counted.
    let pk_on: papaya::HashMap<NodeId, Vec<[u8; 32]>> = papaya::HashMap::new();
    let c_on = std::sync::atomic::AtomicU64::new(0);
    crate::agent::helpers::validate_and_merge_identity(
        &pk_on, &anchor_keys, &c_on, &node, &history, &[k], None, true);
    assert!(pk_on.pin().get(&node).is_none(), "flag on: unsigned entry rejected");
    assert_eq!(c_on.load(std::sync::atomic::Ordering::Relaxed), 1, "rejection counted");
}

/// Identity-auth Phase 1b (SOC 2 WS-E): a directly-connected TLS peer's CA-validated
/// key is harvested from its cert into an authenticated anchor; a `sys/identity` KV entry
/// introducing a *different* key then trips the conflict counter (the poisoning signal).
#[cfg(feature = "tls")]
#[tokio::test]
async fn test_identity_anchor_recorded_and_conflict_flagged() {
    let cert_dir = std::env::temp_dir().join(format!("wse-anchor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let pa = alloc_port();
    let pb = alloc_port();
    let mk = |port: u16, boot: Vec<NodeId>| {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = boot;
        cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
        Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
    };
    let a = mk(pa, vec![]);
    let b = mk(pb, vec![NodeId::new("127.0.0.1", pa).unwrap()]);
    a.start().await.unwrap();
    b.start().await.unwrap();

    // Wait until B has anchored A (B dialed A, so B's outbound writer harvested A's cert key).
    let ida = NodeId::new("127.0.0.1", pa).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let anchored = loop {
        if b.task_ctx.peer_anchor_keys.pin().get(&ida).is_some_and(|s| !s.is_empty()) {
            break true;
        }
        if std::time::Instant::now() > deadline { break false; }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(anchored, "B must anchor A's CA-validated key after dialing it");

    // The anchored key equals A's real identity key.
    let a_real = a.task_ctx.tls.get().unwrap().verifying_key_bytes();
    assert!(b.task_ctx.peer_anchor_keys.pin().get(&ida).unwrap().contains(&a_real),
            "the anchor is A's actual identity key");

    // Since identity and its proof travel as one sealed record (v2.14.0), the watcher reads
    // `sys/identity-signed/{A}` first and the legacy pair only when no sealed record is held
    // (`helpers::resolve_identity_record`). So a poisoned legacy `sys/identity/{A}` is
    // **masked** while B holds A's sealed record — the sealed record is authoritative — and
    // whether the tripwire fired used to depend on whether that record had arrived yet: a
    // race this test lost once in nine runs of main (the v2.17.0 release commit, 2026-09-30).
    // Now the test waits for the sealed record, shows the masking, and then poisons the
    // record the watcher actually reads: the sealed record is tombstoned (any node can gossip
    // a newer tombstone) and the legacy pair carries the foreign key.
    let sealed_key = format!("sys/identity-signed/{ida}");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while b.kv().get(&sealed_key).is_none() {
        assert!(std::time::Instant::now() < deadline, "B must hold A's sealed identity record");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let before = b.system_stats().identity_anchor_conflicts;
    let foreign = [0x42u8; 32];
    let mut poisoned = a_real.to_vec();
    poisoned.extend_from_slice(&foreign);
    // The masked case: with the sealed record present, the legacy poison changes nothing.
    let _ = b.kv().set(format!("sys/identity/{ida}"), bytes::Bytes::from(poisoned.clone()));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(b.system_stats().identity_anchor_conflicts, before,
        "a legacy poison is masked while the sealed record is held: the sealed record is authoritative");
    // The real poisoning: the sealed record gone, the legacy pair introduces a foreign key —
    // the watcher re-scans (both keys sit under the `sys/identity` prefix) and reads the pair.
    let _ = b.kv().delete(sealed_key);
    let _ = b.kv().set(format!("sys/identity/{ida}"), bytes::Bytes::from(poisoned));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if b.system_stats().identity_anchor_conflicts > before { break; }
        assert!(std::time::Instant::now() < deadline, "conflict tripwire never fired");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    a.shutdown().await;
    b.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// SOC 2 WS-D: after a checkpoint, pruned records still verify from the signed
/// checkpoint boundary (not genesis) — the mechanism that makes retention safe.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_audit_checkpoint_prune_and_verify() {
    let gossip_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("wsd-ckpt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    for i in 0..6u32 {
        agent.audit(crate::AuditAction::Write, "op", format!("t-{i}"),
                    crate::AuditOutcome::Success, None).unwrap();
    }
    assert_eq!(agent.audit_verify(&id), Ok(()), "full chain verifies before checkpoint");

    // Checkpoint at the current boundary (6), then prune the exported prefix.
    let (cp_seq, _) = agent.audit_checkpoint().unwrap();
    assert_eq!(cp_seq, 6);
    // Seal 2 more, then prune everything below the checkpoint (records 0..6).
    for i in 6..8u32 {
        agent.audit(crate::AuditAction::Write, "op", format!("t-{i}"),
                    crate::AuditOutcome::Success, None).unwrap();
    }
    let pruned = agent.audit_prune_to_checkpoint();
    assert_eq!(pruned, 6, "records 0..6 pruned");
    assert_eq!(agent.audit_stream(&id).len(), 2, "records 6,7 remain");

    // The pruned stream still verifies — from the checkpoint boundary, not genesis.
    assert_eq!(agent.audit_verify(&id), Ok(()),
               "pruned stream verifies from the signed checkpoint");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// SOC 2 WS-C: an attached AuditSink receives every sealed record, off the write
/// path. Seal a few audit events and assert the sink captured them in order, while
/// the authoritative chain still verifies.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_audit_sink_mirrors_sealed_records() {
    use std::sync::Mutex as StdMutex;

    struct CapturingSink(Arc<StdMutex<Vec<u64>>>);
    impl crate::AuditSink for CapturingSink {
        fn export(&self, record: &crate::SignedAuditRecord) {
            self.0.lock().unwrap().push(record.record.seq);
        }
    }

    let gossip_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("wsc-sink-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(id, cfg));

    let captured = Arc::new(StdMutex::new(Vec::new()));
    agent.with_audit_sink(Arc::new(CapturingSink(Arc::clone(&captured))));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    for i in 0..5u32 {
        agent.audit(crate::AuditAction::Write, "op", format!("target-{i}"),
                    crate::AuditOutcome::Success, None).unwrap();
    }

    // Drain task runs off-path — poll until the sink has all 5, in seal order.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if captured.lock().unwrap().len() >= 5 { break; }
        assert!(std::time::Instant::now() < deadline, "sink never received all records");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(*captured.lock().unwrap(), vec![0, 1, 2, 3, 4], "records mirrored in seal order");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// SOC 2 WS-B: rotation alone is hygiene (the old key stays accepted); the
/// compromise flow rotates AND revokes the old key, so it stops verifying
/// cluster-wide. A revocation must be recorded after `rotate_identity_on_compromise`
/// and none before.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_rotate_on_compromise_revokes_old_key() {
    let gossip_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("wsb-compromise-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(agent.revocation_head().is_none(), "no revocations before the compromise rotation");

    agent.rotate_identity_on_compromise(Duration::from_millis(50)).await.unwrap();

    let (_root, count) = agent.revocation_head()
        .expect("the outgoing key must be revoked by the compromise flow");
    assert_eq!(count, 1, "exactly the old key revoked");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// The adversarial review of #584 (finding 5): a WAL writer refusing appends after a failed
/// write was visible only in the log. `/health` now carries a `persistence` block —
/// `wal_refusing_appends` with the reason — that flips on and, once a snapshot truncates the
/// torn tail, off. Seen failing first: the block was absent (`null`).
#[cfg(unix)]
#[tokio::test]
async fn health_says_when_the_wal_writer_refuses_appends() {
    use crate::config::{OnUnreadable, PersistenceConfig, SyncMode};
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let base = std::env::temp_dir().join(format!("myc-health-wal-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&base);
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.persistence = Some(PersistenceConfig { base_path: base.clone(), sync_mode: SyncMode::Flush, snapshot_wal_threshold: 1_000, snapshot_interval_secs: 3_600, on_unreadable: OnUnreadable::Refuse });
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    let url = format!("http://127.0.0.1:{http_port}/health");
    let client = reqwest::Client::builder().timeout(Duration::from_millis(500)).build().unwrap();
    let mut body = None;
    for _ in 0..100 {
        if let Ok(resp) = client.get(&url).send().await && resp.status() == 200 {
            body = Some(resp.json::<serde_json::Value>().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let body = body.expect("gateway /health never returned 200");
    assert_eq!(body["persistence"]["configured"], serde_json::json!(true), "{body}");
    assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(false), "a healthy writer: {body}");

    let wal = Arc::clone(agent.task_ctx.wal.get().expect("persistence configured"));
    wal.poison_for_test("injected write failure").await.unwrap();
    let body: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(true), "{body}");
    assert_eq!(body["persistence"]["reason"], serde_json::json!("injected write failure"), "{body}");
    assert!(agent.kv().set_requiring_sync(&mycelium_core::receipt::OperationId::new("h-refused"), "h/refused", b"v".to_vec()).await.is_err(),
        "an append is refused while the writer is poisoned");

    wal.trigger_snapshot().await.expect("the repairing snapshot");
    let body: serde_json::Value = client.get(&url).send().await.unwrap().json().await.unwrap();
    assert_eq!(body["persistence"]["wal_refusing_appends"], serde_json::json!(false), "recovered: {body}");
    assert_eq!(body["persistence"]["reason"], serde_json::Value::Null);
    assert!(body["persistence"]["dropped_appends"].as_u64().unwrap() >= 1, "the refusal was counted: {body}");
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&base);
}

/// Operational-readiness invariant: shutdown must actually close the
/// gateway port. A load balancer drains a node by observing connection
/// refusal; a zombie listener that keeps accepting after shutdown() would
/// answer health checks from a dead agent (M2 Run-22 probe).
#[tokio::test]
async fn test_gateway_port_closes_on_shutdown() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();

    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();

    let url = format!("http://127.0.0.1:{http_port}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(250))
        .build()
        .unwrap();
    // Poll until the HTTP server has bound — a fixed sleep races server startup under load.
    let mut up = false;
    for _ in 0..100 {
        if let Ok(resp) = client.get(&url).send().await
            && resp.status() == 200
        {
            up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(up, "gateway /health never returned 200");

    agent.shutdown().await;

    // Poll briefly: the server task abort is asynchronous, but the port
    // must stop accepting within the shutdown grace window.
    let mut closed = false;
    for _ in 0..40 {
        if reqwest::Client::builder()
            .timeout(Duration::from_millis(250))
            .build()
            .unwrap()
            .get(&url)
            .send()
            .await
            .is_err()
        {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "gateway port still accepting after shutdown");
}

/// gw_llm_call reports failures via HTTP status codes (the gateway-wide
/// convention), not a 200 + error-JSON envelope: a no-provider miss is a
/// 404 so plain `curl -f` / `raise_for_status()` callers see the failure.
#[cfg(feature = "llm")]
#[tokio::test]
async fn test_llm_call_no_provider_returns_404() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();

    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://127.0.0.1:{http_port}/gateway/llm/call");
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({"ns":"nobody","name":"provides-this","input":"x"}))
        .send()
        .await
        .expect("llm/call request failed");
    assert_eq!(resp.status(), 404, "no provider must surface as HTTP 404");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "no_provider", "error JSON body is kept alongside the status");

    agent.shutdown().await;
}

#[tokio::test]
async fn test_sse_delivers_signals() {
    use crate::signal::SignalScope;
    use bytes::Bytes;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();

    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    // Must be in the "test-sse" group to admit the signal.
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.mesh().join_group("test-sse");
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Connect SSE client.
    let url = format!("http://127.0.0.1:{http_port}/signals/sse-probe");
    let mut resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .expect("SSE connect failed");
    assert_eq!(resp.status(), 200);

    // Emit a signal to self.
    let _ = agent.mesh().emit("sse-probe", SignalScope::Cluster, Bytes::from_static(b"payload"));

    // Read SSE chunks until we see the expected event or timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                if text.contains("sse-probe") {
                    found = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(found, "SSE event for 'sse-probe' was not received within timeout");

    agent.shutdown().await;
}

// ── MCP endpoint tests ────────────────────────────────────────────────────

fn mcp_agent(http_port: u16) -> Arc<GossipAgent> {
    let gossip_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    Arc::new(GossipAgent::new(id, cfg))
}

#[tokio::test]
async fn test_mcp_initialize() {
    let http_port = alloc_port();
    let agent = mcp_agent(http_port);
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"},
            },
        }))
        .send()
        .await
        .expect("initialize request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["result"]["protocolVersion"], "2024-11-05");
    assert!(body["result"]["serverInfo"]["name"].as_str().is_some());

    agent.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tools_list() {
    let http_port = alloc_port();
    let agent = mcp_agent(http_port);
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _handle = agent.mcp().register_mcp_tool(
        "greet",
        serde_json::json!({
            "type": "object",
            "description": "Greets a person",
            "properties": {"name": {"type": "string"}},
            "required": ["name"],
        }),
        |args| async move {
            Ok(serde_json::json!(format!("hello, {}", args["name"].as_str().unwrap_or("?"))))
        },
    );

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {},
        }))
        .send()
        .await
        .expect("tools/list request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tools = body["result"]["tools"].as_array().unwrap();
    assert!(
        tools.iter().any(|t| t["name"] == "greet"),
        "tool 'greet' not in list: {body}"
    );

    agent.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tools_call_round_trip() {
    let http_port = alloc_port();
    let agent = mcp_agent(http_port);
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let _handle = agent.mcp().register_mcp_tool(
        "square",
        serde_json::json!({
            "type": "object",
            "properties": {"n": {"type": "number"}},
            "required": ["n"],
        }),
        |args| async move {
            let n = args["n"].as_f64().unwrap_or(0.0);
            Ok(serde_json::json!(n * n))
        },
    );

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "square", "arguments": {"n": 5.0}},
        }))
        .send()
        .await
        .expect("tools/call request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("25"), "expected 25, got '{text}'");

    agent.shutdown().await;
}

#[tokio::test]
async fn test_mcp_tools_call_not_found() {
    let http_port = alloc_port();
    let agent = mcp_agent(http_port);
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "no-such-tool", "arguments": {}},
        }))
        .send()
        .await
        .expect("tools/call request failed");

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], -32601);
    assert!(
        body["error"]["message"].as_str().unwrap().contains("no-such-tool"),
        "unexpected error message: {body}"
    );

    agent.shutdown().await;
}

// ── WS1 gateway OAuth2 scope ACLs (compliance feature) ────────────────

#[cfg(feature = "compliance")]
#[test]
fn required_scope_table_maps_families_and_denies_by_default() {
    use axum::http::Method;
    use super::required_scope;
    // read/write split on the same path keys off the method.
    assert_eq!(required_scope(&Method::GET,    "/gateway/kv"), "kv:read");
    assert_eq!(required_scope(&Method::POST,   "/gateway/kv"), "kv:write");
    assert_eq!(required_scope(&Method::DELETE, "/gateway/kv"), "kv:write");
    // resource families.
    assert_eq!(required_scope(&Method::GET,  "/gateway/capability/resolve"), "cap:read");
    assert_eq!(required_scope(&Method::POST, "/gateway/signal/emit"), "mesh:write");
    assert_eq!(required_scope(&Method::GET,  "/gateway/rpc/serve/{kind}"), "mesh:serve");
    assert_eq!(required_scope(&Method::POST, "/gateway/rpc/respond"), "mesh:serve");
    assert_eq!(required_scope(&Method::POST, "/gateway/rpc/call"), "mesh:write");
    assert_eq!(required_scope(&Method::POST, "/gateway/overlay/consistent/set"), "consensus:write");
    assert_eq!(required_scope(&Method::GET,  "/gateway/overlay/consistent/get"), "consensus:read");
    assert_eq!(required_scope(&Method::POST, "/gateway/llm/call"), "llm:invoke");
    assert_eq!(required_scope(&Method::POST, "/gateway/govern/group"), "govern:write");
    assert_eq!(required_scope(&Method::DELETE, "/gateway/govern/group"), "govern:write");
    assert_eq!(required_scope(&Method::GET,  "/gateway/fleet"), "fleet:read");
    assert_eq!(required_scope(&Method::GET,  "/gateway/explain"), "fleet:read");
    assert_eq!(required_scope(&Method::GET,  "/gateway/diagnose"), "fleet:read");
    // companion families (merged routes, 2026-09-04).
    assert_eq!(required_scope(&Method::POST, "/gateway/reason/route"), "llm:invoke");
    // Adopter-surface X3 (second PR): the revoke route under the issue scope.
    assert_eq!(required_scope(&Method::POST, "/gateway/grants/{agent}/{request}/revoke"), "grant:issue");
    // Node-level routes gated since 2026-09-05.
    assert_eq!(required_scope(&Method::POST, "/mcp"),              "mcp:invoke");
    assert_eq!(required_scope(&Method::GET,  "/signals/{kind}"),   "mesh:read");
    assert_eq!(required_scope(&Method::GET,  "/consensus/{*slot}"), "consensus:read");
    assert_eq!(required_scope(&Method::POST, "/gateway/reason/v1/chat/completions"), "llm:invoke");
    assert_eq!(required_scope(&Method::GET,  "/gateway/reason/trace/{run_id}"), "llm:read");
    assert_eq!(required_scope(&Method::PUT,  "/gateway/reason/blob"), "llm:write");
    assert_eq!(required_scope(&Method::POST, "/gateway/wiki/query"), "wiki:read");
    // Federation's consumer side (item 2 row 11): reading about partners and spending a
    // credential on one are different powers, and the table is where that is decided.
    assert_eq!(required_scope(&Method::GET,  "/gateway/federation/domain"), "federation:read");
    assert_eq!(required_scope(&Method::GET,  "/gateway/federation/partners"), "federation:read");
    assert_eq!(required_scope(&Method::GET,  "/gateway/federation/catalog/{domain}"), "federation:read");
    assert_eq!(required_scope(&Method::POST, "/gateway/federation/connect"), "federation:invoke");
    assert_eq!(required_scope(&Method::POST, "/gateway/federation/call"), "federation:invoke");
    assert_eq!(required_scope(&Method::POST, "/gateway/wiki/ingest"), "wiki:write");
    assert_eq!(required_scope(&Method::GET,  "/gateway/bb/depth"), "board:read");
    assert_eq!(required_scope(&Method::POST, "/gateway/tuple/take"), "tuple:write");
    assert_eq!(required_scope(&Method::POST, "/gateway/artifacts/publish"), "artifact:publish");
    assert_eq!(required_scope(&Method::POST, "/gateway/units/declare"), "cap:write");
    // deny-by-default: anything unmapped requires admin — including an unlisted companion path.
    assert_eq!(required_scope(&Method::POST, "/gateway/some/future/route"), "admin");
    assert_eq!(required_scope(&Method::POST, "/gateway/wiki/some/future/verb"), "admin");
}

#[cfg(feature = "compliance")]
#[test]
fn scope_admits_exact_and_wildcard_only() {
    use super::scope_admits;
    let ro = vec!["kv:read".to_string()];
    assert!(scope_admits(&ro, "kv:read"));
    assert!(!scope_admits(&ro, "kv:write"));
    let star = vec!["*".to_string()];
    assert!(scope_admits(&star, "kv:write"));
    assert!(scope_admits(&star, "admin"));
    // Empty grant admits nothing.
    assert!(!scope_admits(&[], "kv:read"));
}

#[cfg(feature = "compliance")]
#[test]
fn resolve_token_scopes_legacy_is_wildcard() {
    use super::resolve_token;
    let mut cfg = GossipConfig::default();
    cfg.gateway_auth_token = Some("legacy-tok".to_string());
    cfg.gateway_scoped_tokens = vec![crate::GatewayToken {
        token:  "ro-tok".to_string(),
        scopes: vec!["kv:read".to_string()],
    }];
    cfg.gateway_named_tokens = vec![crate::GatewayNamedToken {
        name: "ci-bot".to_string(), token: "named-tok".to_string(), scopes: vec!["kv:write".to_string()],
    }];
    // Legacy token → superuser wildcard (unchanged upgrade path), principal qualified by the issuer.
    assert_eq!(resolve_token(&cfg, "gw-1", "legacy-tok"),
               Some(("token:gw-1/legacy".to_string(), vec!["*".to_string()])));
    // Named token → its grant, principal by name (never the secret, stable under reordering).
    assert_eq!(resolve_token(&cfg, "gw-1", "named-tok"), Some(("token:gw-1/ci-bot".to_string(), vec!["kv:write".to_string()])));
    // Positional token → its grant, principal by position.
    assert_eq!(resolve_token(&cfg, "gw-1", "ro-tok"), Some(("token:gw-1/#0".to_string(), vec!["kv:read".to_string()])));
    // Unknown token → None (unauthenticated).
    assert_eq!(resolve_token(&cfg, "gw-1", "nope"), None);
}

/// Defence in depth for #598's review finding 1: an **empty presented bearer** never resolves,
/// whatever the table holds — even a blank secret that reached the config past `validate()` (an
/// agent built from a struct literal and started without the config path). HTTP/1 trims
/// `Authorization: Bearer `; HTTP/2 (served under `gateway_tls`) delivers the empty string.
#[test]
fn resolve_token_never_matches_an_empty_presented_bearer() {
    use super::resolve_token;
    let mut cfg = GossipConfig::default();
    cfg.gateway_auth_token = Some(String::new());
    #[cfg(feature = "compliance")]
    {
        cfg.gateway_scoped_tokens = vec![crate::GatewayToken { token: String::new(), scopes: vec!["*".into()] }];
        cfg.gateway_named_tokens = vec![crate::GatewayNamedToken { name: "ops".into(), token: "  ".into(), scopes: vec!["*".into()] }];
    }
    for presented in ["", " ", "  ", "\t"] {
        assert_eq!(resolve_token(&cfg, "gw-1", presented), None, "{presented:?} must not authenticate");
    }
}

#[cfg(feature = "compliance")]
#[test]
fn regression_parse_hex32_rejects_non_ascii_without_panic() {
    use super::parse_hex32;
    // 64 BYTES but not 64 chars: one 3-byte '€' + 61 ASCII. The old code byte-sliced after a
    // BYTE-length check and panicked on the non-char-boundary (node-abort under panic=abort).
    // Must return None, never panic (audit 2026-07-15 pass 2).
    let s = format!("€{}", "a".repeat(61));
    assert_eq!(s.len(), 64, "precondition: 64 bytes, <64 chars");
    assert_eq!(parse_hex32(&s), None, "non-ASCII 64-byte input must be rejected, not panic");
    // Valid 64-hex still parses; wrong lengths rejected.
    assert!(parse_hex32(&"ab".repeat(32)).is_some());
    assert_eq!(parse_hex32("abcd"), None);
}

/// Review 2026-09-05 finding 4: `/mcp`, `/signals/{kind}` and `/consensus/{*slot}` answered
/// without a bearer when `gateway_auth_token` was set — `tools/call` invoked any cluster tool
/// with this node's identity. They now sit behind the gateway's bearer boundary. The M16
/// public set (`/health`, `/ready`, `/stats`, `/metrics`) and the nonce-capability
/// `/bulk/{id}` stay open — never 401.
#[tokio::test]
/// Identifiers in paths (contracts-axis plan §9, 2026-09-06): every slot the substrate mints
/// is hierarchical (`lock/{name}`, `consistent/{key}`, `leader/{group}`), so the read route
/// captures the **path tail** (`/consensus/{*slot}`) and an operator can type the slot as a
/// receipt shows it. Before this the pattern was one-segment and the runbooks' literal URL
/// 404ed for two months (wiki-lint ledger 2026-09-06). Both the literal and the
/// percent-encoded form must reach the handler and name the same slot. `consensus`-gated
/// because the route is.
#[cfg(feature = "consensus")]
async fn regression_consensus_slot_route_accepts_hierarchical_slots() {
    use axum::http::header::AUTHORIZATION;
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let literal = client.get(format!("{base}/consensus/lock/x"))
        .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
    let encoded = client.get(format!("{base}/consensus/lock%2Fx"))
        .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
    let (ls, es) = (literal.status(), encoded.status());
    let lb = literal.text().await.unwrap_or_default();
    let eb = encoded.text().await.unwrap_or_default();
    agent.shutdown().await;
    assert_eq!(ls, 200, "literal hierarchical slot reaches the handler (body: {lb})");
    assert_eq!(es, 200, "percent-encoded slot still reaches the handler (body: {eb})");
    let lv: serde_json::Value = serde_json::from_str(&lb).unwrap();
    let ev: serde_json::Value = serde_json::from_str(&eb).unwrap();
    assert_eq!(lv["slot"], "lock/x");
    assert_eq!(ev["slot"], "lock/x", "the extractor decodes the tail");
}

#[tokio::test]
async fn regression_node_level_routes_require_bearer_when_token_set() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});

    // Gated: 401 bare, 200 with the configured token.
    let r = client.post(format!("{base}/mcp")).json(&init).send().await.unwrap();
    assert_eq!(r.status(), 401, "/mcp must demand the bearer when a token is set");
    let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer secret")
        .json(&init).send().await.unwrap();
    assert_eq!(r.status(), 200, "the configured token admits /mcp");

    let r = client.get(format!("{base}/signals/probe-kind")).send().await.unwrap();
    assert_eq!(r.status(), 401, "/signals/{{kind}} must demand the bearer");
    let r = client.get(format!("{base}/signals/probe-kind"))
        .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
    assert_eq!(r.status(), 200, "the configured token admits the SSE stream");
    drop(r);

    #[cfg(feature = "consensus")]
    {
        let r = client.get(format!("{base}/consensus/some-slot")).send().await.unwrap();
        assert_eq!(r.status(), 401, "/consensus/{{slot}} must demand the bearer");
        let r = client.get(format!("{base}/consensus/some-slot"))
            .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
        assert_eq!(r.status(), 200, "the configured token admits slot inspection");
    }

    // Public by design — never 401. (`/ready` may be 503 this early; `/metrics` is 404
    // without the `metrics` feature — the property under test is "no bearer demanded".)
    for path in ["/health", "/ready", "/stats", "/metrics"] {
        let r = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_ne!(r.status(), 401, "{path} is public (M16 edge criterion)");
    }
    let r = client.get(format!("{base}/bulk/0000000000000001")).send().await.unwrap();
    assert_eq!(r.status(), 404, "/bulk/{{id}} is a nonce-capability URL: public, unknown nonce → 404");

    agent.shutdown().await;
}

/// Run-61 falsification probe (Security): bearer header *shapes* that a lax parser might
/// admit — lowercase scheme, double space, trailing space, scheme only, token in a query
/// string, Basic scheme, and the token as a cookie. None may reach a gated handler: every
/// one is 401; only the exact `Bearer <token>` is 200.
#[tokio::test]
async fn probe_bearer_header_shapes_are_not_accepted() {
    use axum::http::header::{AUTHORIZATION, COOKIE};
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    // Not in the list: a *trailing* space (`"Bearer secret "`) — HTTP parsers strip trailing
    // OWS from field values (RFC 9110 §5.5), so the server sees the exact token; the probe's
    // first run asserted it must be refused and learned that it is admitted *by the spec*, not
    // by a lax comparison. Internal double space is NOT stripped → refused.
    for bad in ["bearer secret", "Bearer  secret", "Bearer", "BEARER secret",
                "Basic c2VjcmV0", "Token secret", "Bearer secre", "Bearer secrett"] {
        let r = client.get(format!("{base}/gateway/kv/keys")).header(AUTHORIZATION, bad).send().await.unwrap();
        assert_eq!(r.status(), 401, "header {bad:?} must be refused");
    }
    let r = client.get(format!("{base}/gateway/kv/keys?token=secret")).send().await.unwrap();
    assert_eq!(r.status(), 401, "a query-string token must be refused");
    let r = client.get(format!("{base}/gateway/kv/keys")).header(COOKIE, "token=secret").send().await.unwrap();
    assert_eq!(r.status(), 401, "a cookie token must be refused");
    let r = client.get(format!("{base}/gateway/kv/keys")).header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
    assert_eq!(r.status(), 200, "the exact form is admitted");
    agent.shutdown().await;
}

/// **What this checks, and what it cannot** (#587 F4): with a bearer configured, every path in
/// `PUBLIC_PATHS` is **mounted** (an unsupported method answers 405, whatever the handler does
/// — `/metrics` answers 404 by design in a build without `metrics`) and **public** (its own
/// method never answers 401); the gated routes — the library's own, the node-level three, and a
/// route a companion merged under `/gateway/` — answer 401 without it. It **cannot** find a
/// public route nobody listed: axum does not enumerate a router's routes, and a companion's
/// router is not built here. Companion public paths are listed in `COMPANION_PUBLIC_PATHS` and
/// rbac.md, and a new route outside the gated prefixes is a review rule, as rbac.md says.
#[cfg(feature = "a2a")]
#[tokio::test]
async fn every_listed_public_path_is_mounted_and_public_and_gated_paths_are_not() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    let agent = Arc::new(GossipAgent::new(id, cfg).with_a2a());
    async fn ok() -> &'static str { "ok" }
    agent.with_http_routes(axum::Router::new().route("/gateway/app/protected", axum::routing::get(ok)));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    for path in super::PUBLIC_PATHS {
        let concrete = path.replace("{corr_id}", "0");
        let r = if *path == "/a2a" {
            client.post(format!("{base}{concrete}")).json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tasks/get","params":{"id":"x"}})).send().await.unwrap()
        } else {
            client.get(format!("{base}{concrete}")).send().await.unwrap()
        };
        assert_ne!(r.status(), 401, "{path} is public: it never demands the bearer");
        // Mounted: a method the route does not take answers 405 from the router, never 404.
        let r = client.delete(format!("{base}{concrete}")).send().await.unwrap();
        assert_eq!(r.status(), 405, "{path} is mounted");
    }
    for (method, path) in [
        ("GET", "/gateway/kv/keys"), ("GET", "/gateway/app/protected"), ("GET", "/signals/probe"),
        ("POST", "/mcp"), ("GET", "/gateway/tuple/overview"),
    ] {
        let req = if method == "GET" { client.get(format!("{base}{path}")) } else { client.post(format!("{base}{path}")).json(&serde_json::json!({})) };
        let status = req.send().await.unwrap().status();
        // `/gateway/tuple/overview` is a companion route this node does not mount: a 404 from
        // the router proves nothing public answered there either.
        assert!(status == 401 || (path == "/gateway/tuple/overview" && status == 404),
            "{method} {path} is gated: {status}");
    }
    #[cfg(feature = "consensus")]
    assert_eq!(client.get(format!("{base}/consensus/x")).send().await.unwrap().status(), 401);
    agent.shutdown().await;
}

/// Scoped tokens on the node-level routes (compliance): `/mcp` needs `mcp:invoke`,
/// `/signals/{kind}` needs `mesh:read`; a token outside the family is refused 403 naming
/// the required scope; the wildcard admits everything.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn node_level_routes_honour_scoped_tokens() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "mcp-tok".into(),  scopes: vec!["mcp:invoke".into()] },
        crate::GatewayToken { token: "mesh-tok".into(), scopes: vec!["mesh:read".into()] },
        crate::GatewayToken { token: "kv-tok".into(),   scopes: vec!["kv:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});

    let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer mcp-tok")
        .json(&init).send().await.unwrap();
    assert_eq!(r.status(), 200, "mcp:invoke admits /mcp");
    let r = client.post(format!("{base}/mcp")).header(AUTHORIZATION, "Bearer kv-tok")
        .json(&init).send().await.unwrap();
    assert_eq!(r.status(), 403, "kv:read must not reach /mcp");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["required_scope"], "mcp:invoke");

    let r = client.get(format!("{base}/signals/k")).header(AUTHORIZATION, "Bearer mesh-tok")
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "mesh:read admits the SSE stream");
    drop(r);
    let r = client.get(format!("{base}/signals/k")).header(AUTHORIZATION, "Bearer mcp-tok")
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "mcp:invoke must not reach /signals");

    agent.shutdown().await;
}

/// **Closure plan C1.** Every raw route that takes an RPC kind from the body refuses protected
/// work, whatever the token holds (the legacy token holds `*`). A counting MCP tool on the target
/// proves the refusal happens before dispatch: nothing reaches the handler through any of the six.
/// The plant is a non-protected kind on `rpc/call`, which is not refused.
#[tokio::test]
async fn raw_routes_refuse_protected_kinds_and_the_handler_is_never_reached() {
    use axum::http::header::AUTHORIZATION;
    use base64::Engine as _;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    cfg.protected_rpc_kinds = vec!["depot.dispatch".into()];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();

    let reached = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&reached);
    let _tool = agent.mcp().register_mcp_tool("count", serde_json::json!({}), move |_args| {
        let r = Arc::clone(&r2);
        async move { r.fetch_add(1, Ordering::SeqCst); Ok(serde_json::json!("ran")) }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let call = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
                                  "params":{"name":"count","arguments":{}}});
    let b64 = base64::engine::general_purpose::STANDARD.encode(call.to_string());
    let target = id.to_string();

    for kind in ["mcp.invoke", "skill.invoke", "llm.invoke", "depot.dispatch"] {
        let routes: Vec<(&str, serde_json::Value)> = vec![
            ("rpc/call", serde_json::json!({"target": target, "method": kind, "payload_b64": b64, "timeout_secs": 1})),
            ("scatter", serde_json::json!({"targets": [target], "method": kind, "payload_b64": b64, "timeout_secs": 1})),
            ("signal/emit", serde_json::json!({"kind": kind, "scope": format!("node:{target}"), "payload_b64": b64})),
            ("mailbox/deliver", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64})),
            ("shard/emit", serde_json::json!({"kind": kind, "ns": "x", "name": "y", "shard_key": "k", "payload_b64": b64})),
            ("overlay/emit_reliable", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64, "timeout_secs": 1})),
        ];
        for (route, body) in routes {
            let r = client.post(format!("{base}/gateway/{route}")).header(AUTHORIZATION, "Bearer secret")
                .json(&body).send().await.unwrap();
            assert_eq!(r.status(), 403, "{route} must refuse `{kind}`");
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["error"], "protected_kind", "{route} names the refusal for `{kind}`: {v}");
        }
    }

    // The plant: an ordinary kind is not refused (nothing serves it, so it times out).
    let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer secret")
        .json(&serde_json::json!({"target": target, "method": "echo", "payload_b64": b64, "timeout_secs": 1}))
        .send().await.unwrap();
    assert_ne!(r.status(), 403, "a non-protected kind still passes the raw route");

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(reached.load(Ordering::SeqCst), 0, "no raw route reached the tool");
    agent.shutdown().await;
}

/// **The SSE doors streamed every protected RPC request to a `mesh:read` holder.** Both signal
/// streams registered a receiver for whatever kind the path named — the same table `rpc/serve`
/// and the native MCP tools register on, fanned to every receiver — so a `mesh:read` token could
/// open `/signals/mcp.invoke` and read each tool call's whole frame (`payload_b64`: the caller
/// envelope, the carried mandate and possession proof, the correlation nonce) while the raw
/// routes refused to *send* that kind. Observing protected work is refused with the same body
/// the raw routes use; an ordinary kind still streams. Seen failing first: both doors answered
/// 200 for `mcp.invoke`.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn sse_doors_refuse_protected_kinds() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.protected_rpc_kinds = vec!["depot.dispatch".into()];
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "mesh-tok".into(), scopes: vec!["mesh:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    for kind in ["mcp.invoke", "skill.invoke", "llm.invoke", "depot.dispatch"] {
        for path in [format!("/signals/{kind}"), format!("/gateway/signal/sse/{kind}")] {
            let r = client.get(format!("{base}{path}")).header(AUTHORIZATION, "Bearer mesh-tok")
                .send().await.unwrap();
            assert_eq!(r.status(), 403, "{path}: a mesh:read holder cannot observe protected work");
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["error"], "protected_kind", "{path} names the refusal: {v}");
            assert_eq!(v["kind"], kind, "{path}: {v}");
        }
    }
    // The plant: an ordinary kind still streams on both doors.
    for path in ["/signals/repair.request", "/gateway/signal/sse/repair.request"] {
        let r = client.get(format!("{base}{path}")).header(AUTHORIZATION, "Bearer mesh-tok")
            .send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}: an ordinary kind streams");
        drop(r);
    }
    agent.shutdown().await;
}

/// **Closure plan C1, the scope split.** `mesh:serve` serves and responds, and cannot call; a
/// `mesh:write` or `mesh:read` token is refused on the serve routes — the one-release
/// compatibility window from 2.15.0 is closed. A token with neither is refused too.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn mesh_serve_serves_and_responds_but_cannot_call() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "serve-tok".into(),  scopes: vec!["mesh:serve".into()] },
        crate::GatewayToken { token: "legacy-tok".into(), scopes: vec!["mesh:read".into()] },
        crate::GatewayToken { token: "kv-tok".into(),     scopes: vec!["kv:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let respond = serde_json::json!({"nonce_hex": "0000000000000001", "sender": id.to_string(), "result_b64": ""});

    let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer serve-tok")
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "mesh:serve opens the serve stream");
    drop(r);
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-tok")
        .json(&respond).send().await.unwrap();
    // The scope admits the route; the handler then refuses a nonce this principal was never
    // handed (`rpc_respond_answers_only_a_request_this_principal_was_handed`) — 403 from the
    // handler, not the 403 `required_scope` the scope layer sends.
    assert_eq!(r.status(), 403, "mesh:serve reaches the handler, which refuses an unserved nonce");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "unserved_request", "{v}");
    let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer serve-tok")
        .json(&serde_json::json!({"target": id.to_string(), "method": "echo", "timeout_secs": 1}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "mesh:serve cannot call");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:write");

    // The compatibility window (one release from 2.15.0) is closed: a token without `mesh:serve`
    // is refused on the serve routes, naming the scope (360 review, 2026-10-02 — the window
    // had outlived its promise by three releases).
    let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer legacy-tok")
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "a mesh:read token no longer serves");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:serve");
    let r = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer kv-tok")
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "a token with no mesh scope is refused");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:serve");
    agent.shutdown().await;
}

/// **`mesh:serve` bound to neither a kind nor a request: `rpc/respond` could answer any in-flight
/// RPC by nonce.** The route took `nonce_hex` and `sender` from the body with nothing binding them
/// to a request this principal's serve stream was handed, so any `mesh:serve` holder could pre-empt
/// any call it learned a nonce for (and release its parked admission). Now a request streamed on
/// `rpc/serve` is recorded against the principal that opened the stream, and `rpc/respond` answers
/// only a nonce that principal was handed — another principal, or a nonce never streamed, is
/// refused `403 unserved_request` and nothing is emitted. Seen failing first: the other principal's
/// reply answered 200 and the caller received it.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn rpc_respond_answers_only_a_request_this_principal_was_handed() {
    use axum::http::header::AUTHORIZATION;
    use futures_util::StreamExt as _;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "serve-1".into(), scopes: vec!["mesh:serve".into()] },
        crate::GatewayToken { token: "serve-2".into(), scopes: vec!["mesh:serve".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // Principal 1 serves `work`.
    let resp = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer serve-1")
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
    tokio::spawn(async move {
        let mut body = resp.bytes_stream();
        let mut buf = String::new();
        let mut seen_tx = Some(seen_tx);
        while let Some(Ok(chunk)) = body.next().await {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            if let Some(line) = buf.lines().find_map(|l| l.strip_prefix("data:"))
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                && let Some(tx) = seen_tx.take()
            {
                let _ = tx.send(v);
            }
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A member calls `work`; the request is streamed to principal 1.
    let caller = {
        let agent = Arc::clone(&agent);
        let id = id.clone();
        tokio::spawn(async move {
            agent.service().rpc_call(id, "work", b"go".to_vec(), Duration::from_secs(8)).await
        })
    };
    let streamed = tokio::time::timeout(Duration::from_secs(5), seen_rx).await
        .expect("the request was streamed").expect("stream open");
    let nonce_hex = streamed["nonce_hex"].as_str().unwrap().to_string();
    let sender = streamed["sender"].as_str().unwrap().to_string();

    // Principal 2 holds `mesh:serve` too, but was never handed this request.
    let forged = serde_json::json!({"nonce_hex": nonce_hex, "sender": sender, "result_b64": "Zm9yZ2Vk"});
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-2")
        .json(&forged).send().await.unwrap();
    assert_eq!(r.status(), 403, "another principal cannot answer a request it was not handed");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "unserved_request", "{v}");

    // A nonce nobody was handed, from the serving principal itself.
    let unknown = serde_json::json!({"nonce_hex": "00000000000000ff", "sender": sender, "result_b64": ""});
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
        .json(&unknown).send().await.unwrap();
    assert_eq!(r.status(), 403, "a nonce never streamed is refused");

    // The principal that was handed the request answers it, and the caller gets that reply.
    let real = serde_json::json!({"nonce_hex": nonce_hex, "sender": sender, "result_b64": "ZG9uZQ=="});
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
        .json(&real).send().await.unwrap();
    assert_eq!(r.status(), 200, "the serving principal answers");
    let reply = caller.await.unwrap();
    assert_eq!(reply.as_deref().ok(), Some(&b"done"[..]), "the caller received the served reply: {reply:?}");

    // Answered once: a second answer from the same principal is accepted idempotently and dropped
    // (#587 F1), and another principal is still refused.
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
        .json(&real).send().await.unwrap();
    assert_eq!(r.status(), 200, "a duplicate answer from the serving principal is idempotent");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["duplicate"], true, "{v}");
    let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-2")
        .json(&real).send().await.unwrap();
    assert_eq!(r.status(), 403, "another principal is refused after the answer too");
    agent.shutdown().await;
}

/// **#587 F1: replicated serve loops.** Two serve streams of one principal on one kind both
/// receive each request (a signal fans to every receiver), so both replicas answer. The first
/// answer is delivered; the second used to get `403 unserved_request`, which both SDKs raise —
/// killing the losing replica's serve loop. It is now `200 {"ok": true, "duplicate": true}`, the
/// reply dropped (never a second `rpc.result`) and counted (`rpc_respond_duplicates` on `/stats`).
/// Seen failing first: the second replica's answer was 403.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn a_replicated_serve_loop_answers_idempotently() {
    use axum::http::header::AUTHORIZATION;
    use futures_util::StreamExt as _;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "serve-1".into(), scopes: vec!["mesh:serve".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // Two replicas of one agent: same bearer, same kind.
    let mut seen = Vec::new();
    for _ in 0..2 {
        let resp = client.get(format!("{base}/gateway/rpc/serve/work")).header(AUTHORIZATION, "Bearer serve-1")
            .send().await.unwrap();
        assert_eq!(resp.status(), 200);
        let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
        tokio::spawn(async move {
            let mut body = resp.bytes_stream();
            let mut buf = String::new();
            let mut tx = Some(tx);
            while let Some(Ok(chunk)) = body.next().await {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                if let Some(line) = buf.lines().find_map(|l| l.strip_prefix("data:"))
                    && let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                    && let Some(tx) = tx.take()
                {
                    let _ = tx.send(v);
                }
            }
        });
        seen.push(rx);
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let caller = {
        let (agent, id) = (Arc::clone(&agent), id.clone());
        tokio::spawn(async move { agent.service().rpc_call(id, "work", b"go".to_vec(), Duration::from_secs(8)).await })
    };
    let mut answers = Vec::new();
    for (i, rx) in seen.into_iter().enumerate() {
        let v = tokio::time::timeout(Duration::from_secs(5), rx).await.expect("streamed").expect("open");
        let body = serde_json::json!({"nonce_hex": v["nonce_hex"], "sender": v["sender"],
                                      "result_b64": if i == 0 { "Zmlyc3Q=" } else { "c2Vjb25k" }});
        let r = client.post(format!("{base}/gateway/rpc/respond")).header(AUTHORIZATION, "Bearer serve-1")
            .json(&body).send().await.unwrap();
        answers.push(r.status());
    }
    assert_eq!(answers, vec![200, 200], "both replicas' answers are accepted");
    let reply = caller.await.unwrap();
    assert_eq!(reply.as_deref().ok(), Some(&b"first"[..]), "the first answer is the one delivered: {reply:?}");
    let stats: serde_json::Value = client.get(format!("{base}/stats")).send().await.unwrap().json().await.unwrap();
    assert_eq!(stats["rpc_respond_duplicates"], 1, "the dropped answer is counted: {stats}");
    agent.shutdown().await;
}

/// **#587 F7: the SSE doors streamed RPC replies.** A reply the interceptor did not claim — late,
/// or from a node the call was not sent to — falls through to the handler table, so
/// `/signals/rpc.result` streamed tool and LLM output to any `mesh:read` holder. Both doors now
/// refuse `rpc.result` and `bulk.result` as they refuse a protected kind. Seen failing first:
/// `/signals/rpc.result` answered 200.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn sse_doors_refuse_reply_kinds() {
    use axum::http::header::AUTHORIZATION;
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "mesh-tok".into(), scopes: vec!["mesh:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    for kind in ["rpc.result", "bulk.result"] {
        for path in [format!("/signals/{kind}"), format!("/gateway/signal/sse/{kind}")] {
            let r = client.get(format!("{base}{path}")).header(AUTHORIZATION, "Bearer mesh-tok")
                .send().await.unwrap();
            assert_eq!(r.status(), 403, "{path}: replies are not observable");
            let v: serde_json::Value = r.json().await.unwrap();
            assert_eq!(v["error"], "protected_kind", "{path}: {v}");
        }
    }
    agent.shutdown().await;
}

/// **#587 F2.** The served-request window (`SERVED_RPC_TTL_MS`) assumes no gateway door waits
/// longer; `/gateway/llm/call` took `timeout_ms` unclamped. Seen failing first (the clamp toggled
/// off): `llm_call_timeout(u64::MAX)` was ~584 million years.
#[test]
fn the_llm_door_timeout_is_clamped_to_the_gateway_ceiling() {
    use super::{gateway_rpc_timeout_ms, SERVED_RPC_TTL_MS};
    assert_eq!(gateway_rpc_timeout_ms(u64::MAX), Duration::from_millis(SERVED_RPC_TTL_MS));
    assert_eq!(gateway_rpc_timeout_ms(0), Duration::from_millis(1));
    assert_eq!(gateway_rpc_timeout_ms(30_000), Duration::from_secs(30));
}

/// Routes merged via `with_http_routes` sit behind the same auth boundary as the
/// library's own gateway routes: a merged `/gateway/…` path demands the bearer, a merged
/// path outside `/gateway/` stays public. Pre-fix (2026-09-04) the merged `/gateway/`
/// route answered 200 with no token — every companion's gateway surface was open.
#[tokio::test]
async fn test_merged_app_routes_under_gateway_prefix_require_auth() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());

    let agent = Arc::new(GossipAgent::new(id, cfg));
    async fn ok() -> &'static str { "ok" }
    agent.with_http_routes(
        axum::Router::new()
            .route("/gateway/app/protected", axum::routing::get(ok))
            .route("/app/public", axum::routing::get(ok)),
    );
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    let r = client.get(format!("{base}/gateway/app/protected")).send().await.unwrap();
    assert_eq!(r.status(), 401, "a merged /gateway/ route must demand the bearer");
    let r = client.get(format!("{base}/gateway/app/protected"))
        .header(AUTHORIZATION, "Bearer secret").send().await.unwrap();
    assert_eq!(r.status(), 200, "the configured token admits the merged /gateway/ route");
    let r = client.get(format!("{base}/app/public")).send().await.unwrap();
    assert_eq!(r.status(), 200, "a merged route outside /gateway/ stays public");

    agent.shutdown().await;
}

/// Falsification probe (Run 60): path shapes that a naive prefix guard might let past
/// the bearer on a merged route — percent-encoded prefix, double slash, trailing slash,
/// upper-case, dot-segments. None may reach the handler without a bearer: either the
/// guard fires (401) or the router does not match the shape (404). Never 200.
#[tokio::test]
async fn probe_merged_route_guard_survives_path_shapes() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("secret".into());
    let agent = Arc::new(GossipAgent::new(id, cfg));
    async fn ok() -> &'static str { "ok" }
    agent.with_http_routes(
        axum::Router::new()
            .route("/gateway/app/protected", axum::routing::get(ok))
            .route("/gateway/app/{*rest}", axum::routing::get(ok)),
    );
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    for shape in [
        "/gateway/app/protected",
        "/%67ateway/app/protected",
        "/gateway%2Fapp/protected",
        "/gateway//app/protected",
        "/gateway/app/protected/",
        "/GATEWAY/app/protected",
        "/gateway/app/../app/protected",
        "/gateway/app/deep/wild/card",
        "/gateway/app/%2e%2e/protected",
    ] {
        let r = client.get(format!("{base}{shape}")).send().await.unwrap();
        assert_ne!(r.status(), 200, "shape {shape} reached a merged /gateway/ handler without a bearer");
    }
    agent.shutdown().await;
}

/// Scoped tokens on merged companion routes (compliance): a route in the companion
/// scope table is admitted by its family and refused (403) outside it; an unlisted
/// merged `/gateway/` path is deny-by-default `admin`, which only the wildcard reaches.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_scoped_tokens_on_merged_companion_routes() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "llmro".into(), scopes: vec!["llm:read".into()] },
        crate::GatewayToken { token: "wikiw".into(), scopes: vec!["wiki:write".into()] },
        crate::GatewayToken { token: "artpub".into(), scopes: vec!["artifact:publish".into()] },
        crate::GatewayToken { token: "kvw".into(), scopes: vec!["kv:write".into()] },
        crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id, cfg));
    async fn ok() -> &'static str { "ok" }
    agent.with_http_routes(
        axum::Router::new()
            .route("/gateway/reason/trace/{run_id}", axum::routing::get(ok))
            .route("/gateway/wiki/ingest", axum::routing::post(ok))
            .route("/gateway/artifacts/publish", axum::routing::post(ok))
            .route("/gateway/wiki/some/future/verb", axum::routing::post(ok)),
    );
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let get = |path: &str, tok: &str| client.get(format!("{base}{path}")).header(AUTHORIZATION, format!("Bearer {tok}")).send();
    let post = |path: &str, tok: &str| client.post(format!("{base}{path}")).header(AUTHORIZATION, format!("Bearer {tok}")).send();

    assert_eq!(get("/gateway/reason/trace/r1", "llmro").await.unwrap().status(), 200, "llm:read reaches a reason read route");
    assert_eq!(post("/gateway/wiki/ingest", "llmro").await.unwrap().status(), 403, "llm:read is refused on wiki:write");
    assert_eq!(post("/gateway/wiki/ingest", "wikiw").await.unwrap().status(), 200, "wiki:write reaches wiki/ingest");
    let r = post("/gateway/wiki/some/future/verb", "wikiw").await.unwrap();
    assert_eq!(r.status(), 403, "an unlisted companion path is deny-by-default");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["required_scope"], "admin");
    assert_eq!(post("/gateway/wiki/some/future/verb", "super").await.unwrap().status(), 200, "wildcard reaches it");
    // A3: the artifact publish door has its own family; kv:write does not open it.
    assert_eq!(post("/gateway/artifacts/publish", "artpub").await.unwrap().status(), 200, "artifact:publish reaches artifacts/publish");
    assert_eq!(post("/gateway/artifacts/publish", "kvw").await.unwrap().status(), 403, "kv:write is not artifact:publish");
    assert_eq!(post("/gateway/artifacts/publish", "wikiw").await.unwrap().status(), 403, "wiki:write is not artifact:publish");

    agent.shutdown().await;
}

/// End-to-end: a scoped token is admitted on routes within its grant,
/// denied (403) on routes outside it, the wildcard token passes scope
/// gating everywhere, an unknown token is 401, and public routes stay open
/// with no credentials at all.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_scoped_token_acl_end_to_end() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "ro".into(),    scopes: vec!["kv:read".into()] },
        crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
    ];

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // Public route: open, no credentials.
    let r = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(r.status(), 200, "public /health must stay open");

    // No token on a protected route → 401.
    let r = client.get(format!("{base}/gateway/kv/keys")).send().await.unwrap();
    assert_eq!(r.status(), 401, "missing token must be unauthorized");

    // Unknown token → 401.
    let r = client.get(format!("{base}/gateway/kv/keys"))
        .header(AUTHORIZATION, "Bearer bogus").send().await.unwrap();
    assert_eq!(r.status(), 401, "unknown token must be unauthorized");

    // ro token on a kv:read route → admitted (not 401/403).
    let r = client.get(format!("{base}/gateway/kv/keys"))
        .header(AUTHORIZATION, "Bearer ro").send().await.unwrap();
    assert_eq!(r.status(), 200, "kv:read token must reach kv/keys");

    // ro token on a kv:write route → 403 (authenticated, insufficient scope).
    let r = client.post(format!("{base}/gateway/kv"))
        .header(AUTHORIZATION, "Bearer ro")
        .json(&serde_json::json!({"key": "k", "value": "v"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "kv:read token must be forbidden on kv:write");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["required_scope"], "kv:write");

    // super (wildcard) token on the same write route → passes scope gating.
    let r = client.post(format!("{base}/gateway/kv"))
        .header(AUTHORIZATION, "Bearer super")
        .json(&serde_json::json!({"key": "k", "value": "v"}))
        .send().await.unwrap();
    assert_ne!(r.status(), 401, "wildcard token must authenticate");
    assert_ne!(r.status(), 403, "wildcard token must pass scope gating");

    agent.shutdown().await;
}

#[tokio::test]
async fn regression_gateway_rejects_hostile_inputs_without_crashing() {
    // Two untrusted-input fixes on the (default loopback-open) gateway (audit 2026-07-15 pass 2):
    //   1. gw_kv_quorum: an out-of-range `timeout_secs` f64 fed `Duration::from_secs_f64`, which
    //      PANICS → node-abort under the release profile's panic="abort". Must be a clean 400.
    //   2. gw_signal_emit: an unknown `scope` string silently widened to a cluster-wide broadcast.
    //      Must be a 400, not a silent whole-cluster emit.
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // 1. Hostile timeout_secs (finite, JSON-serialisable — NaN/Inf aren't valid JSON so can't
    //    reach the handler) → clean 400, and the node stays up to serve the next request.
    for bad in [-1.0f64, -0.5, 1e300] {
        let r = client.post(format!("{base}/gateway/kv/quorum"))
            .json(&serde_json::json!({"key":"k","min_acks":1,"timeout_secs":bad}))
            .send().await.unwrap();
        assert_eq!(r.status(), 400, "hostile timeout_secs={bad} must be a clean 400, not a crash");
    }
    // A valid timeout still reaches the handler (min_acks=0 fast path → 200), proving the node
    // survived the hostile requests above.
    let r = client.post(format!("{base}/gateway/kv/quorum"))
        .json(&serde_json::json!({"key":"k","min_acks":0,"timeout_secs":1.0}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "node must survive and still serve a valid quorum request");

    // 2. Unknown scope → 400; a valid scope is accepted (not 400).
    let r = client.post(format!("{base}/gateway/signal/emit"))
        .json(&serde_json::json!({"kind":"k","scope":"grp:typo","payload_b64":""}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "unknown scope must be rejected, not widened to cluster-wide");
    let r = client.post(format!("{base}/gateway/signal/emit"))
        .json(&serde_json::json!({"kind":"k","scope":"cluster","payload_b64":""}))
        .send().await.unwrap();
    assert_ne!(r.status(), 400, "a valid 'cluster' scope must be accepted");

    agent.shutdown().await;
}

/// WS-C governance surface (Track 3): an operator publishes tuning + membership
/// intents over HTTP; the writes land in the gossip KV as evaporating soft-state,
/// malformed bodies are rejected, and the effective-state snapshot is served.
/// Open gateway here — scope gating is covered by the next test.
#[tokio::test]
async fn test_gateway_govern_publish_and_snapshot() {
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // Publish a tuning intent → 200, lands at sys/govern/fleet.
    let r = client.post(format!("{base}/gateway/govern/tuning"))
        .json(&serde_json::json!({
            "enabled": true,
            "params": [{"param": "writer_depth", "floor": 1024, "ceiling": 8192, "ratchet": "up"}]
        }))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["key"], "sys/govern/fleet");

    // Unknown param → 400.
    let r = client.post(format!("{base}/gateway/govern/tuning"))
        .json(&serde_json::json!({"params": [{"param": "nope"}]}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "unknown param must be rejected");

    // Empty intent (neither enabled nor params) → 400.
    let r = client.post(format!("{base}/gateway/govern/tuning"))
        .json(&serde_json::json!({}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "empty intent must be rejected");

    // Publish a membership intent → 200, lands at sys/govern/membership/workers.
    let r = client.post(format!("{base}/gateway/govern/membership"))
        .json(&serde_json::json!({"group": "workers", "min": 3, "max": 10}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["key"], "sys/govern/membership/workers");

    // Missing group → 400.
    let r = client.post(format!("{base}/gateway/govern/membership"))
        .json(&serde_json::json!({"min": 1}))
        .send().await.unwrap();
    assert_eq!(r.status(), 400, "membership intent without group must be rejected");

    // Both intents are now in the gossip KV (evaporating soft-state).
    assert!(agent.kv().get("sys/govern/fleet").is_some(), "tuning intent must be in KV");
    assert!(agent.kv().get("sys/govern/membership/workers").is_some(), "membership intent must be in KV");

    // Effective-state snapshot is served and well-formed.
    let r = client.get(format!("{base}/gateway/govern")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let snap: serde_json::Value = r.json().await.unwrap();
    assert!(snap["auto_enabled"].is_boolean());
    assert_eq!(snap["params"].as_array().unwrap().len(), 3);

    agent.shutdown().await;
}

/// Item 4 §7 over the gateway: `GET /gateway/govern` shows the node's profile and its
/// tripwires; `POST /gateway/govern/profile` steps the ladder, and the step reaches the tuning
/// governor's own copy (the fan-out) — read back through the same GET; an unknown name is
/// `400` and changes nothing.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn test_gateway_control_profile_round_trip() {
    use axum::http::header::AUTHORIZATION;
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("t".into());
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    let snap: serde_json::Value = client.get(format!("{base}/gateway/govern"))
        .header(AUTHORIZATION, "Bearer t").send().await.unwrap().json().await.unwrap();
    assert_eq!(snap["control"]["profile"], "legacy", "the default: {snap}");
    assert_eq!(snap["control"]["tuning"]["profile"], "legacy");
    assert_eq!(snap["control"]["would_hold"], 0);

    let r = client.post(format!("{base}/gateway/govern/profile"))
        .header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"profile": "observe"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!((body["profile"].as_str(), body["was"].as_str()), (Some("observe"), Some("legacy")));
    assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "the agent's reading");
    assert_eq!(agent.tuning_governor().profile, crate::control::Profile::Observe, "and the fan-out");
    let snap: serde_json::Value = client.get(format!("{base}/gateway/govern"))
        .header(AUTHORIZATION, "Bearer t").send().await.unwrap().json().await.unwrap();
    assert_eq!(snap["control"]["profile"], "observe");
    assert_eq!(snap["control"]["tuning"]["profile"], "observe");

    let r = client.post(format!("{base}/gateway/govern/profile"))
        .header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"profile": "enforce_local"})).send().await.unwrap();
    assert_eq!(r.status(), 400, "a misspelt name is refused");
    assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "and changes nothing");
    // Like every governance route: a non-object body and an unknown field are refused, not ignored — a
    // `target` here used to be dropped and the step applied to this node.
    for bad in [serde_json::json!(["observe"]), serde_json::json!({"profile": "legacy", "target": "127.0.0.1:1"})] {
        let r = client.post(format!("{base}/gateway/govern/profile"))
            .header(AUTHORIZATION, "Bearer t").json(&bad).send().await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
    assert_eq!(agent.control_profile(), crate::control::Profile::Observe, "and changes nothing");

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// Every governance write route — enumerated from the router in `mod.rs` and every family file, so a new one is covered the day it
/// lands — records one audit attempt per accepted change (#549's review: `govern/profile` recorded none).
#[cfg(feature = "gateway")]
#[tokio::test]
async fn every_governance_write_route_audits_its_change() {
    use axum::http::header::AUTHORIZATION;
    // Every `"/govern/…"` route literal in the module's production files — `mod.rs` (the router) and every family
    // file; the test files are not read — whatever its layout (a chained `.route(`, a feature-gated
    // `let gateway = gateway.route(`, a call split across lines); `/govern` alone is the read route. Scope-map and
    // doc lines carry `/gateway/govern/`, not a bare `"/govern/`. The routes are mounted in `mod.rs` only: a family
    // file with a `.route(` of its own would be a second router, so none may have one.
    let families: [(&str, &str); 15] = [
        ("authz.rs", include_str!("authz.rs")), ("caps.rs", include_str!("caps.rs")),
        // The cross-origin read layer (Y3): a layer, not a router — its read list names `/gateway/govern`, the read route.
        ("cors.rs", include_str!("cors.rs")),
        ("compliance.rs", include_str!("compliance.rs")), ("federation_routes.rs", include_str!("federation_routes.rs")),
        ("govern.rs", include_str!("govern.rs")), ("grants.rs", include_str!("grants.rs")), ("kv.rs", include_str!("kv.rs")),
        ("llm_bridge.rs", include_str!("llm_bridge.rs")), ("mcp_bridge.rs", include_str!("mcp_bridge.rs")),
        ("mesh.rs", include_str!("mesh.rs")), ("node.rs", include_str!("node.rs")),
        ("overlay.rs", include_str!("overlay.rs")), ("schedules.rs", include_str!("schedules.rs")),
        ("signals.rs", include_str!("signals.rs")),
    ];
    for (name, src) in families {
        assert!(!src.contains(".route("), "{name} mounts a route outside the router in mod.rs");
    }
    // …and the list above is every production file in the directory, so a new family is read too.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/agent/http");
    let on_disk: std::collections::BTreeSet<String> = std::fs::read_dir(&dir).expect("src/agent/http")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".rs") && n != "mod.rs" && !n.ends_with("_tests.rs"))
        .collect();
    let listed_files: std::collections::BTreeSet<String> = families.iter().map(|(n, _)| n.to_string()).collect();
    assert_eq!(on_disk, listed_files, "a production file in src/agent/http/ this test does not read");
    let production: String = std::iter::once(include_str!("mod.rs")).chain(families.iter().map(|(_, s)| *s)).collect();
    let routed: std::collections::BTreeSet<String> = production.split("\"/govern/").skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(|r| format!("/gateway/govern/{r}"))
        .collect();
    let bodies: Vec<(&str, serde_json::Value)> = vec![
        ("/gateway/govern/tuning", serde_json::json!({"enabled": true})),
        ("/gateway/govern/timing", serde_json::json!({"health_check_interval_secs": 5})),
        ("/gateway/govern/membership", serde_json::json!({"group": "workers", "min": 1})),
        ("/gateway/govern/topology-override", serde_json::json!({"group": "workers", "override": true})),
        ("/gateway/govern/profile", serde_json::json!({"profile": "observe"})),
        ("/gateway/govern/group", serde_json::json!({"group": "workers"})),
        ("/gateway/govern/electorate", serde_json::json!({"group": "workers"})),
    ];
    let listed: std::collections::BTreeSet<String> = bodies.iter().map(|(r, _)| r.to_string()).collect();
    assert_eq!(routed, listed, "a governance write route without a case here");

    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("t".into());
    // The electorate route declares this node a member: a member must configure `[persistence]`.
    let port = gossip_port;
    let base = std::env::temp_dir().join(format!("mycelium-electorate-{port}"));
    let _ = std::fs::remove_dir_all(&base);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base, sync_mode: crate::config::SyncMode::Flush, snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600, on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);
    let unaudited = || agent.task_ctx.governance_unaudited.load(std::sync::atomic::Ordering::Relaxed);
    for (route, body) in bodies {
        let url = format!("http://127.0.0.1:{http_port}{route}");
        // A refused body changes nothing and is not counted — validation comes before the audit.
        let before = changes();
        let r = client.post(&url).header(AUTHORIZATION, "Bearer t")
            .json(&serde_json::json!({"no_such_field": 1})).send().await.unwrap();
        assert_eq!(r.status(), 400, "{route}");
        assert_eq!(changes(), before, "{route} counted a refused body");
        let r = client.post(&url).header(AUTHORIZATION, "Bearer t").json(&body).send().await.unwrap();
        if !cfg!(feature = "consensus") && route == "/gateway/govern/electorate" {
            // An electorate is a consensus decision: without `consensus` the route says so and records nothing.
            assert_eq!(r.status(), 501, "{route}");
            assert_eq!(changes(), before, "{route} recorded a change it could not make");
            continue;
        }
        assert_eq!(r.status(), 200, "{route}: {}", r.text().await.unwrap_or_default());
        assert_eq!(changes(), before + 1, "{route} recorded no audit attempt");
    }
    // No `[tls]` identity here, so nothing could be sealed (and without `compliance` nothing is tried):
    // every accepted change is counted as unaudited.
    assert_eq!(unaudited(), changes());
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// A governed group's members decide its elections, so changing them is governance: `POST`/`DELETE
/// /gateway/mesh/group` (`mesh:write`) refuses a group under a membership intent **403** `governed_group`, naming
/// `/gateway/govern/group` (`govern:write`), which moves the node in or out; every membership change through either
/// route is audited or counted. A plain group stays a `mesh:write` join.
#[cfg(feature = "gateway")]
#[tokio::test]
async fn a_governed_groups_membership_moves_only_through_a_governance_route() {
    use axum::http::header::AUTHORIZATION;
    let (gossip_port, http_port) = (alloc_port(), alloc_port());
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("t".into());
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}/gateway");
    let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);

    let before = changes();
    let r = client.post(format!("{base}/mesh/group")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "plain"})).send().await.unwrap();
    assert_eq!(r.status(), 200, "a plain group is a mesh:write join");
    assert_eq!(changes(), before, "a plain group's membership is data plane, not a governance change");

    let r = client.post(format!("{base}/govern/membership")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "ruled", "min": 1})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = client.post(format!("{base}/mesh/group")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
    assert_eq!(r.status(), 403, "a governed group refuses the data-plane join");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "governed_group");
    assert!(body["message"].as_str().unwrap().contains("/gateway/govern/group"), "{body}");
    let r = client.delete(format!("{base}/mesh/group?group=ruled")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
    assert_eq!(r.status(), 403, "and the data-plane leave");

    let before = changes();
    let r = client.post(format!("{base}/govern/group")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["governed"], true, "{body}");
    assert!(body["members"].as_array().unwrap().iter().any(|m| m == &serde_json::json!(agent.node_id().to_string())), "{body}");
    assert_eq!(changes(), before + 1);
    // Joining again changes nothing, so nothing is recorded.
    let r = client.post(format!("{base}/govern/group")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "ruled"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(changes(), before + 1, "a no-op join is not a change");

    // The governed leave: as strict as the join — a `target` is refused, not dropped and applied here.
    for bad in ["group=ruled&target=10.0.0.5:9000", "group=", "group=a/b", ""] {
        let r = client.delete(format!("{base}/govern/group?{bad}")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
        assert_eq!(r.status(), 400, "{bad}");
    }
    let r = client.delete(format!("{base}/govern/group?group=ruled")).header(AUTHORIZATION, "Bearer t").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(changes(), before + 2, "the leave is a governance change");

    // A governed group is not redefined through units/declare (`cap:write`): its filter decides who the governor
    // can elect into it (#572's review).
    let unit = "[[group]]\nname = \"ruled\"\n  [group.filter]\n  ns   = \"depot\"\n  name = \"intake\"\n";
    let r = client.post(format!("{base}/units/declare")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({ "toml": unit })).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "governed_group");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A member without `[persistence]` is refused an electorate at the gateway**: `POST /gateway/govern/electorate`
/// answers **409** `persistence_required`, names the group, decides nothing and records nothing (`acceptor_guard`,
/// detection 1). Seen failing first: 200, epoch 1.
#[cfg(all(feature = "gateway", feature = "consensus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_persistence_is_refused_an_electorate_at_the_gateway() {
    use axum::http::header::AUTHORIZATION;
    let (gossip_port, http_port) = (alloc_port(), alloc_port());
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("t".into());
    assert!(cfg.persistence.is_none());
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let _listener = agent.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    agent.mesh().join_group("council");
    let client = reqwest::Client::new();
    let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);
    let before = (changes(), crate::electorate_persistence_refused());
    let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/govern/electorate")).header(AUTHORIZATION, "Bearer t")
        .json(&serde_json::json!({"group": "council"})).send().await.unwrap();
    assert_eq!(r.status(), 409);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "persistence_required", "{body}");
    assert!(body["message"].as_str().unwrap_or_default().contains("council"), "{body}");
    assert!(agent.electorate("council").is_none(), "nothing was decided");
    assert_eq!(changes(), before.0, "a refusal records nothing");
    assert!(crate::electorate_persistence_refused() > before.1, "and is counted");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **P2 at the gateway.** On a node that requires an electorate group for an exclusive outcome, the election and
/// lock routes refuse an ungoverned scope **403** `electorate_not_governed` and decide nothing. `POST
/// /gateway/govern/electorate` (`govern:write`) declares the group's electorate by consensus — here a genesis of
/// one, marked the fleet's exclusive default — and is audited once, when it is decided; after it the
/// data-plane join refuses the group **403** `governed_group`, the same election commits, the lock decides in the
/// fleet's electorate, and `GET /gateway/govern` lists it. A changed roster is **409** `membership_frozen` and
/// records nothing, as does a malformed body. Seen failing first: `/overlay/elect` on the undeclared group answered
/// 200 with this node as leader.
#[cfg(all(feature = "gateway", feature = "consensus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exclusive_outcome_at_the_gateway_needs_an_electorate_group() {
    use axum::http::header::AUTHORIZATION;
    let (gossip_port, http_port) = (alloc_port(), alloc_port());
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_auth_token = Some("t".into());
    cfg.consensus_require_electorate = true;
    // An electorate member must configure `[persistence]` (`acceptor_guard`, detection 1).
    let base = std::env::temp_dir().join(format!("mycelium-gw-electorate-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&base);
    cfg.persistence = Some(crate::config::PersistenceConfig {
        base_path: base.clone(), sync_mode: crate::config::SyncMode::Flush, snapshot_wal_threshold: 1_000,
        snapshot_interval_secs: 3_600, on_unreadable: crate::config::OnUnreadable::Refuse,
    });
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let _listener = agent.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}/gateway");
    let post = |path: &str, body: serde_json::Value| client.post(format!("{base}{path}")).header(AUTHORIZATION, "Bearer t").json(&body).send();

    let r = post("/mesh/group", serde_json::json!({"group": "council"})).await.unwrap();
    assert_eq!(r.status(), 200, "an undeclared group is a plain group");
    let r = post("/overlay/elect", serde_json::json!({"group": "council"})).await.unwrap();
    assert_eq!(r.status(), 403);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!((body["error"].as_str(), body["group"].as_str()), (Some("electorate_not_governed"), Some("council")), "{body}");
    let r = post("/overlay/lock/acquire", serde_json::json!({"name": "door", "ttl_secs": 5})).await.unwrap();
    assert_eq!(r.status(), 403);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["error"], "electorate_not_governed");
    assert!(body["group"].is_null(), "the whole cluster is not an electorate: {body}");
    let r = post("/overlay/consistent/set", serde_json::json!({"key": "cfg/k", "value_b64": "dg=="})).await.unwrap();
    assert_eq!(r.status(), 403, "the consistent write too");
    assert!(agent.consensus().consensus_get("leader/council").is_none(), "nothing was decided");

    let changes = || agent.task_ctx.governance_changes.load(std::sync::atomic::Ordering::Relaxed);
    let before = changes();
    for bad in [serde_json::json!({"group": "a/b"}), serde_json::json!({"group": "council", "exclusive_default": "yes"}),
                serde_json::json!({"group": "council", "size": 1}), serde_json::json!({"group": "council", "retire_to": "council"}),
                serde_json::json!({"group": "council", "retire_to": "next", "exclusive_default": true}),
                serde_json::json!({"group": "council", "predecessors": ["council"]}),
                serde_json::json!({"group": "council", "predecessors": ["x"], "retire_to": "next"}),
                serde_json::json!({"group": "council", "complete": true, "retire_to": "next"}),
                serde_json::json!({"group": "council", "complete": "yes"}),
                serde_json::json!({"group": "council", "drop_predecessor": "council"}),
                serde_json::json!({"group": "council", "drop_predecessor": "x", "complete": true}),
                serde_json::json!({"group": "council", "force": true}),
                serde_json::json!({"group": "council", "drop_predecessor": "x", "force": "yes"})] {
        assert_eq!(post("/govern/electorate", bad.clone()).await.unwrap().status(), 400, "{bad}");
    }
    assert_eq!(changes(), before, "a refused body records nothing");
    let r = post("/govern/electorate", serde_json::json!({"group": "council", "exclusive_default": true})).await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!((body["epoch"].as_u64(), body["changed"].as_bool()), (Some(1), Some(true)), "{body}");
    assert_eq!(changes(), before + 1, "a decided genesis is an audited governance change");
    let r = post("/govern/electorate", serde_json::json!({"group": "council", "exclusive_default": true})).await.unwrap();
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["changed"], false);
    assert_eq!(changes(), before + 1, "declaring the same electorate decides nothing and records nothing");

    let snap: serde_json::Value = client.get(format!("{base}/govern")).header(AUTHORIZATION, "Bearer t")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(snap["electorates"][0]["group"], "council", "{snap}");
    assert_eq!(snap["electorates"][0]["epoch"], 1, "{snap}");
    let r = post("/mesh/group", serde_json::json!({"group": "council"})).await.unwrap();
    assert_eq!(r.status(), 403, "an electorate group is governed");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "governed_group");
    let r = post("/overlay/elect", serde_json::json!({"group": "council"})).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["leader"], agent.node_id().to_string());
    let r = post("/overlay/lock/acquire", serde_json::json!({"name": "door", "ttl_secs": 5})).await.unwrap();
    assert_eq!(r.status(), 200, "the lock decides in the fleet's electorate");

    // A changed roster (an embedded write, a stranger) is not declared: membership is frozen. Nothing recorded.
    let stranger = NodeId::new("127.0.0.1", alloc_port()).unwrap();
    assert!(agent.kv().set(format!("grp/council/{stranger}"), bytes::Bytes::from_static(b"1")));
    let r = post("/govern/electorate", serde_json::json!({"group": "council", "exclusive_default": true})).await.unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "membership_frozen");
    assert_eq!(changes(), before + 1);
    // The way to change it: declare a successor naming it, retire it into it, complete — each audited — after
    // which the group is retired by name and the gateway shows the migration complete.
    assert_eq!(post("/mesh/group", serde_json::json!({"group": "council-2"})).await.unwrap().status(), 200);
    let r = post("/govern/electorate", serde_json::json!({"group": "council-2", "predecessors": ["council"]})).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["predecessors"], serde_json::json!(["council"]));
    let snap: serde_json::Value = client.get(format!("{base}/govern")).header(AUTHORIZATION, "Bearer t")
        .send().await.unwrap().json().await.unwrap();
    let of = |snap: &serde_json::Value, g: &str| snap["electorates"].as_array().unwrap().iter().find(|e| e["group"] == g).cloned().unwrap();
    assert_eq!(of(&snap, "council-2")["migration"][0]["phase"], "waiting", "{snap}");
    let r = post("/govern/electorate", serde_json::json!({"group": "council", "retire_to": "council-2"})).await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["retired_to"].as_str(), Some("council-2"), "{body}");
    assert_eq!(body["passes_default"], true, "{body}");
    assert_eq!(changes(), before + 3, "a successor's genesis and a retirement are audited governance changes");
    let r = post("/govern/electorate", serde_json::json!({"group": "council-2", "complete": true})).await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["migration"][0]["phase"], "complete", "{body}");
    assert_eq!(changes(), before + 4, "the completion is audited");
    let r = post("/govern/electorate", serde_json::json!({"group": "council-2", "complete": true})).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(changes(), before + 4, "a second completion run changes nothing");
    let snap: serde_json::Value = client.get(format!("{base}/govern")).header(AUTHORIZATION, "Bearer t")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(of(&snap, "council")["retired_to"], "council-2", "{snap}");
    assert_eq!(of(&snap, "council-2")["migration"][0]["phase"], "complete", "{snap}");
    let r = post("/govern/electorate", serde_json::json!({"group": "council", "exclusive_default": true})).await.unwrap();
    assert_eq!(r.status(), 409);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "retired");
    // The lock now decides in the successor, which holds the default.
    assert_eq!(post("/overlay/lock/acquire", serde_json::json!({"name": "door2", "ttl_secs": 5})).await.unwrap().status(), 200);
    // A predecessor that will never retire is dropped — audited once — after which the successor decides.
    for g in ["spare", "council-3"] { assert_eq!(post("/mesh/group", serde_json::json!({"group": g})).await.unwrap().status(), 200); }
    assert_eq!(post("/govern/electorate", serde_json::json!({"group": "spare"})).await.unwrap().status(), 200);
    assert_eq!(post("/govern/electorate", serde_json::json!({"group": "council-3", "predecessors": ["spare"]})).await.unwrap().status(), 200);
    let audited = changes();
    let r = post("/govern/electorate", serde_json::json!({"group": "council-3", "drop_predecessor": "spare"})).await.unwrap();
    assert_eq!(r.status(), 409, "spare may still retire: a drop needs its retirement elsewhere, or force");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "refused");
    let r = post("/govern/electorate", serde_json::json!({"group": "council-3", "drop_predecessor": "spare", "force": true})).await.unwrap();
    assert_eq!(r.status(), 200);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!((body["dropped"].as_str(), body["changed"].as_bool(), body["force"].as_bool()), (Some("spare"), Some(true), Some(true)), "{body}");
    assert_eq!(changes(), audited + 1, "a forced drop is audited");
    let r = post("/govern/electorate", serde_json::json!({"group": "council-3", "drop_predecessor": "spare", "force": true})).await.unwrap();
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["changed"], false, "a second drop changes nothing");
    assert_eq!(changes(), audited + 1);
    let snap: serde_json::Value = client.get(format!("{base}/govern")).header(AUTHORIZATION, "Bearer t")
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(of(&snap, "council-3")["migration"][0]["phase"], "dropped", "{snap}");
    assert_eq!(post("/overlay/elect", serde_json::json!({"group": "council-3"})).await.unwrap().status(), 200,
        "a successor whose only predecessor is dropped decides");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// With `compliance` and a `[tls]` identity every accepted governance change is sealed into this node's audit
/// stream — the profile step and an identity revocation included — and none is counted unaudited.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn governance_changes_are_sealed_with_an_identity() {
    use crate::config::TlsConfig;
    use axum::http::header::AUTHORIZATION;
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-govern-audit-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    cfg.gateway_scoped_tokens = vec![crate::GatewayToken {
        token: "gov".into(), scopes: vec!["govern:write".into(), "identity:write".into()],
    }];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let old_key = agent.identity_public_key().expect("tls identity");
    agent.rotate_identity(Duration::from_millis(0)).await.expect("rotate");
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}/gateway");
    for (route, body) in [
        ("govern/profile", serde_json::json!({"profile": "observe"})),
        ("govern/topology-override", serde_json::json!({"group": "workers", "override": true})),
        ("identity/revoke", serde_json::json!({"revoked_key": super::hex32(&old_key)})),
    ] {
        let r = client.post(format!("{base}/{route}")).header(AUTHORIZATION, "Bearer gov").json(&body).send().await.unwrap();
        assert_eq!(r.status(), 200, "{route}: {}", r.text().await.unwrap_or_default());
    }
    let sealed: Vec<String> = agent.audit_stream(&id).into_iter()
        .filter(|r| r.record.principal == "gateway/govern").map(|r| r.record.target).collect();
    for target in ["govern/profile", "sys/topology-override/workers", "identity/revoke"] {
        assert!(sealed.iter().any(|t| t == target), "{target} not sealed: {sealed:?}");
    }
    assert_eq!(agent.task_ctx.governance_unaudited.load(std::sync::atomic::Ordering::Relaxed), 0);
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-C governance scope gating (Track 3, compliance): `govern:read` reaches the
/// snapshot but is forbidden on a publish route; `govern:write` reaches publish.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_govern_scope_gating() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "gov-ro".into(), scopes: vec!["govern:read".into()] },
        crate::GatewayToken { token: "gov-rw".into(), scopes: vec!["govern:write".into()] },
    ];

    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // govern:read reaches the snapshot.
    let r = client.get(format!("{base}/gateway/govern"))
        .header(AUTHORIZATION, "Bearer gov-ro").send().await.unwrap();
    assert_eq!(r.status(), 200, "govern:read must reach the snapshot");

    // govern:read is forbidden on a publish route.
    let r = client.post(format!("{base}/gateway/govern/tuning"))
        .header(AUTHORIZATION, "Bearer gov-ro")
        .json(&serde_json::json!({"enabled": false}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "govern:read must be forbidden on publish");
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["required_scope"], "govern:write");

    // govern:write reaches the publish route.
    let r = client.post(format!("{base}/gateway/govern/tuning"))
        .header(AUTHORIZATION, "Bearer gov-rw")
        .json(&serde_json::json!({"enabled": false}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "govern:write must reach publish");

    // No token → 401.
    let r = client.get(format!("{base}/gateway/govern")).send().await.unwrap();
    assert_eq!(r.status(), 401, "missing token must be unauthorized");

    agent.shutdown().await;
}

/// Legible Emergence Phase 2: the `/gateway/fleet` snapshot endpoint is live and
/// scope-gated (`fleet:read`, deny-by-default) — a `fleet:read` token reads it and
/// gets the relational snapshot shape; a wrong-scope token is 403; no token is 401.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_fleet_snapshot_endpoint_scope_gated() {
    use axum::http::header::AUTHORIZATION;
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.emergent_detectors_enabled = true;
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "fleet-ro".into(), scopes: vec!["fleet:read".into()] },
        crate::GatewayToken { token: "kv-ro".into(),    scopes: vec!["kv:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    agent.mesh().join_group("workers"); // ungoverned: no membership intent anywhere
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // No token → 401.
    let r = client.get(format!("{base}/gateway/fleet")).send().await.unwrap();
    assert_eq!(r.status(), 401, "missing token unauthorized");
    // Wrong scope → 403 naming fleet:read.
    let r = client.get(format!("{base}/gateway/fleet"))
        .header(AUTHORIZATION, "Bearer kv-ro").send().await.unwrap();
    assert_eq!(r.status(), 403, "kv:read token forbidden on fleet:read");
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["required_scope"], "fleet:read");
    // fleet:read token → 200 with the relational snapshot shape.
    let r = client.get(format!("{base}/gateway/fleet"))
        .header(AUTHORIZATION, "Bearer fleet-ro").send().await.unwrap();
    assert_eq!(r.status(), 200, "fleet:read token admitted");
    let body: serde_json::Value = r.json().await.unwrap();
    assert!(body["view_confidence"]["observer"].is_string(), "snapshot carries the RT1 view_confidence header");
    assert!(body["governed_groups"].is_array());
    // #168: every group's observed size, governed or not — an ungoverned group was invisible here.
    assert_eq!(body["group_sizes"], serde_json::json!([{"group": "workers", "observed": 1}]), "{body}");
    assert!(body["throttle_graph"].is_array());
    assert!(body["store_hash"].is_number());
    agent.shutdown().await;
}

/// WS4: an OIDC JWT from a (mock) IdP is validated at the gateway and its
/// groups are mapped to scopes — a `readers`-group token reaches a `kv:read`
/// route but is forbidden on a `kv:write` route; no token is 401.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_oidc_jwt_maps_groups_to_scopes() {
    use axum::{routing::get, Router};
    use axum::http::header::AUTHORIZATION;
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use std::time::{SystemTime, UNIX_EPOCH};

    const TEST_PRIV: &str = include_str!("../../../tests/fixtures/oidc_test.key");
    let jwks_body = include_str!("../../../tests/fixtures/oidc_jwks.json");

    // ── Mock IdP: discovery + JWKS ───────────────────────────────────────
    let idp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let idp_port = idp_listener.local_addr().unwrap().port();
    let issuer = format!("http://127.0.0.1:{idp_port}");
    let disco = serde_json::json!({
        "issuer": issuer,
        "jwks_uri": format!("{issuer}/jwks"),
    }).to_string();
    let idp = Router::new()
        .route("/.well-known/openid-configuration", get(move || {
            let disco = disco.clone();
            async move { ([("content-type", "application/json")], disco) }
        }))
        .route("/jwks", get(move || {
            async move { ([("content-type", "application/json")], jwks_body) }
        }));
    let _idp = tokio::spawn(async move { axum::serve(idp_listener, idp).await.unwrap(); });

    // ── Mycelium node with OIDC configured ───────────────────────────────
    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut group_scopes = std::collections::HashMap::new();
    group_scopes.insert("readers".to_string(), vec!["kv:read".to_string()]);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.oidc = Some(crate::OidcConfig {
        issuer: issuer.clone(),
        audience: "mycelium-cluster".into(),
        group_claim: "groups".into(),
        group_scopes,
        jwks_uri: None, // exercise discovery
    });
    let agent = Arc::new(GossipAgent::new(id, cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // ── Mint a JWT for a "readers" user ──────────────────────────────────
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let claims = serde_json::json!({
        "sub": "alice", "iss": issuer, "aud": "mycelium-cluster",
        "exp": now + 3600, "groups": ["readers"],
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some("test-kid".to_string());
    let jwt = encode(&header, &claims, &EncodingKey::from_rsa_pem(TEST_PRIV.as_bytes()).unwrap()).unwrap();

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");

    // No token → 401.
    let r = client.get(format!("{base}/gateway/kv/keys")).send().await.unwrap();
    assert_eq!(r.status(), 401, "no token must be unauthorized");

    // OIDC JWT with kv:read → admitted on the kv:read route.
    let r = client.get(format!("{base}/gateway/kv/keys"))
        .header(AUTHORIZATION, format!("Bearer {jwt}")).send().await.unwrap();
    assert_eq!(r.status(), 200, "readers JWT must reach kv/keys (kv:read)");

    // Same JWT on a kv:write route → 403 (group grants only kv:read).
    let r = client.post(format!("{base}/gateway/kv"))
        .header(AUTHORIZATION, format!("Bearer {jwt}"))
        .json(&serde_json::json!({"key":"k","value":"v"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "readers JWT must be forbidden on kv:write");

    agent.shutdown().await;
}

/// WS2: the `/gateway/audit` endpoint returns the node's verified audit
/// stream to a token holding `audit:read`, and 403s a token without it.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_audit_endpoint_verifies_and_scope_gates() {
    use crate::config::TlsConfig;
    use crate::{AuditAction, AuditOutcome, GatewayToken};
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-audit-ep-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    cfg.gateway_scoped_tokens = vec![
        GatewayToken { token: "auditor".into(), scopes: vec!["audit:read".into()] },
        GatewayToken { token: "noaudit".into(), scopes: vec!["kv:read".into()] },
    ];

    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Seal two events into this node's stream.
    agent.audit(AuditAction::Invoke, "10.0.0.1:9000", "skill/a", AuditOutcome::Success, None).unwrap();
    agent.audit(AuditAction::Read, "10.0.0.2:9000", "kv/secret", AuditOutcome::Denied, None).unwrap();

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http_port}/gateway/audit");

    // Wrong scope → 403.
    let r = client.get(&url).header(AUTHORIZATION, "Bearer noaudit").send().await.unwrap();
    assert_eq!(r.status(), 403, "kv:read token must not reach the audit trail");

    // Correct scope → 200, verified stream with both records.
    let r = client.get(&url).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
    assert_eq!(r.status(), 200, "audit:read token must reach the audit trail");
    let body: serde_json::Value = r.json().await.unwrap();
    let streams = body["streams"].as_array().expect("streams array");
    let mine = streams.iter()
        .find(|s| s["node"] == id.to_string())
        .expect("this node's stream present");
    assert_eq!(mine["verified"], true, "honest stream must verify");
    assert!(mine["count"].as_u64().unwrap() >= 2, "both sealed records counted");
    assert!(mine["head_hash"].is_string(), "chain tip hash present");
    let recs = mine["records"].as_array().unwrap();
    assert!(recs.iter().all(|r| r["content_hash"].is_string()),
        "every record carries a citable content_hash");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// WS-D / D2 gate (G-D2): the `/gateway/transparency` endpoint serves a Merkle inclusion proof
/// that a fetcher verifies *locally* with the public [`verify_inclusion`], and the endpoint is
/// scope-gated.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn test_gateway_transparency_inclusion_proof_verifies_and_scope_gates() {
    use crate::config::TlsConfig;
    use crate::{verify_inclusion, GatewayToken, ProofStep};
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let cert_dir = std::env::temp_dir().join(format!("myc-transp-ep-{gossip_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    cfg.gateway_scoped_tokens = vec![
        GatewayToken { token: "auditor".into(), scopes: vec!["transparency:read".into()] },
        GatewayToken { token: "noaudit".into(), scopes: vec!["kv:read".into()] },
    ];

    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // Rotate so there is an old key to revoke, then revoke it (two revocations would build a
    // taller tree; one is enough to prove the inclusion path round-trips).
    let old_key = agent.identity_public_key().unwrap();
    agent.rotate_identity(Duration::from_millis(200)).await.unwrap();
    agent.revoke_identity_key(old_key).unwrap();
    tokio::time::sleep(Duration::from_millis(120)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}/gateway/transparency");
    let key_hex: String = old_key.iter().map(|b| format!("{b:02x}")).collect();

    // Wrong scope → 403.
    let r = client.get(&base).header(AUTHORIZATION, "Bearer noaudit").send().await.unwrap();
    assert_eq!(r.status(), 403, "kv:read token must not reach the transparency log");

    // Head: this node has a non-empty revocation root.
    let r = client.get(&base).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
    assert_eq!(r.status(), 200);
    let head: serde_json::Value = r.json().await.unwrap();
    let mine = head["nodes"].as_array().unwrap().iter()
        .find(|n| n["node"] == id.to_string()).expect("this node's head");
    assert!(mine["count"].as_u64().unwrap() >= 1, "the revocation is in the log");

    // Inclusion proof for the revoked key — verify it locally against the root.
    let url = format!("{base}?node={id}&key={key_hex}");
    let r = client.get(&url).header(AUTHORIZATION, "Bearer auditor").send().await.unwrap();
    let proof_doc: serde_json::Value = r.json().await.unwrap();
    assert_eq!(proof_doc["included"], true, "the revoked key is included");
    let hex32 = |s: &str| { let mut o = [0u8; 32];
        for i in 0..32 { o[i] = u8::from_str_radix(&s[i*2..i*2+2], 16).unwrap(); } o };
    let leaf = hex32(proof_doc["leaf"].as_str().unwrap());
    let root = hex32(proof_doc["root"].as_str().unwrap());
    let proof: Vec<ProofStep> = proof_doc["proof"].as_array().unwrap().iter().map(|s| ProofStep {
        sibling:  hex32(s["sibling"].as_str().unwrap()),
        on_right: s["on_right"].as_bool().unwrap(),
    }).collect();
    assert!(verify_inclusion(&leaf, &proof, &root), "the served proof verifies locally");

    // A tampered root must NOT verify.
    let mut bad_root = root;
    bad_root[0] ^= 0xff;
    assert!(!verify_inclusion(&leaf, &proof, &bad_root), "a tampered root is rejected");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// **External review 2026-10-31, F1 (plan row R1, decision D1).** A companion's operation reached through a raw
/// route needs the scope the companion's own route needs. Six raw routes, three families, read and write verbs:
/// a `mesh:write` holder without `board:write` is refused `403 insufficient scope` naming `board:write` on every
/// route that could carry `blackboard.board.post`; a holder of the wrong family (`tuple:write`) or the wrong verb
/// (`board:read`) is refused by name too; the right scope passes the gate (the kind then times out — nothing serves
/// it here, which is the point: the refusal is before dispatch); the `*` token passes; an ordinary kind is untouched.
/// Seen failing first with `companion_kind_scope` returning `None`: every refusal answered 200.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn raw_routes_require_a_companion_kinds_family_scope() {
    use axum::http::header::AUTHORIZATION;
    use base64::Engine as _;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    let raw = vec!["mesh:write".to_string(), "consensus:write".to_string()];
    let with = |extra: &str| { let mut s = raw.clone(); s.push(extra.into()); s };
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "raw-only".into(),    scopes: raw.clone() },
        crate::GatewayToken { token: "board-write".into(), scopes: with("board:write") },
        crate::GatewayToken { token: "board-read".into(),  scopes: with("board:read") },
        crate::GatewayToken { token: "tuple-write".into(), scopes: with("tuple:write") },
        crate::GatewayToken { token: "star".into(),        scopes: vec!["*".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"x");
    let target = id.to_string();
    let bodies = |kind: &str| -> Vec<(&'static str, serde_json::Value)> { vec![
        ("rpc/call", serde_json::json!({"target": target, "method": kind, "payload_b64": b64, "timeout_secs": 1})),
        ("scatter", serde_json::json!({"targets": [target], "method": kind, "payload_b64": b64, "timeout_secs": 1})),
        ("signal/emit", serde_json::json!({"kind": kind, "scope": format!("node:{target}"), "payload_b64": b64})),
        ("mailbox/deliver", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64})),
        ("shard/emit", serde_json::json!({"kind": kind, "ns": "x", "name": "y", "shard_key": "k", "payload_b64": b64})),
        ("overlay/emit_reliable", serde_json::json!({"target": target, "kind": kind, "payload_b64": b64, "timeout_secs": 1})),
    ]};
    let send = |token: &'static str, route: &'static str, body: serde_json::Value| {
        client.post(format!("{base}/gateway/{route}")).header(AUTHORIZATION, format!("Bearer {token}")).json(&body).send()
    };
    let refused = |v: &serde_json::Value, scope: &str, kind: &str, why: &str| {
        assert_eq!(v["error"], "insufficient scope", "{why}: {v}");
        assert_eq!(v["required_scope"], scope, "{why}: {v}");
        assert_eq!(v["kind"], kind, "{why}: {v}");
    };

    // Every raw route refuses the board's write kind to a caller without `board:write`, by name.
    for (route, body) in bodies("blackboard.board.post") {
        let r = send("raw-only", route, body.clone()).await.unwrap();
        assert_eq!(r.status(), 403, "{route}: mesh:write alone carried a board post");
        refused(&r.json().await.unwrap(), "board:write", "blackboard.board.post", route);
        let r = send("tuple-write", route, body.clone()).await.unwrap();
        assert_eq!(r.status(), 403, "{route}: the wrong family's scope carried a board post");
        let r = send("board-read", route, body.clone()).await.unwrap();
        assert_eq!(r.status(), 403, "{route}: the read scope carried a board post");
        refused(&r.json().await.unwrap(), "board:write", "blackboard.board.post", route);
        let r = send("board-write", route, body.clone()).await.unwrap();
        assert_ne!(r.status(), 403, "{route}: board:write was refused its own kind");
        let r = send("star", route, body).await.unwrap();
        assert_ne!(r.status(), 403, "{route}: the `*` token was refused");
    }
    // The other two families, and the read verbs.
    let cases: &[(&str, &str, &str)] = &[
        ("tuple.work.put",       "tuple:write", "raw-only"),
        ("tuple.work.take",      "tuple:write", "board-write"),
        ("wiki.council.ingest",  "wiki:write",  "raw-only"),
        ("wiki.council.access",  "wiki:read",   "raw-only"),
        ("blackboard.board.read", "board:read", "board-write"),
        ("blackboard.board.depth", "board:read", "raw-only"),
        ("tuple.work.depth",     "tuple:read",  "tuple-write"),
        ("blackboard.board",     "board:write", "board-read"),   // a malformed kind under the prefix: the write scope
    ];
    for (kind, scope, token) in cases {
        let (route, body) = bodies(kind).remove(0);
        let r = send(token, route, body).await.unwrap();
        assert_eq!(r.status(), 403, "{kind} under {token}");
        refused(&r.json().await.unwrap(), scope, kind, kind);
    }
    let (route, body) = bodies("blackboard.board.read").remove(0);
    let r = send("board-read", route, body).await.unwrap();
    assert_ne!(r.status(), 403, "board:read was refused a read kind");
    let (route, body) = bodies("tuple.work.depth").remove(0);
    let r = send("tuple-write", route, body).await.unwrap();
    assert_eq!(r.status(), 403, "tuple:write carried a read kind without tuple:read");

    // A body naming both fields is judged by the field the handler reads: `method` on rpc/call, `kind` elsewhere.
    let r = send("raw-only", "rpc/call", serde_json::json!({"target": target, "method": "blackboard.board.post", "kind": "echo", "payload_b64": b64, "timeout_secs": 1})).await.unwrap();
    assert_eq!(r.status(), 403, "rpc/call judged `kind` where the handler reads `method`");
    let r = send("raw-only", "signal/emit", serde_json::json!({"kind": "blackboard.board.post", "method": "echo", "scope": format!("node:{target}"), "payload_b64": b64})).await.unwrap();
    assert_eq!(r.status(), 403, "signal/emit judged `method` where the handler reads `kind`");

    // The plant: an ordinary kind is untouched, and the route's own scope is still required.
    for (route, body) in bodies("echo") {
        let r = send("raw-only", route, body).await.unwrap();
        assert_ne!(r.status(), 403, "{route}: an ordinary kind was refused");
    }
    let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer board-read")
        .json(&serde_json::json!({"target": target, "method": "blackboard.board.read", "payload_b64": b64, "timeout_secs": 1})).send().await.unwrap();
    assert_ne!(r.status(), 403);
    let mut cfg2 = GossipConfig::default();
    cfg2.bind_port = alloc_port();
    cfg2.http_port = Some(alloc_port());
    cfg2.gateway_scoped_tokens = vec![crate::GatewayToken { token: "board-only".into(), scopes: vec!["board:write".into()] }];
    let agent2 = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", cfg2.bind_port).unwrap(), cfg2.clone()));
    agent2.start().await.unwrap();
    let r = client.post(format!("http://127.0.0.1:{}/gateway/rpc/call", cfg2.http_port.unwrap())).header(AUTHORIZATION, "Bearer board-only")
        .json(&serde_json::json!({"target": target, "method": "blackboard.board.post", "payload_b64": b64, "timeout_secs": 1})).send().await.unwrap();
    assert_eq!(r.status(), 403, "board:write alone opened the raw route");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:write", "{v}");
    agent2.shutdown().await;
    agent.shutdown().await;
}

/// **#714's review, findings 1 and 2: the doors that take the kind from the path.** The two signal streams register a
/// tap on the handler table `rpc_rx` registers on and receive every frame of the kind, so a `mesh:read` holder read
/// each board post's payload while `bb/read` refused it; the serve stream answers the kind's requests in the
/// provider's place, so a `mesh:serve` holder served `blackboard.other.post` on a node that hosts no board. Now a
/// companion's kind on a signal stream needs the family's `:read` (whatever the verb — observing is reading), and on
/// the serve stream the family's `:write` (serving is answering writes). The path is read percent-decoded, as the
/// handler reads it. Seen failing first with `PATH_KIND_ROUTES` empty: every refusal answered 200.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn the_signal_streams_and_the_serve_stream_need_a_companion_kinds_family_scope() {
    use axum::http::header::AUTHORIZATION;

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![
        crate::GatewayToken { token: "mesh-read".into(),   scopes: vec!["mesh:read".into()] },
        crate::GatewayToken { token: "board-read".into(),  scopes: vec!["mesh:read".into(), "board:read".into()] },
        crate::GatewayToken { token: "board-write".into(), scopes: vec!["mesh:read".into(), "board:write".into()] },
        crate::GatewayToken { token: "serve-only".into(),  scopes: vec!["mesh:serve".into()] },
        crate::GatewayToken { token: "serve-board".into(), scopes: vec!["mesh:serve".into(), "board:write".into()] },
        crate::GatewayToken { token: "serve-read".into(),  scopes: vec!["mesh:serve".into(), "board:read".into()] },
    ];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let get = |token: &'static str, path: String| client.get(format!("{base}{path}")).header(AUTHORIZATION, format!("Bearer {token}")).send();
    let refused = async |r: reqwest::Response, scope: &str, kind: &str, why: &str| {
        assert_eq!(r.status(), 403, "{why}");
        let v: serde_json::Value = r.json().await.unwrap();
        assert_eq!(v["error"], "insufficient scope", "{why}: {v}");
        assert_eq!(v["required_scope"], scope, "{why}: {v}");
        assert_eq!(v["kind"], kind, "{why}: {v}");
    };

    // The signal streams: a companion's kind needs the family's `:read`, whatever the kind's verb.
    for door in ["/gateway/signal/sse", "/signals"] {
        for (kind, scope) in [("blackboard.board.post", "board:read"), ("blackboard.board.read", "board:read"),
                              ("tuple.work.take", "tuple:read"), ("wiki.council.ingest", "wiki:read")] {
            let r = get("mesh-read", format!("{door}/{kind}")).await.unwrap();
            refused(r, scope, kind, &format!("{door}/{kind} under mesh:read")).await;
        }
        // The write scope is not the read scope.
        let r = get("board-write", format!("{door}/blackboard.board.post")).await.unwrap();
        refused(r, "board:read", "blackboard.board.post", &format!("{door}: board:write observed a board kind")).await;
        // Percent-encoded, the path is the same kind.
        let r = get("mesh-read", format!("{door}/blackboard%2Eboard%2Epost")).await.unwrap();
        refused(r, "board:read", "blackboard.board.post", &format!("{door}: a percent-encoded kind slipped the gate")).await;
        // An ordinary kind streams as before.
        let r = get("mesh-read", format!("{door}/repair.request")).await.unwrap();
        assert_eq!(r.status(), 200, "{door}: an ordinary kind was refused");
        drop(r);
        // With `board:read` the stream opens and delivers the board kind's frames.
        let mut resp = get("board-read", format!("{door}/blackboard.board.post")).await.unwrap();
        assert_eq!(resp.status(), 200, "{door}: board:read was refused the board kind");
        let _ = agent.mesh().emit("blackboard.board.post", crate::SignalScope::Cluster, bytes::Bytes::from_static(b"frame"));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut found = false;
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(300), resp.chunk()).await {
                Ok(Ok(Some(chunk))) => {
                    if String::from_utf8_lossy(&chunk).contains("blackboard.board.post") { found = true; break; }
                }
                Ok(Ok(None)) => break,
                Ok(Err(_)) => break,
                Err(_) => continue,
            }
        }
        assert!(found, "{door}: the frame did not reach the board:read stream");
        drop(resp);
    }

    // The serve stream: a companion's kind needs the family's `:write`, whatever the verb — serving is answering.
    for kind in ["blackboard.other.post", "blackboard.other.read", "tuple.other.take"] {
        let scope = if kind.starts_with("blackboard") { "board:write" } else { "tuple:write" };
        let r = get("serve-only", format!("/gateway/rpc/serve/{kind}")).await.unwrap();
        refused(r, scope, kind, &format!("rpc/serve/{kind} under mesh:serve")).await;
    }
    let r = get("serve-read", "/gateway/rpc/serve/blackboard.other.post".into()).await.unwrap();
    refused(r, "board:write", "blackboard.other.post", "rpc/serve: board:read served a board kind").await;
    let r = get("serve-board", "/gateway/rpc/serve/blackboard.other.post".into()).await.unwrap();
    assert_eq!(r.status(), 200, "rpc/serve: mesh:serve + board:write was refused the board kind");
    drop(r);
    let r = get("serve-only", "/gateway/rpc/serve/work".into()).await.unwrap();
    assert_eq!(r.status(), 200, "rpc/serve: an ordinary kind was refused");
    drop(r);
    // `mesh:serve` is still required: the family scope alone does not serve.
    let r = get("board-write", "/gateway/rpc/serve/blackboard.other.post".into()).await.unwrap();
    assert_eq!(r.status(), 403);
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "mesh:serve", "{v}");
    agent.shutdown().await;
}

/// **#714's review, finding 5.** The raw-route gate reads the body before the handler's `Json` extractor, so a
/// request naming a companion's kind is refused `403` even where the handler would have answered `415` for its
/// content type; a body the gate cannot read as JSON passes through to the handler's own refusal.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn the_raw_route_gate_decides_before_the_handlers_extractor() {
    use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};

    let gossip_port = alloc_port();
    let http_port   = alloc_port();
    let id  = NodeId::new("127.0.0.1", gossip_port).unwrap();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.gateway_scoped_tokens = vec![crate::GatewayToken { token: "raw-only".into(), scopes: vec!["mesh:write".into()] }];
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let body = format!(r#"{{"target": "{id}", "method": "blackboard.board.post", "timeout_secs": 1}}"#);
    let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer raw-only")
        .header(CONTENT_TYPE, "text/plain").body(body.clone()).send().await.unwrap();
    assert_eq!(r.status(), 403, "a text/plain body naming a board kind reached the extractor's 415");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["required_scope"], "board:write", "{v}");
    let r = client.post(format!("{base}/gateway/rpc/call")).header(AUTHORIZATION, "Bearer raw-only")
        .header(CONTENT_TYPE, "application/json").body("not json {").send().await.unwrap();
    assert_eq!(r.status(), 400, "a body that is not JSON is the handler's refusal, not the gate's");
    agent.shutdown().await;
}
