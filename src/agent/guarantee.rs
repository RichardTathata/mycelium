//! The **guarantee registry** and the **startup report** — `docs/plans/guarantees-and-rule-catalogue.md`,
//! increment I1's guarantee half and increment I2.
//!
//! A *guarantee* is a claim the substrate can make for a deployment — *the gateway is not open*, *gossip
//! is authenticated*, *a decision is recorded before dispatch* — together with what it needs (a feature,
//! a setting, an attached component), where it is enforced, and **how this node can check it**. The
//! registry holds the descriptors; [`report`] resolves every one of them against this node, built and
//! configured as it is, into a [`GuaranteeReport`].
//!
//! # The two kinds, resolved differently (plan G4)
//!
//! A **node-enforced** guarantee resolves to one of four states: [`Resolution::Enforced`], or why not —
//! [`Resolution::NotConfigured`] (the setting or attachment is missing), [`Resolution::NotInBuild`] (the
//! feature that provides it is not compiled in), or [`Resolution::NotApplicable`] (the node's role makes
//! it moot, and the report says which role fact — a node with no gateway does not fail a gateway
//! guarantee, G10). An **external prerequisite** is what a node cannot see — network confinement, clock
//! sync, the consensus profile a proposer chooses per call — and resolves only to
//! [`Resolution::NotVerifiableHere`], listed as *unresolved* and never counted as met.
//!
//! The strongest sentence the report says is therefore **"node requirements satisfied"**
//! ([`GuaranteeReport::node_requirements_satisfied`]), never "deployment verified".
//!
//! # Why this exists
//!
//! Four settings were found in two days whose enforcing code sat behind a feature the build did not
//! have: parsed, validated, and ignored — an open gateway, plaintext gossip (plan §8). Each was a row
//! nobody had written: *this guarantee · needs this feature · nothing checks that at start*. The
//! report resolves every row by construction, so a setting that *vanishes* (a `#[cfg]`'d field under a
//! serde that tolerates unknown keys) or *lies* (a field present in every build whose consumers are
//! `#[cfg]`'d) reads `NotInBuild` here rather than silently nothing.
//!
//! # The lifecycle boundary (plan G13)
//!
//! `GossipAgent::start()` logs the report after validation and before admitting traffic, and marks the
//! boundary. An attachment made after it (`with_action_evaluator`, `with_evidence_journal`, …) is
//! counted (`GossipAgent::late_attachments`) and warned about: the logged report is stale from that
//! moment, [`report`] itself recomputes live. The report carries the digest of the configuration it was
//! computed over, so a stale one is detectable. Refusing a profile's unmet guarantees at this boundary
//! is increment I3; this increment only *states*.
//!
//! # Registering more (plan G11)
//!
//! A companion registers its own guarantees through `GossipAgent::register_guarantee` **before**
//! `start()`. A duplicate id is refused, a core id cannot be overridden, and a registration after the
//! boundary is refused — a requirement must not vanish with its registrar, nor arrive after the report
//! that would have shown it.

use std::sync::Arc;

use serde::Serialize;

use super::TaskCtx;
use crate::error::GossipError;

/// A stable guarantee id, `subsystem.name` — never a filename or a line number.
pub type GuaranteeId = &'static str;

/// Which of the two kinds a guarantee is (plan G4).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuaranteeKind {
    /// This node can check it, and `start()` could refuse on it (I3).
    NodeEnforced,
    /// This node cannot see it; evidence lives in the deployment, never in this report.
    ExternalPrerequisite,
}

/// What a guarantee resolved to on this node, built and configured as it is.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Resolution {
    /// The check runs here.
    Enforced,
    /// The build could provide it; the setting or attachment named is missing.
    NotConfigured { missing: &'static str },
    /// The feature that provides it is not compiled into this build.
    NotInBuild { feature: &'static str },
    /// The node's role makes it moot — and this is the role fact, never a waiver (G10).
    NotApplicable { because: &'static str },
    /// Not verifiable from inside a node; this is where the evidence lives instead.
    NotVerifiableHere { evidence: &'static str },
}

impl Resolution {
    /// Whether a node-enforced guarantee counts as met — only `Enforced` does.
    pub fn is_enforced(&self) -> bool {
        matches!(self, Resolution::Enforced)
    }
    /// The variant name, for goldens that must not pin a reason string.
    pub fn state(&self) -> &'static str {
        match self {
            Resolution::Enforced => "enforced",
            Resolution::NotConfigured { .. } => "not_configured",
            Resolution::NotInBuild { .. } => "not_in_build",
            Resolution::NotApplicable { .. } => "not_applicable",
            Resolution::NotVerifiableHere { .. } => "not_verifiable_here",
        }
    }
}

/// How a guarantee is checked against a node: `applies` returns the role fact that makes it moot (or
/// `None` when it applies); `resolve` is consulted only when it applies.
type Applies = Arc<dyn Fn(&TaskCtx) -> Option<&'static str> + Send + Sync>;
type Resolve = Arc<dyn Fn(&TaskCtx) -> Resolution + Send + Sync>;

/// What a guarantee's check may read of the node: the effective configuration and whether the node has
/// passed its lifecycle boundary. A companion's guarantee usually reads its own state, captured in the
/// closure, and this for the configuration it depends on.
pub struct NodeView<'a> {
    ctx: &'a TaskCtx,
}

impl NodeView<'_> {
    /// The effective `GossipConfig`.
    pub fn config(&self) -> &crate::config::GossipConfig {
        &self.ctx.config
    }
    /// Whether `start()` has passed its boundary.
    pub fn started(&self) -> bool {
        self.ctx.started.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// One guarantee: the claim, what it needs, where it is enforced, and how a node checks it (plan G1).
///
/// Descriptors are **metadata plus a check**; they schedule nothing and enforce nothing themselves.
#[derive(Clone)]
pub struct GuaranteeDescriptor {
    /// Stable id, `subsystem.name`.
    pub id: GuaranteeId,
    /// Semantic revision — bumped when the *meaning* of the guarantee or its check changes.
    pub revision: u32,
    /// The owning subsystem.
    pub subsystem: &'static str,
    pub kind: GuaranteeKind,
    /// What the guarantee promises, in one sentence a reader of the report can act on.
    pub promise: &'static str,
    /// The features, settings and attachments it needs — prose, one line.
    pub needs: &'static str,
    /// Where it is enforced: the decision points (rule ids once I4 names them; symbols until then).
    pub enforcement_points: &'static [&'static str],
    /// Where to read more.
    pub docs: &'static str,
    /// The role predicate (G10): `Some(role fact)` when the guarantee does not apply to this node.
    pub(crate) applies: Applies,
    /// The check.
    pub(crate) resolve: Resolve,
}

impl GuaranteeDescriptor {
    /// A guarantee from outside the core set — a companion's. `applies` returns the role fact that
    /// makes the guarantee moot on this node, or `None`; `resolve` is consulted only when it applies.
    /// Register it with `GossipAgent::register_guarantee` before `start()`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: GuaranteeId,
        revision: u32,
        subsystem: &'static str,
        kind: GuaranteeKind,
        promise: &'static str,
        needs: &'static str,
        enforcement_points: &'static [&'static str],
        docs: &'static str,
        applies: impl Fn(&NodeView<'_>) -> Option<&'static str> + Send + Sync + 'static,
        resolve: impl Fn(&NodeView<'_>) -> Resolution + Send + Sync + 'static,
    ) -> Self {
        GuaranteeDescriptor {
            id, revision, subsystem, kind, promise, needs, enforcement_points, docs,
            applies: Arc::new(move |ctx: &TaskCtx| applies(&NodeView { ctx })),
            resolve: Arc::new(move |ctx: &TaskCtx| resolve(&NodeView { ctx })),
        }
    }
}

impl std::fmt::Debug for GuaranteeDescriptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuaranteeDescriptor")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// One guarantee as it resolved on this node.
#[non_exhaustive]
#[derive(Clone, Debug, Serialize)]
pub struct GuaranteeEntry {
    pub id: GuaranteeId,
    pub revision: u32,
    pub subsystem: &'static str,
    pub kind: GuaranteeKind,
    pub promise: &'static str,
    pub enforcement_points: &'static [&'static str],
    pub docs: &'static str,
    pub resolution: Resolution,
}

/// The schema id of the serialised report; a change to its shape is a new schema.
pub const REPORT_SCHEMA: &str = "mycelium.guarantees/1";

/// Every registered guarantee resolved against this node, with what it was computed over.
#[non_exhaustive]
#[derive(Clone, Debug, Serialize)]
pub struct GuaranteeReport {
    pub schema: &'static str,
    pub node_id: String,
    /// The crate version this node was built from.
    pub version: &'static str,
    /// The features compiled in, of the ones a guarantee can depend on.
    pub features: Vec<&'static str>,
    /// SHA-256 over the effective `GossipConfig`, so a report can be matched to the configuration it
    /// was computed over (G13). Attachments are not in it; see `started` and `late_attachments`.
    pub config_digest: String,
    /// Whether the node had passed its lifecycle boundary when this was computed.
    pub started: bool,
    /// Attachments made after the boundary — a non-zero count means the logged report is stale.
    pub late_attachments: u32,
    /// The profile this node runs under (`dev` by default, said so), its revision and required set.
    pub profile: Option<ProfileRef>,
    pub entries: Vec<GuaranteeEntry>,
}

/// The profile a node was started under, as the report carries it (plan G12): its name and revision,
/// whether the configuration selected it (or `dev` applied by default), and the ids it requires.
#[non_exhaustive]
#[derive(Clone, Debug, Serialize)]
pub struct ProfileRef {
    pub name: &'static str,
    pub revision: u32,
    /// `false` when no profile was configured and `dev` applies by default.
    pub selected: bool,
    pub required: Vec<GuaranteeId>,
}

/// A named, versioned set of required guarantee ids (plan G3, G12). A node started under it refuses
/// to start unless every id resolves `Enforced` or `NotApplicable` on that node (G5); an id the registry
/// does not hold refuses too — a requirement must not vanish with its registrar (G11).
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct Profile {
    pub name: &'static str,
    /// Bumped when the required set changes; adding a requirement is a release note (G12).
    pub revision: u32,
    pub required: &'static [GuaranteeId],
    pub about: &'static str,
}

/// Nothing required. Said loudly in the report and the log: this is not a production profile.
pub const DEV: Profile = Profile {
    name: "dev",
    revision: 1,
    required: &[],
    about: "nothing required — a local demo or a development node; the report still says what is and is not enforced",
};

/// The readiness checklist's production set for one trust domain (plan G3): what a node fronting
/// agents must enforce before it is exposed. External prerequisites (network confinement, clock
/// sync, the consensus profile) are not in it — they cannot be; the report lists them unresolved.
pub const SECURE_SINGLE_DOMAIN: Profile = Profile {
    name: "secure-single-domain",
    // Rev 2 (announced in v2.20.0, G12): `id.ca_key_off_node` — a node certificate issued off-node
    // (`mycelium tls issue`, `[tls] cert_pem` + `key_pem`) so the fleet CA's private key is on no
    // node — and `persist.unreadable_refused` (`persistence.on_unreadable = "refuse"`, the default).
    // Rev 1 could not require the first: until v2.20.0 the TLS init re-signed the node cert with
    // the CA key at every start and minted a CA where the key was absent.
    revision: 2,
    required: &[
        "mesh.tls",
        "id.proofs_required",
        "gw.not_open",
        "gw.tls",
        "gw.caller_profile",
        "ae.authorised_at_seam",
        "ae.recorded_before_dispatch",
        "prov.enforcement",
        "a2a.admission",
        "authz.execution_authority",
        "authz.durable_epochs",
        "audit.chain",
        "egress.allow_list",
        "persist.configured",
        "persist.sync_mode",
        "persist.unreadable_refused",
        "id.ca_key_off_node",
    ],
    about: "a node fronting agents in one trust domain: authenticated transport and identity, a closed \
            HTTPS gateway, authority checked and recorded at the gateway and the provider, revocation \
            that survives a restart, a sealed audit chain, an egress allow-list, durable persistence",
};

/// The profile named `name`, if it is one of [`crate::config::PROFILE_NAMES`].
pub fn profile_named(name: &str) -> Option<&'static Profile> {
    match name {
        "dev" => Some(&DEV),
        "secure-single-domain" => Some(&SECURE_SINGLE_DOMAIN),
        _ => None,
    }
}

/// The profile a configuration selects — `dev` when none is set (`validate()` refused unknown names).
pub(crate) fn selected_profile(cfg: &crate::config::GossipConfig) -> (&'static Profile, bool) {
    match cfg.profile.as_deref().and_then(profile_named) {
        Some(p) => (p, true),
        None => (&DEV, false),
    }
}

/// Why a profile refused to start a node: every required id that is not registered, and every one
/// that applies and did not resolve `Enforced`, with what is missing and where to read.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileRefusal {
    pub profile: &'static str,
    pub revision: u32,
    pub unknown: Vec<GuaranteeId>,
    /// `(id, state, what is missing or not in the build, docs)`.
    pub unmet: Vec<(GuaranteeId, &'static str, &'static str, &'static str)>,
}

impl std::fmt::Display for ProfileRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "profile `{}` rev {}: ", self.profile, self.revision)?;
        if !self.unknown.is_empty() {
            write!(f, "{} required guarantee(s) not registered on this node ({}) — a requirement does not vanish with its registrar; ", self.unknown.len(), self.unknown.join(", "))?;
        }
        if !self.unmet.is_empty() {
            write!(f, "{} requirement(s) unmet:", self.unmet.len())?;
            for (id, state, detail, docs) in &self.unmet {
                write!(f, " [{id}: {state} — {detail} → {docs}]")?;
            }
        }
        Ok(())
    }
}

/// Check a report against a profile (plan G5, G11). `Ok` means every required id is registered and
/// resolves `Enforced` or `NotApplicable`; an external prerequisite in a profile's set is a profile
/// bug and reads as unmet, because it can never be met from inside the node.
pub fn check(report: &GuaranteeReport, profile: &Profile) -> Result<(), ProfileRefusal> {
    let mut refusal = ProfileRefusal { profile: profile.name, revision: profile.revision, unknown: Vec::new(), unmet: Vec::new() };
    for id in profile.required {
        match report.entry(id) {
            None => refusal.unknown.push(id),
            Some(e) => match &e.resolution {
                Resolution::Enforced | Resolution::NotApplicable { .. } => {}
                Resolution::NotConfigured { missing } => refusal.unmet.push((e.id, "not_configured", missing, e.docs)),
                Resolution::NotInBuild { feature } => refusal.unmet.push((e.id, "not_in_build", feature, e.docs)),
                Resolution::NotVerifiableHere { evidence } => refusal.unmet.push((e.id, "not_verifiable_here", evidence, e.docs)),
            },
        }
    }
    if refusal.unknown.is_empty() && refusal.unmet.is_empty() { Ok(()) } else { Err(refusal) }
}

impl GuaranteeReport {
    /// Node-enforced guarantees that apply to this node and are not `Enforced`.
    pub fn unmet(&self) -> Vec<GuaranteeId> {
        self.entries
            .iter()
            .filter(|e| e.kind == GuaranteeKind::NodeEnforced)
            .filter(|e| !matches!(e.resolution, Resolution::Enforced | Resolution::NotApplicable { .. }))
            .map(|e| e.id)
            .collect()
    }
    /// External prerequisites — listed, never counted as met.
    pub fn unresolved(&self) -> Vec<GuaranteeId> {
        self.entries.iter().filter(|e| e.kind == GuaranteeKind::ExternalPrerequisite).map(|e| e.id).collect()
    }
    /// Guarantees that do not apply to this node's role, with the role fact.
    pub fn not_applicable(&self) -> Vec<(GuaranteeId, &'static str)> {
        self.entries
            .iter()
            .filter_map(|e| match e.resolution {
                Resolution::NotApplicable { because } => Some((e.id, because)),
                _ => None,
            })
            .collect()
    }
    /// Of the profile's required guarantees, those that apply and are not `Enforced` — what a
    /// profile refuses on. Empty when no profile is carried.
    pub fn required_unmet(&self) -> Vec<GuaranteeId> {
        let Some(p) = &self.profile else { return Vec::new() };
        self.unmet().into_iter().filter(|id| p.required.contains(id)).collect()
    }
    /// **The strongest thing this report says**: every guarantee the carried profile requires is
    /// enforced on this node, or does not apply to it. Under `dev` that is vacuously true, and the
    /// log says so. It is not "deployment verified" — [`Self::unresolved`] is what the node cannot
    /// see, and [`Self::unmet`] is everything else the node could enforce and does not.
    pub fn node_requirements_satisfied(&self) -> bool {
        self.required_unmet().is_empty()
    }
    /// The entry for `id`, if registered.
    pub fn entry(&self, id: &str) -> Option<&GuaranteeEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

// ── The registry ────────────────────────────────────────────────────────────────────────────────

/// The guarantees every node registers at construction. Ids here cannot be overridden (G11).
pub(crate) fn core_ids() -> Vec<GuaranteeId> {
    core_guarantees().into_iter().map(|g| g.id).collect()
}

/// Register a guarantee from outside the core set — a companion's, before `start()`.
pub(crate) fn register(ctx: &TaskCtx, desc: GuaranteeDescriptor) -> Result<(), GossipError> {
    if ctx.started.load(std::sync::atomic::Ordering::Acquire) {
        return Err(GossipError::InvalidField {
            field: "guarantee",
            reason: format!(
                "`{}` registered after start(): the startup report has already been computed and \
                 logged; register guarantees before the lifecycle boundary",
                desc.id
            ),
        });
    }
    let mut regs = ctx.guarantees.lock();
    if regs.iter().any(|g| g.id == desc.id) {
        let core = core_ids().contains(&desc.id);
        return Err(GossipError::InvalidField {
            field: "guarantee",
            reason: if core {
                format!("`{}` is a core guarantee and cannot be overridden by a registration", desc.id)
            } else {
                format!("`{}` is already registered; ids are unique", desc.id)
            },
        });
    }
    regs.push(desc);
    Ok(())
}

/// Resolve one guarantee **without** its role predicate — what the check says of this node's
/// configuration and attachments regardless of whether the role makes it moot. The confined-fleet
/// view uses it; the report never does.
pub(crate) fn resolve_ignoring_role(ctx: &TaskCtx, id: &str) -> Option<Resolution> {
    let regs = ctx.guarantees.lock();
    regs.iter().find(|g| g.id == id).map(|g| (g.resolve)(ctx))
}

/// Resolve every registered guarantee against this node.
pub(crate) fn report(ctx: &TaskCtx) -> GuaranteeReport {
    use std::sync::atomic::Ordering;
    let descs: Vec<GuaranteeDescriptor> = ctx.guarantees.lock().clone();
    let entries = descs
        .iter()
        .map(|g| {
            let resolution = match (g.applies)(ctx) {
                Some(because) => Resolution::NotApplicable { because },
                None => (g.resolve)(ctx),
            };
            GuaranteeEntry {
                id: g.id,
                revision: g.revision,
                subsystem: g.subsystem,
                kind: g.kind,
                promise: g.promise,
                enforcement_points: g.enforcement_points,
                docs: g.docs,
                resolution,
            }
        })
        .collect();
    GuaranteeReport {
        schema: REPORT_SCHEMA,
        node_id: ctx.node_id.to_string(),
        version: env!("CARGO_PKG_VERSION"),
        features: compiled_features(),
        config_digest: config_digest(&ctx.config),
        started: ctx.started.load(Ordering::Acquire),
        late_attachments: ctx.late_attachments.load(Ordering::Relaxed),
        profile: {
            let (p, selected) = selected_profile(&ctx.config);
            Some(ProfileRef { name: p.name, revision: p.revision, selected, required: p.required.to_vec() })
        },
        entries,
    }
}

/// The features a guarantee can depend on, as compiled into this build.
pub(crate) fn compiled_features() -> Vec<&'static str> {
    let mut v = Vec::new();
    if cfg!(feature = "gateway") { v.push("gateway"); }
    if cfg!(feature = "tls") { v.push("tls"); }
    if cfg!(feature = "compliance") { v.push("compliance"); }
    if cfg!(feature = "a2a") { v.push("a2a"); }
    if cfg!(feature = "consensus") { v.push("consensus"); }
    if cfg!(feature = "metrics") { v.push("metrics"); }
    v
}

/// SHA-256 over the effective configuration (serde_json, field order is the struct's).
pub(crate) fn config_digest(cfg: &crate::config::GossipConfig) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(cfg).unwrap_or_default();
    let d = Sha256::digest(&bytes);
    let mut s = String::with_capacity(64);
    for b in d { s.push_str(&format!("{b:02x}")); }
    s
}

/// Log the report as one block — one line per entry, then the summary.
pub(crate) fn log_report(r: &GuaranteeReport) {
    for e in &r.entries {
        tracing::info!(
            guarantee = e.id, kind = ?e.kind, state = e.resolution.state(),
            detail = ?detail(&e.resolution), "guarantee"
        );
    }
    if let Some(p) = &r.profile {
        if p.name == "dev" {
            tracing::warn!(profile = p.name, revision = p.revision, selected = p.selected, "guarantees: profile `dev` — nothing required; this is not a production profile (set `profile = \"secure-single-domain\"` or GOSSIP_PROFILE)");
        } else {
            tracing::info!(profile = p.name, revision = p.revision, required = ?p.required, "guarantees: profile");
        }
    }
    let unmet = r.unmet();
    let required_unmet = r.required_unmet();
    let unresolved = r.unresolved();
    let profile = r.profile.as_ref().map(|p| p.name).unwrap_or("dev");
    if required_unmet.is_empty() {
        tracing::info!(
            profile, not_enforced = ?unmet, unresolved = ?unresolved, config_digest = %r.config_digest,
            "guarantees: node requirements satisfied under profile `{profile}` — {} enforced, {} not applicable, {} not enforced and not required by this profile; {} external prerequisites are not verifiable here (this is not 'deployment verified')",
            r.entries.iter().filter(|e| e.resolution.is_enforced()).count(),
            r.not_applicable().len(),
            unmet.len(),
            unresolved.len()
        );
    } else {
        tracing::warn!(
            profile, required_unmet = ?required_unmet, not_enforced = ?unmet, unresolved = ?unresolved, config_digest = %r.config_digest,
            "guarantees: {} guarantee(s) required by profile `{profile}` NOT met on this node",
            required_unmet.len()
        );
    }
}

fn detail(r: &Resolution) -> Option<&'static str> {
    match r {
        Resolution::Enforced => None,
        Resolution::NotConfigured { missing } => Some(missing),
        Resolution::NotInBuild { feature } => Some(feature),
        Resolution::NotApplicable { because } => Some(because),
        Resolution::NotVerifiableHere { evidence } => Some(evidence),
    }
}

// ── The core guarantees ─────────────────────────────────────────────────────────────────────────
//
// Each is a row of plan §8 made checkable. `applies` names the role fact; `resolve` reads the
// configuration and the attached components through `TaskCtx` — never the network, never a clock.

#[allow(clippy::too_many_arguments)]
fn g(
    id: GuaranteeId,
    revision: u32,
    subsystem: &'static str,
    kind: GuaranteeKind,
    promise: &'static str,
    needs: &'static str,
    enforcement_points: &'static [&'static str],
    docs: &'static str,
    applies: impl Fn(&TaskCtx) -> Option<&'static str> + Send + Sync + 'static,
    resolve: impl Fn(&TaskCtx) -> Resolution + Send + Sync + 'static,
) -> GuaranteeDescriptor {
    GuaranteeDescriptor {
        id, revision, subsystem, kind, promise, needs, enforcement_points, docs,
        applies: Arc::new(applies),
        resolve: Arc::new(resolve),
    }
}

/// The gateway role: this node serves HTTP.
fn gateway_role(ctx: &TaskCtx) -> Option<&'static str> {
    if ctx.config.http_port.is_some() { None } else { Some("no `http_port`: this node runs no gateway") }
}
fn always(_: &TaskCtx) -> Option<&'static str> { None }
fn external(evidence: &'static str) -> impl Fn(&TaskCtx) -> Resolution + Send + Sync + 'static {
    move |_| Resolution::NotVerifiableHere { evidence }
}

pub(crate) fn core_guarantees() -> Vec<GuaranteeDescriptor> {
    use GuaranteeKind::{ExternalPrerequisite as Ext, NodeEnforced as Node};
    vec![
        // A merged router's route outside the gated prefixes is public by construction — a documented
        // class, not a guarantee (`gw.extra_routes_auth`, plan §8; docs/operations/rbac.md).
        g("gw.not_open", 1, "gateway", Node,
          "the HTTP gateway requires a credential on every non-public route",
          "`gateway_auth_token`, or (`compliance`) a token table or `[oidc]`",
          &["gateway_auth"], "docs/operations/rbac.md",
          gateway_role,
          |c| {
              let cfg = &c.config;
              #[allow(unused_mut)]
              let mut closed = cfg.gateway_auth_token.is_some();
              #[cfg(feature = "compliance")]
              { closed |= !cfg.gateway_scoped_tokens.is_empty() || !cfg.gateway_named_tokens.is_empty() || cfg.oidc.is_some(); }
              if closed { Resolution::Enforced } else { Resolution::NotConfigured { missing: "gateway_auth_token (or, with `compliance`, a token table or [oidc])" } }
          }),
        g("gw.token_tables", 1, "gateway", Node,
          "scoped or named tokens close the gateway with a per-route scope floor",
          "`compliance`; `gateway_scoped_tokens` / `gateway_named_tokens`",
          &["gateway_auth", "required_scope"], "docs/operations/rbac.md",
          gateway_role,
          |c| {
              #[cfg(feature = "compliance")]
              { if !c.config.gateway_scoped_tokens.is_empty() || !c.config.gateway_named_tokens.is_empty() { return Resolution::Enforced; } Resolution::NotConfigured { missing: "gateway_scoped_tokens / gateway_named_tokens" } }
              #[cfg(not(feature = "compliance"))]
              { let _ = c; Resolution::NotInBuild { feature: "compliance" } }
          }),
        g("gw.oidc", 1, "gateway", Node,
          "human operators authenticate to the gateway through the IdP, groups mapped to scopes",
          "`compliance`; `[oidc]`",
          &["gateway_auth", "oidc::OidcVerifier"], "docs/operations/sso.md",
          gateway_role,
          |c| {
              #[cfg(feature = "compliance")]
              { if c.config.oidc.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[oidc]" } } }
              #[cfg(not(feature = "compliance"))]
              { let _ = c; Resolution::NotInBuild { feature: "compliance" } }
          }),
        g("gw.tls", 1, "gateway", Node,
          "the gateway serves HTTPS, so bearers and JWTs do not cross the wire in cleartext",
          "`tls`; `[gateway_tls]` (or TLS terminated by a proxy in front — not visible here)",
          &["http::serve_https"], "docs/operations/gateway-tls.md",
          gateway_role,
          |c| {
              #[cfg(feature = "tls")]
              { if c.config.gateway_tls.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[gateway_tls] (or a TLS-terminating proxy, which this node cannot see)" } } }
              #[cfg(not(feature = "tls"))]
              { let _ = c; Resolution::NotInBuild { feature: "tls" } }
          }),
        g("gw.caller_profile", 1, "gateway", Node,
          "a provider sees the gateway's client as the caller, attested by the gateway's identity",
          "`tls`; `[tls]`; `gateway_caller_profile = secure` (the default)",
          &["gateway_caller::attest", "gateway_caller::verify"], "docs/guide/20-authorising-actions.md",
          gateway_role,
          |c| {
              #[cfg(feature = "tls")]
              {
                  use crate::config::GatewayCallerProfile;
                  if c.config.gateway_caller_profile != GatewayCallerProfile::Secure { return Resolution::NotConfigured { missing: "gateway_caller_profile = secure" }; }
                  if c.config.tls.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[tls] (the attestation is signed with the node identity; without it any peer can assert any principal)" } }
              }
              #[cfg(not(feature = "tls"))]
              { let _ = c; Resolution::NotInBuild { feature: "tls" } }
          }),
        g("mesh.tls", 1, "transport", Node,
          "gossip is mTLS: every peer presents a certificate signed by the fleet CA",
          "`tls`; `[tls]`",
          &["lifecycle::start (tls init)", "connection::handshake"], "docs/guide/09-security.md",
          always,
          |c| {
              #[cfg(feature = "tls")]
              { if c.config.tls.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[tls]" } } }
              #[cfg(not(feature = "tls"))]
              { let _ = c; Resolution::NotInBuild { feature: "tls" } }
          }),
        g("id.proofs_required", 1, "identity", Node,
          "an identity entry this node cannot authenticate is rejected (issuer binding rests on it)",
          "`tls`; `[tls]`; `require_identity_proofs = true` (default off)",
          &["lifecycle (identity-proof check)"], "docs/design/identity-authentication.md",
          always,
          |c| {
              #[cfg(feature = "tls")]
              {
                  if c.config.tls.is_none() { return Resolution::NotConfigured { missing: "[tls] — proofs ride on the TLS identity; the flag is inert without it" }; }
                  if c.config.require_identity_proofs { Resolution::Enforced } else { Resolution::NotConfigured { missing: "require_identity_proofs = true" } }
              }
              #[cfg(not(feature = "tls"))]
              { let _ = c; Resolution::NotInBuild { feature: "tls" } }
          }),
        g("id.ca_key_off_node", 1, "identity", Node,
          "the fleet CA's private key is not on this node, so a removed member cannot mint itself a new identity here",
          "`tls`; `[tls] cert_pem` + `key_pem` (a node certificate issued off-node: `mycelium tls issue`); no `ca-key.pem` in this node's certificate directory",
          &["membership removal (closure plan C5)"], "docs/operations/cert-rotation.md",
          always,
          |c| {
              #[cfg(feature = "tls")]
              {
                  match &c.config.tls {
                      None => Resolution::NotConfigured { missing: "[tls]" },
                      Some(t) if t.cert_pem.is_none() => Resolution::NotConfigured { missing: "[tls] cert_pem — without a pre-issued node certificate, start() re-signs one with the CA key, which must then be on this node" },
                      Some(t) if t.auto_cert_dir.join("ca-key.pem").exists() => Resolution::NotConfigured { missing: "the CA private key is in this node's certificate directory (the `auto_cert_dir` development default)" },
                      // No CA here at all: start() mints one into this directory, key included — the
                      // report is computed before that, so say what is about to be true.
                      Some(t) if !t.auto_cert_dir.join("ca-cert.pem").exists() && t.ca_cert_pem.is_none() => Resolution::NotConfigured { missing: "no CA in `auto_cert_dir` yet: start() generates one here, private key included; provision the CA cert and keep its key off the node" },
                      Some(_) => Resolution::Enforced,
                  }
              }
              #[cfg(not(feature = "tls"))]
              { let _ = c; Resolution::NotInBuild { feature: "tls" } }
          }),
        g("ae.authorised_at_seam", 1, "authority", Node,
          "every gateway dispatch is authorised by policy before it runs — permit, deny or indeterminate",
          "`gateway` + `tls`; `with_action_evaluator`",
          &["http::ae_preflight"], "docs/guide/20-authorising-actions.md",
          gateway_role,
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { if c.action_evaluator.get().is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_action_evaluator (without it the seam is inert and the gateway dispatches as before)" } } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("ae.recorded_before_dispatch", 1, "authority", Node,
          "every authorisation decision is journalled, fsynced, before dispatch",
          "`gateway` + `tls`; `with_action_evaluator` and `with_evidence_journal`",
          &["http::ae_record"], "docs/guide/20-authorising-actions.md",
          gateway_role,
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              {
                  if c.action_evaluator.get().is_none() { return Resolution::NotConfigured { missing: "with_action_evaluator (nothing decides, so nothing is recorded)" }; }
                  if c.evidence_journal.get().is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_evidence_journal (the evaluator enforces and records nothing)" } }
              }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("prov.enforcement", 1, "authority", Node,
          "protected work (`mcp.invoke`, `skill.invoke`, …) is authorised where it runs, not only at a gateway",
          "`gateway` + `tls`; `with_provider_enforcement` (with an evaluator; without one every protected call is refused)",
          &["provider_enforcement::check"], "docs/guide/20-authorising-actions.md",
          always,
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { if c.provider_enforcement.load(std::sync::atomic::Ordering::Acquire) { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_provider_enforcement (off: nothing is checked at this provider, whatever the gateway does)" } } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("a2a.admission", 1, "authority", Node,
          "`/a2a` is not anonymous skill dispatch",
          "`a2a`; an evaluator that denies anonymous principals — a bearer does not gate this route",
          &["http::a2a_optional_auth", "a2a preflight"], "docs/operations/production-readiness.md",
          |c| {
              #[cfg(feature = "a2a")]
              { if c.a2a_mounted.load(std::sync::atomic::Ordering::Acquire) { None } else { Some("`/a2a` is not mounted (`with_a2a` not called)") } }
              #[cfg(not(feature = "a2a"))]
              { let _ = c; Some("no `/a2a` in this build (`a2a` feature off)") }
          },
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { if c.action_evaluator.get().is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_action_evaluator — an absent bearer is anonymous and a present one's scopes are dropped on this route" } } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("authz.execution_authority", 1, "authority", Node,
          "mandates are established and verified at this node (scoped authority, revocation)",
          "`gateway` + `tls`; `with_execution_authority`",
          &["gateway_authority::ExecutionAuthority"], "docs/guide/21-mandates.md",
          gateway_role,
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { if c.execution_authority.get().is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_execution_authority" } } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("authz.durable_epochs", 1, "authority", Node,
          "a restart does not restore revoked authority: installed epochs are journalled first",
          "`gateway` + `tls`; `ExecutionAuthority::with_durable_epochs`",
          &["gateway_authority::install_epoch_durably"], "docs/guide/21-mandates.md",
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { if c.execution_authority.get().is_some() { None } else { Some("no execution authority attached") } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Some("no execution authority in this build (`gateway` + `tls` off)") }
          },
          |c| {
              #[cfg(all(feature = "gateway", feature = "tls"))]
              { match c.execution_authority.get() { Some(a) if a.has_durable_epochs() => Resolution::Enforced, _ => Resolution::NotConfigured { missing: "ExecutionAuthority::with_durable_epochs (epochs are in memory only)" } } }
              #[cfg(not(all(feature = "gateway", feature = "tls")))]
              { let _ = c; Resolution::NotInBuild { feature: "gateway + tls" } }
          }),
        g("audit.chain", 1, "audit", Node,
          "governance actions at the gateway are sealed into the tamper-evident chain",
          "`compliance`; `[tls]` (records are sealed with the node identity)",
          &["audit::seal_and_write"], "docs/operations/audit.md",
          gateway_role,
          |c| {
              #[cfg(feature = "compliance")]
              { if c.config.tls.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[tls] — audit records require the tls identity; without it nothing is sealed" } } }
              #[cfg(not(feature = "compliance"))]
              { let _ = c; Resolution::NotInBuild { feature: "compliance" } }
          }),
        g("audit.sink", 1, "audit", Node,
          "sealed records are mirrored to an external sink (SIEM / WORM), original bytes kept",
          "`compliance`; `[tls]`; `with_audit_sink`",
          &["lifecycle (sink drain)"], "docs/operations/audit.md",
          gateway_role,
          |c| {
              #[cfg(feature = "compliance")]
              {
                  if c.audit_sink.get().is_none() { return Resolution::NotConfigured { missing: "with_audit_sink" }; }
                  if c.config.tls.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[tls] — nothing is sealed without the identity, so the sink receives nothing" } }
              }
              #[cfg(not(feature = "compliance"))]
              { let _ = c; Resolution::NotInBuild { feature: "compliance" } }
          }),
        g("egress.allow_list", 1, "egress", Node,
          "the substrate's own outbound calls are restricted to a hostname allow-list (MCP bridge, LLM, probes, skillrunner, the wasm host, the wiki sink, the federation client, OIDC) — the first URL and every redirect hop, with the host read the way the client reads it",
          "`egress.allow_hosts` non-empty (empty allows all); the federation client (the node's policy applied to clients it is handed, `FederationClient::with_egress` elsewhere) and OIDC discovery + JWKS since 2026-10-03 (a denied issuer refuses `start()`). Since 2.23.0: every redirect hop re-checked (≤ 5, never https → http), none followed by a client without the policy or carrying a credential header (`mycelium::egress_client`); the host parsed with the client's own WHATWG parser; an object store gated on the endpoint it dials, not its bucket. An `HttpLibrarySource` or `OllamaProbe` built outside an agent is gated only when built `with_egress`. **Not covered:** name resolution (an allowed name resolving to a denied address), a cloud identity's credential traffic, redirects inside `object_store`'s own client",
          &["config::EgressPolicy::permits_url"], "docs/operations/crown-jewel.md",
          always,
          |c| if c.config.egress.allow_hosts.is_empty() { Resolution::NotConfigured { missing: "egress.allow_hosts (empty allows all)" } } else { Resolution::Enforced }),
        g("persist.configured", 1, "persistence", Node,
          "a restart recovers the KV and acceptor memory from disk",
          "`[persistence]`",
          &["lifecycle (WAL replay, snapshot)"], "docs/operations/deployment.md",
          always,
          |c| if c.config.persistence.is_some() { Resolution::Enforced } else { Resolution::NotConfigured { missing: "[persistence]" } }),
        g("persist.sync_mode", 1, "persistence", Node,
          "a WAL acknowledgement is a durability claim: the write is on disk before the ack",
          "`[persistence]` with `sync_mode` other than `async`",
          &["wal::append (SyncMode)"], "docs/design/contracts-receipts.md",
          |c| if c.config.persistence.is_some() { None } else { Some("no `[persistence]` configured") },
          |c| match c.config.persistence.as_ref().map(|p| p.sync_mode) {
              Some(crate::config::SyncMode::Async) => Resolution::NotConfigured { missing: "sync_mode (async: an ack is `buffered`, which the receipt says honestly)" },
              Some(_) => Resolution::Enforced,
              None => Resolution::NotConfigured { missing: "[persistence]" },
          }),
        g("persist.unreadable_refused", 1, "persistence", Node,
          "a node does not start over persisted state it could not read — nothing is compacted over an unreadable snapshot or WAL",
          "`[persistence]` with `on_unreadable = \"refuse\"` (the default)",
          &["lifecycle (replay; `persistence::quarantine_unreadable`)", "persistence::do_snapshot (aborts on a corrupt record)"], "docs/operations/deployment.md",
          |c| if c.config.persistence.is_some() { None } else { Some("no `[persistence]` configured") },
          |c| match c.config.persistence.as_ref().map(|p| p.on_unreadable) {
              Some(crate::config::OnUnreadable::Refuse) => Resolution::Enforced,
              Some(crate::config::OnUnreadable::Quarantine) => Resolution::NotConfigured { missing: "on_unreadable = \"refuse\" (quarantine moves unreadable files aside and starts from what was readable)" },
              None => Resolution::NotConfigured { missing: "[persistence]" },
          }),
        g("at_rest.cipher", 1, "persistence", Node,
          "the WAL and snapshot are encrypted at rest",
          "`[persistence]`; `with_data_at_rest_cipher`, before `start()`",
          &["lifecycle (cipher read once at start)"], "docs/operations/crown-jewel.md",
          |c| if c.config.persistence.is_some() { None } else { Some("no `[persistence]` configured") },
          |c| if c.at_rest_cipher_attached.load(std::sync::atomic::Ordering::Acquire) { Resolution::Enforced } else { Resolution::NotConfigured { missing: "with_data_at_rest_cipher (plaintext WAL and snapshot)" } }),
        g("cons.safety_profile", 1, "consensus", Ext,
          "safety-sensitive agreement runs the supported profile: a fixed voter set, a strict-majority quorum, identical trust slices",
          "chosen per proposal in `ConsensusConfig`; nothing validates it (plan §8)",
          &["consensus::propose"], "docs/threat-model.md",
          always,
          external("ConsensusConfig is chosen per proposal, not per node; the profile is a documented condition until I3 validates it")),
        g("net.confinement", 1, "deployment", Ext,
          "agent pods reach nothing but their gateway",
          "the network: separate pods, an enforcing CNI, a NetworkPolicy",
          &[], "docs/design/confined-fleet.md",
          always,
          external("the reference deployment's test (scripts/test-confined-fleet.sh) or the network's own telemetry — a node cannot observe its own network policy")),
        g("clock.sync", 1, "deployment", Ext,
          "this node's wall clock is within the bound every expiry and freshness check assumes",
          "deployment time-sync (NTP or equivalent)",
            &[], "docs/threat-model.md",
          always,
          external("the deployment's time-sync monitoring; a node cannot verify its own clock against real time (closure plan C11)")),
    ]
}


/// The guarantee catalogue, generated (plan I2's deferred golden, delivered 2026-10-03): every core
/// descriptor — id, revision, subsystem, kind, promise, what it needs, its enforcement points and
/// docs — and a **state matrix**: each guarantee's resolution on an unstarted node under a few
/// reference configurations, in the build the generator ran under. Never hand-edited; the gate
/// (`guarantee::tests::the_checked_in_guarantee_catalogue_is_current`) fails when the descriptors or
/// their resolutions move, and `UPDATE_GUARANTEE_CATALOGUE=1` regenerates.
pub fn catalogue_markdown(matrix: &[(&str, GuaranteeReport)]) -> String {
    let descs = core_guarantees();
    let mut out = String::new();
    out.push_str("# Guarantee catalogue\n\n**Generated** from `core_guarantees()` (`src/agent/guarantee.rs`); do not edit. Regenerate with `UPDATE_GUARANTEE_CATALOGUE=1 cargo test --lib --features compliance,a2a the_checked_in_guarantee_catalogue_is_current`. A guarantee is a claim the startup report resolves against the node as built and configured — `enforced` · `not_configured` (the setting named) · `not_in_build` (the feature named) · `not_applicable` (the role fact named) · `not_verifiable_here` (external prerequisites, never counted). The plan is `docs/plans/guarantees-and-rule-catalogue.md`; the live report is `guarantee_report()` / `GET /gateway/guarantees`.\n\n");
    out.push_str(&format!("Schema `{REPORT_SCHEMA}` · {} core guarantees.\n\n", descs.len()));
    out.push_str("## The descriptors\n\n| Id | Rev | Subsystem | Kind | Promise | Needs | Enforcement points | Docs |\n|---|---|---|---|---|---|---|---|\n");
    let esc = |t: &str| t.replace('|', "\\|");
    for d in &descs {
        out.push_str(&format!("| `{}` | {} | {} | {:?} | {} | {} | {} | `{}` |\n", d.id, d.revision, d.subsystem, d.kind, esc(d.promise), esc(d.needs),
            d.enforcement_points.iter().map(|e| format!("`{}`", esc(e))).collect::<Vec<_>>().join(", "), d.docs));
    }
    out.push_str("\n## The state matrix\n\nEach guarantee's resolution on an **unstarted** node (nothing attached) under the reference configurations, in the build the generator ran under (`compliance,a2a`, which implies `gateway` + `tls`). A column is a configuration, a cell is the state the report would log at `start()`; what is missing or not applicable is on the report itself, not here.\n\n");
    out.push_str("| Id |");
    for (name, _) in matrix { out.push_str(&format!(" {name} |")); }
    out.push_str("\n|---|");
    for _ in matrix { out.push_str("---|"); }
    out.push('\n');
    for d in &descs {
        out.push_str(&format!("| `{}` |", d.id));
        for (_, r) in matrix {
            out.push_str(&format!(" {} |", r.entry(d.id).map(|e| e.resolution.state()).unwrap_or("—")));
        }
        out.push('\n');
    }
    out
}

/// The catalogue's JSON half: the descriptors, build-independent.
pub fn catalogue_json() -> String {
    #[derive(Serialize)]
    struct Row<'a> { id: &'a str, revision: u32, subsystem: &'a str, kind: GuaranteeKind, promise: &'a str, needs: &'a str, enforcement_points: &'a [&'a str], docs: &'a str }
    #[derive(Serialize)]
    struct Doc<'a> { schema: &'static str, guarantees: Vec<Row<'a>> }
    let descs = core_guarantees();
    let rows = descs.iter().map(|d| Row { id: d.id, revision: d.revision, subsystem: d.subsystem, kind: d.kind, promise: d.promise, needs: d.needs, enforcement_points: d.enforcement_points, docs: d.docs }).collect();
    serde_json::to_string_pretty(&Doc { schema: REPORT_SCHEMA, guarantees: rows }).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};

    fn agent(cfg: GossipConfig) -> GossipAgent {
        GossipAgent::new(NodeId::new("127.0.0.1", crate::test_util::alloc_port()).unwrap(), cfg)
    }
    fn state(r: &GuaranteeReport, id: &str) -> &'static str {
        r.entry(id).unwrap_or_else(|| panic!("{id} registered")).resolution.state()
    }

    /// Every core id is unique, and the registry starts with exactly the core set.
    #[test]
    fn the_core_registry_has_unique_ids() {
        let ids = core_ids();
        let mut dedup = ids.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(ids.len(), dedup.len(), "duplicate core id");
        let r = agent(GossipConfig::default()).guarantee_report();
        assert_eq!(r.entries.len(), ids.len());
        assert_eq!(r.schema, REPORT_SCHEMA);
        assert!(!r.started);
        assert_eq!(r.config_digest.len(), 64);
    }

    /// A default node runs no gateway: every gateway guarantee is `NotApplicable` with the role fact,
    /// not unmet — and the external prerequisites are unresolved, never met.
    #[test]
    fn a_node_with_no_gateway_does_not_fail_a_gateway_guarantee() {
        let r = agent(GossipConfig::default()).guarantee_report();
        for id in ["gw.not_open", "gw.tls", "ae.authorised_at_seam", "audit.sink"] {
            assert_eq!(state(&r, id), "not_applicable", "{id}");
            let (_, because) = r.not_applicable().into_iter().find(|(i, _)| *i == id).unwrap();
            assert!(because.contains("http_port"), "the role fact is named: {because}");
        }
        assert!(!r.unmet().contains(&"gw.not_open"), "not applicable is not unmet");
        for id in ["net.confinement", "clock.sync", "cons.safety_profile"] {
            assert_eq!(state(&r, id), "not_verifiable_here", "{id}");
            assert!(r.unresolved().contains(&id));
            assert!(!r.unmet().contains(&id), "an external prerequisite is never counted, either way");
        }
    }

    /// `NotApplicable` is never a waiver: once the role fact holds, an unmet guarantee is unmet.
    #[test]
    fn once_a_gateway_is_configured_its_guarantees_apply_and_an_open_one_is_unmet() {
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(crate::test_util::alloc_port());
        let r = agent(cfg).guarantee_report();
        assert_eq!(state(&r, "gw.not_open"), "not_configured");
        assert!(r.unmet().contains(&"gw.not_open"));
        assert!(r.node_requirements_satisfied(), "under `dev` nothing is required, and the report says what is not enforced anyway");
        assert!(r.required_unmet().is_empty());
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(crate::test_util::alloc_port());
        cfg.gateway_auth_token = Some("s3cret".into());
        let r = agent(cfg).guarantee_report();
        assert_eq!(state(&r, "gw.not_open"), "enforced");
        assert!(!r.unmet().contains(&"gw.not_open"));
    }

    /// A setting this build cannot act on reads `NotInBuild`, never `NotConfigured` — the report
    /// distinguishes *you did not set it* from *this build could not honour it*.
    #[cfg(not(feature = "compliance"))]
    #[test]
    fn a_compliance_guarantee_reads_not_in_build_without_the_feature() {
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(crate::test_util::alloc_port());
        let r = agent(cfg).guarantee_report();
        for id in ["gw.token_tables", "gw.oidc", "audit.chain", "audit.sink"] {
            assert_eq!(state(&r, id), "not_in_build", "{id}");
        }
    }

    #[cfg(feature = "compliance")]
    #[test]
    fn a_compliance_guarantee_resolves_on_its_setting_with_the_feature() {
        let mut cfg = GossipConfig::default();
        cfg.http_port = Some(crate::test_util::alloc_port());
        let r = agent(cfg.clone()).guarantee_report();
        assert_eq!(state(&r, "gw.token_tables"), "not_configured");
        cfg.gateway_named_tokens = vec![crate::GatewayNamedToken { name: "ops".into(), token: "t".into(), scopes: vec!["*".into()] }];
        let r = agent(cfg).guarantee_report();
        assert_eq!(state(&r, "gw.token_tables"), "enforced");
        assert_eq!(state(&r, "gw.not_open"), "enforced", "a token table closes the gateway");
    }

    /// Persistence: the sync-mode and cipher guarantees apply only once persistence is configured,
    /// and resolve on their own settings after that.
    #[test]
    fn persistence_guarantees_follow_the_configuration() {
        let r = agent(GossipConfig::default()).guarantee_report();
        assert_eq!(state(&r, "persist.configured"), "not_configured");
        assert_eq!(state(&r, "persist.sync_mode"), "not_applicable");
        assert_eq!(state(&r, "at_rest.cipher"), "not_applicable");
        let mut cfg = GossipConfig::default();
        cfg.persistence = Some(crate::config::PersistenceConfig { on_unreadable: Default::default(), base_path: std::env::temp_dir().join(format!("g-{}", crate::test_util::alloc_port())), sync_mode: crate::config::SyncMode::Async, snapshot_wal_threshold: 1_000_000, snapshot_interval_secs: 3_600 });
        let r = agent(cfg).guarantee_report();
        assert_eq!(state(&r, "persist.configured"), "enforced");
        assert_eq!(state(&r, "persist.sync_mode"), "not_configured", "the default sync mode is async");
        assert_eq!(state(&r, "at_rest.cipher"), "not_configured");
    }

    /// A registration from outside the core: accepted once, refused as a duplicate, refused as an
    /// override of a core id — a requirement must not vanish with, or be redefined by, its registrar.
    #[test]
    fn registrations_are_unique_and_cannot_override_the_core() {
        let a = agent(GossipConfig::default());
        let mine = || GuaranteeDescriptor::new("companion.thing", 1, "companion", GuaranteeKind::NodeEnforced, "p", "n", &[], "d", |_| None, |v| if v.config().persistence.is_none() { Resolution::NotConfigured { missing: "[persistence]" } } else { Resolution::Enforced });
        a.register_guarantee(mine()).expect("first registration");
        let dup = a.register_guarantee(mine()).unwrap_err();
        assert!(dup.to_string().contains("already registered"), "{dup}");
        let core = a.register_guarantee(GuaranteeDescriptor::new("egress.allow_list", 9, "x", GuaranteeKind::NodeEnforced, "p", "n", &[], "d", |_| None, |_| Resolution::Enforced)).unwrap_err();
        assert!(core.to_string().contains("core guarantee"), "{core}");
        let r = a.guarantee_report();
        assert_eq!(state(&r, "companion.thing"), "not_configured", "the companion's check read the config through the view");
        assert_eq!(state(&r, "egress.allow_list"), "not_configured", "the core result stood");
    }

    /// `dev` is the default, requires nothing, and the report says it was not selected.
    #[test]
    fn the_dev_profile_requires_nothing_and_the_report_says_so() {
        let r = agent(GossipConfig::default()).guarantee_report();
        let p = r.profile.as_ref().expect("a profile is always reported");
        assert_eq!((p.name, p.revision, p.selected), ("dev", 1, false));
        assert!(p.required.is_empty());
        assert!(check(&r, &DEV).is_ok());
        let mut cfg = GossipConfig::default();
        cfg.profile = Some("dev".into());
        assert!(agent(cfg).guarantee_report().profile.unwrap().selected);
    }

    /// An unknown profile name is refused by `validate()`, naming the known ones.
    #[test]
    fn an_unknown_profile_name_is_refused_by_validate() {
        let mut cfg = GossipConfig::default();
        cfg.profile = Some("prod".into());
        let e = cfg.validate().unwrap_err();
        assert!(matches!(e, GossipError::InvalidField { field: "profile", .. }), "{e}");
        assert!(e.to_string().contains("secure-single-domain"), "{e}");
        let mut ok = GossipConfig::default();
        ok.profile = Some("secure-single-domain".into());
        assert!(ok.validate().is_ok());
    }

    /// Plan G10 and G11 at the profile: a required guarantee that does not apply passes; a required
    /// id the registry does not hold refuses — a requirement does not vanish with its registrar.
    #[test]
    fn a_requirement_that_does_not_apply_passes_and_an_unknown_one_refuses() {
        let r = agent(GossipConfig::default()).guarantee_report(); // no gateway
        let gw_only = Profile { name: "t", revision: 1, required: &["gw.not_open"], about: "" };
        assert!(check(&r, &gw_only).is_ok(), "not applicable is not unmet");
        let ghost = Profile { name: "t", revision: 1, required: &["wiki.nothing"], about: "" };
        let e = check(&r, &ghost).unwrap_err();
        assert_eq!(e.unknown, vec!["wiki.nothing"]);
        assert!(e.to_string().contains("does not vanish"), "{e}");
    }

    /// The secure profile's set is pinned (the readiness checklist copies it; a change is a revision
    /// bump and a release note), every id is a core guarantee, and none is an external prerequisite.
    #[test]
    fn the_secure_profiles_required_set_is_pinned() {
        assert_eq!(SECURE_SINGLE_DOMAIN.revision, 2);
        assert_eq!(SECURE_SINGLE_DOMAIN.required, &[
            "mesh.tls", "id.proofs_required", "gw.not_open", "gw.tls", "gw.caller_profile",
            "ae.authorised_at_seam", "ae.recorded_before_dispatch", "prov.enforcement", "a2a.admission",
            "authz.execution_authority", "authz.durable_epochs", "audit.chain", "egress.allow_list",
            "persist.configured", "persist.sync_mode", "persist.unreadable_refused", "id.ca_key_off_node",
        ]);
        let r = agent(GossipConfig::default()).guarantee_report();
        for id in SECURE_SINGLE_DOMAIN.required {
            assert!(core_ids().contains(id), "{id} is a core guarantee");
            assert_eq!(r.entry(id).unwrap().kind, GuaranteeKind::NodeEnforced, "{id}: an external prerequisite can never be met from inside the node");
        }
    }

    /// Plan G5: under `secure-single-domain` an open node refuses to start, naming each unmet
    /// guarantee with what is missing and where to read, and never passes the boundary.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn the_secure_profile_refuses_an_open_node_by_name() {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(crate::test_util::alloc_port());
        cfg.profile = Some("secure-single-domain".into());
        let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        let e = a.start().await.unwrap_err().to_string();
        assert!(e.contains("profile `secure-single-domain` rev 2"), "{e}");
        for id in ["gw.not_open", "mesh.tls", "egress.allow_list", "persist.configured", "ae.authorised_at_seam"] {
            assert!(e.contains(id), "{id} is named: {e}");
        }
        assert!(e.contains("not_configured") && e.contains("docs/operations/rbac.md"), "what is missing and where to read: {e}");
        assert!(!a.guarantee_report().started, "the boundary was not passed");
    }

    /// Plan G13: an attachment after `start()` is counted and the report says so, because the block
    /// logged at the boundary did not see it. Before the boundary nothing is counted.
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn an_attachment_after_start_is_counted_as_late() {
        let port = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let a = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
        a.with_provider_enforcement();
        assert_eq!(a.late_attachments(), 0, "before the boundary nothing is late");
        assert!(!a.guarantee_report().started);
        a.start().await.expect("start");
        let r = a.guarantee_report();
        assert!(r.started);
        assert_eq!(r.entry("prov.enforcement").unwrap().resolution, Resolution::Enforced);
        a.with_provider_enforcement(); // a second call is ignored by the attacher, but it is still late
        assert_eq!(a.late_attachments(), 1);
        assert_eq!(a.guarantee_report().late_attachments, 1, "the live report carries the count");
        a.shutdown().await;
    }

    /// The report's shape is pinned: the serialised form carries the schema id, and every entry's
    /// state is one of the five names — a consumer can switch on them.
    #[test]
    fn the_report_serialises_with_its_schema_and_five_states() {
        let r = agent(GossipConfig::default()).guarantee_report();
        let v: serde_json::Value = serde_json::to_value(&r).unwrap();
        assert_eq!(v["schema"], REPORT_SCHEMA);
        let states: std::collections::BTreeSet<String> = v["entries"].as_array().unwrap().iter().map(|e| e["resolution"]["state"].as_str().unwrap().to_string()).collect();
        for s in states { assert!(["enforced", "not_configured", "not_in_build", "not_applicable", "not_verifiable_here"].contains(&s.as_str()), "{s}"); }
    }

    /// Plan I2's deferred golden, delivered: the generated guarantee catalogue — descriptors and the
    /// state matrix over reference configurations — is checked in and current. Gated under the CI
    /// feature set the matrix is generated in (`compliance,a2a`), so a resolver that moves, a new
    /// guarantee, or a changed descriptor fails here until the document is regenerated in the open.
    #[cfg(all(feature = "compliance", feature = "a2a"))]
    #[test]
    fn the_checked_in_guarantee_catalogue_is_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let tmp = std::env::temp_dir().join(format!("mycelium-gcat-{}", std::process::id()));
        let mut persisted = GossipConfig::default();
        persisted.persistence = Some(crate::config::PersistenceConfig { base_path: tmp.clone(), sync_mode: crate::config::SyncMode::Flush, snapshot_wal_threshold: 10, snapshot_interval_secs: 300, on_unreadable: Default::default() });
        persisted.egress.allow_hosts = vec!["api.example".into()];
        let mut dev = GossipConfig::default();
        dev.profile = Some("dev".into());
        let mut gateway = GossipConfig::default();
        gateway.http_port = Some(1);
        gateway.gateway_auth_token = Some("t".into());
        let configs = [("default", GossipConfig::default()), ("`dev` profile", dev), ("persistence + egress", persisted), ("gateway + bearer", gateway)];
        let matrix: Vec<(&str, GuaranteeReport)> = configs.iter().map(|(n, c)| (*n, agent(c.clone()).guarantee_report())).collect();
        let md = catalogue_markdown(&matrix);
        let json = catalogue_json();
        let (mp, jp) = (root.join("docs/reference/guarantee-catalogue.md"), root.join("docs/reference/guarantee-catalogue.json"));
        if std::env::var("UPDATE_GUARANTEE_CATALOGUE").is_ok() {
            std::fs::write(&mp, &md).unwrap();
            std::fs::write(&jp, &json).unwrap();
            return;
        }
        let have_md = std::fs::read_to_string(&mp).unwrap_or_default();
        let have_json = std::fs::read_to_string(&jp).unwrap_or_default();
        assert!(have_md == md && have_json == json, "docs/reference/guarantee-catalogue.{{md,json}} are stale: regenerate with UPDATE_GUARANTEE_CATALOGUE=1 cargo test --lib --features compliance,a2a the_checked_in_guarantee_catalogue_is_current");
    }
}
