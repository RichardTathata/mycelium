//! Identity-and-lifecycle N3: `--format json` for `explain`, `declare` and `tls`. The row's exit is *every subcommand's
//! JSON parsed against its golden*: each case runs the built `mycelium` binary, parses its stdout as JSON (one document;
//! `declare` one line per event), and compares it with its golden under `tests/fixtures/cli/`.
//!
//! **The goldens.** A golden is the document itself. Where a value differs from run to run, the golden says how:
//! `${CA}`, `${OUT}` and `${GATEWAY}` inside a string are replaced by this run's CA directory, output directory and
//! gateway URL before the string is compared exactly; `"<sha256>"` is 64 lowercase hex characters (a fingerprint of a
//! freshly generated key, cross-checked against `tls pin` by the test); `"<contains:TEXT>"` is a string holding `TEXT`.
//! Every other value, and every object's set of keys and every array's length, must be equal: a key added, dropped or
//! renamed fails here before it surprises a reader.
#![cfg(feature = "cli")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn mycelium() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mycelium"))
}

fn run(args: &[&str]) -> Output {
    mycelium().args(args).output().expect("the mycelium binary runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).expect("stdout is UTF-8")
}

fn golden(name: &str) -> Value {
    let path = Path::new("tests/fixtures/cli").join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// This run's values for `${CA}`, `${OUT}` and `${GATEWAY}`.
type Vars<'a> = &'a [(&'a str, &'a str)];

/// Compare `got` with `want` as the module doc describes, naming the first difference by its JSON pointer.
fn matches(want: &Value, got: &Value, at: &str, vars: Vars<'_>) -> Result<(), String> {
    match (want, got) {
        (Value::String(w), _) if w.starts_with('<') && w.ends_with('>') => {
            let g = got.as_str().ok_or_else(|| format!("{at}: want a string for {w}, got {got}"))?;
            let ok = match w.as_str() {
                "<sha256>" => g.len() == 64 && g.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                p if p.starts_with("<contains:") => g.contains(&p["<contains:".len()..p.len() - 1]),
                other => return Err(format!("{at}: unknown placeholder {other}")),
            };
            if ok { Ok(()) } else { Err(format!("{at}: {g:?} is not {w}")) }
        }
        (Value::String(w), Value::String(g)) if w.contains("${") => {
            let want = vars.iter().fold(w.clone(), |acc, (k, v)| acc.replace(&format!("${{{k}}}"), v));
            if want.contains("${") {
                return Err(format!("{at}: {w} names a variable this test does not set"));
            }
            if *g == want { Ok(()) } else { Err(format!("{at}: {g:?}, the golden has {want:?}")) }
        }
        (Value::Object(w), Value::Object(g)) => {
            let (wk, gk): (Vec<_>, Vec<_>) = (w.keys().collect(), g.keys().collect());
            if wk != gk {
                return Err(format!("{at}: keys {gk:?}, the golden has {wk:?}"));
            }
            w.iter().try_for_each(|(k, v)| matches(v, &g[k], &format!("{at}/{k}"), vars))
        }
        (Value::Array(w), Value::Array(g)) => {
            if w.len() != g.len() {
                return Err(format!("{at}: {} element(s), the golden has {}", g.len(), w.len()));
            }
            w.iter().zip(g).enumerate().try_for_each(|(i, (w, g))| matches(w, g, &format!("{at}/{i}"), vars))
        }
        _ if want == got => Ok(()),
        _ => Err(format!("{at}: {got}, the golden has {want}")),
    }
}

fn assert_golden(name: &str, got: &Value, vars: Vars<'_>) {
    if let Err(e) = matches(&golden(name), got, "", vars) {
        panic!("{name}: {e}\n{}", serde_json::to_string_pretty(got).unwrap());
    }
}

/// One JSON document on stdout, exit 0.
fn document(args: &[&str]) -> Value {
    let o = run(args);
    assert!(o.status.success(), "{args:?}: {:?}\n{}", o.status, String::from_utf8_lossy(&o.stderr));
    serde_json::from_str(&stdout(&o)).unwrap_or_else(|e| panic!("{args:?}: stdout is not one JSON document ({e}):\n{}", stdout(&o)))
}

/// The matcher itself refuses what it should: a changed value, an added key, a shorter array, a placeholder whose
/// value does not fit — so a golden comparison that passes is one that could have failed.
#[test]
fn the_golden_matcher_refuses_a_shape_change() {
    let want = serde_json::json!({ "a": 1, "h": "<sha256>", "p": "${OUT}/x.pem", "r": "<contains:nobody>", "l": [1, 2] });
    let vars: Vars<'_> = &[("OUT", "/tmp/o")];
    let good = || serde_json::json!({ "a": 1, "h": "0".repeat(64), "p": "/tmp/o/x.pem", "r": "error: nobody offers", "l": [1, 2] });
    assert!(matches(&want, &good(), "", vars).is_ok());
    let mut bads = Vec::new();
    for (k, v) in [("a", serde_json::json!(2)), ("h", serde_json::json!("XYZ")), ("p", serde_json::json!("/elsewhere/o/x.pem")),
                   ("r", serde_json::json!("error: someone")), ("l", serde_json::json!([1])), ("extra", serde_json::json!(0))] {
        let mut b = good();
        b[k] = v;
        bads.push(b);
    }
    for bad in bads {
        assert!(matches(&want, &bad, "", vars).is_err(), "{bad}");
    }
    assert!(matches(&want, &good(), "", &[]).is_err(), "a variable the test does not set fails, never passes");
}

/// `explain <trace> --format json`: the targets, steps, rules and counts of the text form, as one document.
#[test]
fn explain_a_trace_as_json_matches_its_golden() {
    let doc = document(&["explain", "tests/fixtures/cli/trace.jsonl", "--catalogue", "tests/fixtures/cli/catalogue.json", "--format", "json"]);
    assert_golden("explain.json", &doc, &[]);
    let only = document(&["explain", "tests/fixtures/cli/trace.jsonl", "--target", "route/optimize", "--format", "json"]);
    assert_golden("explain-target.json", &only, &[]);
    // `--format text` is the text form, unchanged; anything else is a usage error with no document.
    let text = run(&["explain", "tests/fixtures/cli/trace.jsonl", "--format", "text"]);
    assert!(text.status.success() && stdout(&text).starts_with("text/echo\n"), "{}", stdout(&text));
    let bad = run(&["explain", "tests/fixtures/cli/trace.jsonl", "--format", "yaml"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
}

/// `explain --correlation <id> --format json` and `explain --closure <hash> --format json` keep the closure reader's
/// schema id, `mycelium.closure/1`, with `view` saying which.
#[test]
fn explain_a_correlation_and_a_closure_as_json_match_their_goldens() {
    let chain = document(&["explain", "--correlation", "run-1", "tests/fixtures/cli/records.jsonl", "--format", "json"]);
    assert_golden("explain-correlation.json", &chain, &[]);
    // The prefix a chain prints, as the text form takes it.
    let root = chain["records"][0]["hash"].as_str().unwrap().to_string();
    let closure = document(&["explain", "--closure", &root[..12], "tests/fixtures/cli/records.jsonl", "--fleet", "2", "--format", "json"]);
    assert_golden("explain-closure.json", &closure, &[]);
    let missing = document(&["explain", "--closure", &"0".repeat(64), "tests/fixtures/cli/records.jsonl", "--format", "json"]);
    assert_golden("explain-closure-missing.json", &missing, &[]);
    let refused = run(&["explain", "--closure", "0123", "tests/fixtures/cli/records.jsonl", "--format", "json"]);
    assert_eq!(refused.status.code(), Some(2), "a prefix too short is refused, as in the text form");
    assert!(refused.stdout.is_empty());
}

#[cfg(feature = "tls")]
mod tls {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mycelium-cli-json-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// Every string the document holds, at any depth.
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|v| strings(v, out)),
            Value::Object(o) => o.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }

    /// Each private key file in `dirs`, as hex: its whole PKCS#8 DER, its 32-byte Ed25519 seed and the raw public key
    /// it derives — the 64-hex values a leaked key would print as (#767's review, L3).
    fn key_hexes(dirs: &[&Path]) -> Vec<String> {
        use base64::Engine as _;
        let mut out = Vec::new();
        for dir in dirs {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let text = std::fs::read_to_string(&path).unwrap();
                if !text.contains("PRIVATE KEY-----") {
                    continue;
                }
                let b64: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
                let der = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
                let seed: [u8; 32] = der[der.len() - 32..].try_into().unwrap();
                let public = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
                out.extend([hex(&der), hex(&seed), hex(&public)]);
            }
        }
        assert!(!out.is_empty(), "the directories hold the keys just written");
        out
    }

    /// **No secrets.** A `tls` document names files and fingerprints only: no PEM block of any kind, no line of any
    /// key or certificate file written, no hex form of a key's DER, seed or raw public key, and every string a path, a
    /// 64-hex fingerprint, or one of the names the command was given.
    fn assert_no_secrets(doc: &Value, raw: &str, dirs: &[&Path], given: &[&str]) {
        assert!(!raw.contains("-----BEGIN") && !raw.contains("PRIVATE KEY"), "a PEM block in the output:\n{raw}");
        for dir in dirs {
            for entry in std::fs::read_dir(dir).unwrap() {
                let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
                for line in text.lines().filter(|l| !l.starts_with("-----") && l.len() >= 16) {
                    assert!(!raw.contains(line), "a line of a written file is in the output: {line}");
                }
            }
        }
        for k in key_hexes(dirs) {
            assert!(!raw.contains(&k), "a key's hex form is in the output: {k}");
        }
        let mut all = Vec::new();
        strings(doc, &mut all);
        for s in all {
            let hex = s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit());
            let path = dirs.iter().any(|d| s.starts_with(&d.display().to_string()));
            let named = given.contains(&s.as_str()) || ["mycelium.tls/1", "ca", "issue", "pin", "node", "client"].contains(&s.as_str());
            assert!(hex || path || named, "a string that is neither a path, a fingerprint nor a given name: {s:?}");
        }
    }

    /// `tls ca`, `tls issue --node`, `tls issue --client` and `tls pin`, each with `--format json`: the files written
    /// and each certificate's SPKI SHA-256, matching the goldens; each issued fingerprint is the one `tls pin` prints;
    /// and no document carries key material.
    #[test]
    fn each_tls_subcommand_as_json_matches_its_golden_and_carries_no_secret() {
        let (ca, out) = (scratch("ca"), scratch("out"));
        let (ca_s, out_s) = (ca.display().to_string(), out.display().to_string());
        let vars: Vars<'_> = &[("CA", &ca_s), ("OUT", &out_s)];
        let check = |args: &[&str], golden_name: &str, given: &[&str]| -> Value {
            let o = run(args);
            assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
            let raw = stdout(&o);
            let doc: Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{args:?}: not one JSON document ({e}):\n{raw}"));
            assert_golden(golden_name, &doc, vars);
            let dirs: Vec<&Path> = [ca.as_path(), out.as_path()].into_iter().filter(|d| d.exists()).collect();
            assert_no_secrets(&doc, &raw, &dirs, given);
            doc
        };
        let minted = check(&["tls", "ca", "--out", &ca_s, "--format", "json"], "tls-ca.json", &[]);
        let node = check(&["tls", "issue", "--ca-dir", &ca_s, "--node", "127.0.0.1:7971", "--out", &out_s, "--format", "json"], "tls-issue-node.json", &["127.0.0.1:7971"]);
        assert_eq!(node["ca_spki_sha256"], minted["spki_sha256"], "the CA the node was issued under");
        // `--format` before the subcommand's own flags, too.
        let client = check(&["tls", "--format", "json", "issue", "--ca-dir", &ca_s, "--client", "svc", "--issuer", "fleet", "--out", &out_s], "tls-issue-client.json", &["svc", "fleet", "token:fleet/svc"]);
        let cert = node["cert"].as_str().unwrap().to_string();
        let pinned = check(&["tls", "pin", &cert, "--format", "json"], "tls-pin.json", &[]);
        assert_eq!(pinned["spki_sha256"], node["spki_sha256"], "issue's fingerprint is the pin");
        let text = run(&["tls", "pin", &cert]);
        assert_eq!(stdout(&text).trim(), node["spki_sha256"].as_str().unwrap(), "and the text form's line");
        // The client's fingerprint is its certificate's pin too (#767's review, L3).
        let client_pin = run(&["tls", "pin", client["cert"].as_str().unwrap()]);
        assert!(client_pin.status.success(), "{}", String::from_utf8_lossy(&client_pin.stderr));
        assert_eq!(stdout(&client_pin).trim(), client["spki_sha256"].as_str().unwrap(), "the client's fingerprint is its pin");
        assert_ne!(client["spki_sha256"], node["spki_sha256"]);
        // A refusal is on stderr with its exit code, and no document: a second CA, a key issued over without --force.
        for args in [
            vec!["tls", "ca", "--out", &ca_s, "--format", "json"],
            vec!["tls", "issue", "--ca-dir", &ca_s, "--node", "127.0.0.1:7971", "--out", &out_s, "--format", "json"],
            vec!["tls", "pin", &format!("{ca_s}/ca-cert.pem"), "--format", "json"],
            vec!["tls", "pin", &cert, "--format"],
        ] {
            let o = run(&args);
            assert_eq!(o.status.code(), Some(2), "{args:?}");
            assert!(o.stdout.is_empty(), "{args:?}: {}", stdout(&o));
        }
        let _ = (std::fs::remove_dir_all(&ca), std::fs::remove_dir_all(&out));
    }

    /// A directory another local user can write is named in `warnings` by `issue --client` as by `issue --node`
    /// (#767's review, L2). Seen failing first: `--client` printed `"warnings": []`.
    #[cfg(unix)]
    #[test]
    fn issue_client_warns_of_a_directory_others_can_write() {
        use std::os::unix::fs::PermissionsExt;
        let (ca, out) = (scratch("warn-ca"), scratch("warn-out"));
        let (ca_s, out_s) = (ca.display().to_string(), out.display().to_string());
        assert!(run(&["tls", "ca", "--out", &ca_s]).status.success());
        std::fs::create_dir_all(&out).unwrap();
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o777)).unwrap();
        for (args, what) in [
            (vec!["tls", "issue", "--ca-dir", &ca_s, "--client", "svc", "--issuer", "fleet", "--out", &out_s, "--format", "json"], "client"),
            (vec!["tls", "issue", "--ca-dir", &ca_s, "--node", "127.0.0.1:7972", "--out", &out_s, "--format", "json"], "node"),
        ] {
            let o = run(&args);
            assert!(o.status.success(), "{what}: {}", String::from_utf8_lossy(&o.stderr));
            let doc: Value = serde_json::from_str(&stdout(&o)).unwrap();
            let warnings = doc["warnings"].as_array().unwrap();
            assert_eq!(warnings.len(), 1, "{what}: {doc:#}");
            assert!(warnings[0].as_str().unwrap().contains("writable by group or others"), "{what}: {doc:#}");
        }
        let text = run(&["tls", "issue", "--ca-dir", &ca_s, "--client", "svc", "--issuer", "fleet", "--out", &out_s, "--force"]);
        assert!(String::from_utf8_lossy(&text.stderr).contains("warning: TLS:"), "the text form warns on stderr");
        let _ = (std::fs::remove_dir_all(&ca), std::fs::remove_dir_all(&out));
    }
}

#[cfg(feature = "gateway")]
mod declare {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::Arc;
    use std::time::Duration;

    use mycelium::{GossipAgent, GossipConfig, NodeId};

    /// Below the OS ephemeral range and bind-verified, as `tests/exposure_doors.rs` allocates (`test_util` needs a
    /// feature this target's CI step does not enable).
    fn alloc_port() -> u16 {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let seed = std::process::id().wrapping_mul(2_654_435_761) % 16_000 + 1;
        let _ = NEXT.compare_exchange(0, seed, Ordering::Relaxed, Ordering::Relaxed);
        loop {
            let p = (16_000 + NEXT.fetch_add(1, Ordering::Relaxed) % 16_000) as u16;
            if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
                return p;
            }
        }
    }

    async fn gateway_node() -> (Arc<GossipAgent>, String) {
        let (port, http) = (alloc_port(), alloc_port());
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.http_port = Some(http);
        cfg.gateway_auth_token = Some("s3cret".into());
        let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        agent.start().await.expect("starts");
        let base = format!("http://127.0.0.1:{http}");
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client.get(format!("{base}/health")).send().await.is_ok_and(|r| r.status().is_success()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        (agent, base)
    }

    fn project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mycelium-cli-json-declare-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("units")).unwrap();
        std::fs::write(dir.join("units/a.toml"), "principal = \"a\"\n\n[[capability]]\nns = \"demo\"\nname = \"thing\"\nttl_secs = 30\n").unwrap();
        dir
    }

    /// Each line of stdout, parsed as one JSON object — the `declare --format json` contract.
    fn lines(raw: &str) -> Vec<Value> {
        raw.lines().map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not one JSON object per line ({e}): {l}"))).collect()
    }

    fn declare_args(dir: &Path, gateway: &str, extra: &[&str]) -> Vec<String> {
        let mut a: Vec<String> = ["declare", &dir.display().to_string(), "--gateway", gateway, "--token", "s3cret", "--format", "json"]
            .iter().map(|s| s.to_string()).collect();
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    }

    fn sigint(pid: u32) {
        assert!(Command::new("kill").args(["-INT", &pid.to_string()]).status().unwrap().success());
    }

    /// `declare --format json` once: one line, the `declared` event; a unit `wire-check` refuses: one line, the
    /// `refused` event, exit 1; `--watch`: the `declared` event, then at SIGINT `retracting` and `retracted`.
    #[tokio::test(flavor = "multi_thread")]
    async fn declare_as_json_matches_its_goldens_once_refused_and_watched() {
        let (agent, base) = gateway_node().await;
        let vars: Vars<'_> = &[("GATEWAY", &base)];
        let dir = project("once");
        let once = tokio::task::spawn_blocking({
            let a = declare_args(&dir, &base, &[]);
            move || mycelium().args(&a).output().unwrap()
        }).await.unwrap();
        assert!(once.status.success(), "{}", String::from_utf8_lossy(&once.stderr));
        let got = lines(&stdout(&once));
        assert_eq!(got.len(), 1, "once is one line");
        assert_golden("declare.json", &got[0], vars);

        let refused_dir = project("refused");
        std::fs::write(refused_dir.join("units/b.toml"), "principal = \"b\"\n\n[[requirement]]\nns = \"nobody\"\nname = \"offers\"\n").unwrap();
        let refused = tokio::task::spawn_blocking({
            let a = declare_args(&refused_dir, &base, &[]);
            move || mycelium().args(&a).output().unwrap()
        }).await.unwrap();
        assert_eq!(refused.status.code(), Some(1));
        let got = lines(&stdout(&refused));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_golden("declare-refused.json", &got[0], vars);

        // Watched: the first event, then SIGINT, then the retraction. The child's stdout is read a line at a time on a
        // blocking thread so the node in this process keeps serving.
        let mut child = mycelium().args(declare_args(&dir, &base, &["--watch", "--poll-ms", "100", "--lease-secs", "3"])).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let pid = child.id();
        let out = child.stdout.take().unwrap();
        let read = tokio::task::spawn_blocking(move || {
            let mut reader = BufReader::new(out);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            sigint(pid);
            let mut rest = String::new();
            reader.read_to_string(&mut rest).unwrap();
            let status = child.wait().unwrap();
            (first + &rest, status)
        });
        let (raw, status) = tokio::time::timeout(Duration::from_secs(30), read).await.expect("the watch stops at SIGINT").unwrap();
        assert!(status.success(), "{status:?}\n{raw}");
        let got = lines(&raw);
        assert_eq!(got.len(), 3, "{raw}");
        assert_golden("declare-watch-declared.json", &got[0], vars);
        assert_golden("declare-watch-retracting.json", &got[1], vars);
        assert_golden("declare-watch-retracted.json", &got[2], vars);

        agent.shutdown().await;
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&refused_dir));
    }

    /// A gateway that answers `POST /gateway/units/declare` after `delay` with handle `h-1`, a heartbeat or a `DELETE`
    /// with `{}`, each on its own connection; with `stop_after_declare` it stops listening once it has answered the
    /// declaration, so the retraction finds no gateway. Each request line is sent on the channel as it arrives.
    fn fake_gateway(delay: Duration, stop_after_declare: bool) -> (String, std::sync::mpsc::Receiver<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                    if head.is_empty() {
                        head = line.trim().to_string();
                    }
                }
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
                let _ = tx.send(head.clone());
                let declare = head.starts_with("POST /gateway/units/declare");
                if declare {
                    std::thread::sleep(delay);
                }
                let reply = if declare { r#"{"handle_id":"h-1","not_enforced":[]}"# } else { "{}" };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
                let _ = stream.flush();
                if declare && stop_after_declare {
                    return; // the listener is dropped: a connection is refused from here on
                }
            }
        });
        (base, rx)
    }

    /// The watch's stdout and exit status, with SIGINT sent once `when` returns.
    fn watch_until(dir: &Path, gateway: &str, when: impl FnOnce(&mut BufReader<std::process::ChildStdout>) -> String) -> (String, std::process::ExitStatus) {
        let mut child = mycelium().args(declare_args(dir, gateway, &["--watch", "--poll-ms", "100", "--lease-secs", "90"]))
            .stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let before = when(&mut reader);
        sigint(child.id());
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        (before + &rest, child.wait().unwrap())
    }

    /// **The gateway down at retract time** (#767's review, M1): `retracting` before the retraction, and `retracted`
    /// after it naming the unit whose DELETE nobody answered, exit 1 — never a count of declarations as if retracted.
    /// Seen failing first: the event said `"declarations": 1` before the retraction ran, and its error was discarded.
    #[test]
    fn a_retraction_the_gateway_does_not_answer_is_reported_failed() {
        let (base, _seen) = fake_gateway(Duration::ZERO, true);
        let dir = project("retract-down");
        let (raw, status) = watch_until(&dir, &base, |r| {
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            first
        });
        assert_eq!(status.code(), Some(1), "{status:?}\n{raw}");
        let got = lines(&raw);
        assert_eq!(got.len(), 3, "{raw}");
        assert_eq!(got[0]["event"], "declared", "{raw}");
        assert_eq!(got[1], serde_json::json!({ "schema": "mycelium.declare/1", "event": "retracting", "declarations": 1 }));
        assert_golden("declare-watch-retract-failed.json", &got[2], &[]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A Ctrl-C during the first tick** is a retraction (#767's review, L5): the SIGINT is sent while the gateway is
    /// still holding the first `POST`, and the watch finishes the tick, prints it, and retracts. Seen failing first: the
    /// handler was installed after the first tick, so the signal killed the process with nothing retracted.
    #[test]
    fn a_ctrl_c_during_the_first_tick_retracts() {
        let (base, seen) = fake_gateway(Duration::from_millis(1500), false);
        let dir = project("first-tick");
        let (raw, status) = watch_until(&dir, &base, |_| {
            let first = seen.recv_timeout(Duration::from_secs(30)).expect("the first tick's request reaches the gateway");
            assert!(first.starts_with("POST /gateway/units/declare"), "{first}");
            String::new()
        });
        assert!(status.success(), "{status:?}\n{raw}");
        let got = lines(&raw);
        assert_eq!(got.iter().map(|v| v["event"].as_str().unwrap()).collect::<Vec<_>>(), ["declared", "retracting", "retracted"], "{raw}");
        assert_eq!(got[2]["retracted"], serde_json::json!(["a"]));
        assert!(seen.recv_timeout(Duration::from_secs(5)).unwrap().starts_with("DELETE /gateway/capability/h-1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Identity-and-lifecycle N5: `skills list` and `skills get fleet` as `mycelium.skills/1`. `${GUIDE}` in the golden is
/// the guide's file with the binary's release tag filled in, so the document's `text` is the embedded guide, whole;
/// the text form prints the same bytes and nothing else.
#[test]
fn skills_as_json_match_their_goldens_and_the_text_form_is_the_guide() {
    let guide = std::fs::read_to_string("docs/reference/agent-skills/fleet.md")
        .expect("the guide")
        .replace("{{MYCELIUM_TAG}}", &format!("v{}", env!("CARGO_PKG_VERSION")));
    assert_golden("skills-list.json", &document(&["skills", "list", "--format", "json"]), &[]);
    assert_golden("skills-get.json", &document(&["skills", "get", "fleet", "--format", "json"]), &[("GUIDE", &guide)]);

    for args in [&["skills", "get", "fleet"][..], &["skills", "get", "fleet", "--format", "text"]] {
        let text = run(args);
        assert!(text.status.success() && text.stderr.is_empty(), "{:?}", text);
        assert_eq!(stdout(&text), guide, "the text form is the guide's bytes, the tag filled in");
    }
    assert!(!guide.contains("{{") && guide.contains(&format!("/tags/v{}.tar.gz", env!("CARGO_PKG_VERSION"))));
    let list = run(&["skills", "list"]);
    assert!(list.status.success() && stdout(&list).starts_with("fleet  "), "{:?}", list);
}

/// A refusal is stderr and a non-zero exit with nothing on stdout, in either format: a guide the binary does not
/// serve (named, with the ones it does), no verb, a verb it does not have, a bad `--format`. `--help` is exit 0.
#[test]
fn skills_refuses_on_stderr_with_nothing_on_stdout() {
    for args in [&["skills", "get", "nope"][..], &["skills", "get", "nope", "--format", "json"], &["skills"], &["skills", "get"],
                 &["skills", "put", "fleet"], &["skills", "get", "fleet", "--format", "yaml"], &["skills", "list", "--format"],
                 &["skills", "get", "fleet", "extra"], &["skills", "list", "--verbose"]] {
        let o = run(args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(o.stdout.is_empty(), "{args:?}: {}", stdout(&o));
        assert!(!o.stderr.is_empty(), "{args:?}");
    }
    let unknown = run(&["skills", "get", "nope"]);
    let err = String::from_utf8_lossy(&unknown.stderr).to_string();
    assert!(err.contains("no guide named `nope`") && err.contains("fleet"), "{err}");
    let help = run(&["skills", "--help"]);
    assert!(help.status.success() && help.stdout.is_empty() && String::from_utf8_lossy(&help.stderr).contains("mycelium skills get <name>"));
    // The node's own usage lists it, so `--help` and the guide's table name the same subcommands.
    let usage = run(&["--help"]);
    assert!(String::from_utf8_lossy(&usage.stderr).contains("mycelium skills list | get <name> [--format text|json]  (agent guides)"), "{}", String::from_utf8_lossy(&usage.stderr));
    assert!(String::from_utf8_lossy(&help.stderr).contains("Not the prompt skills a node serves"), "the two senses of `skills` are told apart where the command is");
}

/// **The guide's output rule, against the binary** (#770's review, F1). The agent guide says a usage error leaves
/// stdout empty, and that a run that finds a problem still prints its document and exits 1. `doctor --format json`
/// over a configuration `start()` would refuse is the second: exit 1, one `mycelium.doctor/1` document on stdout
/// carrying the refusal. A usage error of the same command is the first: exit 2, nothing on stdout.
#[test]
fn a_doctor_that_would_refuse_prints_its_document_and_exits_1() {
    let dir = std::env::temp_dir().join(format!("mycelium-cli-json-doctor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("node.toml");
    // The secure profile with nothing it requires configured: `start()` refuses, naming each unmet guarantee.
    std::fs::write(&config, "profile = \"secure-single-domain\"\n").unwrap();
    let o = run(&["doctor", config.to_str().unwrap(), "--format", "json"]);
    assert_eq!(o.status.code(), Some(1), "{}", String::from_utf8_lossy(&o.stderr));
    let doc: Value = serde_json::from_str(&stdout(&o)).unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}", stdout(&o)));
    assert_eq!(doc["schema"], "mycelium.doctor/1");
    assert!(doc["refusal"].is_string() || doc["refusal"].is_object(), "the document carries the refusal: {}", doc["refusal"]);

    let usage = run(&["doctor", "--format"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(usage.stdout.is_empty() && !usage.stderr.is_empty(), "a usage error leaves stdout empty");
    let _ = std::fs::remove_dir_all(&dir);
}
