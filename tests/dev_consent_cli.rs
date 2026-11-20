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
//! and recording `dev`'s decision is `src/agent/dev_consent_tests.rs`. Every child is bounded (240 s in the helper).
#![cfg(all(unix, feature = "cli", feature = "tls", feature = "gateway", feature = "provisional-consent-grade"))]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const NODE: &str = "127.0.0.1:7946";

/// Runs `argv` under a pseudo-terminal, typing each `|`-separated answer when a prompt ends (a literal `\n` inside
/// one is a newline, so one answer can enter two lines at once; with `ahead` the first answer is typed at once, with
/// no newline, before any prompt); or, with `notty`, in a
/// new session with no controlling terminal and `STDIN` as its standard input. Prints everything the child wrote and
/// exits with its code; 124 when the child outlives 240 s (a loaded macOS machine was seen to hold one launch of a
/// freshly built binary for a minute before its first instruction).
const HELPER: &str = r#"
import os, pty, select, subprocess, sys, time
mode, answers, argv = sys.argv[1], [a for a in sys.argv[2].split("|")] if sys.argv[2] != "-" else [], sys.argv[3:]
if mode == "notty":
    p = subprocess.run(argv, input=os.environ.get("STDIN", "").encode(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, start_new_session=True, timeout=240)
    sys.stdout.buffer.write(p.stdout)
    sys.exit(p.returncode)
pid, fd = pty.fork()
if pid == 0:
    os.execv(argv[0], argv)
if mode == "ahead":
    os.write(fd, answers.pop(0).encode())
buf, seen, deadline, mark = b"", 0, time.time() + 240, b"unanswered: "
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
    assert_ne!(out.status.code(), Some(124), "the binary outlived 240 s:\n{}", String::from_utf8_lossy(&out.stdout));
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A request record that agrees with itself: its resource is its tool at its node, and its arguments hash to its
/// digest (RFC 8785, as the request door writes it).
fn row_of(request: &str, tool: &str, arguments: Value) -> Value {
    json!({
        "schema": "mycelium.grant-request/1", "request": request, "agent": "token:fleet/forecaster", "node": NODE,
        "tool": tool, "operation": "tools/call", "resource": format!("tool:{tool}@{NODE}"),
        "arguments_digest": hex(&mycelium::arguments_digest(&arguments)), "arguments": arguments, "requested_at_ms": 1_000, "expires_at_ms": 4_000_000_000_000u64,
        "key": format!("grant/746f6b656e3a666c6565742f666f72656361737465722e/{request}"),
    })
}

fn row(request: &str) -> Value {
    row_of(request, "weather.forecast", json!({"city": "Lyon"}))
}

fn lyon_digest() -> String {
    hex(&mycelium::arguments_digest(&json!({"city": "Lyon"})))
}

/// What the stub gateway was sent: each `POST`'s path and JSON body.
type Posted = Arc<Mutex<Vec<(String, Value)>>>;

/// A stub gateway: `/health` names [`NODE`], `/gateway/grants/pending` lists `rows`, and every `POST` is kept (its
/// path and JSON body) and answered `200`. One request per connection; it stops when the test process ends.
fn stub_gateway(rows: Vec<Value>) -> (String, Posted) {
    stub_gateway_with(rows, 0, None)
}

/// [`stub_gateway`] whose pending list is padded with `pad` bytes, and which runs `on_post` after keeping each `POST`.
fn stub_gateway_with(rows: Vec<Value>, pad: usize, on_post: Option<Box<dyn Fn() + Send>>) -> (String, Posted) {
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
                ("GET", "/gateway/grants/pending") => json!({"ok": true, "pending": rows, "now_ms": 1_000, "pad": "x".repeat(pad)}),
                ("POST", _) => {
                    kept.lock().unwrap().push((path.clone(), serde_json::from_slice(&raw[head_end..]).unwrap_or(Value::Null)));
                    if let Some(on_post) = &on_post {
                        on_post();
                    }
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
    let o = mycelium(&work, &home).args(["dev", "consent", "key", "--issuer", "fleet", "--name", "alex"]).output().unwrap();
    assert!(o.status.success(), "dev consent key: {}", text(&o));
    said.push_str(&text(&o));
    let public = find(&home, ".pub.pem");
    assert!(mycelium(&work, &home).args(["tls", "ca", "--out", ca.to_str().unwrap()]).output().unwrap().status.success());
    let o = mycelium(&work, &home)
        .args(["tls", "issue", "--ca-dir", ca.to_str().unwrap(), "--client", "alex", "--consent", "--public-key", public.to_str().unwrap(), "--grade", grade, "--issuer", "fleet", "--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(o.status.success(), "tls issue --consent: {}", text(&o));
    if enrol {
        let o = mycelium(&work, &home).args(["dev", "consent", "enrol", "--issuer", "fleet", "--name", "alex", "--cert", out.join("consent-fleet-alex.cert.pem").to_str().unwrap()]).output().unwrap();
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

    let o = helped(&work, &home, "pty", "approve||refuse", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "--token", "relay", "--ttl-secs", "120", "r1", "r2", "r3"]);
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
    for shown in ["weather.forecast", &format!("tool:weather.forecast@{NODE}"), &lyon_digest(), "consent:fleet/alex", "grade tty", "for 120 s"] {
        assert!(out.contains(shown), "PROMPT: shows {shown:?}:\n{out}");
    }

    assert_eq!(sent.len(), 2, "POSTED: an issue and a refusal, nothing for the dismissed request: {sent:?}");
    let public = std::fs::read_to_string(find(&home, ".pub.pem")).unwrap();
    let point = mycelium::p256_point_from_spki_pem(&public).unwrap();
    for ((path, body), (request, decision, ttl)) in sent.iter().zip([("r1", "issue", Some(120u64)), ("r3", "refuse", None)]) {
        assert_eq!(path, &format!("/gateway/grants/746f6b656e3a666c6565742f666f72656361737465722e/{request}"), "sent to the record's own key");
        assert_eq!((body["decision"].as_str(), body["ttl_secs"].as_u64()), (Some(decision), ttl), "{body}");
        let signed: mycelium::SignedDecision = serde_json::from_value(body["signed"].clone()).expect("a signed decision");
        assert_eq!((signed.decider.as_str(), signed.node.as_str(), signed.agent.as_str(), signed.request.as_str(), signed.grade.as_deref()), ("consent:fleet/alex", NODE, "token:fleet/forecaster", request, Some("tty")));
        assert_eq!(base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &signed.k).unwrap(), point, "signed by the enrolled key");
        // Identity-and-lifecycle I7-S: the block of the record the prompt showed, inside the signature.
        let shown = signed.shown.as_ref().expect("the decision names what was shown");
        assert_eq!((shown.resource.as_str(), shown.request_expires_at_ms), (format!("tool:{}@{NODE}", shown.tool).as_str(), 4_000_000_000_000), "SHOWN: {body}");
        assert!(signed.verify_signature(decision, ttl, body["reason"].as_str()), "VERIFY: {body}");
        assert!(!signed.verify_signature(if decision == "issue" { "refuse" } else { "issue" }, ttl, body["reason"].as_str()), "and not as the other decision");
    }

    // A wider key file is refused by name before anything is asked.
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let o = helped(&work, &home, "pty", "approve", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "--token", "relay", "r2"]);
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
    let o = helped(&work, &home, "pty", "approve\\napprove|", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "--token", "relay", "r1", "r2"]);
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
    let common = ["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", base.as_str(), "--token", "relay"];

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
    let cert = root.join("out").join("consent-fleet-alex.cert.pem");
    let o = mycelium(&work, &home).args(["dev", "consent", "enrol", "--issuer", "fleet", "--name", "alex", "--cert", cert.to_str().unwrap()]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("grade_not_this_keys") && text(&o).contains("--grade tty"), "refused by name, saying what the key is:\n{}", text(&o));
    let key = find(&home, ".key.p8");
    assert!(!tree(&home).iter().any(|p| p.to_string_lossy().ends_with(".cert.pem")), "the refused certificate was not kept");
    std::fs::copy(&cert, key.to_str().unwrap().replace(".key.p8", ".cert.pem")).unwrap();
    let (base, posted) = stub_gateway(vec![row("r1")]);
    let o = helped(&work, &home, "pty", "approve", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "r1"]);
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("grade_not_this_keys") && !text(&o).contains("unanswered: "), "refused on use, no prompt:\n{}", text(&o));
    assert!(posted.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------------------------------------------
// #813's review: what a record, a terminal and a directory can and cannot do. The tests below drive
// `mycelium::dev_consent`'s public surface directly where no terminal is needed, and the binary where one is.
// ---------------------------------------------------------------------------------------------------------------

use mycelium::dev_consent as dc;

/// A store under a fresh scratch home, with a key made by `dev` and (when `grade` is given) a certificate at that
/// grade issued by a scratch CA and enrolled. Returns the store, the CA directory and the home.
fn api_store(root: &Path, enrol: bool) -> (dc::Store, PathBuf) {
    let ca = root.join("ca");
    mycelium::mint_ca(&ca).expect("a CA");
    let store = dc::Store::at(root.join("home").join("consent"), "fleet", "alex").expect("outside a worktree");
    let key = store.create_key(false).expect("a key");
    if enrol {
        let cert = mycelium::issue_consent_cert(&ca, "fleet", "alex", &key.spki_pem(), mycelium::ConsentGrade::Tty, &root.join("out")).expect("certified");
        store.enrol(&std::fs::read_to_string(cert).unwrap()).expect("enrolled");
    }
    (store, ca)
}

/// An approver answering from a list; `during` runs while the first prompt is up.
struct Scripted {
    answers: Vec<dc::Approval>,
    prompts: usize,
    during: Option<Box<dyn FnOnce()>>,
}

impl dc::Approver for Scripted {
    fn ask(&mut self, _: &dc::Prompt) -> dc::Approval {
        self.prompts += 1;
        if let Some(during) = self.during.take() {
            during();
        }
        if self.answers.is_empty() { dc::Approval::Dismissed } else { self.answers.remove(0) }
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime")
}

/// **A record whose arguments do not hash to its digest prompts nothing** (#813's review, F2). The grant binds the
/// digest; the person reads the arguments; `dev` shows a record only when the one is the hash of the other:
/// - benign arguments beside **another call's digest** — refused (`arguments_unverified`), though the record never
///   changes and no timing is involved;
/// - arguments the record **withholds** (a data class) or **cuts** (over the rendering cap), and a record with
///   none — refused, not shown with a caveat: a person cannot approve what this device cannot relate to the digest;
/// - the RFC 8785 digest, and the pre-I3 form the nodes still honour for the window (`deprecations.md` §93), are
///   each accepted;
/// - a record whose **resource is not its tool at its node** is refused (`record_unreadable`): the evaluator is
///   asked about the resource and the grant permits the tool, so `dev` shows neither when they disagree.
///
/// Seen failing first, one build per plant: the digest comparison removed (*another call's digest: SHOWN*, and the
/// withheld and cut records with it); the resource comparison removed (*a resource naming another tool: SHOWN*);
/// the unsafe-integer refusal removed (*an integer beyond 2^53 …: SHOWN*).
#[test]
fn a_record_whose_arguments_do_not_hash_to_its_digest_prompts_nothing() {
    let lyon = json!({"city": "Lyon"});
    let transfer = json!({"to": "acct-9", "amount": 5000});
    // ECMAScript writes 0.000001 as `0.000001` and serde_json as `1e-6`: the two digest forms differ for it.
    let float = json!({"x": 0.000001});
    let (jcs, legacy) = (hex(&mycelium::arguments_digest(&float)), hex(&mycelium::legacy_arguments_digest(&float)));
    assert_ne!(jcs, legacy, "the two forms differ for this value, so the legacy case below is a real one");
    let with = |change: &dyn Fn(&mut Value)| {
        let mut r = row_of("r1", "weather.forecast", lyon.clone());
        change(&mut r);
        dc::PromptRecord::find(&json!([r]), "r1").map(|_| ()).map_err(|e| format!("{e:?}"))
    };
    let unverified = |r: &Result<(), String>| r.as_ref().err().is_some_and(|e| e.starts_with("ArgumentsUnverified"));
    let mut wrong: Vec<String> = Vec::new();
    for (what, result) in [
        ("another call's digest", with(&|r| r["arguments_digest"] = hex(&mycelium::arguments_digest(&transfer)).into())),
        ("another call's arguments", with(&|r| r["arguments"] = transfer.clone())),
        ("arguments withheld by a data class", with(&|r| r["arguments"] = json!({"withheld": "data class secret: not rendered into the gossiped request", "class": "secret", "bytes": 15}))),
        ("arguments over the rendering cap", with(&|r| r["arguments"] = json!({"withheld": "over the rendering cap", "bytes": 20000, "cap": 16384}))),
        ("no arguments", with(&|r| r["arguments"] = Value::Null)),
        ("a digest that is not hex of anything", with(&|r| r["arguments_digest"] = "ff".repeat(32).into())),
        // #813's confirm round, C2: 2^53 + 1 and 2^53 share a digest, so the digest does not say which is meant.
        ("an integer beyond 2^53, under its neighbour's digest", with(&|r| { r["arguments"] = json!({"account": 9007199254740993u64}); r["arguments_digest"] = hex(&mycelium::arguments_digest(&json!({"account": 9007199254740992u64}))).into(); })),
        ("an integer beyond 2^53, under its own digest", with(&|r| { r["arguments"] = json!({"account": 9007199254740993u64}); r["arguments_digest"] = hex(&mycelium::arguments_digest(&json!({"account": 9007199254740993u64}))).into(); })),
    ] {
        if !unverified(&result) {
            wrong.push(format!("{what}: SHOWN or misnamed ({result:?}), not arguments_unverified"));
        }
    }
    for (what, result) in [
        ("the RFC 8785 digest", with(&|_| {})),
        ("the RFC 8785 digest of a float", with(&|r| { r["arguments"] = float.clone(); r["arguments_digest"] = jcs.clone().into(); })),
        ("the legacy digest of a float, honoured for the window", with(&|r| { r["arguments"] = float.clone(); r["arguments_digest"] = legacy.clone().into(); })),
    ] {
        if result.is_err() {
            wrong.push(format!("{what}: refused ({result:?})"));
        }
    }
    for (what, result) in [
        ("a resource naming another tool", with(&|r| r["resource"] = format!("tool:payments.transfer@{NODE}").into())),
        ("a tool the resource does not name", with(&|r| r["tool"] = "payments.transfer".into())),
        ("a resource at another node", with(&|r| r["resource"] = "tool:weather.forecast@10.0.0.9:7946".into())),
    ] {
        if result != Err("Unreadable(\"resource\")".to_string()) {
            wrong.push(format!("{what}: SHOWN or misnamed ({result:?}), not record_unreadable"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(dc::Answer::NoRecord(dc::NoRecord::ArgumentsUnverified("x")).word(), "arguments_unverified");
}

/// What an 80-column terminal shows of `text`: each line wrapped at 80.
fn rows_at_80(text: &str) -> Vec<String> {
    text.split('\n').flat_map(|line| {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() { vec![String::new()] } else { chars.chunks(80).map(|c| c.iter().collect()).collect() }
    }).collect()
}

/// **The lines an approval is about are the last thing shown, whatever the record holds** (#813's review, F3). The
/// requester controls its arguments through the door, so no forgery is needed: the reviewer's request — a transfer
/// whose arguments are 58 spaces, 17 blank rows and a forged header naming `weather.forecast`, 1 933 characters —
/// scrolled the real lines out of an 80×24 terminal. Now:
/// - the arguments come **first** and are cut to `ARGUMENTS_SHOWN_MAX` characters, with what was cut said;
/// - request, holder, tool, resource, digest and the terms come **after** them, directly above the question, each
///   identifier cut to `ID_SHOWN_MAX`: with every field at its longest, the request line to the question is at most
///   fifteen rows **of an 80-column terminal** — so they are on the screen when the answer is typed there. The
///   terminal's width is not read: on a narrower one the block is longer, and this test does not count that;
/// - the bottom 24 rows of the reviewer's prompt hold the real tool, `payments.transfer`, below anything forged.
///
/// Seen failing first, one build per plant: the arguments rendered below the digest (*the arguments are shown
/// before…*); `ARGUMENTS_SHOWN_MAX` raised to 100 000 (*the arguments are cut…*); `ID_SHOWN_MAX` raised to 10 000
/// (*…at most 15 rows*).
#[test]
fn the_lines_an_approval_is_about_are_the_last_thing_shown_whatever_the_record_holds() {
    let forged = format!("{}{}mycelium consent: a grant request.{}  request    r1{}  holder     token:fleet/forecaster{}  tool       weather.forecast{}", " ".repeat(58), " ".repeat(80 * 17), " ".repeat(46), " ".repeat(65), " ".repeat(45), " ".repeat(1933 - 58 - 80 * 17 - 34 - 46 - 15 - 65 - 35 - 45 - 27));
    let record = dc::PromptRecord::find(&json!([row_of("r1", "payments.transfer", json!({"note": forged}))]), "r1").expect("the record agrees with itself");
    let prompt = dc::Prompt::render(&record, "consent:fleet/alex", mycelium::ConsentGrade::Tty, "http://127.0.0.1:19577", NODE, 300, 1_000).text;
    let rows = rows_at_80(&prompt);
    let screen = &rows[rows.len().saturating_sub(24)..];
    let real = screen.iter().position(|r| r.starts_with("  tool       payments.transfer")).unwrap_or_else(|| panic!("the real tool line is on the last screen:\n{}", screen.join("\n")));
    assert!(!screen.iter().skip(real).any(|r| r.contains("weather.forecast")), "nothing naming another tool is below it:\n{}", screen.join("\n"));
    let (arguments_at, request_at) = (prompt.find("  arguments  ").expect("arguments"), prompt.find("\n  request    ").expect("request"));
    assert!(arguments_at < request_at, "the arguments are shown before the lines an approval is about:\n{prompt}");
    let shown_arguments = &prompt[arguments_at..prompt[arguments_at..].find('\n').map(|n| arguments_at + n).unwrap()];
    assert!(shown_arguments.chars().count() <= dc::ARGUMENTS_SHOWN_MAX + 80 && shown_arguments.contains("more characters not shown]"), "the arguments are cut to the bound, and say so: {} characters", shown_arguments.chars().count());
    assert!(prompt.contains("characters of them are NOT shown above"), "and the digest line says the person has not seen all of them:\n{prompt}");

    // Every field at its longest: the request line to the question still fits well inside one screen.
    let long = "x".repeat(4000);
    let mut hostile = row_of(&long, &long, json!({"note": long}));
    hostile["agent"] = long.clone().into();
    hostile["key"] = format!("grant/seg/{long}").into();
    let record = dc::PromptRecord::find(&json!([hostile]), &long).expect("long fields, and a record that agrees with itself");
    let prompt = dc::Prompt::render(&record, "consent:fleet/alex", mycelium::ConsentGrade::Tty, &long, &long, 3600, 0).text;
    let tail = &prompt[prompt.find("\n  request    ").unwrap() + 1..];
    assert!(rows_at_80(tail).len() <= 15, "from the request line to the question is at most 15 rows of 80 columns, whatever the record holds: {} rows\n{tail}", rows_at_80(tail).len());
}

/// **Everything that is not printable ASCII is written as an escape, and so is a space after a space** (#813's
/// review, F3; its confirm round, C3 and C4). In the arguments and in an identifier alike: the escape character, a
/// bidirectional override and isolate, U+061C, the line and paragraph separators, a tag character, a zero-width
/// space, a byte-order mark, a no-break space, a Hangul filler, a private-use character, a backslash — and **a
/// Hebrew and an Arabic letter, a Cyrillic look-alike, an alphabetic combining mark, a full-width letter and
/// digit**, which an allow-list of "letters and digits" let through — each reach the terminal as `\u{…}`. The
/// price is stated: arguments in a script other than ASCII are shown as escapes. A run of spaces is shown as one
/// space and escapes, so arguments cannot be padded into rows that imitate the prompt's own block.
///
/// Seen failing first, one build per plant: letters and digits beyond ASCII shown as themselves (*U+05D0 reached
/// the terminal in free text*, and the marks and full-width forms with it); a run of spaces shown as it is (*a run
/// of spaces is shown as spaces*).
#[test]
fn everything_that_is_not_printable_ascii_is_an_escape_and_so_is_a_space_after_a_space() {
    let hidden = ['\u{1b}', '\u{202e}', '\u{2066}', '\u{61c}', '\u{2028}', '\u{2029}', '\u{e0041}', '\u{200b}', '\u{feff}', '\u{a0}', '\u{3164}', '\u{e000}', '\u{7f}', '\u{85}', '\\',
        '\u{5d0}', '\u{627}', '\u{430}', '\u{345}', '\u{5b0}', '\u{64e}', '\u{ff41}', '\u{ff11}', '\u{e9}', '\u{6771}'];
    let mut wrong: Vec<String> = Vec::new();
    for c in hidden {
        for (what, shown) in [("free text", dc::shown_text(&format!("a{c}b"), 100)), ("an identifier", dc::shown_id(&format!("a{c}b"), 100))] {
            if shown != format!("a\\u{{{:x}}}b", c as u32) {
                wrong.push(format!("U+{:04X} reached the terminal in {what}: {shown:?}", c as u32));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert_eq!(dc::shown_text("Lyon 42 {\"k\":[1], \"n\": -3.5e2}", 100), "Lyon 42 {\"k\":[1], \"n\": -3.5e2}", "printable ASCII is shown as itself, single spaces included");
    assert_eq!(dc::shown_id("payments.transfer x", 100), "payments.transfer\\u{20}x", "an identifier shows no space");
    assert_eq!(dc::shown_id(&"x".repeat(500), 96), format!("{} …[cut: 404 more characters not shown]", "x".repeat(96)), "a cut says how much");

    // C4: padding cannot lay the arguments out in rows of their own.
    let padded = format!("x{}What `approve` signs is about these lines:{}  tool       weather.forecast", " ".repeat(40), " ".repeat(38));
    let shown = dc::shown_text(&padded, 400);
    assert!(!shown.contains("  "), "a run of spaces is shown as spaces: {shown:?}");
    assert!(shown.starts_with("x \\u{20}\\u{20}"), "one space, then escapes: {shown:?}");
    let record = dc::PromptRecord::find(&json!([row_of("r1", "payments.transfer", json!({"note": padded}))]), "r1").unwrap();
    let prompt = dc::Prompt::render(&record, "consent:fleet/alex", mycelium::ConsentGrade::Tty, "http://g", NODE, 300, 0).text;
    assert_eq!(rows_at_80(&prompt).iter().filter(|row| row.trim_start().starts_with("What `approve` signs")).count(), 1, "one block with that heading on an 80-column terminal:\n{prompt}");
}

/// **The consent directory is refused by its real path** (#813's review, F4). The check used to walk the path as
/// written, so a link above the directory led the key into a repository:
/// - `HOME` a symbolic link to a directory inside a repository — refused (`consent_dir_in_worktree`), as the same
///   directory named directly is;
/// - `…/mycelium` a symbolic link into a repository — refused;
/// - a home with no link and no `.git` above it — a key is created, and its path is the real one.
///
/// **Not detected, stated:** a home tracked by a repository whose `.git` is elsewhere (a bare dotfiles repository).
///
/// Seen failing first: the links not resolved (*HOME a link into a repository: a key was created at …/repo/sub/…*).
#[test]
fn the_consent_directory_is_refused_by_its_real_path() {
    let root = scratch("realpath");
    let (repo, plain) = (root.join("repo"), root.join("plain"));
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::create_dir_all(repo.join("stash")).unwrap();
    std::fs::create_dir_all(&plain).unwrap();
    // The directory `dev` would use under a home, relative to it, on this platform.
    let under = |home: &Path| dc::consent_dir(&|v| (v == "HOME").then(|| home.display().to_string())).unwrap();
    let open = |home: &Path| dc::Store::open(&|v| (v == "HOME").then(|| home.display().to_string()), "fleet", "alex");
    let mut wrong: Vec<String> = Vec::new();

    let linked_home = root.join("linked-home");
    std::os::unix::fs::symlink(repo.join("sub"), &linked_home).unwrap();
    for (what, home) in [("HOME inside a repository", repo.join("sub")), ("HOME a link into a repository", linked_home.clone())] {
        match open(&home).and_then(|s| s.create_key(false).map(|_| s)) {
            Err(r) if r.code() == "consent_dir_in_worktree" => {}
            Err(r) => wrong.push(format!("{what}: refused {}, not consent_dir_in_worktree", r.code())),
            Ok(s) => wrong.push(format!("{what}: a key was created at {}", s.key_path().display())),
        }
    }
    let link_home = root.join("home-with-a-linked-directory");
    let app = under(&link_home);
    std::fs::create_dir_all(app.parent().unwrap().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(repo.join("stash"), app.parent().unwrap()).unwrap();
    match open(&link_home).and_then(|s| s.create_key(false).map(|_| s)) {
        Err(r) if r.code() == "consent_dir_in_worktree" => {}
        Err(r) => wrong.push(format!("the mycelium directory a link into a repository: refused {}, not consent_dir_in_worktree", r.code())),
        Ok(s) => wrong.push(format!("the mycelium directory a link into a repository: a key was created at {}", s.key_path().display())),
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(tree(&repo).is_empty(), "nothing was written into the repository: {:?}", tree(&repo));

    let store = open(&plain).expect("no link, no repository");
    store.create_key(false).expect("a key");
    assert!(store.key_path().starts_with(std::fs::canonicalize(&plain).unwrap()), "the key's path is the real one: {}", store.key_path().display());
    let _ = std::fs::remove_dir_all(&root);
}

/// **Two principals never share a file** (#813's review, F12; its confirm round, C6): `a-b`/`c` and `a`/`b-c` both
/// made `consent-a-b-c.key.p8`, and on a case-insensitive filesystem `Fleet`/`x` and `fleet`/`x` were one file, so
/// `key --force` for one replaced the other's key. The separator is a character neither part can hold, and the name
/// ends in a digest of the principal: the files differ **even when their names are compared without case**.
/// Seen failing first, one build per plant: the `-` separator restored (*the two principals share …*); the digest
/// left out (*… share a file on a case-insensitive filesystem*).
#[test]
fn two_principals_never_share_a_file() {
    let root = scratch("names");
    let dir = root.join("consent");
    let (one, two) = (dc::Store::at(dir.clone(), "a-b", "c").unwrap(), dc::Store::at(dir, "a", "b-c").unwrap());
    assert_ne!(one.principal(), two.principal());
    for (a, b) in [(one.key_path(), two.key_path()), (one.public_key_path(), two.public_key_path()), (one.cert_path(), two.cert_path())] {
        assert_ne!(a, b, "the two principals share {}", a.display());
    }
    let (upper, lower) = (dc::Store::at(root.join("consent"), "Fleet", "x").unwrap(), dc::Store::at(root.join("consent"), "fleet", "x").unwrap());
    for (a, b) in [(upper.key_path(), lower.key_path()), (upper.public_key_path(), lower.public_key_path()), (upper.cert_path(), lower.cert_path())] {
        assert_ne!(a.to_string_lossy().to_lowercase(), b.to_string_lossy().to_lowercase(), "consent:Fleet/x and consent:fleet/x share a file on a case-insensitive filesystem: {}", a.display());
    }
    // On this filesystem, whichever it is: a key made for one is not there for the other, and neither replaces the other's.
    let made = upper.create_key(false).expect("a key for consent:Fleet/x");
    let other = lower.create_key(false).expect("and one for consent:fleet/x, not refused as existing");
    assert_ne!(made.point(), other.point());
    assert_eq!(std::fs::read(upper.public_key_path()).unwrap(), made.spki_pem().into_bytes(), "the first principal's files are its own still");
    let _ = std::fs::remove_dir_all(&root);
}

/// **`enrol` keeps the certificate alone** (#813's review, F12). The file handed to `enrol` holds the certificate,
/// some text, and another PEM block — here a block marked as a private key. What is kept beside the key, and what
/// the publication door would be sent, is the one certificate, re-encoded: nothing else of the file.
/// Seen failing first: the file kept as given (*the kept file holds what was beside the certificate*).
#[test]
fn enrol_keeps_the_certificate_alone() {
    let root = scratch("pem");
    let (store, ca) = api_store(&root, false);
    let public = std::fs::read_to_string(store.public_key_path()).unwrap();
    let cert = std::fs::read_to_string(mycelium::issue_consent_cert(&ca, "fleet", "alex", &public, mycelium::ConsentGrade::Tty, &root.join("out")).unwrap()).unwrap();
    let given = format!("a note the issuer left\n{cert}-----BEGIN PRIVATE KEY-----\nbm90IGEga2V5LCBidXQgbWFya2VkIGFzIG9uZQ==\n-----END PRIVATE KEY-----\n");
    let enrolment = store.enrol(&given).expect("the certificate in it is this key's");
    let kept = std::fs::read_to_string(store.cert_path()).unwrap();
    for (what, text) in [("the kept file", kept.as_str()), ("what the door is sent", enrolment.cert_pem())] {
        assert!(!text.contains("PRIVATE KEY") && !text.contains("a note"), "{what} holds what was beside the certificate:\n{text}");
        assert_eq!(text.matches("-----BEGIN ").count(), 1, "{what} is one PEM block");
    }
    assert_eq!(kept.split_whitespace().collect::<String>(), cert.split_whitespace().collect::<String>(), "and it is the certificate that was issued");
    assert_eq!(store.enrolment().expect("read back").cert_pem(), kept);
    let _ = std::fs::remove_dir_all(&root);
}

/// **`enrol` and `approve` refuse a certificate that is not valid now** (#813's review, F12; its confirm round, C9):
/// `dev` would have signed under it and every node refused. A certificate whose validity ended is refused by name
/// at enrolment and, written beside the key, on use; so is one whose validity has not begun; and one whose validity
/// cannot be read is refused rather than taken as valid.
/// Seen failing first, one build per plant: the validity not read (*an expired certificate was enrolled*);
/// `notBefore` not compared (*a certificate not valid yet was enrolled*); an unreadable bound accepted (*a
/// certificate whose validity cannot be read was enrolled*).
#[test]
fn a_certificate_that_is_not_valid_now_is_refused_at_enrolment_and_on_use() {
    let root = scratch("expired");
    let (store, ca) = api_store(&root, false);
    let public = std::fs::read_to_string(store.public_key_path()).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    let issue = |until: u64, tag: &str| mycelium_core::tls::with_not_after_for_test(until, || std::fs::read_to_string(mycelium::issue_consent_cert(&ca, "fleet", "alex", &public, mycelium::ConsentGrade::Tty, &root.join(tag)).unwrap()).unwrap());
    let (expired, current) = (issue(now - 3_600_000, "expired"), issue(now + 3_600_000, "current"));
    let refused = store.enrol(&expired).map(|_| ()).expect_err("an expired certificate was enrolled");
    assert_eq!(refused.code(), "certificate_expired", "{refused}");
    assert!(!store.cert_path().exists(), "and it was not kept");
    store.enrol(&current).expect("one still valid is enrolled");
    mycelium_core::consent_key::store_public(&store.cert_path(), expired.as_bytes()).unwrap();
    assert_eq!(store.enrolment().map(|_| ()).expect_err("an expired certificate beside the key").code(), "certificate_expired");

    // #813's confirm round, C9: the other bound, and a bound that cannot be read. These two certificates are made
    // here over the stored key itself (the chain is not judged on the developer's side), since the CA's issuer
    // always dates `notBefore` 2024.
    let pair = rcgen::KeyPair::try_from(std::fs::read(store.key_path()).unwrap().as_slice()).expect("the stored PKCS#8");
    let with_validity = |not_before: (i32, u8, u8), not_after: (i32, u8, u8)| {
        let mut params = rcgen::CertificateParams::new(vec![]).unwrap();
        params.subject_alt_names = vec![rcgen::SanType::URI(rcgen::Ia5String::try_from(mycelium::consent_uri("fleet", "alex")).unwrap())];
        params.custom_extensions = vec![rcgen::CustomExtension::from_oid_content(mycelium_core::tls::CONSENT_GRADE_OID, mycelium::ConsentGrade::Tty.extension_der())];
        params.not_before = rcgen::date_time_ymd(not_before.0, not_before.1, not_before.2);
        params.not_after = rcgen::date_time_ymd(not_after.0, not_after.1, not_after.2);
        params.self_signed(&pair).unwrap().pem()
    };
    store.enrol(&with_validity((2024, 1, 1), (2099, 1, 1))).expect("the control: such a certificate, valid now, is enrolled");
    let refused = store.enrol(&with_validity((2090, 1, 1), (2099, 1, 1))).map(|_| ()).expect_err("a certificate not valid yet was enrolled");
    assert_eq!(refused.code(), "certificate_not_yet_valid", "{refused}");
    // A `notAfter` before 1970 is one the reader cannot express: unreadable, and so refused rather than taken as unexpired.
    let refused = store.enrol(&with_validity((1960, 1, 1), (1965, 1, 1))).map(|_| ()).expect_err("a certificate whose validity cannot be read was enrolled");
    assert_eq!(refused.code(), "not_a_consent_certificate", "{refused}");
    assert!(refused.to_string().contains("validity"), "{refused}");
    let _ = std::fs::remove_dir_all(&root);
}

/// **The key and its certificate are read and checked again before every prompt** (#813's review, F8). A `dev` fed
/// ids on its standard input runs for as long as that stays open. While the first request's prompt is up, the
/// certificate beside the key is replaced by one that calls the key an OS-presence key. The first request, read
/// before the change, is signed; the second is answered `enrolment_refused` (`grade_not_this_keys`) and **raises no
/// prompt**.
/// Seen failing first: the enrolment read once, before the queue is served (*the second request was signed*).
#[test]
fn the_key_and_its_certificate_are_checked_again_before_every_prompt() {
    let root = scratch("every-prompt");
    let (store, ca) = api_store(&root, true);
    let public = std::fs::read_to_string(store.public_key_path()).unwrap();
    let presence = std::fs::read_to_string(mycelium::issue_consent_cert(&ca, "fleet", "alex", &public, mycelium::ConsentGrade::OsPresenceMacosUnbound, &root.join("presence")).unwrap()).unwrap();
    let (base, posted) = stub_gateway(vec![row("r1"), row("r2")]);
    let gateway = dc::Gateway::new(&base, Some("relay".into())).unwrap();
    let cert_path = store.cert_path();
    let mut approver = Scripted { answers: vec![dc::Approval::Approve, dc::Approval::Approve], prompts: 0, during: Some(Box::new(move || mycelium_core::consent_key::store_public(&cert_path, presence.as_bytes()).unwrap())) };
    let (handle, queue) = dc::queue(dc::QUEUE_BOUND);
    handle.offer("r1").unwrap();
    handle.offer("r2").unwrap();
    drop(handle);
    let mut said: Vec<(String, String, Value)> = Vec::new();
    runtime().block_on(async { tokio::time::timeout(std::time::Duration::from_secs(30), dc::serve(&gateway, &store, &mut approver, queue, 300, &mut |r, a| said.push((r.to_string(), a.word().to_string(), a.line(r))))).await.expect("served within 30 s") });
    let words: Vec<(&str, &str)> = said.iter().map(|(r, w, _)| (r.as_str(), w.as_str())).collect();
    assert_eq!(words, [("r1", "signed"), ("r2", "enrolment_refused")], "the second request was signed, or misnamed: {said:?}");
    assert_eq!(said[1].2["refusal"], "grade_not_this_keys", "{}", said[1].2);
    assert_eq!((approver.prompts, posted.lock().unwrap().len()), (1, 1), "one prompt, one decision sent");
    let _ = std::fs::remove_dir_all(&root);
}

/// **A gateway's answer and a line of standard input are bounded** (#813's review, F11). A pending list of more
/// than `GATEWAY_ANSWER_MAX` bytes is not read further: the request is answered `gateway`, by name, and nothing is
/// shown. A line of standard input longer than `REQUEST_ID_MAX` bytes is no id: its start is reported, the rest is
/// read and dropped, and the next line is read as usual.
/// Seen failing first, one build per plant: the answer read whole (*an answer over the bound was read: signed*); the
/// line read whole (*a line over the bound was taken as an id*).
#[test]
fn a_gateways_answer_and_a_line_of_standard_input_are_bounded() {
    let root = scratch("bounds");
    let (store, _ca) = api_store(&root, true);
    let (base, posted) = stub_gateway_with(vec![row("r1")], dc::GATEWAY_ANSWER_MAX + 1024, None);
    let gateway = dc::Gateway::new(&base, Some("relay".into())).unwrap();
    let mut approver = Scripted { answers: vec![dc::Approval::Approve], prompts: 0, during: None };
    let enrolment = store.enrolment().unwrap();
    let answer = runtime().block_on(async { tokio::time::timeout(std::time::Duration::from_secs(30), dc::answer_one(&gateway, &enrolment, &mut approver, "r1", 300)).await.expect("answered within 30 s") });
    assert!(matches!(&answer, dc::Answer::Gateway(why) if why.contains("over")), "an answer over the bound was read: {}", answer.word());
    assert_eq!((approver.prompts, posted.lock().unwrap().len()), (0, 0), "nothing shown, nothing sent");

    let mut input = std::io::Cursor::new([vec![b'a'; 3 * 1024 * 1024], b"\nr2\n\n  r3  \n".to_vec(), vec![b'b'; dc::REQUEST_ID_MAX], b"\n".to_vec(), vec![b'c'; dc::REQUEST_ID_MAX + 1]].concat());
    let mut read = Vec::new();
    while let Some(line) = dc::read_request_id(&mut input) {
        read.push(line.map(|id| if id.len() > 8 { format!("{}…{}", &id[..1], id.len()) } else { id }));
    }
    assert_eq!(read, [Err("a".repeat(32)), Ok("r2".into()), Ok(String::new()), Ok("r3".into()), Ok(format!("b…{}", dc::REQUEST_ID_MAX)), Err("c".repeat(32))], "a line over the bound was taken as an id, or a line after it was lost");
    assert_eq!(dc::Answer::IdTooLong.word(), "request_id_too_long");
    let _ = std::fs::remove_dir_all(&root);
}

/// **Text typed before the prompt and not yet entered is not the answer to a bare return** (#813's review, F7).
/// `approve` is typed, with no return, before `dev` has shown anything; when the prompt appears, only a return is
/// entered. The terminal's input is flushed before the prompt is written, so the line that is read is empty and
/// nothing is signed. **What stays, stated:** a process that can type at the terminal types after the prompt, and is
/// signed.
/// Seen failing first: the terminal's input not flushed (*`approve` typed before the prompt was signed on a bare
/// return*).
#[test]
fn text_typed_before_the_prompt_and_not_entered_is_not_the_answer() {
    let root = scratch("partial");
    let (work, home, _) = set_up(&root, "tty", true);
    let (base, posted) = stub_gateway(vec![row("r1")]);
    let o = helped(&work, &home, "ahead", "approve|", "", &["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "--token", "relay", "r1"]);
    let out = text(&o);
    let answered: Vec<(String, String)> = lines(&out).into_iter().map(|(r, a, _)| (r, a)).collect();
    assert_eq!(answered, [("r1".into(), "dismissed".into())], "`approve` typed before the prompt was signed on a bare return:\n{out}");
    assert!(posted.lock().unwrap().is_empty(), "nothing was sent");
    let _ = std::fs::remove_dir_all(&root);
}

/// **`approve -` is refused when standard input is the terminal** (#813's review, F10): the ids and the answers
/// would be read from one keyboard, and a typed `approve` taken as an id. Refused by name, exit 2, before anything
/// is asked.
/// Seen failing first: the refusal removed (the binary read ids from the terminal until the helper's bound).
#[test]
fn approve_from_standard_input_is_refused_when_standard_input_is_the_terminal() {
    let root = scratch("stdin-tty");
    let (work, home, _) = set_up(&root, "tty", true);
    let (base, posted) = stub_gateway(vec![row("r1")]);
    let out = Command::new("python3")
        .args(["-c", HELPER, "pty", "approve", env!("CARGO_BIN_EXE_mycelium"), "dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", &base, "--token", "relay", "-"])
        .current_dir(&work).env("HOME", &home).env("STDIN", "").env_remove("XDG_DATA_HOME").env_remove("MYCELIUM_GATEWAY_TOKEN").stdin(Stdio::null()).output().expect("python3");
    assert_eq!(out.status.code(), Some(2), "refused, not left reading ids from the terminal (124 is the helper's 240 s bound):\n{}", text(&out));
    assert!(text(&out).contains("requests_from_a_terminal"), "by name:\n{}", text(&out));
    assert!(posted.lock().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

/// **The `not_enrolled` refusal quotes commands that run as written** (#813's review, F12). `approve` before `enrol`
/// is refused by name and quotes two commands: the CA holder's `tls issue`, complete but for the two directories
/// only that host knows, and `dev`'s own `enrol`. Each is run here **through `sh -c`, exactly as quoted**, with the
/// two directories filled in; then `approve` finds the enrolment.
/// Seen failing first, one build per plant: the earlier wording, which left `--issuer`, `--ca-dir` and `--out` off
/// (the quoted command exited 2); the public key's path left unquoted (`sh` split it and `tls issue` printed its
/// usage — #813's confirm round, C7; the home holds a space and a quote on every platform since the second confirm
/// round, N3, so this fails on Linux too and not only where the consent path has a space of its own).
#[test]
fn the_not_enrolled_refusal_quotes_commands_that_run() {
    // The scratch name holds a space and a single quote, so the home does on every platform (on Linux the consent
    // path has no space of its own, and an unquoted command would otherwise run there).
    let root = scratch("quoted it's here");
    let (work, home, _) = set_up(&root, "tty", false);
    assert!(home.to_string_lossy().contains(' ') && home.to_string_lossy().contains('\''), "the home holds a space and a quote: {}", home.display());
    let o = mycelium(&work, &home).args(["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "r1"]).output().unwrap();
    let said = text(&o);
    assert!(o.status.code() == Some(2) && said.contains("not_enrolled"), "{said}");
    let quoted: Vec<String> = said.split('`').filter(|part| part.starts_with("mycelium ")).map(|c| c.replace("<ca-dir>", &dc::sh_quoted(root.join("ca").to_str().unwrap())).replace("<out-dir>", &dc::sh_quoted(root.join("out2").to_str().unwrap()))).collect();
    assert_eq!(quoted.len(), 2, "two commands are quoted: {said}");
    assert!(quoted[0].starts_with("mycelium tls issue ") && quoted[1].starts_with("mycelium dev consent enrol "), "{quoted:?}");
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_mycelium")).parent().unwrap().to_path_buf();
    let path = format!("{}:{}", bin_dir.display(), std::env::var("PATH").unwrap_or_default());
    for command in &quoted {
        assert!(!command.contains('<'), "nothing is left to fill in but the two directories: {command}");
        // Exactly as a person would: the printed line, pasted into a POSIX shell. The public key's path holds a
        // space on macOS (`Application Support`), so the line must quote it.
        let ran = Command::new("sh").arg("-c").arg(command).current_dir(&work).env("PATH", &path).env("HOME", &home).env_remove("XDG_DATA_HOME").env_remove("MYCELIUM_GATEWAY_TOKEN").output().expect("sh");
        assert!(ran.status.success(), "the quoted command ran as written, through `sh -c`: {command}\n{}", text(&ran));
    }
    let o = mycelium(&work, &home).args(["dev", "consent", "approve", "--issuer", "fleet", "--name", "alex", "--gateway", "http://127.0.0.1:1", "r1"]).output().unwrap();
    assert!(!text(&o).contains("not_enrolled"), "enrolled now: {}", text(&o));
    let _ = std::fs::remove_dir_all(&root);
}

