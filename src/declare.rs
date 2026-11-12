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

    /// Retract one handle: `Err` names the route and what the gateway said, or that it did not answer — a caller
    /// says so rather than reporting the handle retracted (#767's review, M1).
    async fn drop_handle(&self, handle: &str) -> Result<(), String> {
        let route = format!("/gateway/capability/{handle}");
        self.send(&route, self.request(reqwest::Method::DELETE, &route)).await.map(|_| ())
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
        // A retraction the gateway did not confirm, said in the tick's warnings: the old advert may linger until its
        // lease or the node's restart (#767's review, M1).
        let mut warnings = Vec::new();
        for (name, path, _) in &parsed {
            let toml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let previous = self.handles.remove(name);
            if let Some((old, _)) = &previous
                && let Err(e) = self.drop_handle(old).await
            {
                warnings.push(format!("unit `{name}`: retracting its previous handle {old} failed — {e}"));
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
            if let Some((old, _)) = self.handles.remove(&name)
                && let Err(e) = self.drop_handle(&old).await
            {
                warnings.push(format!("unit `{name}` is gone from the directory, and retracting its handle {old} failed — {e}"));
            }
        }
        // Skills: write each file's template; delete the template of a file that is gone.
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

    /// Retract every declaration this watch holds, and say what each retraction did: a handle whose DELETE the
    /// gateway did not confirm is `failed`, never counted retracted (#767's review, M1). The watch forgets every handle
    /// either way — the node retracts a failed one when its lease lapses, or at its restart for a declaration made
    /// once.
    pub async fn retract(&mut self) -> Retraction {
        let mut out = Retraction::default();
        for (unit, (h, _)) in std::mem::take(&mut self.handles) {
            match self.drop_handle(&h).await {
                Ok(()) => out.retracted.push(unit),
                Err(error) => out.failed.push(FailedRetraction { unit, handle: h, error }),
            }
        }
        out
    }
}

/// What [`Watch::retract`] did, by unit name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retraction {
    /// Units whose handle the gateway confirmed retracted.
    pub retracted: Vec<String>,
    /// Units whose retraction the gateway did not confirm.
    pub failed: Vec<FailedRetraction>,
}

/// One retraction the gateway did not confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedRetraction {
    pub unit: String,
    pub handle: String,
    /// The route and what the gateway said, or that it did not answer.
    pub error: String,
}

/// How the command prints what each tick did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    /// The lines a person reads.
    #[default]
    Text,
    /// One JSON object per line (identity-and-lifecycle N3), schema [`DECLARE_SCHEMA`]: once, a single line; under
    /// `--watch`, a line per event, so the stream reads as JSON Lines.
    Json,
}

/// The schema id of each line `mycelium declare --format json` prints.
pub const DECLARE_SCHEMA: &str = "mycelium.declare/1";

/// One event of the command, as the JSON form prints it: a tick's outcome (`declared`, `refused`, `rejected`,
/// `error`), a heartbeat that lost a declaration (`heartbeat_lost`), or the retraction at Ctrl-C — `retracting` before
/// it, `retracted` after it with each unit's outcome. An
/// unchanged tick prints nothing, as the text form prints nothing. `previous_stands` on a refusal is whether this
/// watch holds an earlier declaration — the text's *the previous declaration stands* against *this run declared
/// nothing*.
pub fn event_json(event: &Event<'_>, gateway: &str) -> Option<serde_json::Value> {
    use serde_json::json;
    let v = match event {
        Event::Tick(Ok(Tick::Unchanged), _) => return None,
        Event::Tick(Ok(Tick::Declared { units, skills, not_enforced, warnings }), _) => {
            json!({ "event": "declared", "gateway": gateway, "units": units, "skills": skills, "not_enforced": not_enforced, "warnings": warnings })
        }
        Event::Tick(Ok(Tick::Refused { errors, report }), held) => {
            json!({ "event": "refused", "errors": errors, "report": report, "previous_stands": *held > 0 })
        }
        Event::Tick(Ok(Tick::Rejected { unit, error }), _) => json!({ "event": "rejected", "unit": unit, "error": error }),
        Event::Tick(Err(e), _) => json!({ "event": "error", "error": e }),
        Event::HeartbeatLost(e) => json!({ "event": "heartbeat_lost", "error": e }),
        Event::Retracting(n) => json!({ "event": "retracting", "declarations": n }),
        Event::Retracted(r) => json!({
            "event": "retracted",
            "retracted": r.retracted,
            "failed": r.failed.iter().map(|f| json!({ "unit": f.unit, "handle": f.handle, "error": f.error })).collect::<Vec<_>>(),
        }),
    };
    let mut v = v;
    v.as_object_mut()?.insert("schema".into(), json!(DECLARE_SCHEMA));
    Some(v)
}

/// What the command prints about: a tick's result with the number of declarations the watch held after it, a lost
/// heartbeat, the retraction of `n` declarations about to start, or what that retraction did.
#[derive(Debug)]
pub enum Event<'a> {
    Tick(&'a Result<Tick, String>, usize),
    HeartbeatLost(&'a str),
    Retracting(usize),
    Retracted(&'a Retraction),
}

/// The text form of one event, as the command has always printed it.
fn event_text(event: &Event<'_>, gateway: &str) -> Option<String> {
    Some(match event {
        Event::Tick(Ok(Tick::Unchanged), _) => return None,
        Event::Tick(Ok(Tick::Refused { errors, report }), held) => format!(
            "{report}\nrefused: wire-check found {errors} error(s); {}",
            if *held == 0 { "this run declared nothing" } else { "the previous declaration stands" }
        ),
        Event::Tick(Ok(Tick::Rejected { unit, error }), _) => {
            format!("rejected: the gateway refused unit `{unit}` — {error}; its previous declaration was put back, the others stand")
        }
        Event::Tick(Ok(Tick::Declared { units, skills, not_enforced, warnings }), _) => {
            let mut out = String::new();
            for wn in warnings {
                out.push_str(&format!("warning: {wn}\n"));
            }
            out.push_str(&format!(
                "declared {units} unit(s) and {skills} skill(s) to {gateway}{}",
                if not_enforced.is_empty() { String::new() } else { format!("; not enforced by this node: {}", not_enforced.join(", ")) }
            ));
            out
        }
        Event::Tick(Err(e), _) => format!("declare: {e}"),
        Event::HeartbeatLost(e) => format!("heartbeat lost a declaration, declaring again at the next tick: {e}"),
        Event::Retracting(n) => format!("retracting {n} declaration(s)"),
        // The text form printed nothing after a retraction that worked, and says only what did not.
        Event::Retracted(r) if r.failed.is_empty() => return None,
        Event::Retracted(r) => r
            .failed
            .iter()
            .map(|f| format!("retracting unit `{}` (handle {}) failed — {}; the node retracts it when its lease lapses", f.unit, f.handle, f.error))
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

fn print_event(event: &Event<'_>, gateway: &str, format: Format) {
    let line = match format {
        Format::Text => event_text(event, gateway),
        Format::Json => event_json(event, gateway).map(|v| v.to_string()),
    };
    if let Some(line) = line {
        println!("{line}");
    }
}

/// The command: declare once, or watch until Ctrl-C, printing what each tick did. Returns the exit code: 0; 1 when
/// the first declaration was refused by `wire-check` or by the gateway, or when a retraction at Ctrl-C was not
/// confirmed.
pub async fn run(cfg: WatchConfig, watch: bool, poll: Duration) -> i32 {
    run_with(cfg, watch, poll, Format::Text).await
}

/// Poll `f` once: `true` when it resolved on that poll — the caller must then not poll it again — `false` when it is
/// pending, with whatever its first poll installs now installed.
async fn ready_now<F: std::future::Future>(mut f: std::pin::Pin<&mut F>) -> bool {
    std::future::poll_fn(|cx| std::task::Poll::Ready(f.as_mut().poll(cx).is_ready())).await
}

/// [`run`], printing each event in `format`.
pub async fn run_with(cfg: WatchConfig, watch: bool, poll: Duration, format: Format) -> i32 {
    let mut w = Watch::new(cfg.clone());
    // Beat within every lease window whatever the poll (#667's review, D4): the sleep is the shorter of the poll
    // and the time to the next beat. One `ctrl_c` future for the whole run, so a signal during a tick is kept — once
    // its handler is installed, which `ctrl_c` does on its first poll. Under `--watch` that poll is here, before the
    // first tick's first request; it used to be the first `select!`, after the first tick printed, so a SIGINT during
    // the first tick took the default action and nothing was retracted (#767's review, L5). Once, no handler is
    // installed: nothing would act on it, and Ctrl-C keeps its default.
    let beat_every = cfg.lease_secs.map(|l| Duration::from_secs((l / 3).max(1)));
    let mut last_beat = crate::sim_seam::mono_instant();
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    // The poll's answer is kept: a future that resolved on it must not be polled again (an `async fn` panics when
    // resumed after completion), so an early `Ready` is the signal itself and the `select!` below takes it without
    // touching the future (#767's confirm round, 1).
    let signalled = watch && ready_now(ctrl_c.as_mut()).await;
    let mut first = true;
    loop {
        let tick = w.tick().await;
        let code = match &tick {
            Ok(Tick::Unchanged | Tick::Declared { .. }) => 0,
            Ok(Tick::Refused { .. } | Tick::Rejected { .. }) | Err(_) => 1,
        };
        print_event(&Event::Tick(&tick, w.handles().len()), &cfg.gateway, format);
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
            _ = async { if !signalled { let _ = (&mut ctrl_c).await; } } => {
                // A second Ctrl-C while the retraction runs is not acted on: the handler is still installed. The
                // retraction is bounded by the gateway's answers; what it does not confirm lapses with the lease.
                print_event(&Event::Retracting(w.handles().len()), &cfg.gateway, format);
                let r = w.retract().await;
                print_event(&Event::Retracted(&r), &cfg.gateway, format);
                return if r.failed.is_empty() { 0 } else { 1 };
            }
        }
        if let Some(every) = beat_every
            && crate::sim_seam::mono_elapsed(&last_beat) >= every
        {
            if let Err(e) = w.heartbeat().await {
                print_event(&Event::HeartbeatLost(&e), &cfg.gateway, format);
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

    /// Every event the command prints has a JSON line (identity-and-lifecycle N3) carrying the schema and the event's
    /// facts — an unchanged tick prints nothing in either form — and the text form is the lines the command printed
    /// before the JSON form existed.
    #[test]
    fn every_event_has_a_json_line_and_the_text_is_unchanged() {
        use serde_json::json;
        let gw = "http://127.0.0.1:8300";
        let declared: Result<Tick, String> =
            Ok(Tick::Declared { units: 2, skills: 1, not_enforced: vec!["mandate".into()], warnings: vec!["w1".into()] });
        let refused: Result<Tick, String> = Ok(Tick::Refused { errors: 1, report: "error: x".into() });
        let rejected: Result<Tick, String> = Ok(Tick::Rejected { unit: "a".into(), error: "403".into() });
        let failed: Result<Tick, String> = Err("/gateway/units/declare: 401".into());
        let unchanged: Result<Tick, String> = Ok(Tick::Unchanged);
        let all_retracted = Retraction { retracted: vec!["a".into()], failed: vec![] };
        let one_failed = Retraction {
            retracted: vec![],
            failed: vec![FailedRetraction { unit: "a".into(), handle: "h1".into(), error: "/gateway/capability/h1: connection refused".into() }],
        };
        let cases: Vec<(Event<'_>, Option<serde_json::Value>, Option<&str>)> = vec![
            (Event::Tick(&declared, 2), Some(json!({ "schema": DECLARE_SCHEMA, "event": "declared", "gateway": gw, "units": 2, "skills": 1, "not_enforced": ["mandate"], "warnings": ["w1"] })),
             Some("warning: w1\ndeclared 2 unit(s) and 1 skill(s) to http://127.0.0.1:8300; not enforced by this node: mandate")),
            (Event::Tick(&refused, 0), Some(json!({ "schema": DECLARE_SCHEMA, "event": "refused", "errors": 1, "report": "error: x", "previous_stands": false })),
             Some("error: x\nrefused: wire-check found 1 error(s); this run declared nothing")),
            (Event::Tick(&refused, 1), Some(json!({ "schema": DECLARE_SCHEMA, "event": "refused", "errors": 1, "report": "error: x", "previous_stands": true })),
             Some("error: x\nrefused: wire-check found 1 error(s); the previous declaration stands")),
            (Event::Tick(&rejected, 1), Some(json!({ "schema": DECLARE_SCHEMA, "event": "rejected", "unit": "a", "error": "403" })),
             Some("rejected: the gateway refused unit `a` — 403; its previous declaration was put back, the others stand")),
            (Event::Tick(&failed, 0), Some(json!({ "schema": DECLARE_SCHEMA, "event": "error", "error": "/gateway/units/declare: 401" })),
             Some("declare: /gateway/units/declare: 401")),
            (Event::HeartbeatLost("a: 404"), Some(json!({ "schema": DECLARE_SCHEMA, "event": "heartbeat_lost", "error": "a: 404" })),
             Some("heartbeat lost a declaration, declaring again at the next tick: a: 404")),
            (Event::Retracting(3), Some(json!({ "schema": DECLARE_SCHEMA, "event": "retracting", "declarations": 3 })), Some("retracting 3 declaration(s)")),
            (Event::Retracted(&all_retracted), Some(json!({ "schema": DECLARE_SCHEMA, "event": "retracted", "retracted": ["a"], "failed": [] })), None),
            (Event::Retracted(&one_failed), Some(json!({ "schema": DECLARE_SCHEMA, "event": "retracted", "retracted": [],
                "failed": [{ "unit": "a", "handle": "h1", "error": "/gateway/capability/h1: connection refused" }] })),
             Some("retracting unit `a` (handle h1) failed — /gateway/capability/h1: connection refused; the node retracts it when its lease lapses")),
            (Event::Tick(&unchanged, 1), None, None),
        ];
        for (event, want_json, want_text) in cases {
            assert_eq!(event_json(&event, gw), want_json, "{event:?}");
            assert_eq!(event_text(&event, gw).as_deref(), want_text, "{event:?}");
        }
    }

    /// A retraction the gateway does not confirm — here the node is gone — is reported failed, unit by unit, never
    /// counted retracted (#767's review, M1). Seen failing first: `retract` discarded the DELETE's error.
    #[tokio::test]
    async fn a_retraction_with_the_gateway_down_is_reported_failed() {
        let (agent, base) = gateway_node().await;
        let dir = project("retract-down");
        unit(&dir, "thing", "1");
        let mut w = Watch::new(cfg(&base, &dir));
        assert!(matches!(w.tick().await.unwrap(), Tick::Declared { units: 1, .. }));
        agent.shutdown().await;
        let r = w.retract().await;
        assert!(r.retracted.is_empty(), "{r:?}");
        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert_eq!(r.failed[0].unit, "a");
        assert!(r.failed[0].error.contains("/gateway/capability/"), "{r:?}");
        assert!(w.handles().is_empty(), "the watch forgets the handle either way");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The eager poll keeps its answer (#767's confirm round, 1): a future that resolves on it is reported, so the
    /// caller never polls it again; a pending one has run to its first await and stays pollable.
    #[tokio::test]
    async fn the_eager_poll_reports_a_future_that_resolved_on_it() {
        let done = async {};
        tokio::pin!(done);
        assert!(ready_now(done.as_mut()).await, "resolved on the one poll");
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let started = std::sync::atomic::AtomicBool::new(false);
        let waiting = async {
            started.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = rx.await;
        };
        tokio::pin!(waiting);
        assert!(!ready_now(waiting.as_mut()).await, "pending");
        assert!(started.load(std::sync::atomic::Ordering::SeqCst), "and it ran to its first await, as `ctrl_c` installs its handler");
        tx.send(()).unwrap();
        waiting.await;
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
