//! The routes that take a consensus **slot name** from the caller read and propose on application slots only
//! (the maintainer's decision of 2026-11-12, the follow-up to the kv:read and log-read decisions; critique plan
//! row CR1). Rule 1's enumeration, one test per door: `GET /consensus/{slot}` (read) and
//! `POST /gateway/consensus/cross_group_propose` (the one consensus route whose body names the slot); the one table both
//! classify by; every door under a token model. The other proposing routes build the slot from a fixed family —
//! `consistent/{key}` (the key checked by `refuse_protected_key`), `lock/{name}`, `leader/{group}`,
//! `clog/{stream}/{group}/claim` (the stream checked by `refuse_owned_stream_read`), `electorate/{group}/…`
//! behind `govern:write` — and take no slot name. Outside these: the raw signal routes take a caller-chosen kind,
//! and refuse the consensus kinds on every one of their doors (`substrate_kind_tests.rs`, critique plan CR25).

use crate::{GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// The rows of `SLOT_FAMILIES` whose read `GET /consensus/{slot}` refuses: exactly these.
const OWNED: &[&str] = &["cn/", "clog/", "electorate/", "ae-seat/", "capauthz/"];
/// The rows it serves, and the cross-group route refuses with the others: exactly these.
const ROUTED: &[&str] = &["lock/", "leader/", "consistent/"];

/// Component-owned slots as their owners name them, each seeded with a committed value.
const OWNED_SLOTS: &[&str] = &["cn/req-1/award", "clog/events/workers/claim", "electorate/council/1",
                               "electorate/council/retire", "electorate/council/carried/old",
                               "ae-seat/0123456789abcdef0123456789abcdef/fedcba9876543210fedcba9876543210/0",
                               "capauthz/demo/echo"];
/// Owned slots nothing was decided on, the bare names and a bare prefix: refused the same.
const OWNED_ABSENT: &[&str] = &["cn/absent/award", "cn/absent/anything", "clog/absent/g/claim", "electorate/absent/1",
                                "ae-seat/absent/absent/0", "capauthz/absent/absent", "cn", "clog", "electorate", "ae-seat",
                                "capauthz", "cn/", "capauthz/", "cn//x"];
/// Names that are *not* a component's slot: a slot is compared as bytes, so each is its own (application) slot
/// whose keys are not the component's. None may be refused, and none may answer an owned slot's value.
const LOOKALIKES: &[&str] = &["CN/req-1/award", "Cn/req-1/award", "/cn/req-1/award", " cn/req-1/award", "cnx/req-1/award",
                              "c", "electorates/council/1", "ae-seats/x", "capauthzx/demo/echo", "x/cn/req-1/award",
                              "./cn/req-1/award", "cn%2Freq-1%2Faward"];

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

/// What the slot route reads for a slot with no decision record: the committed key, as a node older than 2.32.0
/// wrote it and as anti-entropy delivers it. Written directly so every family is seeded the same way, an
/// electorate's included (whose proposals the electorate door checks).
fn seed_slot(agent: &GossipAgent, slot: &str, value: &'static [u8]) {
    let _ = agent.kv().set(format!("consensus/committed/{slot}"), Bytes::from_static(value));
}

fn seed(agent: &GossipAgent) {
    for slot in OWNED_SLOTS {
        seed_slot(agent, slot, b"owned-secret");
    }
    seed_slot(agent, "lock/x", b"127.0.0.1:1:00000000000000aa");
    seed_slot(agent, "leader/g", b"127.0.0.1:1");
    seed_slot(agent, "consistent/orders/1", b"order");
    seed_slot(agent, "consistent/grant/x", b"owned-secret");
    seed_slot(agent, "donation/cross-bloc-42", b"application");
}

/// The slot percent-encoded as a client library would send a path segment (`/` kept: the route captures the tail).
fn encode(slot: &str, slash: bool) -> String {
    slot.bytes().map(|b| match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b':' => (b as char).to_string(),
        b'/' if !slash => "/".to_string(),
        other => format!("%{other:02X}"),
    }).collect()
}

async fn get(client: &reqwest::Client, url: String, bearer: Option<&str>) -> (u16, serde_json::Value) {
    let mut r = client.get(url);
    if let Some(b) = bearer {
        r = r.bearer_auth(b);
    }
    let r = r.send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(serde_json::Value::Null))
}

async fn propose(client: &reqwest::Client, base: &str, slot: &str, group: &str, bearer: Option<&str>) -> (u16, serde_json::Value) {
    let body = serde_json::json!({ "slot": slot, "value_b64": "Zm9yZ2Vk", // "forged"
                                   "groups": [{ "group": group, "quorum": 0.5, "veto": false }] });
    let mut r = client.post(format!("{base}/gateway/consensus/cross_group_propose")).json(&body);
    if let Some(b) = bearer {
        r = r.bearer_auth(b);
    }
    let r = r.send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(serde_json::Value::Null))
}

/// The refusal, whole: 403, `protected_slot`, and nothing of the slot in it.
fn assert_refused(answer: &(u16, serde_json::Value), what: &str) {
    let (status, body) = answer;
    assert_eq!((*status, body["error"].as_str(), &body["ok"]), (403, Some("protected_slot"), &serde_json::json!(false)), "{what}: {body}");
    assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()), "{what}: a refusal names the door: {body}");
    for field in ["committed", "ballot", "lease_ms", "lease_expired", "slot", "persisted"] {
        assert!(body.get(field).is_none(), "{what}: `{field}` in a refusal: {body}");
    }
}

/// This node in group `g` with its acceptor running: a fleet of one that decides a cross-group proposal on `g`.
async fn voting_in_g(agent: &GossipAgent) -> crate::consensus::ConsensusListenerHandle {
    let listener = agent.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    agent.mesh().join_group("g");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while agent.mesh().group_members("g").is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the node never saw itself in g");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    listener
}

/// Every key in the store that names `slot`: the slot's own records and this node's acceptor memory.
fn slot_keys(agent: &GossipAgent, slot: &str) -> Vec<String> {
    let escaped = crate::consensus_life::escape_slot(slot);
    let mut keys: Vec<String> = ["consensus/", "sys/consensus-accepted/"].iter()
        .flat_map(|p| agent.kv().scan_prefix(p))
        .map(|(k, _)| k.to_string())
        .filter(|k| k.ends_with(&format!("/{slot}")) || k.contains(&format!("/{escaped}/")))
        .collect();
    keys.sort();
    keys
}

/// One table, both directions, the rows pinned: every row is refused on proposal; the owned rows are refused on
/// read and the exclusive verbs' are served; each bare name with its row; nothing that resembles a row is
/// refused either way.
#[test]
fn the_slot_routes_classify_by_the_one_table() {
    let rows: Vec<&str> = super::SLOT_FAMILIES.iter().map(|f| f.prefix).collect();
    let pinned: Vec<&str> = OWNED.iter().chain(ROUTED).copied().collect();
    assert_eq!(rows, pinned, "the slot families: a new row is added to OWNED or ROUTED here, with its doors");
    for family in super::SLOT_FAMILIES {
        assert!(family.prefix.ends_with('/') && family.prefix.len() > 1, "{}: a row is a prefix ending in `/`", family.prefix);
        assert!(!family.proposed_through.is_empty(), "{}: a row says where the family is proposed", family.prefix);
        assert_eq!(family.read_through.is_some(), OWNED.contains(&family.prefix), "{}: owned rows name their read; the others are served", family.prefix);
        for slot in [format!("{}x", family.prefix), family.prefix.to_string(), family.prefix.trim_end_matches('/').to_string(),
                     format!("{}a/b/c", family.prefix)] {
            assert_eq!(super::slot_family(&slot), Some(family), "{slot}");
            assert!(super::refuse_owned_slot_proposal(&slot).is_some(), "{slot} is refused on the cross-group route");
            assert_eq!(super::refuse_owned_slot_read(&slot).is_some(), OWNED.contains(&family.prefix), "{slot} on the slot read route");
        }
    }
    for slot in OWNED_SLOTS.iter().chain(OWNED_ABSENT) {
        assert!(super::refuse_owned_slot_read(slot).is_some() && super::refuse_owned_slot_proposal(slot).is_some(), "{slot}");
    }
    for slot in LOOKALIKES.iter().chain(&["donation/cross-bloc-42", "epoch/leader", "work/x", "fed/alpha", "agent/x", "tools/y", "locks/x", "leaders", ""]) {
        assert!(super::refuse_owned_slot_read(slot).is_none(), "{slot:?} is an application slot (read)");
        assert!(super::refuse_owned_slot_proposal(slot).is_none(), "{slot:?} is an application slot (proposal)");
    }
    // A `consistent/{key}` slot is read exactly when the consistent-get route reads `{key}`.
    for key in ["orders/1", "ckpt/t/ns/1", "agent/n/provision/item/error", "x"] {
        assert!(super::refuse_owned_slot_read(&format!("consistent/{key}")).is_none(), "consistent/{key}");
        assert!(super::refuse_protected_key_read(key).is_none(), "{key}");
    }
    for row in super::OWNED_KV_PREFIXES {
        let key = format!("{}x", row.prefix);
        assert!(super::refuse_owned_slot_read(&format!("consistent/{key}")).is_some(), "consistent/{key}: an owned key's slot");
    }
    // The one family with a route of its own is told it; the others are told there is none.
    let electorate = super::slot_family("electorate/council/1").unwrap();
    assert!(electorate.read_through.unwrap().contains("GET /gateway/govern (govern:read)"), "{:?}", electorate.read_through);
    assert!(electorate.proposed_through.contains("POST /gateway/govern/electorate (govern:write)"));
    for slot in ["cn/r/award", "ae-seat/a/b/0", "capauthz/ns/name", "clog/s/g/claim"] {
        let read = super::slot_family(slot).unwrap().read_through.unwrap();
        assert!(read.contains("no route reads") && !read.contains("/consensus/"), "{slot}: {read}");
    }
    assert!(super::slot_family("cn/r/award").unwrap().read_through.unwrap().contains("mycelium-commitment"));
    for (slot, route) in [("lock/x", "/gateway/overlay/lock/acquire"), ("leader/g", "/gateway/overlay/elect"),
                          ("consistent/k", "/gateway/overlay/consistent/set")] {
        assert!(super::slot_family(slot).unwrap().proposed_through.contains(route), "{slot}");
    }
}

/// A slot family is classified, not open by default — as far as this reaches (the table's doc says what it does
/// not): every family the substrate's own lists name has a row, and so has every slot name the namespace gate's
/// allow-list records.
#[test]
fn every_slot_family_is_classified_for_the_slot_routes() {
    use crate::agent::electorate::{ELECTORATE_SLOT_PREFIX, FLEET_EXCLUSIVE_PREFIXES, FLEET_EXCLUSIVE_SUFFIXED, SAFETY_SLOT_FAMILIES};
    let row = |prefix: &str| super::SLOT_FAMILIES.iter().any(|f| f.prefix == prefix);
    let mut known: Vec<(String, &str)> = Vec::new();
    known.extend(SAFETY_SLOT_FAMILIES.iter().map(|f| (f.to_string(), "SAFETY_SLOT_FAMILIES")));
    known.extend(FLEET_EXCLUSIVE_PREFIXES.iter().map(|f| (f.to_string(), "FLEET_EXCLUSIVE_PREFIXES")));
    known.extend(FLEET_EXCLUSIVE_SUFFIXED.iter().map(|(head, _)| (head.to_string(), "FLEET_EXCLUSIVE_SUFFIXED")));
    known.push((ELECTORATE_SLOT_PREFIX.to_string(), "ELECTORATE_SLOT_PREFIX"));
    #[cfg(feature = "tls")]
    known.push((crate::agent::fleet_history::SEAT_SLOT_PREFIX.to_string(), "SEAT_SLOT_PREFIX"));
    // The namespace gate's allow-list has a section of slot names: a head that is a slot, not a KV key.
    let allow = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/kv-namespaces-nonkeys.txt")).unwrap();
    let section: Vec<&str> = allow.lines()
        .skip_while(|l| !l.starts_with("# ── consensus slot names"))
        .skip(1)
        .take_while(|l| !l.starts_with("# ──"))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert!(section.len() >= 5, "the slot-name section of scripts/kv-namespaces-nonkeys.txt was not found: {section:?}");
    known.extend(section.iter().map(|f| (f.to_string(), "scripts/kv-namespaces-nonkeys.txt § consensus slot names")));
    for (family, from) in &known {
        assert!(family.ends_with('/'), "{family} ({from}) is not a family prefix");
        assert!(row(family), "slot family {family} ({from}) has no row in SLOT_FAMILIES: GET /consensus/{{slot}} would read it and \
                              POST /gateway/consensus/cross_group_propose would propose on it");
    }
    // And every slot the substrate treats as exclusive is refused on the proposing route.
    for slot in ["lock/a", "consistent/k", "capauthz/ns/n", "cn/r/award", "clog/s/g/claim", "leader/g", "electorate/g/1"] {
        assert!(crate::agent::electorate::is_fleet_exclusive_slot(slot) || crate::agent::electorate::SAFETY_SLOT_FAMILIES.iter().any(|f| slot.starts_with(f)), "{slot}");
        assert!(super::refuse_owned_slot_proposal(slot).is_some(), "{slot}");
    }
}

/// Door 1, `GET /consensus/{slot}`: a component-owned slot is 403 `protected_slot`, decided or not, in the same
/// words, typed as a path or percent-encoded; a lock's, a leader's, an application key's consistent slot and an
/// application slot are read as before; a name that only resembles an owned slot reads its own (empty) slot.
#[tokio::test]
async fn the_slot_route_refuses_an_owned_slot_and_reads_the_rest() {
    let (agent, base, client) = node(|_| {}).await;
    seed(&agent);
    for slot in OWNED_SLOTS.iter().chain(OWNED_ABSENT) {
        for url in [format!("{base}/consensus/{}", encode(slot, false)), format!("{base}/consensus/{}", encode(slot, true))] {
            assert_refused(&get(&client, url.clone(), None).await, &url);
        }
    }
    // The same answer whether or not anything was decided there.
    for (present, absent) in [("cn/req-1/award", "cn/absent/award"), ("clog/events/workers/claim", "clog/absent/g/claim"),
                              ("electorate/council/1", "electorate/absent/1"), ("capauthz/demo/echo", "capauthz/absent/absent"),
                              ("ae-seat/0123456789abcdef0123456789abcdef/fedcba9876543210fedcba9876543210/0", "ae-seat/absent/absent/0")] {
        let a = get(&client, format!("{base}/consensus/{present}"), None).await;
        let b = get(&client, format!("{base}/consensus/{absent}"), None).await;
        assert_eq!(a, b, "{present} and {absent}");
    }
    // The refusal names the route that reads an electorate, and says an award has none.
    let (_, body) = get(&client, format!("{base}/consensus/electorate/council/1"), None).await;
    assert!(body["message"].as_str().unwrap_or("").contains("GET /gateway/govern (govern:read)"), "{body}");
    let (_, body) = get(&client, format!("{base}/consensus/cn/req-1/award"), None).await;
    assert!(body["message"].as_str().unwrap_or("").contains("mycelium-commitment"), "{body}");
    // An owned key's consistent slot is refused as the consistent-get route refuses the key, in that key's words.
    let answer = get(&client, format!("{base}/consensus/consistent/grant/x"), None).await;
    assert_refused(&answer, "consistent/grant/x");
    assert!(answer.1["message"].as_str().unwrap_or("").contains("/gateway/grants/"), "{}", answer.1);
    assert_eq!(answer, get(&client, format!("{base}/consensus/consistent/grant/absent"), None).await, "present and absent alike");

    // Served, as before: a lock, a leader, an application key's consistent slot, an application slot.
    for (slot, value) in [("lock/x", "MTI3LjAuMC4xOjE6MDAwMDAwMDAwMDAwMDBhYQ=="), ("leader/g", "MTI3LjAuMC4xOjE="),
                          ("consistent/orders/1", "b3JkZXI="), ("donation/cross-bloc-42", "YXBwbGljYXRpb24=")] {
        for url in [format!("{base}/consensus/{slot}"), format!("{base}/consensus/{}", encode(slot, true))] {
            let (status, body) = get(&client, url.clone(), None).await;
            assert_eq!((status, body["slot"].as_str(), body["committed"].as_str()), (200, Some(slot), Some(value)), "{url}: {body}");
        }
    }
    // An undecided application slot answers, with nothing.
    let (status, body) = get(&client, format!("{base}/consensus/some-slot"), None).await;
    assert_eq!((status, &body["committed"], &body["ballot"]), (200, &serde_json::Value::Null, &serde_json::json!(0)), "{body}");
    // A lookalike is its own slot: answered, and empty — no owned value reaches it by case, a leading slash, a
    // space, a longer head, a dot segment or a doubly-encoded slash.
    // (A leading `./` is left out here: an HTTP client resolves the dot segment before it sends, so the node is
    // asked for `cn/req-1/award` itself — and refuses it, as above.)
    for slot in LOOKALIKES.iter().filter(|s| !s.starts_with("./")) {
        let url = format!("{base}/consensus/{}", encode(slot, false));
        let (status, body) = get(&client, url.clone(), None).await;
        assert_eq!(status, 200, "{url}: {body}");
        assert_eq!(body["committed"], serde_json::Value::Null, "{url}: {body}");
    }
    agent.shutdown().await;
}

/// Door 2, `POST /gateway/consensus/cross_group_propose`: a proposal on any family of the table is 403
/// `protected_slot` before anything is proposed — no decision, no ballot, no acceptor record for the slot —
/// and an application slot is decided as before.
#[tokio::test]
async fn the_cross_group_route_refuses_a_proposal_on_an_owned_slot() {
    let (agent, base, client) = node(|_| {}).await;
    let _listener = voting_in_g(&agent).await;
    // The control first: the route decides an application slot on this fleet of one, so a refusal below is the
    // guard's and not a fleet that cannot decide.
    let (status, body) = propose(&client, &base, "donation/cross-bloc-42", "g", None).await;
    assert_eq!((status, &body["ok"]), (200, &serde_json::json!(true)), "an application slot is decided: {body}");
    assert_eq!(agent.consensus().consensus_get("donation/cross-bloc-42").as_deref(), Some(b"forged".as_slice()));
    assert!(!slot_keys(&agent, "donation/cross-bloc-42").is_empty(), "the control's keys are what a proposal leaves");

    let targets = ["cn/req-1/award", "cn/req-1/anything", "capauthz/demo/echo",
                   "ae-seat/0123456789abcdef0123456789abcdef/fedcba9876543210fedcba9876543210/0",
                   "electorate/g/1", "electorate/g/retire", "electorate/g/carried/old", "electorate/g/drop/old",
                   "clog/events/workers/claim", "lock/x", "leader/g", "consistent/orders/1", "consistent/grant/x",
                   "cn", "capauthz", "ae-seat", "electorate", "clog", "lock", "leader", "consistent"];
    // Every slot is tried before anything is asserted, so a failure names each family the route let through and
    // what it did there (this is how the hole was shown on the unguarded route).
    let mut admitted = Vec::new();
    for slot in targets {
        let (status, body) = propose(&client, &base, slot, "g", None).await;
        let decided = agent.consensus().consensus_get(slot).map(|v| String::from_utf8_lossy(&v).into_owned());
        let keys = slot_keys(&agent, slot);
        if (status, body["error"].as_str()) != (403, Some("protected_slot")) || decided.is_some() || !keys.is_empty() {
            admitted.push(format!("{slot}: {status} {body}; decided {decided:?}; {} key(s) written", keys.len()));
        }
    }
    assert!(admitted.is_empty(), "the cross-group route proposed on {} of {} owned slots:\n  {}", admitted.len(), targets.len(), admitted.join("\n  "));
    for slot in targets {
        assert_refused(&propose(&client, &base, slot, "g", None).await, &format!("propose on {slot}"));
    }
    // Each is told where the family is proposed.
    for (slot, door) in [("lock/x", "/gateway/overlay/lock/acquire"), ("leader/g", "/gateway/overlay/elect"),
                         ("consistent/orders/1", "/gateway/overlay/consistent/set"), ("electorate/g/1", "/gateway/govern/electorate"),
                         ("cn/req-1/award", "mycelium-commitment"), ("capauthz/demo/echo", "set_capability_authz_via_consensus")] {
        let (_, body) = propose(&client, &base, slot, "g", None).await;
        assert!(body["message"].as_str().unwrap_or("").contains(door), "{slot}: {body}");
    }
    // The body's slot is a JSON string, compared as bytes: a lookalike is an application slot and is decided.
    for slot in ["CN/req-1/award", "cnx/req-1/award", "/cn/req-1/award", "locks/x"] {
        let (status, body) = propose(&client, &base, slot, "g", None).await;
        assert_eq!((status, &body["ok"]), (200, &serde_json::json!(true)), "{slot}: {body}");
        assert_eq!(agent.consensus().consensus_get("cn/req-1/award"), None, "{slot} did not decide the award");
    }
    agent.shutdown().await;
}

/// The exclusive verbs' own routes still propose in their families: the guard is on the route that takes a slot
/// name, not on the families.
#[tokio::test]
async fn the_routes_that_own_a_family_still_propose_in_it() {
    let (agent, base, client) = node(|_| {}).await;
    let r = client.post(format!("{base}/gateway/overlay/lock/acquire")).json(&serde_json::json!({ "name": "x", "ttl_secs": 30 }))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = client.post(format!("{base}/gateway/overlay/consistent/set")).json(&serde_json::json!({ "key": "orders/1", "value_b64": "b3JkZXI=" }))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    // And the slot route reads what they decided: a lock's holder, the consistent value.
    let (status, body) = get(&client, format!("{base}/consensus/lock/x"), None).await;
    assert_eq!(status, 200);
    assert!(body["committed"].as_str().is_some() && body["lease_ms"].as_u64().is_some(), "a held lock reads held: {body}");
    let (status, body) = get(&client, format!("{base}/consensus/consistent/orders/1"), None).await;
    assert_eq!((status, body["committed"].as_str()), (200, Some("b3JkZXI=")), "{body}");
    agent.shutdown().await;
}

/// The raw KV read refusal for a `consensus/` key no longer sends every reader to the slot route: it says which
/// slots that route reads.
#[tokio::test]
async fn a_raw_kv_read_of_a_consensus_key_is_told_which_slots_the_slot_route_reads() {
    let (agent, base, client) = node(|_| {}).await;
    let (status, body) = get(&client, format!("{base}/gateway/kv?key=consensus/committed/cn/req-1/award"), None).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("protected_key")), "{body}");
    let message = body["message"].as_str().unwrap_or("");
    assert!(message.contains("GET /consensus/{slot}") && message.contains("a component's slot") && message.contains("/gateway/govern"), "{message}");
    agent.shutdown().await;
}

/// Both doors under a token model: the scope a door asks for does not open an owned slot, and neither does `*`;
/// a token without the door's scope is refused for the scope, not told anything about the slot.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn no_scope_opens_an_owned_slot_at_either_door() {
    let (agent, base, client) = node(|cfg| {
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "cr".into(), scopes: vec!["consensus:read".into()] },
            crate::GatewayToken { token: "cw".into(), scopes: vec!["consensus:write".into()] },
            crate::GatewayToken { token: "kv".into(), scopes: vec!["kv:read".into(), "kv:write".into()] },
            crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
        ];
    }).await;
    seed(&agent);
    let _listener = voting_in_g(&agent).await;
    for bearer in ["cr", "super"] {
        for slot in OWNED_SLOTS.iter().chain(OWNED_ABSENT).chain(&["consistent/grant/x"]) {
            for url in [format!("{base}/consensus/{}", encode(slot, false)), format!("{base}/consensus/{}", encode(slot, true))] {
                assert_refused(&get(&client, url.clone(), Some(bearer)).await, &format!("{bearer} {url}"));
            }
        }
        for slot in ["lock/x", "leader/g", "consistent/orders/1", "donation/cross-bloc-42"] {
            let (status, body) = get(&client, format!("{base}/consensus/{slot}"), Some(bearer)).await;
            assert_eq!((status, body["committed"].is_string()), (200, true), "{bearer} {slot}: {body}");
        }
    }
    let (status, body) = propose(&client, &base, "donation/with-a-token", "g", Some("cw")).await;
    assert_eq!((status, &body["ok"]), (200, &serde_json::json!(true)), "consensus:write decides an application slot: {body}");
    for bearer in ["cw", "super"] {
        for slot in ["cn/req-2/award", "capauthz/demo/other", "ae-seat/a/b/0", "electorate/g/1", "clog/events/other/claim",
                     "lock/y", "leader/g2", "consistent/orders/2"] {
            assert_refused(&propose(&client, &base, slot, "g", Some(bearer)).await, &format!("{bearer} propose {slot}"));
            assert_eq!(slot_keys(&agent, slot), Vec::<String>::new(), "{bearer} {slot}: nothing was written");
        }
    }
    // The wrong scope is refused for the scope: nothing of the slot guard is reached.
    for bearer in ["cw", "kv"] {
        let (status, body) = get(&client, format!("{base}/consensus/cn/req-1/award"), Some(bearer)).await;
        assert_eq!(status, 403, "{body}");
        assert_ne!(body["error"], "protected_slot", "{bearer}: the scope is checked first: {body}");
    }
    for bearer in ["cr", "kv"] {
        let (status, body) = propose(&client, &base, "cn/req-2/award", "g", Some(bearer)).await;
        assert_eq!(status, 403, "{body}");
        assert_ne!(body["error"], "protected_slot", "{bearer}: the scope is checked first: {body}");
    }
    agent.shutdown().await;
}
