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

use mycelium::{CapabilityGroupHandle, CapabilityReg, GossipAgent, NodeCapabilityConfig, RequirementHandle};

use crate::artifact::ArtifactSource;
use crate::catalog::InstallableCatalog;
use crate::host::WasmHost;
use crate::librarian::librarian_filter;
use crate::mesh_source::MeshArtifactSource;
use crate::provisioner::Provisioner;
use crate::resources::SystemResourceProbe;
use crate::runtime::{BlobRuntime, FuelPolicy};
use crate::FsLibrarySource;

/// Where a stem node pulls artifact bytes from.
#[derive(Debug, Clone)]
pub enum StemSource {
    /// Peer pull over the mesh from a librarian discovered through the capability ring
    /// (`artifact/librarian`) — the small-artifact path (bounded by the frame cap). Every
    /// catalogue entry is prefetched on each tick, idempotently, so an install finds its bytes.
    Mesh { timeout: Duration },
    /// A library directory this node can read (a mounted volume, or the librarian's own store):
    /// ranged, so blobs stream to the placement root.
    Library(std::path::PathBuf),
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
}

impl Default for StemOptions {
    fn default() -> Self {
        Self {
            source:           StemSource::Mesh { timeout: Duration::from_secs(5) },
            tick:             Duration::from_millis(500),
            self_elect_p:     0.5,
            declare_interval: Duration::from_secs(5),
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

impl Stem {
    /// Declare everything in `units` on `agent` and, if it hosts, start provisioning.
    /// `agent` must already be started.
    pub fn start(agent: Arc<GossipAgent>, units: &NodeCapabilityConfig, opts: StemOptions) -> Result<Self, StemError> {
        units.validate().map_err(|e| StemError(e.to_string()))?;

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
        let (stop, ticker) = match &units.hosts {
            None => {
                if !units.presence.is_empty() {
                    return Err(StemError("[[presence]] needs a [hosts] table: only a hosting unit can keep providers alive".into()));
                }
                (None, None)
            }
            Some(h) => {
                // D19: the engine counts fuel whenever any budget is declared; who pays is the
                // policy below, decided per entry from its verified signer.
                let metered = h.fuel_per_call.is_some() || h.operator_fuel_per_call.is_some();
                let host = if metered { WasmHost::metered() } else { WasmHost::new() }
                    .map_err(|e| StemError(format!("wasm host: {e}")))?;
                let source: Arc<dyn ArtifactSource + Send + Sync> = match &opts.source {
                    StemSource::Mesh { timeout } => {
                        Arc::new(MeshArtifactSource::resolving(Arc::clone(&agent), librarian_filter(), *timeout))
                    }
                    StemSource::Library(dir) => Arc::new(
                        FsLibrarySource::open(dir).map_err(|e| StemError(format!("library {}: {e}", dir.display())))?,
                    ),
                };
                let mesh: Option<Arc<MeshArtifactSource>> = match &opts.source {
                    StemSource::Mesh { timeout } => {
                        Some(Arc::new(MeshArtifactSource::resolving(Arc::clone(&agent), librarian_filter(), *timeout)))
                    }
                    StemSource::Library(_) => None,
                };
                // One source object must serve both roles for the mesh path, or the provisioner
                // reads a cache the prefetcher never filled.
                let (source, mesh) = match mesh {
                    Some(m) => (Arc::clone(&m) as Arc<dyn ArtifactSource + Send + Sync>, Some(m)),
                    None => (source, None),
                };
                let mut prov = Provisioner::new(
                    Arc::clone(&agent),
                    Arc::new(host),
                    InstallableCatalog::from_kv(&agent.kv()),
                    source,
                    opts.self_elect_p,
                );
                if h.kinds.iter().any(|k| k == "blob") {
                    let root = h.placement_root.clone().unwrap_or_else(|| "artifacts".into());
                    prov.register_runtime(Arc::new(BlobRuntime::new(root)));
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
                let ticker = tokio::spawn(async move {
                    loop {
                        if *stop_rx.borrow_and_update() {
                            break;
                        }
                        prov.refresh_catalog(InstallableCatalog::from_kv(&kv));
                        if let Some(m) = &mesh {
                            // The mesh path carries small artifacts only (the frame cap); pulling
                            // every entry once is bounded and idempotent.
                            let ids: Vec<_> = prov.catalog().entries().iter().map(|e| e.artifact).collect();
                            for id in ids {
                                let _ = m.prefetch(&id).await;
                            }
                        }
                        prov.provision_round();
                        hosted_w.store(prov.hosted_count(), Ordering::Relaxed);
                        tokio::select! {
                            _ = tokio::time::sleep(tick) => {}
                            _ = stop_rx.changed() => {}
                        }
                    }
                    drop(prov);
                });
                (Some(stop_tx), Some(ticker))
            }
        };

        Ok(Self { _caps: caps, _reqs: reqs, _groups: groups, stop, ticker, hosted })
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
        };
        let mut fleet: Vec<(Arc<GossipAgent>, Stem)> = Vec::new();
        for _ in 0..3 {
            let a = agent(alloc_port(), Some(seed.node_id().to_socket_addr().port())).await;
            let s = Stem::start(Arc::clone(&a), &units, opts.clone()).unwrap();
            fleet.push((a, s));
        }
        let filter = CapFilter::new("route", "optimize");
        let live = |n: &Arc<GossipAgent>| n.capabilities().resolve(&filter).len();

        let converged = wait_until(90, || live(&seed) == 2).await;
        assert!(converged, "the fleet converges to the presence floor: {} live", live(&seed));
        // A stem publishes its hosted count on its own tick, after the advertisement the seed
        // already saw — so this is waited for too, not read once.
        let hosts_of = |fleet: &Vec<(Arc<GossipAgent>, Stem)>| -> Vec<usize> {
            fleet.iter().enumerate().filter(|(_, (_, s))| s.hosted_count() > 0).map(|(i, _)| i).collect()
        };
        assert!(wait_until(30, || hosts_of(&fleet).len() == 2).await, "exactly two stems host: {:?}", hosts_of(&fleet));
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
        dead_stem.stop().await;
        dead_agent.shutdown().await;
        let dropped = wait_until(60, || live(&seed) < 2).await;
        assert!(dropped, "the dead host's advertisement evaporates");
        let rehealed = wait_until(90, || live(&seed) == 2).await;
        assert!(rehealed, "the standby re-provisions: {} live", live(&seed));
        assert!(wait_until(30, || hosts_of(&fleet).len() == 2).await, "both remaining stems now host: {:?}", hosts_of(&fleet));

        for (a, s) in fleet {
            s.stop().await;
            a.shutdown().await;
        }
        seed.shutdown().await;
        let _ = std::fs::remove_dir_all(&lib_dir);
    }
}
