//! **The other door** — the same remit enforced where a *foreign* agent knocks.
//!
//! ```text
//! cargo run --example a2a_skill_authority --features tls,a2a
//! ```
//!
//! # The claim this exists to make checkable
//!
//! [`mcp_tool_authority`](mcp_tool_authority.rs) shows a policy deciding per call at `POST /mcp`.
//! That is the door *our own* agent code comes through, and it is gated: `/mcp` requires a bearer
//! carrying `mcp:invoke` before the evaluator is ever consulted.
//!
//! `POST /a2a` is the door a **foreign** agent comes through, and it is **public by design** — an
//! A2A client discovers `/.well-known/agent.json` and calls, with no prior credential. The gateway
//! resolves a bearer if one is offered and calls the caller `anonymous` if not, but it does **not
//! refuse for want of one**. So at this door there is no scope check standing in front of the
//! evaluator: the per-call decision is the *only* thing between a stranger and a skill.
//!
//! Which makes the failure mode obvious once stated: **an enforcement point that can be walked
//! around by choosing a different door is not an enforcement point.** A remit enforced on
//! `tools/call` and not on `skill.invoke` is a remit an agent escapes by speaking A2A instead of
//! MCP — and the evidence journal would have gone on recording true sentences about the door that
//! *was* guarded while the remit went unenforced at the one that was not.
//!
//! # What this demonstrates, and what it does not
//!
//! **Demonstrates:** real `tasks/send` and `tasks/sendSubscribe` requests over the wire against a
//! real gateway with an evaluator attached — permitted, denied, undecided; that `/a2a` answers a
//! caller `/mcp` would have rejected at the front step; and that a refusal on the **streaming**
//! path arrives as a `failed` task-status event rather than a silent drop.
//!
//! **Does not demonstrate:** authentication. Every act below is `anonymous`, because that is what
//! this door hands the evaluator by default. `anonymous` is a *principal*, not a bypass — it can be
//! written about in policy, as act 1 does. An operator who wants a named principal here configures
//! `gateway_named_tokens` and the client presents a bearer; nothing in the seam changes.
//!
//! **And "refused" is not always an authority answer.** A skill nobody advertises is refused as
//! `-32001 skill not found` *before* the evaluator runs — act 5 shows this deliberately, because a
//! demonstration in which every refusal looks alike teaches the wrong thing.

use mycelium::{
    ActionEnvelope, ActionEvaluator, ActionMapping, Decision, GossipAgent, GossipConfig, NodeId,
    TlsConfig,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The depot's remit, as a policy — the same shape as `mcp_tool_authority`'s, over `skill.invoke`
/// rather than `tools/call`, to make the point that it is one remit and not two.
struct DepotRemit;

const REVISION: &str = "depot-remit-2026-09-25";

impl ActionEvaluator for DepotRemit {
    /// A dispatch may leave the depot only to an address in the delivery area. That is a check on a
    /// *value*, so the seam must carry it; anything not declared here the policy never sees.
    fn security_relevant_arguments(&self) -> Vec<String> {
        vec!["text".to_string()]
    }

    fn mapping(&self, operation: &str, resource: &str) -> ActionMapping {
        match (operation, resource) {
            ("skill.invoke", r) if r.starts_with("skill:depot/dispatch@") => {
                ActionMapping::mapped("depot-remit", REVISION)
            }
            ("skill.invoke", r) if r.starts_with("skill:ledger/export@") => {
                ActionMapping::mapped("depot-remit", REVISION)
            }
            _ => ActionMapping::unmapped("depot-remit", REVISION),
        }
    }

    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision {
        let skill = envelope.resource.split('@').next().unwrap_or("");
        match skill {
            "skill:ledger/export" => Decision::deny(
                "ledger/export is not in the depot remit",
                REVISION,
            )
            .checking(["remit: depot/dispatch only"]),

            "skill:depot/dispatch" => {
                // `anonymous` is a principal like any other, and the remit says so explicitly
                // rather than by omission — which is the difference between "strangers may
                // dispatch" as a decision and as an accident.
                let text = envelope
                    .selected_arguments
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if text.to_lowercase().contains("kirkintilloch") {
                    Decision::permit(
                        format!("caller {} may dispatch inside the delivery area", envelope.actor),
                        REVISION,
                    )
                    .checking([format!("principal={}", envelope.actor), "area=in".into()])
                } else {
                    Decision::deny(
                        "destination is outside the delivery area",
                        REVISION,
                    )
                    .checking(["area=out"])
                }
            }

            // A skill nobody wrote a rule about. Authority is not established, so the secure
            // profile refuses — rather than permitting because nothing said no.
            _ => Decision::indeterminate(format!("no clause covers {skill}"), REVISION),
        }
    }
}

fn step(n: u8, title: &str) { println!("\n\x1b[1m{n}. {title}\x1b[0m"); }
fn note(s: impl AsRef<str>) { println!("   {}", s.as_ref()); }

#[tokio::main]
async fn main() {
    let gossip: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57840);
    let http = gossip + 1;
    let id = NodeId::new("127.0.0.1", gossip).expect("node id");
    let cert_dir = std::env::temp_dir().join(format!("myc-a2a-authority-{gossip}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(http);
    // A real bearer, and a real gateway that requires it — so act 0's contrast is not staged.
    cfg.gateway_auth_token = Some("depot-token".to_string());
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });

    // `with_a2a()` mounts the door. Without it `/a2a` 404s and none of this runs.
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg).with_a2a());

    // Attach the policy. Without this the seam is INERT: `/a2a` behaves exactly as it always did.
    agent.with_action_evaluator(Arc::new(DepotRemit));
    agent.start().await.expect("start");

    // A real provider behind the door, counting every call that reaches it. This is what makes a
    // refusal checkable: `reached` not moving is the tool not running, not the demo asserting so.
    let reached = Arc::new(AtomicUsize::new(0));
    {
        let (agent, reached) = (Arc::clone(&agent), Arc::clone(&reached));
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                reached.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, b"dispatched".to_vec());
            }
        });
    }

    // Two skills advertised, so both resolve to a provider and every refusal below is the policy
    // rather than a missing address. The third act's skill is advertised too — being *unmapped* is
    // a statement about the policy, not about discovery.
    let _advertised: Vec<_> = [("depot", "dispatch"), ("ledger", "export"), ("weather", "lookup")]
        .into_iter()
        .map(|(ns, name)| {
            agent.capabilities().advertise_capability(
                mycelium::capability::Capability::new(ns, name),
                Duration::from_secs(120),
            )
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let client = reqwest::Client::new();
    let a2a_url = format!("http://127.0.0.1:{http}/a2a");
    let mcp_url = format!("http://127.0.0.1:{http}/mcp");
    let health  = format!("http://127.0.0.1:{http}/health");
    for _ in 0..60 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Every call below presents NO bearer. That is the point.
    let send = |skill: &str, text: &str| {
        let (c, u) = (client.clone(), a2a_url.clone());
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
            "params": {
                "id": format!("task-{skill}"),
                "skillId": skill,
                "message": {"role": "user", "parts": [{"type": "text", "text": text}]},
            },
        });
        async move {
            c.post(&u).json(&body).send().await.expect("a2a request")
                .json::<serde_json::Value>().await.expect("json reply")
        }
    };

    println!("\x1b[1mOne remit, two doors — and this is the unguarded one\x1b[0m");

    // ── 0. the doors are not equally guarded ────────────────────────────────────────────────
    step(0, "the same unauthenticated caller, at both doors");
    let mcp = client
        .post(&mcp_url)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .send().await.expect("mcp request");
    note(format!("POST /mcp  with no bearer → HTTP {}", mcp.status()));
    assert_eq!(mcp.status(), 401, "/mcp must require a bearer, or act 0 proves nothing");
    let card = client
        .get(format!("http://127.0.0.1:{http}/.well-known/agent.json"))
        .send().await.expect("agent card");
    note(format!("GET  /.well-known/agent.json → HTTP {} — discovery is open", card.status()));
    note("POST /a2a  with no bearer → answered (every act below). The caller is `anonymous`.");
    note("✓ so at THIS door no scope check runs first. The evaluator is the whole of the gate,");
    note("  which is why leaving it off `/a2a` would leave the remit enforced at one door only.");

    // ── 1. within the remit ─────────────────────────────────────────────────────────────────
    step(1, "depot/dispatch to Kirkintilloch — inside the delivery area");
    let r = send("depot/dispatch", "deliver to Kirkintilloch").await;
    assert!(r.get("error").is_none(), "a call within the remit must not be refused: {r}");
    assert_eq!(reached.load(Ordering::SeqCst), 1, "the permitted call must actually reach the skill");
    note("gateway: dispatched — the skill ran, and `anonymous` is who the provider was told called");
    note("✓ permitted BY NAME: the remit writes about `anonymous` rather than falling through to it");

    // ── 2. denied on an argument value ──────────────────────────────────────────────────────
    step(2, "depot/dispatch to Vladivostok — outside the delivery area");
    let r = send("depot/dispatch", "deliver to Vladivostok").await;
    let err = r.get("error").expect("an out-of-area dispatch must be refused");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note(format!("         data.reason = {}", err["data"]["reason"]));
    assert_eq!(reached.load(Ordering::SeqCst), 1, "a denied call must not reach the skill");
    note("✓ same skill, same caller, different verdict — decided on the message TEXT, which is");
    note("  carried only because the evaluator declared it security-relevant");

    // ── 3. a skill outside the remit ────────────────────────────────────────────────────────
    step(3, "ledger/export — a skill the remit excludes");
    let r = send("ledger/export", "export everything").await;
    let err = r.get("error").expect("an out-of-remit skill must be refused");
    assert_eq!(err["code"], -32030, "an explicit prohibition denies: {r}");
    note(format!("gateway: refused — {} (code {})", err["message"].as_str().unwrap_or(""), err["code"]));
    note(format!("         data.policy_revision = {}", err["data"]["policy_revision"]));
    note("✓ the refusal body is the one `/mcp` sends: a machine-readable reason and the revision of");
    note("  the artifact that decided — so a caller can tell a denial from a stale policy");

    // ── 4. the one that matters ─────────────────────────────────────────────────────────────
    step(4, "weather/lookup — advertised, and covered by no clause");
    let r = send("weather/lookup", "is it dreich").await;
    let err = r.get("error").expect("an unmapped skill must be refused, not permitted");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note(format!("         data.reason = {}", err["data"]["reason"]));
    note("✓ INDETERMINATE, not denied. The skill resolves; the policy simply has no clause. Authority");
    note("  was not established, so nothing runs — the alternative is allowing because nothing said no");

    // ── 5. not every refusal is an authority answer ─────────────────────────────────────────
    step(5, "depot/teleport — a skill nobody advertises");
    let r = send("depot/teleport", "anywhere").await;
    let err = r.get("error").expect("an unresolvable skill is refused");
    assert_eq!(err["code"], -32001, "skill resolution precedes the evaluator: {r}");
    note(format!("gateway: refused — {} (code {})", err["message"].as_str().unwrap_or(""), err["code"]));
    note("✓ refused BEFORE the evaluator ran, and the code says so. No evidence record claims a");
    note("  policy decided this — 'not found' is a fact about discovery, not about authority");

    // ── 6. the streaming door ───────────────────────────────────────────────────────────────
    step(6, "tasks/sendSubscribe on ledger/export — the same remit, over SSE");
    let stream = client
        .post(&a2a_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tasks/sendSubscribe",
            "params": {
                "id": "task-stream",
                "skillId": "ledger/export",
                "message": {"role": "user", "parts": [{"type": "text", "text": "export everything"}]},
            },
        }))
        .send().await.expect("sendSubscribe");
    let body = tokio::time::timeout(Duration::from_secs(10), stream.text())
        .await
        .expect("the stream must END on a refusal, not hang")
        .expect("stream body");
    for line in body.lines().filter(|l| l.starts_with("data:")) {
        note(format!("event: {}", line.trim_start_matches("data:").trim()));
    }
    assert!(body.contains("\"failed\""), "a refused stream must report `failed`:\n{body}");
    assert_eq!(reached.load(Ordering::SeqCst), 1, "the streamed call must not reach the skill either");
    note("✓ the refusal arrives as a task-status event and the stream CLOSES. A client that opened a");
    note("  subscription learns the same thing a unary caller learns, rather than waiting on silence");

    // ── what this does not claim ────────────────────────────────────────────────────────────
    println!("\n\x1b[1mWhat this does not claim\x1b[0m");
    note("This is a route-level preflight at THIS gateway — it governs what this door dispatches.");
    note("A provider reached another way is not covered: the same limit `mcp_tool_authority` states,");
    note("and the reason `coop/src/bin/procurement_authority.rs` spends an act on a misconfigured");
    note("route where a call was denied AND HAPPENED ANYWAY.");
    note("");
    note(format!("The skill was reached {} time(s) across 7 acts — once, by the one permitted call.",
        reached.load(Ordering::SeqCst)));

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
