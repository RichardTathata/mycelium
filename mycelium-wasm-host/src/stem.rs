//! The **stem node** — a node that declares from its unit file and loads what the declarations
//! call for (`docs/plans/design-time-tooling.md` §13, D18; phase R1's runtime half, R2's image).
//!
//! A stem node holds nothing application-specific at deploy time. Given a
//! [`NodeCapabilityConfig`] it:
//!
//! - **advertises** every `[[capability]]` (always-alive; a `probe_url` is the node binary's
//!   probe loop's business and is logged, not honoured, here),
//! - **declares** every `[[requirement]]` (`declare_requirement`) and **defines** every
//!   `[[group]]` (`define_capability_group`), so the design-time file and the runtime vocabulary
//!   are one thing — every declaration still travels as its own evaporating KV entry, and no
//!   other node reads the file,
//! - and, when the file has a `[hosts]` table, runs a **provisioner** configured from it —
//!   kinds → runtimes, install budget, headroom, trusted publisher keys, placement root, fuel —
//!   with every `[[presence]]` as a supervision policy, ticking against the **live** catalogue
//!   (refreshed from KV each tick, because the catalogue arrives by gossip after the node
//!   starts).
//!
//! `[[lane]]`, `[[mandate]]` and `[[rule]]` are vocabulary for the offline check and the
//! evaluator; a stem node does nothing with them at runtime and says so once at start.
//!
//! **What a stem fleet is not** (plan §13): only WASM components and blobs load dynamically;
//! the stem image is itself a direct deployment; the catalogue is the supply chain, so the
//! publisher keys in `[hosts].trusted_publishers` are the posture to review.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycelium::{CapValue, Capability, CapabilityGroupHandle, CapabilityReg, GossipAgent, NodeCapabilityConfig, RequirementHandle};

use crate::artifact::ArtifactSource;
use crate::catalog::InstallableCatalog;
use crate::host::WasmHost;
use crate::librarian::{librarian_filter, LIBRARIAN_NAME, LIBRARIAN_NS};
use crate::http_source::DiskStagedSource;
use crate::mesh_source::{serve_artifacts, MeshRangedFetcher};
use crate::provisioner::Provisioner;
use crate::resources::SystemResourceProbe;
use crate::runtime::{BlobRuntime, FuelPolicy};
use crate::FsLibrarySource;

/// Where a stem node pulls artifact bytes from.
#[derive(Debug, Clone)]
pub enum StemSource {
    /// Peer pull over the mesh from whoever advertises the librarian (or a cache): a component in
    /// one whole-object pull, a blob past the frame cap in ranges, both **staged to disk** and
    /// verified before the runtime reads them (zero-gaps Z3) — nothing the stem pulls lives in
    /// memory, and what it staged it re-serves to peers.
    Mesh { timeout: Duration },
    /// A library directory this node can read (a mounted volume, or the librarian's own store):
    /// ranged, so blobs stream to the placement root.
    Library(std::path::PathBuf),
    /// An object store by URL (`s3://bucket/prefix`, `gs://…`, `file:///dir`; zero-gaps Z1, D1): the
    /// blobs and the manifest live there, credentials come from the environment, the node's egress
    /// policy gates the URL, and what the stem pulls is staged to disk in ranges exactly as from a
    /// mesh peer — and re-served to peers that have no credentials. Needs feature `object_store`.
    Store { url: String },
}

/// How a stem node runs.
#[derive(Debug, Clone)]
pub struct StemOptions {
    pub source:             StemSource,
    /// The provisioner's tick.
    pub tick:               Duration,
    /// Probability of self-electing for an unmet want on a round (herd damping); `1.0` for a
    /// small fleet or a test.
    pub self_elect_p:       f64,
    /// Re-assertion interval for declared requirements and groups.
    pub declare_interval:   Duration,
    /// How often a live `[[activation]]`'s probe is re-run (X2).
    pub reprobe_every:      Duration,
    /// The decision trace (plan I5): a sink the provisioner records every round's decisions into,
    /// from values it already produced. `None` (the default) records nothing.
    pub trace:              Option<Arc<mycelium::decision::DecisionSink>>,
    /// Where a mesh-sourced stem stages what it pulls (zero-gaps Z3); `None` is
    /// `<placement_root>/stage`. Content-addressed and verified on read, so a stale stage is harmless.
    pub stage_dir:          Option<std::path::PathBuf>,
    /// The most one artifact may stage, in bytes (default
    /// [`DEFAULT_MAX_STAGE_BYTES`](crate::DEFAULT_MAX_STAGE_BYTES), 64 GiB). A holder's
    /// `artifact.size` reply past it — or past the entry's own `size_bytes` hint — is refused
    /// before a byte is requested.
    pub max_stage_bytes:    u64,
}

impl Default for StemOptions {
    fn default() -> Self {
        Self {
            source:           StemSource::Mesh { timeout: Duration::from_secs(5) },
            tick:             Duration::from_millis(500),
            self_elect_p:     0.5,
            declare_interval: Duration::from_secs(5),
            reprobe_every:    Duration::from_secs(10),
            trace:            None,
            stage_dir:        None,
            max_stage_bytes:  crate::DEFAULT_MAX_STAGE_BYTES,
        }
    }
}

/// Why a stem could not start from its file. Each names the section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemError(pub String);

impl std::fmt::Display for StemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for StemError {}

/// A running stem node. Dropping it retracts every advertisement, requirement and group it
/// declared and stops the provisioner's ticker; installed capabilities are withdrawn with the
/// provisioner. Prefer [`stop`](Self::stop) to wait for the ticker.
pub struct Stem {
    _caps:   Vec<CapabilityReg>,
    _reqs:   Vec<RequirementHandle>,
    _groups: Vec<CapabilityGroupHandle>,
    stop:    Option<tokio::sync::watch::Sender<bool>>,
    ticker:  Option<tokio::task::JoinHandle<()>>,
    hosted:  Arc<AtomicUsize>,
    /// X2: on the mesh path a hosting stem re-serves its verified cache to peers (the late
    /// joiner's source once the librarian is gone), advertised as `artifact/librarian` with
    /// `role = "cache"` only once the cache holds something.
    _peer_serve: Option<tokio::task::JoinHandle<()>>,
    /// X2: the `[[activation]]` re-probe task, when the unit declares any.
    reprobe: Option<tokio::task::JoinHandle<()>>,
    /// X2: the `[[serve]]` task, when the unit declares any (feature `llm`).
    serve: Option<tokio::task::JoinHandle<()>>,
    /// Row M: what applies to each `[[activation]]` on this node, as assessed at start.
    confinement: Vec<(String, crate::activation_sandbox::Confinement)>,
    /// Rows L and M: each `[activation.confine.egress]` activation's proxy (or why it could not start),
    /// by its index in the unit's `[[activation]]` list, kept for the stem's life.
    egress: crate::activation::EgressProxies,
    /// `ns/name` of each `[[activation]]`, by index — what [`Stem::set_activation_egress_hosts`] looks up.
    activation_names: Vec<String>,
}

/// Start an egress proxy for every `[[activation]]` whose profile names `[activation.confine.egress]`
/// (post-360 rows L and M). A misconfiguration refuses the stem's start by name — a credential that is not
/// a binding on this node, a host the node's allow-list denies, or any way a binding's value could reach a
/// confined command ([`check_confined_secrets`]). A proxy that cannot **start** (it cannot
/// bind, no runtime) is not a start refusal: its activation is refused, by name, each time it would run.
pub fn start_egress_proxies(agent: &GossipAgent, units: &NodeCapabilityConfig) -> Result<crate::activation::EgressProxies, StemError> {
    start_egress_proxies_on(agent, units, crate::EgressProxy::DEFAULT_BIND)
}

/// [`start_egress_proxies`] on `bind` — a test forces a proxy that cannot start with an address in use.
pub fn start_egress_proxies_on(agent: &GossipAgent, units: &NodeCapabilityConfig, bind: std::net::SocketAddr) -> Result<crate::activation::EgressProxies, StemError> {
    check_confined_secrets(agent, units)?;
    let mut out = std::collections::HashMap::new();
    for (i, a) in units.activations.iter().enumerate() {
        let Some(e) = a.confine.as_ref().and_then(|c| c.egress.as_ref()) else { continue };
        let at = format!("{}/{}", a.ns, a.name);
        // A host the node's own allow-list refuses is a misconfiguration, refused here by name.
        crate::activation_egress::parse_hosts(&e.hosts, agent.egress_policy()).map_err(|why| StemError(format!("[[activation]] {at}: {why}")))?;
        #[cfg(any(feature = "gateway", feature = "llm"))]
        let gate = {
            for c in &e.credentials {
                if !agent.egress_credentials().is_some_and(|g| g.has(c)) {
                    return Err(StemError(format!(
                        "[[activation]] {at}: [activation.confine.egress] credential `{c}` is not a binding on this node — add [[egress_credentials]] name = \"{c}\" to the node's config (it is read at the agent's start)"
                    )));
                }
            }
            agent.egress_credentials()
        };
        #[cfg(not(any(feature = "gateway", feature = "llm")))]
        if let Some(c) = e.credentials.first() {
            return Err(StemError(format!(
                "[[activation]] {at}: [activation.confine.egress] credential `{c}` needs a stem built with the `gateway` or `llm` feature (the node's egress gate attaches it)"
            )));
        }
        let started = crate::EgressProxy::start(crate::activation_egress::ProxyOptions {
            label: at.clone(),
            decl: e,
            node_egress: agent.egress_policy(),
            bind,
            limits: Default::default(),
            peer_uid: None,
            fd_budget: None,
            #[cfg(any(feature = "gateway", feature = "llm"))]
            gate,
        });
        if let Err(why) = &started {
            tracing::warn!(activation = %at, reason = %why, "activation egress proxy could not start: the activation is refused, never run without it");
        }
        out.insert(i, started.map(Arc::new));
    }
    Ok(Arc::new(out))
}

/// No credential reaches a **confined** command another way (#644's review, finding 3): for every confined
/// `[[activation]]`, refused by name —
/// - an `[activation.confine] env` entry naming a binding's `value_env` variable;
/// - an `env` entry whose value, as the stem has it, **is** a binding's value (under any name);
/// - a binding's `value_file` the command could read: under the placement root, a `read` or `write` path, or
///   the platform's always-readable set ([`crate::activation_sandbox::readable_roots`]).
///
/// Every binding is checked against every confined activation, not only those granted it: a confined command
/// that can read a value file needs no grant. Names and paths are never echoed (a value pasted into a field
/// would be).
pub fn check_confined_secrets(agent: &GossipAgent, units: &NodeCapabilityConfig) -> Result<(), StemError> {
    let bindings = &agent.config().egress_credentials;
    let root = units.hosts.as_ref().and_then(|h| h.placement_root.clone()).unwrap_or_else(|| "artifacts".into());
    // #644's round-2 review, N7: a value file with a second hard link has another name that may lie in a
    // confined command's readable paths — refused while any activation is confined. (A file moved or linked
    // into a readable path **after** this check is not seen: the check runs once, at start.)
    if units.activations.iter().any(|a| a.confine.is_some()) {
        for (i, b) in bindings.iter().enumerate() {
            if let Some(file) = &b.value_file
                && crate::activation_sandbox::hard_links(file) > 1
            {
                return Err(StemError(format!(
                    "[[egress_credentials]] [{i}] `{}`: its value_file has another hard link, which a confined command might read — give the value one name",
                    b.name
                )));
            }
        }
    }
    for a in &units.activations {
        let Some(c) = &a.confine else { continue };
        let at = format!("{}/{}", a.ns, a.name);
        for v in &c.env {
            if bindings.iter().any(|b| b.value_env.as_deref() == Some(v.as_str())) {
                return Err(StemError(format!(
                    "[[activation]] {at}: [activation.confine] env passes a variable an [[egress_credentials]] binding reads its value from — the value would be in the command's environment; name the binding in [activation.confine.egress] credentials instead"
                )));
            }
            // #644's round-2 review, N7: a value **inside** a longer string, not only equal to one.
            #[cfg(any(feature = "gateway", feature = "llm"))]
            if let (Ok(value), Some(gate)) = (std::env::var(v), agent.egress_credentials())
                && gate.appears_in(&value)
            {
                return Err(StemError(format!(
                    "[[activation]] {at}: [activation.confine] env passes a variable that holds an [[egress_credentials]] binding's value — it would be in the command's environment"
                )));
            }
        }
        let roots = crate::activation_sandbox::readable_roots(c, std::path::Path::new(&root));
        for (i, b) in bindings.iter().enumerate() {
            let Some(file) = &b.value_file else { continue };
            let abs = std::path::absolute(file).unwrap_or_else(|_| file.clone());
            let file = abs.canonicalize().unwrap_or(abs);
            if roots.iter().any(|r| file.starts_with(r)) {
                return Err(StemError(format!(
                    "[[activation]] {at}: [[egress_credentials]] [{i}] `{}`: its value_file is readable by this confined command (under the placement root, a declared read or write path, or the system set) — move the file out of every confined command's reach",
                    b.name
                )));
            }
        }
    }
    Ok(())
}

/// `"ed25519:<64 hex>"` → the verifying key bytes; `field` names the table entry in the refusal.
fn parse_publisher(field: &str, s: &str) -> Result<[u8; 32], StemError> {
    let hex = s
        .strip_prefix("ed25519:")
        .ok_or_else(|| StemError(format!("hosts.{field}: {s:?} must be ed25519:<64 hex>")))?;
    if hex.len() != 64 {
        return Err(StemError(format!("hosts.{field}: {s:?} must carry 64 hex characters")));
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| StemError(format!("hosts.{field}: {s:?} is not hex")))?;
    }
    Ok(out)
}

/// Zero-gaps Z2 (D2): the bearer a `[[serve]]` sends — `api_key` as written, or the value of the
/// variable `api_key_env` names, read once here. An unset variable is a refusal that names it.
///
/// Post-360 plan row L: a `credential` resolves to the binding's **placeholder**, never its value — the
/// node's egress gate attaches the value at the binding's host (`serve::spawn` hands the backend the
/// gate). Whether the node holds that binding is checked by `Stem::start`, which has the agent.
pub fn resolve_serve_key(d: &mycelium::ServeDecl) -> Result<Option<String>, StemError> {
    if let Some(binding) = &d.credential {
        return Ok(Some(mycelium::credential_placeholder(binding)));
    }
    if let Some(lit) = &d.api_key {
        return Ok(Some(lit.clone()));
    }
    match &d.api_key_env {
        None => Ok(None),
        // Named by its section and field, never by the field's contents: a key pasted into
        // `api_key_env` was echoed into a start-time log line (row L's review, finding 2).
        Some(var) => std::env::var(var).map(Some).map_err(|_| StemError(format!(
            "[[serve]] {}/{}: the variable its api_key_env names is not set — export it on this host, or use api_key for a literal", d.ns, d.name
        ))),
    }
}

impl Stem {
    /// Declare everything in `units` on `agent` and, if it hosts, start provisioning.
    /// `agent` must already be started.
    pub fn start(agent: Arc<GossipAgent>, units: &NodeCapabilityConfig, opts: StemOptions) -> Result<Self, StemError> {
        let watch: crate::activation::WatchList = Arc::new(std::sync::Mutex::new(Vec::new()));
        units.validate().map_err(|e| StemError(e.to_string()))?;
        // Rows L and M: each `[activation.confine.egress]` activation's proxy, before anything can run it.
        let egress = start_egress_proxies(&agent, units)?;

        #[cfg(not(feature = "llm"))]
        if !units.serves.is_empty() {
            return Err(StemError("[[serve]] needs a stem built with the `llm` feature".into()));
        }
        let mut caps = Vec::new();
        for c in &units.capabilities {
            if c.probe_url.is_some() {
                tracing::info!(ns = %c.ns, name = %c.name, "stem: probe_url is not honoured here — advertised as always-alive (the node binary's probe loop honours it)");
            }
            caps.push(agent.capabilities().advertise_capability(c.build_capability_public(), Duration::from_secs(c.ttl_secs)));
        }
        let mut reqs = Vec::new();
        for r in &units.requirements {
            let filter = r.to_filter().map_err(StemError)?;
            reqs.push(agent.capabilities().declare_requirement(filter, opts.declare_interval));
        }
        let mut groups = Vec::new();
        for g in &units.groups {
            let def = g.to_def().map_err(StemError)?;
            groups.push(agent.capabilities().define_capability_group(g.name.as_str(), def, opts.declare_interval));
        }
        if !units.lanes.is_empty() || !units.mandates.is_empty() || !units.rules.is_empty() {
            tracing::info!(
                lanes = units.lanes.len(), mandates = units.mandates.len(), rules = units.rules.len(),
                "stem: lanes, mandates and rules are vocabulary for the check and the evaluator — nothing to do at runtime"
            );
        }

        let hosted = Arc::new(AtomicUsize::new(0));
        let (stop, ticker, peer_serve) = match &units.hosts {
            None => {
                if !units.presence.is_empty() {
                    return Err(StemError("[[presence]] needs a [hosts] table: only a hosting unit can keep providers alive".into()));
                }
                (None, None, None)
            }
            Some(h) => {
                let metered = h.fuel_per_call.is_some() || h.operator_fuel_per_call.is_some();
                let host = wasm_host_for(h)?;
                // Zero-gaps Z3: the mesh path stages to disk through the same `DiskStagedSource` an
                // object store fills — a component in one pull, a blob past the frame cap in ranges —
                // under `<placement_root>/stage` (or `stage_dir`), so nothing pulled lives in memory
                // and the provisioner streams it like a library blob.
                let (source, mesh): (Arc<dyn ArtifactSource + Send + Sync>, Option<Arc<DiskStagedSource>>) = match &opts.source {
                    StemSource::Mesh { timeout } => {
                        let stage_dir = opts.stage_dir.clone().unwrap_or_else(|| match &h.placement_root {
                            Some(root) => std::path::Path::new(root).join("stage"),
                            None => std::env::temp_dir().join(format!("mycelium-stem-stage-{}", agent.node_id().to_socket_addr().port())),
                        });
                        let fetcher = Arc::new(MeshRangedFetcher::resolving(Arc::clone(&agent), librarian_filter(), *timeout));
                        let staged = Arc::new(DiskStagedSource::open(fetcher, &stage_dir)
                            .map_err(|e| StemError(format!("stage {}: {e}", stage_dir.display())))?
                            .with_max_stage_bytes(opts.max_stage_bytes));
                        (Arc::clone(&staged) as Arc<dyn ArtifactSource + Send + Sync>, Some(staged))
                    }
                    StemSource::Library(dir) => (Arc::new(
                        FsLibrarySource::open(dir).map_err(|e| StemError(format!("library {}: {e}", dir.display())))?,
                    ), None),
                    #[cfg(feature = "object_store")]
                    StemSource::Store { url } => {
                        let stage_dir = opts.stage_dir.clone().unwrap_or_else(|| match &h.placement_root {
                            Some(root) => std::path::Path::new(root).join("stage"),
                            None => std::env::temp_dir().join(format!("mycelium-stem-stage-{}", agent.node_id().to_socket_addr().port())),
                        });
                        let fetcher = Arc::new(
                            crate::object_store_source::ObjectStoreFetcher::from_url(url, agent.egress_policy().clone())
                                .map_err(|e| StemError(format!("store {url}: {e}")))?,
                        );
                        let staged = Arc::new(DiskStagedSource::open(fetcher, &stage_dir)
                            .map_err(|e| StemError(format!("stage {}: {e}", stage_dir.display())))?
                            .with_max_stage_bytes(opts.max_stage_bytes));
                        (Arc::clone(&staged) as Arc<dyn ArtifactSource + Send + Sync>, Some(staged))
                    }
                    #[cfg(not(feature = "object_store"))]
                    StemSource::Store { url } => {
                        return Err(StemError(format!("a store URL ({url}) needs a stem built with the `object_store` feature")));
                    }
                };
                // X2: whatever this stem staged and verified, it will answer for — in ranges too — so
                // a peer's pull finds it here before the library of record.
                let peer_serve = mesh.as_ref().map(|m| {
                    serve_artifacts(Arc::clone(&agent), Arc::clone(m) as Arc<dyn ArtifactSource + Send + Sync>)
                });
                let mut prov = Provisioner::new(
                    Arc::clone(&agent),
                    Arc::new(host),
                    InstallableCatalog::from_kv(&agent.kv()),
                    source,
                    opts.self_elect_p,
                );
                if let Some(sink) = opts.trace.clone() {
                    prov.with_decision_trace(sink);
                }
                if h.kinds.iter().any(|k| k == "blob") {
                    let root = h.placement_root.clone().unwrap_or_else(|| "artifacts".into());
                    let mut blob = BlobRuntime::new(root);
                    if !units.activations.is_empty() {
                        blob = blob.with_entry_activation(crate::activation::hook_with_egress(units.activations.clone(), Arc::clone(&watch), Arc::clone(&egress)));
                    }
                    prov.register_runtime(Arc::new(blob));
                }
                if let Some(b) = h.install_budget_bytes {
                    prov.set_install_budget(b);
                }
                if let Some(hr) = h.headroom {
                    prov.set_resource_policy(Arc::new(SystemResourceProbe::new()), hr);
                }
                if !h.trusted_publishers.is_empty() {
                    let keys = h
                        .trusted_publishers
                        .iter()
                        .map(|s| parse_publisher("trusted_publishers", s))
                        .collect::<Result<Vec<_>, _>>()?;
                    prov.require_provenance(keys);
                } else if h.accept_unsigned {
                    // `validate()` refused the empty list unless this was said on purpose; say it
                    // back once, where the operator reads the start.
                    tracing::warn!(
                        "[hosts] accept_unsigned = true: this stem installs entries with no provenance — \
                         any catalogue line any peer gossips installs here; list trusted_publishers to close it"
                    );
                } else {
                    // Unreachable through `NodeCapabilityConfig::validate()`; a config built in code
                    // and never validated gets the same refusal, by name.
                    return Err(StemError(
                        "hosts.trusted_publishers is empty and hosts.accept_unsigned is not set: list the publisher keys, or accept unsigned entries on purpose".into(),
                    ));
                }
                if metered {
                    let operator_publishers = h
                        .operator_publishers
                        .iter()
                        .map(|s| parse_publisher("operator_publishers", s))
                        .collect::<Result<Vec<_>, _>>()?;
                    prov.set_fuel_policy(FuelPolicy {
                        operator_publishers,
                        agent_budget: h.fuel_per_call,
                        operator_budget: h.operator_fuel_per_call,
                    });
                }
                if !h.trusted_reviewers.is_empty() {
                    let keys = h.trusted_reviewers.iter().map(|s| parse_publisher("trusted_reviewers", s)).collect::<Result<Vec<_>, _>>()?;
                    prov.require_reviewers(keys);
                }
                for p in &units.presence {
                    let filter = p.filter.to_filter().map_err(StemError)?;
                    match p.max_providers {
                        Some(max) => prov.supervise_band(filter, p.min_providers, max),
                        None => prov.supervise(filter, p.min_providers),
                    }
                }
                let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
                let tick = opts.tick;
                let kv = agent.kv();
                let hosted_w = Arc::clone(&hosted);
                let cache_agent = Arc::clone(&agent);
                let ticker = tokio::spawn(async move {
                    let mut cache_cap: Option<CapabilityReg> = None;
                    let mut backoff = StageBackoff::default();
                    let mut ticks: u64 = 0;
                    loop {
                        if *stop_rx.borrow_and_update() {
                            break;
                        }
                        prov.refresh_catalog(InstallableCatalog::from_kv(&kv));
                        if let Some(m) = &mesh {
                            // Stage every catalogue entry once (idempotent: a staged id is a size
                            // check, no request) — a component whole, a blob in ranges, each
                            // bounded by the entry's size hint and the stage's ceiling.
                            let wanted: Vec<_> = prov.catalog().entries().iter().map(|e| (e.artifact, e.size_bytes)).collect();
                            backoff.retain(|id| wanted.iter().any(|(w, _)| w == id));
                            for (id, size_hint) in wanted {
                                if !backoff.due(&id, ticks) {
                                    continue;
                                }
                                let outcome = m.try_stage(&id, size_hint).await;
                                backoff.record(id, outcome, ticks);
                            }
                            // Once the stage holds something, say so: a peer holder, not the
                            // library of record.
                            if cache_cap.is_none() && m.stage().list().map(|l| !l.is_empty()).unwrap_or(false) {
                                cache_cap = Some(cache_agent.capabilities().advertise_capability(
                                    Capability::new(LIBRARIAN_NS, LIBRARIAN_NAME).with("role", CapValue::Text(Arc::from("cache"))),
                                    Duration::from_secs(30),
                                ));
                            }
                        }
                        prov.provision_round();
                        hosted_w.store(prov.hosted_count(), Ordering::Relaxed);
                        ticks = ticks.wrapping_add(1);
                        tokio::select! {
                            _ = tokio::time::sleep(tick) => {}
                            _ = stop_rx.changed() => {}
                        }
                    }
                    drop(cache_cap);
                    drop(prov);
                });
                (Some(stop_tx), Some(ticker), peer_serve)
            }
        };

        // Row M: say once, at start, what applies to each activation — confined (and how), unconfined
        // (the default, with the stem's privileges), or refused (a profile this node cannot apply).
        // An activation whose egress proxy could not start is refused (rows L and M).
        let confinement: Vec<(String, crate::activation_sandbox::Confinement)> = units
            .activations
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let assessed = match egress.get(&i) {
                    Some(Err(why)) => crate::activation_sandbox::Confinement::Refused { reason: why.clone() },
                    _ => crate::activation_sandbox::assess(a.confine.as_ref()),
                };
                (format!("{}/{}", a.ns, a.name), assessed)
            })
            .collect();
        for (at, c) in &confinement {
            use crate::activation_sandbox::Confinement::*;
            match c {
                Unconfined => tracing::info!(activation = %at, "activation unconfined: its commands run with this stem's privileges (no [activation.confine])"),
                Confined { backend } => tracing::info!(activation = %at, backend = backend.as_str(), "activation confined"),
                Refused { reason } => tracing::warn!(activation = %at, %reason, "activation refused: its [activation.confine] profile cannot be applied on this node, and it will not run unconfined"),
            }
        }
        let reprobe = (!units.activations.is_empty()).then(|| crate::activation::spawn_reprobe(watch, opts.reprobe_every));
        #[cfg(feature = "llm")]
        let serve = if units.serves.is_empty() {
            None
        } else {
            // Zero-gaps Z2: every key resolved here, before anything is served — an unset variable
            // refuses the start by name rather than serving a skill with no key.
            let resolved = units.serves.iter()
                .map(|d| resolve_serve_key(d).map(|k| (d.clone(), k)))
                .collect::<Result<Vec<_>, _>>()?;
            // Row L: a `credential` must name a binding this node's gate holds — refused by name here,
            // not discovered as an endpoint's 401 (or a skill that never answers) later.
            for d in &units.serves {
                let Some(binding) = &d.credential else { continue };
                let Some(gate) = agent.egress_credentials().filter(|g| g.has(binding)) else {
                    return Err(StemError(format!(
                        "[[serve]] {}/{}: credential `{binding}` is not a binding on this node — add [[egress_credentials]] name = \"{binding}\" to the node's config (it is read at the agent's start)",
                        d.ns, d.name
                    )));
                };
                // Row L's review, finding 9: the endpoint the skill calls must be one its binding
                // authorises — refused here, by name, not as every call's refusal at the gate.
                let url = format!("{}/chat/completions", d.endpoint.trim_end_matches('/'));
                if let Err(e) = gate.authorises(binding, &url) {
                    return Err(StemError(format!("[[serve]] {}/{}: its endpoint is not one credential `{binding}` authorises — {e}", d.ns, d.name)));
                }
            }
            Some(crate::serve::spawn(Arc::clone(&agent), resolved, opts.tick))
        };
        #[cfg(not(feature = "llm"))]
        let serve: Option<tokio::task::JoinHandle<()>> = None;
        let activation_names = units.activations.iter().map(|a| format!("{}/{}", a.ns, a.name)).collect();
        Ok(Self { _caps: caps, _reqs: reqs, _groups: groups, stop, ticker, hosted, _peer_serve: peer_serve, reprobe, serve, confinement, egress, activation_names })
    }

    /// The egress proxy of the `[[activation]]` providing `ns/name`, when its profile names one and it is
    /// running (rows L and M).
    pub fn activation_egress(&self, ns: &str, name: &str) -> Option<Arc<crate::EgressProxy>> {
        let at = format!("{ns}/{name}");
        self.activation_names
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == at)
            .find_map(|(i, _)| self.egress.get(&i).and_then(|r| r.as_ref().ok()).cloned())
    }

    /// Replace the egress allow-list of the `[[activation]]` providing `ns/name` **while the stem runs**
    /// (post-360 row M's live network allow-list): the next request its command makes meets the new list;
    /// the command, its confinement profile and the proxy's port are unchanged. Every entry is checked as
    /// at start (form, duplicates, the node's egress allow-list) before anything changes — on `Err` the old
    /// list stands. Only `hosts` changes live; `credentials` and `env` are fixed for the stem's life.
    pub fn set_activation_egress_hosts(&self, ns: &str, name: &str, hosts: &[String]) -> Result<(), StemError> {
        let proxy = self.activation_egress(ns, name).ok_or_else(|| {
            StemError(format!("[[activation]] {ns}/{name}: no running egress proxy (the stem started without an [activation.confine.egress] table for it — a table added later needs a restart — or its proxy could not start)"))
        })?;
        proxy.set_hosts(hosts).map_err(|e| StemError(format!("[[activation]] {ns}/{name}: {e}")))
    }

    /// Apply `units`' `[activation.confine.egress] hosts` to each running proxy — what `mycelium-stem` does on
    /// `SIGHUP` with its unit file re-read. One answer per activation in `units` that names an egress table —
    /// the list now in force, or why it was refused (the old list then stands) — and one per running proxy the
    /// file no longer names, narrowed to an empty list. Only `hosts` is applied; every other change in the
    /// file needs a restart.
    pub fn reload_egress_hosts(&self, units: &NodeCapabilityConfig) -> Vec<(String, Result<Vec<String>, StemError>)> {
        let named: Vec<(String, &mycelium::EgressDecl)> = units
            .activations
            .iter()
            .filter_map(|a| a.confine.as_ref().and_then(|c| c.egress.as_ref()).map(|e| (format!("{}/{}", a.ns, a.name), e)))
            .collect();
        let mut out: Vec<(String, Result<Vec<String>, StemError>)> = named
            .iter()
            .map(|(at, e)| {
                let (ns, name) = at.split_once('/').unwrap_or((at.as_str(), ""));
                let applied = self
                    .set_activation_egress_hosts(ns, name, &e.hosts)
                    .and_then(|()| self.activation_egress(ns, name).map(|p| p.hosts()).ok_or_else(|| StemError("its proxy is gone".into())));
                (at.clone(), applied)
            })
            .collect();
        // #644's review, finding 2: a running proxy the re-read file no longer names — its table removed, its
        // activation removed or renamed — is narrowed to nothing, by name; the narrowest list is the one the
        // file now states for it.
        for (i, at) in self.activation_names.iter().enumerate() {
            if named.iter().any(|(n, _)| n == at) {
                continue;
            }
            if let Some(Ok(proxy)) = self.egress.get(&i) {
                let applied = proxy.set_hosts(&[]).map(|()| proxy.hosts()).map_err(StemError);
                tracing::warn!(activation = %at, "egress reload: the unit file no longer names this activation's egress table — its proxy now carries nothing (restart the stem to remove it)");
                out.push((at.clone(), applied));
            }
        }
        out
    }

    /// What applies to each `[[activation]]` (`ns/name`) on this node, as assessed at start (row M):
    /// confined and by which backend, unconfined, or refused. The node-wide summary is the
    /// `stem.activation_confined` guarantee ([`crate::activation_sandbox::confinement_guarantee`]).
    pub fn activation_confinement(&self) -> &[(String, crate::activation_sandbox::Confinement)] {
        &self.confinement
    }

    /// How many artifacts this stem currently hosts (as of its last tick).
    pub fn hosted_count(&self) -> usize {
        self.hosted.load(Ordering::Relaxed)
    }

    /// Stop the provisioner's ticker and wait for it; declarations are retracted on drop.
    pub async fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(t) = self.ticker.take() {
            let _ = t.await;
        }
        if let Some(s) = self._peer_serve.take() {
            s.abort();
        }
        if let Some(r) = self.reprobe.take() {
            r.abort();
        }
        if let Some(s) = self.serve.take() {
            s.abort();
        }
    }
}

impl Drop for Stem {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(true);
        }
        if let Some(t) = self.ticker.take() {
            t.abort();
        }
    }
}

/// The stem tick's retry discipline for staging. A failed stage is not retried every tick: after
/// the n-th consecutive failure the next attempt is due `2^min(n,6)` ticks after the failure
/// (2, 4, … at most 64). No holder answering is not a failure — nothing was attempted, and a
/// catalogue line can gossip ahead of its bytes — so it is retried on the next tick. Counted in
/// ticks, so it needs no clock of its own.
#[derive(Default)]
struct StageBackoff {
    /// Per id: consecutive failures, and the tick the next attempt is due.
    failures: std::collections::HashMap<crate::artifact::ArtifactId, (u32, u64)>,
}

impl StageBackoff {
    fn retain(&mut self, keep: impl Fn(&crate::artifact::ArtifactId) -> bool) {
        self.failures.retain(|id, _| keep(id));
    }

    fn due(&self, id: &crate::artifact::ArtifactId, tick: u64) -> bool {
        self.failures.get(id).is_none_or(|&(_, next)| tick >= next)
    }

    fn record(&mut self, id: crate::artifact::ArtifactId, outcome: crate::http_source::StageOutcome, tick: u64) {
        use crate::http_source::StageOutcome::*;
        match outcome {
            Staged => {
                self.failures.remove(&id);
            }
            NoHolder => {}
            Failed => {
                let e = self.failures.entry(id).or_insert((0, 0));
                e.0 = e.0.saturating_add(1);
                e.1 = tick.saturating_add(1u64 << e.0.min(6));
            }
        }
    }
}

/// The wasm host a `[hosts]` table asks for. D19: the engine counts fuel whenever any budget is
/// declared (who pays is the provisioner's policy, decided per entry from its verified signer).
/// Row D: every call is bounded in wall-clock time, metered or not — `call_deadline_ms`, or the
/// host's `DEFAULT_CALL_DEADLINE`.
fn wasm_host_for(h: &mycelium::HostsDecl) -> Result<WasmHost, StemError> {
    let metered = h.fuel_per_call.is_some() || h.operator_fuel_per_call.is_some();
    let host = if metered { WasmHost::metered() } else { WasmHost::new() }
        .map_err(|e| StemError(format!("wasm host: {e}")))?;
    Ok(match h.call_deadline_ms {
        Some(ms) => host.with_call_deadline(Some(Duration::from_millis(ms))),
        None => host,
    })
}

#[cfg(test)]
mod tests {
    /// Review finding 3: `[hosts].call_deadline_ms` reaches the host the stem builds; absent, the
    /// host's default stands. Planted: with the wiring removed the first assertion fails.
    #[test]
    fn hosts_call_deadline_ms_reaches_the_stems_wasm_host() {
        let h = mycelium::HostsDecl { call_deadline_ms: Some(250), ..Default::default() };
        assert_eq!(wasm_host_for(&h).unwrap().call_deadline(), Some(Duration::from_millis(250)));
        let fuelled = mycelium::HostsDecl { call_deadline_ms: Some(250), fuel_per_call: Some(10), ..Default::default() };
        let host = wasm_host_for(&fuelled).unwrap();
        assert!(host.is_metered());
        assert_eq!(host.call_deadline(), Some(Duration::from_millis(250)));
        assert_eq!(wasm_host_for(&mycelium::HostsDecl::default()).unwrap().call_deadline(), Some(crate::host::DEFAULT_CALL_DEADLINE));
    }


    /// A catalogue line can gossip ahead of its bytes: no holder answering is not a failed stage
    /// and must not push the next attempt out; a real failure backs off, the wait doubling per
    /// consecutive failure up to 64 ticks, counted from the failure. Seen failing first: a
    /// `NoHolder` outcome was counted and the id skipped on the next tick.
    #[test]
    fn a_missing_holder_is_not_a_failed_stage_and_a_failure_backs_off_from_the_failure() {
        use crate::http_source::StageOutcome::*;
        let id = crate::artifact::ArtifactId::of(b"named before its bytes arrive");
        let mut b = StageBackoff::default();

        // No holder yet: retried on the very next tick, however long it lasts.
        for t in 0..10 {
            assert!(b.due(&id, t), "tick {t}: a missing holder must not delay the next attempt");
            b.record(id, NoHolder, t);
        }
        // A failure at tick 11 waits two ticks from 11, not to the next multiple of two.
        b.record(id, Failed, 11);
        assert!(!b.due(&id, 12));
        assert!(b.due(&id, 13), "the first failure waits 2 ticks from the failure");
        b.record(id, Failed, 13);
        assert!(!b.due(&id, 16));
        assert!(b.due(&id, 17), "the second waits 4");
        // The wait is capped at 64 ticks.
        for t in [17u64, 100, 200, 300, 400, 500, 600] { b.record(id, Failed, t); }
        assert!(!b.due(&id, 663));
        assert!(b.due(&id, 664), "capped at 64 ticks");
        // A success clears it.
        b.record(id, Staged, 664);
        assert!(b.due(&id, 665));
    }
    use super::*;
    use crate::catalog::{InstallableEntry, Manifest, MANIFEST_FILE};
    use crate::librarian::{spawn_librarian, LibrarianConfig};
    use ed25519_dalek::SigningKey;
    use mycelium::{CapFilter, Capability, NodeId};

    const ECHO_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/echo_component.wasm");

    fn alloc_port() -> u16 {
        mycelium::test_util::alloc_port()
    }

    async fn agent(port: u16, bootstrap: Option<u16>) -> Arc<GossipAgent> {
        let id = NodeId::new("127.0.0.1", port).unwrap();
        let cfg = mycelium::GossipConfig {
            bind_port: port,
            bootstrap_peers: bootstrap.map(|b| vec![NodeId::new("127.0.0.1", b).unwrap()]).unwrap_or_default(),
            health_check_interval_secs: 2,
            ..Default::default()
        };
        let a = Arc::new(GossipAgent::new(id, cfg));
        a.start().await.expect("agent start");
        a
    }

    async fn wait_until(secs: u64, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(secs);
        while std::time::Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        cond()
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!("mycelium-stem-{tag}-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// Rows L and M at the stem: a misconfigured `[activation.confine.egress]` refuses the start by name — an
    /// `env` passing a binding's `value_env` variable, a credential the node has no binding for, a host the
    /// node's allow-list denies — while a proxy that cannot *start* is not a start refusal but its activation's
    /// (`Confinement::Refused`, naming it). A running stem replaces an activation's list live
    /// (`set_activation_egress_hosts`, `reload_egress_hosts` — `mycelium-stem`'s `SIGHUP`), a bad list leaving the
    /// old one in force. Seen failing first with the `env` check removed: the stem started.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_misconfigured_activation_egress_refuses_the_start_and_a_reload_applies_hosts() {
        let units_with = |name: &str, confine: &str, egress: Option<&str>| {
            NodeCapabilityConfig::from_toml_str(&format!(
                "[hosts]\nkinds=[\"blob\"]\naccept_unsigned=true\nplacement_root=\"{}\"\n\
                 [[activation]]\nns=\"data\"\nname=\"{name}\"\ncommand=[\"true\"]\n[activation.confine]\n{confine}{}",
                scratch("egress-units").display(),
                egress.map(|e| format!("[activation.confine.egress]\n{e}")).unwrap_or_default()
            ))
            .unwrap()
        };
        let units = |egress: &str| units_with("pack", "", Some(egress));
        let cfg = mycelium::GossipConfig {
            egress: mycelium::EgressPolicy { allow_hosts: vec!["127.0.0.1".into(), "api.example.com".into()] },
            egress_credentials: vec![mycelium::CredentialBinding {
                name: "model".into(), host: "api.example.com".into(), port: None, path_prefix: None,
                form: mycelium::CredentialForm::Bearer, header: None, username: None,
                value_env: Some("MYCELIUM_TEST_EGRESS_VALUE_ENV_7F".into()), value_file: None, allow_plaintext: false,
            }],
            ..Default::default()
        };
        let unstarted = GossipAgent::new(NodeId::new("127.0.0.1", alloc_port()).unwrap(), cfg);
        let e = start_egress_proxies(&unstarted, &units_with("pack", "env = [\"MYCELIUM_TEST_EGRESS_VALUE_ENV_7F\"]\n", None)).unwrap_err().to_string();
        assert!(e.contains("data/pack") && e.contains("env passes a variable") && !e.contains("MYCELIUM_TEST_EGRESS_VALUE_ENV_7F"), "{e}");
        let e = start_egress_proxies(&unstarted, &units("credentials = [\"model\"]\n")).unwrap_err().to_string();
        assert!(e.contains("data/pack") && e.contains("`model`"), "a credential with no gate to attach it (unstarted, or no gateway build): {e}");
        let e = start_egress_proxies(&unstarted, &units("hosts = [\"https://other.example\"]\n")).unwrap_err().to_string();
        assert!(e.contains("data/pack") && e.contains("allow-list does not permit other.example"), "{e}");

        // A proxy that cannot start: its activation's refusal, not the stem's.
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let map = start_egress_proxies_on(&unstarted, &units("hosts = [\"https://api.example.com\"]\n"), taken.local_addr().unwrap()).expect("not a start refusal");
        assert!(matches!(map.get(&0), Some(Err(why)) if why.contains("data/pack") && why.contains("could not start")), "{map:?}");
        drop(taken);

        // A running stem: the list replaced live, a bad one refused with the old one standing.
        let a = agent(alloc_port(), None).await;
        let stem = Stem::start(Arc::clone(&a), &units("hosts = [\"http://127.0.0.1:9\"]\n"), StemOptions::default()).expect("starts");
        let proxy = stem.activation_egress("data", "pack").expect("a running proxy");
        assert_eq!(proxy.hosts(), ["http://127.0.0.1:9"]);
        let reloaded = stem.reload_egress_hosts(&units("hosts = [\"http://127.0.0.1:9\", \"https://api.example.com\"]\n"));
        assert!(matches!(reloaded.as_slice(), [(at, Ok(h))] if at == "data/pack" && h.len() == 2), "{reloaded:?}");
        assert_eq!(stem.activation_egress("data", "pack").unwrap().port(), proxy.port(), "the same proxy");
        let e = stem.set_activation_egress_hosts("data", "pack", &["https://*.example.com".into()]).unwrap_err().to_string();
        assert!(e.contains("wildcard"), "{e}");
        assert_eq!(proxy.hosts().len(), 2, "the old list stands");
        assert!(stem.set_activation_egress_hosts("data", "other", &[]).is_err(), "no such activation");
        assert!(matches!(stem.activation_confinement(), [(at, _)] if at == "data/pack"));

        // #644's review, finding 2: a reload whose file no longer names the activation's egress table — table
        // removed, or the activation renamed — narrows the running proxy to nothing, by name. Seen failing
        // first: the old list stayed live and the reload said nothing about it.
        let reloaded = stem.reload_egress_hosts(&units_with("pack", "", None));
        assert!(matches!(reloaded.as_slice(), [(at, Ok(h))] if at == "data/pack" && h.is_empty()), "{reloaded:?}");
        assert!(proxy.hosts().is_empty(), "the removed table's list is gone: {:?}", proxy.hosts());
        proxy.set_hosts(&["http://127.0.0.1:9".into()]).unwrap();
        let reloaded = stem.reload_egress_hosts(&units_with("renamed", "", Some("hosts = [\"http://127.0.0.1:9\"]\n")));
        assert!(reloaded.iter().any(|(at, r)| at == "data/pack" && r.as_ref().is_ok_and(|h| h.is_empty())), "{reloaded:?}");
        assert!(proxy.hosts().is_empty(), "a renamed activation's old proxy is narrowed");
        // #646's review, round 2: a table named for an activation that started without a proxy is refused by name
        // (a table added later needs a restart).
        assert!(
            reloaded.iter().any(|(at, r)| at == "data/renamed" && r.as_ref().is_err_and(|e| e.to_string().contains("needs a restart"))),
            "{reloaded:?}"
        );
        stem.stop().await;
        a.shutdown().await;
    }

    /// #644's review, finding 3: no binding's value reaches a confined command another way — a `value_file`
    /// the command could read (under a `read` path, under the placement root, or the system set), or an
    /// `env` entry whose *value* is a binding's value under another name. Each refuses the start by name,
    /// for every confined activation (not only one granted the binding). Seen failing first: both started.
    #[cfg(any(feature = "gateway", feature = "llm"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn no_credential_reaches_a_confined_command_another_way() {
        const VALUE: &str = "sk-act-egress-value-file-7f";
        let dir = scratch("secret-dir");
        let placement = scratch("secret-placement");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&placement).unwrap();
        let file = dir.join("key");
        std::fs::write(&file, VALUE).unwrap();
        let cfg = mycelium::GossipConfig {
            bind_port: alloc_port(),
            egress_credentials: vec![mycelium::CredentialBinding {
                name: "model".into(), host: "api.example.com".into(), port: None, path_prefix: None,
                form: mycelium::CredentialForm::Bearer, header: None, username: None,
                value_env: None, value_file: Some(file.clone()), allow_plaintext: false,
            }],
            ..Default::default()
        };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), cfg));
        a.start().await.expect("starts");
        let units = |placement: &std::path::Path, confine: &str| {
            NodeCapabilityConfig::from_toml_str(&format!(
                "[hosts]\nkinds=[\"blob\"]\naccept_unsigned=true\nplacement_root=\"{}\"\n\
                 [[activation]]\nns=\"data\"\nname=\"pack\"\ncommand=[\"true\"]\n[activation.confine]\n{confine}",
                placement.display()
            ))
            .unwrap()
        };
        let e = check_confined_secrets(&a, &units(&placement, &format!("read = [\"{}\"]\n", dir.display()))).unwrap_err().to_string();
        assert!(e.contains("data/pack") && e.contains("`model`") && e.contains("value_file") && !e.contains(&file.display().to_string()), "{e}");
        let e = check_confined_secrets(&a, &units(&dir, "")).unwrap_err().to_string();
        assert!(e.contains("value_file"), "under the placement root: {e}");
        check_confined_secrets(&a, &units(&placement, "")).expect("out of reach: fine");
        // SAFETY: as `a_served_skill_reads_its_key_from_the_environment` — test code, a unique variable, written once.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var("MYCELIUM_TEST_SAME_VALUE_7F", VALUE); }
        let e = check_confined_secrets(&a, &units(&placement, "env = [\"MYCELIUM_TEST_SAME_VALUE_7F\"]\n")).unwrap_err().to_string();
        assert!(e.contains("holds an [[egress_credentials]] binding's value") && !e.contains(VALUE), "{e}");
        // #644's round-2 review, N7: a value inside a longer string, and a value file with a second hard link
        // (its other name may sit in a readable directory), each refuse the start while any activation is
        // confined. Seen failing first: both started.
        // SAFETY: as above — test code, a unique variable, written once.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var("MYCELIUM_TEST_HOLDS_VALUE_7F", format!("prefix-{VALUE}-suffix")); }
        let e = check_confined_secrets(&a, &units(&placement, "env = [\"MYCELIUM_TEST_HOLDS_VALUE_7F\"]\n")).unwrap_err().to_string();
        assert!(e.contains("holds an [[egress_credentials]] binding's value") && !e.contains(VALUE), "{e}");
        let other = scratch("secret-link");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::hard_link(&file, other.join("alias")).unwrap();
        let e = check_confined_secrets(&a, &units(&placement, "")).unwrap_err().to_string();
        assert!(e.contains("`model`") && e.contains("hard link"), "{e}");
        std::fs::remove_file(other.join("alias")).unwrap();
        check_confined_secrets(&a, &units(&placement, "")).expect("one link again: fine");
        // Stem::start runs the same check.
        let e = Stem::start(Arc::clone(&a), &units(&placement, &format!("read = [\"{}\"]\n", dir.display())), StemOptions::default()).err().expect("refused").to_string();
        assert!(e.contains("value_file"), "{e}");
        a.shutdown().await;
    }

    /// #644's round-3 review, finding 1: a `value_file` that is a symlink is judged by the file it points to — one
    /// with a second hard link refuses the start like a hard-linked file named directly. Seen failing first: the
    /// link's own count (1) was read, and the start went through.
    #[test]
    fn a_symlinked_value_file_is_judged_by_the_file_it_names() {
        let dir = scratch("symlinked-value");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("secret");
        std::fs::write(&target, "sk-symlinked-7f").unwrap();
        std::fs::hard_link(&target, dir.join("second-name")).unwrap();
        let link = dir.join("value-file");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let cfg = mycelium::GossipConfig {
            egress_credentials: vec![mycelium::CredentialBinding {
                name: "model".into(), host: "api.example.com".into(), port: None, path_prefix: None,
                form: mycelium::CredentialForm::Bearer, header: None, username: None,
                value_env: None, value_file: Some(link.clone()), allow_plaintext: false,
            }],
            ..Default::default()
        };
        let a = GossipAgent::new(NodeId::new("127.0.0.1", alloc_port()).unwrap(), cfg);
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "[hosts]\nkinds=[\"blob\"]\naccept_unsigned=true\nplacement_root=\"{}\"\n[[activation]]\nns=\"data\"\nname=\"pack\"\ncommand=[\"true\"]\n[activation.confine]\n",
            scratch("symlinked-placement").display()
        ))
        .unwrap();
        let e = check_confined_secrets(&a, &units).unwrap_err().to_string();
        assert!(e.contains("`model`") && e.contains("hard link"), "{e}");
        std::fs::remove_file(dir.join("second-name")).unwrap();
        check_confined_secrets(&a, &units).expect("one name again: fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// R1's runtime half: a node started from a unit file declares what the file says — the
    /// advertisement, the requirement and the group are visible on a second node — and the
    /// declarations evaporate when the stem is dropped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_stem_declares_from_its_file_and_a_peer_sees_it() {
        let a = agent(alloc_port(), None).await;
        let b = agent(alloc_port(), Some(a.node_id().to_socket_addr().port())).await;
        let units = NodeCapabilityConfig::from_toml_str(
            "principal=\"planner\"\n\
             [[capability]]\nns=\"plan\"\nname=\"route\"\nttl_secs=30\n[capability.attrs]\nregion=\"north\"\n\
             [[requirement]]\nns=\"llm\"\nname=\"inference\"\n\
             [[group]]\nname=\"routers\"\n[group.filter]\nns=\"plan\"\nname=\"route\"\n[[group.provides]]\nns=\"plan\"\nname=\"routing\"\n",
        )
        .unwrap();
        let stem = Stem::start(Arc::clone(&a), &units, StemOptions::default()).unwrap();
        assert_eq!(stem.hosted_count(), 0, "no [hosts] table, no provisioner");

        let seen = wait_until(30, || {
            !b.capabilities().resolve(&CapFilter::new("plan", "route")).is_empty()
                && b.kv().get(&format!("req/{}/llm/inference", a.node_id().as_str())).is_some()
                && b.kv().get("cap-group/routers").is_some()
        })
        .await;
        assert!(seen, "the peer sees the advertisement, the requirement and the group");
        let cap = b.capabilities().resolve(&CapFilter::new("plan", "route")).remove(0).1;
        assert_eq!(cap.attributes.get("region"), Some(&mycelium::CapValue::Text("north".into())));

        drop(stem);
        let gone = wait_until(30, || b.capabilities().resolve(&CapFilter::new("plan", "route")).is_empty()).await;
        assert!(gone, "declarations evaporate with the stem");
        a.shutdown().await;
        b.shutdown().await;
    }

    #[test]
    fn presence_without_hosts_is_refused_and_a_bad_publisher_key_is_named() {
        assert!(parse_publisher("trusted_publishers", "ed25519:zz").unwrap_err().0.contains("64 hex"));
        assert!(parse_publisher("trusted_publishers", "rsa:aa").unwrap_err().0.contains("ed25519:<64 hex>"));
        let ok = parse_publisher("trusted_publishers", &format!("ed25519:{}", "ab".repeat(32))).unwrap();
        assert_eq!(ok, [0xab; 32]);
    }

    /// R2: the stem fleet. Three identical stem nodes from one unit file, a librarian holding
    /// one artifact, a presence floor of two. The fleet converges to two providers of a
    /// capability nobody deployed; kill one host and the third stem brings it back; and the
    /// offline check over the same files agrees with what the fleet did — the first
    /// declared-versus-observed comparison, run locally with no consumer.
    /// A library of blobs with a librarian on `seed`, and a unit file over it: (seed, lib dir,
    /// units text prefix) — shared by the two activation tests.
    async fn blob_library(tag: &str, blobs: &[(&str, &[u8])]) -> (Arc<GossipAgent>, std::path::PathBuf, String, Vec<crate::artifact::ArtifactId>) {
        let lib_dir = scratch(tag);
        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let key = SigningKey::from_bytes(&[9u8; 32]);
        let mut entries = Vec::new();
        let mut ids = Vec::new();
        for (name, bytes) in blobs {
            let id = lib.store(bytes).unwrap();
            ids.push(id);
            entries.push(
                InstallableEntry::new(Capability::new("data", *name), id)
                    .with_kind(crate::artifact::ArtifactKind::Blob)
                    .with_cost(bytes.len() as u64, 1)
                    .signed_by(&key),
            );
        }
        Manifest::from_entries(entries).save(&lib_dir.join(MANIFEST_FILE)).unwrap();
        let seed = agent(alloc_port(), None).await;
        let _librarian = spawn_librarian(
            Arc::clone(&seed),
            Arc::clone(&lib) as Arc<_>,
            LibrarianConfig {
                manifest_path: lib_dir.join(MANIFEST_FILE),
                publisher:     key.verifying_key().to_bytes(),
                sync_interval: Duration::from_millis(200),
                manifest_source: None,
            },
        );
        std::mem::forget(_librarian);
        let publisher_hex: String = key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let place = lib_dir.join("placed");
        let head = format!(
            "principal=\"stem\"\n[hosts]\nkinds=[\"blob\"]\ntrusted_publishers=[\"ed25519:{publisher_hex}\"]\nplacement_root=\"{}\"\n",
            place.display()
        );
        (seed, lib_dir, head, ids)
    }

    fn stem_opts(lib_dir: &std::path::Path) -> StemOptions {
        StemOptions {
            source: StemSource::Library(lib_dir.to_path_buf()),
            tick: Duration::from_millis(300),
            self_elect_p: 1.0,
            declare_interval: Duration::from_secs(2),
            reprobe_every: Duration::from_millis(500),
            trace: None,
            stage_dir: None,
            max_stage_bytes: crate::DEFAULT_MAX_STAGE_BYTES,
        }
    }

    /// X2 (the model demos' gap): a unit's `[[activation]]` hands a placed blob to the local
    /// runtime — here a shell command standing in for `ollama create` — and gates the capability
    /// on its probe. When the probe starts failing, the install is withdrawn and the next round
    /// reinstalls and re-activates. Seen failing first: with the sections ignored, the activation
    /// marker never appeared.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability() {
        let (seed, lib_dir, head, ids) = blob_library("act", &[("pack", b"a data pack, standing in for weights")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[presence]]\nns=\"data\"\nname=\"pack\"\nmin_providers=1\n\
             [[activation]]\nns=\"data\"\nname=\"pack\"\ncommand=[\"sh\",\"-c\",\"cp {{path}} {{path}}.active\"]\nprobe=[\"test\",\"-f\",\"{{path}}.active\"]\n"
        )).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let stem = Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)).unwrap();
        let active = lib_dir.join("placed").join(format!("{}.active", ids[0].to_hex()));
        assert!(wait_until(60, || stem.hosted_count() == 1 && active.exists()).await, "placed, activated, live");
        assert_eq!(std::fs::read(&active).unwrap(), b"a data pack, standing in for weights");

        // The runtime loses it: the probe fails, the install is withdrawn, and the floor brings it
        // back — re-placed and re-activated.
        std::fs::remove_file(&active).unwrap();
        assert!(wait_until(30, || stem.hosted_count() == 0).await, "a failing probe withdraws the install");
        assert!(wait_until(60, || stem.hosted_count() == 1 && active.exists()).await, "restart ≡ provisioning: re-activated");

        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// Row M, end to end: a stem whose `[[activation]]` names a profile runs it confined — `{dir}`
    /// expands to the real placement root, so the command writes its marker there — and says so; the
    /// same activation under a backend this platform lacks is refused at every attempt, nothing is
    /// hosted, and the stem says that too. Written after the hook; planted — with the hook ignoring the
    /// profile, the foreign-backend half hosts the blob and fails.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_confined_activation_runs_in_a_stem_and_the_stem_says_which_applies() {
        use crate::activation_sandbox::Confinement;
        if !crate::activation_sandbox::native_backend_or_skip("a_confined_activation_runs_in_a_stem_and_the_stem_says_which_applies") {
            return;
        }
        let (seed, lib_dir, head, ids) = blob_library("confined", &[("pack", b"bytes for a confined runtime")]).await;
        std::fs::create_dir_all(lib_dir.join("placed")).unwrap();
        let unit = |backend: &str| NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[presence]]\nns=\"data\"\nname=\"pack\"\nmin_providers=1\n\
             [[activation]]\nns=\"data\"\nname=\"pack\"\ncommand=[\"sh\",\"-c\",\"cp {{path}} {{path}}.active\"]\nprobe=[\"test\",\"-f\",\"{{path}}.active\"]\n\
             [activation.confine]\nbackend=\"{backend}\"\nwrite=[\"{{dir}}\"]\n"
        )).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let stem = Stem::start(Arc::clone(&node), &unit("auto"), stem_opts(&lib_dir)).unwrap();
        assert!(matches!(stem.activation_confinement(), [(at, Confinement::Confined { .. })] if at == "data/pack"), "{:?}", stem.activation_confinement());
        let active = lib_dir.join("placed").join(format!("{}.active", ids[0].to_hex()));
        assert!(wait_until(60, || stem.hosted_count() == 1 && active.exists()).await, "placed, activated under the profile, live");
        stem.stop().await;
        node.shutdown().await;

        // The same unit forced onto the backend this platform lacks: refused, never run, never hosted.
        std::fs::remove_dir_all(lib_dir.join("placed")).unwrap();
        let foreign = if cfg!(target_os = "macos") { "landlock" } else { "seatbelt" };
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let sink = Arc::new(mycelium::decision::DecisionSink::default());
        let opts = StemOptions { trace: Some(Arc::clone(&sink)), ..stem_opts(&lib_dir) };
        let stem = Stem::start(Arc::clone(&node), &unit(foreign), opts).unwrap();
        assert!(matches!(stem.activation_confinement(), [(_, Confinement::Refused { reason })] if reason.contains(foreign)), "{:?}", stem.activation_confinement());
        // Review finding 9 (#636): observe the refusal itself, not a quiet interval.
        let refused = || sink.snapshot().iter().any(|r| r.rule == "prov.activation" && r.reason == "confinement_unavailable");
        assert!(wait_until(60, refused).await, "the provisioner tried, and the refusal is recorded by name");
        assert_eq!(stem.hosted_count(), 0, "a refused activation hosts nothing");
        assert!(!active.exists(), "and its command never ran");
        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// X2 (`reheal_deploy`'s bridge, declared): `[[serve]]` registers the routable skill while
    /// the install it waits for is live here, and retracts it when the install goes — so a router
    /// fails over. The backend is never called here; the test is about the skill's lifecycle.
    /// Seen failing first: with the serve task not spawned, `llm/pack-model` never resolved.
    #[cfg(feature = "llm")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_served_skill_follows_the_install_it_waits_for() {
        let (seed, lib_dir, head, ids) = blob_library("serve", &[("pack", b"weights standing in")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[presence]]\nns=\"data\"\nname=\"pack\"\nmin_providers=1\n\
             [[activation]]\nns=\"data\"\nname=\"pack\"\ncommand=[\"sh\",\"-c\",\"test -f {{dir}}/allow && cp {{path}} {{path}}.active\"]\nprobe=[\"test\",\"-f\",\"{{path}}.active\"]\n\
             [[serve]]\nname=\"pack-model\"\nendpoint=\"http://127.0.0.1:9/v1\"\nmodel=\"pack\"\n[serve.while_live]\nns=\"data\"\nname=\"pack\"\n"
        )).unwrap();
        // The activation runs only while `allow` exists, so the test decides when a reinstall may land.
        let allow = lib_dir.join("placed").join("allow");
        std::fs::create_dir_all(lib_dir.join("placed")).unwrap();
        std::fs::write(&allow, b"").unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let stem = Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)).unwrap();
        let skill = CapFilter::new("llm", "pack-model");
        let served_here = |a: &Arc<GossipAgent>| a.capabilities().resolve(&skill).iter().any(|(n, _)| n == a.node_id());
        assert!(wait_until(60, || stem.hosted_count() == 1 && served_here(&node)).await, "the skill is served once the install is live");

        // The install goes (its probe fails and it is withdrawn): the skill goes with it.
        let active = lib_dir.join("placed").join(format!("{}.active", ids[0].to_hex()));
        std::fs::remove_file(&allow).unwrap();
        std::fs::remove_file(&active).unwrap();
        assert!(wait_until(30, || !served_here(&node)).await, "the skill is retracted when the install goes");
        // And it comes back with the reinstall, once a reinstall may land.
        std::fs::write(&allow, b"").unwrap();
        assert!(wait_until(60, || served_here(&node)).await, "the skill returns with the reinstall");

        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// Zero-gaps Z2 (D2): a `[[serve]]` key comes from the environment by name, resolved **at
    /// start**: a literal stays a literal, a named variable is read once, and an unset variable is
    /// a refusal that names it. Written before `api_key_env` existed and seen failing to compile.
    #[test]
    fn a_served_skill_reads_its_key_from_the_environment() {
        let decl = |literal: Option<&str>, env: Option<&str>| mycelium::ServeDecl {
            ns: "llm".into(), name: "m".into(), endpoint: "http://127.0.0.1:9/v1".into(), model: "x".into(),
            while_live: None, api_key: literal.map(str::to_string), api_key_env: env.map(str::to_string),
            credential: None, max_tokens: None, temperature: None,
        };
        // The crate is `#![deny(unsafe_code)]` (post-360 P3); this env write is its only `unsafe`.
        // SAFETY — the hazard, stated honestly: edition 2024 marks `set_var` unsafe because a
        // concurrent read of the environment from ANY thread (a C `getenv` in another test, e.g.
        // inside name resolution) can observe it mid-update, and no lock here prevents that; Rust's
        // own `std::env` calls are serialised by std's internal lock, so the residual race is a libc
        // `getenv` on another test thread. Tolerated because this is test code (the worst outcome is
        // a crashed test run), the variable is unique to this test and written once.
        #[allow(unsafe_code)]
        unsafe { std::env::set_var("MYCELIUM_TEST_SERVE_KEY_SET_7F", "sk-test-7f"); }
        assert_eq!(resolve_serve_key(&decl(None, None)).unwrap(), None, "no key: none sent");
        assert_eq!(resolve_serve_key(&decl(Some("sk-lit"), None)).unwrap().as_deref(), Some("sk-lit"));
        assert_eq!(resolve_serve_key(&decl(None, Some("MYCELIUM_TEST_SERVE_KEY_SET_7F"))).unwrap().as_deref(), Some("sk-test-7f"));
        let e = resolve_serve_key(&decl(None, Some("MYCELIUM_TEST_SERVE_KEY_UNSET_7F"))).unwrap_err();
        // Named by its section and field, never by the field's contents (row L's review, finding 2: a key
        // pasted into `api_key_env` was echoed into a start-time log line).
        assert!(e.to_string().contains("llm/m") && e.to_string().contains("api_key_env"), "{e}");
        assert!(!e.to_string().contains("MYCELIUM_TEST_SERVE_KEY_UNSET_7F"), "{e}");
        let pasted = "sk-live-pasted-into-the-name-field-7f";
        let e = resolve_serve_key(&decl(None, Some(pasted))).unwrap_err();
        assert!(!e.to_string().contains(pasted) && !format!("{e:?}").contains(pasted), "{e}");
    }

    /// Post-360 plan row L, end to end: a stem whose `[[serve]]` names a credential binding serves a
    /// skill whose calls reach the endpoint carrying the binding's value — which the unit file, the
    /// stem and the backend never held (the stem holds the placeholder; the node's gate attaches it).
    #[cfg(feature = "llm")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_served_skill_sends_its_binding_value_which_the_stem_never_held() {
        const SECRET: &str = "sk-row-L-serve-e2e-91c";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint_port = listener.local_addr().unwrap().port();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let log = Arc::clone(&log);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    let mut buf = vec![0u8; 16384];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_string());
                    let body = r#"{"choices":[{"message":{"content":"served"}}],"usage":{"total_tokens":1},"model":"pack"}"#;
                    let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        let secret_file = std::env::temp_dir().join(format!("mycelium-row-l-serve-{}", std::process::id()));
        std::fs::write(&secret_file, SECRET).unwrap();

        let (seed, lib_dir, head, _ids) = blob_library("credential-e2e", &[("pack", b"weights standing in")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[serve]]\nname=\"keyed\"\nendpoint=\"http://localhost:{endpoint_port}/v1\"\nmodel=\"pack\"\ncredential=\"model\"\n"
        )).unwrap();
        assert!(!format!("{units:?}").contains(SECRET));
        let port = alloc_port();
        let cfg = mycelium::GossipConfig {
            bind_port: port,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", seed.node_id().to_socket_addr().port()).unwrap()],
            health_check_interval_secs: 2,
            egress_credentials: vec![mycelium::CredentialBinding {
                name: "model".into(), host: "localhost".into(), path_prefix: Some("/v1".into()),
                form: mycelium::CredentialForm::Bearer, header: None, username: None,
                value_env: None, value_file: Some(secret_file.clone()), allow_plaintext: false, port: Some(endpoint_port),
            }],
            ..Default::default()
        };
        let node = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        node.start().await.expect("agent start");
        let stem = Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)).unwrap();
        let skill = CapFilter::new("llm", "keyed");
        assert!(wait_until(30, || node.capabilities().resolve(&skill).iter().any(|(n, _)| n == node.node_id())).await, "the skill is served");
        let out = node.llm().call_prompt_skill("llm", "keyed", "hello", Default::default(), Duration::from_secs(10)).await.expect("the call answers");
        assert_eq!(out, "served");
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().any(|r| r.contains(&format!("Bearer {SECRET}"))), "the endpoint received the binding's value: {seen:?}");
        assert!(seen.iter().all(|r| !r.contains("mycelium-credential:")), "and never the placeholder");
        assert_eq!(node.egress_credentials().unwrap().uses()[0].attached as usize, seen.len());

        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
        let _ = std::fs::remove_file(&secret_file);
    }

    /// Row L's review, finding 9: a `[[serve]]` whose endpoint its binding does not authorise (another
    /// host, port or path) **refuses the start**, naming the section and the binding — not a skill
    /// served whose every call the gate refuses.
    #[cfg(feature = "llm")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_serve_whose_endpoint_its_binding_does_not_authorise_refuses_the_start() {
        let secret_file = std::env::temp_dir().join(format!("mycelium-row-l-serve-misdirected-{}", std::process::id()));
        std::fs::write(&secret_file, "sk-row-L-misdirected").unwrap();
        let (seed, lib_dir, head, _ids) = blob_library("credential-misdirected", &[("pack", b"weights standing in")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[serve]]\nname=\"keyed\"\nendpoint=\"https://elsewhere.example/v1\"\nmodel=\"pack\"\ncredential=\"model\"\n"
        )).unwrap();
        let port = alloc_port();
        let cfg = mycelium::GossipConfig {
            bind_port: port,
            bootstrap_peers: vec![NodeId::new("127.0.0.1", seed.node_id().to_socket_addr().port()).unwrap()],
            health_check_interval_secs: 2,
            egress_credentials: vec![mycelium::CredentialBinding {
                name: "model".into(), host: "api.example.com".into(), path_prefix: None,
                form: mycelium::CredentialForm::Bearer, header: None, username: None,
                value_env: None, value_file: Some(secret_file.clone()), allow_plaintext: false, port: None,
            }],
            ..Default::default()
        };
        let node = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        node.start().await.expect("agent start");
        let err = match Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)) {
            Ok(stem) => { stem.stop().await; panic!("a serve whose endpoint its binding does not authorise must refuse the start") }
            Err(e) => e.to_string(),
        };
        assert!(err.contains("llm/keyed") && err.contains("`model`") && err.contains("elsewhere.example"), "{err}");
        node.shutdown().await;
        seed.shutdown().await;
        let _ = std::fs::remove_file(&secret_file);
    }

    /// Post-360 plan row L: a `[[serve]]`'s `credential` resolves to the binding's placeholder (the
    /// stem never holds the value), and a binding the node does not have **refuses the start**, naming
    /// it. Seen failing first: the stem started and served with the placeholder as its bearer.
    #[cfg(feature = "llm")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_serve_naming_a_binding_the_node_lacks_refuses_the_start() {
        let d = mycelium::ServeDecl {
            ns: "llm".into(), name: "m".into(), endpoint: "https://api.example/v1".into(), model: "x".into(),
            while_live: None, api_key: None, api_key_env: None, credential: Some("openai".into()),
            max_tokens: None, temperature: None,
        };
        assert_eq!(resolve_serve_key(&d).unwrap(), Some(mycelium::credential_placeholder("openai")));
        let (seed, lib_dir, head, _ids) = blob_library("credential", &[("pack", b"weights standing in")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[serve]]\nname=\"keyed\"\nendpoint=\"http://127.0.0.1:9/v1\"\nmodel=\"pack\"\ncredential=\"openai\"\n"
        )).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let err = match Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)) {
            Ok(stem) => { stem.stop().await; panic!("a serve naming a binding the node lacks must refuse the start") }
            Err(e) => e.to_string(),
        };
        assert!(err.contains("credential `openai`") && err.contains("egress_credentials"), "{err}");
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// Zero-gaps Z2 (D2): a stem whose `[[serve]]` names an unset variable **refuses to start**,
    /// naming the variable — never a skill served with no key, never a silent `"none"`. Seen failing
    /// first: the unknown field was ignored and the stem started.
    #[cfg(feature = "llm")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_serve_whose_key_variable_is_unset_refuses_the_start() {
        let (seed, lib_dir, head, _ids) = blob_library("keyenv", &[("pack", b"weights standing in")]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[serve]]\nname=\"keyed\"\nendpoint=\"http://127.0.0.1:9/v1\"\nmodel=\"pack\"\napi_key_env=\"MYCELIUM_TEST_SERVE_KEY_UNSET_7F\"\n"
        )).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let err = match Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)) {
            Ok(stem) => { stem.stop().await; panic!("a serve whose key variable is unset must refuse the start") }
            Err(e) => e.to_string(),
        };
        assert!(err.contains("llm/keyed") && err.contains("api_key_env"), "{err}");
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// Zero-gaps Z3 (D3): a **mesh-only** stem (no `--library`) installs a blob past the frame cap —
    /// the model demos' shape, which until now every model host avoided with `--library /lib`. Seen
    /// failing first: the whole-object mesh pull could not carry it and nothing was ever hosted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mesh_only_stem_installs_a_blob_past_the_frame_cap() {
        let big: Vec<u8> = (0..12u32 * 1024 * 1024).map(|i| (i.wrapping_mul(2654435761) >> 11) as u8).collect();
        let (seed, lib_dir, head, _ids) = blob_library("bigmesh", &[("weights", &big)]).await;
        let units = NodeCapabilityConfig::from_toml_str(&format!("{head}[[presence]]\nns=\"data\"\nname=\"weights\"\nmin_providers=1\n")).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let opts = StemOptions { source: StemSource::Mesh { timeout: Duration::from_secs(5) }, ..stem_opts(&lib_dir) };
        let stem = Stem::start(Arc::clone(&node), &units, opts).unwrap();
        assert!(wait_until(120, || stem.hosted_count() == 1).await, "the 12 MiB blob installs over the mesh");
        let placed = lib_dir.join("placed");
        let on_disk = std::fs::read_dir(&placed).unwrap().flatten().any(|e| e.metadata().map(|m| m.len() == big.len() as u64).unwrap_or(false));
        assert!(on_disk, "the placed blob is on disk at its full size");
        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// `resolve_artifact_refs`: a profile naming its weights as `artifact:<hex>` is rendered with
    /// the placed path, and activation waits for the weights — ordering by retry, no resolver.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_activation_resolves_artifact_references_to_placed_paths() {
        let weights: &[u8] = b"the weights";
        let weights_hex = crate::artifact::ArtifactId::of(weights).to_hex();
        let profile = format!("FROM artifact:{weights_hex}\nSYSTEM be kind\n");
        let (seed, lib_dir, head, _ids) =
            blob_library("refs", &[("weights", weights), ("profile", profile.as_bytes())]).await;
        let out = lib_dir.join("model.out");
        let units = NodeCapabilityConfig::from_toml_str(&format!(
            "{head}[[presence]]\nns=\"data\"\nname=\"weights\"\nmin_providers=1\n[[presence]]\nns=\"data\"\nname=\"profile\"\nmin_providers=1\n\
             [[activation]]\nns=\"data\"\nname=\"profile\"\nresolve_artifact_refs=true\ncommand=[\"cp\",\"{{rendered}}\",\"{}\"]\n",
            out.display()
        )).unwrap();
        let node = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let stem = Stem::start(Arc::clone(&node), &units, stem_opts(&lib_dir)).unwrap();
        assert!(wait_until(60, || stem.hosted_count() == 2 && out.exists()).await, "both placed, the profile activated");
        let text = std::fs::read_to_string(&out).unwrap();
        let placed = lib_dir.join("placed").join(&weights_hex);
        assert!(text.contains(&format!("FROM {}", placed.display())), "the reference became the placed path: {text}");
        assert!(!text.contains("artifact:"), "{text}");

        stem.stop().await;
        node.shutdown().await;
        seed.shutdown().await;
    }

    /// X2 (the catalog demo's phase 6, as stems): a hosting stem on the mesh path re-serves what
    /// it pulled and verified, advertised as a cache holder once it holds something — so when the
    /// librarian is gone, a late joiner installs from the peer. Seen failing first: with no
    /// re-serving, the late stem found no holder and never installed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_hosting_stem_re_serves_its_verified_cache_to_a_late_joiner() {
        let lib_dir = scratch("cache-lib");
        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let key = SigningKey::from_bytes(&[8u8; 32]);
        let artifact = lib.store(ECHO_COMPONENT).unwrap();
        let entry = InstallableEntry::new(Capability::new("route", "optimize"), artifact)
            .with_cost(ECHO_COMPONENT.len() as u64, 1)
            .signed_by(&key);
        Manifest::from_entries(vec![entry]).save(&lib_dir.join(MANIFEST_FILE)).unwrap();
        let seed = agent(alloc_port(), None).await;
        let librarian = spawn_librarian(
            Arc::clone(&seed),
            Arc::clone(&lib) as Arc<_>,
            LibrarianConfig {
                manifest_path: lib_dir.join(MANIFEST_FILE),
                publisher:     key.verifying_key().to_bytes(),
                sync_interval: Duration::from_millis(200),
                manifest_source: None,
            },
        );
        let publisher_hex: String = key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let unit = |floor: u32| NodeCapabilityConfig::from_toml_str(&format!(
            "principal=\"stem\"\n[hosts]\nkinds=[\"wasm-component\"]\ntrusted_publishers=[\"ed25519:{publisher_hex}\"]\n\
             [[presence]]\nns=\"route\"\nname=\"optimize\"\nmin_providers={floor}\n"
        )).unwrap();
        let opts = StemOptions {
            source: StemSource::Mesh { timeout: Duration::from_secs(5) },
            tick: Duration::from_millis(300),
            self_elect_p: 1.0,
            declare_interval: Duration::from_secs(2),
            reprobe_every: Duration::from_secs(1),
            trace: None,
            stage_dir: None,
            max_stage_bytes: crate::DEFAULT_MAX_STAGE_BYTES,
        };

        // The installer: pulls from the librarian, hosts, and — once its cache holds the bytes —
        // advertises itself as a cache holder.
        let installer = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let installer_stem = Stem::start(Arc::clone(&installer), &unit(1), opts.clone()).unwrap();
        assert!(wait_until(60, || installer_stem.hosted_count() == 1).await, "the installer hosts from the librarian");
        let holders = |a: &Arc<GossipAgent>| a.capabilities().resolve(&librarian_filter()).len();
        assert!(wait_until(30, || holders(&installer) == 2).await, "two holders: the librarian and the installer's cache");

        // The librarian dies with its node.
        drop(librarian);
        seed.shutdown().await;

        // A late joiner, bootstrapping from the installer, wants a second provider: the only
        // holder left is the installer's cache.
        let late = agent(alloc_port(), Some(installer.node_id().to_socket_addr().port())).await;
        let late_stem = Stem::start(Arc::clone(&late), &unit(2), opts.clone()).unwrap();
        assert!(wait_until(90, || late_stem.hosted_count() == 1).await, "the late joiner installed from the peer's cache");
        let reply = late.service()
            .rpc_call(late.node_id().clone(), crate::runtime::cap_invoke_kind("route", "optimize"),
                b"late".to_vec(), Duration::from_secs(5))
            .await.expect("the late joiner serves");
        assert_eq!(reply.as_ref(), b"late");

        late_stem.stop().await;
        installer_stem.stop().await;
        late.shutdown().await;
        installer.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 6)]
    async fn the_stem_fleet_fills_a_presence_floor_and_reheals() {
        // ── the library and its librarian ───────────────────────────────────────
        let lib_dir = scratch("fleet-lib");
        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let artifact = lib.store(ECHO_COMPONENT).unwrap();
        let entry = InstallableEntry::new(Capability::new("route", "optimize"), artifact)
            .with_cost(ECHO_COMPONENT.len() as u64, 1)
            .signed_by(&key);
        Manifest::from_entries(vec![entry]).save(&lib_dir.join(MANIFEST_FILE)).unwrap();
        let seed = agent(alloc_port(), None).await;
        let _librarian = spawn_librarian(
            Arc::clone(&seed),
            Arc::clone(&lib) as Arc<_>,
            LibrarianConfig {
                manifest_path: lib_dir.join(MANIFEST_FILE),
                publisher:     key.verifying_key().to_bytes(),
                sync_interval: Duration::from_millis(200),
                manifest_source: None,
            },
        );
        let publisher_hex: String = key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();

        // ── one unit file, three stems ──────────────────────────────────────────
        let units_toml = format!(
            "principal=\"stem\"\n[hosts]\nkinds=[\"wasm-component\"]\ntrusted_publishers=[\"ed25519:{publisher_hex}\"]\n\
             [[presence]]\nns=\"route\"\nname=\"optimize\"\nmin_providers=2\nmax_providers=2\n"
        );
        let units = NodeCapabilityConfig::from_toml_str(&units_toml).unwrap();
        let opts = StemOptions {
            source: StemSource::Mesh { timeout: Duration::from_secs(5) },
            tick: Duration::from_millis(300),
            self_elect_p: 1.0,
            declare_interval: Duration::from_secs(2),
            reprobe_every: Duration::from_secs(1),
            trace: None,
            stage_dir: None,
            max_stage_bytes: crate::DEFAULT_MAX_STAGE_BYTES,
        };
        // Three stems, started so that the fleet is **deterministic**: with `self_elect_p: 1.0`
        // (no herd damping) three stems started together can all elect in one round — three hosts
        // over a band of two, trimmed back by the ranked shed (before #547 all three shed at once and
        // the band oscillated, seen under a loaded run on 2026-10-03). The staggered start keeps
        // which two host predictable: the first two come up and host, then the third stands by.
        let filter = CapFilter::new("route", "optimize");
        let live = |n: &Arc<GossipAgent>| n.capabilities().resolve(&filter).len();
        let hosts_of = |fleet: &Vec<(Arc<GossipAgent>, Stem)>| -> Vec<usize> {
            fleet.iter().enumerate().filter(|(_, (_, s))| s.hosted_count() > 0).map(|(i, _)| i).collect()
        };
        let mut fleet: Vec<(Arc<GossipAgent>, Stem)> = Vec::new();
        for i in 0..2 {
            let a = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
            let s = Stem::start(Arc::clone(&a), &units, opts.clone()).unwrap();
            fleet.push((a, s));
            assert!(wait_until(90, || live(&seed) == i + 1 && hosts_of(&fleet).len() == i + 1).await,
                "stem {i} hosts: {} live, hosting {:?}", live(&seed), hosts_of(&fleet));
        }
        let standby = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
        let standby_stem = Stem::start(Arc::clone(&standby), &units, opts.clone()).unwrap();
        fleet.push((standby, standby_stem));
        // The standby sees the floor met: a few of its ticks pass and it does not install.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(hosts_of(&fleet), vec![0, 1], "the floor is met, the third stem stands by");
        assert_eq!(live(&seed), 2, "the fleet holds the presence floor");
        let hosting = hosts_of(&fleet);

        // ── the declared-versus-observed comparison ─────────────────────────────
        let description = mycelium::wire_check::ArtifactDescription::from_toml_str(
            "kind=\"wasm-component\"\n[provides]\nns=\"route\"\nname=\"optimize\"\n",
        )
        .unwrap();
        let declared: Vec<mycelium::wire_check::Unit> = (0..3)
            .map(|i| mycelium::wire_check::Unit { name: format!("stem-{i}"), config: units.clone() })
            .collect();
        let report = mycelium::wire_check::check(
            &declared,
            &[("route-optimizer".to_string(), description)],
            &mycelium::wire_check::CheckOptions::default(),
        );
        assert_eq!(report.exit_code(), 0, "the declarations say the floor is hostable: {}", report.render_text());
        for (node, _cap) in seed.capabilities().resolve(&filter) {
            let (i, _) = fleet.iter().enumerate().find(|(_, (a, _))| a.node_id() == &node).expect("a provider is one of the stems");
            assert!(report.units[i].hosts_kinds.contains(&"wasm-component".to_string()), "what bound was declared hostable");
        }

        // ── kill one host: the standby brings the floor back ────────────────────
        let victim = hosting[0];
        let (dead_agent, dead_stem) = fleet.remove(victim);
        let dead_id = dead_agent.node_id().clone();
        dead_stem.stop().await;
        dead_agent.shutdown().await;
        // The dead host's advertisement evaporates (its 5 s TTL) and the standby re-provisions
        // within a tick of that — so `live < 2` can hold for less than one poll interval and a
        // count-based wait could miss it (seen twice under a loaded full run, 2026-10-03). The
        // structural conditions: the victim is gone from the seed's providers for good, and the
        // floor is back with two providers neither of which is the victim.
        let providers = |n: &Arc<GossipAgent>| n.capabilities().resolve(&filter).into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        let gone = wait_until(60, || !providers(&seed).contains(&dead_id)).await;
        assert!(gone, "the dead host's advertisement evaporates: {:?}", providers(&seed));
        let rehealed = wait_until(90, || { let p = providers(&seed); p.len() == 2 && !p.contains(&dead_id) }).await;
        assert!(rehealed, "the standby re-provisions: {:?}", providers(&seed));
        assert!(wait_until(30, || hosts_of(&fleet).len() == 2).await, "both remaining stems now host: {:?}", hosts_of(&fleet));

        for (a, s) in fleet {
            s.stop().await;
            a.shutdown().await;
        }
        seed.shutdown().await;
        let _ = std::fs::remove_dir_all(&lib_dir);
    }
}
