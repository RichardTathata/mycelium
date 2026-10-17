//! Post-360 plan row L — the egress gate's credential half. Each test drives a real request through
//! [`EgressCredentials::send`] to local listeners that record what arrived, so "attached", "refused",
//! "dropped" and "never written out" are read off the wire and the strings, not off the gate's word.

use super::*;
use crate::config::{credential_placeholder, CredentialBinding, CredentialForm, EgressPolicy};
use std::sync::{Arc, Mutex};

const SECRET: &str = "sk-row-L-7f3a9c-never-print-me";

/// A listener that records every request head it receives and answers `200` (or, when `redirect` is
/// set, `307` to it). The recording is the witness: a refused request's listener has none.
async fn recorder(redirect: Option<String>) -> (u16, Arc<Mutex<Vec<String>>>) {
    recorder_with(307, redirect, false).await
}

/// [`recorder`] with the redirect's status chosen, and (`only_root`) a redirect only for `/` — so a
/// redirect can land on the same listener (the same host and port) at another path.
async fn recorder_with(status: u16, redirect: Option<String>, only_root: bool) -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind recorder");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let log = Arc::clone(&log);
            let redirect = redirect.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                log.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).to_string());
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let at_root = head.starts_with("GET / ") || head.starts_with("POST / ");
                let reply = match redirect.filter(|_| !only_root || at_root) {
                    Some(loc) => format!("HTTP/1.1 {status} Redirect\r\nLocation: {loc}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                    None => {
                        let body = r#"{"choices":[{"message":{"content":"ok"}}],"usage":{"total_tokens":1},"model":"m"}"#;
                        format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                    }
                };
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    (port, seen)
}

fn secret_file(tag: &str, contents: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("mycelium-row-l-{}-{tag}", std::process::id()));
    std::fs::write(&p, contents).expect("write the secret file");
    p
}

/// A binding to `host`; a loopback host is pinned to `port` (row L's review, finding 3: a loopback
/// binding must name its port).
fn binding_at(name: &str, host: &str, port: u16, form: CredentialForm, file: &std::path::Path) -> CredentialBinding {
    CredentialBinding { port: Some(port), ..binding(name, host, form, file) }
}

fn binding(name: &str, host: &str, form: CredentialForm, file: &std::path::Path) -> CredentialBinding {
    CredentialBinding {
        port: None,
        name: name.into(),
        host: host.into(),
        path_prefix: None,
        form,
        header: (form == CredentialForm::Header).then(|| "x-api-key".to_string()),
        username: (form == CredentialForm::Basic).then(|| "svc".to_string()),
        value_env: None,
        value_file: Some(file.to_path_buf()),
        allow_plaintext: false,
    }
}

fn gate(bindings: &[CredentialBinding]) -> EgressCredentials {
    EgressCredentials::load(bindings, &EgressPolicy::default()).expect("the gate loads")
}

fn get(url: String, header: &str, value: &str) -> reqwest::Request {
    reqwest::Client::new().get(url).header(header, value).build().unwrap()
}

/// The binding's value goes out on a request to its host — and the placeholder does not.
#[tokio::test]
async fn a_binding_attaches_its_value_at_its_host_in_place_of_the_placeholder() {
    let f = secret_file("attach", &format!("{SECRET}\n"));
    let (port, seen) = recorder(None).await;
    let g = gate(&[binding_at("model", "localhost", port, CredentialForm::Bearer, &f)]);
    let r = g
        .send(get(format!("http://localhost:{port}/v1/chat"), "authorization", &format!("Bearer {}", credential_placeholder("model"))))
        .await
        .expect("sent");
    assert!(r.status().is_success());
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].to_ascii_lowercase().contains(&format!("authorization: bearer {}", SECRET.to_ascii_lowercase())), "{}", seen[0]);
    assert!(!seen[0].contains("mycelium-credential:"), "the placeholder is replaced, not sent: {}", seen[0]);
    assert_eq!(g.uses()[0].attached, 1);
}

/// The header and basic forms, and a placeholder carried in a header the form does not use: the
/// carrier is removed, the form's header is set.
#[tokio::test]
async fn the_header_and_basic_forms_attach_their_own_header() {
    let f = secret_file("forms", SECRET);
    let (port, seen) = recorder(None).await;
    let g = gate(&[
        binding_at("hdr", "localhost", port, CredentialForm::Header, &f),
        binding_at("basic", "localhost", port, CredentialForm::Basic, &f),
    ]);
    g.send(get(format!("http://localhost:{port}/"), "authorization", &format!("Bearer {}", credential_placeholder("hdr")))).await.unwrap();
    g.send(get(format!("http://localhost:{port}/"), "authorization", &credential_placeholder("basic"))).await.unwrap();
    let seen = seen.lock().unwrap().clone();
    let first = seen[0].to_ascii_lowercase();
    assert!(first.contains(&format!("x-api-key: {}", SECRET.to_ascii_lowercase())), "{first}");
    assert!(!first.contains("authorization:"), "the carrier header is removed: {first}");
    use base64::Engine as _;
    let basic = base64::engine::general_purpose::STANDARD.encode(format!("svc:{SECRET}"));
    assert!(seen[1].contains(&format!("Basic {basic}")), "{}", seen[1]);
}

/// A request for any URL the binding does not authorise — another host, a path outside the prefix,
/// another port — is refused **before it is sent**, and counted.
#[tokio::test]
async fn a_request_the_binding_does_not_authorise_is_refused_and_never_sent() {
    let f = secret_file("refuse", SECRET);
    let (port, seen) = recorder(None).await;
    let (other_port, at_other_port) = recorder(None).await;
    let mut scoped = binding_at("scoped", "localhost", port, CredentialForm::Bearer, &f);
    scoped.path_prefix = Some("/v1".into());
    let g = gate(&[binding_at("model", "localhost", port, CredentialForm::Bearer, &f), scoped]);
    let other_host = g.send(get(format!("http://127.0.0.1:{port}/"), "authorization", &credential_placeholder("model"))).await;
    assert!(matches!(other_host, Err(CredentialError::NotAuthorised { .. })), "{other_host:?}");
    let outside = g.send(get(format!("http://localhost:{port}/v10/x"), "authorization", &credential_placeholder("scoped"))).await;
    assert!(matches!(outside, Err(CredentialError::NotAuthorised { .. })), "{outside:?}");
    let dotdot = g.send(get(format!("http://localhost:{port}/v1/../admin"), "authorization", &credential_placeholder("scoped"))).await;
    assert!(matches!(dotdot, Err(CredentialError::NotAuthorised { .. })), "the normalised path is compared: {dotdot:?}");
    let wrong_port = g.send(get(format!("http://localhost:{other_port}/"), "authorization", &credential_placeholder("model"))).await;
    assert!(matches!(wrong_port, Err(CredentialError::NotAuthorised { .. })), "another port on the host: {wrong_port:?}");
    assert!(seen.lock().unwrap().is_empty() && at_other_port.lock().unwrap().is_empty(), "nothing reached a listener");
    let uses = g.uses();
    assert_eq!((uses[0].refused, uses[1].refused), (2, 2));
    assert_eq!((uses[0].attached, uses[1].attached), (0, 0));
    // The plant: under the prefix it is attached.
    g.send(get(format!("http://localhost:{port}/v1/chat"), "authorization", &credential_placeholder("scoped"))).await.expect("under the prefix");
    assert_eq!(g.uses()[1].attached, 1);
}

/// A placeholder naming no binding is refused, and nothing is sent.
#[tokio::test]
async fn an_unknown_placeholder_is_refused_and_never_sent() {
    let f = secret_file("unknown", SECRET);
    let (port, seen) = recorder(None).await;
    let g = gate(&[binding_at("model", "localhost", port, CredentialForm::Bearer, &f)]);
    let r = g.send(get(format!("http://localhost:{port}/"), "x-api-key", &credential_placeholder("nope"))).await;
    assert!(matches!(r, Err(CredentialError::UnknownBinding { ref name }) if name == "nope"), "{r:?}");
    assert!(seen.lock().unwrap().is_empty());
}

/// **A redirect off the host drops the credential.** The first hop (the bound host) gets it; the
/// redirect target (another host) gets neither the value nor the placeholder, and the drop is
/// counted. The plant: a redirect that stays on the bound host and port carries it again.
#[tokio::test]
async fn a_redirect_off_the_bound_host_drops_the_credential() {
    let f = secret_file("redirect", SECRET);
    let (target, at_target) = recorder(None).await;
    let (hop, at_hop) = recorder(Some(format!("http://127.0.0.1:{target}/landing"))).await;
    let g = gate(&[binding_at("model", "localhost", hop, CredentialForm::Header, &f)]);
    let r = g.send(get(format!("http://localhost:{hop}/"), "x-api-key", &credential_placeholder("model"))).await.expect("followed");
    assert!(r.status().is_success());
    assert!(at_hop.lock().unwrap()[0].contains(SECRET), "the bound host received it");
    let landed = at_target.lock().unwrap()[0].clone();
    assert!(!landed.contains(SECRET), "the value left with the redirect: {landed}");
    assert!(!landed.contains("mycelium-credential:"), "the placeholder left with the redirect: {landed}");
    assert!(!landed.to_ascii_lowercase().contains("x-api-key"), "{landed}");
    let u = &g.uses()[0];
    assert_eq!((u.attached, u.dropped_on_redirect), (1, 1));

    // Plant: a redirect that stays on the bound host and port keeps it.
    let (same, at_same) = recorder_with(307, Some("/again".to_string()), true).await;
    let g2 = gate(&[binding_at("model", "localhost", same, CredentialForm::Header, &f)]);
    g2.send(get(format!("http://localhost:{same}/"), "x-api-key", &credential_placeholder("model"))).await.expect("followed");
    let seen = at_same.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "the root, then /again on the same listener");
    assert!(seen[1].starts_with("GET /again") && seen[1].contains(SECRET), "a redirect within the binding re-attaches: {}", seen[1]);
    assert_eq!(g2.uses()[0].attached, 2);
}

/// **A redirect to another port on the bound host drops the credential** (row L's review, finding 3):
/// on loopback each port is a different process, so the port is part of what the binding names.
#[tokio::test]
async fn a_redirect_to_another_port_on_the_bound_host_drops_the_credential() {
    let f = secret_file("port", SECRET);
    let (other, at_other) = recorder(None).await;
    let (hop, at_hop) = recorder(Some(format!("http://localhost:{other}/elsewhere"))).await;
    let g = gate(&[binding_at("model", "localhost", hop, CredentialForm::Bearer, &f)]);
    g.send(get(format!("http://localhost:{hop}/"), "authorization", &credential_placeholder("model"))).await.expect("followed");
    assert!(at_hop.lock().unwrap()[0].contains(SECRET));
    let landed = at_other.lock().unwrap()[0].clone();
    assert!(!landed.contains(SECRET) && !landed.contains("mycelium-credential:"), "another port received it: {landed}");
    assert_eq!(g.uses()[0].dropped_on_redirect, 1);
}

/// **Two bindings on one header are refused** (row L's review, finding 4): only one value could be
/// sent, and counting both over-reported. Nothing is sent, nothing is counted as attached.
#[tokio::test]
async fn two_bindings_that_would_set_one_header_are_refused() {
    let f = secret_file("two", SECRET);
    let (port, seen) = recorder(None).await;
    let g = gate(&[
        binding_at("a", "localhost", port, CredentialForm::Bearer, &f),
        binding_at("b", "localhost", port, CredentialForm::Bearer, &f),
    ]);
    let req = reqwest::Client::new()
        .get(format!("http://localhost:{port}/"))
        .header("authorization", credential_placeholder("a"))
        .header("x-api-key", credential_placeholder("b"))
        .build()
        .unwrap();
    let r = g.send(req).await;
    assert!(matches!(r, Err(CredentialError::ConflictingBindings { .. })), "{r:?}");
    assert!(seen.lock().unwrap().is_empty());
    assert!(g.uses().iter().all(|u| u.attached == 0), "{:?}", g.uses());
}

/// **Only 301, 302, 303, 307 and 308 are followed** (row L's review, finding 5): a 300 is handed back
/// as it arrived, so its `Location` receives nothing — not a replayed body either.
#[tokio::test]
async fn a_300_is_not_followed() {
    let (target, at_target) = recorder(None).await;
    let (hop, _) = recorder_with(300, Some(format!("http://127.0.0.1:{target}/")), false).await;
    let g = gate(&[]);
    let req = reqwest::Client::new().post(format!("http://localhost:{hop}/")).body("the body").build().unwrap();
    let r = g.send(req).await.expect("answered");
    assert_eq!(r.status().as_u16(), 300);
    assert!(at_target.lock().unwrap().is_empty(), "a 300 was followed");
}

/// The node's allow-list still governs: a request the list denies is refused before it is sent, and
/// a redirect to a denied host is refused at the hop.
#[tokio::test]
async fn the_allow_list_still_gates_the_first_url_and_every_hop() {
    let f = secret_file("allow", SECRET);
    let egress = EgressPolicy { allow_hosts: vec!["localhost".into()] };
    let (target, at_target) = recorder(None).await;
    let (hop, _) = recorder(Some(format!("http://127.0.0.1:{target}/"))).await;
    let g = EgressCredentials::load(&[binding_at("model", "localhost", hop, CredentialForm::Bearer, &f)], &egress).unwrap();
    let r = g.send(get(format!("http://localhost:{hop}/"), "authorization", &credential_placeholder("model"))).await;
    assert!(matches!(r, Err(CredentialError::Redirect(_))), "{r:?}");
    assert!(at_target.lock().unwrap().is_empty());
    let r = g.send(get(format!("http://127.0.0.1:{target}/"), "accept", "*/*")).await;
    assert!(matches!(r, Err(CredentialError::EgressDenied { .. })), "{r:?}");
}

/// **The value is in no `Debug`, no `Display`, no error.** Every string the gate and its errors
/// produce is checked for the secret — including the load refusals, which name the binding and the
/// source, never what was read.
#[tokio::test]
async fn the_value_is_in_no_debug_display_or_error() {
    let f = secret_file("debug", SECRET);
    let (port, _) = recorder(None).await;
    let b = binding_at("model", "localhost", port, CredentialForm::Bearer, &f);
    let g = gate(std::slice::from_ref(&b));
    let mut strings = vec![format!("{g:?}"), format!("{b:?}"), format!("{:?}", g.uses())];
    for e in [
        g.send(get(format!("http://127.0.0.1:{port}/"), "authorization", &credential_placeholder("model"))).await.unwrap_err(),
        g.send(get(format!("http://localhost:{port}/"), "authorization", &credential_placeholder("other"))).await.unwrap_err(),
    ] {
        strings.push(format!("{e}"));
        strings.push(format!("{e:?}"));
    }
    // A value that is not a header value: refused by name, the value not echoed.
    let bad = secret_file("bad", &format!("{SECRET}\nsecond-line"));
    let e = EgressCredentials::load(&[binding_at("bad", "localhost", port, CredentialForm::Bearer, &bad)], &EgressPolicy::default()).unwrap_err();
    strings.push(format!("{e}"));
    strings.push(format!("{e:?}"));
    // A sent request's own Debug (what a client error or a trace would print).
    let mut req = get(format!("http://localhost:{port}/"), "authorization", &credential_placeholder("model"));
    req = g.resolve(req, 0).unwrap();
    strings.push(format!("{req:?}"));
    assert!(req.headers()["authorization"].is_sensitive());
    for s in &strings {
        assert!(!s.contains(SECRET), "the secret leaked into: {s}");
    }
}

/// **A secret put in the wrong field is not echoed** (row L's review, finding 2): an operator who
/// writes the value where `value_env` or `value_file` names it gets a refusal naming the binding by
/// index and name, never the field's contents.
#[test]
fn a_secret_in_a_source_field_is_not_echoed_by_a_refusal() {
    for (field, mutate) in [
        ("value_env", Box::new(|b: &mut CredentialBinding| { b.value_file = None; b.value_env = Some(SECRET.into()); }) as Box<dyn Fn(&mut CredentialBinding)>),
        ("value_file", Box::new(|b: &mut CredentialBinding| { b.value_file = Some(SECRET.into()); })),
    ] {
        let mut b = binding("model", "api.example.com", CredentialForm::Bearer, std::path::Path::new("/x"));
        mutate(&mut b);
        let e = EgressCredentials::load(&[b], &EgressPolicy::default()).unwrap_err();
        for shown in [e.to_string(), format!("{e:?}")] {
            assert!(!shown.contains(SECRET), "{field}: the refusal echoed it: {shown}");
            assert!(shown.contains("[0]") && shown.contains("model"), "{field}: names the binding: {shown}");
        }
    }
}

/// The values are read once, at load: an unset variable, an unreadable file and an empty value are
/// refusals that name the binding (by index and name) and which source failed.
#[test]
fn a_value_that_cannot_be_read_is_refused_by_name() {
    let mut b = binding("model", "api.example.com", CredentialForm::Bearer, std::path::Path::new("/nonexistent/row-l"));
    let e = EgressCredentials::load(std::slice::from_ref(&b), &EgressPolicy::default()).unwrap_err().to_string();
    assert!(e.contains("[0]") && e.contains("model") && e.contains("value_file"), "{e}");
    b.value_file = None;
    b.value_env = Some("MYCELIUM_ROW_L_UNSET_7F".into());
    let e = EgressCredentials::load(std::slice::from_ref(&b), &EgressPolicy::default()).unwrap_err().to_string();
    assert!(e.contains("model") && e.contains("value_env"), "{e}");
    let empty = secret_file("empty", "\n");
    let e = EgressCredentials::load(&[binding("model", "api.example.com", CredentialForm::Bearer, &empty)], &EgressPolicy::default()).unwrap_err().to_string();
    assert!(e.contains("empty"), "{e}");
}

/// The LLM backend holds a placeholder, never the key: through the gate the endpoint receives the
/// value; without a gate a placeholder key is refused rather than sent as the bearer.
#[cfg(feature = "llm")]
#[tokio::test]
async fn an_llm_backend_holding_a_placeholder_sends_the_value_only_through_the_gate() {
    use crate::agent::llm::{LlmBackend as _, OpenAiBackend};
    let f = secret_file("llm", SECRET);
    let (port, seen) = recorder(None).await;
    let g = Arc::new(gate(&[binding_at("model", "localhost", port, CredentialForm::Bearer, &f)]));
    let backend = OpenAiBackend::new(format!("http://localhost:{port}/v1"), credential_placeholder("model"), "m");
    let refused = backend.complete("s", "u", 8, 0.0).await;
    assert!(refused.is_err() && seen.lock().unwrap().is_empty(), "no gate: refused, nothing sent");
    let backend = backend.with_credentials(Arc::clone(&g));
    assert!(!format!("{backend:?}").contains(SECRET));
    backend.complete("s", "u", 8, 0.0).await.expect("completed");
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains(SECRET) && seen[0].contains("POST /v1/chat/completions"), "{}", seen[0]);
    // A backend aimed elsewhere with the same placeholder is refused at the gate.
    let (other, at_other) = recorder(None).await;
    let elsewhere = OpenAiBackend::new(format!("http://127.0.0.1:{other}/v1"), credential_placeholder("model"), "m").with_credentials(g);
    assert!(elsewhere.complete("s", "u", 8, 0.0).await.is_err());
    assert!(at_other.lock().unwrap().is_empty());
}

/// An `OpenAiBackend`'s `Debug` never prints a literal key (it had no `Debug` before row L).
#[cfg(feature = "llm")]
#[test]
fn an_llm_backend_debug_redacts_a_literal_key() {
    let b = crate::agent::llm::OpenAiBackend::new("https://api.example/v1", SECRET, "m");
    assert!(!format!("{b:?}").contains(SECRET));
}

/// `start()` reads the values once into the agent's gate, and refuses a value it cannot read — by
/// the binding's index and name.
#[tokio::test]
async fn start_loads_the_gate_and_refuses_an_unreadable_binding() {
    let f = secret_file("start", SECRET);
    let mut cfg = crate::config::GossipConfig { bind_port: crate::test_util::alloc_port(), ..Default::default() };
    cfg.egress_credentials = vec![binding("model", "api.example.com", CredentialForm::Bearer, &f)];
    let plain = crate::GossipAgent::new(crate::NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), crate::config::GossipConfig::default());
    assert!(matches!(
        plain.guarantee_report().entry("egress.credentials").expect("registered").resolution,
        crate::agent::guarantee::Resolution::NotConfigured { .. }
    ));
    let agent = crate::GossipAgent::new(crate::NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), cfg.clone());
    assert!(agent.egress_credentials().is_none(), "nothing is read before start");
    agent.start().await.expect("starts");
    assert_eq!(agent.egress_credentials().expect("loaded at start").names(), vec!["model".to_string()]);
    assert_eq!(
        agent.guarantee_report().entry("egress.credentials").expect("registered").resolution,
        crate::agent::guarantee::Resolution::Enforced
    );
    agent.shutdown().await;

    cfg.bind_port = crate::test_util::alloc_port();
    cfg.egress_credentials[0].value_file = None;
    cfg.egress_credentials[0].value_env = Some("MYCELIUM_ROW_L_START_UNSET_7F".into());
    let agent = crate::GossipAgent::new(crate::NodeId::new("127.0.0.1", cfg.bind_port).unwrap(), cfg);
    let e = agent.start().await.unwrap_err().to_string();
    assert!(e.contains("egress_credentials") && e.contains("model") && e.contains("value_env"), "{e}");
}
