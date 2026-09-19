//! **The control envelope** — v3 item 4's decisive demonstration
//! (`docs/plans/v3-contracts-axis.md` §12.1), as a browser dashboard.
//!
//! ```text
//! cargo run --example control_envelope_viz --features metrics
//! # then open http://127.0.0.1:8096/
//! ```
//!
//! # The claim this exists to make watchable
//!
//! *A fleet's governors act inside one declared envelope: a class of action that should not be
//! taken on a guess is held when the view is uncertain — and whether "held" means **advice** or
//! **refusal** is an operator's choice, made in the open, one rung at a time.*
//!
//! The dashboard runs the same proposal stream past `control::decide` under each profile, live,
//! side by side. Nothing is simulated: `decide`, `spacing_allows`, `may_propose` and `RightsLedger`
//! are the shipped functions the governors call, driven here by a load generator instead of by a
//! real cluster.
//!
//! | Profile | What the same uncertain view produces |
//! |---|---|
//! | `legacy` | `Proceed` — the predicate is never consulted. Today's behaviour, unchanged |
//! | `observe` | `WouldHold` — counted, and the action still proceeds. **The number an operator watches before stepping up** |
//! | `enforce-local` | `Held` — the action does not happen, and the refusal names what was missing |
//! | `enforce-allocated` | as above, **and** an action over its allocated budget is refused by the rights ledger, with the rejection recorded |
//!
//! # Why a dashboard and not a test
//!
//! The rungs are each tested. What a test cannot show is the *shape over time*: the would-hold
//! count climbing while nothing is refused, then the same load under enforcement producing holds
//! instead — which is exactly the judgement an operator has to make before turning it on. The
//! shadow-mode runbook (`docs/operations/control-profiles.md`) describes that judgement; this makes
//! it visible.
//!
//! # What it does not demonstrate
//!
//! **The combined-feedback scenario** — three real governors interacting on one node — is replay
//! **scenario C** (`src/control/scenario_c.rs`), a schedule sweep over their shipped decisions, and
//! it is a test rather than something to watch: its claim is about *every* interleaving in the
//! sweep, which a live dashboard samples one of. The panel says so on screen.
//!
//! Nor does this drive the real membership / tuning / opacity governors: they are crate-private
//! loops fed by a live cluster, and a demo that pretended otherwise would be showing its own
//! scaffolding. What it drives is the contract they all call.

use mycelium::control::{
    assess, decide, holds_on_uncertainty, may_propose, spacing_allows, ActionClass, ActionId,
    ConfidenceBound, ControlSpec, Decision, Profile, SettleState,
};
use mycelium::control::ledger::{Right, RightState, RightsLedger};
use mycelium::mandate::{PrincipalId, TermId};
use mycelium::ViewConfidence;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const HTTP_PORT: u16 = 8096;

/// Rule 4 of the UI-example contract: the concepts this demo exercises, as data.
const CONCEPTS: &str = r#"[
  {"tag":"IV","name":"control contract","gloss":"one admission contract every governor calls — decide · spacing_allows · may_propose"},
  {"tag":"IV","name":"profile ladder","gloss":"legacy → observe → enforce-local → enforce-allocated; shadow first, always"},
  {"tag":"IV","name":"allocated rights","gloss":"a budget held as rights; over it, admission is refused AND the rejection recorded"},
  {"tag":"I","name":"gossip-KV","gloss":"the node backing this dashboard is a real agent with a gateway"}
]"#;

/// One depot proposing scale-ups: the load the envelope is applied to.
struct Depot {
    name: &'static str,
    /// Peers this depot has heard from inside the window — the input the confidence predicate reads.
    peers_heard: usize,
    /// Its own sequence per actuator, so an action has a stable id.
    seq: u64,
    /// When it last acted, for the spacing rule.
    last_action_ms: Option<u64>,
    /// Whether an action it took is still unobserved.
    settle: SettleState,
}

#[derive(Default, Clone)]
struct Counters {
    proceeded: u64,
    would_hold: u64,
    held: u64,
    held_by_spacing: u64,
    held_by_settling: u64,
    rights_admitted: u64,
    rights_refused: u64,
}

struct VizState {
    tick: u64,
    profile: Profile,
    /// Per-profile counters, so the ladder is visible as four columns rather than one moving number.
    per_profile: [(String, Counters); 4],
    /// The live profile's most recent decisions, newest first.
    recent: Vec<String>,
    depots: Vec<(String, usize, bool)>,
    budget_units: u64,
    budget_held: u64,
    ledger_rejections: u64,
    gw_port: u16,
}

impl VizState {
    fn to_json(&self) -> String {
        let cols: Vec<String> = self
            .per_profile
            .iter()
            .map(|(name, c)| {
                format!(
                    r#"{{"profile":"{name}","proceeded":{},"would_hold":{},"held":{},"spacing":{},"settling":{}}}"#,
                    c.proceeded, c.would_hold, c.held, c.held_by_spacing, c.held_by_settling
                )
            })
            .collect();
        let depots: Vec<String> = self
            .depots
            .iter()
            .map(|(n, heard, uncertain)| {
                format!(r#"{{"name":"{n}","peers_heard":{heard},"uncertain":{uncertain}}}"#)
            })
            .collect();
        let recent: Vec<String> = self.recent.iter().map(|r| format!("\"{r}\"")).collect();
        let live = &self.per_profile[self.profile.as_u8() as usize].1;
        format!(
            r#"{{"tick":{},"profile":"{}","columns":[{}],"depots":[{}],"recent":[{}],
"rights":{{"granted":{},"held":{},"admitted":{},"refused":{},"recorded_rejections":{}}},"gw_port":{}}}"#,
            self.tick,
            self.profile.name(),
            cols.join(","),
            depots.join(","),
            recent.join(","),
            self.budget_units,
            self.budget_held,
            live.rights_admitted,
            live.rights_refused,
            self.ledger_rejections,
            self.gw_port,
        )
    }
}

/// The envelope, applied once to one proposal. **Every branch here is a shipped function** — this
/// is the contract the governors call, not a local re-implementation of it.
fn apply_envelope(
    depot: &mut Depot,
    spec: &ControlSpec,
    bound: &ConfidenceBound,
    profile: Profile,
    now_ms: u64,
    counters: &mut Counters,
) -> String {
    let id = ActionId {
        governor: "provisioner".into(),
        actuator: format!("capacity/{}", depot.name),
        seq: depot.seq,
    };

    // 1. The loop-breakers, which apply whatever the view says.
    if !spacing_allows(spec, depot.last_action_ms, now_ms) {
        counters.held_by_spacing += 1;
        return format!("{} · spacing — {}ms since last action", depot.name, now_ms.saturating_sub(depot.last_action_ms.unwrap_or(0)));
    }
    if let Err(unsettled) = may_propose(&depot.settle, spec, now_ms) {
        counters.held_by_settling += 1;
        return format!("{} · settling — action {} unobserved for {}ms", depot.name, unsettled.id.seq, unsettled.pending_for_ms);
    }

    // 2. The confidence predicate, under the profile in force.
    let view = ViewConfidence {
        observer: depot.name.to_string(),
        peers_known: 5,
        peers_heard: depot.peers_heard,
        max_staleness_ms: 400,
        self_degraded: false,
    };
    let class = ActionClass::SpeculativeScaleUp;
    let verdict = decide(class, &view, bound, profile);

    match verdict {
        Decision::Proceed => {
            counters.proceeded += 1;
            depot.seq += 1;
            depot.last_action_ms = Some(now_ms);
            depot.settle = SettleState::Pending { id, since_ms: now_ms };
            format!("{} · proceed — scale up", depot.name)
        }
        Decision::WouldHold(why) => {
            // Advice: counted, and the action still happens. This is the number an operator reads.
            counters.would_hold += 1;
            counters.proceeded += 1;
            depot.seq += 1;
            depot.last_action_ms = Some(now_ms);
            depot.settle = SettleState::Pending { id, since_ms: now_ms };
            format!("{} · WOULD hold ({why:?}) — and proceeded anyway", depot.name)
        }
        Decision::Held(why) => {
            counters.held += 1;
            format!("{} · HELD ({why:?}) — spending rights on a guess", depot.name)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A real node, so the Ops Console has something to target (UI contract rule 1 + 2).
    let gw_port = free_port();
    let gossip_port = free_port();
    let mut cfg = mycelium::GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(gw_port);
    let agent = Arc::new(mycelium::GossipAgent::new(
        mycelium::NodeId::new("127.0.0.1", gossip_port)?,
        cfg,
    ));
    agent.start().await?;
    let _ = agent.kv().set("ui/viz", bytes::Bytes::from(format!("http://127.0.0.1:{HTTP_PORT}/")));
    let _ = agent.kv().set("ui/label", bytes::Bytes::from_static(b"control envelope"));

    // The allocated budget: eight units of provisioning capacity, held under one term.
    let dir = std::env::temp_dir().join(format!("control-envelope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut ledger = RightsLedger::open(dir.join("rights.journal"))?;
    let holder = PrincipalId::new("provisioner").expect("principal");
    let budget_units = 8;
    ledger
        .allocate(Right {
            holder: holder.clone(),
            resource: "capacity".into(),
            units: budget_units,
            allocated_by: PrincipalId::new("coop-board").expect("principal"),
            term: TermId::new("term-1").expect("term"),
            state: RightState::Serving,
            valid_until_ms: u64::MAX,
        })
        .await
        .expect("the board allocates the budget");

    // Deliberately strict, as the type's own docs say: an operator loosens it on evidence, and
    // this dashboard is where that evidence comes from.
    let bound = ConfidenceBound { max_staleness_ms: 2_000, min_peers_heard: 3 };
    let spec = ControlSpec {
        governor: "provisioner".into(),
        actuator: "capacity".into(),
        spacing_ms: 1_500,
        settle_timeout_ms: 4_000,
        bound: bound.clone(),
        profile: Profile::Observe,
    };

    let state = Arc::new(Mutex::new(VizState {
        tick: 0,
        profile: Profile::Observe,
        per_profile: [
            ("legacy".into(), Counters::default()),
            ("observe".into(), Counters::default()),
            ("enforce-local".into(), Counters::default()),
            ("enforce-allocated".into(), Counters::default()),
        ],
        recent: Vec::new(),
        depots: Vec::new(),
        budget_units,
        budget_held: budget_units,
        ledger_rejections: 0,
        gw_port,
    }));

    tokio::spawn(serve_http(Arc::clone(&state), gw_port));

    let mut depots = vec![
        Depot { name: "north", peers_heard: 4, seq: 0, last_action_ms: None, settle: SettleState::Idle },
        Depot { name: "harbour", peers_heard: 2, seq: 0, last_action_ms: None, settle: SettleState::Idle },
        Depot { name: "hill", peers_heard: 1, seq: 0, last_action_ms: None, settle: SettleState::Idle },
    ];

    println!("╔════════════════════════════════════════════════════════╗");
    println!("║  Control envelope → http://127.0.0.1:{HTTP_PORT}              ║");
    println!("║  the same load, one rung at a time: advice → refusal   ║");
    println!("╚════════════════════════════════════════════════════════╝");

    let started = std::time::Instant::now();
    let mut admitted_units: u64 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(700)).await;
        let now_ms = started.elapsed().as_millis() as u64;
        let profile = state.lock().unwrap().profile;

        // The load: each depot's view drifts, so the uncertain set changes over time and the
        // dashboard is never a still picture.
        for (i, d) in depots.iter_mut().enumerate() {
            let phase = ((now_ms / 3_000) as usize + i) % 4;
            d.peers_heard = match phase {
                0 => 4,
                1 => 3,
                2 => 1,
                _ => 2,
            };
            // An action settles once observed — here, after one tick.
            if let SettleState::Pending { since_ms, .. } = &d.settle {
                if now_ms.saturating_sub(*since_ms) > 1_200 {
                    d.settle = SettleState::Idle;
                }
            }
        }

        let mut lines = Vec::new();
        for d in depots.iter_mut() {
            // Apply the envelope under *every* profile, so the columns are comparable: the live
            // profile's column is the one whose actions actually happen.
            for (idx, (_, counters)) in state.lock().unwrap().per_profile.iter_mut().enumerate() {
                if Profile::from_u8(idx as u8) != profile {
                    // Shadow columns: count what this rung would have done, without moving the
                    // depot's own state (which belongs to the live rung).
                    let mut shadow = Depot {
                        name: d.name,
                        peers_heard: d.peers_heard,
                        seq: d.seq,
                        last_action_ms: d.last_action_ms,
                        settle: d.settle.clone(),
                    };
                    let _ = apply_envelope(&mut shadow, &spec, &bound, Profile::from_u8(idx as u8), now_ms, counters);
                }
            }
            let mut live_counters = state.lock().unwrap().per_profile[profile.as_u8() as usize].1.clone();
            let line = apply_envelope(d, &spec, &bound, profile, now_ms, &mut live_counters);

            // Under `enforce-allocated`, an action that got past the predicate still has to be
            // inside its budget — and a refusal is recorded, never silent.
            let mut line = line;
            if profile == Profile::EnforceAllocated && line.contains("proceed") {
                match ledger.admit(&holder, "capacity", admitted_units + 1).await {
                    Ok(_) => {
                        admitted_units += 1;
                        live_counters.rights_admitted += 1;
                    }
                    Err(_) => {
                        live_counters.rights_refused += 1;
                        line = format!("{} · REFUSED — over the allocated budget ({admitted_units}/{budget_units})", d.name);
                    }
                }
            } else if line.contains("proceed") {
                live_counters.rights_admitted += 1;
            }
            {
                let mut s = state.lock().unwrap();
                s.per_profile[profile.as_u8() as usize].1 = live_counters;
            }
            lines.push(line);
        }

        let uncertain: Vec<(String, usize, bool)> = depots
            .iter()
            .map(|d| {
                let view = ViewConfidence {
                    observer: d.name.to_string(),
                    peers_known: 5,
                    peers_heard: d.peers_heard,
                    max_staleness_ms: 400,
                    self_degraded: false,
                };
                (d.name.to_string(), d.peers_heard, assess(&view, &bound).is_err())
            })
            .collect();

        let rejections = ledger.view().rejections();
        let held = ledger.view().held_units(&holder, "capacity");
        let mut s = state.lock().unwrap();
        s.tick += 1;
        s.depots = uncertain;
        s.ledger_rejections = rejections;
        s.budget_held = held;
        for line in lines.into_iter().rev() {
            s.recent.insert(0, line);
        }
        s.recent.truncate(12);
    }
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let p = l.local_addr().expect("bound").port();
    drop(l);
    p
}

/// Minimal HTTP server — the `/state`-JSON + polling-canvas pattern `conway` established.
async fn serve_http(state: Arc<Mutex<VizState>>, gw_port: u16) {
    let listener = match TcpListener::bind(format!("127.0.0.1:{HTTP_PORT}")).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("HTTP server failed to bind :{HTTP_PORT} — {e}");
            return;
        }
    };
    loop {
        let Ok((mut stream, _)) = listener.accept().await else { continue };
        let st = Arc::clone(&state);
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap_or(0);
            let req = std::str::from_utf8(&buf[..n]).unwrap_or("");

            if req.starts_with("OPTIONS") {
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
                    .await;
                return;
            }

            // Stepping the ladder is the operator's action, so the dashboard asks for it by name.
            if let Some(rest) = req.strip_prefix("GET /profile/") {
                let name: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
                if let Some(p) = Profile::parse(&name) {
                    st.lock().unwrap().profile = p;
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
                    .await;
                return;
            }

            if req.contains("GET /state") {
                let json = st.lock().unwrap().to_json();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    json.len(),
                    json
                );
                let _ = stream.write_all(response.as_bytes()).await;
            } else {
                let console_link = format!(
                    "<a class=\"opsbtn\" href=\"http://127.0.0.1:8099/?target=127.0.0.1:{gw_port}\" \
                     title=\"Open this node in the Mycelium Ops Console\">⚙ Ops Console</a>"
                );
                let html = include_str!("control_envelope_viz.html")
                    .replace("__OPS_CONSOLE_LINK__", &console_link)
                    .replace("__CONCEPTS__", CONCEPTS);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    html.len(),
                    html
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
    }
}

// Silence the unused warning when `holds_on_uncertainty` is only read by the doc table above.
#[allow(dead_code)]
fn _class_holds() -> bool {
    holds_on_uncertainty(ActionClass::SpeculativeScaleUp)
}
