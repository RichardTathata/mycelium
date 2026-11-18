//! Identity-and-lifecycle I7, PR 3a: **`mycelium dev consent`, the binary, at a terminal** — what the in-crate tests
//! cannot show, because they answer through a scripted approver:
//!
//! - an answer **typed at the terminal the binary opened** (`/dev/tty`) is what gets signed, and a line on its
//!   standard input is not an answer;
//! - with no terminal, nothing is signed;
//! - the key is created outside the worktree the binary runs in, nothing private is printed, and nothing is written
//!   into the worktree.
//!
//! **What stands in for what.** The terminal is a pseudo-terminal a small Python helper opens (`pty.fork`), typing the
//! answers when the prompt appears: a process, not a person — which is exactly what the `tty` grade can and cannot
//! say (I7's residual B). The gateway is a stub in this file that serves two fixed routes and keeps what is posted to
//! it; the signed bodies it receives are verified here with `SignedDecision::verify_signature`. A real node verifying
//! and recording `dev`'s decision is `src/agent/dev_consent_tests.rs`. Every child is bounded (60 s in the helper).
#![cfg(all(unix, feature = "cli", feature = "tls", feature = "gateway", feature = "provisional-consent-grade"))]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const NODE: &str = "127.0.0.1:7946";

/// Runs `argv` under a pseudo-terminal, typing each `|`-separated answer when a prompt ends (a literal `\n` inside
/// one is a newline, so one answer can enter two lines at once); or, with `notty`, in a
/// new session with no controlling terminal and `STDIN` as its standard input. Prints everything the child wrote and
/// exits with its code; 124 when the child outlives 60 s.
const HELPER: &str = r#"
import os, pty, select, subprocess, sys, time
mode, answers, argv = sys.argv[1], [a for a in sys.argv[2].split("|")] if sys.argv[2] != "-" else [], sys.argv[3:]
if mode == "notty":
    p = subprocess.run(argv, input=os.environ.get("STDIN", "").encode(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, timeout=60)
    sys.stdout.buffer.write(p.stdout)
    sys.exit(p.returncode)
pid, fd = pty.fork()
if pid == 0:
    os.execv(argv[0], argv)
buf, seen, deadline, mark = b"", 0, time.time() + 60, b"unanswered: "
while True:
    left = deadline - time.time()
    if left <= 0:
        os.kill(pid, 9)
        sys.stdout.buffer.write(buf)
        sys.exit(124)
    ready, _, _ = select.select([fd], [], [], left)
    if not ready:
        continue
    try:
        chunk = os.read(fd, 4096)
    except OSError:
        break
    if not chunk:
        break
    buf += chunk
    while answers and buf.count(mark) > seen:
        seen += 1
        os.write(fd, answers.pop(0).replace("\\n", "\n").encode() + b"\n")
_, status = os.waitpid(pid, 0)
sys.stdout.buffer.write(buf)
sys.exit(os.waitstatus_to_exitcode(status))
"#;

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mycelium-dev-consent-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The binary, run in `cwd` with `home` as the user's home and nothing else that names a data directory.
fn mycelium(cwd: &Path, home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_mycelium"));
    c.current_dir(cwd).env("HOME", home).env_remove("XDG_DATA_HOME").env_remove("MYCELIUM_GATEWAY_TOKEN");
    c
}

/// `args` through the helper: under a pseudo-terminal typing `answers`, or with no terminal and `stdin`.
fn helped(cwd: &Path, home: &Path, mode: &str, answers: &str, stdin: &str, args: &[&str]) -> Output {
    let out = Command::new("python3")
        .args(["-c", HELPER, mode, answers, env!("CARGO_BIN_EXE_mycelium")])
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("STDIN", stdin)
        .env_remove("XDG_DATA_HOME")
        .env_remove("MYCELIUM_GATEWAY_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("python3 runs the pseudo-terminal helper (this test needs python3 on PATH)");
    assert_ne!(out.status.code(), Some(124), "the binary outlived 60 s:\n{}", String::from_utf8_lossy(&out.stdout));
    out
}

fn text(o: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

/// The `mycelium.dev.consent/1` lines in `out`, as `(request, answer, whole line)`.
fn lines(out: &str) -> Vec<(String, String, Value)> {
    out.lines()
        .filter_map(|l| l.find('{').and_then(|at| serde_json::from_str::<Value>(l[at..].trim()).ok()))
        .filter(|v| v["schema"] == "mycelium.dev.consent/1")
        .map(|v| (v["request"].as_str().unwrap_or_default().to_string(), v["answer"].as_str().unwrap_or_default().to_string(), v))
        .collect()
}

fn row(request: &str) -> Value {
    json!({
        "schema": "mycelium.grant-request/1", "request": request, "agent": "token:fleet/forecaster", "node": NODE,
        "tool": "weather.forecast", "operation": "tools/call", "resource": format!("tool:weather.forecast@{NODE}"),
        "arguments_digest": "5e".repeat(32), "arguments": {"city": "Lyon"}, "requested_at_ms": 1_000, "expires_at_ms": 4_000_000_000_000u64,
        "key": format!("grant/746f6b656e3a666c6565742f666f72656361737465722e/{request}"),
    })
}

/// What the stub gateway was sent: each `POST`'s path and JSON body.
type Posted = Arc<Mutex<Vec<(String, Value)>>>;

/// A stub gateway: `/health` names [`NODE`], `/gateway/grants/pending` lists `rows`, and every `POST` is kept (its
/// path and JSON body) and answered `200`. One request per connection; it stops when the test process ends.
fn stub_gateway(rows: Vec<Value>) -> (String, Posted) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let posted: Posted = Arc::default();
    let kept = Arc::clone(&posted);
    std::thread::spawn(move || {
        for mut conn in listener.incoming().flatten() {
            let _ = conn.set_read_timeout(Some(std::time::Duration::from_secs(10)));
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head_end, length) = loop {
                let Ok(n) = conn.read(&mut chunk) else { break (0, 0) };
                if n == 0 { break (0, 0); }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..at]).to_lowercase();
                    let length = head.lines().find_map(|l| l.strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
                    break (at + 4, length);
                }
            };
            if head_end == 0 { continue; }
            while raw.len() < head_end + length {
                let Ok(n) = conn.read(&mut chunk) else { break };
                if n == 0 { break; }
                raw.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
            let mut first = head.lines().next().unwrap_or_default().split(' ');
            let (method, path) = (first.next().unwrap_or_default(), first.next().unwrap_or_default().to_string());
            let answer = match (method, path.as_str()) {
                ("GET", "/health") => json!({"status": "ok", "node_id": NODE}),
                ("GET", "/gateway/grants/pending") => json!({"ok": true, "pending": rows, "now_ms": 1_000}),
                ("POST", _) => {
                    kept.lock().unwrap().push((path.clone(), serde_json::from_slice(&raw[head_end..]).unwrap_or(Value::Null)));
                    json!({"ok": true, "stub": "kept"})
                }
                _ => json!({"ok": false}),
            }
            .to_string();
            let _ = conn.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}", answer.len()).as_bytes());
        }
    });
    (base, posted)
}

/// Every 16-byte run of the private half of the PKCS#8 in `key`, as hex, and the file's base64: what no output and
/// no file in the worktree may hold.
fn private_runs(key: &Path) -> (Vec<u8>, Vec<String>) {
    use base64::Engine as _;
    let pkcs8 = std::fs::read(key).expect("the key file");
    let private = pkcs8[..pkcs8.len() - 65].to_vec();
    let mut runs: Vec<String> = private.windows(16).map(|w| w.iter().map(|b| format!("{b:02x}")).collect()).collect();
    runs.push(base64::engine::general_purpose::STANDARD.encode(&pkcs8));
    runs.push(base64::engine::general_purpose::STANDARD.encode(&private));
    (private, runs)
}

/// Every file under `dir`, relative, sorted.
fn tree(dir: &Path) -> Vec<PathBuf> {
    fn walk(at: &Path, root: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(at).unwrap().flatten() {
            let p = entry.path();
            if p.is_dir() { walk(&p, root, out) } else { out.push(p.strip_prefix(root).unwrap().to_path_buf()) }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// A developer's set-up, as the binary makes it from inside a worktree: `dev consent key`, the CA's `tls issue
/// --consent --grade <grade>`, and (unless `enrol` is false) `dev consent enrol`. Returns the worktree, the home and
/// everything the commands printed.
fn set_up(root: &Path, grade: &str, enrol: bool) -> (PathBuf, PathBuf, String) {
    let (work, home, ca, out) = (root.join("work"), root.join("home"), root.join("ca"), root.join("out"));
    std::fs::create_dir_all(work.join(".git")).unwrap();
    std::fs::write(work.join("README.md"), "a project\n").unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let mut said = String::new();
    let o = mycelium(&work, &home).args(["dev", "consent", "key", "--issuer", "fleet", "--name", "richard"]).output().unwrap();
    assert!(o.status.success(), "dev consent key: {}", text(&o));
    said.push_str(&text(&o));
    let public = find(&home, ".pub.pem");
    assert!(mycelium(&work, &home).args(["tls", "ca", "--out", ca.to_str().unwrap()]).output().unwrap().status.success());
    let o = mycelium(&work, &home)
        .args(["tls", "issue", "--ca-dir", ca.to_str().unwrap(), "--client", "richard", "--consent", "--public-key", public.to_str().unwrap(), "--grade", grade, "--issuer", "fleet", "--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "tls issue --consent: {}", text(&o));
    if enrol {
        let o = mycelium(&work, &home).args(["dev", "consent", "enrol", "--issuer", "fleet", "--name", "richard", "--cert", out.join("consent-fleet-richard.cert.pem").to_str().unwrap()]).output().unwrap();
        assert!(o.status.success(), "dev consent enrol: {}", text(&o));
        said.push_str(&text(&o));
    }
    (work, home, said)
}

fn find(under: &Path, suffix: &str) -> PathBuf {
    let found: Vec<PathBuf> = tree(under).into_iter().filter(|p| p.to_string_lossy().ends_with(suffix)).collect();
    assert_eq!(found.len(), 1, "one {suffix} under {}: {found:?}", under.display());
    under.join(&found[0])
}

/// **An answer typed at the terminal is what `dev` signs; the key stays off the worktree and out of every output**
/// (I7: exit 6's terminal, exit 10, and "nothing writes a private key into the repository or a log").
///
/// From inside a worktree, the binary creates the key under the user's home, enrols under a `tty` certificate, and is
/// asked for three requests at a pseudo-terminal where `approve`, an empty line and `refuse` are typed:
/// - the gateway receives **two** decisions — an issue for the first (with the `--ttl-secs` asked for) and a refusal
///   for the third — each signed by the enrolled key, naming `tty`, addressed to the gateway's node, and verifying;
///   nothing is sent for the dismissed one;
/// - the prompt shows the record's tool, resource and digest;
/// - the exit code is 1 (one request was not answered);
/// - the key file is `0600` in a `0700` directory under the home; the worktree holds exactly the files it held; and
///   no run of the private key appears in anything printed, in anything posted, or in any file of the worktree;
/// - then, the key file widened to `0644`: refused by name (`consent_key_too_open`) before anything is asked.
///
/// Seen failing first: `key` printing the PKCS#8 (`PRIVATE KEY: a run of it was printed or posted`). The directory
/// resolved against the current directory is the library test's plant
/// (`the_consent_directory_is_under_the_users_home_and_never_relative`).
#[test]
fn an_answer_typed_at_the_terminal_is_what_dev_signs_and_the_key_stays_off_the_worktree() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = scratch("typed");
    let (work, home, mut said) = set_up(&root, "tty", true);
    let before = tree(&work);
    let key = find(&home, ".key.p8");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o7777;
    assert_eq!((mode(&key), mode(key.parent().unwrap())), (0o600, 0o700), "the key, private under the home");
    assert!(key.starts_with(&home) && !key.starts_with(&work));
    let (base, posted) = stub_gateway(vec![row("r1"), row("r2"), row("r3")]);

    let o = helped(&work, &home, "pty", "approve||refuse", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "richard", "--gateway", &base, "--token", "relay", "--ttl-secs", "120", "r1", "r2", "r3"]);
    let out = text(&o);
    said.push_str(&out);
    let sent = posted.lock().unwrap().clone();

    // Off the worktree, and out of every output — checked first, before what was answered.
    assert_eq!(tree(&work), before, "WORKTREE: nothing was written into it");
    let (private, runs) = private_runs(&key);
    said.push_str(&format!("{sent:?}"));
    for run in &runs {
        assert!(!said.contains(run), "PRIVATE KEY: a run of it was printed or posted");
    }
    for file in tree(&work) {
        let bytes = std::fs::read(work.join(&file)).unwrap();
        assert!(!bytes.windows(16).any(|w| private.windows(16).any(|p| p == w)), "PRIVATE KEY: in the worktree's {}", file.display());
    }

    let answered: Vec<(String, String)> = lines(&out).into_iter().map(|(r, a, _)| (r, a)).collect();
    assert_eq!(answered, [("r1".into(), "signed".into()), ("r2".into(), "dismissed".into()), ("r3".into(), "signed".into())], "ANSWERS:\n{out}");
    assert_eq!(o.status.code(), Some(1), "one request was left unanswered:\n{out}");
    for shown in ["weather.forecast", &format!("tool:weather.forecast@{NODE}"), &"5e".repeat(32), "consent:fleet/richard", "grade tty", "for 120 s"] {
        assert!(out.contains(shown), "PROMPT: shows {shown:?}:\n{out}");
    }

    assert_eq!(sent.len(), 2, "POSTED: an issue and a refusal, nothing for the dismissed request: {sent:?}");
    let public = std::fs::read_to_string(find(&home, ".pub.pem")).unwrap();
    let point = mycelium::p256_point_from_spki_pem(&public).unwrap();
    for ((path, body), (request, decision, ttl)) in sent.iter().zip([("r1", "issue", Some(120u64)), ("r3", "refuse", None)]) {
        assert_eq!(path, &format!("/gateway/grants/746f6b656e3a666c6565742f666f72656361737465722e/{request}"), "sent to the record's own key");
        assert_eq!((body["decision"].as_str(), body["ttl_secs"].as_u64()), (Some(decision), ttl), "{body}");
        let signed: mycelium::SignedDecision = serde_json::from_value(body["signed"].clone()).expect("a signed decision");
        assert_eq!((signed.decider.as_str(), signed.node.as_str(), signed.agent.as_str(), signed.request.as_str(), signed.grade.as_deref()), ("consent:fleet/richard", NODE, "token:fleet/forecaster", request, Some("tty")));
        assert_eq!(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signed.k).unwrap(), point, "signed by the enrolled key");
        assert!(signed.verify_signature(decision, ttl, body["reason"].as_str()), "VERIFY: {body}");
        assert!(!signed.verify_signature(if decision == "issue" { "refuse" } else { "issue" }, ttl, body["reason"].as_str()), "and not as the other decision");
    }

    // A wider key file is refused by name before anything is asked.
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let o = helped(&work, &home, "pty", "approve", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "richard", "--gateway", &base, "--token", "relay", "r2"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("consent_key_too_open") && text(&o).contains("644") && !text(&o).contains("unanswered: "), "WIDER: refused by name, no prompt:\n{}", text(&o));
    assert_eq!(posted.lock().unwrap().len(), 2, "and nothing more was sent");
    let _ = std::fs::remove_dir_all(&root);
}

/// **A line entered before a prompt was shown is not that prompt's answer.** At the first prompt two lines are
/// entered at once, `approve` twice — a double return, a pasted block, an agent typing ahead. The first answers the
/// prompt on the screen. The second is waiting at the terminal when the next request's prompt is about to be shown:
/// it is discarded, the prompt says so, and the request is decided by what is typed **after** it appears — here an
/// empty line, so nothing is signed for it. One decision reaches the gateway, not two.
///
/// Seen failing first: nothing discarded before the prompt (the second request was approved by the line typed
/// before it was shown: two decisions posted).
#[test]
fn a_line_entered_before_a_prompt_was_shown_is_discarded() {
    let root = scratch("ahead");
    let (work, home, _) = set_up(&root, "tty", true);
    let (base, posted) = stub_gateway(vec![row("r1"), row("r2")]);
    let o = helped(&work, &home, "pty", "approve\\napprove|", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "richard", "--gateway", &base, "--token", "relay", "r1", "r2"]);
    let out = text(&o);
    let answered: Vec<(String, String)> = lines(&out).into_iter().map(|(r, a, _)| (r, a)).collect();
    let sent: Vec<String> = posted.lock().unwrap().iter().map(|(path, _)| path.rsplit('/').next().unwrap_or_default().to_string()).collect();
    assert_eq!(sent, ["r1"], "POSTED: one decision, for the prompt that was on the screen:\n{out}");
    assert_eq!(answered, [("r1".into(), "signed".into()), ("r2".into(), "dismissed".into())], "ANSWERS:\n{out}");
    assert!(out.contains("typed before this prompt was shown were discarded"), "the prompt says what it discarded:\n{out}");
    let _ = std::fs::remove_dir_all(&root);
}

/// **Without a terminal `dev` signs nothing, and a line on standard input is not an answer.** The binary runs in a
/// new session with no controlling terminal; `approve` waits on its standard input. With the ids on the command
/// line, the request is answered `no_terminal`; with the ids read from standard input (`-`), each line is an **id**,
/// so `approve` is looked up as a request and is unknown. Nothing is posted either way.
///
/// Seen failing first: `TtyApprover` reading standard input when it has no terminal (the piped `approve` was signed
/// and posted).
#[test]
fn without_a_terminal_dev_signs_nothing_and_standard_input_is_not_an_answer() {
    let root = scratch("notty");
    let (work, home, _) = set_up(&root, "tty", true);
    let (base, posted) = stub_gateway(vec![row("r1"), row("r2")]);
    let common = ["dev", "consent", "approve", "--issuer", "fleet", "--name", "richard", "--gateway", base.as_str(), "--token", "relay"];

    let o = helped(&work, &home, "notty", "-", "approve\napprove\n", &[&common[..], &["r1"]].concat());
    let answered: Vec<(String, String)> = lines(&text(&o)).into_iter().map(|(r, a, _)| (r, a)).collect();
    assert_eq!(answered, [("r1".into(), "no_terminal".into())], "NO TERMINAL:\n{}", text(&o));
    assert_eq!(o.status.code(), Some(1));

    let o = helped(&work, &home, "notty", "-", "r2\napprove\n", &[&common[..], &["-"]].concat());
    let answered: Vec<(String, String)> = lines(&text(&o)).into_iter().map(|(r, a, _)| (r, a)).collect();
    assert_eq!(answered, [("r2".into(), "no_terminal".into()), ("approve".into(), "unknown_request".into())], "STDIN IS IDS:\n{}", text(&o));
    assert!(posted.lock().unwrap().is_empty(), "nothing was signed or sent: {:?}", posted.lock().unwrap());
    let _ = std::fs::remove_dir_all(&root);
}

/// **`dev` does not sign under a certificate that calls its software key an OS-presence key** — at the binary. The
/// operator certifies the key `os_presence(macos, unbound)`; `dev consent enrol` refuses the certificate by name and
/// keeps nothing, and `approve` — with the certificate copied beside the key by hand — refuses before anything is
/// asked. Seen failing first: the grade check removed from `Enrolment::check` (enrolled, exit 0).
#[test]
fn the_binary_refuses_a_presence_grade_for_its_software_key() {
    let root = scratch("grade");
    let (work, home, _) = set_up(&root, "os_presence(macos, unbound)", false);
    let cert = root.join("out").join("consent-fleet-richard.cert.pem");
    let o = mycelium(&work, &home).args(["dev", "consent", "enrol", "--issuer", "fleet", "--name", "richard", "--cert", cert.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("grade_not_this_keys") && text(&o).contains("--grade tty"), "refused by name, saying what the key is:\n{}", text(&o));
    let key = find(&home, ".key.p8");
    assert!(!tree(&home).iter().any(|p| p.to_string_lossy().ends_with(".cert.pem")), "the refused certificate was not kept");
    std::fs::copy(&cert, key.parent().unwrap().join("consent-fleet-richard.cert.pem")).unwrap();
    let (base, posted) = stub_gateway(vec![row("r1")]);
    let o = helped(&work, &home, "pty", "approve", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "richard", "--gateway", &base, "r1"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("grade_not_this_keys") && !text(&o).contains("unanswered: "), "refused on use, no prompt:\n{}", text(&o));
    assert!(posted.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
