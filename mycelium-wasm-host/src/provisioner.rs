//! The **provisioner** — the app-layer loop that *closes* the autonomic loop (M15 item 4):
//! watch demand → resolve unmet requirements against the installable catalog → dispatch to the
//! matching [`ArtifactRuntime`] (pull + verify + install) → advertise, so demand is relieved.
//!
//! **Core Principle 1 (no coordinator).** This is a regular agent built on Mycelium's public API,
//! **not** a substrate mechanism. The library never auto-provisions; the agency to pull-and-run is
//! the node's own local choice. No coordinator assigns provisioning duty — every node runs its own
//! provisioner, each independently observes demand and **self-elects** (probabilistically, to damp
//! the thundering herd; any over-provisioning self-corrects when a future governor sheds providers
//! over `max`). This generalises `demand.rs`'s "the library never auto-advertises" stance.
//!
//! **Kind dispatch (`docs/design/artifact-library.md` §4).** The convergence loops here are
//! kind-agnostic — they reason about capabilities, demand, and provider counts. *How* an artifact
//! becomes live is the registered [`ArtifactRuntime`]'s business: `WasmComponent` instantiates in
//! the sandboxed host, `Blob` places bytes for a node-local runtime. A node without a runtime for
//! an entry's kind (or whose install budget the entry exceeds) silently never self-elects for it —
//! **eligibility is node-local truth** — and a tripwire counter records the skip (detection, not
//! prevention). Installs run as **background tasks against an `Installing` reservation**, so a
//! multi-GB pull never blocks the provision tick and a round never double-starts an artifact.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycelium::control::ledger::{PublishedRightsHead, RightsLedger};
use mycelium::mandate::PrincipalId;
use mycelium::decision::{DecisionSink, InputSnapshot, OutcomeKind, Provenance};
use mycelium::{CapFilter, CapValue, Capability, CapabilityReg, GossipAgent};

use crate::artifact::{ArtifactId, ArtifactKind, ArtifactSource};
use crate::catalog::{InstallableCatalog, InstallableEntry, ResourceRequirements};
use crate::host::WasmHost;
use crate::resources::{ResourceProbe, SystemResourceProbe};
use crate::runtime::{
    ArtifactRuntime, FuelPolicy, Installed, InvocationLog, InvocationRecord, ProgressFn, RuntimeCtx,
    WasmComponentRuntime,
};

/// How often a provisioned capability re-asserts its `cap/` advertisement.
const ADVERTISE_INTERVAL: Duration = Duration::from_secs(5);

/// One hosted artifact's node-level lifecycle state.
enum HostedState {
    /// A background install task is in flight — the reservation that prevents a later round
    /// double-starting the same artifact. The token identifies *which* task, so a
    /// withdraw-then-reinstall never lets a stale task's completion clobber the newer install.
    /// `reserved` carries the entry's declared requirements so concurrent eligibility checks
    /// see resources already spoken for (two 3 GB models must not both pass a 5 GB-free check).
    Installing { token: u64, reserved: ResourceRequirements },
    /// Installed, advertised, serving. (Its real consumption — placed bytes on disk, activated
    /// memory — is visible to the probe directly, so no virtual accounting is kept.)
    Live(LiveHosted),
}

/// The provisioner's **install rights** (item 4 PR 4c, `docs/design/adaptive-stability.md` §4, §9):
/// a hard bound on how many artifacts this node may have reserved-or-live at once, backed by the
/// rights ledger rather than by a self-imposed budget. Each install consumes one unit of `resource`
/// while it is `Installing` or `Live` — a unit in flight is as consumed as one serving — and an
/// install is refused, and the refusal **recorded**, when the node holds too few. The ledger is the
/// allocator's record; the provisioner never allocates to itself.
///
/// The head (`rights/head/{holder}`) is republished after every ledger event the provisioner
/// causes, signed with `signing_key` when one was given — without a key the head goes out
/// unsigned, and a reader must treat it as a claim without proof.
pub struct InstallRights {
    /// Shared with the tasks that record refusals and publish the head. Lock-order table row 37:
    /// `try_lock` on the admission path (never blocking `provision_round`), `lock().await` in
    /// spawned tasks; never held while `hosted` (row 21) is.
    pub(crate) ledger: Arc<tokio::sync::Mutex<RightsLedger>>,
    pub(crate) holder: PrincipalId,
    pub(crate) resource: String,
    signing_key: Option<ed25519_dalek::SigningKey>,
    /// Tripwire: installs refused because the node held too few units (or the ledger was busy) —
    /// under `EnforceAllocated`, the only profile that enforces a rights-backed bound (ADR §7).
    refusals: AtomicU64,
    /// Tripwire: installs that **would** have been refused under `EnforceAllocated` and were admitted
    /// because the node's profile does not enforce Tier C — the number to watch before opting in.
    would_refuse: AtomicU64,
}

impl InstallRights {
    /// Publish the holder's current head into `rights/head/{holder}`, signed when a key is held.
    fn publish_head(&self, agent: &GossipAgent, ledger: &RightsLedger) {
        use ed25519_dalek::Signer;
        let head = ledger.head(&self.holder);
        let signature = self
            .signing_key
            .as_ref()
            .map(|k| k.sign(&head.canonical_bytes()).to_bytes().to_vec())
            .unwrap_or_default();
        let key = format!("{}{}", mycelium::signal::kv_ns::RIGHTS_HEAD, self.holder);
        let _ = agent.kv().set(key.as_str(), PublishedRightsHead { head, signature }.encode());
    }
}

/// Verify a published head against the holder's Ed25519 public key. `false` for an unsigned
/// head: unsigned means unproven, and a reader that wanted proof did not get it.
pub fn verify_published_head(published: &PublishedRightsHead, key: &[u8; 32]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(vk) = VerifyingKey::from_bytes(key) else { return false };
    let Ok(sig) = Signature::from_slice(&published.signature) else { return false };
    vk.verify(&published.head.canonical_bytes(), &sig).is_ok()
}

/// A capability this node has provisioned and is now hosting: the advertisement registration
/// (dropping it tombstones the `cap/` entry) plus the runtime's lifecycle handle.
struct LiveHosted {
    _cap:      CapabilityReg,
    installed: Box<dyn Installed>,
    /// What this install advertises — read by the shed to find what it can withdraw for a band, from
    /// the install itself rather than a catalogue that may have moved on.
    provides:  Capability,
    /// D20: loaded into the shadow lane — advertised as `{ns}/{name}.shadow`, so no filter for
    /// `{ns}/{name}` ever resolves it and no call for the incumbent ever reaches it.
    shadow:    bool,
}

/// The name a proposed entry is advertised under while it is in the shadow lane (D20): beside the
/// incumbent, resolvable on purpose for comparison, never by the incumbent's filter.
pub fn shadow_name(name: &str) -> String {
    format!("{name}.shadow")
}

/// A **capability-presence invariant** (M14 supervision): keep at least `min_providers` live
/// providers of `filter` across the fleet. A standing desired-state, independent of organic demand
/// — it is what makes provisioning *self-healing*: a provider that dies has its `cap/` entry
/// evaporate, the live count drops below `min_providers`, and a supervising node re-provisions.
/// **Restart ≡ first-time provisioning** — the same resolve-and-pull path serves both.
#[derive(Clone, Debug)]
pub struct SupervisionPolicy {
    pub filter:        CapFilter,
    pub min_providers: usize,
    /// Upper bound (Track 2b elastic sizing). `Some(max)` ⇒ when the live provider count exceeds
    /// `max`, the hosts ranked beyond `max` by the band's rendezvous order **withdraw** (cooperative
    /// self-removal — tombstone its `cap/` + stop serving), the symmetric shed path to bring-up.
    /// `None` ⇒ unbounded.
    pub max_providers: Option<usize>,
}

/// An app-layer provisioner / supervisor. Construct it with the node, a [`WasmHost`], an
/// [`InstallableCatalog`], and an [`ArtifactSource`]; call [`provision_round`](Self::provision_round)
/// on a tick to keep the node provisioning what (a) unmet **demand** and (b) **presence
/// invariants** ([`supervise`](Self::supervise)) call for — both are just "desired state" the node
/// reconciles locally, the same resolve-and-pull path. The
/// [`WasmComponent`](ArtifactKind::WasmComponent) runtime is registered automatically; add more
/// kinds via [`register_runtime`](Self::register_runtime).
pub struct Provisioner {
    agent:        Arc<GossipAgent>,
    catalog:      InstallableCatalog,
    source:       Arc<dyn ArtifactSource + Send + Sync>,
    /// Install dispatch: one runtime per [`ArtifactKind`] this node can host.
    runtimes:     HashMap<ArtifactKind, Arc<dyn ArtifactRuntime>>,
    /// Probability of self-electing to satisfy an unmet requirement on a given round (herd
    /// damping). `1.0` = always (fine for a single provisioner); lower it when many nodes run one.
    self_elect_p: f64,
    /// The decision trace (plan I5): off unless attached. Every record is built from values the
    /// round already produced — nothing is re-evaluated — and recorded with no lock held.
    trace:        Option<Arc<Trace>>,
    /// Rounds run here; the trigger every record of a round names.
    rounds:       u64,
    /// One `prov-shed/{band}` advertisement per band this node supervises with a ceiling, reconciled every
    /// round. Present from the first round, so a new install is never unmarked; retracted only while this node
    /// provides the band without holding an install it could withdraw (code, a group projection) — see
    /// [`marks_band`] (#547's reviews).
    shed_regs:    HashMap<String, CapabilityReg>,
    /// Per band, since when (monotonic ns, through the replay seam) each peer has been seen providing it
    /// **without** a `prov-shed` mark. A peer is counted fixed only after [`FIXED_AFTER`] of that — a mark
    /// and a `cap/` entry are separate keys and can arrive apart (a partition healing), and until then the
    /// safe reading is that it sheds (#547's fifth review).
    unmarked_since: HashMap<String, HashMap<mycelium::NodeId, u64>>,
    /// Capability-presence invariants this node supervises (M14).
    policies:     Vec<SupervisionPolicy>,
    /// If non-empty, only catalog entries with valid provenance from one of these publisher keys
    /// are installed (Ed25519 over the entry — kind, content address, declared-provide). Empty =
    /// accept any (integrity-only).
    trusted_publishers: Vec<[u8; 32]>,
    /// D20: the reviewer keys whose acceptance promotes a proposed entry from the shadow lane to
    /// a real load. Empty = no proposal is ever loadable here (it can still shadow).
    trusted_reviewers: Vec<[u8; 32]>,
    /// Skip entries whose `size_bytes` hint exceeds this (node-local install budget). `None` =
    /// unbounded.
    install_budget_bytes: Option<u64>,
    /// Resource-aware eligibility (§4.4): a probe of this node's free memory/disk plus a
    /// headroom fraction (< 1.0) — an entry's declared requirements must fit within
    /// `headroom × available − reserved-by-in-flight-installs`. Default: the system probe at
    /// 0.8. `None` = disabled. Unmeasurable resources are permissive (detection, not
    /// prevention).
    resource_policy: Option<(Arc<dyn ResourceProbe>, f64)>,
    /// Artifacts this node has reserved (install in flight) or brought live — a round never
    /// double-starts. Lock discipline: acquired once per function, flat, never across `await`
    /// (wiki lock-order table row 21).
    hosted:       Arc<Mutex<HashMap<ArtifactId, HostedState>>>,
    /// Tripwire (detection, not prevention): resolvable entries skipped because no runtime is
    /// registered for their kind or they exceed the install budget. Counts skip *events* (one
    /// per entry per round), not distinct entries.
    ineligible:   Arc<AtomicU64>,
    /// The hard bound on concurrent installs, when an operator attached one (item 4 PR 4c).
    /// `None` = the self-imposed budgets above are the only bounds, as before.
    install_rights: Option<Arc<InstallRights>>,
    /// The component host, kept so [`set_fuel_policy`](Self::set_fuel_policy) can re-register the
    /// WASM runtime over it.
    wasm_host:    Arc<WasmHost>,
    /// The WASM runtime's invocation log, shared with it (D19's execution record).
    invocations:  InvocationLog,
}

use crate::runtime::TraceCtx as Trace;

/// One round's recorder: every decision point in `provision_round` hands it the values it already
/// produced. G7: no clock (the record's `at_ms` is absent — the round reads none), no draw of its own,
/// never under `hosted` (row 21), and the sink never waits (row 53).
struct RoundTrace<'a> {
    trace: &'a Trace,
    round: u64,
}

impl RoundTrace<'_> {
    fn record(&self, rule: &str, outcome: OutcomeKind, reason: &str, target: String, inputs: Vec<InputSnapshot>, effect: Option<String>) {
        self.trace.record(rule, format!("round {}", self.round), outcome, reason, target, inputs, effect);
    }
}

/// Why an install did or did not start — the value `prov.demand_response`, `prov.presence_floor` and
/// `prov.rights_admission` record; `start_install` keeps its `bool` for callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartOutcome {
    Started,
    AlreadyHosted,
    NoRuntime,
    RightsRefused,
}

fn cap_target(entry: &InstallableEntry) -> String {
    format!("{}/{}", entry.provides.namespace, entry.provides.name)
}

fn demand_inputs(demand: &mycelium::DemandStatus, hosted: usize) -> Vec<InputSnapshot> {
    vec![
        InputSnapshot::new("capabilities().demand(filter) — `demand/` and `cap/` as gossiped",
            format!("providers={} demanding={}", demand.providers.len(), demand.demanding_nodes.len()), Provenance::Gossiped),
        InputSnapshot::new("Provisioner::hosted (lock-order row 21)", format!("hosted={hosted}"), Provenance::Local),
    ]
}

impl Provisioner {
    /// Build a provisioner that self-elects with probability `self_elect_p` (use `1.0` for a
    /// single provisioner; lower for a fleet of them). Registers the WASM-component runtime over
    /// `host`; add other kinds via [`register_runtime`](Self::register_runtime).
    pub fn new(
        agent: Arc<GossipAgent>,
        host: Arc<WasmHost>,
        catalog: InstallableCatalog,
        source: Arc<dyn ArtifactSource + Send + Sync>,
        self_elect_p: f64,
    ) -> Self {
        let invocations: InvocationLog = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let mut runtimes: HashMap<ArtifactKind, Arc<dyn ArtifactRuntime>> = HashMap::new();
        runtimes.insert(
            ArtifactKind::WasmComponent,
            Arc::new(WasmComponentRuntime::new(Arc::clone(&host)).with_invocation_log(Arc::clone(&invocations))),
        );
        Self {
            agent,
            catalog,
            source,
            runtimes,
            self_elect_p,
            trace: None,
            rounds: 0,
            shed_regs: HashMap::new(),
            unmarked_since: HashMap::new(),
            policies: Vec::new(),
            trusted_publishers: Vec::new(),
            trusted_reviewers: Vec::new(),
            install_budget_bytes: None,
            resource_policy: Some((Arc::new(SystemResourceProbe::new()), 0.8)),
            hosted: Arc::new(Mutex::new(HashMap::new())),
            ineligible: Arc::new(AtomicU64::new(0)),
            install_rights: None,
            wasm_host: host,
            invocations,
        }
    }

    /// D19 — fuel by publisher: an entry signed by a key in `policy.operator_publishers` runs under
    /// `operator_budget` (`None` = unbounded), every other entry under `agent_budget`. The
    /// classification is the entry's verified `signer`, so pair this with
    /// [`require_provenance`](Self::require_provenance): without it a signer is a claim. Replaces
    /// the WASM runtime; installs already live keep the budget they were given.
    pub fn set_fuel_policy(&mut self, policy: FuelPolicy) {
        self.runtimes.insert(
            ArtifactKind::WasmComponent,
            Arc::new(
                WasmComponentRuntime::new(Arc::clone(&self.wasm_host))
                    .with_fuel_policy(policy)
                    .with_invocation_log(Arc::clone(&self.invocations)),
            ),
        );
    }

    /// The WASM runtime's invocation records, newest last: how each hosted call ended, with a
    /// fuel-exhausted stop named as such.
    pub fn invocations(&self) -> Vec<InvocationRecord> {
        self.invocations.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
    }

    /// Bound concurrent installs by the rights this node **holds** in `ledger` for `resource`
    /// (item 4 PR 4c): a round starts an install only while `reserved + live + 1 ≤ held units`,
    /// and refuses — recording `admission.rejected` in the ledger — otherwise. The head is
    /// published into `rights/head/{holder}` now and after every refusal, signed when
    /// `signing_key` is given. The ledger is the allocator's record: allocate the node's rights
    /// there before attaching it, or every install is refused (a refused install is not a
    /// silent skip — see [`rights_refusals`](Self::rights_refusals)).
    pub fn with_install_rights(
        &mut self,
        ledger: RightsLedger,
        holder: PrincipalId,
        resource: impl Into<String>,
        signing_key: Option<ed25519_dalek::SigningKey>,
    ) {
        let rights = Arc::new(InstallRights {
            ledger: Arc::new(tokio::sync::Mutex::new(ledger)),
            holder,
            resource: resource.into(),
            signing_key,
            refusals: AtomicU64::new(0),
            would_refuse: AtomicU64::new(0),
        });
        let agent = Arc::clone(&self.agent);
        let publish = Arc::clone(&rights);
        tokio::spawn(async move {
            let ledger = publish.ledger.lock().await;
            publish.publish_head(&agent, &ledger);
        });
        self.install_rights = Some(rights);
    }

    /// Installs refused under the attached rights (too few units held, or the ledger busy at the
    /// moment of admission). Never reset; an operator compares two readings.
    pub fn rights_refusals(&self) -> u64 {
        self.install_rights.as_ref().map_or(0, |r| r.refusals.load(Ordering::Relaxed))
    }

    /// Installs that would have been refused under `EnforceAllocated` and were admitted because the
    /// node's profile does not enforce a rights-backed bound (ADR §7: shadow before enforcement).
    /// The rejection is still recorded in the ledger. Never reset.
    pub fn rights_would_refuse(&self) -> u64 {
        self.install_rights.as_ref().map_or(0, |r| r.would_refuse.load(Ordering::Relaxed))
    }

    /// The **admission** step before an `Installing` reservation (item 4 PR 4c): does the node
    /// hold enough units for one more concurrent install? Reads the ledger's view under `try_lock`
    /// — the admission path is `provision_round`, which is synchronous and must not block on a
    /// journal write; a busy ledger is a refusal, not a wait. A refusal is counted here and
    /// recorded in the ledger off-path (`admit` appends `admission.rejected`, then the head is
    /// republished). No rights attached = admitted, as before.
    fn admit_install(&self) -> bool {
        let Some(rights) = self.install_rights.as_ref() else { return true };
        let consumed = self.hosted.lock().unwrap().len() as u64;
        let requested = consumed.saturating_add(1);
        let admitted = match rights.ledger.try_lock() {
            Ok(ledger) => ledger.may_admit(&rights.holder, &rights.resource, requested).is_ok(),
            Err(_) => false,
        };
        if admitted {
            return true;
        }
        // The ledger says no. Whether that refuses the install is the node's profile (ADR §7): a
        // rights-backed bound is Tier C, enforced only under `EnforceAllocated`; every other profile
        // admits, counts what would have been refused, and still records the rejection.
        let enforce = self.agent.control_profile() == mycelium::control::Profile::EnforceAllocated;
        if enforce {
            rights.refusals.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("mycelium_artifact_installs_refused_by_rights_total").increment(1);
        } else {
            rights.would_refuse.fetch_add(1, Ordering::Relaxed);
        }
        let rights = Arc::clone(rights);
        let agent = Arc::clone(&self.agent);
        tokio::spawn(async move {
            let mut ledger = rights.ledger.lock().await;
            // `admit` records the rejection (and only a rejection); a refusal because the ledger
            // was busy is re-judged here against the live view.
            let _ = ledger.admit(&rights.holder, &rights.resource, requested).await;
            rights.publish_head(&agent, &ledger);
        });
        !enforce
    }

    /// Register a runtime for an additional [`ArtifactKind`] (e.g. a blob/model runtime). A node
    /// only ever self-elects for kinds it has a runtime for — eligibility is node-local truth.
    pub fn register_runtime(&mut self, runtime: Arc<dyn ArtifactRuntime>) {
        self.runtimes.insert(runtime.kind(), runtime);
    }

    /// Replace the catalogue this provisioner resolves against — the gossiped catalogue is a
    /// **snapshot** at construction (`InstallableCatalog::from_kv`), so a node that starts before
    /// the librarian's entries arrive must refresh it (a stem node does, every tick).
    pub fn refresh_catalog(&mut self, catalog: InstallableCatalog) {
        self.catalog = catalog;
    }

    /// The entries currently resolved against.
    pub fn catalog(&self) -> &InstallableCatalog {
        &self.catalog
    }

    /// Cap the artifacts this node will elect to install by their `size_bytes` hint.
    pub fn set_install_budget(&mut self, max_bytes: u64) {
        self.install_budget_bytes = Some(max_bytes);
    }

    /// Override the resource-eligibility policy (§4.4): an entry's declared requirements must
    /// fit within `headroom × available − reserved`, where `available` comes from `probe`
    /// (memory globally; disk at the kind's runtime `resource_root`). `headroom` is clamped to
    /// `(0, 1]` — a node never commits 100 %+ of what it has free. The default is the system
    /// probe at `0.8`.
    pub fn set_resource_policy(&mut self, probe: Arc<dyn ResourceProbe>, headroom: f64) {
        self.resource_policy = Some((probe, headroom.clamp(f64::MIN_POSITIVE, 1.0)));
    }

    /// Disable resource-aware eligibility entirely (kind + budget checks remain).
    pub fn disable_resource_policy(&mut self) {
        self.resource_policy = None;
    }

    /// Requirements reserved by in-flight installs (the `Installing` states).
    fn reserved_requirements(&self) -> ResourceRequirements {
        let map = self.hosted.lock().unwrap();
        let mut sum = ResourceRequirements::default();
        for state in map.values() {
            if let HostedState::Installing { reserved, .. } = state {
                sum.disk_bytes = sum.disk_bytes.saturating_add(reserved.disk_bytes);
                sum.mem_bytes = sum.mem_bytes.saturating_add(reserved.mem_bytes);
            }
        }
        sum
    }

    /// Require **signed provenance**: only install catalog entries carrying a valid Ed25519
    /// signature over the entry (kind + content address + declared-provide) from one of `trusted`
    /// publisher keys. Without this, the provisioner trusts the catalog for integrity (hash match)
    /// but not origin; with it, an unsigned or untrusted-signer artifact is refused even if its
    /// bytes hash correctly.
    /// D20: the reviewers whose acceptance makes a proposed entry loadable for real. A proposal
    /// without a listed reviewer's acceptance loads only into the shadow lane
    /// ([`shadow_name`]), which takes no demand.
    pub fn require_reviewers(&mut self, reviewers: Vec<[u8; 32]>) {
        self.trusted_reviewers = reviewers;
    }

    /// D20: is `entry` loadable for real here (not a proposal, or accepted by a listed reviewer)?
    fn loadable(&self, entry: &InstallableEntry) -> bool {
        entry.is_loadable(&self.trusted_reviewers)
    }

    /// Number of capabilities live in the shadow lane (D20).
    pub fn shadow_count(&self) -> usize {
        self.hosted
            .lock()
            .unwrap()
            .values()
            .filter(|s| matches!(s, HostedState::Live(LiveHosted { shadow: true, .. })))
            .count()
    }

    /// The artifacts live in the shadow lane (D20).
    fn shadow_artifacts(&self) -> Vec<ArtifactId> {
        self.hosted
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| matches!(s, HostedState::Live(LiveHosted { shadow: true, .. })))
            .map(|(a, _)| *a)
            .collect()
    }

    pub fn require_provenance(&mut self, trusted: Vec<[u8; 32]>) {
        self.trusted_publishers = trusted;
    }

    /// True if `entry` may be installed under the current provenance policy.
    fn provenance_ok(&self, entry: &InstallableEntry) -> bool {
        self.trusted_publishers.is_empty() || entry.verify_provenance(&self.trusted_publishers)
    }

    /// Attach the decision trace (plan I5). Every decision `provision_round` makes is then recorded
    /// from the values it already produced — nothing is re-evaluated, no decision changes — and never
    /// under `hosted`; the sink never blocks. The stamps (node, build, configuration digest, profile)
    /// are taken once, here, from the startup report. Off unless called.
    pub fn with_decision_trace(&mut self, sink: Arc<DecisionSink>) -> &mut Self {
        let report = self.agent.guarantee_report();
        self.trace = Some(Arc::new(Trace {
            sink,
            node: report.node_id.clone(),
            build: report.version.to_string(),
            config_digest: report.config_digest.clone(),
            profile: report.profile.as_ref().map(|p| (p.name.to_string(), p.revision)),
        }));
        self
    }

    /// The attached decision sink, if any.
    pub fn decision_trace(&self) -> Option<Arc<DecisionSink>> {
        self.trace.as_ref().map(|t| Arc::clone(&t.sink))
    }

    fn round_trace(&self) -> Option<RoundTrace<'_>> {
        self.trace.as_deref().map(|trace| RoundTrace { trace, round: self.rounds })
    }

    /// True if this node *can* host `entry`: a runtime is registered for its kind, it fits the
    /// install budget, and its declared requirements fit this node's **resource headroom**
    /// (§4.4: `headroom × available − reserved-by-in-flight`; memory globally, disk at the
    /// runtime's `resource_root`; undeclared requirements and unmeasurable resources are
    /// permissive). A miss is silent non-participation — some node that fits elects; this is
    /// the fleet's placement algorithm, with no scheduler and no resource gossip — plus a
    /// tripwire tick, see [`ineligible_skips`](Self::ineligible_skips).
    fn eligibility(&self, entry: &InstallableEntry) -> Result<(), &'static str> {
        let Some(runtime) = self.runtimes.get(&entry.kind) else {
            self.ineligible.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("mycelium_artifact_ineligible_skips_total", "reason" => "no_runtime")
                .increment(1);
            tracing::debug!(ns = %entry.provides.namespace, name = %entry.provides.name,
                kind = ?entry.kind, "no runtime registered for kind — not electing");
            return Err("no_runtime");
        };
        if let Some(max) = self.install_budget_bytes
            && entry.size_bytes > max
        {
            self.ineligible.fetch_add(1, Ordering::Relaxed);
            metrics::counter!("mycelium_artifact_ineligible_skips_total", "reason" => "budget")
                .increment(1);
            tracing::debug!(ns = %entry.provides.namespace, name = %entry.provides.name,
                size = entry.size_bytes, budget = max, "entry exceeds install budget — not electing");
            return Err("budget");
        }
        if let Some((probe, headroom)) = &self.resource_policy
            && (entry.requires.mem_bytes > 0 || entry.requires.disk_bytes > 0)
        {
            let reserved = self.reserved_requirements();
            let usable = |avail: u64| (avail as f64 * headroom) as u64;
            if entry.requires.mem_bytes > 0
                && let Some(avail) = probe.available_memory_bytes()
                && entry.requires.mem_bytes.saturating_add(reserved.mem_bytes) > usable(avail)
            {
                self.ineligible.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("mycelium_artifact_ineligible_skips_total", "reason" => "memory")
                    .increment(1);
                tracing::debug!(ns = %entry.provides.namespace, name = %entry.provides.name,
                    require = entry.requires.mem_bytes, reserved = reserved.mem_bytes,
                    available = avail, headroom, "memory requirement exceeds headroom — not electing");
                return Err("memory");
            }
            if entry.requires.disk_bytes > 0
                && let Some(root) = runtime.resource_root()
                && let Some(avail) = probe.available_disk_bytes(root)
                && entry.requires.disk_bytes.saturating_add(reserved.disk_bytes) > usable(avail)
            {
                self.ineligible.fetch_add(1, Ordering::Relaxed);
                metrics::counter!("mycelium_artifact_ineligible_skips_total", "reason" => "disk")
                    .increment(1);
                tracing::debug!(ns = %entry.provides.namespace, name = %entry.provides.name,
                    require = entry.requires.disk_bytes, reserved = reserved.disk_bytes,
                    available = avail, headroom, "disk requirement exceeds headroom — not electing");
                return Err("disk");
            }
        }
        Ok(())
    }

    /// Tripwire counter: skip events for resolvable-but-ineligible entries (no runtime for the
    /// kind, or over the install budget). One tick per skipped entry per round.
    pub fn ineligible_skips(&self) -> u64 {
        self.ineligible.load(Ordering::Relaxed)
    }

    /// Add a capability-presence invariant (M14 supervision): keep ≥ `min_providers` live providers
    /// of `filter` alive, re-provisioning from the catalog when the count drops (a dead provider's
    /// `cap/` evaporates → count falls → re-satisfied). Drives bring-up *without* organic demand.
    pub fn supervise(&mut self, filter: CapFilter, min_providers: usize) {
        self.policies.push(SupervisionPolicy { filter, min_providers, max_providers: None });
    }

    /// Like [`supervise`](Self::supervise) but with an upper bound (Track 2b elastic sizing): keep
    /// the live provider count within `[min, max]`. Below `min` → bring up (soft self-election); above
    /// `max` → the hosts ranked beyond `max` **withdraw** (cooperative self-removal; ranked, so hosts with
    /// the same view agree on which). Bounds are convergence targets, not guarantees, consistent with
    /// the membership governor.
    pub fn supervise_band(&mut self, filter: CapFilter, min_providers: usize, max_providers: usize) {
        self.policies.push(SupervisionPolicy {
            filter,
            min_providers,
            max_providers: Some(max_providers),
        });
    }

    /// Stop hosting `artifact`: tombstone its `cap/` advertisement and let the runtime tear down
    /// ([`Installed::uninstall`] — abort the serve task, delete placed bytes). An **in-flight**
    /// install is cancelled by removing its reservation; the install task's token check tears the
    /// finished result down on completion. Cooperative self-removal — the symmetric counterpart
    /// to [`try_start_install_as`](Self::try_start_install_as).
    fn withdraw(&mut self, artifact: &ArtifactId) -> bool {
        let removed = self.hosted.lock().unwrap().remove(artifact);
        match removed {
            Some(HostedState::Live(live)) => {
                live.installed.uninstall(); // dropping live._cap tombstones the cap/ entry
                true
            }
            Some(HostedState::Installing { .. }) => true,
            None => false,
        }
    }

    /// Number of capabilities this node is hosting **live** via provisioning. In-flight installs
    /// are not counted — see [`installing_count`](Self::installing_count).
    pub fn hosted_count(&self) -> usize {
        self.hosted
            .lock()
            .unwrap()
            .values()
            .filter(|s| matches!(s, HostedState::Live(_)))
            .count()
    }

    /// Number of installs currently in flight (reserved, background task running).
    pub fn installing_count(&self) -> usize {
        self.hosted
            .lock()
            .unwrap()
            .values()
            .filter(|s| matches!(s, HostedState::Installing { .. }))
            .count()
    }

    /// True if `artifact` is reserved or live on this node.
    fn is_hosted(&self, artifact: &ArtifactId) -> bool {
        self.hosted.lock().unwrap().contains_key(artifact)
    }

    /// The live install this node can withdraw for `filter`, if any — read from what each install
    /// advertises, so an entry since removed from the catalogue, or a different entry for the same band,
    /// still counts; a shadow never matches (its name is the shadow name), and an `Installing` reservation
    /// is not a provider yet.
    fn hosted_artifact_for(&self, filter: &CapFilter) -> Option<ArtifactId> {
        let hosted = self.hosted.lock().unwrap();
        hosted.iter().find_map(|(artifact, state)| match state {
            HostedState::Live(live) if filter.matches(&live.provides) => Some(*artifact),
            _ => None,
        })
    }

    /// Start bringing one capability live on this node: **reserve** the artifact, then run the
    /// kind's runtime install as a background task (pull + verify + install), advertising the
    /// declared-provide and flipping the reservation to `Live` on success (on failure the
    /// reservation is dropped, so a later round retries — restart ≡ provisioning). Returns why it
    /// did or did not start (`StartOutcome`) — the value the trace records. Shared by the demand and
    /// presence paths — the one resolve-and-pull path the architecture promises. Into the shadow lane
    /// when `shadow` (D20): the entry is installed and served under [`shadow_name`], so it is
    /// resolvable for comparison and never for the incumbent's demand.
    fn try_start_install_as(&self, mut entry: InstallableEntry, shadow: bool) -> StartOutcome {
        if shadow {
            entry.provides.name = Arc::from(shadow_name(&entry.provides.name));
        }
        let Some(runtime) = self.runtimes.get(&entry.kind).map(Arc::clone) else {
            return StartOutcome::NoRuntime; // eligible() screens this; belt-and-braces for direct callers
        };

        static INSTALL_SEQ: AtomicU64 = AtomicU64::new(0);
        let token = INSTALL_SEQ.fetch_add(1, Ordering::Relaxed);
        // The contract's reserve step comes before the reservation it protects (persist-then-act
        // in the ledger's terms): a node that holds too few install rights does not enter
        // `Installing`. Reads and releases the ledger before `hosted` is taken (row 37 → row 21,
        // never nested).
        if self.is_hosted(&entry.artifact) {
            return StartOutcome::AlreadyHosted;
        }
        if !self.admit_install() {
            tracing::info!(ns = %entry.provides.namespace, name = %entry.provides.name,
                "install refused: the node holds too few install rights");
            return StartOutcome::RightsRefused;
        }
        {
            let mut map = self.hosted.lock().unwrap();
            if map.contains_key(&entry.artifact) {
                return StartOutcome::AlreadyHosted;
            }
            map.insert(
                entry.artifact,
                HostedState::Installing { token, reserved: entry.requires },
            );
        }
        metrics::counter!("mycelium_artifact_installs_started_total").increment(1);

        let agent = Arc::clone(&self.agent);
        let source = Arc::clone(&self.source);
        let hosted = Arc::clone(&self.hosted);
        let trace = self.trace.clone();
        let round = self.rounds;
        tokio::spawn(async move {
            let artifact = entry.artifact;
            let provides = entry.provides.clone();
            let target = format!("{}/{}", provides.namespace, provides.name);
            // `prov.install`'s record, after `hosted` is released (G7), from the outcome the task has.
            let record = |outcome: OutcomeKind, reason: &str, effect: Option<String>| {
                if let Some(t) = &trace {
                    t.record("prov.install", format!("round {round} · install token {token}"), outcome, reason, target.clone(), Vec::new(), effect);
                }
            };

            // Loading tier: while the install runs, `{ns}/loading` is advertised with a `pct`
            // attribute stepped in tens — the capability-tier convention the llm_agent example
            // established, here driven by real bytes from the runtime's pull. Each step
            // tombstones-then-re-advertises (that order makes the advertise the LWW winner);
            // the whole tier drops when the install resolves. Lock-order table row 22.
            let loading: Arc<Mutex<(u64, Option<CapabilityReg>)>> =
                Arc::new(Mutex::new((u64::MAX, None)));
            let progress: ProgressFn = {
                let loading = Arc::clone(&loading);
                let agent = Arc::clone(&agent);
                let ns = provides.namespace.clone();
                Arc::new(move |fetched, total| {
                    if total == 0 {
                        return;
                    }
                    let step = (fetched.saturating_mul(100) / total).min(100) / 10 * 10;
                    let mut tier = loading.lock().unwrap();
                    if tier.0 == step {
                        return;
                    }
                    tier.0 = step;
                    let mut cap = Capability::new(ns.clone(), "loading");
                    cap.attributes.insert("pct".into(), CapValue::Integer(step as i64));
                    tier.1.take(); // tombstone the previous step first
                    tier.1 =
                        Some(agent.capabilities().advertise_capability(cap, ADVERTISE_INTERVAL));
                })
            };
            let ctx = RuntimeCtx { agent: Arc::clone(&agent), trace: trace.clone(), install_token: token };

            let result = runtime.install(entry, source, ctx, progress).await;
            loading.lock().unwrap().1.take(); // install resolved — the loading tier ends

            match result {
                Ok(installed) => {
                    // Advertise only after a successful install: a resolvable `cap/` entry always
                    // has a live receiver behind it (the runtime registered its serve path before
                    // returning).
                    let cap = agent
                        .capabilities()
                        .advertise_capability(provides.clone(), ADVERTISE_INTERVAL);
                    // Take the lock once; hand ownership back out on the not-ours path so the
                    // teardown below runs *outside* the lock. Dropping `installed` would NOT stop
                    // the serve path (JoinHandle drop detaches) — uninstall must be explicit.
                    let leftover = {
                        let mut map = hosted.lock().unwrap();
                        match map.get(&artifact) {
                            Some(HostedState::Installing { token: t, .. }) if *t == token => {
                                map.insert(
                                    artifact,
                                    HostedState::Live(LiveHosted { _cap: cap, installed, provides: provides.clone(), shadow }),
                                );
                                None
                            }
                            _ => Some((cap, installed)),
                        }
                    };
                    match leftover {
                        None => {
                            metrics::counter!("mycelium_artifact_installs_completed_total")
                                .increment(1);
                            tracing::info!(ns = %provides.namespace, name = %provides.name,
                                "provisioned + serving capability");
                            record(OutcomeKind::Action, "completed", Some(format!("install token {token}: live, advertised")));
                        }
                        Some((cap, installed)) => {
                            // Withdrawn (or superseded) while installing: nothing references
                            // this install — tombstone the just-made ad and tear it down.
                            drop(cap);
                            installed.uninstall();
                            tracing::info!(ns = %provides.namespace, name = %provides.name,
                                "install finished after withdraw — torn down");
                            record(OutcomeKind::NoAction, "superseded", None);
                        }
                    }
                }
                Err(e) => {
                    metrics::counter!("mycelium_artifact_installs_failed_total",
                        "stage" => e.stage()).increment(1);
                    tracing::warn!(ns = %provides.namespace, name = %provides.name, %e,
                        "provisioning failed");
                    {
                        let mut map = hosted.lock().unwrap();
                        if matches!(map.get(&artifact),
                            Some(HostedState::Installing { token: t, .. }) if *t == token)
                        {
                            map.remove(&artifact);
                        }
                    }
                    record(OutcomeKind::Refusal, e.stage(), None);
                }
            }
        });
        StartOutcome::Started
    }

    /// `eligible()`, with its `prov.eligible` record: the reason the live evaluation produced, never a
    /// second evaluation (the check has side effects — counters — and is evaluated exactly once).
    fn eligible_traced(&self, entry: &InstallableEntry, target: &str) -> bool {
        let verdict = self.eligibility(entry);
        if let Some(t) = self.round_trace() {
            match verdict {
                Ok(()) => t.record("prov.eligible", OutcomeKind::Action, "eligible", target.to_string(), Vec::new(), None),
                Err(reason) => t.record("prov.eligible", OutcomeKind::Refusal, reason, target.to_string(), Vec::new(), None),
            }
        }
        verdict.is_ok()
    }

    /// `self_elects()`, with its `prov.self_election` record — one draw, recorded after it was made.
    fn self_elects_traced(&self, target: &str) -> bool {
        let elected = self.self_elects();
        if let Some(t) = self.round_trace() {
            let (outcome, reason) = if elected { (OutcomeKind::Action, "elected") } else { (OutcomeKind::Deferral, "declined") };
            t.record("prov.self_election", outcome, reason, target.to_string(),
                vec![InputSnapshot::new("StemOptions::self_elect_p", format!("p={}", self.self_elect_p), Provenance::Configured)], None);
        }
        elected
    }

    /// The record for an install the demand or presence pass tried to start, from the outcome the
    /// start produced; a rights refusal also records `prov.rights_admission`.
    fn record_start(&self, target: &str, rule: &str, acted: &str, outcome: StartOutcome, inputs: Vec<InputSnapshot>) {
        let Some(t) = self.round_trace() else { return };
        match outcome {
            StartOutcome::Started => t.record(rule, OutcomeKind::Action, acted, target.to_string(), inputs, Some("start_install".into())),
            StartOutcome::AlreadyHosted => t.record(rule, OutcomeKind::Refusal, "already_hosted", target.to_string(), inputs, None),
            StartOutcome::NoRuntime => t.record(rule, OutcomeKind::Refusal, "ineligible", target.to_string(), inputs, None),
            StartOutcome::RightsRefused => {
                t.record("prov.rights_admission", OutcomeKind::Refusal, "allocation_exhausted", target.to_string(),
                    vec![InputSnapshot::new("InstallRights::ledger (lock-order row 37)", "allocation exhausted", Provenance::Local)], None);
                t.record(rule, OutcomeKind::Refusal, "rights_refused", target.to_string(), inputs, None);
            }
        }
    }

    /// True if this node should self-elect to act this round (herd damping).
    fn self_elects(&self) -> bool {
        // Through the seam: under `sim` the draw is a recorded kernel choice, so a replay reproduces
        // who installed (the `select` stream, as the capability resolver's tie-break). Raw `fastrand`
        // here was invisible to a recording (2026-10-03).
        f64::from(mycelium::sim_seam::rng_f32("select")) < self.self_elect_p
    }

    /// One convergence pass over **both** desired-state sources, returning how many installs were
    /// newly **started** (installs run as background tasks — poll
    /// [`hosted_count`](Self::hosted_count) / the capability ring for completion). Idempotent —
    /// already-reserved/hosted/satisfied entries are skipped.
    ///
    /// 1. **Demand-driven** (M15): a catalog entry whose declared-provide has demand but no live
    ///    provider → bring it live (relieves demand).
    /// 2. **Presence-driven** (M14 supervision): a policy whose live provider count is below
    ///    `min_providers` → resolve the catalog and bring a provider live. Self-healing falls out:
    ///    a dead provider's `cap/` evaporates, the count drops, this fires again — restart and
    ///    first-time provisioning are the same path.
    pub fn provision_round(&mut self) -> usize {
        let mut started = 0;
        self.rounds += 1;
        let hosted_now = self.hosted_count();

        // ── Probe-gated health (§4.2) ────────────────────────────────────────
        // A Live install whose probe fails is withdrawn — advertisement retracted, runtime
        // torn down — and the demand/presence passes reinstall it as soon as the retracted
        // ad clears the local capability view (typically the next round; the tombstone write
        // is not synchronous with this pass): restart ≡ provisioning. Health is the hosting
        // node's own observation; there is no fleet health protocol. Probes run under the
        // hosted lock — they must be cheap and non-blocking (see `Installed::probe`).
        let unhealthy: Vec<ArtifactId> = {
            let map = self.hosted.lock().unwrap();
            map.iter()
                .filter_map(|(artifact, state)| match state {
                    HostedState::Live(live) if !live.installed.probe() => Some(*artifact),
                    _ => None,
                })
                .collect()
        };
        let any_unhealthy = !unhealthy.is_empty();
        for artifact in unhealthy {
            metrics::counter!("mycelium_artifact_probe_withdrawals_total").increment(1);
            tracing::warn!(artifact = %artifact,
                "hosted install failed its probe — withdrawing (this round reinstalls if still wanted)");
            self.withdraw(&artifact);
            if let Some(t) = self.round_trace() {
                t.record("prov.health_pass", OutcomeKind::Action, "probe_failed", artifact.to_string(), Vec::new(), Some(format!("withdraw {artifact}")));
            }
        }
        if !any_unhealthy
            && hosted_now > 0
            && let Some(t) = self.round_trace()
        {
            t.record("prov.health_pass", OutcomeKind::NoAction, "all_healthy", format!("{hosted_now} live"), Vec::new(), None);
        }

        // ── Promotion (D20) ──────────────────────────────────────────────────
        // A shadow whose entry has since been accepted (or has left the catalogue) is withdrawn,
        // so the demand pass below can bring the accepted entry live under its own name once
        // demand is unmet. Acceptance travels as the same catalogue line, rewritten.
        for artifact in self.shadow_artifacts() {
            let still_a_proposal = self
                .catalog
                .entries()
                .iter()
                .any(|e| e.artifact == artifact && e.proposed && !self.loadable(e));
            if !still_a_proposal {
                tracing::info!(artifact = %artifact, "shadow withdrawn: its entry was accepted or retired");
                self.withdraw(&artifact);
            }
            if let Some(t) = self.round_trace() {
                if still_a_proposal {
                    t.record("prov.promotion", OutcomeKind::NoAction, "still_proposed", artifact.to_string(), Vec::new(), None);
                } else {
                    t.record("prov.promotion", OutcomeKind::Action, "accepted_withdraw_shadow", artifact.to_string(), Vec::new(), Some(format!("withdraw {artifact}")));
                }
            }
        }

        // ── Demand-driven (M15) ──────────────────────────────────────────────
        let entries: Vec<InstallableEntry> = self.catalog.entries().to_vec();
        // Dedup by CAPABILITY within a round, not just by ArtifactId. Two catalog entries providing
        // the same `(ns,name)` via DIFFERENT artifacts would both read demand as unmet this round
        // (neither install has advertised yet) and both start → two runtimes both `rpc_rx` the same
        // `cap.invoke/{ns}/{name}` kind, and `deliver_to_handlers` fans out to BOTH, so every
        // invocation executes on both components (double side effects) (audit 2026-07-15 pass 5).
        let mut started_caps: std::collections::HashSet<(String, String)> = Default::default();
        for entry in entries {
            let target = cap_target(&entry);
            if self.is_hosted(&entry.artifact) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.demand_response", OutcomeKind::Refusal, "already_hosted", target, Vec::new(), None);
                }
                continue;
            }
            if !self.provenance_ok(&entry) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.demand_response", OutcomeKind::Refusal, "provenance_rejected", target, Vec::new(), None);
                }
                continue;
            }
            let cap_key = (entry.provides.namespace.to_string(), entry.provides.name.to_string());
            if started_caps.contains(&cap_key) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.demand_response", OutcomeKind::Deferral, "already_started_this_round", target, Vec::new(), None);
                }
                continue; // a sibling artifact for this capability already started this round
            }
            let filter =
                CapFilter::new(entry.provides.namespace.clone(), entry.provides.name.clone());
            let demand = self.agent.capabilities().demand(&filter);
            let inputs = || demand_inputs(&demand, hosted_now);
            if !self.loadable(&entry) {
                // D20: a proposal without acceptance loads only into the shadow lane, and only
                // where the capability is wanted at all — beside the incumbent, taking none of
                // its demand. Its own dedup: one shadow per artifact (`is_hosted`, above).
                if demand.demanding_nodes.is_empty() {
                    if let Some(t) = self.round_trace() {
                        t.record("prov.demand_response", OutcomeKind::NoAction, "no_demand", target, inputs(), None);
                    }
                    continue;
                }
                if !self.eligible_traced(&entry, &target) {
                    if let Some(t) = self.round_trace() {
                        t.record("prov.demand_response", OutcomeKind::Refusal, "ineligible", target, inputs(), None);
                    }
                    continue;
                }
                if !self.self_elects_traced(&target) {
                    if let Some(t) = self.round_trace() {
                        t.record("prov.demand_response", OutcomeKind::Deferral, "self_election_declined", target, inputs(), None);
                    }
                    continue;
                }
                let outcome = self.try_start_install_as(entry, true);
                if outcome == StartOutcome::Started {
                    metrics::counter!("mycelium_artifact_shadow_installs_total").increment(1);
                    started += 1;
                }
                self.record_start(&target, "prov.demand_response", "unmet_demand_shadow", outcome, inputs());
                continue;
            }
            let unmet = demand.providers.is_empty() && !demand.demanding_nodes.is_empty();
            if !unmet {
                if let Some(t) = self.round_trace() {
                    let reason = if demand.demanding_nodes.is_empty() { "no_demand" } else { "provider_present" };
                    t.record("prov.demand_response", OutcomeKind::NoAction, reason, target, inputs(), None);
                }
                continue;
            }
            // Eligibility is checked only for entries this node would otherwise act on — an
            // idle catalog must not tick the tripwire every round.
            if !self.eligible_traced(&entry, &target) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.demand_response", OutcomeKind::Refusal, "ineligible", target, inputs(), None);
                }
                continue;
            }
            if !self.self_elects_traced(&target) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.demand_response", OutcomeKind::Deferral, "self_election_declined", target, inputs(), None);
                }
                continue;
            }
            let outcome = self.try_start_install_as(entry, false);
            if outcome == StartOutcome::Started {
                started += 1;
                started_caps.insert(cap_key);
            }
            self.record_start(&target, "prov.demand_response", "unmet_demand_live", outcome, inputs());
        }

        // ── Presence-driven (M14 supervision) ────────────────────────────────
        let policies = self.policies.clone();
        for policy in &policies {
            // Live provider count is freshness-aware: a crashed provider's cap/ entry ages out,
            // so `providers` reflects only currently-live providers (this is the self-heal trigger).
            let live = self.agent.capabilities().demand(&policy.filter).providers.len();
            let target = format!("{}/{}", policy.filter.namespace, policy.filter.name);
            let inputs = || vec![
                InputSnapshot::new("capabilities().demand(filter).providers — `cap/` as gossiped",
                    format!("live={live} min={}", policy.min_providers), Provenance::Gossiped),
                InputSnapshot::new("Provisioner::hosted (lock-order row 21)", format!("hosted={hosted_now}"), Provenance::Local),
            ];
            if live >= policy.min_providers {
                if let Some(t) = self.round_trace() {
                    t.record("prov.presence_floor", OutcomeKind::NoAction, "floor_met", target, inputs(), None);
                }
                continue; // invariant already satisfied across the fleet
            }
            // Resolve the catalog for an artifact that would satisfy the invariant — a loadable
            // one (D20: a proposal keeps no floor).
            let Some(entry) = self
                .catalog
                .entries()
                .iter()
                .filter(|e| policy.filter.matches(&e.provides) && self.loadable(e))
                .min_by_key(|e| (e.size_bytes, e.est_install_secs))
                .cloned()
            else {
                if let Some(t) = self.round_trace() {
                    t.record("prov.presence_floor", OutcomeKind::Refusal, "no_loadable_candidate", target, inputs(), None);
                }
                continue; // nothing loadable in the catalog provides it
            };
            let refused = if self.is_hosted(&entry.artifact) {
                Some("already_hosted")
            } else if !self.provenance_ok(&entry) {
                Some("provenance_rejected")
            } else if !self.eligible_traced(&entry, &target) {
                Some("ineligible")
            } else {
                None
            };
            if let Some(reason) = refused {
                if let Some(t) = self.round_trace() {
                    t.record("prov.presence_floor", OutcomeKind::Refusal, reason, target, inputs(), None);
                }
                continue; // already reserved/hosted here, or fails policy
            }
            if !self.self_elects_traced(&target) {
                if let Some(t) = self.round_trace() {
                    t.record("prov.presence_floor", OutcomeKind::Deferral, "self_election_declined", target, inputs(), None);
                }
                continue;
            }
            let outcome = self.try_start_install_as(entry, false);
            if outcome == StartOutcome::Started {
                started += 1;
            }
            self.record_start(&target, "prov.presence_floor", "below_floor", outcome, inputs());
        }

        // ── Shed-driven (Track 2b elastic sizing) ────────────────────────────
        // Symmetric to bring-up: when a band's live provider count exceeds `max`, the hosts ranked
        // beyond `max` withdraw. Over-provisioning (e.g. a transient duplicate after a herd of
        // self-elections) self-corrects here, in one round.
        // `prov-shed/{band}` for every band this node supervises with a ceiling, except while it provides the
        // band without an install it could withdraw — the one case where being ranked would stall the band.
        let me = self.agent.node_id().clone();
        let mut marked: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut can_shed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in &policies {
            let Some(max) = p.max_providers else { continue };
            let band = shed_band(&p.filter, max);
            let holds = self.hosted_artifact_for(&p.filter).is_some();
            let provides = self.agent.capabilities().demand(&p.filter).providers.contains(&me);
            if holds {
                can_shed.insert(band.clone());
            }
            if marks_band(provides, holds) {
                marked.insert(band);
            }
        }
        self.shed_regs.retain(|band, _| marked.contains(band));
        for band in &marked {
            if !self.shed_regs.contains_key(band) {
                let reg = self.agent.capabilities().advertise_capability(Capability::new(SHED_NS, band.clone()), ADVERTISE_INTERVAL);
                self.shed_regs.insert(band.clone(), reg);
            }
        }
        for policy in &policies {
            let Some(max) = policy.max_providers else { continue };
            let providers = self.agent.capabilities().demand(&policy.filter).providers;
            let live = providers.len();
            let target = format!("{}/{}", policy.filter.namespace, policy.filter.name);
            if live <= max {
                self.unmarked_since.remove(&shed_band(&policy.filter, max));
                if let Some(t) = self.round_trace() {
                    t.record("prov.shed", OutcomeKind::NoAction, "within_ceiling", target,
                        vec![InputSnapshot::new("capabilities().demand(filter).providers — `cap/` as gossiped",
                            format!("live={live} max={max}"), Provenance::Gossiped)], None);
                }
                continue; // within the band
            }
            // Which hosts above the ceiling withdraw is ranked, not drawn: every host with the same view
            // agrees that exactly the surplus goes. A per-host draw at `self_elect_p` let every one
            // withdraw at once — max+1 → 0 → back, round after round (#545). Only providers that will act
            // on the ceiling are ranked — they advertise `prov-shed/{band}`; one that does not
            // (registered in code, a stem without this band) keeps its place, and the rest rank against
            // what is left — once seen unmarked for FIXED_AFTER; until then it is presumed to shed (a `cap/`
            // entry and its mark can arrive apart). Which providers shed is read from their marks, present
            // from a band's first round, so a new install is never unmarked. A view with k wrong entries can
            // leave the band at max − k for a round (the floor refills); it never cascades.
            let band = shed_band(&policy.filter, max);
            let shedders: Vec<mycelium::NodeId> =
                self.agent.capabilities().demand(&CapFilter::new(SHED_NS, band.clone())).providers;
            // This node by what it holds now — whether or not its advertisement has been written yet.
            let i_can_shed = can_shed.contains(&band);
            let now = mycelium::sim_seam::mono_now_ns();
            let seen = self.unmarked_since.entry(band.clone()).or_default();
            let (entries, presumed) = rank_entries(&providers, &shedders, &me, i_can_shed, seen, now);
            let view = shed_view(&entries, max, &target);
            let position = view.ranked.iter().position(|n| *n == me);
            let inputs = || vec![
                InputSnapshot::new("capabilities().demand(filter).providers — `cap/` as gossiped",
                    format!("live={live} max={max}"), Provenance::Gossiped),
                InputSnapshot::new("the providers that shed (marked), ranked by the band's rendezvous order",
                    format!("position={position:?} keep={} fixed={}", view.keep, live - view.ranked.len()), Provenance::Gossiped),
                InputSnapshot::new("unmarked peers presumed to shed — seen unmarked for less than FIXED_AFTER (this node's monotonic observation)",
                    format!("presumed={presumed}"), Provenance::Local),
            ];
            let Some(artifact) = self.hosted_artifact_for(&policy.filter) else {
                if let Some(t) = self.round_trace() {
                    t.record("prov.shed", OutcomeKind::NoAction, "not_hosting", target, inputs(), None);
                }
                continue;
            };
            if view.sheds(&me) {
                self.withdraw(&artifact); // cooperative self-removal
                if let Some(t) = self.round_trace() {
                    t.record("prov.shed", OutcomeKind::Action, "above_ceiling", target, inputs(), Some(format!("withdraw {artifact}")));
                }
            } else if let Some(t) = self.round_trace() {
                t.record("prov.shed", OutcomeKind::NoAction, "ranked_within_ceiling", target, inputs(), None);
            }
        }

        started
    }
}

/// The capability namespace a provisioner advertises to say it will act on a band's shed: once per band it
/// supervises with a ceiling **and** holds a live install it can withdraw for. A band ranks only providers
/// that advertise it (#547).
pub const SHED_NS: &str = "prov-shed";

/// The `prov-shed` name for a band: `{ns}:{name}:{hash}`, the hash over the filter's gossip encoding (the
/// one `req/` carries: attributes and schema; ranking reset, `max_age` is never encoded) and the ceiling —
/// two bands on one capability that differ in any of them are different bands, and every node, of any
/// build that speaks this wire, computes the same name.
pub fn shed_band(filter: &CapFilter, max: usize) -> String {
    let mut identity = CapFilter { ranking: None, ..filter.clone() };
    identity.max_age = None;
    let mut bytes = identity.encode().to_vec();
    bytes.extend_from_slice(&(max as u64).to_le_bytes());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    format!("{}:{}:{h:016x}", filter.namespace, filter.name)
}

/// One round's shedding view of a band whose live providers exceed its ceiling.
struct ShedView {
    /// The providers that shed, best first, by [`mycelium::election::rank`] on the band's name.
    ranked: Vec<mycelium::NodeId>,
    /// How many of `ranked` stay: the ceiling, less the providers that do not shed (they stay regardless).
    keep: usize,
}

/// Whether a node advertises `prov-shed` for a band it supervises with a ceiling: always, except while it
/// **provides** the band without holding an install it could withdraw — then ranking it would stall the band
/// above its ceiling, since it cannot leave. A node that does not provide the band is never read (only
/// providers are ranked), so marking it from the start costs nothing and means a new install is marked the
/// moment it goes live — a herd of new installs ranks exactly (#547's fourth review).
fn marks_band(provides: bool, holds_withdrawable: bool) -> bool {
    !provides || holds_withdrawable
}

/// How long a peer must be seen providing a band without a `prov-shed` mark before it is counted as one
/// that will not shed: two advertise intervals, more than the gap between a `cap/` entry and a mark
/// arriving apart.
const FIXED_AFTER: Duration = Duration::from_secs(2 * ADVERTISE_INTERVAL.as_secs());

/// Whether a peer providing the band is ranked as one that sheds: marked, or unmarked for less than
/// [`FIXED_AFTER`]. In doubt it sheds — the error is a band above its ceiling for a few seconds, never
/// below it; counting a provider fixed too early is what over-sheds (#547's fifth review).
fn presumed_sheds(marked: bool, unmarked_for: Option<Duration>) -> bool {
    marked || unmarked_for.is_none_or(|d| d < FIXED_AFTER)
}

/// Each provider of a band and whether it is ranked as one that sheds, updating `seen` (when each unmarked
/// peer was first seen unmarked, monotonic ns) — and how many unmarked peers are presumed to shed. This node
/// counts itself by `i_can_shed`; a peer by its mark, or, unmarked, by [`presumed_sheds`]. `seen` keeps only
/// current, unmarked providers, so a peer that is marked and later unmarked starts again.
fn rank_entries(
    providers: &[mycelium::NodeId],
    shedders: &[mycelium::NodeId],
    me: &mycelium::NodeId,
    i_can_shed: bool,
    seen: &mut HashMap<mycelium::NodeId, u64>,
    now_ns: u64,
) -> (Vec<(mycelium::NodeId, bool)>, usize) {
    seen.retain(|n, _| providers.contains(n) && !shedders.contains(n));
    let mut presumed = 0;
    let entries = providers.iter().map(|n| {
        if n == me {
            return (n.clone(), i_can_shed);
        }
        if shedders.contains(n) {
            return (n.clone(), true);
        }
        let since = *seen.entry(n.clone()).or_insert(now_ns);
        let sheds = presumed_sheds(false, Some(Duration::from_nanos(now_ns.saturating_sub(since))));
        presumed += usize::from(sheds);
        (n.clone(), sheds)
    }).collect();
    (entries, presumed)
}

/// `entries` is each live provider and whether it sheds (it advertises [`SHED_NS`] for the band).
fn shed_view(entries: &[(mycelium::NodeId, bool)], max: usize, band: &str) -> ShedView {
    let shedders: Vec<mycelium::NodeId> = entries.iter().filter(|(_, s)| *s).map(|(n, _)| n.clone()).collect();
    let fixed = entries.len() - shedders.len();
    let ranked = mycelium::election::rank(band, &shedders, mycelium::election::Rule::Rendezvous)
        .into_iter().cloned().collect();
    ShedView { ranked, keep: max.saturating_sub(fixed) }
}

impl ShedView {
    /// Whether `me` withdraws: ranked beyond the ones that stay. A host absent from its own view stays —
    /// its advertisement has not reached itself, so it cannot know its rank.
    fn sheds(&self, me: &mycelium::NodeId) -> bool {
        self.ranked.iter().position(|n| n == me).is_some_and(|i| i >= self.keep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::InstallableEntry;
    use crate::InMemorySource;
    use mycelium::Capability;

    const ECHO_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/echo_component.wasm");

    fn nodes(base: u16, n: u16) -> Vec<mycelium::NodeId> {
        (0..n).map(|i| mycelium::NodeId::new("127.0.0.1", base + i).unwrap()).collect()
    }

    fn all_shed(ns: &[mycelium::NodeId]) -> Vec<(mycelium::NodeId, bool)> {
        ns.iter().map(|n| (n.clone(), true)).collect()
    }

    /// Issue #545: over one view of a band's providers, exactly the surplus withdraws — never all of them,
    /// never none — whatever the band or the size. The draw this replaced let every host withdraw at once.
    #[test]
    fn exactly_the_surplus_above_a_ceiling_withdraws() {
        for n in 1..=7u16 {
            let providers = nodes(7000, n);
            for max in 0..n as usize {
                for band in ["demo/echo", "route/optimize"] {
                    let view = shed_view(&all_shed(&providers), max, band);
                    let leaving = providers.iter().filter(|p| view.sheds(p)).count();
                    assert_eq!(leaving, providers.len() - max, "n={n} max={max} band={band}");
                }
            }
        }
        // A host that cannot see itself among the providers does not withdraw.
        let view = shed_view(&all_shed(&nodes(7100, 3)), 1, "demo/echo");
        assert!(!view.sheds(&mycelium::NodeId::new("127.0.0.1", 7199).unwrap()));
    }

    /// The first review of #547: a provider that does not shed — registered in code, or a stem without
    /// this band — must not hold a band above its ceiling. It does not advertise `prov-shed`, keeps its place,
    /// and the providers that do shed rank against what is left, in one round.
    #[test]
    fn a_provider_that_does_not_shed_does_not_hold_the_band_above_its_ceiling() {
        let all = nodes(7200, 4);
        for fixed_at in 0..all.len() {
            for (max, band) in [(2usize, "demo/echo"), (1, "route/optimize"), (3, "route/optimize")] {
                let entries: Vec<(mycelium::NodeId, bool)> =
                    all.iter().enumerate().map(|(i, n)| (n.clone(), i != fixed_at)).collect();
                let view = shed_view(&entries, max, band);
                let remaining: Vec<_> = all.iter().enumerate().filter(|(i, n)| *i == fixed_at || !view.sheds(n)).collect();
                assert_eq!(remaining.len(), max.max(1), "fixed at {fixed_at}, max {max}, band {band}");
                assert!(remaining.iter().any(|(i, _)| *i == fixed_at));
            }
        }
    }

    /// The reviews of #547: views differ — a crashed shedder still advertised, a peer's mark not yet
    /// arrived. The bound: the count stays at or above `max − (entries the view gets wrong)`, and it does
    /// not cascade — nothing accumulates across rounds, and a surviving host's rank only improves as others
    /// leave. Here two stale shedders ranked above everyone, and one missing mark, at a ceiling of 4 — so
    /// the bound is 1. (With as many wrong entries as the ceiling, the bound is 0: the band can empty for a
    /// round, and the floor refills it.)
    #[test]
    fn a_wrong_view_costs_at_most_its_wrong_entries_and_never_cascades() {
        let real = nodes(7300, 5);
        let max = 4;
        // Two crashed providers still advertised everywhere, ranked above every real one: search band names
        // until two of a pool rank first, so the worst placement is constructed rather than hoped for.
        let pool = nodes(7400, 40);
        let all: Vec<mycelium::NodeId> = real.iter().chain(pool.iter()).cloned().collect();
        let (band, ghosts) = (0..1000).find_map(|k| {
            let band = format!("demo/echo-{k}");
            let order = mycelium::election::rank(&band, &all, mycelium::election::Rule::Rendezvous);
            let top: Vec<mycelium::NodeId> = order.iter().take(2).map(|n| (*n).clone()).collect();
            top.iter().all(|n| pool.contains(n)).then_some((band, top))
        }).expect("some band ranks two pool nodes first");
        let band = band.as_str();
        assert_eq!(ghosts.len(), 2, "two ghosts rank above every real provider");
        let missing_mark = real[0].clone(); // every other node counts it as one that does not shed
        let wrong = ghosts.len() + 1;
        let mut live = real.clone();
        let mut history = Vec::new();
        for _round in 0..10 {
            let mut leaving = Vec::new();
            for host in &live {
                let entries: Vec<(mycelium::NodeId, bool)> = live.iter().chain(ghosts.iter())
                    .map(|n| (n.clone(), n == host || *n != missing_mark))
                    .collect();
                if entries.len() > max && shed_view(&entries, max, band).sheds(host) {
                    leaving.push(host.clone());
                }
            }
            live.retain(|n| !leaving.contains(n));
            history.push(live.len());
        }
        assert!(live.len() + wrong >= max, "never below max − wrong entries: {history:?}");
        assert!(history.iter().all(|c| *c == history[0]), "nothing changes after the first round: {history:?}");
    }

    /// The fourth review of #547: a herd — every stem installs at once (self_elect_p = 1.0, the band below
    /// its floor) — must land on the ceiling, not empty. The marks are present before the installs go live,
    /// so every host sees every other new provider as one that sheds, and ranks exactly. Had a new install
    /// been unmarked until its next round, each host would count the others fixed and every one withdraw.
    #[test]
    fn a_herd_of_new_installs_lands_on_the_ceiling() {
        for n in 2..=7u16 {
            let herd = nodes(7500, n);
            for max in 1..n as usize {
                // Marks exist from the band's first round: every provider is marked in every view.
                let marked: Vec<(mycelium::NodeId, bool)> = herd.iter().map(|h| (h.clone(), marks_band(false, false))).collect();
                let view = shed_view(&marked, max, "demo/echo");
                let left = herd.iter().filter(|h| !view.sheds(h)).count();
                assert_eq!(left, max, "n={n} max={max}");
            }
        }
        // The rule: marked unless providing without anything withdrawable.
        assert!(marks_band(false, false) && marks_band(true, true) && marks_band(false, true));
        assert!(!marks_band(true, false));
    }

    /// The fifth review of #547: a partition heals and the other side's `cap/` entries arrive before their
    /// marks. Counted fixed at once, they would push keep to 0 and empty this side; presumed to shed until
    /// unmarked for FIXED_AFTER, they rank with everyone and the band lands on its ceiling.
    #[test]
    fn a_partition_heals_without_emptying_the_band() {
        let a = nodes(7600, 2);
        let b = nodes(7610, 2);
        let all: Vec<mycelium::NodeId> = a.iter().chain(b.iter()).cloned().collect();
        let max = 2;
        // Side A's view just after the heal: its own providers marked, B's caps arrived, B's marks not yet.
        let entries = |unmarked_for: Duration| -> Vec<(mycelium::NodeId, bool)> {
            all.iter().map(|n| (n.clone(), presumed_sheds(a.contains(n), (!a.contains(n)).then_some(unmarked_for)))).collect()
        };
        let just_healed = shed_view(&entries(Duration::from_millis(400)), max, "demo/echo");
        let kept = all.iter().filter(|h| !just_healed.sheds(h)).count();
        assert_eq!(kept, max, "every provider ranks with every other: exactly the ceiling stays");
        // The same view, read as a truly fixed pair of peers long after: A then yields to them.
        let settled = shed_view(&entries(FIXED_AFTER), max, "demo/echo");
        assert!(a.iter().all(|h| settled.sheds(h)), "providers unmarked for FIXED_AFTER are fixed");
        assert!(presumed_sheds(true, None) && presumed_sheds(false, None) && presumed_sheds(false, Some(Duration::ZERO)));
        assert!(!presumed_sheds(false, Some(FIXED_AFTER)));
    }

    /// The bookkeeping behind FIXED_AFTER: an unmarked peer is timed from first sight, presumed to shed until
    /// FIXED_AFTER has passed, then fixed; a peer that gains its mark is dropped from the map, and one that
    /// loses it again starts over; a peer that stops providing is forgotten.
    #[test]
    fn an_unmarked_peer_is_timed_from_first_sight_and_reset_by_its_mark() {
        let me = mycelium::NodeId::new("127.0.0.1", 7700).unwrap();
        let peer = mycelium::NodeId::new("127.0.0.1", 7701).unwrap();
        let providers = vec![me.clone(), peer.clone()];
        let mut seen = HashMap::new();
        let t0 = 1_000_000_000u64;
        let fixed = FIXED_AFTER.as_nanos() as u64;
        let sheds = |seen: &mut HashMap<_, _>, shedders: &[mycelium::NodeId], now: u64| {
            rank_entries(&providers, shedders, &me, true, seen, now).0.into_iter().find(|(n, _)| *n == peer).unwrap().1
        };
        assert!(sheds(&mut seen, &[], t0), "first sight: presumed to shed");
        assert_eq!(seen.get(&peer), Some(&t0));
        assert!(sheds(&mut seen, &[], t0 + fixed - 1), "still within FIXED_AFTER");
        assert!(!sheds(&mut seen, &[], t0 + fixed), "unmarked for FIXED_AFTER: fixed");
        assert!(sheds(&mut seen, std::slice::from_ref(&peer), t0 + fixed + 1), "marked: sheds");
        assert!(!seen.contains_key(&peer), "a marked peer is dropped from the map");
        assert!(sheds(&mut seen, &[], t0 + 2 * fixed), "unmarked again: timed from the start");
        assert_eq!(seen.get(&peer), Some(&(t0 + 2 * fixed)));
        let (_, presumed) = rank_entries(&providers, &[], &me, true, &mut seen, t0 + 2 * fixed + 1);
        assert_eq!(presumed, 1);
        rank_entries(std::slice::from_ref(&me), &[], &me, true, &mut seen, t0 + 3 * fixed);
        assert!(seen.is_empty(), "a peer that stops providing is forgotten");
    }

    /// The third review of #547: two bands on one capability that differ in an attribute, a schema or the
    /// ceiling are different bands — so a node acting on one is never ranked as acting on the other.
    #[test]
    fn bands_that_differ_only_in_attributes_or_ceiling_have_different_shed_names() {
        let mut eu = CapFilter::new("route", "optimize");
        eu.attributes.insert("region".into(), mycelium::CapConstraint::Eq(CapValue::Text("eu".into())));
        let mut us = CapFilter::new("route", "optimize");
        us.attributes.insert("region".into(), mycelium::CapConstraint::Eq(CapValue::Text("us".into())));
        assert_ne!(shed_band(&eu, 2), shed_band(&us, 2));
        assert_ne!(shed_band(&eu, 2), shed_band(&eu, 3));
        assert_eq!(shed_band(&eu, 2), shed_band(&eu.clone(), 2));
        assert!(!shed_band(&CapFilter::new("tuple", "orders.primary"), 1).ends_with(".primary"),
            "never mistaken for a single-writer role by P10");
    }

    fn alloc_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    async fn live_agent() -> Arc<GossipAgent> {
        for _ in 0..16 {
            let port = alloc_port();
            let id = mycelium::NodeId::new("127.0.0.1", port).unwrap();
            let cfg = mycelium::GossipConfig { bind_port: port, ..Default::default() };
            let agent = Arc::new(GossipAgent::new(id, cfg));
            if agent.start().await.is_ok() {
                return agent;
            }
        }
        panic!("could not bind a gossip port after 16 attempts");
    }

    /// Structural poll: installs run as background tasks, so completion is awaited, not assumed.
    async fn wait_live(prov: &Provisioner, n: usize) {
        for _ in 0..200 {
            if prov.hosted_count() == n && prov.installing_count() == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!(
            "hosted_count did not settle at {n} (live={}, installing={})",
            prov.hosted_count(),
            prov.installing_count()
        );
    }

    // ── Item 4 PR 4c: the provisioner against the rights ledger ──────────────────────────────

    fn rights_ledger(tag: &str) -> RightsLedger {
        let dir = std::env::temp_dir().join(format!("myc-rights-{tag}-{}", alloc_port()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        RightsLedger::open(dir.join("rights.journal")).expect("ledger opens")
    }

    fn principal(s: &str) -> PrincipalId {
        PrincipalId::new(s).unwrap()
    }

    async fn declare_and_await_demand(agent: &GossipAgent, ns: &str, name: &str) -> mycelium::RequirementHandle {
        let req = agent
            .capabilities()
            .declare_requirement(CapFilter::new(ns, name), Duration::from_secs(30));
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new(ns, name)).demanding_nodes.is_empty() {
                return req;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("requirement {ns}/{name} did not register as demand");
    }

    /// Structural poll: the rejection is recorded off the admission path, so it is awaited.
    async fn wait_rejections(rights: &InstallRights, n: u64) {
        for _ in 0..200 {
            if rights.ledger.lock().await.view().rejections() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the ledger never recorded {n} rejection(s)");
    }

    /// No right allocated → the install is refused before any `Installing` reservation, the
    /// refusal is counted on the provisioner and **recorded** in the ledger, and the published
    /// head says the holder holds nothing.
    #[tokio::test]
    async fn an_install_is_refused_and_recorded_when_the_node_holds_no_rights() {
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));
        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.with_install_rights(rights_ledger("none"), principal("node-a"), "installs", None);
        agent.set_control_profile(mycelium::control::Profile::EnforceAllocated);

        let _req = declare_and_await_demand(&agent, "text", "echo").await;
        assert_eq!(prov.provision_round(), 0, "no rights, no install");
        assert_eq!(prov.installing_count(), 0, "refused before the reservation, not after");
        assert_eq!(prov.rights_refusals(), 1);

        let rights = Arc::clone(prov.install_rights.as_ref().unwrap());
        wait_rejections(&rights, 1).await;

        let key = format!("{}node-a", mycelium::signal::kv_ns::RIGHTS_HEAD);
        let published = PublishedRightsHead::decode(&agent.kv().get(&key).expect("head published"))
            .expect("head decodes");
        assert_eq!(published.head.holder, principal("node-a"));
        assert!(published.head.totals.is_empty(), "nothing held: {:?}", published.head.totals);
        assert!(!published.is_signed(), "no key was given, so the head is a claim without proof");
        assert!(!verify_published_head(&published, &[0u8; 32]), "unsigned never verifies");
    }

    /// ADR §7, shadow before enforcement: with no rights allocated and the node's profile left at
    /// its default (`Legacy`), the install is **admitted**, the would-refuse tripwire counts it, and
    /// the ledger still records the rejection — the number an operator watches before opting into
    /// `EnforceAllocated`. `Observe` and `EnforceLocal` behave the same for a Tier C bound.
    #[tokio::test]
    async fn without_enforce_allocated_a_rights_shortfall_is_counted_and_recorded_but_admitted() {
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));
        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.with_install_rights(rights_ledger("shadow"), principal("node-s"), "installs", None);
        assert_eq!(agent.control_profile(), mycelium::control::Profile::Legacy, "the default");

        let _req = declare_and_await_demand(&agent, "text", "echo").await;
        assert_eq!(prov.provision_round(), 1, "admitted under a non-enforcing profile");
        assert_eq!((prov.rights_refusals(), prov.rights_would_refuse()), (0, 1));
        let rights = Arc::clone(prov.install_rights.as_ref().unwrap());
        wait_rejections(&rights, 1).await; // recorded all the same
        wait_live(&prov, 1).await;
    }

    /// One unit allocated → one install proceeds, the second is refused; the head carries the
    /// allocation and verifies under the holder's key. Rights bound *concurrency*, and a live
    /// install is as consumed as one in flight.
    #[tokio::test]
    async fn installs_are_bounded_by_the_allocated_units_and_the_head_is_signed() {
        use ed25519_dalek::SigningKey;
        use mycelium::control::ledger::{Right, RightState};
        use mycelium::mandate::TermId;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let mut source = InMemorySource::new();
        let id_a = source.insert(ECHO_COMPONENT.to_vec());
        // A second, different, valid component — whichever the round picks first installs, the
        // other is refused; the test does not depend on the catalog's order.
        let id_b = source.insert(include_bytes!("../tests/fixtures/unit_convert_component.wasm").to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id_a));
        catalog.add(InstallableEntry::new(Capability::new("text", "echo2"), id_b));

        let mut ledger = rights_ledger("one");
        ledger
            .allocate(Right {
                holder: principal("node-b"),
                resource: "installs".into(),
                units: 1,
                allocated_by: principal("operator"),
                term: TermId::new("t1").unwrap(),
                state: RightState::Serving,
                valid_until_ms: u64::MAX,
            })
            .await
            .expect("allocated");
        let signing = SigningKey::from_bytes(&[7u8; 32]);
        let public = signing.verifying_key().to_bytes();

        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.with_install_rights(ledger, principal("node-b"), "installs", Some(signing));
        agent.set_control_profile(mycelium::control::Profile::EnforceAllocated);

        let _r1 = declare_and_await_demand(&agent, "text", "echo").await;
        let _r2 = declare_and_await_demand(&agent, "text", "echo2").await;
        assert_eq!(prov.provision_round(), 1, "one unit held: exactly one install starts");
        assert_eq!(prov.rights_refusals(), 1, "the second was refused");
        let rights = Arc::clone(prov.install_rights.as_ref().unwrap());
        wait_rejections(&rights, 1).await;

        let key = format!("{}node-b", mycelium::signal::kv_ns::RIGHTS_HEAD);
        let published = PublishedRightsHead::decode(&agent.kv().get(&key).expect("head published"))
            .expect("head decodes");
        assert_eq!(published.head.totals, vec![("installs".to_string(), 1)]);
        assert!(published.is_signed());
        assert!(verify_published_head(&published, &public), "signed by the holder's key");
        assert!(!verify_published_head(&published, &[9u8; 32]), "and by no other");

        // A live install still consumes the unit: a later round refuses again rather than
        // starting the second artifact once the first is serving.
        wait_live(&prov, 1).await;
        assert_eq!(prov.provision_round(), 0, "the one unit is consumed by the live install");
        assert_eq!(prov.rights_refusals(), 2);
    }

    #[tokio::test]
    async fn provisions_an_unmet_requirement_then_stops_once_satisfied() {
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        // Catalog: installing this artifact would provide text/echo.
        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));

        let mut prov = Provisioner::new(
            Arc::clone(&agent),
            host,
            catalog,
            Arc::new(source),
            1.0, // single provisioner: always self-elect
        );

        // No requirement yet → nothing to provision.
        assert_eq!(prov.provision_round(), 0, "no demand, no provisioning");

        // Declare a requirement for text/echo → demand with no provider.
        let _req = agent
            .capabilities()
            .declare_requirement(CapFilter::new("text", "echo"), Duration::from_secs(30));
        // Let the req/ write land.
        let mut saw_demand = false;
        for _ in 0..40 {
            let d = agent.capabilities().demand(&CapFilter::new("text", "echo"));
            if !d.demanding_nodes.is_empty() {
                saw_demand = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_demand, "requirement should register as demand");

        // The provisioner satisfies it: starts the install (a background task), which pulls +
        // instantiates + advertises on completion.
        assert_eq!(prov.provision_round(), 1, "unmet requirement should start an install");
        wait_live(&prov, 1).await;

        // The advertisement relieves demand — a provider now exists.
        let mut saw_provider = false;
        for _ in 0..40 {
            let d = agent.capabilities().demand(&CapFilter::new("text", "echo"));
            if !d.providers.is_empty() {
                saw_provider = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_provider, "provisioned capability should advertise itself");

        // Idempotent: a second round provisions nothing (already hosted + now has a provider).
        assert_eq!(prov.provision_round(), 0, "satisfied requirement is not re-provisioned");
        assert_eq!(prov.hosted_count(), 1);

        // The provisioned capability is callable: an inbound RPC reaches the component's `handle`.
        let reply = agent
            .service()
            .rpc_call(
                agent.node_id().clone(),
                crate::runtime::cap_invoke_kind("text", "echo"),
                b"call me".to_vec(),
                Duration::from_secs(5),
            )
            .await
            .expect("served capability replies");
        assert_eq!(reply.as_ref(), b"call me", "the hosted component echoed the invocation");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn supervision_brings_a_capability_live_with_no_demander() {
        // M14: a presence invariant ("keep >=1 provider of text/echo alive") provisions the
        // capability with NO organic demand — and because a restart re-runs this same path when a
        // dead provider's cap/ evaporates, restart and first-time provisioning are identical.
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);

        // No demand and no policy yet → nothing happens.
        assert_eq!(prov.provision_round(), 0, "no demand, no policy");

        // Supervise: keep >=1 provider of text/echo alive.
        prov.supervise(CapFilter::new("text", "echo"), 1);

        // The presence invariant alone (no demander) brings the capability live.
        assert_eq!(prov.provision_round(), 1, "presence invariant provisions with no demander");
        wait_live(&prov, 1).await;

        // It actually serves invocations.
        let reply = agent
            .service()
            .rpc_call(
                agent.node_id().clone(),
                crate::runtime::cap_invoke_kind("text", "echo"),
                b"supervised".to_vec(),
                Duration::from_secs(5),
            )
            .await
            .expect("served");
        assert_eq!(reply.as_ref(), b"supervised");

        // Invariant satisfied (a live provider exists) → subsequent rounds are no-ops.
        let mut saw_provider = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("text", "echo")).providers.is_empty() {
                saw_provider = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_provider, "supervised capability advertises a provider");
        assert_eq!(prov.provision_round(), 0, "invariant satisfied → no re-provisioning");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn track2b_sheds_a_provider_when_over_max() {
        // Track 2b: the symmetric shed path. Bring a capability live, then a band whose `max` is
        // below the live count makes the hosting node ranked beyond it withdraw (tombstone + stop
        // serving). (Single-node: max=0 forces shed of the one local provider — the cross-node
        // case is the identical code path against real provider counts.)
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);

        // Bring it live via a min>=1 invariant.
        prov.supervise(CapFilter::new("text", "echo"), 1);
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;

        // Wait until the provider is observable (the shed decision is based on the live count).
        let mut saw_provider = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("text", "echo")).providers.is_empty() {
                saw_provider = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_provider, "provider must be observable before it can be shed");

        // Replace the policy with a band that wants *fewer* than are live (max=0) → shed.
        prov.policies.clear();
        prov.supervise_band(CapFilter::new("text", "echo"), 0, 0);
        prov.provision_round(); // shed pass withdraws the local provider
        assert_eq!(prov.hosted_count(), 0, "over-max provider is withdrawn");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn require_provenance_refuses_unsigned_but_installs_trusted_signed() {
        use crate::catalog::InstallableEntry;
        use ed25519_dalek::SigningKey;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let sk = SigningKey::from_bytes(&[42u8; 32]);
        let trusted = sk.verifying_key().to_bytes();

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let source: Arc<dyn crate::ArtifactSource + Send + Sync> = Arc::new(source);

        // Unsigned catalog entry + provenance required → refused.
        let mut unsigned_cat = InstallableCatalog::new();
        unsigned_cat.add(InstallableEntry::new(Capability::new("text", "echo"), id));
        let mut p1 = Provisioner::new(
            Arc::clone(&agent), Arc::clone(&host), unsigned_cat, Arc::clone(&source), 1.0);
        p1.require_provenance(vec![trusted]);
        p1.supervise(CapFilter::new("text", "echo"), 1);
        assert_eq!(p1.provision_round(), 0, "unsigned artifact refused under provenance policy");
        assert_eq!(p1.hosted_count(), 0);

        // Signed by the trusted publisher → installed.
        let mut signed_cat = InstallableCatalog::new();
        signed_cat.add(InstallableEntry::new(Capability::new("text", "echo"), id).signed_by(&sk));
        let mut p2 = Provisioner::new(Arc::clone(&agent), host, signed_cat, source, 1.0);
        p2.require_provenance(vec![trusted]);
        p2.supervise(CapFilter::new("text", "echo"), 1);
        assert_eq!(p2.provision_round(), 1, "trusted-signed artifact starts installing");
        wait_live(&p2, 1).await;

        agent.shutdown().await;
    }

    /// D19's gate (plan F1): an entry signed by an agent principal that loops is stopped at the
    /// budget and the record says so by name; the operator's entry runs unbounded to completion
    /// under the same policy. (That the *budget*, not the component, decides is the host-level
    /// test `a_metered_host_gives_each_instance_its_own_budget` — one node hosts one content
    /// address once, so the same bytes cannot be catalogued twice here.) Seen failing first with
    /// the budget applied uniformly: the operator's echo stopped too.
    #[tokio::test]
    async fn an_agent_published_entry_that_loops_is_stopped_at_its_budget_and_the_operators_is_not() {
        use crate::catalog::InstallableEntry;
        use crate::runtime::{FuelPolicy, InvocationOutcome};
        use ed25519_dalek::SigningKey;
        const SPIN_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/spin_component.wasm");
        const AGENT_BUDGET: u64 = 1_000;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::metered().expect("engine"));
        let operator = SigningKey::from_bytes(&[11u8; 32]);
        let author = SigningKey::from_bytes(&[12u8; 32]);
        let operator_key = operator.verifying_key().to_bytes();
        let author_key = author.verifying_key().to_bytes();

        let mut source = InMemorySource::new();
        let spin = source.insert(SPIN_COMPONENT.to_vec());
        let echo = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        // The agent's loop and the operator's echo.
        catalog.add(InstallableEntry::new(Capability::new("text", "spin"), spin).signed_by(&author));
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), echo).signed_by(&operator));

        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.require_provenance(vec![operator_key, author_key]);
        prov.set_fuel_policy(FuelPolicy {
            operator_publishers: vec![operator_key],
            agent_budget: Some(AGENT_BUDGET),
            operator_budget: None,
        });
        for name in ["spin", "echo"] {
            prov.supervise(CapFilter::new("text", name), 1);
        }
        for _ in 0..40 {
            prov.provision_round();
            if prov.hosted_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        wait_live(&prov, 2).await;

        let call = |name: &'static str| {
            let agent = Arc::clone(&agent);
            async move {
                agent
                    .service()
                    .rpc_call(agent.node_id().clone(), crate::runtime::cap_invoke_kind("text", name),
                        b"payload".to_vec(), Duration::from_secs(10))
                    .await
                    .expect("the serve loop answers")
            }
        };
        let spun = String::from_utf8_lossy(&call("spin").await).into_owned();
        assert!(spun.contains("fuel exhausted") && spun.contains(&AGENT_BUDGET.to_string()),
            "the agent's loop is stopped at its budget and the reply names it: {spun}");
        assert_eq!(call("echo").await.as_ref(), b"payload", "the operator's echo runs unbounded to completion");

        let outcomes: Vec<(String, InvocationOutcome)> =
            prov.invocations().into_iter().map(|r| (r.provides, r.outcome)).collect();
        assert_eq!(outcomes, vec![
            ("text/spin".to_string(), InvocationOutcome::FuelExhausted { budget: AGENT_BUDGET }),
            ("text/echo".to_string(), InvocationOutcome::Completed),
        ], "the execution record names the stop and the budget");
        let signers: Vec<Vec<u8>> = prov.invocations().into_iter().map(|r| r.signer).collect();
        assert_eq!(signers, vec![author_key.to_vec(), operator_key.to_vec()]);

        // Still live: a stopped call is a stopped call, not a dead install — the trapped instance
        // is replaced (wasmtime refuses to re-enter one), and the next call gets a fresh budget.
        // Found by this gate: before the replacement, the second call answered *cannot enter
        // component instance*, a dead install behind a live advertisement.
        assert_eq!(prov.hosted_count(), 2);
        let again = String::from_utf8_lossy(&call("spin").await).into_owned();
        assert!(again.contains("fuel exhausted"), "{again}");
        agent.shutdown().await;
    }

    /// Zero-gaps Z4 (D4): a `tool/{name}` component that answers `describe` publishes **its own**
    /// input schema and description as the MCP tool, not the bridge's generic `{"type":"object"}` —
    /// so an agent reading `tools/{name}/{node}` learns the arguments. Seen failing first: the bridge
    /// published the generic schema whatever the component said.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn a_tool_component_publishes_its_own_schema() {
        use crate::catalog::InstallableEntry;
        use ed25519_dalek::SigningKey;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let key = SigningKey::from_bytes(&[41u8; 32]);
        let mut source = InMemorySource::new();
        let id = source.insert(include_bytes!("../tests/fixtures/calculate_tool_component.wasm").to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("tool", "calculate"), id).signed_by(&key));
        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let _req = agent.capabilities().declare_requirement(CapFilter::new("tool", "calculate"), Duration::from_secs(30));
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("tool", "calculate")).demanding_nodes.is_empty() { break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;

        let key = format!("tools/calculate/{}", agent.node_id());
        let mut schema = None;
        for _ in 0..80 {
            if let Some(v) = agent.kv().get(&key) { schema = serde_json::from_slice::<serde_json::Value>(&v).ok(); break; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let schema = schema.expect("the bridged tool is in KV");
        assert_eq!(schema["inputSchema"]["properties"]["expression"]["type"], "string", "the component's own schema: {schema}");
        assert!(schema["description"].as_str().unwrap_or("").contains("arithmetic"), "the component's own description: {schema}");

        // And the bridge still answers a call with the component's arithmetic.
        let call = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "calculate", "arguments": {"expression": "330 * 1024"}}});
        let reply = agent.service().rpc_call(agent.node_id().clone(), "mcp.invoke", call.to_string().into_bytes(), Duration::from_secs(10)).await.expect("the tool answers over mcp.invoke");
        let resp: serde_json::Value = serde_json::from_slice(&reply).unwrap();
        let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("").to_string();
        assert!(text.contains("337920"), "330 × 1024 from inside the component: {resp}");
        agent.shutdown().await;
    }

    /// D20's gate (plan F2): a proposed entry loads only into the shadow lane — resolvable as
    /// `{name}.shadow` for comparison, never by the incumbent's filter — a stranger's acceptance
    /// changes nothing, and a listed reviewer's acceptance promotes it. Seen failing first: with
    /// no D20 the proposal installed for real on the first round.
    #[tokio::test]
    async fn a_proposed_entry_loads_only_into_the_shadow_lane_until_a_listed_reviewer_accepts_it() {
        use crate::catalog::InstallableEntry;
        use ed25519_dalek::SigningKey;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let author = SigningKey::from_bytes(&[31u8; 32]);
        let reviewer = SigningKey::from_bytes(&[32u8; 32]);
        let stranger = SigningKey::from_bytes(&[33u8; 32]);

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let proposed = InstallableEntry::new(Capability::new("text", "echo"), id).as_proposed().signed_by(&author);
        let mut catalog = InstallableCatalog::new();
        catalog.add(proposed.clone());

        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.require_provenance(vec![author.verifying_key().to_bytes()]);
        prov.require_reviewers(vec![reviewer.verifying_key().to_bytes()]);

        let _req = agent.capabilities().declare_requirement(CapFilter::new("text", "echo"), Duration::from_secs(30));
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("text", "echo")).demanding_nodes.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Round 1: the proposal shadows — live, but not as text/echo.
        assert_eq!(prov.provision_round(), 1, "the proposal loads into the shadow lane");
        wait_live(&prov, 1).await;
        assert_eq!(prov.shadow_count(), 1);
        let mut shadow_seen = false;
        for _ in 0..40 {
            if !agent.capabilities().resolve(&CapFilter::new("text", "echo.shadow")).is_empty() {
                shadow_seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(shadow_seen, "the shadow is resolvable by its own name, for comparison");
        assert!(agent.capabilities().resolve(&CapFilter::new("text", "echo")).is_empty(),
            "the incumbent's filter never resolves a shadow");
        let reply = agent.service()
            .rpc_call(agent.node_id().clone(), crate::runtime::cap_invoke_kind("text", "echo.shadow"),
                b"compare me".to_vec(), Duration::from_secs(5))
            .await.expect("the shadow answers a call made to it by name");
        assert_eq!(reply.as_ref(), b"compare me");
        assert_eq!(prov.provision_round(), 0, "a shadow is not re-installed and takes no demand");

        // A stranger's acceptance changes nothing.
        let mut cat = InstallableCatalog::new();
        cat.add(proposed.clone().accepted_by(&stranger).unwrap());
        prov.refresh_catalog(cat);
        assert_eq!(prov.provision_round(), 0);
        assert_eq!(prov.shadow_count(), 1, "still a shadow under a forged acceptance");
        assert!(agent.capabilities().resolve(&CapFilter::new("text", "echo")).is_empty());

        // The listed reviewer's acceptance promotes it: the shadow is withdrawn, then the entry
        // loads for real under its own name and takes the demand.
        let mut cat = InstallableCatalog::new();
        cat.add(proposed.clone().accepted_by(&reviewer).unwrap());
        prov.refresh_catalog(cat);
        let mut promoted = false;
        for _ in 0..40 {
            prov.provision_round();
            if prov.shadow_count() == 0 && prov.hosted_count() == 1 {
                promoted = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(promoted, "accepted: withdrawn from the shadow lane and loaded for real");
        let mut real_seen = false;
        for _ in 0..40 {
            if !agent.capabilities().resolve(&CapFilter::new("text", "echo")).is_empty() {
                real_seen = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(real_seen, "after acceptance the entry takes demand under its own name");
        let reply = agent.service()
            .rpc_call(agent.node_id().clone(), crate::runtime::cap_invoke_kind("text", "echo"),
                b"for real".to_vec(), Duration::from_secs(5))
            .await.expect("serves");
        assert_eq!(reply.as_ref(), b"for real");

        agent.shutdown().await;
    }

    /// X2 (the tool-growth demo's tool-host, as a stem): an installed `tool/{name}` component is
    /// bridged as MCP tool `{name}` — `tools/{name}/{node}` appears in KV and a `tools/call` over
    /// `mcp.invoke` reaches the component. Seen failing first: with no bridge the KV key never
    /// appeared.
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn an_installed_tool_component_is_bridged_as_an_mcp_tool() {
        use crate::catalog::InstallableEntry;
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("tool", "echo"), id));
        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.supervise(CapFilter::new("tool", "echo"), 1);
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;

        let key = format!("tools/echo/{}", agent.node_id());
        let mut registered = false;
        for _ in 0..40 {
            if agent.kv().get(&key).is_some() {
                registered = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(registered, "the bridge registered {key}");

        let call = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "echo", "arguments": {"kg": 5000.0}},
        });
        let reply = agent.service()
            .rpc_call(agent.node_id().clone(), mycelium::signal_kind::MCP_INVOKE, call.to_string().into_bytes(), Duration::from_secs(10))
            .await.expect("mcp.invoke answers");
        let resp: serde_json::Value = serde_json::from_slice(&reply).expect("a JSON-RPC reply");
        let text = resp["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(text.contains("5000"), "the echo component's output came back through the MCP door: {resp}");

        // Shed it (after the advertisement is observable, as the lifecycle test does) and the
        // tool goes with the install.
        let mut observable = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("tool", "echo")).providers.is_empty() {
                observable = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(observable);
        prov.policies.clear();
        prov.supervise_band(CapFilter::new("tool", "echo"), 0, 0);
        prov.provision_round();
        assert_eq!(prov.hosted_count(), 0);
        agent.shutdown().await;
    }

    /// A probe reporting fixed numbers — resource-eligibility tests must not depend on the
    /// machine they run on.
    struct FixedProbe {
        mem:  u64,
        disk: u64,
    }
    impl ResourceProbe for FixedProbe {
        fn available_memory_bytes(&self) -> Option<u64> {
            Some(self.mem)
        }
        fn available_disk_bytes(&self, _at: &std::path::Path) -> Option<u64> {
            Some(self.disk)
        }
    }

    /// A Blob runtime whose installs block on a semaphore — lets a test hold an install
    /// in-flight while asserting what a concurrent eligibility check sees.
    struct GatedRuntime {
        gate: Arc<tokio::sync::Semaphore>,
    }
    struct NoopInstalled;
    impl Installed for NoopInstalled {
        fn probe(&self) -> bool {
            true
        }
        fn uninstall(self: Box<Self>) {}
    }
    #[async_trait::async_trait]
    impl ArtifactRuntime for GatedRuntime {
        fn kind(&self) -> ArtifactKind {
            ArtifactKind::Blob
        }
        async fn install(
            &self,
            _entry: InstallableEntry,
            _source: Arc<dyn ArtifactSource + Send + Sync>,
            _ctx: RuntimeCtx,
            _progress: ProgressFn,
        ) -> Result<Box<dyn Installed>, crate::runtime::InstallError> {
            let permit = self
                .gate
                .acquire()
                .await
                .map_err(|e| crate::runtime::InstallError::Host(e.to_string()))?;
            permit.forget();
            Ok(Box::new(NoopInstalled))
        }
    }

    #[tokio::test]
    async fn resource_requirements_gate_election_and_count_inflight_reservations() {
        // §4.4: a node must not elect for an artifact it cannot fit — and "fit" must account
        // for installs already in flight (two 6 GB models must not both pass a 10 GB check).
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let a = source.insert(b"model-a".to_vec());
        let b = source.insert(b"model-b".to_vec());
        let c = source.insert(b"model-c".to_vec());

        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("llm", "model-a"), a)
            .with_kind(ArtifactKind::Blob).with_requirements(0, 6_000));
        catalog.add(InstallableEntry::new(Capability::new("llm", "model-b"), b)
            .with_kind(ArtifactKind::Blob).with_requirements(0, 6_000));
        catalog.add(InstallableEntry::new(Capability::new("llm", "model-c"), c)
            .with_kind(ArtifactKind::Blob).with_requirements(0, 12_000)); // never fits alone

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        prov.register_runtime(Arc::new(GatedRuntime { gate: Arc::clone(&gate) }));
        prov.set_resource_policy(Arc::new(FixedProbe { mem: 10_000, disk: 10_000 }), 1.0);
        prov.supervise(CapFilter::new("llm", "model-a"), 1);
        prov.supervise(CapFilter::new("llm", "model-b"), 1);
        prov.supervise(CapFilter::new("llm", "model-c"), 1);

        // Round 1: model-a starts (reserves 6_000); model-b would joint-exceed (6k + 6k > 10k)
        // and is skipped; model-c exceeds alone (12k > 10k) and is skipped.
        assert_eq!(prov.provision_round(), 1, "only the first model fits");
        assert_eq!(prov.installing_count(), 1);
        assert_eq!(prov.ineligible_skips(), 2, "joint-exceed + exceeds-alone each ticked");

        // While model-a is still installing, nothing changes on a re-round.
        assert_eq!(prov.provision_round(), 0, "reservation still holds the headroom");
        assert!(prov.ineligible_skips() >= 3, "model-b (and c) keep ticking while blocked");

        // Let model-a finish: its reservation clears (real consumption is the probe's business,
        // and this probe is fixed) → model-b now fits; model-c still never does.
        gate.add_permits(1);
        wait_live(&prov, 1).await;
        assert_eq!(prov.provision_round(), 1, "freed headroom admits the second model");
        gate.add_permits(1);
        wait_live(&prov, 2).await;
        assert_eq!(prov.provision_round(), 0, "model-c can never fit on this node");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn blob_kind_installs_via_a_registered_runtime() {
        // The kind-dispatch acceptance path: a Blob entry, a registered BlobRuntime, and the
        // same demand/presence machinery — the artifact ends up *placed* (not instantiated)
        // and its capability advertised.
        use crate::artifact::FsLibrarySource;
        use crate::runtime::BlobRuntime;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "mycelium-prov-blob-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let lib_dir = base.join("library");
        let place_dir = base.join("models");

        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let payload: Vec<u8> = (0..100u8).collect();
        let id = lib.store(&payload).unwrap();

        let mut catalog = InstallableCatalog::new();
        catalog.add(
            InstallableEntry::new(Capability::new("llm", "weights"), id)
                .with_kind(ArtifactKind::Blob)
                .with_cost(payload.len() as u64, 1),
        );

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, lib as Arc<_>, 1.0);
        prov.register_runtime(Arc::new(BlobRuntime::new(&place_dir).with_chunk_bytes(16)));
        prov.supervise(CapFilter::new("llm", "weights"), 1);

        assert_eq!(prov.provision_round(), 1, "blob install starts");
        wait_live(&prov, 1).await;

        // Placed (streamed from the ranged library source), content-correct.
        assert_eq!(std::fs::read(place_dir.join(id.to_hex())).unwrap(), payload);

        // And advertised like any provisioned capability.
        let mut saw_provider = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("llm", "weights")).providers.is_empty() {
                saw_provider = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(saw_provider, "placed blob's capability is advertised");

        agent.shutdown().await;
        std::fs::remove_dir_all(&base).ok();
    }

    /// An `Installed` that records its own teardown.
    struct TrackingInstalled {
        uninstalled: Arc<std::sync::atomic::AtomicBool>,
    }
    impl Installed for TrackingInstalled {
        fn probe(&self) -> bool {
            true
        }
        fn uninstall(self: Box<Self>) {
            self.uninstalled.store(true, Ordering::SeqCst);
        }
    }

    /// A Blob-kind runtime for exercising the provisioner's concurrency paths: optionally
    /// blocks on a semaphore mid-install, optionally fails the first N installs, and returns
    /// a teardown-tracking handle.
    struct TrackingRuntime {
        gate:           Option<Arc<tokio::sync::Semaphore>>,
        fail_remaining: Arc<AtomicU64>,
        uninstalled:    Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl ArtifactRuntime for TrackingRuntime {
        fn kind(&self) -> ArtifactKind {
            ArtifactKind::Blob
        }
        async fn install(
            &self,
            _entry: InstallableEntry,
            _source: Arc<dyn ArtifactSource + Send + Sync>,
            _ctx: RuntimeCtx,
            _progress: ProgressFn,
        ) -> Result<Box<dyn Installed>, crate::runtime::InstallError> {
            if let Some(gate) = &self.gate {
                let permit = gate
                    .acquire()
                    .await
                    .map_err(|e| crate::runtime::InstallError::Host(e.to_string()))?;
                permit.forget();
            }
            if self
                .fail_remaining
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(crate::runtime::InstallError::Fetch("transient install failure".into()));
            }
            Ok(Box::new(TrackingInstalled { uninstalled: Arc::clone(&self.uninstalled) }))
        }
    }

    #[tokio::test]
    async fn wasm_component_full_lifecycle_install_invoke_shed_reinstall() {
        // The WasmComponent kind through its whole life via the provisioner: supervision
        // installs → the capability serves real invocations → the shed band withdraws it →
        // re-supervising reinstalls the SAME artifact and it serves again (restart ≡
        // provisioning as a test, not just a demo).
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);

        // Act 1 — install + serve.
        prov.supervise(CapFilter::new("text", "echo"), 1);
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;
        let reply = agent.service()
            .rpc_call(agent.node_id().clone(), crate::runtime::cap_invoke_kind("text", "echo"),
                b"first life".to_vec(), Duration::from_secs(5))
            .await.expect("serves");
        assert_eq!(reply.as_ref(), b"first life");

        // Act 2 — shed (cooperative self-removal).
        let mut observable = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("text", "echo")).providers.is_empty() {
                observable = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(observable);
        prov.policies.clear();
        prov.supervise_band(CapFilter::new("text", "echo"), 0, 0);
        prov.provision_round();
        assert_eq!(prov.hosted_count(), 0, "shed withdrew the component");

        // Act 3 — reinstall: the same demand machinery brings the same artifact back.
        prov.policies.clear();
        prov.supervise(CapFilter::new("text", "echo"), 1);
        let mut restarted = 0;
        for _ in 0..40 {
            restarted += prov.provision_round();
            if prov.hosted_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(restarted >= 1, "re-supervision reinstalls");
        wait_live(&prov, 1).await;
        let reply = agent.service()
            .rpc_call(agent.node_id().clone(), crate::runtime::cap_invoke_kind("text", "echo"),
                b"second life".to_vec(), Duration::from_secs(5))
            .await.expect("serves after reinstall");
        assert_eq!(reply.as_ref(), b"second life");

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn blob_full_lifecycle_install_probe_selfheal_and_shed() {
        // The Blob kind through its whole life: supervision installs (streamed from the
        // library, placed on disk, advertised) → the placed file is destroyed out-of-band →
        // the probe pass catches it and the SAME round reinstalls (probe-gated self-heal) →
        // the shed band withdraws it and the placed file is actually deleted.
        use crate::artifact::FsLibrarySource;
        use crate::runtime::BlobRuntime;

        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "mycelium-blob-lifecycle-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let (lib_dir, place_dir) = (base.join("library"), base.join("models"));

        let lib = Arc::new(FsLibrarySource::open(&lib_dir).unwrap());
        let payload: Vec<u8> = (0..200u8).cycle().take(500).collect();
        let id = lib.store(&payload).unwrap();
        let placed = place_dir.join(id.to_hex());

        let mut catalog = InstallableCatalog::new();
        catalog.add(
            InstallableEntry::new(Capability::new("llm", "weights"), id)
                .with_kind(ArtifactKind::Blob)
                .with_cost(payload.len() as u64, 1),
        );

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, lib as Arc<_>, 1.0);
        prov.register_runtime(Arc::new(BlobRuntime::new(&place_dir).with_chunk_bytes(64)));

        // Act 1 — install: streamed, placed, advertised.
        prov.supervise(CapFilter::new("llm", "weights"), 1);
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;
        assert_eq!(std::fs::read(&placed).unwrap(), payload);
        let mut observable = false;
        for _ in 0..40 {
            if !agent.capabilities().demand(&CapFilter::new("llm", "weights")).providers.is_empty() {
                observable = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(observable, "placed blob advertises");

        // Act 2 — sabotage: the placed file vanishes. The default probe (file exists) fails,
        // the probe pass withdraws, and supervision reinstalls from the library once the
        // retracted ad clears the local view (content-addressed placement makes the re-pull
        // honest: dest is gone). Structural poll over rounds — the tombstone lands async.
        std::fs::remove_file(&placed).unwrap();
        let mut restarted = 0;
        for _ in 0..80 {
            restarted += prov.provision_round();
            if restarted >= 1 && prov.hosted_count() == 1 && prov.installing_count() == 0
                && placed.exists()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(restarted >= 1, "probe failure leads to a reinstall");
        wait_live(&prov, 1).await;
        assert_eq!(std::fs::read(&placed).unwrap(), payload, "self-healed: file re-placed");

        // Act 3 — shed: cooperative self-removal deletes the placed bytes.
        prov.policies.clear();
        prov.supervise_band(CapFilter::new("llm", "weights"), 0, 0);
        prov.provision_round();
        assert_eq!(prov.hosted_count(), 0, "over-max blob is withdrawn");
        assert!(!placed.exists(), "uninstall deleted the placed blob");

        agent.shutdown().await;
        std::fs::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn failed_install_drops_the_reservation_and_a_later_round_retries() {
        // The Err path of start_install: a transient install failure must remove the
        // Installing reservation (not leave a zombie that blocks forever), and the next
        // round retries the same artifact to success — restart ≡ provisioning.
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(b"flaky model".to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("llm", "flaky"), id)
            .with_kind(ArtifactKind::Blob));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let uninstalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        prov.register_runtime(Arc::new(TrackingRuntime {
            gate:           None,
            fail_remaining: Arc::new(AtomicU64::new(1)), // first install fails
            uninstalled:    Arc::clone(&uninstalled),
        }));
        prov.supervise(CapFilter::new("llm", "flaky"), 1);

        assert_eq!(prov.provision_round(), 1, "first attempt starts");
        // The failure clears the reservation…
        let mut cleared = false;
        for _ in 0..200 {
            if prov.installing_count() == 0 && prov.hosted_count() == 0 {
                cleared = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(cleared, "failed install drops the Installing reservation");

        // …so the next round retries, and this one sticks.
        assert_eq!(prov.provision_round(), 1, "retry starts");
        wait_live(&prov, 1).await;

        agent.shutdown().await;
    }

    #[tokio::test]
    async fn withdraw_during_install_tears_down_the_stale_result() {
        // The token-check path: an install withdrawn while in flight must, on completion,
        // find its reservation gone and tear its fresh result down (advertisement dropped,
        // Installed::uninstall called) — never resurrect itself into the map.
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(b"withdrawn mid-flight".to_vec());
        let artifact = id;
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("llm", "midflight"), id)
            .with_kind(ArtifactKind::Blob));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let uninstalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        prov.register_runtime(Arc::new(TrackingRuntime {
            gate:           Some(Arc::clone(&gate)),
            fail_remaining: Arc::new(AtomicU64::new(0)),
            uninstalled:    Arc::clone(&uninstalled),
        }));
        prov.supervise(CapFilter::new("llm", "midflight"), 1);

        assert_eq!(prov.provision_round(), 1);
        assert_eq!(prov.installing_count(), 1, "install is in flight, blocked on the gate");

        // Withdraw while installing (in the fleet this happens when another provider makes
        // the band's live count exceed max while ours is still pulling).
        assert!(prov.withdraw(&artifact), "reservation withdrawn mid-install");
        assert_eq!(prov.installing_count(), 0);

        // Let the install finish: the completion's token check must tear it down.
        gate.add_permits(1);
        let mut torn_down = false;
        for _ in 0..200 {
            if uninstalled.load(std::sync::atomic::Ordering::SeqCst) {
                torn_down = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(torn_down, "the stale install was explicitly uninstalled");
        assert_eq!(prov.hosted_count(), 0, "it never resurrected into the hosted map");
        assert_eq!(prov.installing_count(), 0);

        agent.shutdown().await;
    }

    /// M2 Run-38 falsification probe (Resource Management), kept: an agent shut down while an
    /// install is still in flight must stay harmless — the detached install task completes
    /// against the dead agent, its advertisement is a no-op write, nothing panics, and the
    /// provisioner's state stays coherent (the install may land Live in the map — the map is
    /// node-local bookkeeping; the node itself is gone).
    #[tokio::test]
    async fn agent_shutdown_mid_install_is_harmless() {
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(b"shutdown mid-flight".to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("llm", "shutdown"), id)
            .with_kind(ArtifactKind::Blob));

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let gate = Arc::new(tokio::sync::Semaphore::new(0));
        let uninstalled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        prov.register_runtime(Arc::new(TrackingRuntime {
            gate:           Some(Arc::clone(&gate)),
            fail_remaining: Arc::new(AtomicU64::new(0)),
            uninstalled:    Arc::clone(&uninstalled),
        }));
        prov.supervise(CapFilter::new("llm", "shutdown"), 1);
        assert_eq!(prov.provision_round(), 1);
        assert_eq!(prov.installing_count(), 1);

        // Shut the node down under the in-flight install, then let the install finish.
        agent.shutdown().await;
        gate.add_permits(1);

        // The task completes without panicking the runtime; the reservation resolves.
        let mut settled = false;
        for _ in 0..200 {
            if prov.installing_count() == 0 {
                settled = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(settled, "the in-flight install resolves after shutdown (no zombie reservation)");
        // And the provisioner is still coherent to use (no poisoned lock, no panic).
        assert_eq!(prov.provision_round(), 0, "rounds on a shut agent are safe no-ops");
    }

    /// M2 Run-38 falsification probe (Philosophy / no-coordinator), kept: resource-aware
    /// self-election must leave NO fleet-visible scheduler state. After a full install cycle,
    /// the only KV the provisioner produced is the capability advertisement family for what it
    /// hosts (`cap/...`) — no assignment keys, no resource gossip, no queue: eligibility was
    /// decided and forgotten locally.
    #[tokio::test]
    async fn self_election_writes_no_scheduler_state_to_the_fleet() {
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id)
            .with_requirements(0, 1_000)); // resource-checked election (real probe, default policy)

        // Snapshot every key before the provisioner exists.
        let before: std::collections::HashSet<String> =
            agent.kv().keys().into_iter().map(|k| k.to_string()).collect();

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.supervise(CapFilter::new("text", "echo"), 1);
        assert_eq!(prov.provision_round(), 1);
        wait_live(&prov, 1).await;
        for _ in 0..3 {
            prov.provision_round(); // extra rounds: probe pass + idempotence also write nothing
        }

        // The ad's KV write lands asynchronously — poll structurally for it before diffing.
        let mut after: Vec<String> = Vec::new();
        for _ in 0..80 {
            after = agent.kv().keys().into_iter()
                .map(|k| k.to_string())
                .filter(|k| !before.contains(k))
                .collect();
            if !after.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(!after.is_empty(), "the capability advertisement itself must appear");
        for key in &after {
            assert!(
                key.starts_with("cap/"),
                "self-election left non-advertisement fleet state: {key} — that would be a \
                 scheduler wearing a costume (design §4.4)"
            );
        }

        agent.shutdown().await;
    }

    /// Run-38 floor fix (Observability): the tripwires are no longer programmatic-only — they
    /// reach the `metrics` facade, so any embedder with a recorder (the node's `/metrics`
    /// exporter) sees resource-skip storms without extra plumbing.
    #[test]
    fn tripwire_counters_reach_the_metrics_facade() {
        use metrics_util::debugging::{DebugValue, DebuggingRecorder};

        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        metrics::with_local_recorder(&recorder, || {
            // An unstarted agent suffices: the presence pass reads only local (empty) state,
            // resolves the catalog, and skips on "no runtime for kind" — the tripwire path.
            let port =
                std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let id = mycelium::NodeId::new("127.0.0.1", port).unwrap();
            let agent = Arc::new(GossipAgent::new(
                id,
                mycelium::GossipConfig { bind_port: port, ..Default::default() },
            ));
            let host = Arc::new(WasmHost::new().expect("engine"));
            let mut source = InMemorySource::new();
            let blob = source.insert(b"unhostable model".to_vec());
            let mut catalog = InstallableCatalog::new();
            catalog.add(InstallableEntry::new(Capability::new("llm", "unhostable"), blob)
                .with_kind(ArtifactKind::Blob));
            let mut prov = Provisioner::new(agent, host, catalog, Arc::new(source), 1.0);
            prov.supervise(CapFilter::new("llm", "unhostable"), 1);
            assert_eq!(prov.provision_round(), 0);
            assert!(prov.ineligible_skips() >= 1);
        });

        let snapshot = snapshotter.snapshot().into_vec();
        let skip = snapshot.iter().find(|(key, _, _, _)| {
            key.key().name() == "mycelium_artifact_ineligible_skips_total"
                && key.key().labels().any(|l| l.key() == "reason" && l.value() == "no_runtime")
        });
        match skip {
            Some((_, _, _, DebugValue::Counter(n))) => {
                assert!(*n >= 1, "skip counter incremented through the facade")
            }
            other => panic!("expected the no_runtime skip counter in the snapshot, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unregistered_kind_and_over_budget_entries_are_skipped_and_counted() {
        // Eligibility is node-local truth: no runtime for a kind → never self-elect for it; over
        // the install budget → same. Both are silent non-participation plus a tripwire tick
        // (detection, not prevention) — never an error.
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));

        let mut source = InMemorySource::new();
        let blob_id = source.insert(b"model weights stand-in".to_vec());
        let wasm_id = source.insert(ECHO_COMPONENT.to_vec());

        // A blob-kind entry (no blob runtime registered here) + an over-budget wasm entry.
        let mut catalog = InstallableCatalog::new();
        catalog.add(
            InstallableEntry::new(Capability::new("llm", "weights"), blob_id)
                .with_kind(ArtifactKind::Blob),
        );
        catalog.add(
            InstallableEntry::new(Capability::new("text", "echo"), wasm_id).with_cost(10_000, 1),
        );

        let mut prov =
            Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        prov.set_install_budget(1_000); // below the wasm entry's 10_000 hint
        prov.supervise(CapFilter::new("llm", "weights"), 1);
        prov.supervise(CapFilter::new("text", "echo"), 1);

        assert_eq!(prov.provision_round(), 0, "neither entry is eligible on this node");
        assert_eq!(prov.hosted_count(), 0);
        assert_eq!(prov.installing_count(), 0);
        assert_eq!(prov.ineligible_skips(), 2, "one tripwire tick per skipped entry");

        // Raising the budget makes the wasm entry eligible; the blob kind stays unhostable.
        prov.set_install_budget(1_000_000);
        assert_eq!(prov.provision_round(), 1, "wasm entry becomes eligible");
        wait_live(&prov, 1).await;
        assert_eq!(prov.ineligible_skips(), 3, "blob entry ticked again this round");

        agent.shutdown().await;
    }

    /// I5's gate, half one — **the trace changes no decision**: the same scenario, one provisioner
    /// with a sink and one without, decides and hosts identically; and the traced one says what it
    /// decided, in order, with typed reasons: eligible → elected → `unmet_demand_live` → `completed`,
    /// then next round `all_healthy` and `already_hosted`. (Draw-count equivalence under `sim` is
    /// `the_trace_makes_no_extra_draw_under_the_replay_seams` below, feature `sim`.)
    #[tokio::test]
    async fn the_trace_changes_no_decision_and_says_what_the_round_decided() {
        use mycelium::decision::{DecisionSink, OutcomeKind};
        async fn scenario(sink: Option<Arc<DecisionSink>>) -> (usize, usize, Vec<(String, String, String)>) {
            let agent = live_agent().await;
            let host = Arc::new(WasmHost::new().expect("engine"));
            let mut source = InMemorySource::new();
            let id = source.insert(ECHO_COMPONENT.to_vec());
            let mut catalog = InstallableCatalog::new();
            catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));
            let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
            if let Some(s) = &sink {
                prov.with_decision_trace(Arc::clone(s));
            }
            let _req = declare_and_await_demand(&agent, "text", "echo").await;
            let started = prov.provision_round();
            wait_live(&prov, 1).await;
            let second = prov.provision_round();
            assert_eq!(second, 0);
            let records = match &sink {
                Some(s) => {
                    for _ in 0..100 {
                        if s.snapshot().iter().any(|r| r.rule == "prov.install") { break }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    s.snapshot().into_iter().map(|r| (r.rule, format!("{:?}", r.outcome), r.reason)).collect()
                }
                None => Vec::new(),
            };
            let hosted = prov.hosted_count();
            agent.shutdown().await;
            (started, hosted, records)
        }
        let (off_started, off_hosted, off_records) = scenario(None).await;
        let sink = Arc::new(DecisionSink::default());
        let (on_started, on_hosted, on_records) = scenario(Some(Arc::clone(&sink))).await;
        assert_eq!((off_started, off_hosted), (1, 1));
        assert_eq!((on_started, on_hosted), (off_started, off_hosted), "trace on decides as trace off");
        assert!(off_records.is_empty(), "no sink, no records");

        let names: Vec<&str> = on_records.iter().map(|(r, _, _)| r.as_str()).collect();
        let reasons: Vec<&str> = on_records.iter().map(|(_, _, s)| s.as_str()).collect();
        // Round 1: eligible, elected, started; the install task's completion; round 2: healthy, hosted.
        assert_eq!(&names[..3], ["prov.eligible", "prov.self_election", "prov.demand_response"], "{on_records:?}");
        assert_eq!(&reasons[..3], ["eligible", "elected", "unmet_demand_live"], "{on_records:?}");
        assert!(on_records.iter().any(|(r, o, s)| r == "prov.install" && o == "Action" && s == "completed"), "{on_records:?}");
        let round2: Vec<&str> = on_records.iter().rev().take(2).map(|(_, _, s)| s.as_str()).collect();
        assert_eq!(round2, ["already_hosted", "all_healthy"], "{on_records:?}");
        assert_eq!(sink.dropped(), 0);
        let one = sink.snapshot().remove(0);
        assert_eq!(one.rule_revision, 1);
        assert!(one.trigger.starts_with("round 1") && !one.build.is_empty() && one.at_ms.is_none());
        assert!(one.config_digest.as_deref().is_some_and(|d| !d.is_empty()), "stamped with the configuration digest");
        assert_eq!(one.outcome, OutcomeKind::Action);
        let _ = OutcomeKind::Refusal;
    }

    /// I5's gate, half two — **saturation**: a sink of one record keeps one, drops the rest and counts
    /// them, and the round still installs.
    #[tokio::test]
    async fn a_saturated_sink_drops_and_counts_and_the_round_still_installs() {
        use mycelium::decision::{DecisionSink, SinkConfig};
        let agent = live_agent().await;
        let host = Arc::new(WasmHost::new().expect("engine"));
        let mut source = InMemorySource::new();
        let id = source.insert(ECHO_COMPONENT.to_vec());
        let mut catalog = InstallableCatalog::new();
        catalog.add(InstallableEntry::new(Capability::new("text", "echo"), id));
        let mut prov = Provisioner::new(Arc::clone(&agent), host, catalog, Arc::new(source), 1.0);
        let sink = Arc::new(DecisionSink::new(SinkConfig { max_records: 1, ..Default::default() }));
        prov.with_decision_trace(Arc::clone(&sink));
        let _req = declare_and_await_demand(&agent, "text", "echo").await;
        assert_eq!(prov.provision_round(), 1, "the round installs whatever the sink keeps");
        wait_live(&prov, 1).await;
        let st = sink.stats();
        assert_eq!((st.recorded, st.held), (1, 1));
        assert!(st.dropped_full >= 2, "{st:?}");
        assert_eq!(sink.snapshot()[0].rule, "prov.eligible", "the prefix, not the tail");
        agent.shutdown().await;
    }

    /// I5's last gate, under the replay seams: the self-election draw is the round's one draw, and
    /// with a sink it is made exactly as often as without — the kernel's choice trace holds the same
    /// `select` draws. The provisioner is built on an unstarted agent (the draw touches no mesh), on
    /// tokio's current-thread runtime so every seam read is this thread's.
    #[cfg(feature = "sim")]
    #[tokio::test]
    async fn the_trace_makes_no_extra_draw_under_the_replay_seams() {
        use mycelium::decision::DecisionSink;
        use mycelium::sim_seam::{install, take, SimContext};
        use mycelium_sim::{ChoiceKind, Kernel, Sources};
        fn draws(sink: Option<Arc<DecisionSink>>) -> (Vec<bool>, Vec<String>) {
            install(SimContext { kernel: Kernel::recording(), sources: Sources::seeded(7, 1_700_000_000_000), node: "pilot".into(), offsets: Default::default() });
            let port = alloc_port();
            let agent = Arc::new(GossipAgent::new(mycelium::NodeId::new("127.0.0.1", port).unwrap(), mycelium::GossipConfig { bind_port: port, ..Default::default() }));
            let host = Arc::new(WasmHost::new().expect("engine"));
            let mut prov = Provisioner::new(agent, host, InstallableCatalog::new(), Arc::new(InMemorySource::new()), 0.5);
            if let Some(s) = &sink { prov.with_decision_trace(Arc::clone(s)); }
            let outcomes: Vec<bool> = (0..16).map(|_| prov.self_elects_traced("text/echo")).collect();
            let ctx = take().expect("this thread's kernel");
            let streams = ctx.kernel.trace().entries().iter().filter(|c| c.kind == ChoiceKind::Rng).map(|c| c.stream.clone()).collect();
            (outcomes, streams)
        }
        let (off, off_streams) = draws(None);
        let sink = Arc::new(DecisionSink::default());
        let (on, on_streams) = draws(Some(Arc::clone(&sink)));
        assert_eq!(off, on, "the same seed draws the same elections with a sink as without");
        assert_eq!(off_streams, on_streams, "and the kernel saw the same draws: {off_streams:?} vs {on_streams:?}");
        assert_eq!(off_streams.iter().filter(|s| s.as_str() == "select").count(), 16);
        assert_eq!(sink.stats().recorded, 16, "every draw recorded, none drawn by the record");
    }
}
