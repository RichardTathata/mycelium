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
pub fn resolve_serve_key(d: &mycelium::ServeDecl) -> Result<Option<String>, StemError> {
    if let Some(lit) = &d.api_key {
        return Ok(Some(lit.clone()));
    }
    match &d.api_key_env {
        None => Ok(None),
        Some(var) => std::env::var(var).map(Some).map_err(|_| StemError(format!(
            "[[serve]] {}/{}: api_key_env `{var}` is not set — export it on this host, or use api_key for a literal", d.ns, d.name
        ))),
    }
}

impl Stem {
    /// Declare everything in `units` on `agent` and, if it hosts, start provisioning.
    /// `agent` must already be started.
    pub fn start(agent: Arc<GossipAgent>, units: &NodeCapabilityConfig, opts: StemOptions) -> Result<Self, StemError> {
        let watch: crate::activation::WatchList = Arc::new(std::sync::Mutex::new(Vec::new()));
        units.validate().map_err(|e| StemError(e.to_string()))?;

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
                // D19: the engine counts fuel whenever any budget is declared; who pays is the
                // policy below, decided per entry from its verified signer.
                let metered = h.fuel_per_call.is_some() || h.operator_fuel_per_call.is_some();
                let host = if metered { WasmHost::metered() } else { WasmHost::new() }
                    .map_err(|e| StemError(format!("wasm host: {e}")))?;
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
                            .map_err(|e| StemError(format!("stage {}: {e}", stage_dir.display())))?);
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
                            .map_err(|e| StemError(format!("stage {}: {e}", stage_dir.display())))?);
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
                        blob = blob.with_entry_activation(crate::activation::hook(units.activations.clone(), Arc::clone(&watch)));
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
                    loop {
                        if *stop_rx.borrow_and_update() {
                            break;
                        }
                        prov.refresh_catalog(InstallableCatalog::from_kv(&kv));
                        if let Some(m) = &mesh {
                            // Stage every catalogue entry once (idempotent: a staged id is a size
                            // check, no request) — a component whole, a blob in ranges.
                            let ids: Vec<_> = prov.catalog().entries().iter().map(|e| e.artifact).collect();
                            for id in ids {
                                let _ = m.stage_artifact(&id).await;
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
            Some(crate::serve::spawn(Arc::clone(&agent), resolved, opts.tick))
        };
        #[cfg(not(feature = "llm"))]
        let serve: Option<tokio::task::JoinHandle<()>> = None;
        Ok(Self { _caps: caps, _reqs: reqs, _groups: groups, stop, ticker, hosted, _peer_serve: peer_serve, reprobe, serve })
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

#[cfg(test)]
mod tests {
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
            max_tokens: None, temperature: None,
        };
        #[allow(unused_unsafe)]
        unsafe { std::env::set_var("MYCELIUM_TEST_SERVE_KEY_SET_7F", "sk-test-7f"); }
        assert_eq!(resolve_serve_key(&decl(None, None)).unwrap(), None, "no key: none sent");
        assert_eq!(resolve_serve_key(&decl(Some("sk-lit"), None)).unwrap().as_deref(), Some("sk-lit"));
        assert_eq!(resolve_serve_key(&decl(None, Some("MYCELIUM_TEST_SERVE_KEY_SET_7F"))).unwrap().as_deref(), Some("sk-test-7f"));
        let e = resolve_serve_key(&decl(None, Some("MYCELIUM_TEST_SERVE_KEY_UNSET_7F"))).unwrap_err();
        assert!(e.to_string().contains("MYCELIUM_TEST_SERVE_KEY_UNSET_7F") && e.to_string().contains("api_key_env"), "{e}");
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
        assert!(err.contains("MYCELIUM_TEST_SERVE_KEY_UNSET_7F") && err.contains("api_key_env"), "{err}");
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
