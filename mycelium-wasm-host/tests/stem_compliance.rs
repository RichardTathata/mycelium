//! **The published stem's feature set takes the gateway's credential and authority tables** — a `mycelium-stem` built
//! with `compliance` (release.yml's `STEM_FEATURES`) starts with a named-token table and an `[authority]` table, and
//! attaches the authority as the `mycelium` binary does: the stem calls `GossipAgent::start()`, whose preflight plans
//! `[authority]` and attaches the evaluator, the execution authority, provider enforcement and the evidence journal
//! before the lifecycle boundary (`src/agent/lifecycle.rs`, `src/agent/declared_authority.rs`). So the gateway refuses
//! an unauthenticated request, the token is honoured, and the guarantee report reads the seam enforced.
//!
//! A test build cannot show the refusal without the feature: this crate's dev-dependencies turn mycelium's
//! `compliance` on for every target `cargo test` builds, the binary included. The refusal was seen by hand on a
//! `cargo build` without it (the PR records it); this test pins the release set.
#![cfg(all(unix, feature = "stem", feature = "compliance"))]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
async fn a_compliance_stem_starts_with_a_token_table_and_attaches_its_authority() {
    let dir = std::env::temp_dir().join(format!("stem-compliance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("units")).unwrap();
    let unit = dir.join("units").join("stem-unit.toml");
    std::fs::write(&unit, "principal = \"stem-unit\"\n\n[[rule]]\nactor = \"*\"\noperation = \"tools/call\"\nresource = \"tool:echo.say\"\n").unwrap();
    let port = mycelium::test_util::alloc_port();
    let http = mycelium::test_util::alloc_port();
    let cfg = dir.join("node.toml");
    std::fs::write(&cfg, format!(
        "bind_address = \"127.0.0.1\"\nbind_port = {port}\nhttp_port = {http}\n\n\
         [[gateway_named_tokens]]\nname = \"ops\"\ntoken = \"ops-secret\"\nscopes = [\"*\"]\n\n\
         [persistence]\nbase_path = \"{data}\"\n\n\
         [authority]\ndeclarations = \"{units}\"\nunit = \"stem-unit\"\njournal = \"{journal}\"\n",
        data = dir.join("data").display(), units = dir.join("units").display(), journal = dir.join("evidence.journal").display(),
    )).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_mycelium-stem"))
        .args(["--units", unit.to_str().unwrap(), "--config", cfg.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the stem binary starts");
    let mut guard = KillOnDrop(child);

    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{http}/gateway/guarantees");
    let deadline = Instant::now() + Duration::from_secs(60);
    let anonymous = loop {
        if let Ok(r) = client.get(&url).send().await {
            break r.status().as_u16();
        }
        if let Some(status) = guard.0.try_wait().unwrap() {
            let mut err = String::new();
            use std::io::Read;
            let _ = guard.0.stderr.take().unwrap().read_to_string(&mut err);
            panic!("the stem exited before its gateway answered ({status}):\n{err}");
        }
        assert!(Instant::now() < deadline, "the stem's gateway never answered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(anonymous, 401, "the token table is enforced: no bearer, no answer");

    let report: serde_json::Value = client.get(&url).bearer_auth("ops-secret").send().await.unwrap().json().await.unwrap();
    let state = |id: &str| {
        report["entries"].as_array().unwrap().iter().find(|e| e["id"] == id).map(|e| e["resolution"]["state"].clone())
    };
    for id in ["gw.not_open", "ae.authorised_at_seam", "ae.recorded_before_dispatch", "authz.execution_authority", "prov.enforcement"] {
        assert_eq!(state(id), Some(serde_json::json!("enforced")), "{id}: the stem attaches [authority] as start() does\n{report}");
    }
    assert!(dir.join("evidence.journal").exists(), "the evidence journal is opened at start");

    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
}
