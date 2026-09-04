//! Wedge ① — capability-routed inference: a load-aware routing policy over the mesh.
//!
//! Capability **resolution is load-blind** (`resolve` ranks by freshness/attributes/
//! locality only — an overloaded node's entry ages out, nothing more), so this module is
//! the routing layer the substrate deliberately does not bake in: *resolve → drop opaque
//! nodes → rank by pheromone fill + this node's in-flight reservations → fail over down
//! the candidate list.*
//!
//! **Reservations (2026-09-04).** The pheromone is the provider's *self-report*, and it
//! reaches this node only after the provider writes it and gossip carries it. Between our
//! dispatch and that update, the only evidence that a provider is busier than its trail
//! says is *what we ourselves sent it* — so each in-flight call this router has open
//! against a provider counts as a local reservation, weighted into the rank. Without it,
//! N concurrent callers on one node all read the same stale fill and, via the
//! deterministic id tiebreak, all pick the same provider (the thundering herd NVIDIA's
//! PAIR router documents its local reservations against). Node-local knowledge composed
//! with the shared medium — never a global schedule: the reservation is neither gossiped
//! nor written as a pheromone; the provider's own trail stays the fleet-wide signal.
//!
//! Convention (bound in `docs/plans/mycelium-reason.md`, 2026-07-08 addendum): **a model
//! is a prompt skill** — capability `llm/{model-id}` via `register_prompt_skill`
//! (matching the `model_deploy` precedent) — plus a parallel **attributed metadata ad**
//! `llm-meta/{model-id}` (ctx window, family, extras). The second ad exists because
//! re-advertising the same `(node, ns, name)` key with attributes would LWW-churn
//! against the skill's own persist task.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use mycelium::signal::signal_kind;
use mycelium::{CapConstraint, CapFilter, GossipAgent, NodeId};
#[cfg(feature = "llm")]
use mycelium::Capability;

use crate::trace::TraceRecorder;

// ── The `llm-meta/{model}` attribute vocabulary (core-only) ───────────────────

/// Attribute names for the `llm-meta/{model}` ad — the **vocabulary** a serving node
/// speaks and a [`ModelQuery::constraints`] caller matches against. The ad is a plain
/// attributed capability, so any key is *expressible*; this module fixes the names and
/// types that mean the same thing fleet-wide, so a constraint written on one node matches
/// an ad written on another.
///
/// | attribute | `CapValue` | source | meaning |
/// |---|---|---|---|
/// | [`CTX_WINDOW`] | `Integer` | engine (`/api/show`) or profile | context window, tokens |
/// | [`FAMILY`] | `Text` | engine or profile | model family (`llama`, `qwen2`, …) |
/// | [`ENGINE`] | `Text` | serve side | `ollama` · `lmstudio` · `openai` · `echo` · … |
/// | [`WARM`] | `Bool` | engine process list (`/api/ps`) | weights resident in memory now — a cold model pays a load before its first token |
/// | [`VRAM_USED_MB`] | `Integer` | engine process list | MiB of accelerator memory this model holds while warm |
/// | [`VRAM_FREE_MB`] | `Integer` | embedder-measured (`nvidia-smi`, Metal) | MiB free on the serving device — engines do not report it |
/// | [`TOKENS_PER_SEC`] | `Float` | embedder-measured | recent decode throughput on this device |
/// | [`PARAM_SIZE`] | `Text` | engine (`/api/show`) | parameter count as the engine prints it (`8B`) |
/// | [`QUANT`] | `Text` | engine (`/api/show`) | quantization level (`Q4_K_M`) |
///
/// Static attributes ride the profile once; dynamic ones (`warm`, `vram_used_mb`,
/// `tokens_per_sec`) are refreshed via [`ModelReg::refresh_meta`] — the Ollama collector
/// (`feature = "ollama"`) drives that loop. A constraint on an attribute a provider does
/// not advertise excludes that provider (the substrate's `CapFilter` rule) — so callers
/// constrain only on what their fleet actually publishes.
pub mod llm_meta {
    /// Context window in tokens (`Integer`).
    pub const CTX_WINDOW: &str = "ctx_window";
    /// Model family (`Text`).
    pub const FAMILY: &str = "family";
    /// Serving engine (`Text`): `ollama`, `lmstudio`, `openai`, `echo`, ….
    pub const ENGINE: &str = "engine";
    /// Weights resident now (`Bool`).
    pub const WARM: &str = "warm";
    /// Accelerator memory this model holds while warm, MiB (`Integer`).
    pub const VRAM_USED_MB: &str = "vram_used_mb";
    /// Free accelerator memory on the serving device, MiB (`Integer`, embedder-measured).
    pub const VRAM_FREE_MB: &str = "vram_free_mb";
    /// Recent decode throughput (`Float`, embedder-measured).
    pub const TOKENS_PER_SEC: &str = "tokens_per_sec";
    /// Parameter count as the engine prints it (`Text`).
    pub const PARAM_SIZE: &str = "param_size";
    /// Quantization level (`Text`).
    pub const QUANT: &str = "quant";
}

// ── Serve side (feature `llm`) ────────────────────────────────────────────────

/// What a serving node says about its model — the payload of the `llm-meta/{model}` ad.
/// Attribute names: [`llm_meta`].
#[cfg(feature = "llm")]
#[derive(Clone, Debug)]
pub struct ModelProfile {
    /// Model id — becomes the capability name in both `llm/{model}` and `llm-meta/{model}`.
    pub model: String,
    /// Context window in tokens (advertised as an `Integer` attribute `ctx_window`).
    pub ctx_window: Option<i64>,
    /// Model family (advertised as a `Text` attribute `family`).
    pub family: Option<String>,
    /// Additional typed attributes, advertised as-is.
    pub extra: Vec<(String, mycelium::CapValue)>,
}

#[cfg(feature = "llm")]
impl ModelProfile {
    /// A profile with only the model id — add attributes with [`with`](Self::with).
    pub fn new(model: impl Into<String>) -> Self {
        Self { model: model.into(), ctx_window: None, family: None, extra: Vec::new() }
    }

    /// Builder-style attribute (see [`llm_meta`] for the shared vocabulary). A repeated
    /// key replaces the earlier value; [`llm_meta::CTX_WINDOW`] / [`llm_meta::FAMILY`]
    /// land in the typed fields.
    pub fn with(mut self, attr: &str, value: mycelium::CapValue) -> Self {
        self.set(attr, value);
        self
    }

    /// In-place form of [`with`](Self::with).
    pub fn set(&mut self, attr: &str, value: mycelium::CapValue) {
        match (attr, &value) {
            (llm_meta::CTX_WINDOW, mycelium::CapValue::Integer(n)) => self.ctx_window = Some(*n),
            (llm_meta::FAMILY, mycelium::CapValue::Text(t)) => self.family = Some(t.to_string()),
            _ => {
                self.extra.retain(|(k, _)| k != attr);
                self.extra.push((attr.to_owned(), value));
            }
        }
    }

    /// The `llm-meta/{model}` capability this profile advertises.
    pub fn meta_capability(&self) -> Capability {
        let mut meta = Capability::new("llm-meta", self.model.as_str());
        if let Some(ctx) = self.ctx_window {
            meta = meta.with(llm_meta::CTX_WINDOW, mycelium::CapValue::Integer(ctx));
        }
        if let Some(family) = &self.family {
            meta = meta.with(llm_meta::FAMILY, mycelium::CapValue::Text(Arc::from(family.as_str())));
        }
        for (k, v) in &self.extra {
            meta = meta.with(k.as_str(), v.clone());
        }
        meta
    }
}

/// RAII registration for a served model: the prompt skill + the metadata ad.
/// Dropping retracts both (skill dispatch entry, `llm/…` cap, `llm-meta/…` cap).
#[cfg(feature = "llm")]
pub struct ModelReg {
    model: String,
    _skill: mycelium::PromptSkillHandle,
    meta: Option<mycelium::CapabilityReg>,
    advertised: Capability,
}

#[cfg(feature = "llm")]
impl ModelReg {
    /// The `llm-meta` capability currently advertised.
    pub fn advertised_meta(&self) -> &Capability {
        &self.advertised
    }

    /// Re-advertise the `llm-meta/{model}` ad with `profile`'s attributes (the dynamic
    /// ones — [`llm_meta::WARM`], [`llm_meta::VRAM_USED_MB`], … — change over a model's
    /// life). The skill registration is untouched. A no-op when nothing changed.
    ///
    /// **Why this is sequenced rather than a bare drop-and-advertise:** retracting an ad
    /// tombstones its KV key from the old persist task, and a fresh ad on the same key
    /// writes on the new task's first tick — two tasks on one node's HLC, where the later
    /// write wins LWW. If the tombstone ever landed second the ad would vanish until the
    /// new task's next 30 s tick. In practice it does not: measured over 60 warm/cold
    /// flips (2026-09-04), the bare form never blinked — tokio's timer driver runs the
    /// already-woken old task before the new interval's first tick. That ordering is a
    /// runtime detail, not a contract, so this method makes it explicit: the retraction
    /// is *observed* (a bounded structural poll on `resolve`) before the new ad is
    /// advertised. Cost: one scheduler hop. Returns `true` when the ad was replaced.
    pub async fn refresh_meta(&mut self, agent: &Arc<GossipAgent>, profile: &ModelProfile) -> bool {
        let next = profile.meta_capability();
        if next == self.advertised {
            return false;
        }
        let caps = agent.capabilities();
        drop(self.meta.take());
        let filter = CapFilter::new("llm-meta", self.model.as_str());
        let me = agent.node_id().clone();
        // Bounded structural wait for the tombstone (typically one scheduler hop).
        for _ in 0..200 {
            if !caps.resolve(&filter).iter().any(|(n, _)| *n == me) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        self.meta = Some(caps.advertise_capability(next.clone(), META_INTERVAL));
        self.advertised = next;
        true
    }
}

/// Re-advertise interval for the `llm-meta` ad (freshness window is 3× this).
#[cfg(feature = "llm")]
const META_INTERVAL: Duration = Duration::from_secs(30);

/// Serve `profile.model` on this node: register the prompt skill (capability
/// `llm/{model}`, template in KV, `llm.invoke` dispatch) and advertise the parallel
/// attributed `llm-meta/{model}` ad that [`ModelQuery::constraints`] are tested against.
#[cfg(feature = "llm")]
pub async fn serve_model(
    agent: &Arc<GossipAgent>,
    profile: ModelProfile,
    template: mycelium::PromptTemplate,
    backend: Arc<dyn mycelium::LlmBackend>,
) -> Result<ModelReg, mycelium::PromptSkillError> {
    let skill = agent.llm().register_prompt_skill("llm", &profile.model, template, backend).await?;

    let meta = profile.meta_capability();
    let meta_reg = agent.capabilities().advertise_capability(meta.clone(), META_INTERVAL);

    Ok(ModelReg { model: profile.model, _skill: skill, meta: Some(meta_reg), advertised: meta })
}

// ── Call side (core-only — no feature gate) ───────────────────────────────────

/// Routing policy knobs.
#[derive(Clone, Debug)]
pub struct RouterConfig {
    /// How many candidates to try before giving up (failover depth).
    pub max_attempts: usize,
    /// RPC timeout for the **final** attempt (or a lone candidate): the full inference
    /// budget, since there is no one left to fail over to.
    pub call_timeout: Duration,
    /// RPC timeout for any **non-final** attempt, i.e. while failover candidates remain.
    /// Deliberately shorter than `call_timeout`: a mesh RPC to a dead peer has no fast
    /// connection-refused, so without this a candidate that died inside the SWIM detection
    /// window (still in `peers()` for a beat) would burn the full inference budget before
    /// failing over. Failing *over* fast is the right call when an alternative exists;
    /// the last candidate still gets `call_timeout` so a genuinely slow lone provider is
    /// not cut off.
    pub failover_timeout: Duration,
    /// Freshness window for opacity + pheromone load reads.
    pub load_max_age: Duration,
    /// How much rank score one of *this router's* in-flight calls adds to a provider —
    /// the local reservation (module doc). Fill is 0.0–1.0, so the default `0.1` reads
    /// as "ten of our own open calls weigh like a fully loaded trail". `0.0` disables
    /// reservations (pure pheromone + id order).
    pub reservation_weight: f32,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            call_timeout: Duration::from_secs(30),
            failover_timeout: Duration::from_secs(8),
            load_max_age: Duration::from_secs(10),
            reservation_weight: 0.1,
        }
    }
}

/// What to route: a model id plus optional constraints over the `llm-meta/{model}` ad
/// (e.g. `("ctx_window", CapConstraint::Gte(CapValue::Integer(32_768)))`). Empty
/// constraints skip the metadata lookup entirely.
#[derive(Clone, Debug)]
pub struct ModelQuery {
    pub model: String,
    pub constraints: Vec<(String, CapConstraint)>,
}

impl ModelQuery {
    pub fn new(model: impl Into<String>) -> Self {
        Self { model: model.into(), constraints: Vec::new() }
    }
}

/// A successfully routed inference.
#[derive(Debug, Clone)]
pub struct Routed {
    pub output: String,
    pub model_used: String,
    pub tokens_used: u32,
    /// The provider that answered.
    pub provider: NodeId,
    /// 1-based attempt index (1 = first candidate answered).
    pub attempt: usize,
}

/// Why routing failed.
#[derive(Debug)]
pub enum RouteError {
    /// No live provider advertises `llm/{model}` (after constraint + opacity filtering).
    NoProvider,
    /// Every attempted candidate failed; per-node error strings in attempt order.
    Exhausted(Vec<(NodeId, String)>),
}

impl fmt::Display for RouteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RouteError::NoProvider => write!(f, "no provider for the requested model"),
            RouteError::Exhausted(fails) => {
                write!(f, "all {} attempted provider(s) failed: ", fails.len())?;
                let mut first = true;
                for (node, err) in fails {
                    if !first {
                        write!(f, "; ")?;
                    }
                    write!(f, "{node}: {err}")?;
                    first = false;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// Load-aware, failover-capable router over `llm/{model}` providers. Core-only: a node
/// needs no `llm` feature (and no local backend) to *call* models served elsewhere.
pub struct InferenceRouter {
    agent: Arc<GossipAgent>,
    cfg: RouterConfig,
    /// Local reservations: calls this router currently has open, per provider (node id
    /// string). Leaf lock — taken for a map read/write only, never across an await
    /// (lock-order row 36). Node-local by design: see the module doc.
    inflight: Mutex<HashMap<String, u32>>,
}

/// RAII reservation on one provider for the duration of one attempt.
struct Reservation<'r> {
    router: &'r InferenceRouter,
    node: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut map = self.router.inflight.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = map.get_mut(&self.node) {
            *n -= 1;
            if *n == 0 {
                map.remove(&self.node);
            }
        }
        metrics::gauge!("mycelium_reason_route_inflight").decrement(1.0);
    }
}

impl InferenceRouter {
    pub fn new(agent: Arc<GossipAgent>, cfg: RouterConfig) -> Self {
        Self { agent, cfg, inflight: Mutex::new(HashMap::new()) }
    }

    /// Calls this router currently has open against `node` (the local reservation count).
    pub fn inflight(&self, node: &NodeId) -> u32 {
        let map = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&node.to_string()).copied().unwrap_or(0)
    }

    fn reserve(&self, node: &NodeId) -> Reservation<'_> {
        let key = node.to_string();
        {
            let mut map = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            *map.entry(key.clone()).or_insert(0) += 1;
        }
        metrics::gauge!("mycelium_reason_route_inflight").increment(1.0);
        Reservation { router: self, node: key }
    }

    /// The ranked candidate list for `q`: resolve `llm/{model}`, intersect with the
    /// `llm-meta` ad when constraints are given, **drop nodes SWIM believes are dead**,
    /// drop opaque nodes, then sort by (score, node id) — the id tiebreak makes the order
    /// deterministic. The score is the pheromone fill (max `fill_ratio` across the node's
    /// fresh load entries; no trail is 0.0, transparent) **plus this router's open calls
    /// against the node × [`RouterConfig::reservation_weight`]** — the local reservation
    /// that covers the trail's propagation lag (module doc). The returned `f32` is that
    /// score.
    ///
    /// The liveness filter is load-bearing for failover. A killed node lingers in the
    /// capability *freshness* window for ~90 s (3× the 30 s re-advertise interval) — it
    /// stops refreshing but there is no instant tombstone, so `resolve` keeps returning
    /// it. Routing to it is expensive: a mesh RPC to a dead peer has no fast connection-
    /// refused, so it blocks the whole per-attempt timeout. `peers()` is the SWIM
    /// live-membership view, from which a failed node departs within its detection window
    /// (~`swim_probe_interval + swim_suspicion_timeout`, a few seconds by default) — an
    /// order of magnitude faster than freshness. So the router routes only to nodes SWIM
    /// currently believes are alive (plus **self**, always live and never in `peers()`).
    /// A brief window remains between a node's death and SWIM detecting it, bounded by one
    /// per-attempt timeout; that is inherent to the failure detector, not the router.
    pub fn candidates(&self, q: &ModelQuery) -> Vec<(NodeId, f32)> {
        let caps = self.agent.capabilities();
        let mut nodes: Vec<NodeId> =
            caps.resolve(&CapFilter::new("llm", q.model.as_str())).into_iter().map(|(n, _)| n).collect();

        if !q.constraints.is_empty() {
            let mut meta_filter = CapFilter::new("llm-meta", q.model.as_str());
            for (attr, c) in &q.constraints {
                meta_filter = meta_filter.with(attr.as_str(), c.clone());
            }
            let meta_nodes: Vec<NodeId> =
                caps.resolve(&meta_filter).into_iter().map(|(n, _)| n).collect();
            nodes.retain(|n| meta_nodes.contains(n));
        }

        // Liveness: keep self (always live, never listed in its own peer set) + nodes SWIM
        // currently believes are alive. Drops a killed peer an order of magnitude sooner
        // than the capability freshness window would.
        let self_id = self.agent.node_id();
        let live: HashSet<NodeId> = self.agent.peers().into_iter().collect();
        nodes.retain(|n| n == self_id || live.contains(n));

        nodes.retain(|n| !caps.is_node_opaque(n, signal_kind::LLM_INVOKE, self.cfg.load_max_age));

        // Pheromone fill per node (max fill_ratio over its fresh load entries) plus the
        // local reservation — a snapshot of the in-flight map, taken once, lock released
        // before ranking.
        let load = caps.peer_load(self.cfg.load_max_age);
        let inflight: HashMap<String, u32> =
            self.inflight.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let mut ranked: Vec<(NodeId, f32)> = nodes
            .into_iter()
            .map(|n| {
                let ns = n.to_string();
                let fill = load
                    .iter()
                    .filter(|(node, _, _)| node.as_ref() == ns)
                    .map(|(_, _, s)| s.fill_ratio)
                    .fold(0.0_f32, f32::max);
                let reserved = inflight.get(&ns).copied().unwrap_or(0) as f32;
                (n, fill + reserved * self.cfg.reservation_weight)
            })
            .collect();
        rank(&mut ranked);
        ranked
    }

    /// Route one inference: walk [`candidates`](Self::candidates) up to
    /// `max_attempts`, one RPC per candidate, failing over on error replies and RPC
    /// timeouts. When `trace` is given, the route decision is recorded once and each
    /// attempt as an `llm_call` event.
    pub async fn call(
        &self,
        q: &ModelQuery,
        input: &str,
        context: &HashMap<String, String>,
        trace: Option<&TraceRecorder>,
    ) -> Result<Routed, RouteError> {
        let candidates = self.candidates(q);
        let Some((chosen, _)) = candidates.first() else {
            metrics::counter!("mycelium_reason_route_no_provider_total").increment(1);
            return Err(RouteError::NoProvider);
        };
        if let Some(t) = trace {
            t.route(&q.model, &candidates, chosen);
        }

        // Same JSON the core's `llm.invoke` dispatch parses and `gw_llm_call` speaks
        // over the gateway (the structs are pub(crate) in core; the shape is wire-public).
        let request = serde_json::json!({
            "prompt": format!("llm/{}", q.model),
            "input": input,
            "context": context,
        });
        let payload = Bytes::from(request.to_string().into_bytes());

        // How many we will actually try — the last of these gets the full `call_timeout`,
        // earlier ones the shorter `failover_timeout` (fail over fast, don't burn the
        // inference budget on a candidate that may have just died).
        let to_try = candidates.len().min(self.cfg.max_attempts);
        let mut failures: Vec<(NodeId, String)> = Vec::new();
        for (attempt, (node, _fill)) in candidates.iter().take(self.cfg.max_attempts).enumerate() {
            metrics::counter!("mycelium_reason_route_attempts_total").increment(1);
            let per_attempt_timeout = if attempt + 1 == to_try {
                self.cfg.call_timeout
            } else {
                self.cfg.failover_timeout
            };
            let started = std::time::Instant::now();
            let reply = {
                // Reserved for exactly the attempt: the guard drops when the reply (or the
                // timeout) comes back, whether or not we fail over.
                let _reservation = self.reserve(node);
                self.agent
                    .service()
                    .rpc_call(node.clone(), signal_kind::LLM_INVOKE, payload.clone(), per_attempt_timeout)
                    .await
            };
            let duration_ms = started.elapsed().as_millis() as u64;

            let err = match reply {
                Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(v) => {
                        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
                            let detail = v.get("detail").and_then(|d| d.as_str()).unwrap_or("");
                            format!("{e}: {detail}")
                        } else {
                            let output = v["output"].as_str().unwrap_or_default().to_owned();
                            let model_used = v["model_used"].as_str().unwrap_or_default().to_owned();
                            let tokens_used = v["tokens_used"].as_u64().unwrap_or(0) as u32;
                            if let Some(t) = trace {
                                t.llm_call(node, true, tokens_used, duration_ms, None);
                            }
                            return Ok(Routed {
                                output,
                                model_used,
                                tokens_used,
                                provider: node.clone(),
                                attempt: attempt + 1,
                            });
                        }
                    }
                    Err(e) => format!("undecodable reply: {e}"),
                },
                Err(e) => e.to_string(),
            };
            if let Some(t) = trace {
                t.llm_call(node, false, 0, duration_ms, Some(&err));
            }
            failures.push((node.clone(), err));
            // Failover: this attempt failed and at least one candidate remains to try.
            if attempt + 1 < to_try {
                metrics::counter!("mycelium_reason_route_failovers_total").increment(1);
            }
        }
        metrics::counter!("mycelium_reason_route_exhausted_total").increment(1);
        Err(RouteError::Exhausted(failures))
    }
}

/// The candidate order: score ascending, then node-id string (deterministic ties).
fn rank(ranked: &mut [(NodeId, f32)]) {
    ranked.sort_by(|(na, fa), (nb, fb)| {
        fa.total_cmp(fb).then_with(|| na.to_string().cmp(&nb.to_string()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The candidate ordering contract, tested as a pure sort (the same comparator
    /// `candidates()` applies): score ascending, then node-id string for determinism.
    #[test]
    fn candidate_ordering_is_deterministic() {
        let n = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
        let mut ranked = vec![
            (n(9003), 0.5_f32),
            (n(9002), 0.0),
            (n(9001), 0.5),
            (n(9000), 0.0),
        ];
        rank(&mut ranked);
        let order: Vec<String> = ranked.iter().map(|(n, _)| n.to_string()).collect();
        assert_eq!(
            order,
            vec!["127.0.0.1:9000", "127.0.0.1:9002", "127.0.0.1:9001", "127.0.0.1:9003"],
        );
        // Re-sorting an already-sorted list is a fixpoint (stability under repetition).
        let again = {
            let mut r = ranked.clone();
            rank(&mut r);
            r
        };
        assert_eq!(
            again.iter().map(|(n, _)| n.to_string()).collect::<Vec<_>>(),
            order,
        );
    }

    #[test]
    fn route_error_display() {
        let n = |p: u16| NodeId::new("127.0.0.1", p).unwrap();
        assert_eq!(RouteError::NoProvider.to_string(), "no provider for the requested model");
        let e = RouteError::Exhausted(vec![
            (n(9000), "timeout".into()),
            (n(9001), "llm_error: boom".into()),
        ]);
        let s = e.to_string();
        assert!(s.contains("all 2 attempted provider(s) failed"));
        assert!(s.contains("127.0.0.1:9000: timeout"));
        assert!(s.contains("127.0.0.1:9001: llm_error: boom"));
    }

    /// The reservation guard: `reserve` counts up, dropping counts down (and clears the
    /// entry at zero), and a reserved provider's score rises by `reservation_weight`
    /// per open call — enough to lose an equal-fill tie it would otherwise win on id.
    #[tokio::test]
    async fn reservations_count_and_release() {
        let node = NodeId::new("127.0.0.1", 9100).unwrap();
        let agent = Arc::new(GossipAgent::new(node.clone(), mycelium::GossipConfig::default()));
        let router = InferenceRouter::new(agent, RouterConfig::default());
        let a = NodeId::new("127.0.0.1", 9000).unwrap();
        let b = NodeId::new("127.0.0.1", 9001).unwrap();
        assert_eq!(router.inflight(&a), 0);
        {
            let _r1 = router.reserve(&a);
            let _r2 = router.reserve(&a);
            assert_eq!(router.inflight(&a), 2);
            // Scoring as candidates() does it: fill 0 for both, a carries two reservations.
            let w = router.cfg.reservation_weight;
            let mut ranked = vec![(a.clone(), 0.0 + 2.0 * w), (b.clone(), 0.0)];
            rank(&mut ranked);
            assert_eq!(ranked[0].0, b, "the reserved provider loses the tie it wins on id alone");
        }
        assert_eq!(router.inflight(&a), 0, "guards released, entry cleared");
        assert!(router.inflight.lock().unwrap().is_empty());
    }
}
