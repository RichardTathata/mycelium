//! **Which tools an agent may actually call** — policy at the gateway, per call.
//!
//! ```text
//! cargo run --example mcp_tool_authority --features tls,compliance
//! ```
//!
//! # The claim this exists to make checkable
//!
//! A scope is a coarse answer. `POST /mcp` requires `mcp:invoke`, and that is binary: a caller may
//! invoke tools, or may not. It says nothing about **which** tool, or with what arguments — so an
//! agent granted "can use tools" can use *every* tool the fleet advertises.
//!
//! An [`ActionEvaluator`] attached to the node decides **per call**, between the auth layer and the
//! dispatch. Three verdicts, and the third is the one that matters:
//!
//! | Verdict | What it means | What the gateway does |
//! |---|---|---|
//! | `Permit` | the policy established authority for *this* call | dispatch |
//! | `Deny` | the policy established that this is **not** allowed | refuse, nothing runs |
//! | `Indeterminate` | the policy could **not decide** — a missing fact, an unrecognised clause, an evaluator error | **refuse** |
//!
//! *Authority not established* is a different fact from *denied*, and collapsing them is how a
//! policy engine ends up lying in both directions: reporting a gap as a prohibition, or — far worse
//! — falling through to allow because nothing said no. Here it refuses, and the evidence records
//! **why** it could not decide.
//!
//! # What this demonstrates, and what it does not
//!
//! **Demonstrates:** a real `POST /mcp` `tools/call` over the wire, against a real gateway, with a
//! real evaluator attached — permitted, denied, and undecided, plus the argument-sensitive case
//! that no digest could answer.
//!
//! **Does not demonstrate:** Cedar. The shipped adapter is a **private companion**; everything here
//! runs on the public seam, so an adopter can read it and build against it. A Cedar adapter is one
//! `impl ActionEvaluator`, and `tests/ae_external_adapter.rs` is the gate proving a *foreign* crate
//! can write one.
//!
//! **And it is a route-level preflight, not enforcement at the effect.** It governs what *this
//! gateway* will dispatch. A provider reached another way is not covered — the same limit the
//! fencing token carries in `coordination_integrity`, and the reason
//! [`procurement_authority`](coop/src/bin/procurement_authority.rs) spends an act on a misconfigured
//! route that denied a call *and it happened anyway*.

use mycelium::{
    ActionEnvelope, ActionEvaluator, ActionMapping, Decision, GossipAgent, GossipConfig, NodeId,
    TlsConfig, Verdict,
};
use std::sync::Arc;
use std::time::Duration;

/// The agent's purchasing remit, as a policy.
///
/// A real adapter parses Cedar and reports the `sha256` of its policy source as the revision. This
/// one is hand-written so the whole rule is visible on one screen — but it obeys the same contract,
/// including the part that matters most: it **never returns `Permit` for a call it did not
/// understand**.
struct PurchasingRemit;

/// The revision an evidence record carries. A real adapter derives this from the policy bytes, so
/// it cannot drift from the artifact actually loaded — it can no more be told to report a revision
/// than a file can be told its own hash.
const REVISION: &str = "remit-2026-09-25";

impl ActionEvaluator for PurchasingRemit {
    /// The argument *values* this policy needs to decide. A ceiling on `amount_pence` cannot be
    /// checked against a digest — the seam must carry the value, and this is how it knows to.
    ///
    /// It is also the whole of what this policy will see: the envelope carries a digest of every
    /// argument and the *selected* values only. Declaring less is seeing less.
    fn security_relevant_arguments(&self) -> Vec<String> {
        vec!["amount_pence".to_string()]
    }

    /// Which operations this policy speaks about at all. An unmapped operation is **not** a denial;
    /// it is the policy declining to have an opinion, and the seam turns that into
    /// `Indeterminate`.
    fn mapping(&self, operation: &str, resource: &str) -> ActionMapping {
        match (operation, resource) {
            ("tools/call", r) if r.starts_with("tool:purchase.place@") =>
                ActionMapping::mapped("purchasing-remit", REVISION),
            ("tools/call", r) if r.starts_with("tool:ledger.export@") =>
                ActionMapping::mapped("purchasing-remit", REVISION),
            _ => ActionMapping::unmapped("purchasing-remit", REVISION),
        }
    }

    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision {
        let tool = envelope.resource.split('@').next().unwrap_or("");

        match tool {
            // Exporting the ledger is outside the remit entirely. A standing answer about
            // authority: retrying will not change it.
            "tool:ledger.export" => Decision::deny(
                "ledger.export is not in the purchasing remit", REVISION,
            ).checking(["remit: purchase.place only"]),

            "tool:purchase.place" => {
                // The ceiling. Note `selected_arguments`, not "arguments": the envelope carries a
                // **digest** of the whole argument set plus only the values this evaluator asked
                // for by name. A policy sees what it declared it needs and no more — so attaching
                // an evaluator is not a licence to read every payload crossing the gateway.
                match envelope.selected_arguments.get("amount_pence").and_then(|v| v.as_u64()) {
                    Some(amount) if amount <= 50_000 => Decision::permit(
                        format!("within remit: {amount}p ≤ 50000p"), REVISION,
                    ).checking([format!("amount_pence={amount}"), "ceiling=50000".into()]),

                    Some(amount) => Decision::deny(
                        format!("over remit ceiling: {amount}p > 50000p"), REVISION,
                    ).checking([format!("amount_pence={amount}"), "ceiling=50000".into()]),

                    // The argument is missing or not a number. The policy has a rule and cannot
                    // apply it — which is neither permission nor prohibition.
                    None => Decision::indeterminate(
                        "amount_pence absent or not an integer; the ceiling cannot be applied",
                        REVISION,
                    ).with_errors(["required argument `amount_pence` missing"]),
                }
            }

            // A tool nobody wrote a rule about. Authority is not established, so the secure
            // profile refuses — rather than permitting because nothing said no.
            _ => Decision::indeterminate(
                format!("no clause covers {tool}"), REVISION,
            ),
        }
    }
}

fn step(n: u8, title: &str) { println!("\n\x1b[1m{n}. {title}\x1b[0m"); }
fn note(s: impl AsRef<str>) { println!("   {}", s.as_ref()); }

#[tokio::main]
async fn main() {
    let gossip: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(57800);
    let http = gossip + 1;
    let id = NodeId::new("127.0.0.1", gossip).expect("node id");
    let cert_dir = std::env::temp_dir().join(format!("myc-mcp-authority-{gossip}"));
    let _ = std::fs::remove_dir_all(&cert_dir);

    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip;
    cfg.http_port = Some(http);
    cfg.gateway_auth_token = Some("agent-token".to_string());
    // The evidence journal is signed, so the decisions this gateway makes are attributable.
    cfg.tls = Some(TlsConfig { auto_cert_dir: cert_dir.clone(), ..TlsConfig::default() });
    let agent = Arc::new(GossipAgent::new(id.clone(), cfg));

    // Attach the policy. Without this the seam is INERT and the gateway behaves exactly as it
    // always did — nothing is silently enforced because a crate got linked.
    agent.with_action_evaluator(Arc::new(PurchasingRemit));
    agent.start().await.expect("start");

    // Three REAL tools, served by this node. `register_mcp_tool` both runs the handler and writes
    // `tools/{name}/{node}`, which is how the gateway finds a provider — so a permitted call
    // actually executes and returns a result, rather than being permitted into a void.
    let _purchase = agent.mcp().register_mcp_tool(
        "purchase.place",
        serde_json::json!({"type": "object", "properties": {"amount_pence": {"type": "integer"}}}),
        |args| async move {
            let amount = args["amount_pence"].as_u64().unwrap_or(0);
            Ok(serde_json::json!({"ordered": true, "amount_pence": amount}))
        },
    );
    let _ledger = agent.mcp().register_mcp_tool(
        "ledger.export",
        serde_json::json!({"type": "object"}),
        |_args| async move { Ok(serde_json::json!({"rows": 4471})) },
    );
    let _weather = agent.mcp().register_mcp_tool(
        "weather.lookup",
        serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        |args| async move { Ok(serde_json::json!({"city": args["city"], "sky": "dreich"})) },
    );
    // All three are live and callable. Nothing below is stopped by a missing provider — every
    // refusal you see is the policy.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http}/mcp");
    let health = format!("http://127.0.0.1:{http}/health");
    for _ in 0..60 {
        if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let call = |tool: &str, args: serde_json::Value| {
        let c = client.clone();
        let u = url.clone();
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": args},
        });
        async move {
            c.post(&u).bearer_auth("agent-token").json(&body)
                .send().await.expect("mcp request")
                .json::<serde_json::Value>().await.expect("json reply")
        }
    };

    println!("\x1b[1mOne agent, three tools, one remit\x1b[0m");
    note("The agent's token carries `mcp:invoke` — so scopes alone would let it call all three.");

    // ── 1. within the remit ─────────────────────────────────────────────────────────────────
    step(1, "purchase.place, £120 — within the remit");
    let r = call("purchase.place", serde_json::json!({"amount_pence": 12_000})).await;
    assert!(r.get("error").is_none(), "a call within the remit must not be refused: {r}");
    note(format!("gateway: dispatched — the tool ran and returned {}", r["result"]));
    note("✓ permitted, executed, answered. The other four acts below are the same request shape");
    note("  reaching a different verdict — so every refusal you see is the policy, not plumbing.");

    // ── 2. over the ceiling ─────────────────────────────────────────────────────────────────
    step(2, "purchase.place, £900 — over the ceiling");
    let r = call("purchase.place", serde_json::json!({"amount_pence": 90_000})).await;
    let err = r.get("error").expect("an over-limit call must be refused");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note("✓ denied on an argument VALUE — a ceiling is not something a digest can check, which is");
    note("  why the evaluator declares `amount_pence` as security-relevant and the seam carries it");

    // ── 3. a tool outside the remit ─────────────────────────────────────────────────────────
    step(3, "ledger.export — a tool the remit excludes");
    let r = call("ledger.export", serde_json::json!({})).await;
    let err = r.get("error").expect("an out-of-remit tool must be refused");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note("✓ a standing answer about authority: retrying will not change it");

    // ── 4. the one that matters ─────────────────────────────────────────────────────────────
    step(4, "weather.lookup — a tool nobody wrote a rule about");
    let r = call("weather.lookup", serde_json::json!({"city": "Glasgow"})).await;
    let err = r.get("error").expect("an unmapped tool must be refused, not permitted");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note("✓ **not denied — undecided.** The policy has no clause about this tool, so authority was");
    note("  never established. The secure profile refuses rather than falling through to allow,");
    note("  and the evidence records that it could not decide rather than that it said no.");
    note("  Those are different facts, and an engine that conflates them lies in both directions.");

    // ── 5. and the missing argument ─────────────────────────────────────────────────────────
    step(5, "purchase.place with no amount — a rule that cannot be applied");
    let r = call("purchase.place", serde_json::json!({"supplier": "orchard-co"})).await;
    let err = r.get("error").expect("an inapplicable rule must refuse");
    note(format!("gateway: refused — {}", err["message"].as_str().unwrap_or("")));
    note("✓ the policy HAS a rule here and could not apply it. Also indeterminate, also refused —");
    note("  a missing fact must never resolve in the caller's favour.");

    println!("\n\x1b[1mScopes say whether an agent may call tools. Policy says which ones.\x1b[0m");
    println!("And when policy cannot say, the answer is `refuse`, not `permit`.");

    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
