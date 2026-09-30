//! Artifact runtimes — the kind-dispatch axis of install (`docs/design/artifact-library.md` §4).
//!
//! "Install" is not one operation. A WASM component is pulled → verified → **instantiated**
//! (`WasmHost`) and served over RPC; a model blob is pulled → verified → **placed** for a
//! node-local runtime to consume, its capability probe-gated on that runtime answering. The
//! [`ArtifactRuntime`] trait owns the *how* per [`ArtifactKind`]; the `Provisioner` owns the
//! *when* (the demand/presence/shed convergence loops, which are kind-agnostic) and dispatches
//! through its runtime registry. A node registers runtimes only for the kinds it can host —
//! **eligibility is node-local truth** (no GPU → no model runtime → this node never self-elects
//! for model artifacts; some other node does).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use mycelium::GossipAgent;

use crate::artifact::{ArtifactId, ArtifactKind, ArtifactSource};
use crate::catalog::InstallableEntry;
use crate::host::{HostState, Instance, WasmHost, WasmHostError};

/// D19 — who pays for fuel. An entry's publisher is its verified `signer` (inside the signature,
/// and checked against the provisioner's trusted keys before install): a signer listed here is
/// the operator and its entries run under `operator_budget` (`None` = unbounded); every other
/// signer is an agent principal and its entries run under `agent_budget`. The classification rests
/// on provenance, so it is meaningful only where provenance is required — the stem refuses an
/// operator list without a trusted list, by name.
#[derive(Clone, Debug, Default)]
pub struct FuelPolicy {
    pub operator_publishers: Vec<[u8; 32]>,
    pub agent_budget:        Option<u64>,
    pub operator_budget:     Option<u64>,
}

impl FuelPolicy {
    /// The budget an entry signed by `signer` runs under.
    pub fn budget_for(&self, signer: &[u8]) -> Option<u64> {
        if self.operator_publishers.iter().any(|k| k.as_slice() == signer) {
            self.operator_budget
        } else {
            self.agent_budget
        }
    }
}

/// How one invocation of a hosted component ended — the runtime's execution record, kept on the
/// node so an operator can see *fuel exhausted* by name rather than infer it from a reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InvocationOutcome {
    Completed,
    /// The component returned its own error string.
    ComponentError(String),
    /// The host trapped or failed at the ABI (not fuel).
    HostError(String),
    /// Stopped at its fuel budget (D19).
    FuelExhausted { budget: u64 },
}

/// One entry of the runtime's invocation log: which artifact, serving which capability, published
/// by whom, ended how.
#[derive(Clone, Debug)]
pub struct InvocationRecord {
    pub artifact: ArtifactId,
    pub provides: String,
    pub signer:   Vec<u8>,
    pub outcome:  InvocationOutcome,
}

/// The newest [`INVOCATION_LOG_CAP`] records, oldest dropped first. Wiki lock-order table row 50:
/// a leaf, one lock per function, never across `await`.
pub type InvocationLog = Arc<Mutex<VecDeque<InvocationRecord>>>;

/// How many invocation records the runtime keeps.
pub const INVOCATION_LOG_CAP: usize = 1024;

fn record_invocation(log: &InvocationLog, rec: InvocationRecord) {
    let mut g = log.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() >= INVOCATION_LOG_CAP {
        g.pop_front();
    }
    g.push_back(rec);
}

/// The RPC `kind` an inbound caller uses to invoke the hosted capability `ns/name`. A caller
/// resolves the capability to a provider node, then `rpc_call(provider, cap_invoke_kind(ns, name),
/// payload, timeout)`; the hosting runtime's serve loop routes it to the component's `handle`
/// export.
pub fn cap_invoke_kind(namespace: &str, name: &str) -> String {
    format!("cap.invoke/{namespace}/{name}")
}

/// Install-progress callback: `(bytes_fetched, bytes_total)`. `bytes_total = 0` means unknown.
/// For large blob pulls this drives the *real* loading-tier advertisement (the percent the
/// `llm_agent` example used to simulate); small WASM installs report a single completion tick.
pub type ProgressFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// What a runtime needs from the node to bring an artifact live.
#[derive(Clone)]
pub struct RuntimeCtx {
    pub agent: Arc<GossipAgent>,
}

/// Why an install failed — **typed by stage** so callers can match on cause (retry a
/// [`Fetch`](Self::Fetch), refuse a [`Verify`](Self::Verify), alert on
/// [`Resources`](Self::Resources)) instead of parsing strings. The provisioner logs it, drops
/// the reservation, and lets a later round retry (restart ≡ provisioning).
#[derive(Debug)]
pub enum InstallError {
    /// No holder produced the bytes (source miss, ranged read failed, remote unreachable).
    /// Transient by nature — the canonical retry case.
    Fetch(String),
    /// Bytes failed the content address — an untrusted holder lied or corrupted. Retrying a
    /// *different* holder may succeed; the same bytes never will.
    Verify(String),
    /// Local filesystem placement failed (temp write, rename, placement dir).
    Place(String),
    /// The runtime's activation hook refused — e.g. a prerequisite artifact not yet placed
    /// (retried by design), or the node-local runtime rejected the artifact.
    Activation(String),
    /// The declared resource requirements provably cannot fit on this node (fail-fast check).
    Resources(String),
    /// Engine/task-level failure (wasmtime instantiate, a panicked pull task).
    Host(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (stage, msg) = match self {
            Self::Fetch(m) => ("fetch", m),
            Self::Verify(m) => ("verify", m),
            Self::Place(m) => ("place", m),
            Self::Activation(m) => ("activation", m),
            Self::Resources(m) => ("resources", m),
            Self::Host(m) => ("host", m),
        };
        write!(f, "install failed ({stage}): {msg}")
    }
}

impl std::error::Error for InstallError {}

impl InstallError {
    /// Stable stage label (metrics/log dimension).
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Fetch(_) => "fetch",
            Self::Verify(_) => "verify",
            Self::Place(_) => "place",
            Self::Activation(_) => "activation",
            Self::Resources(_) => "resources",
            Self::Host(_) => "host",
        }
    }
}

impl From<WasmHostError> for InstallError {
    fn from(e: WasmHostError) -> Self {
        match e {
            WasmHostError::Fetch(m) => Self::Fetch(m),
            WasmHostError::Verify(m) => Self::Verify(m),
            other => Self::Host(other.to_string()),
        }
    }
}

/// A live installation — the node-level lifecycle handle a runtime returns from
/// [`ArtifactRuntime::install`].
pub trait Installed: Send {
    /// Is the capability actually servable right now? For a WASM component: the serve task
    /// lives. For a placed model: the local runtime answers (the probe-gating hook). The
    /// provisioner consults this every `provision_round` and **withdraws** a failing install
    /// (the same round reinstalls if demand/supervision still wants it — restart ≡
    /// provisioning). Called under the provisioner's hosted lock: keep it **cheap and
    /// non-blocking** (a file-exists check, a cached health bit — never a network call;
    /// push slow health checks into a background task that flips a flag this reads).
    fn probe(&self) -> bool;

    /// Cooperative teardown: stop serving and clean up (abort tasks, delete placed bytes).
    fn uninstall(self: Box<Self>);
}

/// How one [`ArtifactKind`] is installed and torn down on this node.
#[async_trait::async_trait]
pub trait ArtifactRuntime: Send + Sync {
    /// The artifact kind this runtime installs.
    fn kind(&self) -> ArtifactKind;

    /// Where installs of this kind land on disk, if anywhere — the path the provisioner's
    /// resource-eligibility check measures free disk at (`docs/design/artifact-library.md`
    /// §4.4). `None` (the default) means installs are not disk-resident (a WASM component
    /// lives in memory) and only the memory requirement is checked.
    fn resource_root(&self) -> Option<&std::path::Path> {
        None
    }

    /// Pull (from `source`), verify, and bring the artifact live. May be long-running (a
    /// multi-GB blob) — the provisioner runs it as a background task against an `Installing`
    /// reservation, never blocking the provision tick. The runtime must have a live inbound
    /// path (serve handler, probing runtime) *before* returning: the provisioner advertises
    /// the declared-provide on success, and a resolvable capability must always have a
    /// receiver behind it.
    async fn install(
        &self,
        entry: InstallableEntry,
        source: Arc<dyn ArtifactSource + Send + Sync>,
        ctx: RuntimeCtx,
        progress: ProgressFn,
    ) -> Result<Box<dyn Installed>, InstallError>;
}

/// Serve loop for one hosted WASM capability: owns the component [`Instance`] (wasmtime stores
/// are single-threaded, so one task per instance serialises calls) and answers each inbound RPC
/// by invoking the component's `handle` export and replying with its output.
/// What the serve loop needs to replace a trapped instance: a trap poisons a component instance
/// (wasmtime refuses to re-enter it), so a call stopped at its budget would otherwise leave a
/// dead install behind an advertised capability. The loop instantiates afresh from the verified
/// bytes; the guest's in-memory state is lost with the instance (its KV subtree is not — that
/// lives in the node's store), and the record names the stop.
struct Reinstantiate {
    host:   Arc<WasmHost>,
    bytes:  Vec<u8>,
    budget: Option<u64>,
}

/// What one serve loop knows about the install it serves: the record's identity and the means to
/// replace a trapped instance.
struct Served {
    log:      InvocationLog,
    artifact: ArtifactId,
    provides: String,
    signer:   Vec<u8>,
    ns:       Arc<str>,
    again:    Reinstantiate,
}

impl Served {
    fn fresh(&self, agent: &GossipAgent) -> Result<Instance, WasmHostError> {
        let state = HostState::new(agent.node_id().clone(), self.ns.clone(), agent.kv(), agent.mesh());
        self.again.host.instantiate_with_fuel(&self.again.bytes, state, self.again.budget)
    }
}

async fn serve_loop(agent: Arc<GossipAgent>, mut instance: Instance, mut rx: mycelium::RpcRequestRx, served: Served) {
    let Served { log, artifact, provides, signer, .. } = &served;
    let (artifact, provides) = (*artifact, provides.clone());
    while let Some(req) = rx.recv().await {
        let payload = req.payload().to_vec();
        // NB: wasm execution is synchronous and blocks this task for its duration — fine for
        // short handlers; a fuel budget bounds a runaway one (D19), epoch limits + spawn_blocking
        // stay a follow-up.
        let (result, outcome): (Vec<u8>, InvocationOutcome) = match instance.invoke("invoke", payload) {
            Ok(Ok(out)) => (out, InvocationOutcome::Completed),
            Ok(Err(e)) => (format!("component-error: {e}").into_bytes(), InvocationOutcome::ComponentError(e)),
            Err(WasmHostError::FuelExhausted { budget }) => (
                format!("host-error: {}", WasmHostError::FuelExhausted { budget }).into_bytes(),
                InvocationOutcome::FuelExhausted { budget },
            ),
            Err(e) => (format!("host-error: {e}").into_bytes(), InvocationOutcome::HostError(e.to_string())),
        };
        if matches!(outcome, InvocationOutcome::FuelExhausted { .. } | InvocationOutcome::HostError(_)) {
            // The trapped instance cannot be entered again: replace it, or stop serving so the
            // provisioner's probe withdraws and reinstalls (restart ≡ provisioning).
            match served.fresh(&agent) {
                Ok(fresh) => {
                    tracing::info!(%artifact, provides = %provides, "trapped instance replaced with a fresh one");
                    instance = fresh;
                }
                Err(e) => {
                    tracing::error!(%artifact, provides = %provides, error = %e,
                        "could not replace the trapped instance — the serve task stops; the probe will withdraw the install");
                    agent.service().rpc_respond(&req, result);
                    record_invocation(log, InvocationRecord { artifact, provides, signer: signer.clone(), outcome });
                    return;
                }
            }
        }
        let label = match &outcome {
            InvocationOutcome::Completed => "completed",
            InvocationOutcome::ComponentError(_) => "component_error",
            InvocationOutcome::HostError(_) => "host_error",
            InvocationOutcome::FuelExhausted { .. } => "fuel_exhausted",
        };
        metrics::counter!("mycelium_artifact_invocations_total", "outcome" => label).increment(1);
        if let InvocationOutcome::FuelExhausted { budget } = &outcome {
            tracing::warn!(%artifact, provides = %provides, budget, "hosted component stopped at its fuel budget");
        }
        record_invocation(log, InvocationRecord {
            artifact,
            provides: provides.clone(),
            signer: signer.clone(),
            outcome,
        });
        agent.service().rpc_respond(&req, result);
    }
}

/// The [`ArtifactKind::WasmComponent`] runtime — the original install path (pull + verify +
/// instantiate + serve), relocated behind the trait unchanged. `WasmHost` is the engine *inside*
/// this runtime, not the definition of install.
pub struct WasmComponentRuntime {
    host:   Arc<WasmHost>,
    /// D19: who runs metered. `None` = every entry gets the host's own default budget, as before.
    policy: Option<FuelPolicy>,
    log:    InvocationLog,
}

impl WasmComponentRuntime {
    pub fn new(host: Arc<WasmHost>) -> Self {
        Self { host, policy: None, log: Arc::new(Mutex::new(VecDeque::new())) }
    }

    /// Attach the fuel policy (D19). A budget in it needs a metered host
    /// ([`WasmHost::metered`] / [`WasmHost::with_fuel_per_call`]); an install on an unmetered host
    /// under a budget fails by name rather than running unbounded.
    pub fn with_fuel_policy(mut self, policy: FuelPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Share this runtime's invocation log (so a provisioner can hand it out).
    pub fn with_invocation_log(mut self, log: InvocationLog) -> Self {
        self.log = log;
        self
    }

    /// The runtime's invocation log, newest last.
    pub fn invocations(&self) -> Vec<InvocationRecord> {
        self.log.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }
}

#[async_trait::async_trait]
impl ArtifactRuntime for WasmComponentRuntime {
    fn kind(&self) -> ArtifactKind {
        ArtifactKind::WasmComponent
    }

    async fn install(
        &self,
        entry: InstallableEntry,
        source: Arc<dyn ArtifactSource + Send + Sync>,
        ctx: RuntimeCtx,
        progress: ProgressFn,
    ) -> Result<Box<dyn Installed>, InstallError> {
        let state = HostState::new(
            ctx.agent.node_id().clone(),
            entry.provides.namespace.clone(),
            ctx.agent.kv(),
            ctx.agent.mesh(),
        );
        // Components are small (well under the mesh frame cap): pull + verify + instantiate in
        // one step. Chunked/ranged pulls with incremental progress are the blob runtime's job.
        // The verified bytes are kept so a trapped instance can be replaced (see `Reinstantiate`).
        let bytes = source
            .fetch(&entry.artifact)
            .ok_or_else(|| WasmHostError::Fetch(format!("no source holds artifact {}", entry.artifact)))?;
        crate::artifact::verify_artifact(&bytes, &entry.artifact).map_err(|e| WasmHostError::Verify(e.to_string()))?;
        let budget = match &self.policy {
            Some(p) => p.budget_for(&entry.signer),
            None => self.host.fuel_per_call(),
        };
        let instance = self.host.instantiate_with_fuel(&bytes, state, budget)?;
        progress(entry.size_bytes, entry.size_bytes);

        // Register the inbound serve handler *before* returning, so the advertisement the
        // provisioner makes on success always finds a live RPC receiver.
        let invoke_kind = cap_invoke_kind(&entry.provides.namespace, &entry.provides.name);
        #[cfg(feature = "gateway")]
        let bridge_kind: Arc<str> = Arc::from(invoke_kind.as_str());
        let rx = ctx.agent.service().rpc_rx(invoke_kind);
        let provides = format!("{}/{}", entry.provides.namespace, entry.provides.name);
        let served = Served {
            log: Arc::clone(&self.log),
            artifact: entry.artifact,
            provides,
            signer: entry.signer.clone(),
            ns: entry.provides.namespace.clone(),
            again: Reinstantiate { host: Arc::clone(&self.host), bytes: bytes.to_vec(), budget },
        };
        let serve = tokio::spawn(serve_loop(Arc::clone(&ctx.agent), instance, rx, served));
        #[cfg(feature = "gateway")]
        let mcp_tool = if entry.provides.namespace.as_ref() == "tool" {
            let agent = Arc::clone(&ctx.agent);
            let kind = bridge_kind;
            let schema = serde_json::json!({
                "description": format!("{} — an installed WASM component, bridged over mcp.invoke", entry.provides.name),
                "inputSchema": {"type": "object"},
            });
            Some(ctx.agent.mcp().register_mcp_tool(entry.provides.name.clone(), schema, move |args: serde_json::Value| {
                let agent = Arc::clone(&agent);
                let kind = Arc::clone(&kind);
                async move {
                    let out = agent
                        .service()
                        .rpc_call(agent.node_id().clone(), kind, args.to_string().into_bytes(), std::time::Duration::from_secs(30))
                        .await
                        .map_err(|e| format!("bridge: {e}"))?;
                    if out.starts_with(b"component-error:") || out.starts_with(b"host-error:") {
                        return Err(String::from_utf8_lossy(&out).into_owned());
                    }
                    Ok(serde_json::from_slice::<serde_json::Value>(&out)
                        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&out).into_owned())))
                }
            }))
        } else {
            None
        };
        Ok(Box::new(WasmInstalled {
            serve,
            #[cfg(feature = "gateway")]
            _mcp_tool: mcp_tool,
        }))
    }
}

/// A live, serving WASM component installation.
struct WasmInstalled {
    serve: tokio::task::JoinHandle<()>,
    /// X2: a `tool/{name}` component is also an MCP tool `{name}` (`tools/{name}/{node}` in KV,
    /// `tools/call` over `mcp.invoke`), forwarded to the component's serve loop. Dropped with the
    /// install, so the tool disappears with the capability. Needs the node's gateway feature.
    #[cfg(feature = "gateway")]
    _mcp_tool: Option<mycelium::McpToolHandle>,
}

impl Installed for WasmInstalled {
    fn probe(&self) -> bool {
        !self.serve.is_finished()
    }

    fn uninstall(self: Box<Self>) {
        self.serve.abort();
    }
}

/// The [`ArtifactKind::Blob`] runtime — **place-and-probe** install for artifacts a node-local
/// runtime consumes from disk (LLM/ONNX weights, data packs; design §4.2, §5).
///
/// Install = pull → verify → **place** at `{place_dir}/{artifact-hex}`, complete-or-absent
/// (temp write + rename, hash checked before the rename — a crash mid-pull leaves no partial
/// placed file). When the source supports ranged reads ([`RangedArtifactSource`]) the pull
/// **streams chunk-by-chunk with real progress** — a multi-GB blob never materialises in
/// memory; otherwise it falls back to whole-bytes `fetch`. After placement an optional
/// **activation** hook hands the file to the local runtime (an Ollama load, an onnxruntime
/// session); the capability's health is the **probe** (default: the placed file exists —
/// override with e.g. "does the local runtime answer").
pub struct BlobRuntime {
    place_dir:   std::path::PathBuf,
    chunk_bytes: u64,
    probe:       Arc<ProbeFn>,
    activate:    Option<Arc<ActivateFn>>,
}

/// Health probe over the placed blob's path — see [`BlobRuntime::with_probe`].
pub type ProbeFn = dyn Fn(&std::path::Path) -> bool + Send + Sync;
/// Post-placement activation hook — see [`BlobRuntime::with_activation`].
pub type ActivateFn = dyn Fn(&std::path::Path) -> Result<(), String> + Send + Sync;

/// Default pull chunk: 4 MiB.
const DEFAULT_BLOB_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

impl BlobRuntime {
    /// A blob runtime placing artifacts under `place_dir` (created on first install). Default
    /// probe: the placed file exists.
    pub fn new(place_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            place_dir:   place_dir.into(),
            chunk_bytes: DEFAULT_BLOB_CHUNK_BYTES,
            probe:       Arc::new(|path| path.exists()),
            activate:    None,
        }
    }

    /// Override the ranged-pull chunk size (min 1; tests use tiny chunks to exercise streaming).
    pub fn with_chunk_bytes(mut self, chunk_bytes: u64) -> Self {
        self.chunk_bytes = chunk_bytes.max(1);
        self
    }

    /// Hook run after place + verify, before the capability goes live — hand the file to the
    /// node-local runtime (trigger a model load, open a session). An `Err` fails the install
    /// (the placed file stays for the retry — the pull is already verified).
    pub fn with_activation(
        mut self,
        activate: impl Fn(&std::path::Path) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.activate = Some(Arc::new(activate));
        self
    }

    /// Override the health probe (default: placed file exists). This is what gates the
    /// capability: "the local runtime actually answers", not "bytes are on disk".
    pub fn with_probe(
        mut self,
        probe: impl Fn(&std::path::Path) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.probe = Arc::new(probe);
        self
    }
}

/// Pull `id` from `source` into `tmp`, hashing as it goes: streamed in `chunk_bytes` ranges
/// when the source supports it (progress per chunk), whole-bytes otherwise (one progress
/// tick). The hash must equal the content address or the temp file is removed and the pull
/// fails — the source is untrusted either way.
fn pull_to_temp(
    source: &(dyn ArtifactSource + Send + Sync),
    id: &crate::artifact::ArtifactId,
    chunk_bytes: u64,
    tmp: &std::path::Path,
    progress: &ProgressFn,
) -> Result<(), InstallError> {
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let result = (|| -> Result<(), InstallError> {
        if let Some(ranged) = source.as_ranged()
            && let Some(total) = ranged.size(id)
        {
            let mut file = std::fs::File::create(tmp)
                .map_err(|e| InstallError::Place(format!("create temp: {e}")))?;
            let mut hasher = Sha256::new();
            let mut fetched = 0u64;
            while fetched < total {
                let want = chunk_bytes.min(total - fetched);
                let chunk = ranged
                    .fetch_range(id, fetched, want)
                    .ok_or_else(|| InstallError::Fetch(format!("ranged read failed at {fetched}")))?;
                if chunk.is_empty() {
                    return Err(InstallError::Fetch(format!(
                        "source returned an empty range at {fetched}/{total}"
                    )));
                }
                hasher.update(&chunk);
                file.write_all(&chunk).map_err(|e| InstallError::Place(format!("write temp: {e}")))?;
                fetched += chunk.len() as u64;
                progress(fetched, total);
            }
            let digest: [u8; 32] = hasher.finalize().into();
            if crate::artifact::ArtifactId::from_bytes(digest) != *id {
                return Err(InstallError::Verify("streamed bytes do not hash to the content address".into()));
            }
        } else {
            let bytes = source
                .fetch(id)
                .ok_or_else(|| InstallError::Fetch(format!("no source holds artifact {id}")))?;
            crate::artifact::verify_artifact(&bytes, id)
                .map_err(|e| InstallError::Verify(e.to_string()))?;
            std::fs::write(tmp, &bytes).map_err(|e| InstallError::Place(format!("write temp: {e}")))?;
            let len = bytes.len() as u64;
            progress(len, len);
        }
        Ok(())
    })();

    if result.is_err() {
        std::fs::remove_file(tmp).ok();
    }
    result
}

#[async_trait::async_trait]
impl ArtifactRuntime for BlobRuntime {
    fn kind(&self) -> ArtifactKind {
        ArtifactKind::Blob
    }

    fn resource_root(&self) -> Option<&std::path::Path> {
        Some(&self.place_dir)
    }

    async fn install(
        &self,
        entry: InstallableEntry,
        source: Arc<dyn ArtifactSource + Send + Sync>,
        _ctx: RuntimeCtx,
        progress: ProgressFn,
    ) -> Result<Box<dyn Installed>, InstallError> {
        std::fs::create_dir_all(&self.place_dir)
            .map_err(|e| InstallError::Place(format!("create place dir: {e}")))?;
        let dest = self.place_dir.join(entry.artifact.to_hex());

        // Fail fast before a multi-GB pull if the declared footprint provably can't land.
        // Advisory only (time-of-check): the eligibility gate applies headroom *policy*; this
        // is the absolute impossibility check, and a mid-pull ENOSPC still fails cleanly
        // (temp-write + rename ⇒ complete-or-absent, the reservation drops, a later round
        // retries — possibly on a node that fits).
        if entry.requires.disk_bytes > 0 && !dest.exists() {
            use crate::resources::ResourceProbe;
            if let Some(avail) =
                crate::resources::SystemResourceProbe::new().available_disk_bytes(&self.place_dir)
                && entry.requires.disk_bytes > avail
            {
                return Err(InstallError::Resources(format!(
                    "insufficient disk at {}: artifact requires {} bytes, {} available",
                    self.place_dir.display(),
                    entry.requires.disk_bytes,
                    avail
                )));
            }
        }

        if dest.exists() {
            // Content-addressed name ⇒ already placed and verified; reactivate only.
            progress(entry.size_bytes, entry.size_bytes);
        } else {
            static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let tmp = self.place_dir.join(format!(
                ".tmp-{}-{}-{}",
                entry.artifact.to_hex(),
                std::process::id(),
                TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            ));
            // The pull is synchronous I/O (ranged reads + file writes + hashing) — run it on
            // the blocking pool so a multi-GB stream never stalls the async workers.
            let pull = {
                let source = Arc::clone(&source);
                let id = entry.artifact;
                let chunk = self.chunk_bytes;
                let tmp = tmp.clone();
                let progress = Arc::clone(&progress);
                tokio::task::spawn_blocking(move || {
                    pull_to_temp(&*source, &id, chunk, &tmp, &progress)
                })
            };
            pull.await.map_err(|e| InstallError::Host(format!("pull task: {e}")))??;
            std::fs::rename(&tmp, &dest)
                .map_err(|e| InstallError::Place(format!("place blob: {e}")))?;
        }

        if let Some(activate) = &self.activate {
            activate(&dest).map_err(InstallError::Activation)?;
        }

        Ok(Box::new(BlobInstalled { path: dest, probe: Arc::clone(&self.probe) }))
    }
}

/// A placed (and activated) blob installation.
struct BlobInstalled {
    path:  std::path::PathBuf,
    probe: Arc<ProbeFn>,
}

impl Installed for BlobInstalled {
    fn probe(&self) -> bool {
        (self.probe)(&self.path)
    }

    fn uninstall(self: Box<Self>) {
        std::fs::remove_file(&self.path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::{ArtifactId, FsLibrarySource, RangedArtifactSource};
    use crate::catalog::InstallableEntry;
    use bytes::Bytes;
    use mycelium::Capability;
    use std::sync::Mutex;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mycelium-blob-runtime-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Unstarted agent — the blob runtime never touches the mesh; `RuntimeCtx` just needs one.
    fn idle_ctx() -> RuntimeCtx {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let id = mycelium::NodeId::new("127.0.0.1", port).unwrap();
        let agent = Arc::new(GossipAgent::new(
            id,
            mycelium::GossipConfig { bind_port: port, ..Default::default() },
        ));
        RuntimeCtx { agent }
    }

    type ProgressLog = Arc<Mutex<Vec<(u64, u64)>>>;

    fn recording_progress() -> (ProgressFn, ProgressLog) {
        let log: ProgressLog = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&log);
        (Arc::new(move |f, t| l.lock().unwrap().push((f, t))), log)
    }

    #[tokio::test]
    async fn blob_install_streams_places_activates_probes_and_uninstalls() {
        let lib_dir = scratch_dir("lib");
        let place_dir = scratch_dir("place");
        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let payload: Vec<u8> = (0..100u8).collect();
        let id = lib.store(&payload).unwrap();
        let entry = InstallableEntry::new(Capability::new("llm", "weights"), id)
            .with_kind(ArtifactKind::Blob)
            .with_cost(payload.len() as u64, 1);

        let activated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let act = Arc::clone(&activated);
        let runtime = BlobRuntime::new(&place_dir).with_chunk_bytes(16).with_activation(move |path| {
            act.store(path.exists(), std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });

        let (progress, log) = recording_progress();
        let installed = runtime
            .install(entry, Arc::clone(&lib) as Arc<_>, idle_ctx(), progress)
            .await
            .expect("blob installs");

        // Placed, content-correct, complete-or-absent (no temp residue).
        let dest = place_dir.join(id.to_hex());
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert!(activated.load(std::sync::atomic::Ordering::SeqCst), "activation saw the placed file");

        // Streamed: 100 bytes at 16-byte chunks = 7 monotonic progress ticks ending complete.
        let ticks = log.lock().unwrap().clone();
        assert_eq!(ticks.len(), 7, "ranged pull reports per-chunk progress");
        assert!(ticks.windows(2).all(|w| w[0].0 < w[1].0), "progress is monotonic");
        assert_eq!(*ticks.last().unwrap(), (100, 100));

        assert!(installed.probe(), "default probe: placed file exists");
        installed.uninstall();
        assert!(!dest.exists(), "uninstall removes the placed blob");

        std::fs::remove_dir_all(&lib_dir).ok();
        std::fs::remove_dir_all(&place_dir).ok();
    }

    /// A source that serves bytes which do not hash to the requested id — the untrusted-source
    /// case, in both whole-bytes and ranged flavours.
    struct LyingSource {
        ranged: bool,
    }

    impl ArtifactSource for LyingSource {
        fn fetch(&self, _id: &ArtifactId) -> Option<Bytes> {
            Some(Bytes::from_static(b"not the bytes you asked for"))
        }
        fn as_ranged(&self) -> Option<&dyn RangedArtifactSource> {
            if self.ranged { Some(self) } else { None }
        }
    }

    impl RangedArtifactSource for LyingSource {
        fn size(&self, _id: &ArtifactId) -> Option<u64> {
            Some(27)
        }
        fn fetch_range(&self, _id: &ArtifactId, offset: u64, len: u64) -> Option<Bytes> {
            let fake = b"not the bytes you asked for";
            let start = offset as usize;
            let end = (offset + len).min(27) as usize;
            Some(Bytes::copy_from_slice(&fake[start..end]))
        }
    }

    #[tokio::test]
    async fn blob_install_rejects_lying_sources_and_leaves_no_partial_file() {
        for ranged in [false, true] {
            let place_dir = scratch_dir(if ranged { "lie-ranged" } else { "lie-whole" });
            let id = ArtifactId::of(b"the artifact the catalogue promised");
            let entry = InstallableEntry::new(Capability::new("llm", "weights"), id)
                .with_kind(ArtifactKind::Blob);
            let runtime = BlobRuntime::new(&place_dir).with_chunk_bytes(8);

            let (progress, _log) = recording_progress();
            let err = match runtime
                .install(entry, Arc::new(LyingSource { ranged }), idle_ctx(), progress)
                .await
            {
                Err(e) => e,
                Ok(_) => panic!("hash mismatch must fail the install (ranged={ranged})"),
            };
            assert!(err.to_string().contains("hash") || err.to_string().contains("content address"),
                "verify failure surfaces: {err}");

            // Complete-or-absent: nothing placed, no temp residue.
            let residue: Vec<_> = std::fs::read_dir(&place_dir).unwrap().collect();
            assert!(residue.is_empty(), "ranged={ranged}: place dir must be empty, got {residue:?}");

            std::fs::remove_dir_all(&place_dir).ok();
        }
    }

    #[tokio::test]
    async fn blob_install_fails_fast_on_an_impossible_disk_requirement() {
        let place_dir = scratch_dir("nospace");
        let id = ArtifactId::of(b"colossal model");
        let entry = InstallableEntry::new(Capability::new("llm", "weights"), id)
            .with_kind(ArtifactKind::Blob)
            .with_requirements(u64::MAX / 2, 0);

        let (progress, log) = recording_progress();
        let result = BlobRuntime::new(&place_dir)
            .install(entry, Arc::new(crate::InMemorySource::new()), idle_ctx(), progress)
            .await;
        match result {
            // Measurable platform: the declared footprint provably can't land → fail fast,
            // before any pull. Unmeasurable platform (permissive contract): the empty source
            // fails the pull instead — either way it's an Err and nothing was placed.
            Err(e) => assert!(
                e.to_string().contains("insufficient disk") || e.to_string().contains("no source"),
                "got: {e}"
            ),
            Ok(_) => panic!("an impossible disk requirement must not install"),
        }
        let residue: Vec<_> = std::fs::read_dir(&place_dir).unwrap().collect();
        assert!(residue.is_empty(), "nothing placed, no temp residue");
        drop(log);

        std::fs::remove_dir_all(&place_dir).ok();
    }

    #[tokio::test]
    async fn blob_install_reuses_an_already_placed_artifact_without_a_source() {
        // Content-addressed placement means a re-install (restart ≡ provisioning) needs no
        // pull at all: the placed file short-circuits, activation still runs.
        let place_dir = scratch_dir("reuse");
        let payload = b"already on this node".to_vec();
        let id = ArtifactId::of(&payload);
        std::fs::write(place_dir.join(id.to_hex()), &payload).unwrap();

        let entry = InstallableEntry::new(Capability::new("llm", "weights"), id)
            .with_kind(ArtifactKind::Blob)
            .with_cost(payload.len() as u64, 1);
        // An empty source: any pull attempt would fail — reuse must not pull.
        let empty = Arc::new(crate::InMemorySource::new());

        let (progress, log) = recording_progress();
        let installed = BlobRuntime::new(&place_dir)
            .install(entry, empty, idle_ctx(), progress)
            .await
            .expect("already-placed blob reinstalls without a source");
        assert!(installed.probe());
        assert_eq!(*log.lock().unwrap().last().unwrap(), (20, 20), "reuse reports completion");

        std::fs::remove_dir_all(&place_dir).ok();
    }
}
