//! The Phase-1 hinge, end to end: a **curator over a `GitStore`** — proposals drain into real,
//! scoped git commits. This is the council-wiki substrate shape from
//! `docs/design/transparency-council-substrate.md`: the elected single writer per council group
//! applies agent proposals and every apply is a commit to `councils/<slug>`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_wiki::{mint_section_id, GitStore, GitStoreConfig, Wiki, WikiConfig, WikiRole, WikiStore};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curator_applies_proposals_as_scoped_git_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(
        GitStore::open(GitStoreConfig {
            dir: tmp.path().to_path_buf(),
            subdir: "councils/testville".into(),
            message_prefix: "wiki(testville)".into(),
            ..Default::default()
        })
        .unwrap(),
    );

    let agent = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let wiki = Wiki::new(
        Arc::clone(&agent),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store),
    )
    .await;

    let section = mint_section_id("testville", "minutes/2026-08-15", 1, 1);
    wiki.propose(
        "minutes/2026-08-15",
        section,
        "Opening",
        "The council opened the meeting.",
        Default::default(),
    );

    // Structural poll: the drain applies the proposal → the page reads back from the store.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if store.read("minutes/2026-08-15").unwrap().is_some() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the curator never applied the proposal");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The applies are real git commits (section write + manifest splice), scoped and prefixed.
    let count: u32 = git(tmp.path(), &["rev-list", "--count", "HEAD"]).trim().parse().unwrap();
    assert!(count >= 2, "section write + manifest splice = at least two commits, got {count}");
    let subjects = git(tmp.path(), &["log", "--format=%s"]);
    assert!(subjects.lines().all(|l| l.starts_with("wiki(testville): ")), "prefixed messages: {subjects}");
    let touched = git(tmp.path(), &["log", "--name-only", "--format="]);
    assert!(
        touched.lines().filter(|l| !l.is_empty()).all(|l| l.starts_with("councils/testville/")),
        "every commit is scoped to the council subtree: {touched}"
    );
    // …and the document in history is pure markdown with the applied text.
    let doc = git(tmp.path(), &["show", "HEAD:councils/testville/minutes/2026-08-15.md"]);
    assert!(doc.contains("The council opened the meeting."), "the applied text is in the committed doc");

    wiki.shutdown().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
