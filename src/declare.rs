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
    /// The lease the declaration lives under, when watching: `heartbeat()` must be called within every window, and
    /// a watch that dies is retracted by the node when the lease lapses. `None` — the once mode — binds the
    /// declaration to the node's lifetime, as the SDKs' `declare_units` does without a lease (#667's review, D3).
    pub lease_secs: Option<u64>,
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tick {
    /// Nothing under `units/` or `skills/` changed since the last tick.
    Unchanged,
    /// `wire-check` found errors; the previous declaration stands. `report` is its text.
    Refused { errors: usize, report: String },
    /// The gateway refused a unit the check passed (a reserved namespace, a stem-only section, a governed group);
    /// that unit's previous declaration was put back, the others stand, and the tick is not retried until a file
    /// changes (#667's review, D6).
    Rejected { unit: String, error: String },
    /// Declared: how many units and skills, and what the node said it does not enforce.
    Declared { units: usize, skills: usize, not_enforced: Vec<String>, warnings: Vec<String> },
}

/// A gateway's refusal: `status` is `(code, insufficient_scope)` when the gateway answered at all.
struct GatewayError {
    status: Option<(u16, bool)>,
    text: String,
}

impl GatewayError {
    /// The watch itself is refused — nothing it sends would be accepted — as opposed to one unit's content.
    fn refuses_the_watch(&self) -> bool {
        matches!(self.status, None | Some((401, _)) | Some((403, true)))
    }
}

/// A watch over one project against one gateway. Holds the handles of its declarations.
pub struct Watch {
    cfg: WatchConfig,
    client: reqwest::Client,
    /// Per unit name: the handle the gateway gave, and the text it was given — kept so a refused re-declaration
    /// can put the previous one back.
    handles: BTreeMap<String, (String, String)>,
    /// The skills declared, `(ns, name)`, so a removed file's template is deleted.
    skills: Vec<(String, String)>,
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
        Self { cfg, client: reqwest::Client::new(), handles: BTreeMap::new(), skills: Vec::new(), seen: BTreeMap::new(), ticked: false }
    }

    /// The handles this watch currently holds, by unit name.
    pub fn handles(&self) -> BTreeMap<String, String> {
        self.handles.iter().map(|(k, (h, _))| (k.clone(), h.clone())).collect()
    }

    async fn declare_unit(&self, toml: &str) -> Result<(String, Vec<String>), GatewayError> {
        let mut body = serde_json::json!({ "toml": toml, "interval_secs": self.cfg.interval_secs });
        if let Some(lease) = self.cfg.lease_secs {
            body["lease_secs"] = serde_json::json!(lease);
        }
        let reply = self.send_typed("/gateway/units/declare", self.request(reqwest::Method::POST, "/gateway/units/declare").json(&body)).await?;
        let handle = reply["handle_id"].as_str().ok_or_else(|| GatewayError { status: None, text: "units/declare answered without a handle_id".to_string() })?.to_string();
        let not_enforced = reply["not_enforced"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        Ok((handle, not_enforced))
    }

    async fn drop_handle(&self, handle: &str) {
        let route = format!("/gateway/capability/{handle}");
        let _ = self.send(&route, self.request(reqwest::Method::DELETE, &route)).await;
    }

    fn request(&self, method: reqwest::Method, route: &str) -> reqwest::RequestBuilder {
        let r = self.client.request(method, format!("{}{}", self.cfg.gateway.trim_end_matches('/'), route));
        match &self.cfg.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    async fn send(&self, route: &str, req: reqwest::RequestBuilder) -> Result<serde_json::Value, String> {
        self.send_typed(route, req).await.map_err(|e| e.text)
    }

    /// A refusal with its status kept, so a caller can tell the watch being refused (no credential, no scope)
    /// from one unit's content being refused (a reserved namespace, a stem-only section).
    async fn send_typed(&self, route: &str, req: reqwest::RequestBuilder) -> Result<serde_json::Value, GatewayError> {
        let resp = req.send().await.map_err(|e| GatewayError { status: None, text: format!("{route}: {e}") })?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(GatewayError { status: Some((status.as_u16(), body.contains("insufficient scope"))), text: format!("{route}: {status} {body}") });
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
        // Declare. A unit's previous handle is retracted first, so a renamed capability does not linger: one KV key
        // per `ns/name` means the other order would tombstone the new advert. Between the two a reader sees no
        // provider for a moment; that window is inherent and the page says so (#667's review, Q9). If the gateway
        // refuses the new text, the previous text is declared again, so a refused edit still changes nothing.
        let mut not_enforced: Vec<String> = Vec::new();
        for (name, path, _) in &parsed {
            let toml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let previous = self.handles.remove(name);
            if let Some((old, _)) = &previous {
                self.drop_handle(old).await;
            }
            match self.declare_unit(&toml).await {
                Ok((handle, ne)) => {
                    self.handles.insert(name.clone(), (handle, toml));
                    for s in ne {
                        if !not_enforced.contains(&s) {
                            not_enforced.push(s);
                        }
                    }
                }
                Err(error) => {
                    // Put the previous declaration back either way. A watch the gateway refuses outright (no
                    // credential, no scope) is an error; one unit's content refused is a rejection, remembered
                    // with this tree so it is not retried every poll.
                    if let Some((_, old_toml)) = previous
                        && let Ok((handle, _)) = self.declare_unit(&old_toml).await
                    {
                        self.handles.insert(name.clone(), (handle, old_toml));
                    }
                    if error.refuses_the_watch() {
                        return Err(error.text);
                    }
                    self.seen = now;
                    self.ticked = true;
                    return Ok(Tick::Rejected { unit: name.clone(), error: error.text });
                }
            }
        }
        // Retract units that are gone from the directory.
        let present: Vec<&String> = parsed.iter().map(|(n, _, _)| n).collect();
        let gone: Vec<String> = self.handles.keys().filter(|k| !present.contains(k)).cloned().collect();
        for name in gone {
            if let Some((old, _)) = self.handles.remove(&name) {
                self.drop_handle(&old).await;
            }
        }
        // Skills: write each file's template; delete the template of a file that is gone.
        let mut warnings = Vec::new();
        let mut declared_skills = Vec::with_capacity(skills.len());
        for (ns, name, path) in &skills {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !text.contains("{{input}}") {
                warnings.push(format!("{}: no `{{{{input}}}}` placeholder — the skill will answer this text verbatim", path.display()));
            }
            let template = serde_json::json!({ "system": "", "user_template": text, "max_tokens": 256, "temperature": 0.0, "metadata": {} });
            let route = format!("/gateway/prompts/{ns}/{name}");
            self.send(&route, self.request(reqwest::Method::PUT, &route).json(&template)).await?;
            declared_skills.push((ns.clone(), name.clone()));
        }
        for (ns, name) in std::mem::replace(&mut self.skills, declared_skills) {
            if !skills.iter().any(|(n, m, _)| *n == ns && *m == name) {
                let route = format!("/gateway/prompts/{ns}/{name}");
                let _ = self.send(&route, self.request(reqwest::Method::DELETE, &route)).await;
            }
        }
        self.seen = now;
        self.ticked = true;
        Ok(Tick::Declared { units: parsed.len(), skills: skills.len(), not_enforced, warnings })
    }

    /// Keep every declaration alive: one heartbeat per handle. Call within every `lease_secs` window. A handle
    /// the node no longer knows — it restarted, or the lease lapsed — is forgotten here and the next `tick`
    /// declares again whether or not a file changed (#667's review, D2): the route's own contract says a 404 is
    /// *re-advertise*.
    pub async fn heartbeat(&mut self) -> Result<(), String> {
        let mut lost = Vec::new();
        for (name, (h, _)) in &self.handles {
            let route = format!("/gateway/capability/{h}/heartbeat");
            if let Err(e) = self.send(&route, self.request(reqwest::Method::POST, &route)).await {
                lost.push((name.clone(), e));
            }
        }
        if lost.is_empty() {
            return Ok(());
        }
        for (name, _) in &lost {
            self.handles.remove(name);
        }
        self.ticked = false;
        Err(lost.into_iter().map(|(n, e)| format!("{n}: {e}")).collect::<Vec<_>>().join("; "))
    }

    /// Retract every declaration this watch holds.
    pub async fn retract(&mut self) {
        for (_, (h, _)) in std::mem::take(&mut self.handles) {
            self.drop_handle(&h).await;
        }
    }
}

/// The command: declare once, or watch until Ctrl-C, printing what each tick did. Returns the exit code: 0; 1 when
/// the first declaration was refused by `wire-check` or by the gateway.
pub async fn run(cfg: WatchConfig, watch: bool, poll: Duration) -> i32 {
    let mut w = Watch::new(cfg.clone());
    // Beat within every lease window whatever the poll (#667's review, D4): the sleep is the shorter of the poll
    // and the time to the next beat. One `ctrl_c` future for the whole run, so a signal during a tick is kept.
    let beat_every = cfg.lease_secs.map(|l| Duration::from_secs((l / 3).max(1)));
    let mut last_beat = crate::sim_seam::mono_instant();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    let mut first = true;
    loop {
        let code = match w.tick().await {
            Ok(Tick::Unchanged) => 0,
            Ok(Tick::Refused { errors, report }) => {
                println!("{report}");
                println!("refused: wire-check found {errors} error(s); {}", if w.handles().is_empty() { "this run declared nothing" } else { "the previous declaration stands" });
                1
            }
            Ok(Tick::Rejected { unit, error }) => {
                println!("rejected: the gateway refused unit `{unit}` — {error}; its previous declaration was put back, the others stand");
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
        let until_beat = beat_every.map(|every| every.saturating_sub(crate::sim_seam::mono_elapsed(&last_beat)));
        let wait = until_beat.map_or(poll, |u| u.min(poll));
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = &mut ctrl_c => {
                println!("retracting {} declaration(s)", w.handles().len());
                w.retract().await;
                return 0;
            }
        }
        if let Some(every) = beat_every
            && crate::sim_seam::mono_elapsed(&last_beat) >= every
        {
            if let Err(e) = w.heartbeat().await {
                println!("heartbeat lost a declaration, declaring again at the next tick: {e}");
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
        WatchConfig { gateway: base.to_string(), token: Some("s3cret".into()), project: dir.to_path_buf(), interval_secs: 30, lease_secs: Some(90) }
    }

    async fn providers(base: &str, name: &str) -> Vec<serde_json::Value> {
        let r = reqwest::Client::new().get(format!("{base}/gateway/capability/resolve?ns=demo&name={name}")).bearer_auth("s3cret").send().await.unwrap();
        r.json::<serde_json::Value>().await.unwrap()["providers"].as_array().cloned().unwrap_or_default()
    }

    /// The gateway answers a declare or a retract before the advert's write or the tombstone lands (the persist
    /// task's first tick), so a reader polls to a deadline, as the HTTP tests do (#667's review, D1).
    async fn wait_for(base: &str, name: &str, want: impl Fn(&[serde_json::Value]) -> bool) -> Vec<serde_json::Value> {
        let deadline = crate::sim_seam::mono_instant() + Duration::from_secs(5);
        loop {
            let p = providers(base, name).await;
            if want(&p) || crate::sim_seam::mono_instant() >= deadline {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
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
        let p = wait_for(&base, "thing", |p| !p.is_empty()).await;
        assert_eq!(p[0]["attributes"]["v"], "1", "{p:?}");
        assert_eq!(w.tick().await.unwrap(), Tick::Unchanged, "nothing changed");
        unit(&dir, "other", "2");
        let t = w.tick().await.unwrap();
        assert!(matches!(t, Tick::Declared { units: 1, .. }), "{t:?}");
        let p = wait_for(&base, "other", |p| !p.is_empty()).await;
        assert_eq!(p[0]["attributes"]["v"], "2", "{p:?}");
        assert!(wait_for(&base, "thing", |p| p.is_empty()).await.is_empty(), "the renamed capability's old advert is retracted");
        w.retract().await;
        assert!(wait_for(&base, "other", |p| p.is_empty()).await.is_empty(), "retract removes the declaration");
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
        assert_eq!(wait_for(&base, "thing", |p| !p.is_empty()).await.len(), 1, "the previous declaration stands");
        assert_eq!(w.handles().len(), 1);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unit the check passes but the gateway refuses (a reserved namespace) is rejected once — not retried every
    /// poll — and its previous declaration is put back, so a refused edit still changes nothing (#667's review, D6).
    #[tokio::test]
    async fn a_unit_the_gateway_refuses_is_rejected_once_and_its_previous_declaration_put_back() {
        let (agent, base) = gateway_node().await;
        let dir = project("rejected");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { .. }));
        wait_for(&base, "thing", |p| !p.is_empty()).await;
        // The same unit, now also offering a capability in a namespace the gateway reserves.
        std::fs::write(
            dir.join("units/a.toml"),
            "principal = \"a\"\n\n[[capability]]\nns = \"demo\"\nname = \"thing\"\nttl_secs = 30\n  [capability.attrs]\n  v = \"1\"\n\n[[capability]]\nns = \"llm-meta\"\nname = \"x\"\nttl_secs = 30\n",
        )
        .unwrap();
        let t = w.tick().await.unwrap();
        assert!(matches!(&t, Tick::Rejected { unit, .. } if unit == "a"), "{t:?}");
        assert_eq!(wait_for(&base, "thing", |p| !p.is_empty()).await.len(), 1, "the previous declaration was put back");
        assert_eq!(w.handles().len(), 1, "and its handle is held");
        assert_eq!(w.tick().await.unwrap(), Tick::Unchanged, "the refusal is not retried while nothing changed");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A handle the node no longer knows — here retracted behind the watch's back, as a restart or a lapsed lease
    /// would — fails the heartbeat, and the next tick declares again although no file changed (#667's review, D2).
    #[tokio::test]
    async fn a_lost_handle_is_declared_again_at_the_next_tick() {
        let (agent, base) = gateway_node().await;
        let dir = project("lost");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { .. }));
        wait_for(&base, "thing", |p| !p.is_empty()).await;
        assert!(w.heartbeat().await.is_ok(), "a live handle beats");
        let handle = w.handles().values().next().unwrap().clone();
        reqwest::Client::new().delete(format!("{base}/gateway/capability/{handle}")).bearer_auth("s3cret").send().await.unwrap();
        wait_for(&base, "thing", |p| p.is_empty()).await;
        let err = w.heartbeat().await.expect_err("a lost handle fails the heartbeat");
        assert!(err.contains("404"), "{err}");
        assert!(w.handles().is_empty(), "the lost handle is forgotten");
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { .. }), "declared again with no file changed");
        assert_eq!(wait_for(&base, "thing", |p| !p.is_empty()).await.len(), 1);
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A unit removed from the directory is retracted at the next tick.
    #[tokio::test]
    async fn a_unit_removed_from_the_directory_is_retracted() {
        let (agent, base) = gateway_node().await;
        let dir = project("removed");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { units: 1, .. }));
        wait_for(&base, "thing", |p| !p.is_empty()).await;
        std::fs::remove_file(dir.join("units/a.toml")).unwrap();
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { units: 0, .. }));
        assert!(wait_for(&base, "thing", |p| p.is_empty()).await.is_empty(), "retracted");
        assert!(w.handles().is_empty());
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
        // A removed skill file deletes its template (#667's review, Q7).
        std::fs::remove_file(dir.join("skills/demo/greeting.md")).unwrap();
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { skills: 0, .. }));
        assert!(agent.llm().get_prompt("demo", "greeting").is_none(), "the template is deleted with the file");
        agent.shutdown().await;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
