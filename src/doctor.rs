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
    pub effective: Vec<Field>,
    pub refusal: Option<Refusal>,
    pub report: Option<GuaranteeReport>,
    pub confinement: Option<ConfinementReport>,
    pub persistence: Option<Persistence>,
}

/// Run the doctor over `config_path` (or, with none, the defaults and the environment, as
/// `mycelium` with no `-c` does). Call it inside a tokio runtime context: the node's constructor
/// registers with the runtime as the binary's does, though nothing is started.
pub fn run(config_path: Option<&Path>) -> Doctor {
    let mut d = Doctor {
        schema: SCHEMA,
        config_path: config_path.map(Path::to_path_buf),
        effective: Vec::new(),
        refusal: None,
        report: None,
        confinement: None,
        persistence: None,
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
    // Built, never started: no port, no directory, no attachment. The same `preflight()` and
    // `profile_check()` that `start()` runs, in its order; the report taken between them as `start()`
    // takes it — except that `started` reads false here, which the report records.
    let agent = GossipAgent::new(node, cfg);
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
            for e in &r.entries {
                let (state, detail) = state_of(&e.resolution);
                if state != "enforced" {
                    let _ = writeln!(s, "  {}: {state} — {detail} → {}", e.id, e.docs);
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
