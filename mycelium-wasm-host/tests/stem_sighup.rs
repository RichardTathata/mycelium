//! **`mycelium-stem` applies a rewritten egress allow-list on SIGHUP, without a restart** (post-360 row M's live
//! allow-list, the binary's half). `Stem::reload_egress_hosts` is tested in the library; this runs the real
//! binary and sends the real signal, so what it pins is the path between them — the handler installed, the unit
//! file re-read, the list reaching the running proxy — and the handler's promise for a file that does not load:
//! every list stands; for a file that loads but names a list the node refuses: that activation's list stands; and
//! for a stem whose unit file names no egress table: SIGHUP is handled, never SIGHUP's default (terminate).
//!
//! The observable is the proxy itself. The stem is its own librarian over a one-blob library; the blob's
//! confined activation writes its proxy URL (port and token, from its cleared environment) into its placement
//! directory, and the test then asks that proxy for two loopback upstreams, A and B, as the command would.
#![cfg(all(unix, feature = "stem"))]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::SigningKey;
use mycelium::Capability;
use mycelium_wasm_host::{ArtifactKind, FsLibrarySource, InstallableEntry, Manifest, MANIFEST_FILE};

/// Kills the stem if the test panics before it exits, so a failed run leaves no process behind.
struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// A loopback upstream that records each request line and answers `200 ok`.
fn upstream() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for sock in listener.incoming() {
            let Ok(mut sock) = sock else { return };
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).unwrap_or(0);
            let head = String::from_utf8_lossy(&buf[..n]);
            log.lock().unwrap().push(head.lines().next().unwrap_or_default().to_string());
            let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        }
    });
    (port, seen)
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The proxy, as the activation's environment names it: `http://<user>:<token>@127.0.0.1:<port>`.
struct Proxy {
    port: u16,
    auth: String,
}

impl Proxy {
    fn parse(url: &str) -> Self {
        let rest = url.trim().strip_prefix("http://").unwrap_or_else(|| panic!("a proxy URL: {url:?}"));
        let (userinfo, addr) = rest.split_once('@').expect("userinfo in the proxy URL");
        let port = addr.trim_end_matches('/').rsplit_once(':').expect("a port").1.parse().expect("a numeric port");
        Self { port, auth: format!("Basic {}", base64(userinfo.as_bytes())) }
    }

    /// The status of `GET http://127.0.0.1:<upstream>/<path>` through the proxy, with its token — or the
    /// transport error, which a caller polling to a deadline counts as "not yet" (#646's review, finding 4).
    fn try_get(&self, upstream: u16, path: &str) -> Result<u16, String> {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).map_err(|e| format!("connect: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        write!(
            s,
            "GET http://127.0.0.1:{upstream}/{path} HTTP/1.1\r\nHost: 127.0.0.1:{upstream}\r\nProxy-Authorization: {}\r\nConnection: close\r\n\r\n",
            self.auth
        )
        .map_err(|e| format!("write: {e}"))?;
        let mut resp = Vec::new();
        let _ = s.read_to_end(&mut resp);
        let resp = String::from_utf8_lossy(&resp);
        resp.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("no HTTP answer: {resp:?}"))
    }

    /// [`Self::try_get`] where an answer is expected now.
    fn get(&self, upstream: u16, path: &str) -> u16 {
        self.try_get(upstream, path).unwrap_or_else(|e| panic!("through the proxy to :{upstream}: {e}"))
    }
}

fn unit_file(head: &str, hosts: &str) -> String {
    format!(
        "{head}[[presence]]\nns=\"data\"\nname=\"pack\"\nmin_providers=1\n\
         [[activation]]\nns=\"data\"\nname=\"pack\"\n\
         command=[\"sh\",\"-c\",\"printf %s \\\"$MYCELIUM_EGRESS_PROXY\\\" > {{dir}}/proxy-url && cp {{path}} {{path}}.active\"]\n\
         probe=[\"test\",\"-f\",\"{{path}}.active\"]\n\
         [activation.confine]\nwrite=[\"{{dir}}\"]\n\
         [activation.confine.egress]\nhosts = {hosts}\n"
    )
}

/// The stem's log so far (it logs to stdout, redirected to `log`).
fn log_of(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

fn wait_for(what: &str, child: &mut std::process::Child, log: &Path, secs: u64, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !cond() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the stem exited ({status}) waiting for {what}; its log:\n{}", log_of(log));
        }
        assert!(Instant::now() < deadline, "{what}: not within {secs} s; the stem's log:\n{}", log_of(log));
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Replace the unit file as `unit-file.md` says to: write a new file, then `rename(2)` it over the old one, so a
/// reload never reads half a file.
fn replace(units: &Path, body: &str) {
    let tmp = units.with_extension("toml.new");
    std::fs::write(&tmp, body).unwrap();
    std::fs::rename(&tmp, units).unwrap();
}

fn sighup(child: &std::process::Child) {
    let sent = Command::new("kill").args(["-HUP", &child.id().to_string()]).status().unwrap();
    assert!(sent.success());
}

#[test]
fn sighup_applies_a_rewritten_egress_allow_list_and_a_file_that_does_not_load_keeps_the_old_one() {
    const T: &str = "sighup_applies_a_rewritten_egress_allow_list_and_a_file_that_does_not_load_keeps_the_old_one";
    match mycelium_wasm_host::activation_sandbox::assess(Some(&mycelium::ConfineDecl::default())) {
        mycelium_wasm_host::Confinement::Confined { .. } => {}
        other => {
            if std::env::var_os("MYCELIUM_CONFINE_LIVE_REQUIRED").is_some() {
                panic!("{T}: MYCELIUM_CONFINE_LIVE_REQUIRED is set and this host cannot confine: {other:?}");
            }
            eprintln!("{T}: SKIPPED — this host cannot confine ({other:?}); set MYCELIUM_CONFINE_LIVE_REQUIRED to fail instead");
            return;
        }
    }

    let dir: PathBuf = std::env::temp_dir().join(format!("stem-sighup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let lib_dir = dir.join("library");
    let placed = dir.join("placed");
    std::fs::create_dir_all(&placed).unwrap();
    // The stem is its own librarian over a one-blob library, the manifest signed by a test key.
    let lib = FsLibrarySource::open(&lib_dir).unwrap();
    let bytes = b"bytes for a confined runtime behind its egress proxy";
    let id = lib.store(bytes).unwrap();
    let key = SigningKey::from_bytes(&[7u8; 32]);
    let entry = InstallableEntry::new(Capability::new("data", "pack"), id)
        .with_kind(ArtifactKind::Blob)
        .with_cost(bytes.len() as u64, 1)
        .signed_by(&key);
    Manifest::from_entries(vec![entry]).save(&lib_dir.join(MANIFEST_FILE)).unwrap();
    let publisher: String = key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let head = format!(
        "principal=\"stem\"\n[hosts]\nkinds=[\"blob\"]\ntrusted_publishers=[\"ed25519:{publisher}\"]\nplacement_root=\"{}\"\n",
        placed.display()
    );

    let ((a, at_a), (b, at_b)) = (upstream(), upstream());
    let units = dir.join("units.toml");
    std::fs::write(&units, unit_file(&head, &format!("[\"http://127.0.0.1:{a}\"]"))).unwrap();
    // The node's own egress allow-list: loopback only, so a reload naming another host loads and is refused.
    let config = dir.join("gossip.toml");
    std::fs::write(&config, "[egress]\nallow_hosts = [\"127.0.0.1\"]\n").unwrap();
    let log = dir.join("stem.log");
    let port = mycelium::test_util::alloc_port();
    let child = Command::new(env!("CARGO_BIN_EXE_mycelium-stem"))
        .args([
            "--units", units.to_str().unwrap(), "-c", config.to_str().unwrap(),
            "--port", &port.to_string(), "--host", "127.0.0.1",
            "--library", lib_dir.to_str().unwrap(),
            "--librarian", lib_dir.join(MANIFEST_FILE).to_str().unwrap(),
            "--publisher", &format!("ed25519:{publisher}"),
            "--self-elect", "1", "--tick-ms", "200",
        ])
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the stem binary starts");
    let mut guard = KillOnDrop(child);
    let child = &mut guard.0;

    // 1. Up, the reload handler installed, the blob placed and its confined activation run: it wrote its
    //    proxy's URL. (A freshly linked binary can stall at its first exec on macOS — a generous bound.)
    wait_for("the SIGHUP handler", child, &log, 90, || log_of(&log).contains("SIGHUP re-reads the unit file"));
    let url_file = placed.join("proxy-url");
    wait_for("the activation's proxy URL", child, &log, 90, || std::fs::read_to_string(&url_file).is_ok_and(|s| !s.is_empty()));
    let proxy = Proxy::parse(&std::fs::read_to_string(&url_file).unwrap());

    // 2. A is listed and served; B is refused, and B's upstream sees nothing.
    assert_eq!(proxy.get(a, "first"), 200, "the listed host, through the proxy");
    assert_eq!(proxy.get(b, "refused"), 403, "an unlisted host is refused by the proxy");
    assert!(at_b.lock().unwrap().is_empty(), "the refused host saw nothing: {:?}", at_b.lock().unwrap());

    // 3–4. The unit file rewritten to list B too, then SIGHUP: B is served by the same process, the same proxy.
    replace(&units, &unit_file(&head, &format!("[\"http://127.0.0.1:{a}\", \"http://127.0.0.1:{b}\"]")));
    sighup(child);
    wait_for("B served after SIGHUP", child, &log, 30, || proxy.try_get(b, "after-hup") == Ok(200));
    assert!(log_of(&log).contains("SIGHUP: egress allow-list applied"), "the reload is logged");
    assert!(at_b.lock().unwrap().iter().any(|l| l.starts_with("GET /after-hup")), "{:?}", at_b.lock().unwrap());
    assert!(child.try_wait().unwrap().is_none(), "no restart: the stem is still the same process");

    // 5. A rewrite that does not load (a wildcard names no one host): the handler says so and every list
    //    stands — A and B both still served, by the same process.
    replace(&units, &unit_file(&head, "[\"https://*.example.com\"]"));
    sighup(child);
    wait_for("the refused reload's log line", child, &log, 30, || log_of(&log).contains("SIGHUP: the unit file does not load"));
    assert_eq!(proxy.get(b, "kept"), 200, "the old list stands: B");
    assert_eq!(proxy.get(a, "kept"), 200, "the old list stands: A");
    assert!(at_a.lock().unwrap().iter().any(|l| l.starts_with("GET /kept")));
    assert!(child.try_wait().unwrap().is_none(), "a bad reload is not a crash");

    // 6. A rewrite that loads, but whose list this activation's proxy refuses — a host the node's egress
    //    allow-list denies (#646's review, finding 3: the handler's per-activation refusal, through the binary):
    //    that refusal is logged by name, and the old list stands.
    replace(&units, &unit_file(&head, &format!("[\"http://127.0.0.1:{a}\", \"http://denied.example\"]")));
    sighup(child);
    wait_for("the per-activation refusal's log line", child, &log, 30, || {
        log_of(&log).lines().any(|l| l.contains("SIGHUP: egress allow-list not applied") && l.contains("data/pack"))
    });
    assert_eq!(proxy.get(b, "still"), 200, "the refused list did not replace the old one: B still served");
    assert!(at_b.lock().unwrap().iter().any(|l| l.starts_with("GET /still")));
    assert!(child.try_wait().unwrap().is_none());

    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #646's review, finding 1: a stem whose unit file names **no** egress table handles SIGHUP — re-reads the file,
/// says there is nothing to apply — and keeps running. Seen failing first: the handler was installed only for a
/// unit with an egress table, so SIGHUP took its default action and the stem died.
#[test]
fn a_stem_without_an_egress_table_survives_sighup() {
    let dir = std::env::temp_dir().join(format!("stem-sighup-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let units = dir.join("units.toml");
    std::fs::write(&units, "").unwrap();
    let log = dir.join("stem.log");
    let port = mycelium::test_util::alloc_port();
    let child = Command::new(env!("CARGO_BIN_EXE_mycelium-stem"))
        .args(["--units", units.to_str().unwrap(), "--port", &port.to_string(), "--host", "127.0.0.1"])
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the stem binary starts");
    let mut guard = KillOnDrop(child);
    let child = &mut guard.0;

    // Up when its gossip port accepts a connection, and running its loop.
    wait_for("the stem's port", child, &log, 90, || TcpStream::connect(("127.0.0.1", port)).is_ok());
    wait_for("the stem up", child, &log, 30, || log_of(&log).contains("stem node up"));
    sighup(child);
    wait_for("the SIGHUP handled", child, &log, 30, || log_of(&log).contains("SIGHUP: no egress allow-list to apply"));
    sighup(child);
    std::thread::sleep(Duration::from_millis(300));
    assert!(child.try_wait().unwrap().is_none(), "SIGHUP never terminates the stem");

    // And it still stops in order.
    let _ = Command::new("kill").args(["-TERM", &child.id().to_string()]).status();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the stem ignored SIGTERM for 20 s");
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "{status}");
    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
}
