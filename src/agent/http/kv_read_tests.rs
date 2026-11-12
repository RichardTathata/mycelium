//! The raw KV **read** doors read application keys only (the kv:read decision, 2026-11-12), mirroring the write
//! doors (#549). Rule 1's enumeration, one test per door: `GET /gateway/kv`, `GET /gateway/kv/keys` (unprefixed,
//! a prefix inside a names-hidden namespace, a prefix that spans one), `GET /gateway/overlay/consistent/get`'s
//! raw-key fallback; and the routes the three dependants moved to — `GET /gateway/govern`'s `topology_overrides`
//! and `GET /gateway/schedules/last`.

use crate::{GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// The names the listing hides: exactly these rows, and each one's keys are owned.
const HIDDEN: &[&str] = &["sys/audit/", "sys/client-cert/", "audit/", "mailbox/", "mandate/", "rights/", "grant/"];

async fn node(cfg_edit: impl FnOnce(&mut GossipConfig)) -> (Arc<GossipAgent>, String, reqwest::Client) {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg_edit(&mut cfg);
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    agent.start().await.unwrap();
    let base = format!("http://127.0.0.1:{http_port}");
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while client.get(format!("{base}/health")).send().await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "the gateway did not come up");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    (agent, base, client)
}

/// Owned records as the substrate would have written them, and application keys beside them.
fn seed(agent: &GossipAgent) -> (Vec<String>, Vec<&'static str>) {
    let owned: Vec<String> = super::OWNED_KV_PREFIXES.iter().map(|p| format!("{}probe/1", p.prefix)).collect();
    for key in &owned {
        let _ = agent.kv().set(key.clone(), Bytes::from_static(b"owned"));
    }
    let application = vec!["orders/42", "ckpt/t/ns/1", "ui/viz", "agent/n/provision/item/error", "gravel/1", "grantee"];
    for key in &application {
        let _ = agent.kv().set(key.to_string(), Bytes::from_static(b"application"));
    }
    (owned, application)
}

async fn get(client: &reqwest::Client, url: String, bearer: Option<&str>) -> (u16, serde_json::Value) {
    let mut r = client.get(url);
    if let Some(b) = bearer {
        r = r.bearer_auth(b);
    }
    let r = r.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(serde_json::Value::Null))
}

fn names(body: &serde_json::Value) -> Vec<String> {
    body["keys"].as_array().expect("a keys array").iter().map(|k| k.as_str().unwrap().to_string()).collect()
}

/// One table, two directions: the hidden rows are the ones decided, every hidden name is an owned key, and the
/// read predicate is the write predicate — a key the write doors refuse is a key the read doors refuse.
#[test]
fn the_read_and_write_doors_classify_by_the_one_table() {
    let hidden: Vec<&str> = super::OWNED_KV_PREFIXES.iter().filter(|p| p.names_hidden).map(|p| p.prefix).collect();
    let mut want = HIDDEN.to_vec();
    want.sort_unstable();
    let mut got = hidden.clone();
    got.sort_unstable();
    assert_eq!(got, want, "the names-hidden rows");
    for p in super::OWNED_KV_PREFIXES {
        let key = format!("{}x", p.prefix);
        assert!(!super::raw_kv_writable(&key), "{key} is owned");
        assert_eq!(super::refuse_protected_key(&key).is_some(), super::refuse_protected_key_read(&key).is_some(), "{key}");
        assert!(super::refuse_protected_key_read(&key).is_some(), "{key} is refused on the read doors");
        assert_eq!(super::raw_kv_name_hidden(&key), HIDDEN.iter().any(|h| key.starts_with(h)), "{key}");
        // A hidden sub-row is inside an owned row: hiding never reaches a key the doors would serve.
        if p.names_hidden {
            assert!(super::refuse_hidden_prefix(p.prefix).is_some(), "a listing inside {}", p.prefix);
        } else {
            assert!(super::refuse_hidden_prefix(p.prefix).is_none(), "{} stays listable", p.prefix);
        }
    }
    for key in ["orders/42", "ckpt/t/ns/1", "ckptw/t/ns/1/task/0", "strands/s", "manifest/current", "schemas/s", "ui/viz",
                "agent/n/provision/item/error", "grantee", "mailboxes/1"] {
        assert!(super::refuse_protected_key(key).is_none() && super::refuse_protected_key_read(key).is_none(), "{key}");
        assert!(!super::raw_kv_name_hidden(key), "{key} is listed");
    }
    // The listed namespaces the Ops Console and the CI probes read by name stay listed.
    for key in ["grp/workers/n", "sys/identity/n", "sys/caller-context/n", "cap/n/demo/echo", "skills/ns/n", "consensus/committed/s"] {
        assert!(!super::raw_kv_name_hidden(key), "{key}");
    }
}

/// Door 1, `GET /gateway/kv?key=`: every owned key is 403 `protected_key`, present or absent alike; an
/// application key is read.
#[tokio::test]
async fn kv_get_refuses_an_owned_key_and_reads_an_application_key() {
    let (agent, base, client) = node(|_| {}).await;
    let (owned, application) = seed(&agent);
    for key in owned.iter().map(String::as_str).chain(["grant/absent/none", "sys/audit/absent/9", "sched/x~anonymous/last/n"]) {
        let (status, body) = get(&client, format!("{base}/gateway/kv?key={key}"), None).await;
        assert_eq!(status, 403, "GET {key}: {body}");
        assert_eq!(body["error"], "protected_key", "{key}");
        assert!(body.get("value_b64").is_none() && body.get("found").is_none(), "{key}: {body}");
    }
    // The refusal names the route that reads the namespace.
    for (key, route) in [("grant/a/r", "/gateway/grants/pending"), ("sys/audit/n/1", "/gateway/audit"),
                         ("sys/topology-override/workers", "/gateway/govern"), ("sched/s/last/n", "/gateway/schedules/last"),
                         ("grp/workers/n", "/gateway/mesh/group"), ("mailbox/n/k/1", "/gateway/mailbox/{kind}")] {
        let (_, body) = get(&client, format!("{base}/gateway/kv?key={key}"), None).await;
        assert!(body["message"].as_str().unwrap_or("").contains(route), "{key}: {body}");
    }
    for key in application {
        let (status, body) = get(&client, format!("{base}/gateway/kv?key={key}"), None).await;
        assert_eq!(status, 200, "GET {key}");
        assert_eq!(body["found"], true, "{key}: {body}");
    }
    let (status, body) = get(&client, format!("{base}/gateway/kv?key=orders/absent"), None).await;
    assert_eq!((status, &body["found"]), (200, &serde_json::json!(false)));
    agent.shutdown().await;
}

/// Door 2, `GET /gateway/kv/keys`: the unprefixed listing and a prefix that spans a hidden namespace leave the
/// hidden names out and keep the rest; a prefix inside a hidden namespace is 403 `protected_key`.
#[tokio::test]
async fn kv_keys_hides_the_hidden_names_and_refuses_a_prefix_inside_them() {
    let (agent, base, client) = node(|_| {}).await;
    let (owned, application) = seed(&agent);
    let (status, body) = get(&client, format!("{base}/gateway/kv/keys"), None).await;
    assert_eq!(status, 200);
    let all = names(&body);
    for key in &owned {
        let hidden = HIDDEN.iter().any(|h| key.starts_with(h));
        assert_eq!(all.contains(key), !hidden, "{key} in the unprefixed listing");
    }
    for key in &application {
        assert!(all.iter().any(|k| k == key), "{key} listed");
    }
    for h in HIDDEN {
        for prefix in [h.to_string(), format!("{h}probe/")] {
            let (status, body) = get(&client, format!("{base}/gateway/kv/keys?prefix={prefix}"), None).await;
            assert_eq!(status, 403, "prefix {prefix}: {body}");
            assert_eq!(body["error"], "protected_key", "{prefix}");
            assert!(body.get("keys").is_none(), "{prefix}: {body}");
        }
    }
    // A prefix that spans hidden rows: `sys/` keeps its listed names and drops the audit chain and client certificates.
    let (status, body) = get(&client, format!("{base}/gateway/kv/keys?prefix=sys/"), None).await;
    assert_eq!(status, 200);
    let sys = names(&body);
    assert!(sys.iter().any(|k| k == "sys/probe/1"), "{sys:?}");
    assert!(!sys.iter().any(|k| k.starts_with("sys/audit/") || k.starts_with("sys/client-cert/")), "{sys:?}");
    // A prefix shorter than a hidden row's: the application keys it matches, none of the row's.
    let (status, body) = get(&client, format!("{base}/gateway/kv/keys?prefix=gra"), None).await;
    assert_eq!(status, 200);
    let mut gra = names(&body);
    gra.sort();
    assert_eq!(gra, ["grantee", "gravel/1"]);
    // A listed owned namespace and an application one are listed whole.
    for (prefix, want) in [("cap/", "cap/probe/1"), ("grp/", "grp/probe/1"), ("orders/", "orders/42")] {
        let (status, body) = get(&client, format!("{base}/gateway/kv/keys?prefix={prefix}"), None).await;
        assert_eq!(status, 200, "{prefix}");
        assert_eq!(names(&body), [want], "{prefix}");
    }
    agent.shutdown().await;
}

/// Door 3, `GET /gateway/overlay/consistent/get?key=`: its fallback reads the raw key, so an owned key is 403
/// there too; an application key still falls back to its raw value.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn consistent_get_refuses_an_owned_key_at_its_raw_fallback() {
    let (agent, base, client) = node(|_| {}).await;
    let (owned, _) = seed(&agent);
    for key in &owned {
        let (status, body) = get(&client, format!("{base}/gateway/overlay/consistent/get?key={key}"), None).await;
        assert_eq!(status, 403, "consistent/get {key}: {body}");
        assert_eq!(body["error"], "protected_key", "{key}");
    }
    let (status, body) = get(&client, format!("{base}/gateway/overlay/consistent/get?key=orders/42"), None).await;
    assert_eq!(status, 200);
    assert_eq!(body["found"], true, "{body}");
    agent.shutdown().await;
}

/// The topology override's read moved to `GET /gateway/govern`: the groups whose override is engaged — exactly
/// `true`, the value the consensus gate reads — and no other.
#[tokio::test]
async fn govern_lists_the_topology_overrides_in_force() {
    let (agent, base, client) = node(|_| {}).await;
    let (_, body) = get(&client, format!("{base}/gateway/govern"), None).await;
    assert_eq!(body["topology_overrides"], serde_json::json!([]), "{body}");
    let over = format!("{base}/gateway/govern/topology-override");
    for group in ["workers", "auditors"] {
        let r = client.post(&over).json(&serde_json::json!({"group": group, "override": true})).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    // A value other than `true` engages nothing, so it is not reported.
    let _ = agent.kv().set("sys/topology-override/typo".to_string(), Bytes::from_static(b"false"));
    let (status, body) = get(&client, format!("{base}/gateway/govern"), None).await;
    assert_eq!(status, 200);
    assert_eq!(body["topology_overrides"], serde_json::json!(["auditors", "workers"]), "{body}");
    let r = client.post(&over).json(&serde_json::json!({"group": "workers", "override": false})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let (_, body) = get(&client, format!("{base}/gateway/govern"), None).await;
    assert_eq!(body["topology_overrides"], serde_json::json!(["auditors"]), "{body}");
    agent.shutdown().await;
}

/// A schedule's last run moved to `GET /gateway/schedules/last?id=`: the caller's own schedule's entries (an open
/// gateway's caller is `anonymous`), never an application's schedule of the same id or another principal's.
#[tokio::test]
async fn schedules_last_reads_the_callers_own_schedule() {
    let (agent, base, client) = node(|_| {}).await;
    let url = |id: &str| format!("{base}/gateway/schedules/last?id={id}");
    let (status, body) = get(&client, url("close"), None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, serde_json::json!({"schedule": "close", "last": []}));
    let _ = agent.kv().set("sched/close~anonymous/last/10.0.0.5:9000".to_string(), Bytes::from_static(b"7 1000 0"));
    let _ = agent.kv().set("sched/close~anonymous/last/10.0.0.6:9000".to_string(), Bytes::from_static(b"not a run"));
    let _ = agent.kv().set("sched/close/last/10.0.0.5:9000".to_string(), Bytes::from_static(b"99 1000 0"));
    let _ = agent.kv().set("sched/close~oidc%3Aother/last/10.0.0.5:9000".to_string(), Bytes::from_static(b"98 1000 0"));
    let (status, body) = get(&client, url("close"), None).await;
    assert_eq!(status, 200);
    assert_eq!(body, serde_json::json!({"schedule": "close",
        "last": [{"node": "10.0.0.5:9000", "slot": 7, "period_ms": 1000, "phase_ms": 0}]}));
    // An id that cannot name a schedule — a second key segment, another principal's scoped id — reads nothing.
    for bad in ["", "a/b", "close.holder"] {
        let (status, _) = get(&client, url(bad), None).await;
        assert_eq!(status, 400, "id {bad:?}");
    }
    agent.shutdown().await;
}

/// Each door under a token model: a `kv:read`-only token is refused every owned key 403 `protected_key` — not for
/// want of a scope, and `"*"` is refused the same — while it still reads an application key and the listed names;
/// the doors with their own scope refuse it for the scope.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn a_kv_read_token_reads_application_keys_only_at_every_door() {
    let (agent, base, client) = node(|cfg| {
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "ro".into(), scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "cons".into(), scopes: vec!["consensus:read".into()] },
            crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
        ];
    }).await;
    let (owned, application) = seed(&agent);
    for bearer in ["ro", "super"] {
        for key in &owned {
            let (status, body) = get(&client, format!("{base}/gateway/kv?key={key}"), Some(bearer)).await;
            assert_eq!((status, body["error"].as_str()), (403, Some("protected_key")), "{bearer} GET {key}: {body}");
        }
        for h in HIDDEN {
            let (status, body) = get(&client, format!("{base}/gateway/kv/keys?prefix={h}"), Some(bearer)).await;
            assert_eq!((status, body["error"].as_str()), (403, Some("protected_key")), "{bearer} keys {h}: {body}");
        }
        let (status, body) = get(&client, format!("{base}/gateway/kv/keys"), Some(bearer)).await;
        assert_eq!(status, 200);
        let all = names(&body);
        assert!(!all.iter().any(|k| HIDDEN.iter().any(|h| k.starts_with(h))), "{bearer}: {all:?}");
        assert!(all.iter().any(|k| k == "cap/probe/1") && all.iter().any(|k| k == "orders/42"), "{bearer}: {all:?}");
        for key in &application {
            let (status, body) = get(&client, format!("{base}/gateway/kv?key={key}"), Some(bearer)).await;
            assert_eq!((status, &body["found"]), (200, &serde_json::json!(true)), "{bearer} GET {key}");
        }
    }
    // The doors with their own scope: `kv:read` does not open them, and the scope that does is refused an owned key.
    #[cfg(feature = "consensus")]
    {
        let url = |key: &str| format!("{base}/gateway/overlay/consistent/get?key={key}");
        let (status, body) = get(&client, url("grant/probe/1"), Some("ro")).await;
        assert_eq!(status, 403, "{body}");
        assert_ne!(body["error"], "protected_key", "kv:read is not consensus:read");
        for bearer in ["cons", "super"] {
            let (status, body) = get(&client, url("grant/probe/1"), Some(bearer)).await;
            assert_eq!((status, body["error"].as_str()), (403, Some("protected_key")), "{bearer}: {body}");
            let (status, body) = get(&client, url("orders/42"), Some(bearer)).await;
            assert_eq!((status, &body["found"]), (200, &serde_json::json!(true)), "{bearer}");
        }
    }
    for path in ["/gateway/schedules/last?id=close", "/gateway/govern"] {
        let (status, _) = get(&client, format!("{base}{path}"), Some("ro")).await;
        assert_eq!(status, 403, "kv:read on {path}");
        let (status, _) = get(&client, format!("{base}{path}"), Some("super")).await;
        assert_eq!(status, 200, "* on {path}");
    }
    agent.shutdown().await;
}
