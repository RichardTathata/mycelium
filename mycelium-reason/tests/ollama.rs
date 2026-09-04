//! The Ollama collector against a fake daemon, and the refresh gate: a served model's
//! `llm-meta` ad follows the daemon's warm/cold state, and every refresh becomes visible
//! to a constraint-bearing query within a short bound. (The bound would also catch the
//! theoretical retract-vs-advertise LWW race `ModelReg::refresh_meta` sequences against —
//! but note that race did *not* reproduce: a bare drop-and-advertise passed 60 flips at a
//! 1.5 s bound. The gate guards the refresh behaviour, not a reproduced race.)
#![cfg(all(feature = "gateway", feature = "llm", feature = "ollama"))]
#![allow(clippy::field_reassign_with_default)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use mycelium::{CapConstraint, CapFilter, CapValue, EchoBackend, GossipAgent, GossipConfig, NodeId, PromptTemplate};
use mycelium_reason::{ModelProfile, OllamaProbe, llm_meta, serve_model, spawn_meta_refresher};

/// A fake Ollama: `/api/ps` lists `llama3:latest` while `warm` is set; `/api/show` knows
/// `llama3` and nothing else.
async fn fake_ollama(warm: Arc<AtomicBool>) -> String {
    async fn ps(State(warm): State<Arc<AtomicBool>>) -> axum::Json<serde_json::Value> {
        let models = if warm.load(Ordering::SeqCst) {
            serde_json::json!([{ "name": "llama3:latest", "model": "llama3:latest", "size_vram": 4_400_000_000_u64 }])
        } else {
            serde_json::json!([])
        };
        axum::Json(serde_json::json!({ "models": models }))
    }
    async fn show(axum::Json(body): axum::Json<serde_json::Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        if body["model"] != "llama3" {
            return (axum::http::StatusCode::NOT_FOUND, "no such model").into_response();
        }
        axum::Json(serde_json::json!({
            "details": { "family": "llama", "parameter_size": "8B", "quantization_level": "Q4_K_M" },
            "model_info": { "general.architecture": "llama", "llama.context_length": 8192 },
        }))
        .into_response()
    }
    let app = axum::Router::new().route("/api/ps", get(ps)).route("/api/show", post(show)).with_state(warm);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_reads_process_list_and_show() {
    let warm = Arc::new(AtomicBool::new(true));
    let probe = OllamaProbe::new(fake_ollama(Arc::clone(&warm)).await);

    let st = probe.probe("llama3").await.unwrap();
    assert!(st.warm);
    assert_eq!(st.vram_used_mb, Some(4_400_000_000 / (1024 * 1024)));
    assert_eq!(st.family.as_deref(), Some("llama"));
    assert_eq!(st.param_size.as_deref(), Some("8B"));
    assert_eq!(st.quant.as_deref(), Some("Q4_K_M"));
    assert_eq!(st.ctx_window, Some(8192));

    warm.store(false, Ordering::SeqCst);
    let st = probe.probe("llama3").await.unwrap();
    assert!(!st.warm);
    assert_eq!(st.vram_used_mb, None);

    assert!(matches!(probe.probe("nope").await, Err(mycelium_reason::OllamaError::UnknownModel(_))));
}

/// The refresher keeps the ad current, and each refresh is visible to a constraint query
/// within 3 s — five warm/cold flips in a row, the stale value gone each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn refresher_follows_warm_state_without_blinking_out() {
    let warm = Arc::new(AtomicBool::new(false));
    let daemon = fake_ollama(Arc::clone(&warm)).await;

    let mut started = None;
    for _ in 0..16 {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if agent.start().await.is_ok() {
            started = Some(agent);
            break;
        }
    }
    let agent = started.expect("could not bind an agent after 16 attempts");

    let template = PromptTemplate {
        system: "echo".into(),
        user_template: "{{input}}".into(),
        max_tokens: 64,
        temperature: 0.0,
        metadata: HashMap::new(),
    };
    let base = ModelProfile::new("llama3");
    let reg = serve_model(&agent, base.clone(), template, Arc::new(EchoBackend)).await.unwrap();
    let _refresher = spawn_meta_refresher(
        Arc::clone(&agent),
        reg,
        base,
        OllamaProbe::new(daemon),
        Duration::from_millis(100),
    );

    let caps = agent.capabilities();
    let warm_query = |w: bool| CapFilter::new("llm-meta", "llama3").with(llm_meta::WARM, CapConstraint::Eq(CapValue::Bool(w)));
    let visible = |w: bool| !caps.resolve(&warm_query(w)).is_empty();
    async fn within(mut cond: impl FnMut() -> bool, bound: Duration) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < bound {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        cond()
    }

    // First probe: cold, with the statics filled from /api/show.
    assert!(within(|| visible(false), Duration::from_secs(3)).await, "cold ad visible");
    let ctx = CapFilter::new("llm-meta", "llama3").with(llm_meta::CTX_WINDOW, CapConstraint::Gte(CapValue::Integer(8192)));
    assert!(!caps.resolve(&ctx).is_empty(), "ctx_window came from /api/show");
    let engine = CapFilter::new("llm-meta", "llama3").with(llm_meta::ENGINE, CapConstraint::Eq(CapValue::Text(Arc::from("ollama"))));
    assert!(!caps.resolve(&engine).is_empty(), "engine=ollama advertised");

    for flip in 0..5 {
        let now_warm = flip % 2 == 0;
        warm.store(now_warm, Ordering::SeqCst);
        assert!(
            within(|| visible(now_warm), Duration::from_secs(3)).await,
            "flip {flip}: warm={now_warm} visible within the bound (the ad must not blink out)"
        );
        assert!(!visible(!now_warm), "flip {flip}: the stale value is gone");
    }

    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
