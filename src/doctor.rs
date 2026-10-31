//! `mycelium doctor` — the readiness report without a node (docs/plans/adopter-surface.md, X1).
//!
//! What `start()` would print and whether it would refuse, from a configuration file and the
//! environment alone. It calls the node's own functions — `preflight()` and `profile_check()` in
//! `agent/lifecycle.rs`, the guarantee report, the confinement report — over a node that is built and
//! never started, so it binds no port, creates no persistence directory and attaches nothing. The
//! words are `start()`'s because the code is.
//!
//! **What it cannot see.** A guarantee that depends on an attachment made in code — an action
//! evaluator, an evidence journal, an audit sink — reads here as it reads for the `mycelium` binary,
//! which attaches none before `start()`. An embedder that attaches them reads its own
//! `guarantee_report()` after attaching. Unreadable persisted state is found only by opening the
//! directory, which `doctor` does not do; that refusal stays with `start()` (R7, 2.20.0).
//!
//! **The effective configuration.** Every field after defaults, the file and the environment, with
//! its source named — `default`, `file` or `env` — in the configuration's own key order (the JSON
//! form's, alphabetical), nested tables flattened to dotted keys. It is the configuration as loaded:
//! a tuning field left at its auto value prints that value, and the node derives the running one at
//! construction. A secret — `gateway_auth_token`, a `token` in either table, and a credential binding's
//! `value_env`, `value_file` and `username`, which post-360 row L keeps out of every `Debug` — is shown as
//! `set` or `unset`, never as its value, in the text and the JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::agent::confinement::ConfinementReport;
use crate::agent::guarantee::{GuaranteeReport, Resolution};
use crate::{GossipAgent, GossipConfig, GossipError, NodeId};

/// Where an effective field's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// `GossipConfig::default()`.
    Default,
    /// The configuration file.
    File,
    /// A `GOSSIP_*` environment override.
    Env,
}

/// One effective field: its dotted key, its value as the JSON form carries it (a secret reads `"set"` or
/// `"unset"`, an absent `Option` is `null`), its source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Field {
    pub key: String,
    pub value: serde_json::Value,
    pub source: Source,
}

/// Why `start()` would refuse, in its words: the stage that refused (`load`, `environment`, `node`,
/// `preflight` — which begins with `validate()`, as `start()` does — or `profile`), the field it named,
/// and the reason as `start()` would print it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub stage: &'static str,
    pub field: String,
    pub reason: String,
}

/// The persistence directory as it is, never as it would be: `doctor` creates nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Persistence {
    pub base_path: PathBuf,
    pub exists: bool,
    pub is_dir: bool,
    /// The read-only bit of the path's mode when it exists; absent when it does not. The mode, not an
    /// access check: a directory another user owns can read writable here and refuse this one.
    pub mode_readonly: Option<bool>,
}

/// The report. `report` and `confinement` are present whenever a node could be built from the
/// configuration, even when `preflight` refused — a reader sees what the node would have said.
#[derive(Debug, Serialize)]
pub struct Doctor {
    /// The JSON form's schema id; a field added later keeps the id, a field removed or renamed bumps it.
    pub schema: &'static str,
    pub config_path: Option<PathBuf>,
    /// Whether the node was built by [`run_attached`], with the embedder's code attachments, rather
    /// than from the file alone. In the JSON form (`mycelium.doctor/1`) the `attached` field; `false` from
    /// `mycelium doctor`, which only ever builds from the file.
    pub attached: bool,
    pub effective: Vec<Field>,
    pub refusal: Option<Refusal>,
    pub report: Option<GuaranteeReport>,
    pub confinement: Option<ConfinementReport>,
    pub persistence: Option<Persistence>,
    /// Each reported guarantee's row of the shared-responsibility matrix ([`matrix_row`]), by id.
    pub matrix: BTreeMap<&'static str, &'static str>,
}

/// Each core guarantee's row of the shared-responsibility matrix (`docs/operations/shared-responsibility-matrix.md`
/// § From a `doctor` line to its row): who holds the control a `doctor` line reports on, so an auditor goes from a
/// state to a responsibility without the code (adopter-surface Y10). A guarantee the matrix has no row for reads
/// `no row` with the reason. `scripts/check-pilot-kit.py` holds this table and the page's equal, cell for cell (links
/// and emphasis stripped); `every_core_guarantee_names_its_matrix_row` holds it complete over the catalogue.
pub const MATRIX_ROWS: &[(&str, &str)] = &[
    ("gw.not_open", "Gateway access control (CC6)"),
    ("gw.exposed_closed", "Gateway access control (CC6)"),
    ("gw.token_tables", "Gateway access control (CC6)"),
    ("gw.oidc", "Operator authentication (SSO) (CC6)"),
    ("gw.tls", "Encryption in transit (gateway) (CC6)"),
    ("gw.caller_profile", "Capability authorization (CC6)"),
    ("mesh.tls", "Node authentication (CC6) · Encryption in transit (gossip) (CC6)"),
    ("mesh.signed_writes", "Peer-identity integrity (CC6)"),
    ("id.proofs_required", "Peer-identity integrity (CC6)"),
    ("id.ca_key_off_node", "Key custody (CA, at-rest KMS) (CC6)"),
    ("id.revocation_authority", "Key management — compromise (CC6)"),
    ("ae.authorised_at_seam", "Capability authorization (CC6)"),
    ("ae.recorded_before_dispatch", "Capability authorization (CC6) · Audit trail (CC7)"),
    ("prov.enforcement", "Capability authorization (CC6)"),
    ("a2a.admission", "Gateway access control (CC6)"),
    ("authz.execution_authority", "Scoped authority over a resource (mandates) (CC6)"),
    ("authz.durable_epochs", "Scoped authority over a resource (mandates) (CC6)"),
    ("audit.chain", "Audit trail (CC7)"),
    ("audit.sink", "Audit export (CC7)"),
    ("egress.allow_list", "Egress control (Confidentiality & Availability)"),
    ("egress.credentials", "Egress control (Confidentiality & Availability)"),
    ("persist.configured", "Availability (Confidentiality & Availability)"),
    ("persist.sync_mode", "Availability (Confidentiality & Availability)"),
    ("persist.unreadable_refused", "Availability (Confidentiality & Availability)"),
    ("at_rest.cipher", "Encryption at rest (CC6)"),
    ("cons.safety_profile", "no row — an exclusive outcome decided once is processing integrity, a category this matrix does not cover; guide 04 is its record"),
    ("fed.domain_enforced", "Cross-domain access (federated domains) (CC6)"),
    ("fed.pinned", "Cross-domain access (federated domains) (CC6)"),
    ("fed.body_bound", "Cross-domain access (federated domains) (CC6)"),
    ("fed.partner_slots", "Cross-domain access (federated domains) (CC6)"),
    ("fed.caller_named", "Cross-domain access (federated domains) (CC6)"),
    ("fed.catalogue_signed", "Cross-domain access (federated domains) (CC6)"),
    ("fed.partner_egress", "Cross-domain access (federated domains) (CC6)"),
    ("fed.partner_posture", "Cross-domain access (federated domains) (CC6)"),
    ("fed.non_merger", "Cross-domain access (federated domains) (CC6)"),
    ("fed.clock_agreement", "Cross-domain access (federated domains) (CC6)"),
    ("trace.spans_not_evidence", "Metrics / observability (CC7)"),
    ("net.confinement", "Egress control (Confidentiality & Availability)"),
    ("clock.sync", "Metrics / observability (CC7)"),
];

/// What a guarantee this table does not name reads — a companion's, registered at run time.
pub const NO_MATRIX_ROW: &str = "no row (not a core guarantee: the matrix covers the core catalogue)";

/// The matrix row a guarantee's `doctor` line names.
pub fn matrix_row(id: &str) -> &'static str {
    MATRIX_ROWS.iter().find(|(g, _)| *g == id).map(|(_, row)| *row).unwrap_or(NO_MATRIX_ROW)
}

/// Run the doctor over `config_path` (or, with none, the defaults and the environment, as
/// `mycelium` with no `-c` does). Call it inside a tokio runtime context: the node's constructor
/// registers with the runtime as the binary's does, though nothing is started.
pub fn run(config_path: Option<&Path>) -> Doctor {
    run_inner(config_path, None)
}

/// The doctor over a node an embedder builds: the same load, `preflight()`, report and
/// `profile_check()` as [`run`], with `attach` applied to the node between its construction and
/// `preflight()` — the function the embedder's own program runs before `start()`, so what it attaches
/// in code (an action evaluator, an evidence journal, a federation edge or clients) is what the report
/// reads, and the refusal is the one that program's `start()` would give (adopter-surface Y8). The node
/// is still never started: no port is bound and no persistence directory is created by the doctor —
/// but `attach` runs as written, so an evidence journal it opens is opened. [`Doctor::attached`] is
/// `true`, and the text no longer says the doctor cannot see code attachments.
pub fn run_attached(config_path: Option<&Path>, attach: impl FnOnce(GossipAgent) -> GossipAgent) -> Doctor {
    run_inner(config_path, Some(Box::new(attach)))
}

type Attach<'a> = Box<dyn FnOnce(GossipAgent) -> GossipAgent + 'a>;

fn run_inner(config_path: Option<&Path>, attach: Option<Attach<'_>>) -> Doctor {
    let mut d = Doctor {
        schema: SCHEMA,
        config_path: config_path.map(Path::to_path_buf),
        attached: attach.is_some(),
        effective: Vec::new(),
        refusal: None,
        report: None,
        confinement: None,
        persistence: None,
        matrix: BTreeMap::new(),
    };
    let defaults = GossipConfig::default();
    // The file layer, parsed without the environment, so the two can be told apart below. The node
    // binary does the same in `load_from_file` and then applies the environment.
    let file_layer: GossipConfig = match config_path {
        Some(path) => {
            let parsed = std::fs::read_to_string(path)
                .map_err(GossipError::Io)
                .and_then(|text| toml::from_str::<GossipConfig>(&text).map_err(GossipError::Toml));
            match parsed {
                Ok(c) => c,
                Err(e) => {
                    d.refusal = Some(Refusal { stage: "load", field: path.display().to_string(), reason: e.to_string() });
                    return d;
                }
            }
        }
        None => defaults.clone(),
    };
    let mut cfg = file_layer.clone();
    if let Err(e) = cfg.apply_env_overrides() {
        d.refusal = Some(refusal("environment", &e));
        return d;
    }
    d.effective = effective_fields(&defaults, &file_layer, &cfg);
    // `validate()` is not called here: `preflight()` begins with it, as `start()` does, so calling it
    // twice printed its warnings twice (#665's review, D3). A configuration that fails it is refused
    // at the `preflight` stage, naming its field.
    d.persistence = cfg.persistence.as_ref().map(|p| inspect_persistence(&p.base_path));
    let node = match NodeId::new(&cfg.bind_address, cfg.bind_port) {
        Ok(n) => n,
        Err(e) => {
            d.refusal = Some(refusal("node", &e));
            return d;
        }
    };
    // Built, never started: no port and no directory — and no attachment, unless `run_attached` was given one, which
    // then runs as the embedder wrote it (a journal it opens is opened). The same `preflight()` and `profile_check()`
    // that `start()` runs, in its order; the report taken between them as `start()` takes it — except that `started`
    // reads false here, which the report records.
    let agent = GossipAgent::new(node, cfg);
    let agent = match attach {
        Some(attach) => attach(agent),
        None => agent,
    };
    if let Err(e) = agent.preflight() {
        d.refusal = Some(refusal("preflight", &e));
    }
    let report = agent.guarantee_report();
    d.confinement = Some(agent.confinement_report());
    if d.refusal.is_none()
        && let Err(e) = agent.profile_check(&report)
    {
        d.refusal = Some(refusal("profile", &e));
    }
    d.matrix = report.entries.iter().map(|e| (e.id, matrix_row(e.id))).collect();
    d.report = Some(report);
    d
}

/// The effective fields of `env` with each one's source, by comparing the three layers: a key whose
/// value differs between `env` and `file` came from the environment; between `file` and `defaults`,
/// from the file; otherwise it is the default. Flattened to dotted keys; a secret's value redacted.
pub fn effective_fields(defaults: &GossipConfig, file: &GossipConfig, env: &GossipConfig) -> Vec<Field> {
    let layer = |c: &GossipConfig| -> BTreeMap<String, serde_json::Value> {
        let mut out = BTreeMap::new();
        if let Ok(v) = serde_json::to_value(c) {
            flatten("", &v, &mut out);
        }
        out
    };
    let (d, f, e) = (layer(defaults), layer(file), layer(env));
    let null = serde_json::Value::Null;
    e.iter()
        .map(|(key, value)| {
            let in_file = f.get(key).unwrap_or(&null);
            let in_default = d.get(key).unwrap_or(&null);
            let source = if value != in_file {
                Source::Env
            } else if in_file != in_default {
                Source::File
            } else {
                Source::Default
            };
            Field { key: key.clone(), value: redacted(key, value), source }
        })
        .collect()
}

/// The JSON form's schema id.
pub const SCHEMA: &str = "mycelium.doctor/1";

/// A key whose value is never shown: the gateway token, a `token` in either table, and a credential
/// binding's `value_env`, `value_file` and `username` — the names row L's `Debug` keeps out of every
/// print, kept out of this one too (#665's review, D5) — plus the generic spellings of a secret.
fn is_secret(key: &str) -> bool {
    if key == "gateway_auth_token" {
        return true;
    }
    matches!(
        key.rsplit('.').next().map(|s| s.trim_end_matches(|c: char| c == ']' || c.is_ascii_digit() || c == '[')),
        Some("token" | "value_env" | "value_file" | "username" | "password" | "secret" | "api_key" | "api_key_env")
    )
}

/// The value the JSON form carries: a secret collapses to `"set"`/`"unset"`; everything else is itself.
fn redacted(key: &str, v: &serde_json::Value) -> serde_json::Value {
    if is_secret(key) {
        return match v {
            serde_json::Value::Null => "unset".into(),
            serde_json::Value::String(s) if s.is_empty() => "unset".into(),
            _ => "set".into(),
        };
    }
    v.clone()
}

/// The text form of a value: a string quoted, an absent `Option` as `unset`, a secret as its marker bare.
fn render_value(key: &str, v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "unset".into(),
        serde_json::Value::String(s) if is_secret(key) => s.clone(),
        serde_json::Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

fn flatten(prefix: &str, v: &serde_json::Value, out: &mut BTreeMap<String, serde_json::Value>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, v) in m {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten(&key, v, out);
            }
        }
        serde_json::Value::Array(a) if a.iter().any(serde_json::Value::is_object) => {
            for (i, x) in a.iter().enumerate() {
                flatten(&format!("{prefix}[{i}]"), x, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

fn refusal(stage: &'static str, e: &GossipError) -> Refusal {
    let field = match e {
        GossipError::InvalidField { field, .. } => (*field).to_string(),
        _ => "configuration".to_string(),
    };
    Refusal { stage, field, reason: e.to_string() }
}

fn inspect_persistence(base: &Path) -> Persistence {
    let meta = std::fs::metadata(base).ok();
    Persistence {
        base_path: base.to_path_buf(),
        exists: meta.is_some(),
        is_dir: meta.as_ref().is_some_and(|m| m.is_dir()),
        mode_readonly: meta.map(|m| m.permissions().readonly()),
    }
}

fn state_of(r: &Resolution) -> (&'static str, &'static str) {
    match r {
        Resolution::Enforced => ("enforced", ""),
        Resolution::NotConfigured { missing } => ("not_configured", missing),
        Resolution::NotInBuild { feature } => ("not_in_build", feature),
        Resolution::NotApplicable { because } => ("not_applicable", because),
        Resolution::NotVerifiableHere { evidence } => ("not_verifiable_here", evidence),
    }
}

impl Doctor {
    /// The text form: the verdict first, then the guarantees that are not enforced, the confinement
    /// report's unmet settings, the persistence directory as found, and the effective configuration.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut s = String::new();
        let _ = writeln!(s, "mycelium doctor — {}", env!("CARGO_PKG_VERSION"));
        let _ = writeln!(
            s,
            "configuration: {}",
            self.config_path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "defaults + environment (no file)".into())
        );
        if let Some(r) = &self.report {
            let _ = writeln!(s, "build features: {}", r.features.join(", "));
            if let Some(p) = &r.profile {
                let _ = writeln!(s, "profile: {} rev {}{}", p.name, p.revision, if p.selected { "" } else { " (none set: dev requires nothing)" });
            }
        }
        match &self.refusal {
            Some(r) => {
                let _ = writeln!(s, "start(): WOULD REFUSE at {} on `{}`:", r.stage, r.field);
                let _ = writeln!(s, "  {}", r.reason);
            }
            None => {
                let _ = writeln!(s, "start(): would start");
            }
        }
        if let Some(r) = &self.report {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for e in &r.entries {
                *counts.entry(state_of(&e.resolution).0).or_default() += 1;
            }
            let summary: Vec<String> = counts.iter().map(|(k, v)| format!("{k} {v}")).collect();
            let _ = writeln!(s, "guarantees: {}", summary.join(" · "));
            // What is met is named too, not only counted: a count says how many, not which.
            let enforced: Vec<&str> = r.entries.iter().filter(|e| state_of(&e.resolution).0 == "enforced").map(|e| e.id).collect();
            let _ = writeln!(s, "enforced: {}", if enforced.is_empty() { "none".to_string() } else { enforced.join(", ") });
            // Who holds each enforced one, grouped by its matrix row in the order the guarantees were named.
            let mut by_row: Vec<(&str, Vec<&str>)> = Vec::new();
            for id in &enforced {
                let row = matrix_row(id);
                match by_row.iter_mut().find(|(r, _)| *r == row) {
                    Some((_, ids)) => ids.push(id),
                    None => by_row.push((row, vec![id])),
                }
            }
            for (row, ids) in by_row {
                let _ = writeln!(s, "  enforced → matrix: {row}: {}", ids.join(", "));
            }
            if r.profile.as_ref().is_some_and(|p| p.name == "secure-multi-domain") && r.faces_no_partner() {
                // `doctor` builds a node from a file, which never has an edge or a client attached: say what
                // that means here rather than leave six `not_applicable` lines to be read as a pass. Over an
                // embedder's attachments (`run_attached`) the line is a fact about the node, not the doctor.
                let why = if self.attached { "" } else { " (doctor cannot see an edge or clients attached in code)" };
                let _ = writeln!(s, "federation: {}{why}", crate::agent::guarantee::FACES_NO_PARTNER);
            }
            for e in &r.entries {
                let (state, detail) = state_of(&e.resolution);
                if state != "enforced" {
                    let _ = writeln!(s, "  {}: {state} — {detail} → {} → matrix: {}", e.id, e.docs, matrix_row(e.id));
                }
            }
        }
        if let Some(c) = &self.confinement {
            let unmet = c.unmet();
            let _ = writeln!(s, "confinement: {}", if unmet.is_empty() { "every node-level setting met".to_string() } else { format!("unmet: {}", unmet.join(", ")) });
        }
        if let Some(p) = &self.persistence {
            let _ = writeln!(
                s,
                "persistence: {} — {}{}",
                p.base_path.display(),
                if !p.exists { "absent (doctor creates nothing; start() would)" } else if !p.is_dir { "exists and is not a directory (start() would refuse)" } else { "exists" },
                match p.mode_readonly { Some(true) => ", read-only by its mode bits", _ => "" }
            );
        }
        let _ = writeln!(s, "effective configuration ({} fields):", self.effective.len());
        for f in &self.effective {
            let src = match f.source { Source::Default => "default", Source::File => "file", Source::Env => "env" };
            let _ = writeln!(s, "  {} = {}   # {src}", f.key, render_value(&f.key, &f.value));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_cfg(name: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mycelium-doctor-{name}-{}.toml", std::process::id()));
        std::fs::write(&p, body).expect("write");
        p
    }

    /// X1's first exit: for a configuration `start()` refuses under `secure-single-domain`, `doctor`
    /// names the same unmet guarantees in the same words — shown by running both in one test.
    #[tokio::test]
    async fn doctor_names_the_refusal_start_would_give() {
        let port = crate::test_util::alloc_port();
        let path = write_cfg("profile", &format!("bind_port = {port}\nprofile = \"secure-single-domain\"\n"));
        let d = run(Some(&path));
        let refusal = d.refusal.clone().expect("secure-single-domain over a bare configuration refuses");
        assert_eq!(refusal.stage, "profile", "{refusal:?}");
        assert!(refusal.reason.contains("requirement(s) unmet"), "{}", refusal.reason);
        let cfg = GossipConfig::load_from_file(&path).expect("loads");
        let node = NodeId::new(&cfg.bind_address, cfg.bind_port).expect("node");
        let agent = GossipAgent::new(node, cfg);
        let err = agent.start().await.expect_err("start refuses under the profile");
        assert_eq!(refusal.reason, err.to_string(), "doctor and start() say the same thing");
        assert!(d.report.is_some() && d.confinement.is_some(), "the report is still taken");
    }

    /// Y7's exit: `doctor` prints the federation family. Under `secure-multi-domain` over a bare
    /// configuration it names the profile and its revision, lists every `fed.*` id, and gives the same
    /// refusal `start()` would — including `fed.domain_enforced`, the one federation fact a
    /// configuration alone decides. The edge's and the clients' facts read `not_applicable`: they are
    /// code attachments, which a node `doctor` builds from a file never has. Seen failing first: the
    /// profile name was refused by `validate()` (stage `preflight`).
    #[tokio::test]
    async fn doctor_prints_the_federation_family_under_the_multi_domain_profile() {
        let port = crate::test_util::alloc_port();
        let path = write_cfg("multi-domain", &format!("bind_port = {port}\nprofile = \"secure-multi-domain\"\n"));
        let d = run(Some(&path));
        let refusal = d.refusal.clone().expect("secure-multi-domain over a bare configuration refuses");
        assert_eq!(refusal.stage, "profile", "{refusal:?}");
        assert!(refusal.reason.contains("profile `secure-multi-domain` rev 1"), "{}", refusal.reason);
        assert!(refusal.reason.contains("[fed.domain_enforced: not_configured"), "{}", refusal.reason);
        let text = d.render();
        assert!(text.contains("profile: secure-multi-domain rev 1"), "{text}");
        for id in ["fed.domain_enforced", "fed.pinned", "fed.body_bound", "fed.partner_slots", "fed.caller_named",
                   "fed.catalogue_signed", "fed.partner_egress", "fed.partner_posture", "fed.non_merger", "fed.clock_agreement"] {
            assert!(text.contains(&format!("  {id}: ")), "{id} is printed: {text}");
        }
        assert!(text.contains("fed.pinned: not_applicable"), "{text}");
        assert!(text.contains("federation: this node faces no federation partner"), "said in one line: {text}");
        let cfg = GossipConfig::load_from_file(&path).expect("loads");
        let agent = GossipAgent::new(NodeId::new(&cfg.bind_address, cfg.bind_port).expect("node"), cfg);
        let err = agent.start().await.expect_err("start refuses under the profile");
        assert_eq!(refusal.reason, err.to_string(), "doctor and start() say the same thing");
    }

    /// Adopter-surface Y8: the doctor over a node an embedder builds sees what it attaches in code. Over the same
    /// file, `run` reads the client-side `fed.*` guarantees `not_applicable` — a node from a file has no client —
    /// and `run_attached`, given the attachment the program makes, reads them as that client makes them: here
    /// unpinned, so `fed.pinned` is `not_configured` and the profile refuses naming it, as that program's `start()`
    /// would. Seen failing first with `run_inner` ignoring `attach` (`fed.pinned` read `not_applicable`).
    #[cfg(all(feature = "gateway", feature = "tls"))]
    #[tokio::test]
    async fn the_doctor_over_an_attached_node_reads_its_attachments_and_starts_refusal() {
        use crate::federation::{client::{FederationClient, GatewayEndpoint}, DomainId};
        let port = crate::test_util::alloc_port();
        let path = write_cfg("attached", &format!("bind_port = {port}\nprofile = \"secure-multi-domain\"\n"));
        let client = || std::sync::Arc::new(FederationClient::new(
            DomainId::new("a.example").unwrap(), "node:x", ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]),
            DomainId::new("b.example").unwrap(), vec![GatewayEndpoint { id: "b#0".into(), base_url: "https://127.0.0.1:1".into() }],
            1, std::time::Duration::from_secs(60),
        ));
        let state = |d: &Doctor, id: &str| d.report.as_ref().and_then(|r| r.entry(id)).map(|e| state_of(&e.resolution).0);
        let plain = run(Some(&path));
        assert!(!plain.attached);
        assert_eq!(state(&plain, "fed.pinned"), Some("not_applicable"));
        assert!(plain.render().contains("doctor cannot see an edge or clients attached in code"));
        let attached = run_attached(Some(&path), |agent| agent.with_federation_clients([client()]));
        assert!(attached.attached);
        assert_eq!(state(&attached, "fed.pinned"), Some("not_configured"), "the doctor reads the attached client");
        let refusal = attached.refusal.clone().expect("an unpinned client under the profile refuses");
        assert!(refusal.reason.contains("fed.pinned: not_configured"), "{}", refusal.reason);
        assert!(!attached.render().contains("doctor cannot see"), "the doctor no longer says it cannot see attachments");
        // The same node, started: the same refusal in the same words.
        let cfg = GossipConfig::load_from_file(&path).expect("loads");
        let agent = GossipAgent::new(NodeId::new(&cfg.bind_address, cfg.bind_port).expect("node"), cfg).with_federation_clients([client()]);
        let err = agent.start().await.expect_err("start refuses under the profile");
        assert_eq!(refusal.reason, err.to_string(), "doctor and the embedder's start() say the same thing");
    }

    /// X1's second exit: a table a build cannot enforce is reported as `start()` reports it — the
    /// v2.18.1 refusal naming the feature — without starting. Runs in the default build, which has
    /// `gateway` and not `compliance`.
    #[cfg(all(feature = "gateway", not(feature = "compliance")))]
    #[tokio::test]
    async fn a_table_this_build_cannot_enforce_is_a_refusal_naming_the_feature() {
        let port = crate::test_util::alloc_port();
        let path = write_cfg("tokens", &format!("bind_port = {port}\n[[gateway_scoped_tokens]]\ntoken = \"t\"\nscopes = [\"kv:read\"]\n"));
        let d = run(Some(&path));
        let r = d.refusal.clone().expect("a token table without compliance refuses");
        assert_eq!(r.stage, "preflight", "{r:?}");
        assert_eq!(r.field, "gateway_scoped_tokens");
        assert!(r.reason.contains("`compliance` feature"), "{}", r.reason);
        assert!(d.report.is_some(), "the report is still taken after a preflight refusal");
    }

    /// X1's third exit: a persistence directory that does not exist is reported, not created.
    #[tokio::test]
    async fn a_missing_persistence_directory_is_reported_and_not_created() {
        let port = crate::test_util::alloc_port();
        let dir = std::env::temp_dir().join(format!("mycelium-doctor-absent-{port}"));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write_cfg("persist", &format!("bind_port = {port}\n[persistence]\nbase_path = \"{}\"\n", dir.display()));
        let d = run(Some(&path));
        let p = d.persistence.clone().expect("persistence configured");
        assert!(!p.exists && !p.is_dir && p.mode_readonly.is_none(), "{p:?}");
        assert!(!dir.exists(), "doctor creates nothing");
        if std::env::var_os("GOSSIP_PROFILE").is_none() {
            assert!(d.refusal.is_none(), "{:?}", d.refusal);
        }
        assert!(d.render().contains("absent (doctor creates nothing"));
    }

    /// The effective configuration: one field three ways — untouched, set by the file, set by the
    /// environment layer — each with its source named. The layers are built directly, since the
    /// crate's tests hold no lock over the process environment.
    #[test]
    fn the_effective_configuration_names_each_fields_source() {
        let defaults = GossipConfig::default();
        let mut file = defaults.clone();
        file.bind_port = 7001;
        let mut env = file.clone();
        env.http_port = Some(7002);
        let fields = effective_fields(&defaults, &file, &env);
        let src = |k: &str| fields.iter().find(|f| f.key == k).unwrap_or_else(|| panic!("no field {k}")).source;
        assert_eq!(src("bind_port"), Source::File);
        assert_eq!(src("http_port"), Source::Env);
        assert_eq!(src("bind_address"), Source::Default);
        assert_eq!(fields.iter().find(|f| f.key == "bind_port").unwrap().value, serde_json::json!(7001));
        assert_eq!(fields.iter().find(|f| f.key == "bind_address").unwrap().value, serde_json::json!("127.0.0.1"), "a string is a JSON string, not a quoted string");
    }

    /// Row L's names stay out of the print: a credential binding's `value_env`, `value_file` and
    /// `username` read `set`, in both forms (#665's review, D5).
    #[test]
    fn a_credential_bindings_source_names_are_redacted() {
        let defaults = GossipConfig::default();
        let with: GossipConfig = toml::from_str(
            "[[egress_credentials]]\nname = \"openai\"\nhost = \"api.openai.com\"\nform = \"bearer\"\nvalue_env = \"MY_SECRET_VAR\"\n",
        )
        .expect("parses");
        let fields = effective_fields(&defaults, &with, &with);
        let f = fields.iter().find(|f| f.key == "egress_credentials[0].value_env").expect("the field");
        assert_eq!(f.value, serde_json::json!("set"));
        assert!(!serde_json::to_string(&fields).unwrap().contains("MY_SECRET_VAR"));
        assert!(!fields.iter().map(|f| render_value(&f.key, &f.value)).collect::<String>().contains("MY_SECRET_VAR"));
    }

    /// The guarantees a configuration enforces are named, not only counted (a walk-through of tutorial 7: the reader
    /// could not see which were met; the summary prints only the states it holds, so `enforced 0` never appears).
    #[tokio::test]
    async fn doctor_names_what_it_enforces() {
        let port = crate::test_util::alloc_port();
        let http = crate::test_util::alloc_port();
        let path = write_cfg("named-enforced", &format!(
            "bind_port = {port}\nhttp_port = {http}\n[[gateway_named_tokens]]\nname = \"a\"\ntoken = \"a-token\"\nscopes = [\"kv:read\"]\n"));
        let d = run(Some(&path));
        let text = d.render();
        let line = text.lines().find(|l| l.starts_with("enforced: ")).unwrap_or_else(|| panic!("an `enforced:` line: {text}"));
        let named: Vec<&str> = line["enforced: ".len()..].split(", ").filter(|id| *id != "none").collect();
        let counted = d.report.as_ref().expect("a report").entries.iter().filter(|e| state_of(&e.resolution).0 == "enforced").count();
        assert_eq!(named.len(), counted, "every enforced guarantee is named, and only those: {line}");
        // A named-token table is enforced only where `compliance` is built (v2.18.1).
        #[cfg(feature = "compliance")]
        assert!(named.contains(&"gw.token_tables"), "{line}");
    }

    /// Every core guarantee names its row of the shared-responsibility matrix, once, and the table names nothing the
    /// catalogue lacks (adopter-surface Y10: `doctor` names the row; the page's copy is held equal by
    /// `scripts/check-pilot-kit.py`).
    #[test]
    fn every_core_guarantee_names_its_matrix_row() {
        let core: Vec<&str> = crate::agent::guarantee::core_guarantees().iter().map(|g| g.id).collect();
        let named: Vec<&str> = MATRIX_ROWS.iter().map(|(id, _)| *id).collect();
        for id in &core {
            assert_eq!(named.iter().filter(|n| *n == id).count(), 1, "`{id}` must have exactly one matrix row");
        }
        for id in &named {
            assert!(core.contains(id), "the matrix table names `{id}`, which is not a core guarantee");
        }
        assert!(MATRIX_ROWS.iter().all(|(_, row)| !row.is_empty()));
        assert_eq!(matrix_row("companion.something"), NO_MATRIX_ROW);
    }

    /// Each guarantee line `doctor` prints names its matrix row: a line for a guarantee not enforced ends with it, and
    /// every enforced one is listed under exactly one `enforced → matrix:` line naming its row. The JSON form carries
    /// the same row for every reported guarantee.
    #[tokio::test]
    async fn doctor_names_each_guarantees_matrix_row() {
        let port = crate::test_util::alloc_port();
        let http = crate::test_util::alloc_port();
        let path = write_cfg("matrix-rows", &format!(
            "bind_port = {port}\nhttp_port = {http}\n[[gateway_named_tokens]]\nname = \"a\"\ntoken = \"a-token\"\nscopes = [\"kv:read\"]\n"));
        let d = run(Some(&path));
        let text = d.render();
        let report = d.report.as_ref().expect("a report");
        for e in &report.entries {
            let row = matrix_row(e.id);
            if state_of(&e.resolution).0 == "enforced" {
                let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("  enforced → matrix: ")
                    && l.rsplit_once(": ").is_some_and(|(_, ids)| ids.split(", ").any(|i| i == e.id))).collect();
                assert_eq!(lines.len(), 1, "`{}` is enforced and listed under one matrix line: {text}", e.id);
                assert!(lines[0].starts_with(&format!("  enforced → matrix: {row}: ")), "{}", lines[0]);
            } else {
                let line = text.lines().find(|l| l.starts_with(&format!("  {}: ", e.id)))
                    .unwrap_or_else(|| panic!("a line for `{}`: {text}", e.id));
                assert!(line.ends_with(&format!(" → matrix: {row}")), "{line}");
            }
            assert_eq!(d.matrix.get(e.id), Some(&row), "the JSON form's row for `{}`", e.id);
        }
    }

    /// A secret is shown as `set`, never its value — in the text and in the JSON form.
    #[tokio::test]
    async fn a_secret_is_shown_as_set_and_never_its_value() {
        let port = crate::test_util::alloc_port();
        let path = write_cfg("secret", &format!("bind_port = {port}\ngateway_auth_token = \"hunter2-very-secret\"\n"));
        let d = run(Some(&path));
        let text = d.render();
        assert!(text.contains("gateway_auth_token = set"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        let json = serde_json::to_string(&d).expect("json");
        assert!(!json.contains("hunter2"), "the JSON form redacts too");
        // `run(None)` reads the live environment (the node binary's own path); a shell that exports a
        // token is not a failure of this test.
        if std::env::var_os("GOSSIP_GATEWAY_AUTH_TOKEN").is_none() {
            let bare = run(None);
            assert!(bare.render().contains("gateway_auth_token = unset"), "{}", bare.render());
        }
    }

    /// Every top-level field the configuration serialises has a row in `configuration.md` § The
    /// fields, so the effective configuration and the reference cannot drift apart unnoticed.
    #[test]
    fn every_top_level_field_is_in_the_configuration_reference_table() {
        let doc = include_str!("../docs/reference/configuration.md");
        let section = doc.split("## The fields").nth(1).expect("the fields section");
        let table = section.split("\n## ").next().unwrap_or(section);
        let documented: std::collections::BTreeSet<String> = table
            .lines()
            .filter(|l| l.starts_with("| "))
            .filter_map(|l| l.split('|').nth(1))
            .map(|c| c.trim().trim_matches('`').to_string())
            .filter(|c| c != "field" && !c.is_empty())
            .collect();
        let v = serde_json::to_value(GossipConfig::default()).expect("serialises");
        let keys: Vec<String> = v.as_object().expect("a table").keys().cloned().collect();
        let missing: Vec<&String> = keys.iter().filter(|k| !documented.contains(*k)).collect();
        assert!(missing.is_empty(), "fields with no row in configuration.md § The fields: {missing:?}");
    }
}
