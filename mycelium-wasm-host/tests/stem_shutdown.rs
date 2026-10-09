//! **`mycelium-stem` shuts down on SIGTERM as it does on SIGINT** — writing its `--trace-dir` output and
//! withdrawing its installs (doc-coverage run 22, code gap 2).
//!
//! `docker stop` and a Kubernetes pod stop send SIGTERM. The stem awaited `ctrl_c()` only, so a stopped
//! container was killed without its decision trace and without a graceful withdrawal — and the runbook's
//! "run the stem with `--trace-dir`, read the `prov.shed` decisions" collected nothing.
#![cfg(all(unix, feature = "stem"))]

use std::net::TcpStream;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sigterm_writes_the_trace_and_exits_cleanly() {
    let dir = std::env::temp_dir().join(format!("stem-sigterm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let units = dir.join("units.toml");
    std::fs::write(&units, "").unwrap();
    let trace = dir.join("trace");
    let port = mycelium::test_util::alloc_port();

    let mut child = Command::new(env!("CARGO_BIN_EXE_mycelium-stem"))
        .args(["--units", units.to_str().unwrap(), "--port", &port.to_string(), "--host", "127.0.0.1",
               "--trace-dir", trace.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the stem binary starts");

    // Up when its gossip port accepts a connection.
    let deadline = Instant::now() + Duration::from_secs(30);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the stem never bound its port");
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the stem exited before binding: {status}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let killed = Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap();
    assert!(killed.success());

    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the stem ignored SIGTERM for 20 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(status.success(), "a SIGTERM is an orderly shutdown, not a crash: {status}");
    assert!(trace.join("decisions.jsonl").exists(), "the decision trace is written on SIGTERM");
    let _ = std::fs::remove_dir_all(&dir);
}
