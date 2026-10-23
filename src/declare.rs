//! `mycelium declare` — a project's declaration, sent to a gateway once or kept current as its files change
//! (docs/plans/adopter-surface.md, X10).
//!
//! A project is a directory with `units/*.toml` and, where it has skills, `skills/<ns>/<name>.md`. One tick reads
//! both, and when anything changed since the last tick (or on the first) runs the same `wire_check::check` the
//! `wire-check` subcommand runs: with an error, the previous declaration stands and the tick says so; without one,
//! every unit is re-declared through `POST /gateway/units/declare` — the handle of the previous declaration retracted
//! through `DELETE /gateway/capability/{handle}` first, so a renamed capability does not linger — and every skill file
//! is written through `PUT /gateway/prompts/{ns}/{name}`, the route that owns `prompts/`. Nothing is restarted: the
//! loop is over data the node accepts live.
//!
//! The watch polls the files' modification times and sizes on a plain timer. It runs no node and is outside every
//! recording, like `wire-check` and `doctor`; the plan's *Input*-class seam for a filesystem event source is not
//! built — a poll needs none, and a recording of a watched session is not a thing this tool produces.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::wire_check::{check, CheckOptions, Unit};
use crate::NodeCapabilityConfig;

/// Where to declare, as what, and how often the gateway should hear from us.
#[derive(Debug, Clone)]
pub struct WatchConfig {
    /// The gateway's base URL, e.g. `http://127.0.0.1:8300`.
    pub gateway: String,
    /// The bearer presented to the gateway; `cap:write` for units, `llm:write` for skills.
    pub token: Option<String>,
    /// The project directory: `units/` and, optionally, `skills/`.
    pub project: PathBuf,
    /// The advert's refresh interval, sent as `interval_secs`.
    pub interval_secs: u64,
    /// The lease the declaration lives under; `heartbeat()` must be called within every window.
    pub lease_secs: u64,
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// Nothing under `units/` or `skills/` changed since the last tick.
    Unchanged,
    /// `wire-check` found errors; the previous declaration stands. `report` is its text.
    Refused { errors: usize, report: String },
    /// Declared: how many units and skills, and what the node said it does not enforce.
    Declared { units: usize, skills: usize, not_enforced: Vec<String>, warnings: Vec<String> },
}

/// A watch over one project against one gateway. Holds the handles of its declarations.
pub struct Watch {
    cfg: WatchConfig,
    client: reqwest::Client,
    handles: BTreeMap<String, String>,
    seen: BTreeMap<PathBuf, (u128, u64)>,
    ticked: bool,
}

/// A skill file's `(ns, name, text)` — `skills/<ns>/<name>.md`.
fn skill_files(project: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(namespaces) = std::fs::read_dir(project.join("skills")) else { return out };
    for ns in namespaces.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(ns.path()) else { continue };
        for f in files.filter_map(|e| e.ok()) {
            let p = f.path();
            if p.extension().is_some_and(|x| x == "md")
                && let Some(name) = p.file_stem().map(|s| s.to_string_lossy().into_owned())
            {
                out.push((ns.file_name().to_string_lossy().into_owned(), name, p));
            }
        }
    }
    out.sort();
    out
}

fn unit_files(project: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let dir = project.join("units");
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .filter_map(|p| p.file_stem().map(|s| (s.to_string_lossy().into_owned(), p.clone())))
        .collect();
    files.sort();
    Ok(files)
}

fn fingerprint(paths: impl Iterator<Item = PathBuf>) -> BTreeMap<PathBuf, (u128, u64)> {
    paths
        .filter_map(|p| {
            let m = std::fs::metadata(&p).ok()?;
            let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
            Some((p, (modified, m.len())))
        })
        .collect()
}

impl Watch {
    pub fn new(cfg: WatchConfig) -> Self {
        Self { cfg, client: reqwest::Client::new(), handles: BTreeMap::new(), seen: BTreeMap::new(), ticked: false }
    }

    /// The handles this watch currently holds, by unit name.
    pub fn handles(&self) -> &BTreeMap<String, String> {
        &self.handles
    }

    fn request(&self, method: reqwest::Method, route: &str) -> reqwest::RequestBuilder {
        let r = self.client.request(method, format!("{}{}", self.cfg.gateway.trim_end_matches('/'), route));
        match &self.cfg.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    async fn send(&self, route: &str, req: reqwest::RequestBuilder) -> Result<serde_json::Value, String> {
        let resp = req.send().await.map_err(|e| format!("{route}: {e}"))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("{route}: {status} {body}"));
        }
        Ok(serde_json::from_str(&body).unwrap_or(serde_json::Value::Null))
    }

    /// One pass: read, compare, and declare if anything changed.
    pub async fn tick(&mut self) -> Result<Tick, String> {
        let units = unit_files(&self.cfg.project)?;
        let skills = skill_files(&self.cfg.project);
        let now = fingerprint(units.iter().map(|(_, p)| p.clone()).chain(skills.iter().map(|(_, _, p)| p.clone())));
        if self.ticked && now == self.seen {
            return Ok(Tick::Unchanged);
        }
        // The same check the `wire-check` subcommand runs, over the same loader.
        let mut parsed = Vec::with_capacity(units.len());
        for (name, path) in &units {
            match NodeCapabilityConfig::load_from_file(path) {
                Ok(config) => parsed.push((name.clone(), path.clone(), Unit { name: name.clone(), config })),
                Err(e) => {
                    self.seen = now;
                    self.ticked = true;
                    return Ok(Tick::Refused { errors: 1, report: format!("{}: {e}", path.display()) });
                }
            }
        }
        let report = check(&parsed.iter().map(|(_, _, u)| u.clone()).collect::<Vec<_>>(), &[], &CheckOptions { authority: true, ..CheckOptions::default() });
        if report.exit_code() != 0 {
            self.seen = now;
            self.ticked = true;
            let text = report.render_text();
            let errors = text.lines().filter(|l| l.starts_with("error")).count().max(1);
            return Ok(Tick::Refused { errors, report: text });
        }
        // Declare: retract the previous handle of each unit first, so a renamed capability does not linger.
        let mut not_enforced = Vec::new();
        for (name, path, _) in &parsed {
            if let Some(old) = self.handles.remove(name) {
                let _ = self.send(&format!("/gateway/capability/{old}"), self.request(reqwest::Method::DELETE, &format!("/gateway/capability/{old}"))).await;
            }
            let toml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let body = serde_json::json!({ "toml": toml, "interval_secs": self.cfg.interval_secs, "lease_secs": self.cfg.lease_secs });
            let reply = self.send("/gateway/units/declare", self.request(reqwest::Method::POST, "/gateway/units/declare").json(&body)).await?;
            if let Some(h) = reply["handle_id"].as_str() {
                self.handles.insert(name.clone(), h.to_string());
            }
            for s in reply["not_enforced"].as_array().into_iter().flatten().filter_map(|v| v.as_str()) {
                if !not_enforced.iter().any(|x| x == s) {
                    not_enforced.push(s.to_string());
                }
            }
        }
        // Retract units that are gone from the directory.
        let present: Vec<&String> = parsed.iter().map(|(n, _, _)| n).collect();
        let gone: Vec<String> = self.handles.keys().filter(|k| !present.contains(k)).cloned().collect();
        for name in gone {
            if let Some(old) = self.handles.remove(&name) {
                let _ = self.send(&format!("/gateway/capability/{old}"), self.request(reqwest::Method::DELETE, &format!("/gateway/capability/{old}"))).await;
            }
        }
        let mut warnings = Vec::new();
        for (ns, name, path) in &skills {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !text.contains("{{input}}") {
                warnings.push(format!("{}: no `{{{{input}}}}` placeholder — the skill will answer this text verbatim", path.display()));
            }
            let template = serde_json::json!({ "system": "", "user_template": text, "max_tokens": 256, "temperature": 0.0, "metadata": {} });
            let route = format!("/gateway/prompts/{ns}/{name}");
            self.send(&route, self.request(reqwest::Method::PUT, &route).json(&template)).await?;
        }
        self.seen = now;
        self.ticked = true;
        Ok(Tick::Declared { units: parsed.len(), skills: skills.len(), not_enforced, warnings })
    }

    /// Keep every declaration alive: one heartbeat per handle. Call within every `lease_secs` window.
    pub async fn heartbeat(&self) -> Result<(), String> {
        for h in self.handles.values() {
            let route = format!("/gateway/capability/{h}/heartbeat");
            self.send(&route, self.request(reqwest::Method::POST, &route)).await?;
        }
        Ok(())
    }

    /// Retract every declaration this watch holds.
    pub async fn retract(&mut self) {
        for (_, h) in std::mem::take(&mut self.handles) {
            let route = format!("/gateway/capability/{h}");
            let _ = self.send(&route, self.request(reqwest::Method::DELETE, &route)).await;
        }
    }
}

/// The command: declare once, or watch until Ctrl-C, printing what each tick did. Returns the exit code: 0; 1 when
/// the first declaration was refused by `wire-check` or by the gateway.
pub async fn run(cfg: WatchConfig, watch: bool, poll: Duration) -> i32 {
    let mut w = Watch::new(cfg.clone());
    let beat_every = Duration::from_secs((cfg.lease_secs / 3).max(1));
    let mut last_beat = crate::sim_seam::mono_instant();
    let mut first = true;
    loop {
        let code = match w.tick().await {
            Ok(Tick::Unchanged) => 0,
            Ok(Tick::Refused { errors, report }) => {
                println!("{report}");
                println!("refused: wire-check found {errors} error(s); {}", if w.handles().is_empty() { "this run declared nothing" } else { "the previous declaration stands" });
                1
            }
            Ok(Tick::Declared { units, skills, not_enforced, warnings }) => {
                for wn in &warnings {
                    println!("warning: {wn}");
                }
                println!(
                    "declared {units} unit(s) and {skills} skill(s) to {}{}",
                    cfg.gateway,
                    if not_enforced.is_empty() { String::new() } else { format!("; not enforced by this node: {}", not_enforced.join(", ")) }
                );
                0
            }
            Err(e) => {
                println!("declare: {e}");
                1
            }
        };
        if !watch {
            return code;
        }
        if first && code != 0 {
            return code;
        }
        first = false;
        tokio::select! {
            _ = tokio::time::sleep(poll) => {}
            _ = tokio::signal::ctrl_c() => {
                println!("retracting {} declaration(s)", w.handles().len());
                w.retract().await;
                return 0;
            }
        }
        if crate::sim_seam::mono_elapsed(&last_beat) >= beat_every {
            if let Err(e) = w.heartbeat().await {
                println!("heartbeat: {e}");
            }
            last_beat = crate::sim_seam::mono_instant();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GossipAgent, GossipConfig, NodeId};
    use std::sync::Arc;

    async fn gateway_node() -> (Arc<GossipAgent>, String) {
        let port = crate::test_util::alloc_port();
        let http = crate::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(http);
        cfg.gateway_auth_token = Some("s3cret".into());
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.expect("starts");
        let base = format!("http://127.0.0.1:{http}");
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client.get(format!("{base}/health")).send().await.is_ok_and(|r| r.status().is_success()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        (agent, base)
    }

    fn project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mycelium-declare-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("units")).unwrap();
        dir
    }

    fn unit(dir: &Path, cap: &str, attr: &str) {
        std::fs::write(
            dir.join("units/a.toml"),
            format!("principal = \"a\"\n\n[[capability]]\nns = \"demo\"\nname = \"{cap}\"\nttl_secs = 30\n  [capability.attrs]\n  v = \"{attr}\"\n"),
        )
        .unwrap();
    }

    fn cfg(base: &str, dir: &Path) -> WatchConfig {
        WatchConfig { gateway: base.to_string(), token: Some("s3cret".into()), project: dir.to_path_buf(), interval_secs: 30, lease_secs: 90 }
    }

    async fn providers(base: &str, name: &str) -> Vec<serde_json::Value> {
        let r = reqwest::Client::new().get(format!("{base}/gateway/capability/resolve?ns=demo&name={name}")).bearer_auth("s3cret").send().await.unwrap();
        r.json::<serde_json::Value>().await.unwrap()["providers"].as_array().cloned().unwrap_or_default()
    }

    /// The row's exit: a unit edit under the watch changes what the fleet reports within one declare, and a renamed
    /// capability is retracted, not left lingering beside the new one.
    #[tokio::test]
    async fn a_unit_edit_is_redeclared_and_the_old_handle_retracted() {
        let (agent, base) = gateway_node().await;
        let dir = project("edit");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        let t = w.tick().await.unwrap();
        assert!(matches!(t, Tick::Declared { units: 1, skills: 0, .. }), "{t:?}");
        assert_eq!(providers(&base, "thing").await[0]["attributes"]["v"], "1");
        assert_eq!(w.tick().await.unwrap(), Tick::Unchanged, "nothing changed");
        unit(&dir, "other", "2");
        let t = w.tick().await.unwrap();
        assert!(matches!(t, Tick::Declared { units: 1, .. }), "{t:?}");
        assert_eq!(providers(&base, "other").await[0]["attributes"]["v"], "2");
        assert!(providers(&base, "thing").await.is_empty(), "the renamed capability's old advert is retracted");
        w.retract().await;
        assert!(providers(&base, "other").await.is_empty(), "retract removes the declaration");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unit `wire-check` refuses leaves the previous declaration in place; the tick says so.
    #[tokio::test]
    async fn a_unit_wire_check_refuses_keeps_the_previous_declaration() {
        let (agent, base) = gateway_node().await;
        let dir = project("refuse");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { .. }));
        // A requirement nobody in the declaration offers: an error, not a warning.
        std::fs::write(dir.join("units/b.toml"), "principal = \"b\"\n\n[[requirement]]\nns = \"nobody\"\nname = \"offers\"\n").unwrap();
        let t = w.tick().await.unwrap();
        assert!(matches!(t, Tick::Refused { .. }), "{t:?}");
        assert_eq!(providers(&base, "thing").await.len(), 1, "the previous declaration stands");
        assert_eq!(w.handles().len(), 1);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The wrong token is an error naming the route and the status, not a silent nothing.
    #[tokio::test]
    async fn a_refused_token_is_an_error_naming_the_route() {
        let (agent, base) = gateway_node().await;
        let dir = project("token");
        unit(&dir, "thing", "1");
        let mut c = cfg(&base, &dir);
        c.token = Some("wrong".into());
        let mut w = Watch::new(c);
        let err = w.tick().await.expect_err("refused");
        assert!(err.contains("/gateway/units/declare") && err.contains("401"), "{err}");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A skill file becomes the prompt template under `prompts/{ns}/{name}`, and an edit replaces it.
    #[cfg(feature = "llm")]
    #[tokio::test]
    async fn a_skill_file_becomes_a_prompt_template() {
        let (agent, base) = gateway_node().await;
        let dir = project("skill");
        unit(&dir, "thing", "1");
        std::fs::create_dir_all(dir.join("skills/demo")).unwrap();
        std::fs::write(dir.join("skills/demo/greeting.md"), "Greet {{input}}.").unwrap();
        let mut w = Watch::new(cfg(&base, &dir));
        let t = w.tick().await.unwrap();
        assert!(matches!(t, Tick::Declared { skills: 1, .. }), "{t:?}");
        assert_eq!(agent.llm().get_prompt("demo", "greeting").unwrap().user_template, "Greet {{input}}.");
        std::fs::write(dir.join("skills/demo/greeting.md"), "Wave at {{input}}.").unwrap();
        // The same second is the same mtime on a coarse filesystem; the size differs here, which is enough.
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { .. }));
        assert_eq!(agent.llm().get_prompt("demo", "greeting").unwrap().user_template, "Wave at {{input}}.");
        std::fs::write(dir.join("skills/demo/greeting.md"), "No placeholder here at all.").unwrap();
        let t = w.tick().await.unwrap();
        assert!(matches!(&t, Tick::Declared { warnings, .. } if warnings.len() == 1), "{t:?}");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
