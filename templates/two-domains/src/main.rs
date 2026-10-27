//! two-domains: two fleets, each its own domain under `secure-multi-domain`, calling each other across one edge.
//!
//! The food bank (`foodbank-gw`, `foodbank-1`) serves `pantry/stock` and `pantry/reserve`; the kitchen (`kitchen-gw`,
//! `kitchen-1`) serves `menu/today`. Each gateway runs a federation edge that signs its catalogue, binds every call's
//! body and meters each partner, and holds a client for the other, pinned to that gateway's key. A call made at one
//! gateway by its named local caller crosses the edge carrying that caller's name, and both domains' policies decide it
//! — the caller's before a byte leaves, the server's at the provider that serves it. Every node is one process:
//!
//! ```text
//! cargo run -- start <node>     # nodes/<node>.toml, with what this file attaches; runs until SIGINT/SIGTERM
//! cargo run -- doctor <node>    # what start() would say over the same attachments — never starts the node
//! cargo run -- keys <domain>    # a domain's signing key: secrets/<domain>.seed, and exchange/<domain>.pub to hand over
//! cargo run -- tokens <gateway> # its named local callers' tokens: secrets/<gateway>.tokens, read from the environment
//! cargo run -- tamper <node> <export>  # what a body rewritten on the path looks like to the partner's edge
//! cargo run -- get|post <node> <path> [json]  # a local caller at a gateway: its domain's CA, its named token
//! ```
mod authority;
mod fleet;

use mycelium::GossipAgent;
use std::sync::Arc;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["start", node] => start(node).await,
        ["doctor", node] => doctor(node),
        ["keys", domain] => fleet::keys(domain),
        ["tokens", node] => fleet::tokens(node),
        ["tamper", node, export] => fleet::tamper(node, export).await,
        ["get", node, path] => fleet::local_call(node, path, None).await,
        ["post", node, path, body] => fleet::local_call(node, path, Some(body)).await,
        _ => {
            eprintln!("usage: cargo run -- start <node> | doctor <node> | keys <domain> | tokens <gateway> | tamper <node> <export> | get|post <node> <path> [json]");
            2
        }
    };
    std::process::exit(code);
}

/// `doctor <node>`: the library's doctor over this node as `start` builds it — the file, then the attachments.
fn doctor(node: &str) -> i32 {
    let fleet = fleet::Fleet::load().unwrap_or_else(|e| fleet::refuse(&e));
    let report = mycelium::doctor::run_attached(Some(&fleet::node_path(node)), |agent| fleet.attach(node, agent));
    print!("{}", report.render());
    if report.refusal.is_some() { 3 } else { 0 }
}

/// `start <node>`: build, attach, start, serve this domain's exports if the node is a member that has them.
async fn start(node: &str) -> i32 {
    let fleet = fleet::Fleet::load().unwrap_or_else(|e| fleet::refuse(&e));
    let cfg = mycelium::GossipConfig::load_from_file(fleet::node_path(node)).unwrap_or_else(|e| fleet::refuse(&e.to_string()));
    let id = mycelium::NodeId::new(&cfg.bind_address, cfg.bind_port).unwrap_or_else(|e| fleet::refuse(&e.to_string()));
    let agent = Arc::new(fleet.attach(node, GossipAgent::new(id, cfg)));
    let shutdown = mycelium::shutdown::ShutdownSignal::install().expect("signal handlers");
    if let Err(e) = agent.start().await {
        eprintln!("{node}: start refused: {e}");
        return 3;
    }
    println!("{node}: up as {} under {}", agent.node_id(), agent.guarantee_report().profile.map(|p| format!("{} rev {}", p.name, p.revision)).unwrap_or_default());
    let _serving = fleet.serves(node).then(|| serve(&agent, &fleet.exports_of(node)));
    // Ready when this node can reach every other node of its domain and, for each, holds the marker that says it
    // verifies a gateway's caller context — a gateway dispatches to no provider whose marker it has not seen — and
    // when each of its domain's exports resolves to a provider.
    let peers = fleet.domain_peers(node);
    let exports = fleet.exports_of(node);
    let marker = |id: &mycelium::NodeId| agent.kv().get(&format!("{}{id}", mycelium::signal::kv_ns::CALLER_CONTEXT)).is_some();
    let provided = |e: &String| {
        let (ns, name) = e.split_once('/').expect("an export is ns/name");
        !agent.capabilities().resolve(&mycelium::CapFilter::new(ns, name)).is_empty()
    };
    for _ in 0..600 {
        let connected = agent.connected_peers();
        if peers.iter().all(|p| connected.contains(p) && marker(p)) && exports.iter().all(provided) {
            println!("{node}: ready — {} other node(s) of its domain connected", peers.len());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    shutdown.wait().await.ok();
    agent.shutdown().await;
    0
}

/// The exports, as skills this member provides. The answer names who asked — the partner's principal, carried across
/// the edge and checked here by the provider's own policy — and a `reserve` takes a while, as a real one might.
fn serve(agent: &Arc<GossipAgent>, exports: &[String]) -> Vec<mycelium::CapabilityReg> {
    let ads = exports.iter().map(|e| {
        let (ns, name) = e.split_once('/').expect("an export is ns/name");
        agent.capabilities().advertise_capability(mycelium::Capability::new(ns, name), Duration::from_secs(5))
    }).collect();
    let (a, mut rx) = (Arc::clone(agent), agent.service().rpc_rx("skill.invoke"));
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let a = Arc::clone(&a);
            tokio::spawn(async move {
                let text = String::from_utf8_lossy(&req.payload()).to_string();
                let who = a.request_principal(&req).map(|p| p.name()).unwrap_or_else(|e| format!("unknown ({e})"));
                let answer = if text.starts_with("reserve") {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    format!("reserved for {who}: {}", text.trim_start_matches("reserve").trim())
                } else if text.starts_with("menu") {
                    format!("for {who}: today's menu needs 12 crates of apples")
                } else {
                    format!("for {who}: 40 crates of apples, 12 of bread")
                };
                a.service().rpc_respond(&req, answer.into_bytes());
            });
        }
    });
    ads
}
