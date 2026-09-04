//! **`ollama_serve` — one binary, PAIR-shaped.** Serve a local Ollama model into the mesh
//! with a live `llm-meta` ad, and expose the OpenAI-compatible façade — so any
//! OpenAI-speaking client, pointed at `http://127.0.0.1:HTTP_PORT/gateway/reason/v1`,
//! has its calls routed across every node that runs this (or any `serve_model`) for the
//! same model id. No proxy process, no control plane: each node routes from its own view.
//!
//! - `OLLAMA_URL` (default `http://127.0.0.1:11434`) — the local daemon.
//! - `MODEL`      (default `llama3`) — the Ollama model id to serve; becomes `llm/{MODEL}`.
//! - `BIND_PORT`  (required) · `HTTP_PORT` (required) · `BOOTSTRAP` (optional `host:port`).
//! - `GOSSIP_GATEWAY_AUTH_TOKEN` (optional) — when set, clients pass it as their API key.
//!
//! What it does that a plain proxy does not: the ad carries `engine=ollama`, `warm`,
//! `vram_used_mb`, `family`, `ctx_window`, `param_size`, `quant` (the `llm_meta`
//! vocabulary), refreshed every 15 s from the daemon — a Rust caller can constrain on
//! them (`ModelQuery::constraints`); the façade routes by model id and the router's load +
//! reservation rank. Try it, two terminals:
//!
//! ```text
//! BIND_PORT=7201 HTTP_PORT=8201 cargo run -p mycelium-reason --features llm,gateway,ollama --example ollama_serve
//! BIND_PORT=7202 HTTP_PORT=8202 BOOTSTRAP=127.0.0.1:7201 cargo run … --example ollama_serve
//! curl -s http://127.0.0.1:8201/gateway/reason/v1/models
//! curl -s http://127.0.0.1:8201/gateway/reason/v1/chat/completions -H 'content-type: application/json' \
//!   -d '{"model":"llama3","messages":[{"role":"user","content":"Name three root vegetables."}]}'
//! ```
//!
//! Manual (needs a running Ollama with `MODEL` pulled) — not a CI example.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mycelium::{GossipAgent, GossipConfig, NodeId, OpenAiBackend, PromptTemplate};
use mycelium_reason::{FsBlobStore, ModelProfile, OllamaProbe, reason_router, serve_model, spawn_meta_refresher};

fn required<T: std::str::FromStr>(name: &str) -> T {
    let raw = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required (see module doc)"));
    raw.parse().unwrap_or_else(|_| panic!("{name}={raw} did not parse"))
}

#[tokio::main]
async fn main() {
    let bind_port: u16 = required("BIND_PORT");
    let http_port: u16 = required("HTTP_PORT");
    let ollama = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
    let model = std::env::var("MODEL").unwrap_or_else(|_| "llama3".into());
    let bootstrap_peers = match std::env::var("BOOTSTRAP") {
        Ok(peer) => {
            let (host, port) = peer.rsplit_once(':').expect("BOOTSTRAP must be host:port");
            vec![NodeId::new(host, port.parse().expect("BOOTSTRAP port")).expect("BOOTSTRAP invalid")]
        }
        Err(_) => Vec::new(),
    };

    let mut cfg = GossipConfig { bind_port, http_port: Some(http_port), bootstrap_peers, ..Default::default() };
    cfg.apply_env_overrides().expect("GOSSIP_* env overrides"); // GOSSIP_GATEWAY_AUTH_TOKEN and friends
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", bind_port).expect("BIND_PORT"), cfg));
    let blobs = tempfile_dir();
    let store = Arc::new(FsBlobStore::open(&blobs).expect("blob dir"));
    agent.with_http_routes(reason_router(Arc::clone(&agent), store));
    agent.start().await.expect("agent start");

    // The prompt skill: system + history + input rendered for the chat backend. The
    // façade supplies `system` and `history` from the OpenAI messages.
    let template = PromptTemplate {
        system: "{{system}}".into(),
        user_template: "{{history}}\n{{input}}".into(),
        max_tokens: 512,
        temperature: 0.2,
        metadata: HashMap::new(),
    };
    // Ollama's OpenAI-compatible endpoint; the model is bound here, per node.
    let backend = Arc::new(OpenAiBackend::new(format!("{ollama}/v1"), "ollama", model.clone()));
    let profile = ModelProfile::new(model.clone());
    let reg = serve_model(&agent, profile.clone(), template, backend).await.expect("serve_model");
    let _refresher =
        spawn_meta_refresher(Arc::clone(&agent), reg, profile, OllamaProbe::new(ollama), Duration::from_secs(15));

    println!("serving llm/{model} — OpenAI base URL: http://127.0.0.1:{http_port}/gateway/reason/v1");
    let _ = tokio::signal::ctrl_c().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

fn tempfile_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mycelium-ollama-serve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp blob dir");
    dir
}
