//! WS-G / M13 · G-G1c: the keyed two-stream rendezvous (fan-in join) drives across the HTTP
//! gateway — a consumer `take_by_key` parks until a producer `put` with the matching key arrives,
//! exactly the path the Python/TypeScript SDKs use over the wire.
//!
//! ER7 (R11-03, R11-10): items under one key form a FIFO queue — two outputs under one correlation are both
//! claimable, through `put_keyed`, through `complete_keyed` and across a WAL reopen — and each hand-off to a
//! parked worker is counted once in `taken`. These run on the Rust API (there is no gateway route for
//! `complete_keyed`) and need no gateway.

use bytes::Bytes;
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "gateway")]
fn b64(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(feature = "gateway")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn keyed_join_over_the_gateway() {
    let base: u16 = 23200 + (std::process::id() % 380) as u16 * 2;
    let http_port = base + 1;

    let mut cfg = GossipConfig::default();
    cfg.bind_port = base;
    cfg.http_port = Some(http_port);
    cfg.health_check_max_jitter_ms = 50;
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", base).expect("node id"),
        cfg,
    ));
    let ts = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig {
            namespace: Arc::from("join"),
            role: TupleRole::Primary,
            cap_refresh: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .await
    .expect("tuple space");
    agent.with_http_routes(Arc::clone(&ts).http_router());
    agent.start().await.expect("agent start");

    let url = format!("http://127.0.0.1:{http_port}");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("{url}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A consumer parks on take_by_key BEFORE the matching item exists (the join's blocking side).
    let url2 = url.clone();
    let http2 = http.clone();
    let consumer = tokio::spawn(async move {
        http2
            .post(format!("{url2}/gateway/tuple/take_by_key"))
            .json(&serde_json::json!({ "ns": "join", "stage": "po", "key": "inv-7", "timeout_secs": 8 }))
            .send()
            .await
            .expect("take_by_key request")
            .json::<serde_json::Value>()
            .await
            .expect("take_by_key json")
    });

    // Give the consumer a moment to park, then a NON-matching put (wrong key) must NOT satisfy it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    http.post(format!("{url}/gateway/tuple/put"))
        .json(&serde_json::json!({ "ns": "join", "stage": "po", "key": "inv-OTHER", "payload_b64": b64(b"nope") }))
        .send().await.expect("decoy put");

    // The matching keyed put completes the rendezvous.
    let resp: serde_json::Value = http
        .post(format!("{url}/gateway/tuple/put"))
        .json(&serde_json::json!({ "ns": "join", "stage": "po", "key": "inv-7", "payload_b64": b64(b"purchase-order") }))
        .send().await.expect("matching put").json().await.expect("put json");
    let matched_id = resp["id"].as_u64().expect("id");

    let claimed = consumer.await.expect("consumer task");
    assert_eq!(claimed["id"].as_u64(), Some(matched_id), "the keyed take claims the matching item");
    assert_eq!(claimed["key"].as_str(), Some("inv-7"));
    assert_eq!(claimed["payload_b64"].as_str(), Some(b64(b"purchase-order").as_str()));

    // The decoy (wrong key) is still queued — claimable only by its own key.
    let decoy: serde_json::Value = http
        .post(format!("{url}/gateway/tuple/take_by_key"))
        .json(&serde_json::json!({ "ns": "join", "stage": "po", "key": "inv-OTHER", "timeout_secs": 3 }))
        .send().await.expect("decoy take").json().await.expect("decoy json");
    assert_eq!(decoy["payload_b64"].as_str(), Some(b64(b"nope").as_str()));

    ts.shutdown().await;
    agent.shutdown().await;
}

// ── ER7 ──────────────────────────────────────────────────────────────────────

async fn start_agent() -> Arc<GossipAgent> {
    let port = mycelium::test_util::alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    cfg.health_check_max_jitter_ms = 50;
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).expect("node id"), cfg));
    agent.start().await.expect("agent start");
    agent
}

/// A primary on its own agent; WAL-backed at `wal` when given, with the stage watermark `high_watermark`.
async fn primary(ns: &str, wal: Option<&Path>, high_watermark: u32) -> (Arc<GossipAgent>, Arc<TupleSpace>) {
    let agent = start_agent().await;
    let ts = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig {
            namespace: Arc::from(ns),
            role: TupleRole::Primary,
            persist: wal.is_some(),
            wal_path: wal.map(Path::to_path_buf).unwrap_or_default(),
            high_watermark,
            cap_refresh: Duration::from_millis(200),
            ..Default::default()
        },
    )
    .await
    .expect("tuple space");
    (agent, ts)
}

fn temp_wal(name: &str) -> PathBuf {
    let tag = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("mts-er7-{}-{name}-{tag}.log", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// Takes every item under `key` on `stage`, acking each, until a take times out; returns the ids in order.
async fn drain_key(ts: &TupleSpace, stage: &str, key: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    while let Ok((id, _)) = ts.take_by_key(stage, key, Duration::from_millis(300)).await {
        ts.ack(id).await.expect("ack");
        ids.push(id);
    }
    ids
}

fn depth_of(ts: &TupleSpace, stage: &str) -> u32 {
    ts.local_depth(Some(stage)).expect("primary").iter().map(|d| d.depth).sum()
}

/// R11-03: two outputs accepted under one correlation are both claimable, in order — through `put_keyed` and
/// through `complete_keyed` — the depth returns to zero after the drain, and a watermark-2 stage accepts two more.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_outputs_under_one_key_are_both_claimable() {
    // Through put_keyed, on a WAL-backed primary.
    let wal = temp_wal("put-keyed");
    let (agent, ts) = primary("er7a", Some(&wal), 2).await;
    let a = ts.put_keyed("inv", "invoice-17", Bytes::from_static(b"line-1")).await.unwrap();
    let b = ts.put_keyed("inv", "invoice-17", Bytes::from_static(b"line-2")).await.unwrap();
    assert_eq!(drain_key(&ts, "inv", "invoice-17").await, vec![a, b], "both lines, in arrival order");
    assert_eq!(depth_of(&ts, "inv"), 0);
    for k in ["invoice-18", "invoice-19"] {
        ts.put_keyed("inv", k, Bytes::from_static(b"x")).await.expect("a watermark-2 stage accepts two more");
    }
    ts.shutdown().await;
    agent.shutdown().await;
    let _ = std::fs::remove_file(&wal);

    // Through complete_keyed: two inputs advanced into one keyed output.
    let wal = temp_wal("complete-keyed");
    let (agent, ts) = primary("er7b", Some(&wal), 2).await;
    for p in [&b"in-1"[..], &b"in-2"[..]] {
        ts.put("in", Bytes::copy_from_slice(p)).await.unwrap();
    }
    let mut outs = Vec::new();
    for _ in 0..2 {
        let (id, _) = ts.take("in", Duration::from_secs(1)).await.unwrap();
        outs.push(ts.complete_keyed(id, "out", "invoice-17", Bytes::from_static(b"o")).await.unwrap());
    }
    assert_eq!(drain_key(&ts, "out", "invoice-17").await, outs, "both outputs, in order");
    assert_eq!(depth_of(&ts, "out"), 0);
    for k in ["invoice-18", "invoice-19"] {
        ts.put_keyed("out", k, Bytes::from_static(b"x")).await.expect("a watermark-2 stage accepts two more");
    }
    ts.shutdown().await;
    agent.shutdown().await;
    let _ = std::fs::remove_file(&wal);
}

/// R11-03: a WAL holding two live records under one key recovers both on a reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reopened_wal_with_two_same_key_items_serves_both() {
    let wal = temp_wal("reopen");
    let (agent, ts) = primary("er7c", Some(&wal), 500).await;
    let a = ts.put_keyed("inv", "invoice-17", Bytes::from_static(b"line-1")).await.unwrap();
    let b = ts.put_keyed("inv", "invoice-17", Bytes::from_static(b"line-2")).await.unwrap();
    ts.shutdown().await;
    agent.shutdown().await;
    drop(ts);

    let (agent, ts) = primary("er7c", Some(&wal), 500).await;
    assert_eq!(depth_of(&ts, "inv"), 2);
    assert_eq!(drain_key(&ts, "inv", "invoice-17").await, vec![a, b], "both served after the reopen");
    assert_eq!(depth_of(&ts, "inv"), 0);
    ts.shutdown().await;
    agent.shutdown().await;
    let _ = std::fs::remove_file(&wal);
}

/// Polls until `n` workers are parked on `stage`, for up to two seconds.
async fn wait_parked(ts: &TupleSpace, stage: &str, n: u32) {
    for _ in 0..200 {
        if ts.local_depth(Some(stage)).expect("primary").iter().map(|d| d.waiters).sum::<u32>() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// R11-10: a hand-off to a parked worker is one take — `(admitted, taken)` is `(2, 2)` after one parked and one
/// buffered take, plain and keyed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_handoff_counts_each_item_once() {
    let (agent, ts) = primary("er7d", None, 500).await;
    let counts = |stage: &str| {
        let a = ts.admission(Some(stage)).expect("primary");
        (a[0].admitted, a[0].taken)
    };
    // Plain.
    let t = Arc::clone(&ts);
    let parked = tokio::spawn(async move { t.take("fifo", Duration::from_secs(5)).await });
    wait_parked(&ts, "fifo", 1).await;
    let first = ts.put("fifo", Bytes::from_static(b"1")).await.unwrap();
    let (got, _) = parked.await.unwrap().unwrap();
    assert_eq!(got, first);
    ts.ack(got).await.unwrap();
    assert_eq!(counts("fifo"), (1, 1), "the hand-off is one take");
    let second = ts.put("fifo", Bytes::from_static(b"2")).await.unwrap();
    let (got, _) = ts.take("fifo", Duration::from_secs(1)).await.unwrap();
    assert_eq!(got, second);
    ts.ack(got).await.unwrap();
    assert_eq!(counts("fifo"), (2, 2));
    // Keyed.
    let t = Arc::clone(&ts);
    let parked = tokio::spawn(async move { t.take_by_key("join", "c-1", Duration::from_secs(5)).await });
    wait_parked(&ts, "join", 1).await;
    let first = ts.put_keyed("join", "c-1", Bytes::from_static(b"1")).await.unwrap();
    let (got, _) = parked.await.unwrap().unwrap();
    assert_eq!(got, first);
    ts.ack(got).await.unwrap();
    assert_eq!(counts("join"), (1, 1), "the keyed hand-off is one take");
    let second = ts.put_keyed("join", "c-1", Bytes::from_static(b"2")).await.unwrap();
    let (got, _) = ts.take_by_key("join", "c-1", Duration::from_secs(1)).await.unwrap();
    assert_eq!(got, second);
    ts.ack(got).await.unwrap();
    assert_eq!(counts("join"), (2, 2));
    ts.shutdown().await;
    agent.shutdown().await;
}
