//! The Ollama collector (feature `ollama`) — fills the [`llm_meta`](crate::llm_meta)
//! vocabulary from a local Ollama daemon and keeps a served model's `llm-meta/{model}` ad
//! current as the engine's state changes.
//!
//! Two engine surfaces are read: `GET /api/ps` (the *process list* — which models are
//! resident right now and how much accelerator memory each holds) and `POST /api/show`
//! (static facts: family, parameter size, quantization, context length). The collector
//! is a **serve-side** concern: it runs on the node that serves the model, next to the
//! daemon, and what it learns becomes attributes on that node's ad — the mesh sees typed
//! capability attributes, never Ollama. Nothing here routes; the router stays engine-blind.
//!
//! The dynamic attributes (`warm`, `vram_used_mb`) are the ones a placement decision most
//! wants and engines least advertise: a cold model pays its load time before the first
//! token. [`spawn_meta_refresher`] re-probes on an interval and re-advertises only on
//! change, through [`ModelReg::refresh_meta`] (which sequences retract-then-advertise so the
//! ad never blinks out — see its doc).

use std::sync::Arc;
use std::time::Duration;

use mycelium::{CapValue, GossipAgent};

use crate::route::{ModelProfile, ModelReg, llm_meta};

/// Reads one Ollama daemon's view of a model.
#[derive(Clone)]
pub struct OllamaProbe {
    base_url: String,
    client: reqwest::Client,
}

/// What the daemon reported for one model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OllamaModelState {
    /// Listed in `/api/ps` — weights resident now.
    pub warm: bool,
    /// `size_vram` from the process list, in MiB (only while warm).
    pub vram_used_mb: Option<i64>,
    /// `details.family` from `/api/show`.
    pub family: Option<String>,
    /// `details.parameter_size` (e.g. `8B`).
    pub param_size: Option<String>,
    /// `details.quantization_level` (e.g. `Q4_K_M`).
    pub quant: Option<String>,
    /// `model_info["<arch>.context_length"]`.
    pub ctx_window: Option<i64>,
}

#[derive(Debug)]
pub enum OllamaError {
    Http(String),
    UnknownModel(String),
    Parse(String),
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OllamaError::Http(e) => write!(f, "ollama unreachable: {e}"),
            OllamaError::UnknownModel(m) => write!(f, "ollama has no model named {m}"),
            OllamaError::Parse(e) => write!(f, "unexpected ollama reply: {e}"),
        }
    }
}

impl std::error::Error for OllamaError {}

impl OllamaProbe {
    /// `base_url` is the daemon root, e.g. `http://127.0.0.1:11434`.
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        Self { base_url, client: reqwest::Client::new() }
    }

    /// One probe: process list + show. A model absent from both is [`OllamaError::UnknownModel`].
    pub async fn probe(&self, model: &str) -> Result<OllamaModelState, OllamaError> {
        let ps: serde_json::Value = self
            .client
            .get(format!("{}/api/ps", self.base_url))
            .send()
            .await
            .map_err(|e| OllamaError::Http(e.to_string()))?
            .json()
            .await
            .map_err(|e| OllamaError::Parse(e.to_string()))?;

        let mut state = OllamaModelState::default();
        if let Some(entries) = ps.get("models").and_then(|m| m.as_array()) {
            let hit = entries.iter().find(|e| {
                ["name", "model"].iter().any(|k| e.get(k).and_then(|v| v.as_str()).is_some_and(|n| names_match(n, model)))
            });
            if let Some(e) = hit {
                state.warm = true;
                state.vram_used_mb = e.get("size_vram").and_then(|v| v.as_i64()).map(|b| b / (1024 * 1024));
            }
        }

        let show = self
            .client
            .post(format!("{}/api/show", self.base_url))
            .json(&serde_json::json!({ "model": model }))
            .send()
            .await
            .map_err(|e| OllamaError::Http(e.to_string()))?;
        if show.status().as_u16() == 404 {
            return Err(OllamaError::UnknownModel(model.to_owned()));
        }
        if !show.status().is_success() {
            return Err(OllamaError::Http(format!("show: HTTP {}", show.status())));
        }
        let show: serde_json::Value = show.json().await.map_err(|e| OllamaError::Parse(e.to_string()))?;
        if let Some(d) = show.get("details") {
            state.family = d.get("family").and_then(|v| v.as_str()).map(str::to_owned);
            state.param_size = d.get("parameter_size").and_then(|v| v.as_str()).map(str::to_owned);
            state.quant = d.get("quantization_level").and_then(|v| v.as_str()).map(str::to_owned);
        }
        if let Some(info) = show.get("model_info").and_then(|v| v.as_object()) {
            state.ctx_window = info
                .iter()
                .find(|(k, _)| k.ends_with(".context_length"))
                .and_then(|(_, v)| v.as_i64());
        }
        Ok(state)
    }
}

/// `llama3` matches `llama3`, `llama3:latest`; `llama3:8b` matches only itself.
fn names_match(listed: &str, wanted: &str) -> bool {
    listed == wanted
        || (!wanted.contains(':') && listed.strip_suffix(":latest") == Some(wanted))
}

impl OllamaModelState {
    /// Write this state into `profile` as [`llm_meta`] attributes. Dynamic attributes
    /// (`engine`, `warm`, `vram_used_mb`) always overwrite; static ones (`family`,
    /// `ctx_window`, `param_size`, `quant`) fill in only what the profile left unset, so an
    /// operator's explicit value wins over the daemon's.
    pub fn apply(&self, profile: &mut ModelProfile) {
        profile.set(llm_meta::ENGINE, CapValue::Text(Arc::from("ollama")));
        profile.set(llm_meta::WARM, CapValue::Bool(self.warm));
        profile.extra.retain(|(k, _)| k != llm_meta::VRAM_USED_MB);
        if let Some(mb) = self.vram_used_mb {
            profile.set(llm_meta::VRAM_USED_MB, CapValue::Integer(mb));
        }
        if profile.family.is_none() {
            profile.family = self.family.clone();
        }
        if profile.ctx_window.is_none() {
            profile.ctx_window = self.ctx_window;
        }
        let has = |p: &ModelProfile, k: &str| p.extra.iter().any(|(kk, _)| kk == k);
        if let Some(v) = &self.param_size
            && !has(profile, llm_meta::PARAM_SIZE)
        {
            profile.set(llm_meta::PARAM_SIZE, CapValue::Text(Arc::from(v.as_str())));
        }
        if let Some(v) = &self.quant
            && !has(profile, llm_meta::QUANT)
        {
            profile.set(llm_meta::QUANT, CapValue::Text(Arc::from(v.as_str())));
        }
    }
}

/// Owns a served model's registration and keeps its `llm-meta` ad current from the
/// daemon. Dropping it stops the loop and retracts the model (the [`ModelReg`] inside).
pub struct MetaRefresher {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MetaRefresher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Probe every `interval` and re-advertise `reg`'s `llm-meta` ad when the daemon's view of
/// `base_profile.model` changed (a no-op probe re-advertises nothing). A failed probe is
/// logged and the last-good ad stands — the daemon being briefly unreachable is not a
/// reason to withdraw the model; if it is down for real, the prompt skill's own calls fail
/// and the router fails over. Returns the handle that owns the registration.
pub fn spawn_meta_refresher(
    agent: Arc<GossipAgent>,
    mut reg: ModelReg,
    base_profile: ModelProfile,
    probe: OllamaProbe,
    interval: Duration,
) -> MetaRefresher {
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            match probe.probe(&base_profile.model).await {
                Ok(state) => {
                    let mut profile = base_profile.clone();
                    state.apply(&mut profile);
                    if reg.refresh_meta(&agent, &profile).await {
                        tracing::info!(model = %profile.model, warm = state.warm, "llm-meta ad refreshed from ollama");
                    }
                }
                Err(e) => tracing::warn!(model = %base_profile.model, error = %e, "ollama probe failed; last-good llm-meta ad stands"),
            }
        }
    });
    MetaRefresher { task }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_matching_follows_ollama_naming() {
        assert!(names_match("llama3:latest", "llama3"));
        assert!(names_match("llama3", "llama3"));
        assert!(names_match("llama3:8b", "llama3:8b"));
        assert!(!names_match("llama3:8b", "llama3"));
        assert!(!names_match("llama3:latest", "llama3:8b"));
    }

    #[test]
    fn apply_overwrites_dynamic_and_fills_static() {
        let mut p = ModelProfile::new("m").with(llm_meta::FAMILY, CapValue::Text(Arc::from("operator-said")));
        let warm = OllamaModelState {
            warm: true,
            vram_used_mb: Some(4200),
            family: Some("llama".into()),
            param_size: Some("8B".into()),
            quant: Some("Q4_K_M".into()),
            ctx_window: Some(8192),
        };
        warm.apply(&mut p);
        assert_eq!(p.family.as_deref(), Some("operator-said"), "explicit static value wins");
        assert_eq!(p.ctx_window, Some(8192));
        let get = |p: &ModelProfile, k: &str| p.extra.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get(&p, llm_meta::WARM), Some(CapValue::Bool(true)));
        assert_eq!(get(&p, llm_meta::VRAM_USED_MB), Some(CapValue::Integer(4200)));
        assert_eq!(get(&p, llm_meta::ENGINE), Some(CapValue::Text(Arc::from("ollama"))));
        // Going cold: warm flips, vram is withdrawn, statics stay.
        OllamaModelState { warm: false, vram_used_mb: None, ..warm }.apply(&mut p);
        assert_eq!(get(&p, llm_meta::WARM), Some(CapValue::Bool(false)));
        assert_eq!(get(&p, llm_meta::VRAM_USED_MB), None);
        assert_eq!(get(&p, llm_meta::PARAM_SIZE), Some(CapValue::Text(Arc::from("8B"))));
    }
}
