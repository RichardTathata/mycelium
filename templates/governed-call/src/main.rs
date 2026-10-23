//! governed-call: two nodes, one governed call — permitted, refused, and recorded.
//!
//! Node A serves a tool under a policy enforced *at the provider*, with an evidence journal (node-local, fsynced)
//! and a decision trace. Node B, a member of the same fleet, calls the tool directly — no gateway between them — once in remit and
//! once out of it, then calls a prompt skill whose text is `skills/demo/greeting.md`. Everything the fleet decided is in
//! `data/`: `mycelium explain data/decisions.jsonl` · `mycelium doctor node.toml`.
mod policy;
use mycelium::decision::{DecisionSink, SinkConfig};
use mycelium::{EchoBackend, EvidenceJournal, EvidenceProfile, GossipAgent, GossipConfig, NodeCapabilityConfig, NodeId};
use mycelium::{PromptSkillError, PromptTemplate, read_evidence_journal, run_capability_probes};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

fn node(cfg: &GossipConfig) -> Result<Arc<GossipAgent>, Box<dyn std::error::Error>> {
    Ok(Arc::new(GossipAgent::new(NodeId::new(&cfg.bind_address, cfg.bind_port)?, cfg.clone())))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("data")?;
    // Node A, the provider: policy, journal and trace are attached before start(), which is the lifecycle boundary.
    let cfg_a = GossipConfig::load_from_file("node.toml")?;
    let a = node(&cfg_a)?;
    a.with_evidence_journal(EvidenceJournal::open("data/evidence.log", EvidenceProfile::Strict)?);
    a.with_action_evaluator(Arc::new(policy::Remit));
    a.with_provider_enforcement();
    let trace = Arc::new(DecisionSink::new(SinkConfig::default()));
    a.with_decision_trace(Arc::clone(&trace));
    a.start().await?;
    let _tool = a.mcp().register_mcp_tool("purchase.place", serde_json::json!({"type": "object"}), |args| async move {
        Ok(serde_json::json!({ "placed": args }))
    });
    let units = NodeCapabilityConfig::load_from_file("units/purchaser.toml")?;
    tokio::spawn(run_capability_probes(Arc::clone(&a), units, Arc::new(AtomicBool::new(false)), |_| {}));

    // Node B, a member: it joins by reachability, serves the prompt skill, and calls A's tool directly.
    let mut cfg_b = cfg_a.clone();
    cfg_b.bind_port = cfg_a.bind_port.checked_add(1).ok_or("bind_port + 1 must fit in a port: lower bind_port in node.toml")?;
    cfg_b.bootstrap_peers = vec![a.node_id().clone()];
    let b = node(&cfg_b)?;
    b.start().await?;
    let skill = PromptTemplate { system: String::new(), user_template: std::fs::read_to_string("skills/demo/greeting.md")?, max_tokens: 64, temperature: 0.0, metadata: Default::default() };
    let _skill = b.llm().register_prompt_skill("demo", "greeting", skill, Arc::new(EchoBackend)).await?;
    while b.connected_peers().is_empty() { tokio::time::sleep(Duration::from_millis(50)).await }

    // `_meta.correlation` names the run; every record this call produces carries it: `mycelium explain --correlation run-1 data/evidence.log`.
    let call = |ledger: &str| serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "purchase.place", "arguments": {"ledger": ledger, "amount_pence": 1200}, "_meta": {"correlation": "run-1"}}}).to_string().into_bytes();
    for ledger in ["petty-cash", "capital"] {
        let raw = b.service().rpc_call(a.node_id().clone(), "mcp.invoke", call(ledger), Duration::from_secs(5)).await?;
        let reply: serde_json::Value = serde_json::from_slice(&raw)?;
        match reply.get("error") {
            None => println!("ledger={ledger}: PERMITTED — {}", reply["result"]),
            Some(e) => println!("ledger={ledger}: REFUSED — {} ({})", e["data"]["reason"], e["message"]),
        }
    }
    let mut answer = Err(PromptSkillError::NoProvider { ns: "demo".into(), name: "greeting".into() });
    for _ in 0..100 {   // B's advert reaches A by gossip; a bounded wait, so a fleet that never converges fails
        answer = a.llm().call_prompt_skill("demo", "greeting", "the new joiner", Default::default(), Duration::from_secs(5)).await;
        if !matches!(answer, Err(PromptSkillError::NoProvider { .. })) { break }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let answer = answer?;
    println!("skill demo/greeting answered: {answer}");

    std::fs::write("data/decisions.jsonl", trace.to_jsonl())?;
    for record in read_evidence_journal(std::path::Path::new("data/evidence.log"))? {
        println!("evidence: {}", String::from_utf8_lossy(&record));
    }
    println!("decision trace: data/decisions.jsonl ({} records); evidence journal: data/evidence.log", trace.snapshot().len());
    b.shutdown().await;
    a.shutdown().await;
    Ok(())
}
