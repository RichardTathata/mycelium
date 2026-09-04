//! **`openai_serve` — the stacking example: Mycelium *over* an OpenAI-compatible engine.**
//!
//! Serve a model through **any** OpenAI-compatible endpoint — NVIDIA PAIR's proxy, LM Studio,
//! vLLM, Ollama's `/v1`, a cloud API — as a mesh capability `llm/{MODEL}`, and expose the
//! OpenAI-compatible façade so clients route across every node running this. The division
//! of labour the PAIR comparison settled on (`docs/plans/mycelium-reason.md`, 2026-09-04):
//! **the engine (or PAIR) places the GPU work; Mycelium routes the capability** — which node,
//! by load + local reservations, with failover. No proxy process of Mycelium's own.
//!
//! Unlike `ollama_serve`, nothing here reads the engine's process list: the `llm-meta` ad is
//! **static**, declared by env, so it works against endpoints that are not Ollama. Put the
//! attributes you know (`CTX_WINDOW`, `FAMILY`, `ENGINE`) on the ad and constraint-routing
//! sees them; leave them unset and the model is still routable by id.
//!
//! - `OPENAI_BASE_URL` (default `http://127.0.0.1:11434/v1`) — the engine's OpenAI root
//!   (PAIR: `http://<pair-host>:11434/v1`; LM Studio: `http://127.0.0.1:1234/v1`).
//! - `OPENAI_API_KEY` (default `none`) — sent as the bearer; local engines ignore it.
//! - `MODEL` (default `llama3`) — the engine's model id; becomes `llm/{MODEL}`.
//! - `ENGINE` (default `openai`) · `CTX_WINDOW` · `FAMILY` — optional static ad attributes.
//! - `BIND_PORT` (required) · `HTTP_PORT` (required) · `BOOTSTRAP` (optional `host:port`).
//! - `GOSSIP_GATEWAY_AUTH_TOKEN` (optional) — when set, clients pass it as their API key.
//!
//! Two terminals, then any OpenAI client at `http://127.0.0.1:8211/gateway/reason/v1`:
//!
//! ```text
//! OPENAI_BASE_URL=http://pair-host:11434/v1 BIND_PORT=7211 HTTP_PORT=8211 \
//!   cargo run -p mycelium-reason --features llm,gateway --example openai_serve
//! OPENAI_BASE_URL=http://127.0.0.1:1234/v1 BIND_PORT=7212 HTTP_PORT=8212 BOOTSTRAP=127.0.0.1:7211 \
//!   cargo run -p mycelium-reason --features llm,gateway --example openai_serve
//! curl -s http://127.0.0.1:8211/gateway/reason/v1/chat/completions -H 'content-type: application/json' \
//!   -d '{"model":"llama3","messages":[{"role":"user","content":"Name three root vegetables."}]}'
//! ```
//!
//! Deterministic run with no engine: `python3 examples/community/mock_llm.py` stands in for
//! the endpoint on `:11434` (the default `OPENAI_BASE_URL`), so the whole path — client →
//! façade → mesh route → `OpenAiBackend` → engine — runs on one laptop. Manual example, not
//! in CI; the façade and router it composes are CI-gated in the crate's test suites.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use mycelium::{CapValue, GossipAgent, GossipConfig, NodeId, OpenAiBackend, PromptTemplate};
use mycelium_reason::{FsBlobStore, ModelProfile, llm_meta, reason_router, serve_model};

fn required<T: std::str::FromStr>(name: &str) -> T {
    let raw = std::env::var(name).unwrap_or_else(|_| panic!("{name} is required (see module doc)"));
    raw.parse().unwrap_or_else(|_| panic!("{name}={raw} did not parse"))
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[tokio::main]
async fn main() {
    let bind_port: u16 = required("BIND_PORT");
    let http_port: u16 = required("HTTP_PORT");
    let base_url = optional("OPENAI_BASE_URL").unwrap_or_else(|| "http://127.0.0.1:11434/v1".into());
    let api_key = optional("OPENAI_API_KEY").unwrap_or_else(|| "none".into());
    let model = optional("MODEL").unwrap_or_else(|| "llama3".into());
    let bootstrap_peers = match optional("BOOTSTRAP") {
        Some(peer) => {
            let (host, port) = peer.rsplit_once(':').expect("BOOTSTRAP must be host:port");
            vec![NodeId::new(host, port.parse().expect("BOOTSTRAP port")).expect("BOOTSTRAP invalid")]
        }
        None => Vec::new(),
    };

    let mut cfg = GossipConfig { bind_port, http_port: Some(http_port), bootstrap_peers, ..Default::default() };
    cfg.apply_env_overrides().expect("GOSSIP_* env overrides");
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", bind_port).expect("BIND_PORT"), cfg));
    let blobs = std::env::temp_dir().join(format!("mycelium-openai-serve-{}", std::process::id()));
    std::fs::create_dir_all(&blobs).expect("temp blob dir");
    let store = Arc::new(FsBlobStore::open(&blobs).expect("blob dir"));
    agent.with_http_routes(reason_router(Arc::clone(&agent), store));
    agent.start().await.expect("agent start");

    // The prompt skill renders what the façade supplies: `system` + `history` + `input`.
    let template = PromptTemplate {
        system: "{{system}}".into(),
        user_template: "{{history}}\n{{input}}".into(),
        max_tokens: 512,
        temperature: 0.2,
        metadata: HashMap::new(),
    };
    let backend = Arc::new(OpenAiBackend::new(base_url.clone(), api_key, model.clone()));

    // A static `llm-meta` ad: what the operator knows about this endpoint's model.
    let mut profile = ModelProfile::new(model.clone()).with(
        llm_meta::ENGINE,
        CapValue::Text(Arc::from(optional("ENGINE").unwrap_or_else(|| "openai".into()).as_str())),
    );
    if let Some(ctx) = optional("CTX_WINDOW").and_then(|v| v.parse::<i64>().ok()) {
        profile = profile.with(llm_meta::CTX_WINDOW, CapValue::Integer(ctx));
    }
    if let Some(family) = optional("FAMILY") {
        profile = profile.with(llm_meta::FAMILY, CapValue::Text(Arc::from(family.as_str())));
    }
    let _reg = serve_model(&agent, profile, template, backend).await.expect("serve_model");

    println!("serving llm/{model} via {base_url} — OpenAI base URL: http://127.0.0.1:{http_port}/gateway/reason/v1");
    let _ = tokio::signal::ctrl_c().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
