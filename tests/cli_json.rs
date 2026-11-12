//! Identity-and-lifecycle N3: `--format json` for `explain`, `declare` and `tls`. The row's exit is *every subcommand's
//! JSON parsed against its golden*: each case runs the built `mycelium` binary, parses its stdout as JSON (one document;
//! `declare` one line per event), and compares it with its golden under `tests/fixtures/cli/`.
//!
//! **The goldens.** A golden is the document itself. Where a value differs from run to run — a path under a scratch
//! directory, a fingerprint of a freshly generated key, a gateway's port — the golden holds a placeholder string
//! instead: `"<sha256>"` (64 lowercase hex characters), `"<path:SUFFIX>"` (a string ending in `SUFFIX`), `"<gateway>"`
//! (the URL this test's node serves) and `"<string>"` (any string). Every other value, and every object's set of keys
//! and every array's length, must be equal: a key added, dropped or renamed fails here before it surprises a reader.
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

/// Compare `got` with `want` as the module doc describes, naming the first difference by its JSON pointer.
fn matches(want: &Value, got: &Value, at: &str, gateway: Option<&str>) -> Result<(), String> {
    match (want, got) {
        (Value::String(w), _) if w.starts_with('<') && w.ends_with('>') => {
            let g = got.as_str().ok_or_else(|| format!("{at}: want a string for {w}, got {got}"))?;
            let ok = match w.as_str() {
                "<sha256>" => g.len() == 64 && g.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                "<string>" => true,
                "<gateway>" => Some(g) == gateway,
                p if p.starts_with("<path:") => g.ends_with(&p["<path:".len()..p.len() - 1]),
                other => return Err(format!("{at}: unknown placeholder {other}")),
            };
            if ok { Ok(()) } else { Err(format!("{at}: {g:?} is not {w}")) }
        }
        (Value::Object(w), Value::Object(g)) => {
            let (wk, gk): (Vec<_>, Vec<_>) = (w.keys().collect(), g.keys().collect());
            if wk != gk {
                return Err(format!("{at}: keys {gk:?}, the golden has {wk:?}"));
            }
            w.iter().try_for_each(|(k, v)| matches(v, &g[k], &format!("{at}/{k}"), gateway))
        }
        (Value::Array(w), Value::Array(g)) => {
            if w.len() != g.len() {
                return Err(format!("{at}: {} element(s), the golden has {}", g.len(), w.len()));
            }
            w.iter().zip(g).enumerate().try_for_each(|(i, (w, g))| matches(w, g, &format!("{at}/{i}"), gateway))
        }
        _ if want == got => Ok(()),
        _ => Err(format!("{at}: {got}, the golden has {want}")),
    }
}

fn assert_golden(name: &str, got: &Value, gateway: Option<&str>) {
    if let Err(e) = matches(&golden(name), got, "", gateway) {
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
    let want = serde_json::json!({ "a": 1, "h": "<sha256>", "p": "<path:/x.pem>", "l": [1, 2] });
    assert!(matches(&want, &serde_json::json!({ "a": 1, "h": "0".repeat(64), "p": "/tmp/x.pem", "l": [1, 2] }), "", None).is_ok());
    for bad in [
        serde_json::json!({ "a": 2, "h": "0".repeat(64), "p": "/tmp/x.pem", "l": [1, 2] }),
        serde_json::json!({ "a": 1, "h": "0".repeat(64), "p": "/tmp/x.pem", "l": [1, 2], "extra": 0 }),
        serde_json::json!({ "a": 1, "h": "0".repeat(64), "p": "/tmp/x.pem", "l": [1] }),
        serde_json::json!({ "a": 1, "h": "XYZ", "p": "/tmp/x.pem", "l": [1, 2] }),
        serde_json::json!({ "a": 1, "h": "0".repeat(64), "p": "/tmp/x.key", "l": [1, 2] }),
    ] {
        assert!(matches(&want, &bad, "", None).is_err(), "{bad}");
    }
}

/// `explain <trace> --format json`: the targets, steps, rules and counts of the text form, as one document.
#[test]
fn explain_a_trace_as_json_matches_its_golden() {
    let doc = document(&["explain", "tests/fixtures/cli/trace.jsonl", "--catalogue", "tests/fixtures/cli/catalogue.json", "--format", "json"]);
    assert_golden("explain.json", &doc, None);
    let only = document(&["explain", "tests/fixtures/cli/trace.jsonl", "--target", "route/optimize", "--format", "json"]);
    assert_golden("explain-target.json", &only, None);
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
    assert_golden("explain-correlation.json", &chain, None);
    // The prefix a chain prints, as the text form takes it.
    let root = chain["records"][0]["hash"].as_str().unwrap().to_string();
    let closure = document(&["explain", "--closure", &root[..12], "tests/fixtures/cli/records.jsonl", "--fleet", "2", "--format", "json"]);
    assert_golden("explain-closure.json", &closure, None);
    let missing = document(&["explain", "--closure", &"0".repeat(64), "tests/fixtures/cli/records.jsonl", "--format", "json"]);
    assert_golden("explain-closure-missing.json", &missing, None);
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

    /// Every string the document holds, at any depth.
    fn strings(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Array(a) => a.iter().for_each(|v| strings(v, out)),
            Value::Object(o) => o.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }

    /// **No secrets.** A `tls` document names files and fingerprints only: no PEM block of any kind, no line of any
    /// key or certificate file written, and every string a path, a 64-hex fingerprint, or one of the names the
    /// command was given.
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
    /// and each certificate's SPKI SHA-256, matching the goldens; the fingerprint is the one `tls pin` prints; and no
    /// document carries key material.
    #[test]
    fn each_tls_subcommand_as_json_matches_its_golden_and_carries_no_secret() {
        let (ca, out) = (scratch("ca"), scratch("out"));
        let (ca_s, out_s) = (ca.display().to_string(), out.display().to_string());
        let check = |args: &[&str], golden_name: &str, given: &[&str]| -> Value {
            let o = run(args);
            assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
            let raw = stdout(&o);
            let doc: Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{args:?}: not one JSON document ({e}):\n{raw}"));
            assert_golden(golden_name, &doc, None);
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
}

#[cfg(feature = "gateway")]
mod declare {
    use super::*;
    use std::io::{BufRead, BufReader};
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

    /// `declare --format json` once: one line, the `declared` event; a unit `wire-check` refuses: one line, the
    /// `refused` event, exit 1; `--watch`: the `declared` event, then at SIGINT the `retracted` event.
    #[tokio::test(flavor = "multi_thread")]
    async fn declare_as_json_matches_its_goldens_once_refused_and_watched() {
        let (agent, base) = gateway_node().await;
        let dir = project("once");
        let dir_s = dir.display().to_string();
        let args = |extra: &[&str]| {
            let mut a = vec!["declare".to_string(), dir_s.clone(), "--gateway".into(), base.clone(), "--token".into(), "s3cret".into(), "--format".into(), "json".into()];
            a.extend(extra.iter().map(|s| s.to_string()));
            a
        };
        let once = tokio::task::spawn_blocking({
            let a = args(&[]);
            move || mycelium().args(&a).output().unwrap()
        }).await.unwrap();
        assert!(once.status.success(), "{}", String::from_utf8_lossy(&once.stderr));
        let got = lines(&stdout(&once));
        assert_eq!(got.len(), 1, "once is one line");
        assert_golden("declare.json", &got[0], Some(&base));

        let refused_dir = project("refused");
        std::fs::write(refused_dir.join("units/b.toml"), "principal = \"b\"\n\n[[requirement]]\nns = \"nobody\"\nname = \"offers\"\n").unwrap();
        let refused = tokio::task::spawn_blocking({
            let mut a = args(&[]);
            a[1] = refused_dir.display().to_string();
            move || mycelium().args(&a).output().unwrap()
        }).await.unwrap();
        assert_eq!(refused.status.code(), Some(1));
        let got = lines(&stdout(&refused));
        assert_eq!(got.len(), 1, "{got:?}");
        assert_golden("declare-refused.json", &got[0], Some(&base));

        // Watched: the first event, then SIGINT, then the retraction. The child's stdout is read a line at a time on a
        // blocking thread so the node in this process keeps serving.
        let mut child = mycelium().args(args(&["--watch", "--poll-ms", "100", "--lease-secs", "3"])).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let pid = child.id();
        let out = child.stdout.take().unwrap();
        let read = tokio::task::spawn_blocking(move || {
            let mut reader = BufReader::new(out);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            let _ = Command::new("kill").args(["-INT", &pid.to_string()]).status();
            let mut rest = String::new();
            std::io::Read::read_to_string(&mut reader, &mut rest).unwrap();
            let status = child.wait().unwrap();
            (first + &rest, status)
        });
        let (raw, status) = tokio::time::timeout(Duration::from_secs(30), read).await.expect("the watch stops at SIGINT").unwrap();
        assert!(status.success(), "{status:?}\n{raw}");
        let got = lines(&raw);
        assert_eq!(got.len(), 2, "{raw}");
        assert_golden("declare-watch-declared.json", &got[0], Some(&base));
        assert_golden("declare-watch-retracted.json", &got[1], Some(&base));

        agent.shutdown().await;
        let _ = (std::fs::remove_dir_all(&dir), std::fs::remove_dir_all(&refused_dir));
    }
}
