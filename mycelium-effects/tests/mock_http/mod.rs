//! A destination outside the fleet, for the side-effect contract's tests: an HTTP server on a thread of
//! its own that **loses responses on demand**. A payload beginning `lose:` is applied and the connection
//! closed without an answer (the destination did it and the caller cannot know); `vanish:` is not applied
//! and not answered (the destination did not do it); `boom` answers 500; a replayed key answers
//! `replayed`; a key with other content answers 409. Its probe route answers from what it applied, and
//! while `probes_silent` is set every probe closes without an answer. Every request is counted, so a test
//! can say how many times the destination was reached.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Default)]
pub struct State {
    /// key -> content hash hex, as applied.
    pub committed: Mutex<HashMap<String, String>>,
    /// Every request that reached the server, POST and GET.
    pub reached: AtomicUsize,
    /// POSTs that applied an effect (fresh).
    pub applied: AtomicUsize,
    /// When set, every probe answers nothing.
    pub probes_silent: AtomicBool,
}

pub struct MockHttp {
    pub port: u16,
    pub state: Arc<State>,
}

impl MockHttp {
    pub fn start() -> Self {
        let port = mycelium::test_util::alloc_port();
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind the mock destination");
        let state = Arc::new(State::default());
        let s2 = Arc::clone(&state);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let s = Arc::clone(&s2);
                thread::spawn(move || handle(stream, &s));
            }
        });
        Self { port, state }
    }

    #[allow(dead_code)]
    pub fn reached(&self) -> usize {
        self.state.reached.load(Ordering::SeqCst)
    }
    #[allow(dead_code)]
    pub fn applied(&self) -> usize {
        self.state.applied.load(Ordering::SeqCst)
    }
}

fn handle(mut stream: TcpStream, state: &State) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // Read until the headers' end, then the declared body.
    let (head, body) = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).into_owned();
            let len: usize = head
                .lines()
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = buf[pos + 4..].to_vec();
            while body.len() < len {
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => body.extend_from_slice(&chunk[..n]),
                }
            }
            break (head, body);
        }
    };
    state.reached.fetch_add(1, Ordering::SeqCst);
    let request_line = head.lines().next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let header = |name: &str| head.lines().find_map(|l| l.strip_prefix(name)).map(|v| v.trim().to_string());
    let respond = |stream: &mut TcpStream, status: u16, body: &str| {
        let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let _ = stream.flush();
    };
    match method {
        "POST" => {
            let key = header("Idempotency-Key: ").unwrap_or_default();
            let hash = header("Content-Hash: ").unwrap_or_default();
            let text = String::from_utf8_lossy(&body).into_owned();
            if text == "boom" {
                respond(&mut stream, 500, "the business change refused this payload");
                return;
            }
            if text.starts_with("vanish:") {
                // Not applied, not answered: the caller cannot know, and a probe says not done.
                drop(stream);
                return;
            }
            let outcome = {
                let mut committed = state.committed.lock().unwrap();
                match committed.get(&key) {
                    Some(h) if *h == hash => "replayed",
                    Some(h) => {
                        let msg = format!("conflict {h}");
                        respond(&mut stream, 409, &msg);
                        return;
                    }
                    None => {
                        committed.insert(key.clone(), hash);
                        state.applied.fetch_add(1, Ordering::SeqCst);
                        "fresh"
                    }
                }
            };
            if text.starts_with("lose:") {
                // Applied — and the answer lost.
                drop(stream);
                return;
            }
            respond(&mut stream, 200, outcome);
        }
        "GET" => {
            if state.probes_silent.load(Ordering::SeqCst) {
                drop(stream);
                return;
            }
            let key = path.rsplit("/effects/").next().unwrap_or("").to_string();
            let done = state.committed.lock().unwrap().contains_key(&key);
            respond(&mut stream, 200, if done { "done" } else { "not_done" });
        }
        _ => respond(&mut stream, 405, "method"),
    }
}
