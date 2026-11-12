//! The log **read** doors read application streams only (critique plan row CR1's follow-up, 2026-11-12),
//! mirroring the append and compact doors (#549) and the raw KV read doors (#774). Rule 1's enumeration, one test
//! per door: `GET /gateway/overlay/log/scan`, `/log/subscribe` (SSE) and `/log/group/subscribe` (SSE, a consensus
//! claim); the one table both directions classify by; and every door under a token model.

use crate::{GossipAgent, GossipConfig, NodeId};
use bytes::Bytes;
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

/// The rows of `OWNED_LOG_STREAMS`: exactly these. A row added to the table fails the test below until it is
/// listed here, with its read named.
const OWNED: &[&str] = &["cn/", "wiki/", "reason/"];

/// Streams as their owners name them (`mycelium-commitment`, `mandate::restart::proposal_stream`,
/// `mycelium-reason`'s trace substream), each seeded with an entry.
const OWNED_STREAMS: &[&str] = &["cn/req-1/offers", "cn/req-1/closes", "cn/req-1/reports", "cn/req-1/assessments",
                                 "wiki/g/proposals", "reason/run-1/10.0.0.5:9000"];

/// Owned streams nothing was appended to, the bare names and a bare prefix: refused the same.
const OWNED_ABSENT: &[&str] = &["cn/absent/offers", "wiki/absent/proposals", "reason/absent/n", "cn", "wiki", "reason",
                                "cn/", "reason/", "cn//x"];

/// Names that are *not* a component's stream: the store compares bytes, so each of these is its own (application)
/// stream whose keys lie outside `log/cn/`, `log/wiki/` and `log/reason/`. None may be refused, and none may
/// return an owned stream's entries.
const LOOKALIKES: &[&str] = &["CN/req-1/offers", "Cn/req-1/offers", "/cn/req-1/offers", "//cn/req-1/offers",
                              " cn/req-1/offers", "cn%2Freq-1%2Foffers", "c", "cnx/req-1/offers", "reasons/run-1",
                              "wikis", "x/../cn/req-1/offers", "./cn/req-1/offers", ""];

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

/// An entry in every owned stream, as its component would have appended it, and one in an application stream.
fn seed(agent: &GossipAgent) {
    for stream in OWNED_STREAMS {
        agent.kv().append(stream, Bytes::from_static(b"owned"));
    }
    agent.kv().append("events", Bytes::from_static(b"application"));
}

/// A `GET` with the stream percent-encoded as a client library would send it.
fn url(base: &str, door: &str, stream: &str, extra: &str) -> String {
    let encoded: String = stream.bytes().map(|b| match b {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'/' | b':' => (b as char).to_string(),
        other => format!("%{other:02X}"),
    }).collect();
    format!("{base}/gateway/overlay/log/{door}?stream={encoded}{extra}")
}

struct Answer { status: u16, content_type: String, response: reqwest::Response }

async fn open(client: &reqwest::Client, url: String, bearer: Option<&str>) -> Answer {
    let mut r = client.get(url);
    if let Some(b) = bearer {
        r = r.bearer_auth(b);
    }
    let response = r.send().await.unwrap();
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    Answer { status: response.status().as_u16(), content_type, response }
}

async fn json(a: Answer) -> serde_json::Value {
    a.response.json().await.unwrap_or(serde_json::Value::Null)
}

/// The refusal, whole: 403, a JSON body (never an event stream), `protected_stream`, and no entry in it.
async fn assert_refused(a: Answer, what: &str) -> serde_json::Value {
    assert_eq!(a.status, 403, "{what}");
    assert!(a.content_type.starts_with("application/json"), "{what}: a JSON refusal, not {:?}", a.content_type);
    let body = json(a).await;
    assert_eq!(body["error"], "protected_stream", "{what}: {body}");
    assert_eq!(body["ok"], false, "{what}");
    let text = body.to_string();
    assert!(!text.contains("value_b64") && !text.contains("hlc"), "{what}: no entry in a refusal: {text}");
    body
}

/// The first SSE `data:` object of an open stream, or `None` if none arrives in `within`.
async fn first_event(mut a: Answer, within: Duration) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + within;
    let mut text = String::new();
    loop {
        let chunk = tokio::time::timeout_at(deadline, a.response.chunk()).await.ok()?.ok()??;
        text.push_str(&String::from_utf8_lossy(&chunk));
        if let Some(line) = text.lines().find_map(|l| l.strip_prefix("data:"))
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
        {
            return Some(v);
        }
    }
}

/// One table, both directions, the rows pinned: every row is refused on write and on read by the one predicate,
/// its bare name with it; each row names its read; nothing that merely resembles a row is refused either way.
#[test]
fn the_log_doors_classify_by_the_one_table() {
    let rows: Vec<&str> = super::OWNED_LOG_STREAMS.iter().map(|p| p.prefix).collect();
    assert_eq!(rows, OWNED, "the owned streams: a new row is added here, with its read");
    for row in super::OWNED_LOG_STREAMS {
        assert!(row.prefix.ends_with('/') && row.prefix.len() > 1, "{}: a row is a prefix ending in `/`", row.prefix);
        assert!(!row.read_through.is_empty(), "{}: a row says how its stream is read", row.prefix);
        for stream in [format!("{}x", row.prefix), row.prefix.to_string(), row.prefix.trim_end_matches('/').to_string(),
                       format!("{}a/b/c", row.prefix)] {
            assert_eq!(super::owned_log_stream(&stream), Some(row), "{stream}");
            assert!(super::refuse_owned_stream(&stream).is_some(), "{stream} is refused on append and compact");
            assert!(super::refuse_owned_stream_read(&stream).is_some(), "{stream} is refused on scan and subscribe");
        }
    }
    for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
        assert_eq!(super::refuse_owned_stream(stream).is_some(), super::refuse_owned_stream_read(stream).is_some(), "{stream}");
        assert!(super::refuse_owned_stream_read(stream).is_some(), "{stream}");
    }
    for stream in LOOKALIKES.iter().chain(&["events", "orders/42", "audit-log", "s13-observations"]) {
        assert!(super::refuse_owned_stream(stream).is_none(), "{stream:?} is an application stream (write)");
        assert!(super::refuse_owned_stream_read(stream).is_none(), "{stream:?} is an application stream (read)");
    }
    // The one stream with a route of its own is told it; the others are told there is none.
    let reason = super::owned_log_stream("reason/run-1/n").unwrap();
    assert!(reason.read_through.contains("GET /gateway/reason/trace/{run_id} (llm:read)"), "{}", reason.read_through);
    for stream in ["cn/r/offers", "wiki/g/proposals"] {
        let row = super::owned_log_stream(stream).unwrap();
        assert!(row.read_through.contains("no route reads them") && !row.read_through.contains("/gateway/"), "{stream}");
    }
}

/// The raw KV read refusal for a key under `log/` or `clog/` sends an application stream's reader to the scan
/// route, and an owned stream's reader where the scan route now sends it: not to a route that refuses it.
#[tokio::test]
async fn a_raw_kv_read_of_an_owned_streams_key_is_not_sent_to_the_scan_route() {
    let (agent, base, client) = node(|_| {}).await;
    for (key, to_scan) in [("log/events/0000000000000001/n", true), ("clog/events/workers/cursor", true),
                           ("log/cn/req-1/offers/0000000000000001/n", false), ("clog/cn/req-1/offers/g/cursor", false),
                           ("log/reason/run-1/n/0000000000000001/n", false), ("log/wiki/g/proposals/0000000000000001/n", false)] {
        let body = json(open(&client, format!("{base}/gateway/kv?key={key}"), None).await).await;
        assert_eq!(body["error"], "protected_key", "{key}: {body}");
        let message = body["message"].as_str().unwrap_or("");
        assert_eq!(message.contains("/gateway/overlay/log/scan"), to_scan, "{key}: {message}");
    }
    let body = json(open(&client, format!("{base}/gateway/kv?key=log/reason/run-1/n/1/n"), None).await).await;
    assert!(body["message"].as_str().unwrap_or("").contains("/gateway/reason/trace/{run_id}"), "{body}");
    agent.shutdown().await;
}

/// Door 1, `GET /gateway/overlay/log/scan`: an owned stream is 403 `protected_stream`, with entries or without,
/// in the same words; an application stream is read; a name that only resembles an owned stream reads its own
/// (empty) stream, never the owned one's entries.
#[tokio::test]
async fn log_scan_refuses_an_owned_stream_and_reads_an_application_stream() {
    let (agent, base, client) = node(|_| {}).await;
    seed(&agent);
    for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
        assert_refused(open(&client, url(&base, "scan", stream, ""), None).await, &format!("scan {stream}")).await;
        // A window does not open it either.
        assert_refused(open(&client, url(&base, "scan", stream, "&from=0&to=1"), None).await, &format!("scan {stream} window")).await;
    }
    // The same answer whether or not the stream holds anything.
    for (present, absent) in [("cn/req-1/offers", "cn/absent/offers"), ("wiki/g/proposals", "wiki/absent/proposals"),
                              ("reason/run-1/10.0.0.5:9000", "reason/absent/n")] {
        let a = json(open(&client, url(&base, "scan", present, ""), None).await).await;
        let b = json(open(&client, url(&base, "scan", absent, ""), None).await).await;
        assert_eq!(a, b, "{present} and {absent}");
    }
    // A stream sent percent-encoded is the same stream: decoded before it is classified.
    let encoded = format!("{base}/gateway/overlay/log/scan?stream=cn%2Freq-1%2Foffers");
    assert_refused(open(&client, encoded, None).await, "scan cn%2Freq-1%2Foffers").await;
    let encoded = format!("{base}/gateway/overlay/log/scan?stream=%63%6E/req-1/offers");
    assert_refused(open(&client, encoded, None).await, "scan %63%6E/req-1/offers").await;
    // The refusal names the route that reads a trace, and says a commitment's records have none.
    let body = json(open(&client, url(&base, "scan", "reason/run-1/10.0.0.5:9000", ""), None).await).await;
    assert!(body["message"].as_str().unwrap_or("").contains("/gateway/reason/trace/{run_id}"), "{body}");
    let body = json(open(&client, url(&base, "scan", "cn/req-1/offers", ""), None).await).await;
    assert!(body["message"].as_str().unwrap_or("").contains("mycelium-commitment"), "{body}");

    // An application stream reads, as before.
    let a = open(&client, url(&base, "scan", "events", ""), None).await;
    assert_eq!(a.status, 200);
    let body = json(a).await;
    let entries = body.as_array().expect("an array of entries");
    assert_eq!(entries.len(), 1, "{body}");
    assert_eq!(entries[0]["value_b64"], "YXBwbGljYXRpb24=", "{body}");
    // A lookalike is its own stream: answered, and empty — no owned entry reaches it by case, a leading slash, a
    // double-encoded slash, a shorter prefix or a dot segment.
    for stream in LOOKALIKES {
        let a = open(&client, url(&base, "scan", stream, ""), None).await;
        assert_eq!(a.status, 200, "scan {stream:?}");
        assert_eq!(json(a).await, serde_json::json!([]), "scan {stream:?}");
    }
    // The two directions agree at the doors themselves: every stream the scan refuses, append and compact refuse.
    for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
        let r = client.post(format!("{base}/gateway/overlay/log/append")).json(&serde_json::json!({"stream": stream, "value_b64": ""})).send().await.unwrap();
        assert_eq!(r.status(), 403, "append {stream}");
        let r = client.post(format!("{base}/gateway/overlay/log/compact")).json(&serde_json::json!({"stream": stream, "before_hlc": 1})).send().await.unwrap();
        assert_eq!(r.status(), 403, "compact {stream}");
    }
    // And the owned entries are where they were: a refusal reads and writes nothing.
    for stream in OWNED_STREAMS {
        assert_eq!(agent.kv().scan_log(stream, 0, u64::MAX).len(), 1, "{stream}");
    }
    agent.shutdown().await;
}

/// Door 2, `GET /gateway/overlay/log/subscribe` (SSE): an owned stream is refused with a JSON 403 — the stream is
/// never opened, so no event is sent — and an application stream opens and delivers.
#[tokio::test]
async fn log_subscribe_refuses_an_owned_stream_before_any_event() {
    let (agent, base, client) = node(|_| {}).await;
    seed(&agent);
    for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
        assert_refused(open(&client, url(&base, "subscribe", stream, ""), None).await, &format!("subscribe {stream}")).await;
        assert_refused(open(&client, url(&base, "subscribe", stream, "&since=0"), None).await, &format!("subscribe {stream} since")).await;
    }
    let a = json(open(&client, url(&base, "subscribe", "cn/req-1/offers", ""), None).await).await;
    let b = json(open(&client, url(&base, "subscribe", "cn/absent/offers", ""), None).await).await;
    assert_eq!(a, b, "present and absent alike");

    let a = open(&client, url(&base, "subscribe", "events", ""), None).await;
    assert_eq!(a.status, 200);
    assert!(a.content_type.starts_with("text/event-stream"), "{:?}", a.content_type);
    let event = first_event(a, Duration::from_secs(10)).await.expect("the application stream's entry");
    assert_eq!((event["stream"].as_str(), event["value_b64"].as_str()), (Some("events"), Some("YXBwbGljYXRpb24=")), "{event}");
    // A lookalike opens its own stream and has nothing to deliver.
    for stream in ["CN/req-1/offers", "/cn/req-1/offers", "cn%2Freq-1%2Foffers", ""] {
        let a = open(&client, url(&base, "subscribe", stream, ""), None).await;
        assert_eq!(a.status, 200, "subscribe {stream:?}");
        assert!(first_event(a, Duration::from_millis(300)).await.is_none(), "subscribe {stream:?} delivered an entry");
    }
    agent.shutdown().await;
}

/// Door 3, `GET /gateway/overlay/log/group/subscribe` (SSE): refused the same, before the claim is proposed —
/// so a `consensus:read` caller writes no `clog/{owned stream}/…` claim or position — and an application
/// stream's group still takes its claim and delivers.
#[cfg(feature = "consensus")]
#[tokio::test]
async fn log_group_subscribe_refuses_an_owned_stream_before_the_claim() {
    let (agent, base, client) = node(|_| {}).await;
    seed(&agent);
    for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
        assert_refused(open(&client, url(&base, "group/subscribe", stream, "&group=g"), None).await, &format!("group subscribe {stream}")).await;
    }
    // A group name cannot carry the stream: `stream` is what is classified, whatever `group` holds.
    assert_refused(open(&client, url(&base, "group/subscribe", "cn", "&group=req-1/offers"), None).await, "group subscribe cn + group").await;
    let a = json(open(&client, url(&base, "group/subscribe", "cn/req-1/offers", "&group=g"), None).await).await;
    let b = json(open(&client, url(&base, "group/subscribe", "cn/absent/offers", "&group=g"), None).await).await;
    assert_eq!(a, b, "present and absent alike");
    // Nothing was proposed or recorded for any of them: the acquire loop proposes at once, so give it a moment.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let touched: Vec<String> = agent.kv().keys().into_iter()
        .filter(|k| k.starts_with("clog/") || k.contains("clog/cn") || k.contains("clog/wiki") || k.contains("clog/reason"))
        .map(|k| k.to_string())
        .collect();
    assert!(touched.is_empty(), "a refused group subscribe wrote {touched:?}");

    let a = open(&client, url(&base, "group/subscribe", "events", "&group=workers"), None).await;
    assert_eq!(a.status, 200);
    assert!(a.content_type.starts_with("text/event-stream"), "{:?}", a.content_type);
    let event = first_event(a, Duration::from_secs(30)).await.expect("the application stream's entry, once the claim is held");
    assert_eq!(event["value_b64"], "YXBwbGljYXRpb24=", "{event}");
    agent.shutdown().await;
}

/// Every door under a token model: a `consensus:read`-only token and `"*"` are refused an owned stream 403
/// `protected_stream` — not for want of a scope — and still read and subscribe to an application stream; a token
/// without `consensus:read` is refused for the scope, in other words.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn a_consensus_read_token_reads_application_streams_only_at_every_log_door() {
    let (agent, base, client) = node(|cfg| {
        cfg.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "ro".into(), scopes: vec!["kv:read".into()] },
            crate::GatewayToken { token: "cons".into(), scopes: vec!["consensus:read".into()] },
            crate::GatewayToken { token: "super".into(), scopes: vec!["*".into()] },
        ];
    }).await;
    seed(&agent);
    let mut doors = vec![("scan", ""), ("subscribe", "")];
    if cfg!(feature = "consensus") {
        doors.push(("group/subscribe", "&group=g"));
    }
    for (door, extra) in &doors {
        for bearer in ["cons", "super"] {
            for stream in OWNED_STREAMS.iter().chain(OWNED_ABSENT) {
                assert_refused(open(&client, url(&base, door, stream, extra), Some(bearer)).await, &format!("{bearer} {door} {stream}")).await;
            }
            let a = open(&client, url(&base, door, "events", extra), Some(bearer)).await;
            assert_eq!(a.status, 200, "{bearer} {door} events");
            if *door == "scan" {
                assert_eq!(json(a).await.as_array().map(Vec::len), Some(1), "{bearer} scan events");
            } else {
                assert!(a.content_type.starts_with("text/event-stream"), "{bearer} {door}: {:?}", a.content_type);
                if *door == "subscribe" {
                    let event = first_event(a, Duration::from_secs(10)).await.expect("the entry");
                    assert_eq!(event["value_b64"], "YXBwbGljYXRpb24=", "{bearer} {door}");
                }
            }
        }
        // `kv:read` is not `consensus:read`: refused for the scope, owned stream or not, and not as a protected stream.
        for stream in ["cn/req-1/offers", "events"] {
            let a = open(&client, url(&base, door, stream, extra), Some("ro")).await;
            assert_eq!(a.status, 403, "ro {door} {stream}");
            assert_ne!(json(a).await["error"], "protected_stream", "ro {door} {stream}: a scope refusal");
        }
        // No token at all is not let in by the refusal either.
        let a = open(&client, url(&base, door, "cn/req-1/offers", extra), None).await;
        assert_eq!(a.status, 401, "no token on {door}");
    }
    // The raw KV routes do not hand the same entries over under `kv:read` (#774), nor under `"*"`.
    let key = agent.kv().scan_prefix("log/cn/req-1/offers/").into_iter().next().expect("the seeded offer's key").0;
    for bearer in ["ro", "super"] {
        let a = open(&client, format!("{base}/gateway/kv?key={key}"), Some(bearer)).await;
        assert_eq!(a.status, 403, "{bearer} GET {key}");
        assert_eq!(json(a).await["error"], "protected_key", "{bearer}");
    }
    agent.shutdown().await;
}
