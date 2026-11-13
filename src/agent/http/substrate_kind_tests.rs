//! The gateway's doors refuse **the substrate's own signal kinds** (2026-11-13; critique plan row CR25): the four
//! consensus kinds, the node-to-node RPC kinds under `sys.` and the bulk transport's ticket (`invoke.bulk`), for
//! every token, on observe, on serve and on send; the RPC reply kinds on send; and a hosted capability's
//! `cap.invoke/…` on observe and on send — its door is `/mcp`.
//!
//! Rule 1's enumeration — every door that takes a kind from its caller, one row each in [`every_door`]:
//! `GET /signals/{kind}`, `GET /gateway/signal/sse/{kind}`, `GET /gateway/rpc/serve/{kind}`,
//! `GET /gateway/mailbox/{kind}`, and the six raw routes (`rpc/call`, `scatter`, `signal/emit`, `mailbox/deliver`,
//! `shard/emit`, `overlay/emit_reliable`). `/mcp`, `/a2a` and `/gateway/llm/*` fix their kind themselves.
//!
//! The five proofs each failed on the unfixed code and say what they showed there:
//! [`a_tap_never_carries_a_peers_proposed_value`], [`an_emitted_commit_writes_no_decision_record`],
//! [`a_tapped_nonce_forges_no_rpc_reply`], [`a_hosted_capability_is_not_called_around_mcp`] and
//! [`a_tapped_bulk_ticket_fetches_no_payload`]. [`every_kind_constant_is_classified_for_the_gateways_doors`] is
//! what makes a new kind constant fail until someone decides which side it is on.

use crate::agent::{is_substrate_kind, HOSTED_CAPABILITY_KIND_PREFIX, SUBSTRATE_KINDS, SUBSTRATE_KIND_PREFIXES};
use crate::{GossipAgent, GossipConfig, NodeId};
use base64::Engine as _;
use bytes::Bytes;
use std::{sync::Arc, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

fn b64(bytes: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(bytes) }

/// Kinds under the two prefixes that no list names: refused before anyone lists them.
const UNLISTED: &[&str] = &["consensus.future", "consensus.", "sys.anything", "sys.", "sys.explain.v2"];

/// The listed kinds outside the two prefixes: refused by name alone.
const BY_NAME: &[&str] = &["invoke.bulk"];

/// Kinds that are *not* the substrate's: the handler table compares bytes, so each of these is its own
/// (application) kind no substrate receiver listens on. None may be refused.
const LOOKALIKES: &[&str] = &["consensus", "Consensus.propose", "CONSENSUS.COMMIT", "consensusx.propose", "xconsensus.propose",
                              "sys", "Sys.explain", "system.explain", "sysx.explain", "repair.request", "rpc.results",
                              "cap.invoke", "cap.invokes/text/echo", "invoke.bulks", "Invoke.bulk", "invoke"];

const REPLY_KINDS: &[&str] = &[crate::signal::signal_kind::RPC_RESULT, crate::signal::signal_kind::BULK_RESULT];

/// The legacy token, which holds `*`: what is refused for it is refused for every token.
const TOKEN: &str = "secret";

/// A scoped token holding every scope the ten doors ask (`mesh:read`, `mesh:write`, `mesh:serve`, and
/// `consensus:write` for `overlay/emit_reliable`) and nothing else — where the build has scoped tokens.
#[cfg(feature = "compliance")]
const SCOPED: &str = "every-kind-door";

/// The tokens every door loop runs under: the one that holds `*`, and the scoped one where the build has it. A
/// refusal must not depend on which, and an ordinary kind must pass under both.
fn tokens() -> Vec<&'static str> {
    #[cfg(feature = "compliance")]
    { vec![TOKEN, SCOPED] }
    #[cfg(not(feature = "compliance"))]
    { vec![TOKEN] }
}

async fn node(cfg_edit: impl FnOnce(&mut GossipConfig)) -> (Arc<GossipAgent>, String, reqwest::Client) {
    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.http_addr = "127.0.0.1".to_string();
    cfg.health_check_max_jitter_ms = 50;
    cfg.gateway_auth_token = Some(TOKEN.into());
    #[cfg(feature = "compliance")]
    cfg.gateway_scoped_tokens.push(crate::GatewayToken {
        token: SCOPED.into(),
        scopes: ["mesh:read", "mesh:write", "mesh:serve", "consensus:write"].iter().map(|s| s.to_string()).collect(),
    });
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

/// Two nodes that know each other; the first has the gateway.
async fn pair(cfg_edit: impl FnOnce(&mut GossipConfig)) -> (Arc<GossipAgent>, Arc<GossipAgent>, String, reqwest::Client) {
    let peer_port = alloc_port();
    let peer_id = NodeId::new("127.0.0.1", peer_port).unwrap();
    let seed = peer_id.clone();
    let (gw, base, client) = node(move |c| { c.bootstrap_peers = vec![seed]; cfg_edit(c); }).await;
    let mut cfg = GossipConfig::default();
    cfg.bind_port = peer_port;
    cfg.bootstrap_peers = vec![gw.node_id().clone()];
    cfg.health_check_max_jitter_ms = 50;
    // The peer serves HTTP too: a bulk caller stages its payload on its own gateway.
    cfg.http_port = Some(alloc_port());
    cfg.http_addr = "127.0.0.1".to_string();
    let peer = Arc::new(GossipAgent::new(peer_id, cfg));
    peer.start().await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while gw.peers().is_empty() || peer.peers().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the two nodes did not meet");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    (gw, peer, base, client)
}

/// A kind as a client library puts it in a path.
fn seg(kind: &str) -> String {
    kind.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        other => format!("%{other:02X}"),
    }).collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Door { Observe, Serve, Mailbox, Send }

/// Every door that takes a kind from its caller, as a request for `kind` under `token`.
fn every_door(client: &reqwest::Client, base: &str, target: &str, kind: &str, token: &str) -> Vec<(Door, String, reqwest::RequestBuilder)> {
    let payload = b64(b"x");
    let get = |door: Door, path: String| (door, format!("{path} [{token}]"), client.get(format!("{base}{path}")).bearer_auth(token));
    let post = |route: &str, body: serde_json::Value| {
        (Door::Send, format!("/gateway/{route} [{token}]"), client.post(format!("{base}/gateway/{route}")).bearer_auth(token).json(&body))
    };
    vec![
        get(Door::Observe, format!("/signals/{}", seg(kind))),
        get(Door::Observe, format!("/gateway/signal/sse/{}", seg(kind))),
        get(Door::Serve, format!("/gateway/rpc/serve/{}", seg(kind))),
        get(Door::Mailbox, format!("/gateway/mailbox/{}", seg(kind))),
        post("rpc/call", serde_json::json!({"target": target, "method": kind, "payload_b64": payload, "timeout_secs": 1})),
        post("scatter", serde_json::json!({"targets": [target], "method": kind, "payload_b64": payload, "timeout_secs": 1})),
        post("signal/emit", serde_json::json!({"kind": kind, "scope": format!("node:{target}"), "payload_b64": payload})),
        post("mailbox/deliver", serde_json::json!({"target": target, "kind": kind, "payload_b64": payload})),
        post("shard/emit", serde_json::json!({"kind": kind, "ns": "x", "name": "y", "shard_key": "k", "payload_b64": payload})),
        post("overlay/emit_reliable", serde_json::json!({"target": target, "kind": kind, "payload_b64": payload, "timeout_secs": 1})),
    ]
}

/// `None` when the door refused `kind` as it must — `403 protected_kind`, naming the kind — or what it answered.
async fn not_refused(path: &str, kind: &str, req: reqwest::RequestBuilder) -> Option<String> {
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    if status != 403 {
        return Some(format!("{path} answered {status} for `{kind}`"));
    }
    let v: serde_json::Value = r.json().await.unwrap_or_default();
    (v["error"] != "protected_kind" || v["kind"] != kind).then(|| format!("{path} refused `{kind}` as {v}"))
}

/// Every door of `doors`, asked together; the ones that did not refuse.
async fn doors_not_refusing(kind: &str, doors: Vec<(Door, String, reqwest::RequestBuilder)>) -> Vec<String> {
    futures_util::future::join_all(doors.into_iter().map(|(_, path, req)| async move { not_refused(&path, kind, req).await }))
        .await.into_iter().flatten().collect()
}

/// **The loop over the list × every door.** A listed kind, and an unlisted one under either prefix, is refused
/// `403 protected_kind` at all ten doors, for the token that holds `*`. Seen failing first with the guards as they
/// were: 110 of the 110 doors did not refuse — the two signal streams and the mailbox stream answered 200 for all
/// eleven kinds; the serve stream 200 for eight and `served_in_process` for `consensus.propose`, `consensus.commit`
/// and `sys.replica-sync.v1`; `signal/emit`, `mailbox/deliver` and `overlay/emit_reliable` 200; `rpc/call` and
/// `scatter` 200 or a 504; `shard/emit` 404 (no provider), having passed its kind check. With `invoke.bulk` off
/// the list (the plant for [`a_tapped_bulk_ticket_fetches_no_payload`]) its doors are listed the same way. Run
/// under the token that holds `*` and under a scoped one (`compliance`): the refusal is the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_door_refuses_a_substrate_kind() {
    let (agent, base, client) = node(|_| {}).await;
    #[cfg(feature = "consensus")]
    let _listener = agent.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    let target = agent.node_id().to_string();
    let mut open = Vec::new();
    for token in tokens() {
        for kind in SUBSTRATE_KINDS.iter().chain(UNLISTED) {
            assert!(is_substrate_kind(kind), "{kind}");
            let doors = every_door(&client, &base, &target, kind, token);
            assert_eq!(doors.len(), 10);
            open.extend(doors_not_refusing(kind, doors).await);
        }
    }
    assert!(open.is_empty(), "{} doors did not refuse a substrate kind:\n{}", open.len(), open.join("\n"));
    agent.shutdown().await;
}

/// The same on a node with **no consensus listener** and nothing in process on the kind: the serve stream's
/// refusal does not depend on a receiver being registered. Seen failing first: every consensus kind answered 200.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_serve_stream_refuses_a_consensus_kind_without_a_listener() {
    let (agent, base, client) = node(|_| {}).await;
    let mut open = Vec::new();
    for kind in ["consensus.propose", "consensus.vote", "consensus.commit", "consensus.nack"] {
        let path = format!("/gateway/rpc/serve/{kind}");
        open.extend(not_refused(&path, kind, client.get(format!("{base}{path}")).bearer_auth(TOKEN)).await);
    }
    assert!(open.is_empty(), "{open:#?}");
    agent.shutdown().await;
}

/// The RPC reply kinds are refused on every **send** door and on the mailbox stream, as they already were on the
/// observe and serve doors. Seen failing first with the guards as they were: seven doors accepted each of
/// `rpc.result` and `bulk.result` — the six raw routes and the mailbox stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_send_door_refuses_a_reply_kind() {
    let (agent, base, client) = node(|_| {}).await;
    let target = agent.node_id().to_string();
    let mut open = Vec::new();
    for token in tokens() {
        for kind in REPLY_KINDS {
            // Observe and serve refused them before this change; the mailbox stream follows `mailbox/deliver`.
            open.extend(doors_not_refusing(kind, every_door(&client, &base, &target, kind, token)).await);
        }
    }
    assert!(open.is_empty(), "{} doors did not refuse a reply kind:\n{}", open.len(), open.join("\n"));
    agent.shutdown().await;
}

/// A hosted capability's kind is refused on the two signal streams, the mailbox stream and the six raw routes
/// — nine doors; the serve stream is not changed (it answers `served_in_process` where a component serves the
/// kind). Seen failing first: with the observe refusal alone, the six raw routes and the mailbox stream did not
/// refuse; before that, both signal streams answered 200.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nine_doors_refuse_a_hosted_capability_kind_and_the_serve_stream_is_unchanged() {
    let (agent, base, client) = node(|_| {}).await;
    let target = agent.node_id().to_string();
    let mut open = Vec::new();
    for token in tokens() {
        for kind in ["cap.invoke/text/echo", "cap.invoke/", "cap.invoke/route/optimize.shadow"] {
            assert!(kind.starts_with(HOSTED_CAPABILITY_KIND_PREFIX));
            let (serve, refused): (Vec<_>, Vec<_>) = every_door(&client, &base, &target, kind, token).into_iter()
                .partition(|(door, _, _)| *door == Door::Serve);
            assert_eq!((serve.len(), refused.len()), (1, 9));
            open.extend(doors_not_refusing(kind, refused).await);
            for (_, path, req) in serve {
                let r = req.send().await.unwrap();
                assert_eq!(r.status(), 200, "{path} is not changed for `{kind}`: nothing serves it on this node");
            }
        }
    }
    assert!(open.is_empty(), "{} doors did not refuse a hosted capability's kind:\n{}", open.len(), open.join("\n"));
    agent.shutdown().await;
}

/// **The plant: an ordinary kind, and every lookalike, is still observable, servable and sendable at every door**,
/// under the token that holds `*` and under the scoped one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ordinary_kind_passes_every_door() {
    let (agent, base, client) = node(|_| {}).await;
    #[cfg(feature = "consensus")]
    let _listener = agent.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    let target = agent.node_id().to_string();
    for token in tokens() {
        for kind in LOOKALIKES {
            assert!(!is_substrate_kind(kind), "{kind}");
            // Together: three of the sends wait a second for a reply no one gives.
            let answers = futures_util::future::join_all(every_door(&client, &base, &target, kind, token).into_iter()
                .map(|(door, path, req)| async move { (door, path, req.send().await.unwrap()) })).await;
            for (door, path, r) in answers {
                match door {
                    // Not a refusal of any sort: neither the kind (403 `protected_kind`) nor a scope (403, 401).
                    Door::Send => assert!(r.status() != 403 && r.status() != 401, "{path} refused the ordinary kind `{kind}`: {} {:?}", r.status(), r.text().await),
                    _ => assert_eq!(r.status(), 200, "{path} must stream the ordinary kind `{kind}`"),
                }
            }
        }
    }
    // …and an emitted ordinary signal arrives on a tap, payload intact.
    let mut tap = client.get(format!("{base}/gateway/signal/sse/repair.request")).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(tap.status(), 200);
    let r = client.post(format!("{base}/gateway/signal/emit")).bearer_auth(TOKEN)
        .json(&serde_json::json!({"kind": "repair.request", "scope": "cluster", "payload_b64": b64(b"pump-7")}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200);
    let event = next_event(&mut tap, Duration::from_secs(5)).await.expect("the ordinary signal reaches its tap");
    assert_eq!(event, b"pump-7");
    agent.shutdown().await;
}

/// The next SSE event's payload on an open stream, decoded; `None` when none arrives in `within`.
async fn next_event(stream: &mut reqwest::Response, within: Duration) -> Option<Vec<u8>> {
    let until = tokio::time::Instant::now() + within;
    let mut text = String::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(until, stream.chunk()).await {
        text.push_str(&String::from_utf8_lossy(&chunk));
        for line in text.lines().filter_map(|l| l.strip_prefix("data:")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                && let Some(p) = v["payload_b64"].as_str()
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(p)
            {
                return Some(bytes);
            }
        }
    }
    None
}

/// Every payload a stream carries until `within` has passed.
#[cfg(feature = "consensus")]
async fn drain(stream: &mut reqwest::Response, within: Duration) -> Vec<Vec<u8>> {
    let until = tokio::time::Instant::now() + within;
    let mut text = String::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(until, stream.chunk()).await {
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text.lines().filter_map(|l| l.strip_prefix("data:"))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .filter_map(|v| v["payload_b64"].as_str().and_then(|p| base64::engine::general_purpose::STANDARD.decode(p).ok()))
        .collect()
}

/// A token of exactly `scopes` where the build has scoped tokens; the legacy token elsewhere.
fn scoped(cfg: &mut GossipConfig, scopes: &[&str]) -> &'static str {
    #[cfg(feature = "compliance")]
    {
        cfg.gateway_scoped_tokens.push(crate::GatewayToken { token: "scoped".into(), scopes: scopes.iter().map(|s| s.to_string()).collect() });
        "scoped"
    }
    #[cfg(not(feature = "compliance"))]
    {
        let _ = (cfg, scopes);
        TOKEN
    }
}

/// **Proof (a): a `mesh:read` tap carried a peer's proposed value.** A peer proposes a value on a commitment award
/// slot (`cn/…/award`, a component's); a client holding only `mesh:read` on the other node's gateway opens each
/// consensus kind on each signal stream. On the unfixed code all eight streams answered 200, and the proposed value
/// and the slot name arrived in `payload_b64` on `consensus.propose` (the PROPOSE) and on `consensus.commit` (the
/// COMMIT) at both doors; the test failed on the first of them.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tap_never_carries_a_peers_proposed_value() {
    let mut token = TOKEN;
    let (gw, peer, base, client) = pair(|c| token = scoped(c, &["mesh:read"])).await;
    let _lg = gw.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    let _lp = peer.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    const SECRET: &[u8] = b"AWARD-to-driver-a-at-41.50";
    let mut carried = Vec::new();
    let mut statuses = Vec::new();
    for (n, kind) in ["consensus.propose", "consensus.vote", "consensus.commit", "consensus.nack"].into_iter().enumerate() {
        for door in ["/signals/", "/gateway/signal/sse/"] {
            let path = format!("{door}{kind}");
            let mut tap = client.get(format!("{base}{path}")).bearer_auth(token).send().await.unwrap();
            statuses.push((path.clone(), tap.status().as_u16()));
            if tap.status() != 200 { continue; }
            // The stream is open: a peer proposes, and whatever the tap carries is read.
            let slot = format!("cn/req-{n}-{}/award", if door == "/signals/" { "node" } else { "gw" });
            let proposer = peer.consensus();
            let s = slot.clone();
            let proposal = tokio::spawn(async move {
                proposer.cluster_propose(&s, Bytes::from_static(SECRET), crate::consensus::ConsensusConfig::default()).await
            });
            let frames = drain(&mut tap, Duration::from_secs(3)).await;
            let _ = proposal.await;
            let has = |frame: &[u8], needle: &[u8]| frame.windows(needle.len()).any(|w| w == needle);
            if frames.iter().any(|f| has(f, SECRET) && has(f, slot.as_bytes())) {
                carried.push(path);
            }
        }
    }
    assert!(carried.is_empty(), "a `mesh:read` tap carried the proposed value and its slot on: {carried:?}");
    for (path, status) in statuses {
        assert_eq!(status, 403, "{path} is refused for a `mesh:read` token");
    }
    gw.shutdown().await;
    peer.shutdown().await;
}

/// **Proof (b): without `[tls]`, an emitted COMMIT wrote a decision record on every node.** A client holding only
/// `mesh:write` posts a `CommitTerm` it made up — a slot no one proposed, a value no one voted for — to
/// `/gateway/signal/emit` under `consensus.commit`. On the unfixed code the route answered 200, and the gateway's
/// node and its peer both wrote the record: `live_committed_value` returned the forged value on each (no quorum, no
/// vote, no proposer), which is what `GET /consensus/{slot}` and `consistent_get` read. With `[tls]` the frame would
/// need a member's signature (`decode_verify`); this test's mesh has none, as a mesh without `[tls]` has none.
#[cfg(feature = "consensus")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_emitted_commit_writes_no_decision_record() {
    use crate::consensus::{encode_consensus_msg, live_committed_value, causal_now_ms, ConsensusMsg};
    use crate::consensus_life::{Envelope, Term};
    let mut token = TOKEN;
    let (gw, peer, base, client) = pair(|c| token = scoped(c, &["mesh:write"])).await;
    let _lg = gw.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    let _lp = peer.consensus().start_consensus_listener(crate::consensus::ConsensusConfig::default());
    let slot = "cn/req-9/award";
    const FORGED: &[u8] = b"AWARD-to-the-attacker";
    let nobody = NodeId::new("127.0.0.1", 1).unwrap();
    let envelope = Envelope::new(slot, Bytes::from_static(FORGED), Term::Permanent, 7, nobody, 7).encode();
    let frame = encode_consensus_msg(&ConsensusMsg::CommitTerm { slot: Arc::from(slot), ballot: 7, envelope });
    let r = client.post(format!("{base}/gateway/signal/emit")).bearer_auth(token)
        .json(&serde_json::json!({"kind": "consensus.commit", "scope": "cluster", "payload_b64": b64(&frame)}))
        .send().await.unwrap();
    let status = r.status().as_u16();
    // Long enough for the frame to be delivered here and gossiped to the peer, had it been sent.
    let read = |a: &GossipAgent| live_committed_value(&a.task_ctx.kv_state, slot, causal_now_ms(&a.task_ctx.hlc));
    let until = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < until && (read(&gw).is_none() || read(&peer).is_none()) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let wrote: Vec<&str> = [("the gateway's node", read(&gw)), ("its peer", read(&peer))].into_iter()
        .filter(|(_, v)| v.as_deref() == Some(FORGED)).map(|(who, _)| who).collect();
    assert!(wrote.is_empty(), "a `mesh:write` client's COMMIT was written as the slot's decision on: {wrote:?} (emit answered {status})");
    assert_eq!(read(&gw), None);
    assert_eq!(read(&peer), None);
    assert_eq!(status, 403, "the emit is refused");
    gw.shutdown().await;
    peer.shutdown().await;
}

/// **Proof (c): a tapped nonce forged an RPC reply.** A caller on another node calls `depot.quote` on the
/// gateway's node, whose provider takes a moment to answer. A client holding `mesh:read` and `mesh:write` on the
/// provider's gateway taps `depot.quote` (an ordinary kind, observable), reads the call's correlation nonce from
/// the first eight bytes of `payload_b64`, and emits `rpc.result` to the caller with that nonce. On the unfixed
/// code the emit answered 200 and the caller's `rpc_call` returned the forged bytes: a reply is claimed on the
/// nonce and `sender == target`, and the gateway's node *is* the target.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tapped_nonce_forges_no_rpc_reply() {
    let mut token = TOKEN;
    let (provider, caller, base, client) = pair(|c| token = scoped(c, &["mesh:read", "mesh:write"])).await;
    let mut rx = provider.service().rpc_rx("depot.quote");
    let p = Arc::clone(&provider);
    let serving = tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            p.service().rpc_respond(&req, Bytes::from_static(b"REAL 41.50"));
        }
    });
    let mut tap = client.get(format!("{base}/gateway/signal/sse/depot.quote")).bearer_auth(token).send().await.unwrap();
    assert_eq!(tap.status(), 200, "an ordinary RPC kind is observable");
    let c = Arc::clone(&caller);
    let target = provider.node_id().clone();
    let call = tokio::spawn(async move {
        c.service().rpc_call(target, "depot.quote", Bytes::from_static(b"quote?"), Duration::from_secs(6)).await
    });
    let request = next_event(&mut tap, Duration::from_secs(5)).await.expect("the request reaches the tap");
    let mut forged = request[..8].to_vec();
    forged.extend_from_slice(b"FORGED 0.01");
    let r = client.post(format!("{base}/gateway/signal/emit")).bearer_auth(token)
        .json(&serde_json::json!({"kind": "rpc.result", "scope": format!("node:{}", caller.node_id()), "payload_b64": b64(&forged)}))
        .send().await.unwrap();
    let status = r.status().as_u16();
    let answer = call.await.unwrap().expect("the call is answered");
    assert_eq!(&answer[..], b"REAL 41.50", "the caller took a reply the gateway's client wrote (emit answered {status})");
    assert_eq!(status, 403, "the emit of a reply kind is refused");
    serving.abort();
    provider.shutdown().await;
    caller.shutdown().await;
}

/// `sys.explain` by another door: `POST /gateway/rpc/call` reached a peer's event ring under `mesh:write`, where
/// `GET /gateway/explain` asks `fleet:read`. Closed by the prefix. Seen failing first: 200 with the ring.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_event_ring_is_not_read_through_rpc_call() {
    let mut token = TOKEN;
    let (gw, peer, base, client) = pair(|c| { c.emergent_detectors_enabled = true; token = scoped(c, &["mesh:write"]); }).await;
    for target in [gw.node_id().to_string(), peer.node_id().to_string()] {
        let r = client.post(format!("{base}/gateway/rpc/call")).bearer_auth(token)
            .json(&serde_json::json!({"target": target, "method": "sys.explain", "payload_b64": b64(&0u64.to_le_bytes()), "timeout_secs": 2}))
            .send().await.unwrap();
        assert_eq!(r.status(), 403, "rpc/call to {target}");
        assert_eq!(r.json::<serde_json::Value>().await.unwrap()["error"], "protected_kind");
    }
    gw.shutdown().await;
    peer.shutdown().await;
}

/// The next SSE event on an open stream as `(sender, payload)`; `None` when none arrives in `within`.
async fn next_frame(stream: &mut reqwest::Response, within: Duration) -> Option<(String, Vec<u8>)> {
    let until = tokio::time::Instant::now() + within;
    let mut text = String::new();
    while let Ok(Ok(Some(chunk))) = tokio::time::timeout_at(until, stream.chunk()).await {
        text.push_str(&String::from_utf8_lossy(&chunk));
        for line in text.lines().filter_map(|l| l.strip_prefix("data:")) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim())
                && let (Some(sender), Some(p)) = (v["sender"].as_str(), v["payload_b64"].as_str())
                && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(p)
            {
                return Some((sender.to_string(), bytes));
            }
        }
    }
    None
}

/// **Proof (d): a hosted capability was called around `/mcp`.** A component's capability is served on
/// `cap.invoke/{namespace}/{name}` by a receiver on its node (`rpc_rx`, as `mycelium-wasm-host`'s runtime
/// registers it — the receiver here stands in for the component's serve loop) and reached over HTTP as a tool
/// through `/mcp`, which asks `mcp:invoke` and runs the preflight. A client holding only `mesh:write` — refused
/// at `/mcp` — posted the kind to `POST /gateway/rpc/call`. On the unfixed code the route answered 200 with the
/// capability's output, and the handler had run. Now it is refused, the handler never runs for the client, and a
/// **member's** own call (`ServiceHandle::rpc_call`, what the MCP bridge makes in process) is served as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hosted_capability_is_not_called_around_mcp() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut token = TOKEN;
    let (agent, base, client) = node(|c| token = scoped(c, &["mesh:write"])).await;
    let cap = "cap.invoke/text/echo";
    let ran = Arc::new(AtomicUsize::new(0));
    let mut rx = agent.service().rpc_rx(cap);
    let (a, r) = (Arc::clone(&agent), Arc::clone(&ran));
    let serving = tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            r.fetch_add(1, Ordering::SeqCst);
            let mut out = b"component ran: ".to_vec();
            out.extend_from_slice(&req.payload());
            a.service().rpc_respond(&req, out);
        }
    });
    // The door for a bridged component is `/mcp`: this token does not open it (where the build has scopes).
    #[cfg(feature = "compliance")]
    {
        let call = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "echo", "arguments": {}}});
        let r = client.post(format!("{base}/mcp")).bearer_auth(token).json(&call).send().await.unwrap();
        assert_eq!(r.status(), 403, "a `mesh:write` token is refused at /mcp");
    }
    let target = agent.node_id().to_string();
    let r = client.post(format!("{base}/gateway/rpc/call")).bearer_auth(token)
        .json(&serde_json::json!({"target": target, "method": cap, "payload_b64": b64(b"{\"text\":\"hi\"}"), "timeout_secs": 3}))
        .send().await.unwrap();
    let status = r.status().as_u16();
    let body: serde_json::Value = r.json().await.unwrap_or_default();
    let answer = body["result_b64"].as_str().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok());
    assert!(
        answer.is_none() && ran.load(Ordering::SeqCst) == 0,
        "a `mesh:write` client invoked the hosted capability through rpc/call: status {status}, the handler ran {} time(s), answer {:?}",
        ran.load(Ordering::SeqCst), answer.map(|a| String::from_utf8_lossy(&a).into_owned()),
    );
    assert_eq!(status, 403);
    assert_eq!(body["error"], "protected_kind", "{body}");
    assert!(body["message"].as_str().unwrap_or("").contains("/mcp"), "the refusal names the door: {body}");
    // A member is not a client: the node's own call reaches the component.
    let own = agent.service().rpc_call(agent.node_id().clone(), cap, Bytes::from_static(b"member"), Duration::from_secs(3)).await;
    assert_eq!(own.as_deref().ok(), Some(&b"component ran: member"[..]), "a member's in-process call is served");
    assert_eq!(ran.load(Ordering::SeqCst), 1);
    serving.abort();
    agent.shutdown().await;
}

/// **Proof (e): a tapped bulk ticket fetched the bulk payload.** A member calls `bulk_call` on the gateway's node:
/// it stages the payload on its own gateway and sends an `invoke.bulk` ticket, `nonce ‖ port ‖ kind`. A client
/// holding only `mesh:read` on the *responder's* gateway tapped `invoke.bulk`, read the nonce and the port from
/// `payload_b64` and the caller's address from `sender`, and requested `GET /bulk/{nonce}` on the caller — a
/// route outside the token layer, by design, whose nonce is its credential. On the unfixed code the tap answered
/// 200 and the fetch returned the whole request payload while the call was in flight. Now the tap is refused, and
/// the members' bulk call completes as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tapped_bulk_ticket_fetches_no_payload() {
    let mut token = TOKEN;
    let (responder, caller, base, client) = pair(|c| token = scoped(c, &["mesh:read"])).await;
    const MANIFEST: &[u8] = b"MANIFEST: 40 pallets to depot 7, gate code 4411";
    let _serve = responder.service().bulk_serve("depot.manifest", |_sender, payload| async move {
        // Long enough for a tap to act while the payload is staged.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        Bytes::from(format!("received {} bytes", payload.len()))
    });
    let mut fetched: Option<Vec<u8>> = None;
    let mut statuses = Vec::new();
    for door in ["/signals/invoke.bulk", "/gateway/signal/sse/invoke.bulk"] {
        let mut tap = client.get(format!("{base}{door}")).bearer_auth(token).send().await.unwrap();
        statuses.push((door, tap.status().as_u16()));
        if tap.status() != 200 { continue; }
        let (c, target) = (Arc::clone(&caller), responder.node_id().clone());
        let call = tokio::spawn(async move {
            c.service().bulk_call(target, "depot.manifest", Bytes::from_static(MANIFEST), Duration::from_secs(8)).await
        });
        if let Some((sender, ticket)) = next_frame(&mut tap, Duration::from_secs(5)).await
            && ticket.len() >= 10
        {
            let nonce = u64::from_le_bytes(ticket[..8].try_into().unwrap());
            let port = u16::from_le_bytes(ticket[8..10].try_into().unwrap());
            let host = sender.rsplit_once(':').map(|(h, _)| h.to_string()).unwrap_or_default();
            // No bearer: the staging route is public, the nonce its credential.
            if let Ok(r) = client.get(format!("http://{host}:{port}/bulk/{nonce:016x}")).send().await
                && r.status() == 200
            {
                fetched = r.bytes().await.ok().map(|b| b.to_vec());
            }
        }
        let _ = call.await;
    }
    assert!(
        fetched.as_deref() != Some(MANIFEST),
        "a `mesh:read` client read a bulk request's payload: the tap gave it the ticket, and /bulk/{{nonce}} the bytes (taps answered {statuses:?})",
    );
    assert_eq!(fetched, None);
    for (door, status) in statuses {
        assert_eq!(status, 403, "{door} is refused for a `mesh:read` token");
    }
    // The members' own bulk call is not a gateway door.
    let answer = caller.service()
        .bulk_call(responder.node_id().clone(), "depot.manifest", Bytes::from_static(MANIFEST), Duration::from_secs(8)).await;
    assert_eq!(answer.as_deref().ok(), Some(format!("received {} bytes", MANIFEST.len()).as_bytes()), "a member's bulk call completes");
    responder.shutdown().await;
    caller.shutdown().await;
}

/// Every `.rs` file under `dir`, test files included (a kind constant in a test would be a kind all the same).
fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `(name, value)` for every `const NAME: &str = "value";` (or `&'static str`) on one line of `src`.
fn str_consts(src: &str) -> Vec<(String, String)> {
    src.lines().filter_map(|line| {
        let rest = line.trim_start().trim_start_matches("pub(crate) ").trim_start_matches("pub(super) ").trim_start_matches("pub ");
        let rest = rest.strip_prefix("const ")?;
        let (name, rest) = rest.split_once(':')?;
        let rest = rest.trim_start().strip_prefix('&')?.trim_start_matches("'static").trim_start().strip_prefix("str")?;
        let value = rest.trim_start().strip_prefix('=')?.trim_start().strip_prefix('"')?.split_once('"')?.0;
        Some((name.trim().to_string(), value.to_string()))
    }).collect()
}

/// The constants of `pub mod {name} {{ … }}` in `src`.
fn module_consts(src: &str, name: &str) -> Vec<(String, String)> {
    let module = src.split_once(&format!("pub mod {name} {{")).unwrap_or_else(|| panic!("no module {name}")).1;
    str_consts(module.split_once("\n}").expect("the module's end").0)
}

/// Which side of the gateway's doors a kind constant is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Refused `403 protected_kind` on observe, the mailbox stream and the six send routes.
    Refused,
    /// Open at the gateway **by a decision**, which the string names. Not an omission.
    Open(&'static str),
}

/// **Every signal-kind constant the library and its companions define, classified.** A row per value: the
/// constants of `signal_kind` (`mycelium-core/src/signal.rs`) and `consensus_kind` (`src/consensus.rs`), and every
/// string constant whose name contains `KIND` under `src/` and each `mycelium-*/src`.
const CLASSIFIED: &[(&str, Class)] = &[
    // The consensus protocol and the node-to-node `sys.` calls: the substrate's own.
    ("consensus.propose", Class::Refused), ("consensus.vote", Class::Refused),
    ("consensus.commit", Class::Refused), ("consensus.nack", Class::Refused),
    ("sys.explain", Class::Refused), ("sys.replica-sync.v1", Class::Refused),
    // The bulk transport: its ticket's nonce is the credential for the staged payload.
    ("invoke.bulk", Class::Refused),
    // RPC replies: claimed on a nonce and the sender, which an emit through the gateway supplies.
    ("rpc.result", Class::Refused), ("bulk.result", Class::Refused),
    // Protected work, each with a door of its own (closure plan C1).
    ("mcp.invoke", Class::Refused), ("llm.invoke", Class::Refused),
    // Conventions the library names and neither registers for nor emits: an application's to use.
    ("invoke", Class::Open("a naming convention for applications; no library code registers or emits it")),
    ("invoke.result", Class::Open("a naming convention for applications; no library code registers or emits it")),
    ("contract.available", Class::Open("a naming convention for applications; no library code registers or emits it")),
    ("contract.withdrawn", Class::Open("a naming convention for applications; no library code registers or emits it")),
    ("cluster.event", Class::Open("a naming convention for applications; no library code registers or emits it")),
    ("health.probe", Class::Open("a naming convention for applications; liveness is SWIM's, not a signal's")),
    ("health.ack", Class::Open("a naming convention for applications; liveness is SWIM's, not a signal's")),
    // Open by the decision of 2026-11-13, each stated in rbac.md and on the critique plan.
    ("boundary.opaque", Class::Open("a load announcement; an emitted one makes a proposer re-read the KV, nothing more (CR27)")),
    ("boundary.transparent", Class::Open("a load announcement (CR27)")),
    ("agent.state", Class::Open("a node's state transitions by name (CR27)")),
    ("agent.approve", Class::Open("the supervisor pattern is driven by signals, an SDK supervisor included; unsigned (CR26)")),
    ("agent.veto", Class::Open("the supervisor pattern is driven by signals; any `mesh:write` holder can veto, it fails closed (CR26)")),
    ("artifact.fetch", Class::Open("a tap shows the artifact ids asked for, never a reply; a call fetches a signed, public artifact (CR27)")),
    ("artifact.size", Class::Open("as artifact.fetch (CR27)")),
    ("artifact.fetch_range", Class::Open("as artifact.fetch (CR27)")),
    ("reason.blob.fetch", Class::Open("a tap shows the blob ids asked for, never a reply; a call fetches a blob by an id the caller knows (CR27)")),
    // Not a signal kind: `opacity.rs`'s name for the persistence load mark.
    ("persistence", Class::Open("not a signal kind: the opacity governor's label for its persistence mark")),
];

/// **A new kind constant fails here until it is classified, and the list is derived from the constants.**
///
/// What this gates: every constant of `signal_kind` and `consensus_kind`, and every `…KIND…` string constant in
/// the library crates and the companions, has a row in [`CLASSIFIED`]; a *refused* row is refused by the doors'
/// own functions and an *open* row is not, and says why it is open; every constant under a substrate prefix, and
/// every refused substrate kind, is in [`SUBSTRATE_KINDS`], which names nothing no constant holds.
///
/// What it does **not** gate, because no constant holds them: a kind written as a literal at its call site
/// (`"skill.invoke"`, refused by `BUILTIN_PROTECTED_RPC_KINDS`), and the families built with `format!` —
/// `blackboard.{ns}.{op}`, `tuple.{ns}.{op}`, `wiki.{group}.{op}` (gated by family scope, `companion_kind_scope`),
/// `cap.invoke/{ns}/{name}` (refused by prefix) and a component's `comp/{ns}/…` (its own). **The default for a
/// kind outside every list is open**: a new internal kind is refused only if it is named under `consensus.` or
/// `sys.`, or is a constant this test then makes someone classify.
///
/// Seen failing first with a fifth constant planted in `consensus_kind` (`"consensus.planted"`), and with
/// `invoke.bulk` off the list while its row said *refused*.
#[test]
fn every_kind_constant_is_classified_for_the_gateways_doors() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("mycelium-core/src"), &mut files);
    for entry in std::fs::read_dir(root).expect("the workspace root").flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("mycelium-") {
            rust_files(&entry.path().join("src"), &mut files);
        }
    }
    files.sort();
    files.dedup();
    assert!(files.len() > 100, "the library sources were not found from {root:?}: {} files", files.len());
    let mut found = std::collections::BTreeMap::new();
    for file in &files {
        let Ok(src) = std::fs::read_to_string(file) else { continue };
        let at = file.strip_prefix(root).unwrap_or(file).display().to_string();
        for (name, value) in str_consts(&src) {
            // A `…KIND…` constant is a kind; a `…PREFIX…` one is a family's prefix, classified by its family.
            if (name.contains("KIND") && !name.contains("PREFIX")) || SUBSTRATE_KIND_PREFIXES.iter().any(|p| value.starts_with(p)) {
                found.insert(value, format!("{name} in {at}"));
            }
        }
    }
    let read = |path: &str| std::fs::read_to_string(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let signal = module_consts(&read("mycelium-core/src/signal.rs"), "signal_kind");
    assert!(signal.len() >= 17, "signal_kind's constants were not read: {signal:?}");
    for (name, value) in signal {
        found.insert(value, format!("signal_kind::{name}"));
    }
    let consensus = module_consts(&read("src/consensus.rs"), "consensus_kind");
    assert!(consensus.len() >= 4, "consensus_kind's constants were not read: {consensus:?}");
    for (name, value) in consensus {
        assert!(is_substrate_kind(&value), "consensus_kind::{name} = {value:?} is outside the substrate prefixes: no door refuses it");
        found.insert(value, format!("consensus_kind::{name}"));
    }

    let cfg = GossipConfig::default();
    let class_of = |kind: &str| CLASSIFIED.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c);
    for (value, at) in &found {
        let Some(class) = class_of(value) else {
            panic!("{value:?} ({at}) is a kind constant with no row in CLASSIFIED: decide whether the gateway's doors refuse it");
        };
        let (send, observe) = (
            super::authz::refuse_protected_kind(&cfg, value).is_some(),
            super::authz::refuse_unobservable_kind(&cfg, value).is_some(),
        );
        match class {
            Class::Refused => assert!(send && observe, "{value:?} ({at}) is classified refused, and the doors do not refuse it (send {send}, observe {observe})"),
            Class::Open(why) => {
                assert!(!send && !observe, "{value:?} ({at}) is classified open, and the doors refuse it");
                assert!(why.len() > 20, "{value:?} is open and its row does not say why");
            }
        }
        if SUBSTRATE_KIND_PREFIXES.iter().any(|p| value.starts_with(p)) {
            assert!(SUBSTRATE_KINDS.contains(&value.as_str()), "{value:?} ({at}) is a substrate kind constant missing from SUBSTRATE_KINDS");
        }
    }
    for (kind, _) in CLASSIFIED {
        assert!(found.contains_key(*kind), "CLASSIFIED names {kind:?}, which no constant holds");
        assert_eq!(CLASSIFIED.iter().filter(|(k, _)| k == kind).count(), 1, "{kind:?} is classified twice");
    }
    for kind in SUBSTRATE_KINDS {
        assert!(found.contains_key(*kind), "SUBSTRATE_KINDS names {kind:?}, which no constant holds");
        assert_eq!(class_of(kind), Some(Class::Refused), "{kind:?}");
        assert!(
            BY_NAME.contains(kind) != SUBSTRATE_KIND_PREFIXES.iter().any(|p| kind.starts_with(p)),
            "{kind:?} is under a prefix, or named in BY_NAME — one or the other",
        );
    }
    #[cfg(feature = "consensus")]
    for kind in [crate::consensus_kind::PROPOSE, crate::consensus_kind::VOTE, crate::consensus_kind::COMMIT, crate::consensus_kind::NACK] {
        assert!(SUBSTRATE_KINDS.contains(&kind));
    }
    assert!(SUBSTRATE_KINDS.contains(&crate::agent::emergent::EXPLAIN_RPC_KIND));
    assert!(SUBSTRATE_KINDS.contains(&crate::agent::replica_sync::RPC_KIND));
    assert!(SUBSTRATE_KINDS.contains(&crate::signal::signal_kind::INVOKE_BULK));
}
