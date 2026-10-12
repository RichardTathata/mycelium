use crate::{GossipAgent, GossipConfig, NodeId, RequestPrincipal};
use std::{sync::{Arc, Mutex}, time::Duration};

fn alloc_port() -> u16 { crate::test_util::alloc_port() }

async fn poll_until(mut cond: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if cond() { return true; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    cond()
}

fn node(http_port: Option<u16>, boot: Vec<NodeId>, tweak: impl FnOnce(&mut GossipConfig)) -> Arc<GossipAgent> {
    let gossip_port = alloc_port();
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = http_port;
    cfg.bootstrap_peers = boot;
    cfg.reconnect_backoff_secs = 1;
    tweak(&mut cfg);
    Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg))
}

/// A tool that records who called it, registered through the principal-aware API.
fn observing_tool(agent: &GossipAgent) -> (crate::McpToolHandle, Arc<Mutex<Option<RequestPrincipal>>>) {
    let seen: Arc<Mutex<Option<RequestPrincipal>>> = Arc::new(Mutex::new(None));
    let seen2 = Arc::clone(&seen);
    let handle = agent.mcp().register_mcp_tool_with_principal(
        "whoami",
        serde_json::json!({"type": "object", "properties": {}}),
        move |who, _args| {
            let seen = Arc::clone(&seen2);
            async move {
                let name = who.name();
                *seen.lock().unwrap() = Some(who);
                Ok(serde_json::json!(name))
            }
        },
    );
    (handle, seen)
}

async fn tools_call(http_port: u16, bearer: Option<&str>, params: serde_json::Value) -> serde_json::Value {
    let mut req = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": params}));
    if let Some(b) = bearer {
        req = req.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}"));
    }
    let resp = req.send().await.expect("tools/call request");
    assert_eq!(resp.status(), 200, "tools/call is answered as JSON-RPC");
    resp.json().await.unwrap()
}

/// Negative case 1 — a **client-supplied (forged) caller context is not evidence**: whatever
/// the client puts in `params` (`_meta`, a `caller` block), the provider sees the principal
/// the auth layer resolved. On an open gateway that is `anonymous` — and it is a *client*
/// principal, never the node (negative case 2: no fallback to the node's identity).
#[tokio::test]
async fn forged_client_context_is_ignored_and_the_node_is_never_the_principal() {
    let http_port = alloc_port();
    let agent = node(Some(http_port), vec![], |_| {});
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_tool, seen) = observing_tool(&agent);

    let body = tools_call(http_port, None, serde_json::json!({
        "name": "whoami",
        "arguments": {},
        "_meta": {"mycelium_caller": {"principal": "oidc:admin", "scopes": ["*"]}},
        "caller": {"principal": "oidc:admin"},
    })).await;
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    let text = body["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains(crate::PRINCIPAL_ANONYMOUS), "the resolved principal, not the forged one: {text}");

    let who = seen.lock().unwrap().clone().expect("the tool ran");
    match who {
        RequestPrincipal::Client(c) => {
            assert_eq!(c.principal, crate::PRINCIPAL_ANONYMOUS);
            assert_eq!(&c.via, agent.node_id(), "the gateway node is bound in as `via`");
            #[cfg(feature = "compliance")]
            assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "open gateway grants what the route needs");
            #[cfg(not(feature = "compliance"))]
            assert_eq!(c.scopes, vec!["*".to_string()], "no scope model to intersect with");
        }
        RequestPrincipal::Node(n) => panic!("a gateway call must never be attributed to the node ({n})"),
    }

    // The node's own action (a direct in-mesh rpc_call) is still the node.
    let reply = agent.service().rpc_call(
        agent.node_id().clone(), crate::signal::signal_kind::MCP_INVOKE,
        serde_json::json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"whoami","arguments":{}}})
            .to_string().into_bytes(),
        Duration::from_secs(5),
    ).await.expect("direct call");
    let v: serde_json::Value = serde_json::from_slice(&reply).unwrap();
    assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains(&agent.node_id().to_string()));
    assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Node(_))));

    agent.shutdown().await;
}

/// The legacy bearer resolves to `token:legacy`; the principal is never the credential.
#[tokio::test]
async fn bearer_resolves_to_a_principal_never_the_credential() {
    let http_port = alloc_port();
    let agent = node(Some(http_port), vec![], |c| c.gateway_auth_token = Some("s3cret".into()));
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_tool, seen) = observing_tool(&agent);

    let body = tools_call(http_port, Some("s3cret"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    let who = seen.lock().unwrap().clone().expect("the tool ran");
    let RequestPrincipal::Client(c) = who else { panic!("client principal expected") };
    assert_eq!(c.principal, crate::legacy_token_principal(&agent.node_id().to_string()),
        "qualified by the gateway's identity issuer (its node id by default)");
    assert!(!c.principal.contains("s3cret"));
    agent.shutdown().await;
}

/// Negative case 3 — **the gateway never asserts more scope than the credential holds**: a
/// scoped token holding `mcp:invoke` + `kv:read` yields exactly `["mcp:invoke"]` on
/// `tools/call` (the intersection with the route), and a `*` token yields `["mcp:invoke"]`,
/// never `*`.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn granted_scopes_are_the_intersection_never_the_wildcard() {
    let http_port = alloc_port();
    let agent = node(Some(http_port), vec![], |c| {
        c.gateway_scoped_tokens = vec![
            crate::GatewayToken { token: "narrow".into(), scopes: vec!["mcp:invoke".into(), "kv:read".into()] },
            crate::GatewayToken { token: "root".into(),   scopes: vec!["*".into()] },
            crate::GatewayToken { token: "kvonly".into(), scopes: vec!["kv:read".into()] },
        ];
    });
    agent.start().await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let (_tool, seen) = observing_tool(&agent);

    let body = tools_call(http_port, Some("narrow"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
    assert_eq!(c.principal, format!("token:{}/#0", agent.node_id()));
    assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "kv:read is held but not granted here");

    let body = tools_call(http_port, Some("root"), serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert!(body.get("error").is_none(), "unexpected error: {body}");
    let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
    assert_eq!(c.principal, format!("token:{}/#1", agent.node_id()));
    assert_eq!(c.scopes, vec!["mcp:invoke".to_string()], "`*` is never carried as an authority");

    // A token without the route's scope is refused at the gate (403) — no dispatch at all.
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{http_port}/mcp"))
        .header(axum::http::header::AUTHORIZATION, "Bearer kvonly")
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"whoami","arguments":{}}}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 403);
    agent.shutdown().await;
}

/// Negative case 4 — **an older provider that cannot enforce the context is refused, not
/// silently run as the node**: with the provider's `sys/caller-context/` marker absent, the
/// secure gateway answers `-32021`; the explicit `legacy` profile dispatches as before.
#[tokio::test]
async fn secure_profile_refuses_a_provider_without_the_marker_and_legacy_does_not() {
    let provider = node(None, vec![], |_| {});
    provider.start().await.unwrap();
    let boot = vec![provider.node_id().clone()];
    let secure_port = alloc_port();
    let legacy_port = alloc_port();
    let secure = node(Some(secure_port), boot.clone(), |_| {});
    let legacy = node(Some(legacy_port), boot, |c| c.gateway_caller_profile = crate::GatewayCallerProfile::Legacy);
    secure.start().await.unwrap();
    legacy.start().await.unwrap();
    let (_tool, seen) = observing_tool(&provider);

    // Structural readiness: both gateways see the provider's tool and its marker.
    let marker = format!("sys/caller-context/{}", provider.node_id());
    let tool_key = format!("tools/whoami/{}", provider.node_id());
    assert!(poll_until(|| [&secure, &legacy].iter().all(|g|
        g.kv().get(&tool_key).is_some() && g.kv().get(&marker).is_some()
    ), Duration::from_secs(15)).await, "gateways learn the provider's tool and marker");

    // Both profiles reach the provider while the marker is present.
    let body = tools_call(secure_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert!(body.get("error").is_none(), "secure dispatch with marker: {body}");
    assert!(matches!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Client(_))));

    // Simulate a pre-item-7 provider: tombstone its marker (LWW, later HLC wins everywhere).
    assert!(secure.kv().delete(marker.clone()));
    assert!(poll_until(|| secure.kv().get(&marker).is_none() && legacy.kv().get(&marker).is_none(),
                       Duration::from_secs(10)).await, "the tombstone reaches both gateways");

    let body = tools_call(secure_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert_eq!(body["error"]["code"], -32021, "secure profile refuses: {body}");
    assert_eq!(body["error"]["data"]["reason"], "provider_without_caller_context");
    assert!(body["error"]["message"].as_str().unwrap().contains(&provider.node_id().to_string()),
            "the refusal names the provider");

    // The explicit legacy profile still dispatches — as the node, which is what the
    // provider (here: one that would strip nothing) then sees.
    *seen.lock().unwrap() = None;
    let body = tools_call(legacy_port, None, serde_json::json!({"name": "whoami", "arguments": {}})).await;
    assert!(body.get("error").is_none(), "legacy dispatch: {body}");
    assert_eq!(seen.lock().unwrap().clone(), Some(RequestPrincipal::Node(legacy.node_id().clone())),
               "legacy = node-as-caller, exactly the pre-item-7 behaviour");

    secure.shutdown().await;
    legacy.shutdown().await;
    provider.shutdown().await;
}

/// The `authorized_callers` gate (§6.4): a provider restricting the allowlist to the gateway
/// **node** rejects a gateway **client**, although the node is listed — and admits the node's
/// own direct call. Listing the client's principal admits it. A forged envelope from another
/// node is refused as a caller-context error, never as the node. Under `tls` + `compliance`
/// the context is Ed25519-attested by the gateway and verified against its identity key.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn authorized_callers_judges_the_client_not_the_gateway_node() {
    use crate::signal::SignalScope;
    use bytes::{BufMut, BytesMut};

    let cert_dir = std::env::temp_dir().join(format!("gwcaller-gate-{}", alloc_port()));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let tls = |c: &mut GossipConfig| {
        c.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    };
    let http_port = alloc_port();
    let gateway = node(Some(http_port), vec![], |c| { tls(c); c.gateway_auth_token = Some("tok".into()); });
    gateway.start().await.unwrap();
    let boot = vec![gateway.node_id().clone()];
    let provider = node(None, boot.clone(), tls);
    let rogue = node(None, boot, tls);
    provider.start().await.unwrap();
    rogue.start().await.unwrap();

    // Structural readiness: the mesh forms and the provider knows the gateway's identity key
    // (harvested from its cert on connect — what the attestation verifies against).
    let gid = gateway.node_id().clone();
    assert!(poll_until(|| provider.peers().len() >= 2 && rogue.peers().len() >= 2
                && provider.task_ctx.peer_keys.pin().get(&gid).is_some_and(|k| !k.is_empty()),
            Duration::from_secs(30)).await, "mesh forms and the provider anchors the gateway's key");

    // The provider serves `guarded.echo` with a switchable allowlist, judged by
    // `request_authorized` (the caller-context-aware gate). The reply names the outcome.
    let allow: Arc<Mutex<Vec<Arc<str>>>> = Arc::new(Mutex::new(vec![Arc::from(gid.to_string().as_str())]));
    let attested: Arc<Mutex<Option<crate::CallerAttestation>>> = Arc::new(Mutex::new(None));
    {
        let p = Arc::clone(&provider);
        let allow = Arc::clone(&allow);
        let attested = Arc::clone(&attested);
        let mut rx = provider.service().rpc_rx("guarded.echo");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let list = allow.lock().unwrap().clone();
                let reply = match p.request_authorized(&req, &list) {
                    Ok(true) => {
                        if let Ok(RequestPrincipal::Client(c)) = p.request_principal(&req) {
                            *attested.lock().unwrap() = Some(c.attestation);
                        }
                        format!("ok:{}", p.request_principal(&req).unwrap().name())
                    }
                    Ok(false) => "denied:authorized_callers".to_string(),
                    Err(e) => format!("denied:caller_context:{e}"),
                };
                p.service().rpc_respond(&req, reply.into_bytes());
            }
        });
    }

    let client = reqwest::Client::new();
    let call = |payload: &str| {
        let client = client.clone();
        let body = serde_json::json!({"target": provider.node_id().to_string(), "method": "guarded.echo",
                                      "payload_b64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, payload),
                                      "timeout_secs": 10});
        async move {
            let r = client.post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call"))
                .header(axum::http::header::AUTHORIZATION, "Bearer tok")
                .json(&body).send().await.unwrap();
            let status = r.status();
            let v: serde_json::Value = r.json().await.unwrap();
            let text = v["result_b64"].as_str().map(|b| {
                String::from_utf8(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).unwrap()).unwrap()
            });
            (status, text, v)
        }
    };

    // 1. Node listed, client not: the gateway client is rejected …
    let (status, text, v) = call("hi").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(text.as_deref(), Some("denied:authorized_callers"), "the node being listed admits no client");
    // … while the node's own direct call is admitted.
    let reply = gateway.service().rpc_call(provider.node_id().clone(), "guarded.echo", b"hi".to_vec(), Duration::from_secs(10))
        .await.expect("direct reply");
    assert_eq!(String::from_utf8(reply.to_vec()).unwrap(), format!("ok:{gid}"));

    // 2. The client's principal listed: admitted, and the context was signature-attested.
    let legacy = crate::legacy_token_principal(&gid.to_string());
    *allow.lock().unwrap() = vec![Arc::from(legacy.as_str())];
    let (_, text, v) = call("hi").await;
    assert_eq!(text.as_deref(), Some(&*format!("ok:{legacy}")), "{v}");
    assert!(matches!(*attested.lock().unwrap(), Some(crate::CallerAttestation::Signed { .. })),
            "under tls the provider verified the gateway's signature");

    // 3. A forged envelope from another node (unsigned, claiming the token principal):
    // refused as a caller-context error — never judged as the sending node.
    let forged_env = serde_json::json!({"v": 1, "p": legacy, "via": rogue.node_id().to_string(), "s": [], "t": 0}).to_string();
    let mut framed = BytesMut::new();
    framed.put_slice(&[0x00, b'G', b'W', b'C', crate::CALLER_CONTEXT_VERSION]);
    framed.put_u16(forged_env.len() as u16);
    framed.put_slice(forged_env.as_bytes());
    framed.put_slice(b"hi");
    let reply = rogue.service().rpc_call(provider.node_id().clone(), "guarded.echo", framed.freeze(), Duration::from_secs(10))
        .await.expect("forged call still gets a reply");
    let reply = String::from_utf8(reply.to_vec()).unwrap();
    // Refused at the `rpc_rx` boundary itself (review finding 3): the provider loop never
    // sees the request; the boundary answers with the refusal.
    assert!(reply.contains("caller context refused"), "forgery is refused as such: {reply}");
    // Even with an open allowlist a forged context is refused.
    *allow.lock().unwrap() = vec![];
    let reply = rogue.service().rpc_call(provider.node_id().clone(), "guarded.echo",
        { let mut b = BytesMut::new(); b.put_slice(&[0x00, b'G', b'W', b'C', crate::CALLER_CONTEXT_VERSION]);
          b.put_u16(forged_env.len() as u16); b.put_slice(forged_env.as_bytes()); b.put_slice(b"hi"); b.freeze() },
        Duration::from_secs(10)).await.expect("reply");
    assert!(String::from_utf8(reply.to_vec()).unwrap().contains("caller context refused"));
    let _ = SignalScope::Cluster; // keep the import honest under cfg combinations

    rogue.shutdown().await;
    provider.shutdown().await;
    gateway.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// `/a2a` (public by design) still resolves a principal: no bearer ⇒ `anonymous`; a valid
/// bearer ⇒ its principal; an unrecognised bearer ⇒ 401 (never downgraded to anonymous).
#[cfg(feature = "a2a")]
#[tokio::test]
async fn a2a_resolves_the_caller_principal() {
    let http_port = alloc_port();
    let agent = {
        let gossip_port = alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = gossip_port;
        cfg.http_port = Some(http_port);
        cfg.gateway_auth_token = Some("tok".into());
        let a = GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg).with_a2a();
        Arc::new(a)
    };
    agent.start().await.unwrap();
    let _reg = agent.capabilities().advertise_capability(
        crate::capability::Capability::new("demo", "whoami"), Duration::from_secs(5));
    {
        let a = Arc::clone(&agent);
        let mut rx = agent.service().rpc_rx("skill.invoke");
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                let reply = match a.request_principal(&req) {
                    Ok(p) => p.name(),
                    Err(e) => format!("refused:{e}"),
                };
                a.service().rpc_respond(&req, reply.into_bytes());
            }
        });
    }
    let cap_key = format!("cap/{}/demo/whoami", agent.node_id());
    assert!(poll_until(|| agent.kv().get(&cap_key).is_some(), Duration::from_secs(5)).await);

    let client = reqwest::Client::new();
    let send = |bearer: Option<&str>| {
        let client = client.clone();
        let bearer = bearer.map(str::to_owned);
        async move {
            let mut req = client.post(format!("http://127.0.0.1:{http_port}/a2a")).json(&serde_json::json!({
                "jsonrpc": "2.0", "id": 1, "method": "tasks/send",
                "params": {"skillId": "demo/whoami", "message": {"role": "user", "parts": [{"type": "text", "text": "?"}]}},
            }));
            if let Some(b) = bearer { req = req.header(axum::http::header::AUTHORIZATION, format!("Bearer {b}")); }
            let r = req.send().await.unwrap();
            let status = r.status();
            let v: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            (status, v)
        }
    };
    let (status, v) = send(None).await;
    assert_eq!(status, 200);
    assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], crate::PRINCIPAL_ANONYMOUS, "{v}");
    let (status, v) = send(Some("tok")).await;
    assert_eq!(status, 200);
    assert_eq!(v["result"]["artifacts"][0]["parts"][0]["text"], crate::legacy_token_principal(&agent.node_id().to_string()), "{v}");
    let (status, _) = send(Some("wrong")).await;
    assert_eq!(status, 401, "a presented but unrecognised bearer is refused, not anonymised");

    agent.shutdown().await;
}

/// Review finding 1, closed: a raw `/gateway/signal/emit` of RPC-shaped bytes reaches an
/// `rpc_rx` provider **refused** — the gateway node promises an envelope on every RPC it
/// originates, so the bare frame is `Missing`, never the node's own action; the gateway's
/// genuine dispatch on the same route is judged by the client's principal.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn raw_gateway_signal_cannot_pass_as_the_node() {
    use base64::Engine as _;
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("gwraw-{http_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let a = node(Some(http_port), vec![], |c| {
        c.gateway_auth_token = Some("review-token".into());
        c.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    });
    a.start().await.unwrap();
    // Observe the raw signal below the verifying `rpc_rx` boundary too.
    let mut raw_rx = a.task_ctx.signal_handlers.register(Arc::from("review.guarded"));
    let mut rx = a.service().rpc_rx("review.guarded");
    let target = a.node_id().clone();
    let node_allow: Vec<Arc<str>> = vec![Arc::from(target.to_string().as_str())];
    let client = reqwest::Client::new();

    // The genuine gateway dispatch: the client is not the node.
    let safe = client.post(format!("http://127.0.0.1:{http_port}/gateway/rpc/call"))
        .bearer_auth("review-token")
        .json(&serde_json::json!({"target": target.to_string(), "method": "review.guarded", "payload_b64": "eA==", "timeout_secs": 2}));
    let call = tokio::spawn(async move { safe.send().await });
    let req = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    assert!(matches!(a.request_principal(&req).unwrap(), RequestPrincipal::Client(_)));
    assert!(!a.request_authorized(&req, &node_allow).unwrap(), "listing the node admits no client");
    a.service().rpc_respond(&req, bytes::Bytes::new());
    let _ = call.await;
    let _ = raw_rx.recv().await; // drain the genuine one

    // The bypass: RPC-shaped bytes through the raw signal route.
    let mut raw = 1234u64.to_le_bytes().to_vec(); raw.extend_from_slice(b"x");
    let resp = client.post(format!("http://127.0.0.1:{http_port}/gateway/signal/emit"))
        .bearer_auth("review-token")
        .json(&serde_json::json!({"kind": "review.guarded", "scope": format!("node:{target}"),
                                  "payload_b64": base64::engine::general_purpose::STANDARD.encode(raw)}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200, "emitting a plain signal is still allowed");
    // Below the boundary the frame arrives — and is judged Missing, never Node.
    let sig = tokio::time::timeout(Duration::from_secs(3), raw_rx.recv()).await.unwrap().unwrap();
    let req = crate::RpcRequest::from(sig);
    assert_eq!(a.gateway_caller(&req), Err(crate::CallerError::Missing));
    assert!(a.request_authorized(&req, &node_allow).is_err(), "never admitted as the node");
    // The verifying `rpc_rx` never yields it.
    assert!(tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.is_err(),
        "rpc_rx refuses the raw frame instead of yielding it");
    // And the node's own direct call still works, as its own action.
    let caller = Arc::clone(&a);
    let t2 = target.clone();
    let own = tokio::spawn(async move {
        caller.service().rpc_call(t2, "review.guarded", b"x".to_vec(), Duration::from_secs(2)).await
    });
    let req = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    assert_eq!(a.request_principal(&req).unwrap(), RequestPrincipal::Node(target.clone()));
    assert!(a.request_authorized(&req, &node_allow).unwrap(), "the node's own call is admitted by its id");
    a.service().rpc_respond(&req, bytes::Bytes::new());
    let _ = own.await;
    a.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}

/// Review finding 2, closed at the HTTP level: the same bearer position on two gateways
/// resolves to two principals; a named token under an explicit shared issuer resolves to one.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn token_identities_are_qualified_by_the_issuing_gateway() {
    let provider = node(None, vec![], |_| {});
    provider.start().await.unwrap();
    let boot = vec![provider.node_id().clone()];
    let p1 = alloc_port(); let p2 = alloc_port();
    let tokens = |c: &mut GossipConfig| {
        c.gateway_scoped_tokens = vec![crate::GatewayToken { token: "pos".into(), scopes: vec!["*".into()] }];
        c.gateway_named_tokens = vec![crate::GatewayNamedToken { name: "ci-bot".into(), token: "named".into(), scopes: vec!["*".into()] }];
    };
    let g1 = node(Some(p1), boot.clone(), |c| { tokens(c); });
    let g2 = node(Some(p2), boot, |c| { tokens(c); c.gateway_identity_issuer = Some("fleet-gw".into()); });
    g1.start().await.unwrap(); g2.start().await.unwrap();
    let (_tool, seen) = observing_tool(&provider);
    let tool_key = format!("tools/whoami/{}", provider.node_id());
    let marker = format!("sys/caller-context/{}", provider.node_id());
    assert!(poll_until(|| [&g1, &g2].iter().all(|g| g.kv().get(&tool_key).is_some() && g.kv().get(&marker).is_some()), Duration::from_secs(15)).await);
    let who = |port: u16, bearer: &'static str, seen: Arc<Mutex<Option<RequestPrincipal>>>| async move {
        let body = tools_call(port, Some(bearer), serde_json::json!({"name": "whoami", "arguments": {}})).await;
        assert!(body.get("error").is_none(), "{body}");
        let RequestPrincipal::Client(c) = seen.lock().unwrap().clone().unwrap() else { panic!() };
        c.principal
    };
    let a = who(p1, "pos", Arc::clone(&seen)).await;
    let b = who(p2, "pos", Arc::clone(&seen)).await;
    assert_eq!(a, format!("token:{}/#0", g1.node_id()));
    assert_eq!(b, "token:fleet-gw/#0");
    assert_ne!(a, b, "the same list position on two gateways is two identities");
    let n1 = who(p1, "named", Arc::clone(&seen)).await;
    assert_eq!(n1, format!("token:{}/ci-bot", g1.node_id()));
    g1.shutdown().await; g2.shutdown().await; provider.shutdown().await;
}

/// **A named token is a token model.** Found 2026-09-23 while adding the federation verbs'
/// scope test, which failed with a 504 where a 403 was expected — the scope layer had not run
/// at all.
///
/// `gateway_auth`'s open-gateway predicate counted `gateway_auth_token` and
/// `gateway_scoped_tokens` and **not** `gateway_named_tokens`, which 2.10.0 added and
/// `resolve_token` has honoured since. So a deployment whose only credential model was named
/// tokens — the model `GossipConfig`'s own documentation says to *prefer* — ran an open
/// gateway: no bearer required, and `open_gateway_scopes` handing each route exactly the scope
/// it asks for, which is deny-by-default inverted into admit-by-default.
///
/// It hid because the tokens kept working: presenting one was admitted (`resolve_token` knows
/// them), and the only way to see the hole was to present **nothing**. The existing identity
/// test above configures both tables, so it never could.
///
/// Both halves are asserted, because the fix must not simply close the port: no bearer is 401,
/// and a named token still resolves to its own principal.
#[cfg(feature = "compliance")]
#[tokio::test]
async fn named_tokens_alone_still_close_the_gateway() {
    let g = node(Some(alloc_port()), vec![], |c| {
        c.gateway_named_tokens = vec![crate::GatewayNamedToken {
            name: "ci-bot".into(), token: "named".into(), scopes: vec!["kv:read".into()],
        }];
    });
    g.start().await.unwrap();
    let port = g.config().http_port.unwrap();
    let url = format!("http://127.0.0.1:{port}/gateway/kv/keys");
    let http = reqwest::Client::new();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{port}/health")).send().await.is_ok() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let anonymous = http.get(&url).send().await.expect("gateway answered");
    assert_eq!(anonymous.status(), 401, "a named-token deployment is not an open gateway");

    let wrong = http.get(&url).header(axum::http::header::AUTHORIZATION, "Bearer nope").send().await.unwrap();
    assert_eq!(wrong.status(), 401, "an unrecognised bearer is refused");

    let named = http.get(&url).header(axum::http::header::AUTHORIZATION, "Bearer named").send().await.unwrap();
    assert_eq!(named.status(), 200, "the named token still admits what its scopes allow");

    // ...and its scopes still bound it: `kv:read` is not `kv:write`.
    let write = http.post(format!("http://127.0.0.1:{port}/gateway/kv"))
        .header(axum::http::header::AUTHORIZATION, "Bearer named")
        .json(&serde_json::json!({"key": "probe/x", "value_b64": ""}))
        .send().await.unwrap();
    assert_eq!(write.status(), 403, "a read token does not write");
    g.shutdown().await;

    // The write's receipt (doc-coverage run 17, code gap 2): a writer sees rung 2, not a bare ok.
    let g = node(Some(alloc_port()), vec![], |c| {
        c.gateway_named_tokens = vec![crate::GatewayNamedToken {
            name: "writer".into(), token: "w".into(), scopes: vec!["kv:write".into()],
        }];
    });
    g.start().await.unwrap();
    let port = g.config().http_port.unwrap();
    for _ in 0..100 {
        if http.get(format!("http://127.0.0.1:{port}/health")).send().await.is_ok() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let write = http.post(format!("http://127.0.0.1:{port}/gateway/kv"))
        .header(axum::http::header::AUTHORIZATION, "Bearer w")
        .json(&serde_json::json!({"key": "probe/y", "value_b64": "aGk="}))
        .send().await.unwrap();
    assert_eq!(write.status(), 200);
    let body: serde_json::Value = write.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["operation_id"].as_str().is_some_and(|s| !s.is_empty()), "{body}");
    assert_eq!(body["local_durability"], "not_configured", "no persistence on this node: rung 2 says so — {body}");

    // Run 62: a bearer is matched exactly or not at all. Interior whitespace, case, or a scheme
    // the parser does not strip must never admit — and on a closed gateway "not admitted" is
    // 401, never a downgrade to anonymous 200. (Trailing whitespace is deliberately not a case:
    // HTTP strips trailing OWS from a field value before the server sees it — RFC 9110 §5.5,
    // established by Run 61's probe — so `"Bearer w "` reaches the token table as `"w"`.)
    for (hdr, why) in [
        ("Bearer  w", "double space"),
        ("bearer w", "lowercase scheme"),
        ("Bearer W", "wrong case in the token"),
        ("Bearerw", "no separator"),
    ] {
        let r = http.get(format!("http://127.0.0.1:{port}/gateway/kv/keys"))
            .header(axum::http::header::AUTHORIZATION, hdr)
            .send().await.unwrap();
        assert_eq!(r.status(), 401, "{why}: {hdr:?} must not be admitted");
    }

    g.shutdown().await;
}

/// **`/gateway/llm/call` and `/gateway/llm/stream` ran no action preflight.** The raw routes send
/// `llm.invoke` to these two doors (`refuse_protected_kind` names `/gateway/llm/call`), yet neither
/// consulted the evaluator `/mcp` and `/a2a` consult: a policy denying `llm.invoke` was walked
/// around by choosing this door. Both now run `ae_preflight` under their own enforcement points,
/// carry a presented mandate (`_meta.mandate`) to the provider, and record the outcome; a denial
/// is `403 {"error": "policy", …}` on `/call` and an in-stream `{"type": "error", "error":
/// "policy", …}` on `/stream`, and the provider is never reached. Seen failing first: `/call`
/// answered 200 and the provider ran.
#[cfg(all(feature = "llm", feature = "tls"))]
#[tokio::test]
async fn the_llm_doors_run_the_action_preflight() {
    use crate::capability::Capability;
    use crate::{ReferenceEvaluator, Rule};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let gossip_port = alloc_port();
    let http_port = alloc_port();
    let cert_dir = std::env::temp_dir().join(format!("ae-llm-doors-{http_port}"));
    let _ = std::fs::remove_dir_all(&cert_dir);
    let mut cfg = GossipConfig::default();
    cfg.bind_port = gossip_port;
    cfg.http_port = Some(http_port);
    cfg.tls = Some(crate::TlsConfig { auto_cert_dir: cert_dir.clone(), ..Default::default() });
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", gossip_port).unwrap(), cfg));
    let me = agent.node_id().clone();
    let resource = format!("prompt:llm/echo@{me}");
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-llm-doors")
            .with_catalogue("cat-test", "1")
            .map_action("llm.invoke", resource.clone())
            .prohibit(Rule::new("*", "llm.invoke", resource)),
    ));
    agent.start().await.unwrap();
    let _reg = agent.capabilities().advertise_capability(Capability::new("llm", "echo"), Duration::from_secs(30));
    let reached = Arc::new(AtomicUsize::new(0));
    {
        let (agent, reached) = (Arc::clone(&agent), Arc::clone(&reached));
        let mut rx = agent.service().rpc_rx(crate::signal::signal_kind::LLM_INVOKE);
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                reached.fetch_add(1, Ordering::SeqCst);
                agent.service().rpc_respond(&req, br#"{"output":"ran"}"#.to_vec());
            }
        });
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{http_port}");
    let r = client.post(format!("{base}/gateway/llm/call"))
        .json(&serde_json::json!({"ns": "llm", "name": "echo", "input": "x"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 403, "/gateway/llm/call: the evaluator's denial is answered");
    let v: serde_json::Value = r.json().await.unwrap();
    assert_eq!(v["error"], "policy", "{v}");
    assert_eq!(v["data"]["reason"], "action_denied", "{v}");
    assert_eq!(v["data"]["policy_revision"], "rev-llm-doors", "{v}");

    let r = client.post(format!("{base}/gateway/llm/stream"))
        .json(&serde_json::json!({"ns": "llm", "name": "echo", "input": "x"}))
        .send().await.unwrap();
    assert_eq!(r.status(), 200, "SSE commits its status line before the body");
    let text = r.text().await.unwrap();
    let data = text.lines().find_map(|l| l.strip_prefix("data:")).expect("one event").trim();
    let v: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(v["type"], "error", "{v}");
    assert_eq!(v["error"], "policy", "/gateway/llm/stream: the denial is the stream's one event: {v}");
    assert_eq!(v["data"]["reason"], "action_denied", "{v}");

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(reached.load(Ordering::SeqCst), 0, "the provider was never reached through either door");
    agent.shutdown().await;
    let _ = std::fs::remove_dir_all(&cert_dir);
}
