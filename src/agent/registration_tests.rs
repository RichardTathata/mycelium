//! **Two registrations of one name do not withdraw each other** (engineering review ER10; tickets R6-06, R10-03,
//! R21 "two editions", R22 "MCP replacement"; decision D3). A node's advertisement is its own promise: a handle
//! withdraws only the registration it made, the key carries the newest live registration's value, and only the last
//! tombstones it. The guard is node-local (`mycelium_core::kv_persist::Registrations`), not a Layer I write guard.

use crate::capability::{CapFilter, Capability, ReqEntry};
use crate::{GossipAgent, GossipConfig, NodeId};
use std::sync::Arc;
use std::time::Duration;

/// A refresh period no test waits out: a key live after a drop is live because nothing withdrew it, not because a
/// remaining writer re-asserted it.
const LONG: Duration = Duration::from_secs(30);
/// How long a test waits for a withdrawal it expects not to happen. The unfixed code tombstoned within milliseconds.
const SETTLE: Duration = Duration::from_millis(400);

async fn node() -> GossipAgent {
    let port = crate::test_util::alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = port;
    let agent = GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg);
    agent.start().await.unwrap();
    agent
}

async fn until(mut f: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if f() { return true; }
        if tokio::time::Instant::now() >= deadline { return false; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn live(agent: &GossipAgent, key: &str) -> bool {
    agent.kv().get(key).is_some()
}

fn demanded(agent: &GossipAgent, filter: &CapFilter) -> bool {
    agent.capabilities().demand(filter).demanding_nodes.iter().any(|n| n == agent.node_id())
}

/// **R6-06: a remaining requirement keeps its name declared until it drops** — in both drop orders. Two
/// `declare_requirement`s of one model (two resumed workflows' `require_model`) write one key; until ER10 dropping
/// either tombstoned it, and the demand read zero for up to a refresh interval while the other was still held.
/// A second model's requirement drops independently (the control). Seen failing first: "the first drop withdrew
/// the requirement the second handle still holds" in both orders.
#[tokio::test]
async fn a_remaining_requirement_keeps_its_name_declared_until_it_drops() {
    let agent = node().await;
    let model = CapFilter::new("llm", "summary-small");
    let other = CapFilter::new("llm", "other-model");
    let key = format!("req/{}/llm/summary-small", agent.node_id());
    let other_key = format!("req/{}/llm/other-model", agent.node_id());
    for first_dropped in [0usize, 1] {
        let mut handles = vec![
            Some(agent.capabilities().declare_requirement(model.clone(), LONG)),
            Some(agent.capabilities().declare_requirement(model.clone(), LONG)),
        ];
        let other_handle = agent.capabilities().declare_requirement(other.clone(), LONG);
        assert!(until(|| live(&agent, &key) && live(&agent, &other_key), Duration::from_secs(5)).await, "both names declared");
        handles[first_dropped].take();
        tokio::time::sleep(SETTLE).await;
        assert!(live(&agent, &key) && demanded(&agent, &model),
            "order {first_dropped}: the first drop withdrew the requirement the second handle still holds");
        drop(other_handle);
        assert!(until(|| !live(&agent, &other_key), Duration::from_secs(5)).await, "the other model's requirement withdraws on its own");
        assert!(live(&agent, &key), "order {first_dropped}: another name's withdrawal took this one");
        handles.clear();
        assert!(until(|| !live(&agent, &key), Duration::from_secs(5)).await, "order {first_dropped}: the last handle's drop withdraws the name");
        assert!(!demanded(&agent, &model));
    }
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **The opacity of a remaining requirement stays tracked** (ER10's enumeration; the sibling of R6-06 in the
/// consolidated opacity watcher). Two unmet requirements of one name share `sys/load/{node}/req/{ns}/{name}`; until
/// ER10 dropping the first cleared it and removed *every* registration of the key from the watcher, so the second
/// requirement's opacity was never written again. Seen failing first: "the first drop cleared the opacity the
/// second requirement still holds".
#[tokio::test]
async fn a_remaining_requirement_stays_opaque_until_it_drops() {
    let agent = node().await;
    let filter = CapFilter::new("llm", "nobody-serves-this");
    let opacity = format!("sys/load/{}/req/llm/nobody-serves-this", agent.node_id());
    for first_dropped in [0usize, 1] {
        let mut handles = vec![
            Some(agent.capabilities().declare_requirement(filter.clone(), LONG)),
            Some(agent.capabilities().declare_requirement(filter.clone(), LONG)),
        ];
        assert!(until(|| live(&agent, &opacity), Duration::from_secs(5)).await, "an unmet requirement is opaque");
        handles[first_dropped].take();
        tokio::time::sleep(SETTLE).await;
        assert!(live(&agent, &opacity), "order {first_dropped}: the first drop cleared the opacity the second requirement still holds");
        handles.clear();
        assert!(until(|| !live(&agent, &opacity), Duration::from_secs(5)).await, "order {first_dropped}: the last drop clears it");
    }
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **R21's library half: two different filters for one name** — the record carries the newest declaration's filter,
/// and when that one drops the older one's filter is written back at once, not at its next refresh. (The unit-file
/// half — two editions refused where a unit is declared — is `two_editions_of_one_requirement_are_refused_by_a_stem`
/// in `mycelium-wasm-host`'s `stem`, and the gateway's `units/declare` uses the same check.) Seen failing first: "dropping the newer edition withdrew the older one's requirement".
#[tokio::test]
async fn dropping_the_newer_edition_writes_the_older_one_back() {
    let agent = node().await;
    let v1 = CapFilter::new("library", "hours").with_schema("v1");
    let v2 = CapFilter::new("library", "hours").with_schema("v2");
    let key = format!("req/{}/library/hours", agent.node_id());
    let filter_held = |a: &GossipAgent| a.kv().get(&key).and_then(|b| ReqEntry::decode(&b)).map(|e| e.filter);
    let _old = agent.capabilities().declare_requirement(v1.clone(), LONG);
    let new = agent.capabilities().declare_requirement(v2.clone(), LONG);
    assert!(until(|| filter_held(&agent) == Some(v2.clone()), Duration::from_secs(5)).await, "the newest declaration's filter is the record");
    drop(new);
    assert!(until(|| filter_held(&agent) == Some(v1.clone()), SETTLE).await,
        "dropping the newer edition withdrew the older one's requirement (held: {:?})", filter_held(&agent));
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A capability advertised twice is withdrawn by its last handle only** — the persist task's family member every
/// `advertise_capability` caller shares (a provisioner's build, a schedule's candidacy, a stem's `[[capability]]`).
/// Seen failing first: "the older handle's drop withdrew the newer advertisement".
#[tokio::test]
async fn a_capability_advertised_twice_is_withdrawn_by_its_last_handle() {
    let agent = node().await;
    let key = format!("cap/{}/route/twice", agent.node_id());
    let old = agent.capabilities().advertise_capability(Capability::new("route", "twice"), LONG);
    let new = agent.capabilities().advertise_capability(Capability::new("route", "twice"), LONG);
    assert!(until(|| live(&agent, &key), Duration::from_secs(5)).await);
    drop(old);
    tokio::time::sleep(SETTLE).await;
    assert!(live(&agent, &key), "the older handle's drop withdrew the newer advertisement");
    drop(new);
    assert!(until(|| !live(&agent, &key), Duration::from_secs(5)).await, "the last handle withdraws it");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **A refresher whose task never ran withdraws on drop** — the registration is made when the task's future is
/// built, so a future dropped unpolled (a task set aborted, a runtime going down) must not leave a registration that
/// holds the key with nobody refreshing it. A pin of `HeldKey`'s drop: the unfixed code had no registrations, so it
/// passes there too.
#[tokio::test]
async fn a_refresher_dropped_unpolled_hands_the_key_back() {
    let agent = node().await;
    let key = format!("cap/{}/route/unpolled", agent.node_id());
    let _live = agent.capabilities().advertise_capability(Capability::new("route", "unpolled"), Duration::from_millis(100));
    assert!(until(|| live(&agent, &key), Duration::from_secs(5)).await);
    let core = Arc::clone(&agent.task_ctx.core);
    let (_cancel, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let fut = mycelium_core::kv_persist::run_kv_persist_task(
        Arc::clone(&core), cancel_rx, agent.task_ctx.shutdown_tx.subscribe(), Arc::from(key.as_str()),
        Duration::from_millis(100), Arc::new(|| bytes::Bytes::from_static(b"never")), None,
    );
    assert_eq!(core.soft_state_holders.count(&key), 2, "the future registered when it was built");
    drop(fut);
    assert_eq!(core.soft_state_holders.count(&key), 1, "dropped unpolled, it withdrew");
    let ts = |a: &GossipAgent| a.task_ctx.kv_state.store.pin().get(key.as_str()).map(|e| e.timestamp);
    let before = ts(&agent);
    assert!(until(|| ts(&agent) != before && live(&agent, &key), Duration::from_secs(5)).await,
        "the live handle refreshes the key again");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── MCP tools (R22, ER5-F1's stem path) ──────────────────────────────────────────────────────────────────────

fn tool_call(name: &str) -> Vec<u8> {
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": {}}})
        .to_string().into_bytes()
}

async fn call_tool(agent: &GossipAgent, name: &str) -> String {
    let reply = agent.service()
        .rpc_call(agent.node_id().clone(), crate::signal::signal_kind::MCP_INVOKE, tool_call(name), Duration::from_secs(5))
        .await.expect("the tool answers");
    String::from_utf8_lossy(&reply).into_owned()
}

fn register(agent: &GossipAgent, name: &str, schema: serde_json::Value, answer: &'static str) -> crate::McpToolHandle {
    agent.mcp().register_mcp_tool(name, schema, move |_args| async move { Ok(serde_json::json!({ "served_by": answer })) })
}

fn schema(required: &str) -> serde_json::Value {
    serde_json::json!({"type": "object", "required": [required]})
}

/// **R22: a replacement MCP tool survives the older installation's retirement.** Install `notice` (the calculate
/// component), then a replacement under the same name (the weather component), then retire the first: discovery must
/// still return the replacement's schema and calls still reach it; an independently named tool is untouched; the
/// replacement's own drop removes the name. While both are live only the newest serves. Seen failing first: "the
/// older tool's retirement withdrew the replacement's discovery entry".
#[tokio::test]
async fn a_replacement_mcp_tool_survives_the_older_ones_retirement() {
    let agent = node().await;
    let key = format!("tools/notice/{}", agent.node_id());
    let guide_key = format!("tools/guide/{}", agent.node_id());
    let held_schema = |a: &GossipAgent| a.kv().get(&key).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let _guide = register(&agent, "guide", schema("text"), "guide");
    let calculate = register(&agent, "notice", schema("expression"), "calculate");
    let weather = register(&agent, "notice", schema("city"), "weather");
    assert_eq!(held_schema(&agent), Some(schema("city")), "the replacement's schema is the discovery entry");
    for _ in 0..5 {
        let r = call_tool(&agent, "notice").await;
        assert!(r.contains("weather"), "while both are live the newest serves the name: {r}");
    }
    drop(calculate);
    tokio::time::sleep(SETTLE).await;
    assert_eq!(held_schema(&agent), Some(schema("city")), "the older tool's retirement withdrew the replacement's discovery entry");
    assert!(call_tool(&agent, "notice").await.contains("weather"));
    assert!(live(&agent, &guide_key) && call_tool(&agent, "guide").await.contains("guide"), "an independent name is untouched");
    drop(weather);
    assert!(until(|| !live(&agent, &key), Duration::from_secs(5)).await, "the last registration's drop removes the name");
    assert!(live(&agent, &guide_key));
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Dropping the replacement while the original is live restores the original** — its schema written back at once
/// and its handler serving again: the original's promise still stands. Seen failing first: "dropping the
/// replacement withdrew the tool the original still serves".
#[tokio::test]
async fn dropping_the_replacement_restores_the_original_tool() {
    let agent = node().await;
    let key = format!("tools/notice/{}", agent.node_id());
    let held_schema = |a: &GossipAgent| a.kv().get(&key).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let _original = register(&agent, "notice", schema("expression"), "calculate");
    let replacement = register(&agent, "notice", schema("city"), "weather");
    drop(replacement);
    assert!(until(|| held_schema(&agent) == Some(schema("expression")), SETTLE).await,
        "dropping the replacement withdrew the tool the original still serves (held: {:?})", held_schema(&agent));
    assert!(call_tool(&agent, "notice").await.contains("calculate"), "the original serves the name again");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **ER5-F1's stem path, at the library's surface:** a new stem's tool `x` is live; an install still in flight from
/// the stopped stem finishes, registers `tools/x/{node}` under a newer stamp, finds its provisioner closed and
/// retires. Until ER10 that retirement tombstoned the key — the live tool undiscoverable for good, since `tools/` is
/// written once — and the late install's loop answered the name beside the live one. Now the late registration's
/// drop writes the live tool's schema back. (The provisioner-level reproduction needs an install held past a stem
/// restart, which the stack below ER10 does not yet expose.) Seen failing first: "the late install's retirement left
/// the live tool undiscoverable".
#[tokio::test]
async fn a_late_install_retiring_leaves_the_live_tool_discoverable() {
    let agent = node().await;
    let key = format!("tools/x/{}", agent.node_id());
    let held_schema = |a: &GossipAgent| a.kv().get(&key).and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    let _live = register(&agent, "x", schema("live"), "new-stem");
    let late = register(&agent, "x", schema("late"), "old-stem");
    drop(late);
    assert!(until(|| held_schema(&agent) == Some(schema("live")), SETTLE).await,
        "the late install's retirement left the live tool undiscoverable (held: {:?})", held_schema(&agent));
    assert!(call_tool(&agent, "x").await.contains("new-stem"));
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

// ── Prompt skills (R10-03) ───────────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "llm")]
fn template(tag: &str) -> crate::PromptTemplate {
    crate::PromptTemplate {
        system: "s".into(),
        user_template: format!("{tag} {{{{input}}}}"),
        max_tokens: 16,
        temperature: 0.0,
        metadata: std::collections::HashMap::new(),
    }
}

/// **R10-03: a replacement prompt skill stays callable when the older registration drops** — with a new backend
/// object and with the same one. Discovery and invocation hold at once, not after the next refresh; the newest's own
/// drop, as the last, retracts the advertisement and the backend. Seen failing first: with a new backend, "the old
/// handle's drop withdrew the replacement's advertisement"; with the same backend, also its backend.
#[cfg(feature = "llm")]
#[tokio::test]
async fn a_replacement_prompt_skill_survives_the_older_registrations_drop() {
    let agent = node().await;
    let cap_key = format!("cap/{}/review/brief", agent.node_id());
    for shared_backend in [false, true] {
        let first: Arc<dyn crate::LlmBackend> = Arc::new(crate::EchoBackend);
        let second = if shared_backend { Arc::clone(&first) } else { Arc::new(crate::EchoBackend) as Arc<dyn crate::LlmBackend> };
        let old = agent.llm().register_prompt_skill("review", "brief", template("old"), first).await.unwrap();
        let new = agent.llm().register_prompt_skill("review", "brief", template("new"), second).await.unwrap();
        assert!(until(|| live(&agent, &cap_key), Duration::from_secs(5)).await);
        drop(old);
        tokio::time::sleep(SETTLE).await;
        assert!(live(&agent, &cap_key), "shared {shared_backend}: the old handle's drop withdrew the replacement's advertisement");
        assert!(agent.task_ctx.llm_skills.pin().get("review/brief").is_some(),
            "shared {shared_backend}: the old handle's drop removed the replacement's backend");
        let out = agent.llm().call_prompt_skill("review", "brief", "hi", Default::default(), Duration::from_secs(5)).await
            .unwrap_or_else(|e| panic!("shared {shared_backend}: the replacement is not callable: {e}"));
        assert_eq!(out, "echo: new hi", "the replacement's template serves");
        drop(new);
        assert!(until(|| !live(&agent, &cap_key), Duration::from_secs(5)).await, "shared {shared_backend}: the last drop retracts the advertisement");
        assert!(until(|| agent.task_ctx.llm_skills.pin().get("review/brief").is_none(), Duration::from_secs(5)).await,
            "shared {shared_backend}: the last drop removes the backend");
    }
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// **Dropping the newest prompt registration while an older one is live restores the older one's backend**, and
/// the skill stays advertised and callable — two distinct names sharing one backend stay independent (the control).
/// Seen failing first: "dropping the newest removed the backend the older registration still advertises".
#[cfg(feature = "llm")]
#[tokio::test]
async fn dropping_the_newest_prompt_registration_restores_the_older_backend() {
    let agent = node().await;
    let cap_key = format!("cap/{}/review/brief", agent.node_id());
    let shared: Arc<dyn crate::LlmBackend> = Arc::new(crate::EchoBackend);
    let _old = agent.llm().register_prompt_skill("review", "brief", template("old"), Arc::clone(&shared)).await.unwrap();
    let _other = agent.llm().register_prompt_skill("review", "other", template("other"), Arc::clone(&shared)).await.unwrap();
    let new = agent.llm().register_prompt_skill("review", "brief", template("new"), Arc::new(crate::EchoBackend)).await.unwrap();
    assert!(until(|| live(&agent, &cap_key), Duration::from_secs(5)).await);
    drop(new);
    tokio::time::sleep(SETTLE).await;
    let held = agent.task_ctx.llm_skills.pin().get("review/brief").cloned();
    assert!(held.as_ref().is_some_and(|b| Arc::ptr_eq(b, &shared)),
        "dropping the newest removed the backend the older registration still advertises");
    assert!(live(&agent, &cap_key), "the older registration's advertisement was withdrawn");
    // The template is cluster configuration, not a registration's: the newest write stays.
    let out = agent.llm().call_prompt_skill("review", "brief", "hi", Default::default(), Duration::from_secs(5)).await.unwrap();
    assert_eq!(out, "echo: new hi");
    assert!(agent.llm().call_prompt_skill("review", "other", "hi", Default::default(), Duration::from_secs(5)).await.is_ok(),
        "a distinct name sharing the backend is untouched");
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
