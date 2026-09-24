//! **A coordinator nobody declared** — the browser showcase.
//!
//! ```text
//! cargo run --example coordination_viz --features metrics
//! # then open http://127.0.0.1:8100/
//! ```
//!
//! The CLI twin, [`coordinator_by_accretion`](coordinator_by_accretion.rs), asserts the property
//! and exits. This one lets you *watch* it: a real three-node fleet, real ring names, and the real
//! `mycelium::election` API deciding who holds what — flipping between the rule that produced the
//! pathology and the rule that fixed it, every few seconds, while a concentration gauge tracks the
//! damage.
//!
//! # What you are watching
//!
//! Every node here is equally capable and runs every companion. Each single-writer ring elects
//! **independently**. Under *lowest candidate id wins*, all of them order the candidates the same
//! way — so the same node wins every ring, deterministically, and again after every restart. Nobody
//! declared that node a coordinator; it arrived by accretion.
//!
//! The thing worth noticing on screen is **not** that one node is busy. It is that nothing else in
//! the substrate was complaining: P2 watches role *churn* and a node calmly holding everything
//! produces none; P6 watches coverage *gaps* and every capability has a provider — the same one.
//! The fleet reads healthy on every other measurement, right up until that node goes away.

use mycelium::election::{winner, Rule};
use mycelium::{GossipAgent, GossipConfig, NodeId};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const HTTP_PORT: u16 = 8100;
const OPS_PORT: u16 = 8101;
const BASE_PORT: u16 = 57600;
const PHASE_MS: u64 = 4_000;

/// The concepts box (UI-example contract rule 4) — data here, drawn by the page.
const CONCEPTS: &str = r#"[
  {"tag":"IV","name":"capability rings","gloss":"single-writer roles: tuple-space curator · blackboard primary · wiki curator"},
  {"tag":"election","name":"mycelium::election","gloss":"rendezvous — hash(ring, node) — negotiated from the candidate set, never flag-dayed"},
  {"tag":"P10","name":"role concentration","gloss":"the share of live single-writer roles one node holds; on /stats, with a partition guard"},
  {"tag":"gateway","name":"gateway + metrics","gloss":"/stats · /gateway/fleet · /metrics — open this fleet in the Ops Console"}
]"#;

/// The single-writer rings a fully-provisioned node would join.
const RINGS: [(&str, &str); 3] = [
    ("tuple/depot.primary", "tuple-space curator"),
    ("blackboard/depot.primary", "blackboard primary"),
    ("wiki/depot.curator", "wiki curator"),
];

fn concentration_pct(holders: &[String]) -> u64 {
    let mut counts: HashMap<&String, u64> = HashMap::new();
    for h in holders {
        *counts.entry(h).or_insert(0) += 1;
    }
    counts.values().copied().max().unwrap_or(0) * 100 / holders.len().max(1) as u64
}

#[tokio::main]
async fn main() {
    #[cfg(feature = "metrics")]
    let _ = metrics_exporter_prometheus::PrometheusBuilder::new().install();

    // A real fleet — three equally-capable nodes, each of which would run every companion.
    let mut agents: Vec<Arc<GossipAgent>> = Vec::new();
    for i in 0..3u16 {
        let port = BASE_PORT + i;
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.bootstrap_peers = if i == 0 {
            vec![]
        } else {
            vec![NodeId::new("127.0.0.1", BASE_PORT).expect("bootstrap")]
        };
        if i == 0 {
            cfg.http_port = Some(OPS_PORT);
        }
        let a = Arc::new(GossipAgent::new(
            NodeId::new("127.0.0.1", port).expect("node id"), cfg));
        a.start().await.expect("start");
        agents.push(a);
    }

    // The Ops Console click-through convention (`ui/viz` + `ui/label`).
    let _ = agents[0].kv().set("ui/viz", format!("http://127.0.0.1:{HTTP_PORT}/"));
    let _ = agents[0].kv().set("ui/label", "Coordinator by accretion".to_string());

    let nodes: Vec<NodeId> = agents.iter().map(|a| a.node_id().clone()).collect();
    println!("coordination_viz — open http://127.0.0.1:{HTTP_PORT}/   (Ctrl-C to stop)");

    let tick = Arc::new(AtomicU64::new(0));
    {
        let tick = Arc::clone(&tick);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(PHASE_MS)).await;
                tick.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", HTTP_PORT))
        .await
        .expect("bind viz port");

    loop {
        let Ok((mut stream, _)) = listener.accept().await else { continue };
        let nodes = nodes.clone();
        let tick = Arc::clone(&tick);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let wants_state = req.starts_with("GET /state");

            if wants_state {
                let t = tick.load(Ordering::Relaxed);
                // Alternate the rule every phase, so the contrast is the thing you watch.
                let (rule, rule_name) = if t % 2 == 0 {
                    (Rule::LowestId, "lowest candidate id wins")
                } else {
                    (Rule::Rendezvous, "rendezvous — hash(ring, node)")
                };

                let mut rings = Vec::new();
                let mut holders = Vec::new();
                for (ring, label) in RINGS {
                    let w = winner(ring, &nodes, rule)
                        .expect("a non-empty candidate set elects")
                        .to_string();
                    rings.push(format!(
                        r#"{{"ring":"{ring}","label":"{label}","holder":"{w}"}}"#));
                    holders.push(w);
                }
                let pct = concentration_pct(&holders);
                let distinct = holders.iter().collect::<HashSet<_>>().len();
                let verdict = if pct == 100 {
                    "one node holds EVERY single-writer role — and P2 (churn) and P6 (gaps) both read this fleet as healthy"
                } else {
                    "spread across holders — a failover now moves one role, not all of them as a block"
                };

                let json = format!(
                    r#"{{"rule":"{rule_name}","lowest_id":{lowest},"nodes":{nodes_json},"rings":[{rings}],"concentration_pct":{pct},"distinct_holders":{distinct},"verdict":"{verdict}","tick":{t}}}"#,
                    lowest = (t % 2 == 0),
                    nodes_json = format!(
                        "[{}]",
                        nodes.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(",")),
                    rings = rings.join(","),
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json.len(), json);
                let _ = stream.write_all(resp.as_bytes()).await;
            } else {
                let console_link = if cfg!(feature = "gateway") {
                    format!(
                        "<a class=\"opsbtn\" href=\"http://127.0.0.1:8099/?target=127.0.0.1:{OPS_PORT}\" \
                         title=\"Open this cluster in the Mycelium Ops Console\">⚙ Ops Console</a>")
                } else {
                    String::new()
                };
                let html = include_str!("coordination_viz.html")
                    .replace("__OPS_CONSOLE_LINK__", &console_link)
                    .replace("__CONCEPTS__", CONCEPTS);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(), html);
                let _ = stream.write_all(resp.as_bytes()).await;
            }
        });
    }
}
