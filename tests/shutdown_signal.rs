//! **A second stop signal exits a hung shutdown** (`mycelium::shutdown::ShutdownSignal`).
//!
//! The handlers stay installed after the first signal, so without a watcher a second Ctrl-C or SIGTERM did
//! nothing and a shutdown that hung waited for SIGKILL. The test runs its own binary as a child that takes
//! the first signal and then hangs, and requires the second to end it with `128 + SIGTERM`.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CHILD: &str = "MYCELIUM_SHUTDOWN_SIGNAL_CHILD";

/// The child half: inert unless the parent sets `CHILD`.
#[test]
fn shutdown_signal_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let signal = mycelium::shutdown::ShutdownSignal::install().unwrap();
        println!("ready");
        signal.wait().await.unwrap();
        println!("first");
        // A shutdown that hangs.
        std::future::pending::<()>().await;
    });
}

#[test]
fn a_second_signal_exits_a_hung_shutdown() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "shutdown_signal_child", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Read the child's output on a thread, so a missing marker fails on a deadline instead of hanging.
    // libtest prints `test NAME ... ` without a newline before the test's own output, so a marker can end
    // a line rather than fill it.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() { break; }
        }
    });
    let wait_for = |want: &str| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(l) if l.trim_end().ends_with(want) => return,
                Ok(_) => continue,
                Err(e) => panic!("the child never printed {want:?}: {e}"),
            }
        }
    };
    let term = |pid: u32| assert!(Command::new("kill").args(["-TERM", &pid.to_string()]).status().unwrap().success());

    wait_for("ready");
    term(child.id());
    wait_for("first");
    term(child.id());

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("a second SIGTERM did not end a hung shutdown");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(143), "a forced exit reports 128 + SIGTERM: {status}");
}
