//! **This node's authority from its declaration** — `[authority]` (identity-and-lifecycle I0).
//!
//! A node built from configuration alone — the `mycelium` binary — had no way to attach an action evaluator, an
//! execution authority, provider enforcement or an evidence journal: those were `with_*` calls an embedder made in
//! code, so every door of the published node enforced nothing. With `[authority]` set, `start()` does what an
//! embedder's program would:
//!
//! 1. **reads the declaration** — every `*.toml` in `declarations` is a unit file; the unit whose `principal` is
//!    `unit` is this node's ([`plan`]);
//! 2. **builds the reference evaluator from that unit's `[[rule]]`s only**, each as written and, for the forms the
//!    doors present, two more readings — a bare `tool:<name>` also at this node (`tool:<name>@<node>`, the resource
//!    the provider's door names), and a grant-requiring rule whose actor is the unit's own principal also for this
//!    node's principal (`node:<id>`, the one principal that holds a node's grant). That is what `mycelium test
//!    --faults grant` builds ([`crate::faults`]'s `grant` class) narrowed to one unit: a declaration carries no
//!    placement, so another unit's rule cannot be qualified to the node it runs on, and is not loaded;
//! 3. **maps every door this build serves** ([`super::http::evaluated_doors`] and the provider's protected kinds), so
//!    a door's request is decided by the rules rather than refused *unmapped*, and refuses the start naming any door
//!    the evaluator leaves unmapped ([`check_doors`]);
//! 4. **constructs the execution authority** (the strict gate over `scope`, the tier and the freshness bound; grants
//!    entitled for this node, or the fleet);
//! 5. at `start()`, **opens the journal and attaches all four** before the lifecycle boundary ([`attach`]).
//!
//! **Refused at start, by name:** an unreadable or empty directory; no unit, or two, with that principal; a rule
//! that does not build; an exposure rule (Y5 — `[authority]` attaches no exposure policy, and a rule accepted and
//! ignored is the v2.18.1 class); a gate the strict profile refuses; an evaluator already attached in code (one
//! source of authority per node); a journal that does not open; and a build without `compliance`.
//!
//! **Stated, not solved:** a grant issued at this gateway for an agent on another node is evaluated here under this
//! unit's rules, so the issuing rule (`requires_grant`, its issuer) must be declared in every unit that may issue,
//! as the scaffolds do. A rule's actor is matched exactly: an issuer at this gateway is a gateway principal
//! (`token:<issuer>/<name>`, `oidc:<issuer>/<subject>`), so the rule names it so. **Not built:** other units'
//! rules; the Cedar evaluator from configuration (private); a gate spanning more than one `ResourceAuthority`;
//! mandate authorities' keys (`TrustedExternalIssuers` is empty — row I1 makes it a shared source).

use std::path::PathBuf;
use std::sync::Arc;

use super::action_evaluator::{ActionEvaluator, MappingStatus, ReferenceEvaluator};
use super::gateway_authority::ExecutionAuthority;
use crate::config::{AuthorityConfig, AuthorityTier, GossipConfig, GrantEntitlement};
use crate::error::GossipError;
use crate::NodeId;

/// The authority a declaration describes for this node, built and checked — everything but the journal, which is
/// opened at `start()` ([`attach`]).
pub(crate) struct DeclaredAuthority {
    /// The unit's file name (its stem), for the report.
    pub(crate) unit_file: String,
    /// The unit's principal (`[authority].unit`).
    pub(crate) principal: String,
    /// How many `[[rule]]`s of the unit were loaded, as written.
    pub(crate) rules: usize,
    pub(crate) evaluator: Arc<dyn ActionEvaluator>,
    pub(crate) authority: Arc<ExecutionAuthority>,
    pub(crate) journal: PathBuf,
    /// Where the execution authority's epochs are journaled: `<persistence>/<node>/authority/epochs.journal`.
    pub(crate) epochs: PathBuf,
    /// Every door mapped, as `(enforcement point, operation)`.
    pub(crate) doors: Vec<(String, String)>,
}

fn refused(reason: impl Into<String>) -> GossipError {
    GossipError::InvalidField { field: "authority", reason: reason.into() }
}

/// Every door the evaluator must map on this node: the gateway's ([`super::http::evaluated_doors`]) and the
/// provider's — each protected kind, under the operation `provider_enforcement` derives for it (`mcp.invoke` is
/// `tools/call`; every other kind is its own name).
pub(crate) fn doors(cfg: &GossipConfig) -> Vec<(String, String)> {
    let mut doors: Vec<(String, String)> = super::http::evaluated_doors().into_iter().map(|(p, o)| (p.to_string(), o.to_string())).collect();
    let provider = super::provider_enforcement::ENFORCEMENT_POINT_PROVIDER;
    for kind in super::BUILTIN_PROTECTED_RPC_KINDS.iter().map(|k| k.to_string()).chain(cfg.protected_rpc_kinds.iter().cloned()) {
        let operation = if kind == crate::signal::signal_kind::MCP_INVOKE { "tools/call".to_string() } else { kind };
        doors.push((provider.to_string(), operation));
    }
    doors.sort();
    doors.dedup();
    doors
}

/// Refuse a door the evaluator leaves unmapped: its requests would be refused before any rule is read, so the
/// node would start with a door that enforces by accident rather than by declaration.
pub(crate) fn check_doors(evaluator: &dyn ActionEvaluator, doors: &[(String, String)]) -> Result<(), GossipError> {
    let unmapped: Vec<String> = doors
        .iter()
        .filter(|(_, op)| evaluator.mapping(op, "*").status != MappingStatus::Mapped)
        .map(|(point, op)| format!("{point} ({op})"))
        .collect();
    if unmapped.is_empty() {
        Ok(())
    } else {
        Err(refused(format!("the evaluator built from the declaration leaves {} unmapped: {}", if unmapped.len() == 1 { "a door" } else { "doors" }, unmapped.join(", "))))
    }
}

/// What can be told about the journal's path without opening it — the doctor's check, and `start()`'s before it binds:
/// not a directory, and if it exists a file this process may write. A path whose parent does not exist yet is not
/// refused (the journal creates it). Its one `symlink_metadata` is start-time configuration I/O, outside the replay
/// seams like the unit loader's reads, and recorded in `scripts/sim-seams-baseline.txt`.
fn check_journal_path(path: &std::path::Path) -> Result<(), GossipError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => Err(refused(format!("journal {}: is a directory", path.display()))),
        Ok(m) if !m.is_file() => Err(refused(format!("journal {}: is not a regular file", path.display()))),
        Ok(m) if m.permissions().readonly() => Err(refused(format!("journal {}: is read-only", path.display()))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(refused(format!("journal {}: {e}", path.display()))),
    }
}

/// The policy revision: the unit's principal and its rules as parsed, hashed — a changed rule is a new revision, so a
/// grant bound to the old one is not admitted under the new rules; a comment or a reordered key in the file is not.
fn revision(bytes: &[u8]) -> String {
    use sha2::Digest;
    let d = sha2::Sha256::digest(bytes);
    let hex: String = d.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("declared:{hex}")
}

/// Read `[authority]`'s declaration and build this node's authority from it, refusing by name what cannot be
/// built. Pure over files: no port, no journal opened.
pub(crate) fn plan(cfg: &GossipConfig, node: &NodeId, auth: &AuthorityConfig) -> Result<DeclaredAuthority, GossipError> {
    // A grant's request, its answer and each door's use marker are KV records the WAL keeps, and the epochs are a
    // node-local journal under the persistence directory: without `[persistence]` a restart forgets which grants
    // were used, and a used grant would admit again.
    let Some(persistence) = &cfg.persistence else {
        return Err(refused("[authority] needs [persistence]: a grant's request, its answer and each door's use marker are kept by the WAL, and the execution authority's epochs under the persistence directory; without it a restart forgets which grants were used"));
    };
    check_journal_path(&auth.journal)?;
    let epochs = persistence.base_path.join(node.to_string()).join("authority").join("epochs.journal");
    let dir = &auth.declarations;
    let units = crate::faults::load_units(dir).map_err(|e| refused(format!("declarations: {e}")))?;
    if units.is_empty() {
        return Err(refused(format!("declarations: {} holds no unit file (*.toml)", dir.display())));
    }
    let mine: Vec<&crate::wire_check::Unit> = units.iter().filter(|u| u.config.principal.as_deref() == Some(auth.unit.as_str())).collect();
    let unit = match mine.as_slice() {
        [u] => *u,
        [] => {
            return Err(refused(format!(
                "no unit in {} declares principal = {:?} (the units there declare {})",
                dir.display(),
                auth.unit,
                units.iter().map(|u| format!("{} → {}", u.name, u.config.principal.as_deref().unwrap_or("no principal"))).collect::<Vec<_>>().join(", ")
            )))
        }
        more => {
            return Err(refused(format!(
                "{} units in {} declare principal = {:?} ({}): one unit is this node's",
                more.len(),
                dir.display(),
                auth.unit,
                more.iter().map(|u| u.name.as_str()).collect::<Vec<_>>().join(", ")
            )))
        }
    };
    if let Some(r) = unit.config.rules.iter().find(|r| r.is_exposure()) {
        return Err(refused(format!(
            "unit {}: the exposure rule on {} {} is not attached by [authority] (it needs an exposure policy and a source, attached in code: with_exposure_policy, with_exposure_source); attach them in code, or remove the rule",
            unit.name, r.operation, r.resource
        )));
    }
    // The rules as parsed, not the file re-read: a second read of the declaration would be a filesystem call outside
    // the replay seams (`scripts/check-sim-seams.sh`), and the rules are what the revision is of.
    let revision = revision(format!("{}\n{:?}", auth.unit, unit.config.rules).as_bytes());
    let node_principal = super::gateway_caller::node_principal(node);
    let mut ev = ReferenceEvaluator::new(revision.clone()).with_catalogue("declaration", revision);
    for r in &unit.config.rules {
        let built = r.to_rule().map_err(|e| refused(format!("unit {}: [[rule]] {} {} {}: {e}", unit.name, r.actor, r.operation, r.resource)))?;
        let mut forms = vec![built.clone()];
        // The doors present a tool or a skill at the node that serves it (`tool:<name>@<node>`,
        // `skill:<ns>/<name>@<node>`); a bare one in the unit is qualified to this node — the only node a declaration
        // can place this unit's rules on. A tool or skill on another node is not admitted here.
        if (r.resource.starts_with("tool:") || r.resource.starts_with("skill:")) && !r.resource.contains('@') {
            let mut at_node = built;
            at_node.resource = format!("{}@{node}", r.resource);
            forms.push(at_node);
        }
        if r.requires_grant && r.actor == auth.unit {
            for f in forms.clone() {
                let mut as_node = f;
                as_node.actor = node_principal.clone();
                forms.push(as_node);
            }
        }
        for f in forms {
            ev = ev.allow(f);
        }
    }
    let doors = doors(cfg);
    for (_, op) in &doors {
        ev = ev.map_action(op.clone(), "*");
    }
    // An operation a rule names that no door serves is mapped too: the rule is the declaration's, and a door that
    // is added later decides under it rather than refusing it unmapped.
    for r in &unit.config.rules {
        if r.operation != "*" {
            ev = ev.map_action(r.operation.clone(), "*");
        }
    }
    check_doors(&ev, &doors)?;

    use crate::knowledge::issuer::TrustedExternalIssuers;
    use crate::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
    use crate::mandate::grant::{EntitlementTable, GrantVerifier};
    use crate::mandate::{PrincipalId, ResourceAuthority};
    let tier = match auth.tier {
        AuthorityTier::Serialised => ResourceTier::Serialised,
        AuthorityTier::Transactional => ResourceTier::Transactional,
        AuthorityTier::Advisory => ResourceTier::Advisory,
    };
    let f = auth.freshness_ms;
    let scope = auth.scope.clone().unwrap_or_else(|| unit.name.clone());
    let gate = ExecutionGate::strict(
        ResourceAuthority::new(scope, 1),
        tier,
        ClockModel { skew_ms: auth.skew_ms },
        FreshnessPolicy { freshness_ms: f, interval_ms: f / 4, delivery_ms: f / 12 },
    )
    .map_err(|e| refused(format!("the execution gate refuses tier {:?} with freshness_ms {f} (checkpoint every {} ms, delivery {} ms) under skew_ms {}: {e:?}", auth.tier, f / 4, f / 12, auth.skew_ms)))?;
    let mut entitlements = EntitlementTable::new();
    let me = PrincipalId::new(&node_principal).ok_or_else(|| refused(format!("{node_principal} is not a principal")))?;
    entitlements.entitle_grants(me);
    if auth.entitle_grants == GrantEntitlement::Fleet {
        entitlements.entitle_grants_from_fleet();
    }
    let authority = Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitlements), TrustedExternalIssuers::new()));
    Ok(DeclaredAuthority {
        unit_file: unit.name.clone(),
        principal: auth.unit.clone(),
        rules: unit.config.rules.len(),
        evaluator: Arc::new(ev),
        authority,
        journal: auth.journal.clone(),
        epochs,
        doors,
    })
}

/// Attach a planned authority to a node about to start: the journal opened `Strict` first, the durable epochs opened
/// and given to the execution authority, then the evaluator, the authority and provider enforcement. Marks the node's
/// attachments as the table's, so a retried `start()` does not read them as code's.
pub(crate) fn attach(agent: &super::GossipAgent, planned: &DeclaredAuthority) -> Result<(), GossipError> {
    let journal = super::evidence_journal::EvidenceJournal::open(&planned.journal, super::evidence_journal::EvidenceProfile::Strict)
        .map_err(|e| refused(format!("journal {}: {e}", planned.journal.display())))?;
    let epochs = crate::mandate::authority::DurableEpochs::open(&planned.epochs).map_err(|e| refused(format!("epochs {}: {e:?}", planned.epochs.display())))?;
    planned.authority.with_durable_epochs(epochs);
    agent.with_evidence_journal(journal);
    agent.with_action_evaluator(Arc::clone(&planned.evaluator));
    agent.with_execution_authority(Arc::clone(&planned.authority));
    agent.with_provider_enforcement();
    agent.task_ctx.declared_attached.store(true, std::sync::atomic::Ordering::Release);
    tracing::info!(
        unit = %planned.unit_file,
        principal = %planned.principal,
        rules = planned.rules,
        doors = planned.doors.len(),
        journal = %planned.journal.display(),
        epochs = %planned.epochs.display(),
        "[authority]: this node's evaluator, execution authority, provider enforcement and evidence journal, from its declaration"
    );
    Ok(())
}

/// The doctor's view of a planned authority: the evaluator, the execution authority and provider enforcement set on
/// a node that never starts, in memory only — no journal or epochs opened, no sweep spawned, no reader started
/// (`with_execution_authority` would spawn and `mark_started`). The journal and the epochs read as `start()` would
/// leave them (`guarantee.rs`), the journal's path checked by [`plan`].
pub(crate) fn attach_for_report(agent: &super::GossipAgent, planned: &DeclaredAuthority) {
    let _ = agent.task_ctx.action_evaluator.set(Arc::clone(&planned.evaluator));
    let _ = agent.task_ctx.execution_authority.set(Arc::clone(&planned.authority));
    agent.task_ctx.provider_enforcement.store(true, std::sync::atomic::Ordering::Release);
    agent.task_ctx.declared_attached.store(true, std::sync::atomic::Ordering::Release);
}
