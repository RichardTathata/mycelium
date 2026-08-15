//! The `GitMirror` projection sink — real `git` in a tempdir (the CLI is the dependency; CI and any
//! operator wanting a git mirror have it). Verifies the design contract of
//! `docs/design/wiki-git-store.md`: one commit per applied round, **pure** rendered documents (no
//! CAS tokens — versioning is git's ancestry), history retained, egress fail-closed, sink failures
//! never load-bearing, `rebuild()` heals everything, and the curator's `drain_once` actually
//! notifies the sink (via a recording sink — the trait seam, no store downcasts anywhere).
#![allow(clippy::field_reassign_with_default)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycelium::EgressPolicy;
use mycelium_wiki::{
    mint_section_id, AppliedRound, ChangeSink, FsStore, GitMirror, GitMirrorConfig, Section,
    WikiStore,
};

fn sec(id: &str, heading: &str, body: &str, attrs: &[(&str, &str)]) -> Section {
    Section {
        id: Arc::from(id),
        heading: heading.into(),
        body: body.into(),
        attributes: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
    }
}

fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// Run git in `dir`, panicking on failure — test harness only.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn round(group: &str, pages: &[&str], proposals: usize, authors: &[&str]) -> AppliedRound {
    AppliedRound {
        group: group.into(),
        pages: pages.iter().map(|p| p.to_string()).collect(),
        proposals,
        authors: authors.iter().map(|a| a.to_string()).collect(),
    }
}

/// One commit per applied round; the rendered document is **pure** (front-matter = page path +
/// attributes, section ids as comments — byte-exact, no CAS/version tokens); an edit makes a second
/// commit and `git show HEAD~1:` still recovers the first text (history retained — the audit
/// property the projection exists for); an idempotent re-round commits nothing.
#[test]
fn one_commit_per_round_pure_document_history_retained() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(FsStore::open(tmp.path().join("store"), "council").unwrap());
    let page = "southwark/2024/cabinet/2024-03-11";
    store
        .write_page(
            page,
            &[sec("s1", "Retrofit scheme approved", "The cabinet approves the borough-wide retrofit.",
                  &[("decision_id", "FTT-2024-0311-07"), ("topic", "climate")])],
            &attrs(&[("council", "southwark"), ("year", "2024")]),
        )
        .unwrap();

    let mirror_dir = tmp.path().join("mirror");
    let mirror = GitMirror::open(
        Arc::clone(&store),
        GitMirrorConfig { dir: mirror_dir.clone(), ..Default::default() },
    )
    .unwrap();

    mirror.round_applied(&round("council", &[page], 1, &["ivy"]));

    let log = git(&mirror_dir, &["log", "--format=%s"]);
    assert_eq!(log.lines().count(), 1, "one applied round = one commit");
    assert!(log.contains("1 proposal(s) by ivy"), "provenance in the message: {log}");

    let rendered = std::fs::read_to_string(mirror_dir.join(format!("{page}.md"))).unwrap();
    let expected = "---\n\
        page: southwark/2024/cabinet/2024-03-11\n\
        council: southwark\n\
        year: 2024\n\
        ---\n\
        \n\
        <!-- section s1 · decision_id=FTT-2024-0311-07 · topic=climate -->\n\
        # Retrofit scheme approved\n\
        \n\
        The cabinet approves the borough-wide retrofit.\n";
    assert_eq!(rendered, expected, "the projection carries the document, not the machinery");

    // Edit → second commit; the first text stays reachable in history.
    store
        .write_page(
            page,
            &[sec("s1", "Retrofit scheme approved", "Amended: the scheme is phased over two years.",
                  &[("decision_id", "FTT-2024-0311-07"), ("topic", "climate")])],
            &attrs(&[("council", "southwark"), ("year", "2024")]),
        )
        .unwrap();
    mirror.round_applied(&round("council", &[page], 2, &["ivy", "rowan"]));
    assert_eq!(git(&mirror_dir, &["log", "--format=%s"]).lines().count(), 2);
    let old = git(&mirror_dir, &["show", &format!("HEAD~1:{page}.md")]);
    assert!(old.contains("borough-wide retrofit"), "history must retain the pre-edit text");

    // Idempotent re-round: store unchanged → nothing to record.
    mirror.round_applied(&round("council", &[page], 2, &["ivy"]));
    assert_eq!(git(&mirror_dir, &["log", "--format=%s"]).lines().count(), 2, "no empty commits");
}

/// Egress is fail-closed at construction: a non-local remote whose host the policy denies refuses
/// to open. A local-path remote is exempt (it leaves no machine) even under a restrictive policy —
/// and actually round-trips a push, with the divergence tripwire quiet.
#[test]
fn egress_fail_closed_and_local_remote_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(FsStore::open(tmp.path().join("store"), "g").unwrap());
    store.write_page("p", &[sec("s1", "H", "body", &[])], &BTreeMap::new()).unwrap();

    // Denied host → refused before any git state exists (fail-closed).
    let denied = GitMirror::open(
        Arc::clone(&store),
        GitMirrorConfig {
            dir: tmp.path().join("m-denied"),
            remote: Some("git@github.com:acme/corpus.git".into()),
            egress: Some(EgressPolicy { allow_hosts: vec!["git.internal".into()] }),
            ..Default::default()
        },
    );
    assert!(matches!(denied, Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied));

    // Local bare remote under the same restrictive policy: permitted, and the push lands.
    let remote_dir = tmp.path().join("remote.git");
    std::fs::create_dir_all(&remote_dir).unwrap();
    git(&remote_dir, &["init", "--bare", "-b", "main"]);
    let mirror = GitMirror::open(
        Arc::clone(&store),
        GitMirrorConfig {
            dir: tmp.path().join("m-local"),
            remote: Some(remote_dir.to_string_lossy().into_owned()),
            egress: Some(EgressPolicy { allow_hosts: vec!["git.internal".into()] }),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(mirror.mirror_pages(&["p".into()], "wiki(g): test round").unwrap());
    mirror.push_now().unwrap();
    assert_eq!(git(&remote_dir, &["log", "--format=%s", "main"]).lines().count(), 1);
    assert_eq!(mirror.push_divergences(), 0, "tripwire quiet on an honest remote");
}

/// A sink failure is the sink's problem: destroying the mirror after open must not panic the
/// notification path (the apply already landed in the store; `rebuild()` into a fresh dir heals).
#[test]
fn sink_failure_is_never_load_bearing() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(FsStore::open(tmp.path().join("store"), "g").unwrap());
    store.write_page("p", &[sec("s1", "H", "body", &[])], &BTreeMap::new()).unwrap();
    let mirror_dir = tmp.path().join("mirror");
    let mirror =
        GitMirror::open(Arc::clone(&store), GitMirrorConfig { dir: mirror_dir.clone(), ..Default::default() })
            .unwrap();
    std::fs::remove_dir_all(&mirror_dir).unwrap();
    mirror.round_applied(&round("g", &["p"], 1, &["ivy"])); // must log-and-return, not panic
}

/// `rebuild()` regenerates the whole projection from the system of record — new pages appear, and a
/// previously-committed file whose content the store no longer holds is removed (the erasure
/// procedure's second step: erase in the record, then rebuild a mirror containing only survivors).
#[test]
fn rebuild_regenerates_from_the_record() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(FsStore::open(tmp.path().join("store"), "g").unwrap());
    store.write_page("kept", &[sec("s1", "Kept", "stays", &[])], &BTreeMap::new()).unwrap();

    let mirror_dir = tmp.path().join("mirror");
    let mirror =
        GitMirror::open(Arc::clone(&store), GitMirrorConfig { dir: mirror_dir.clone(), ..Default::default() })
            .unwrap();
    // Commit a file the store does NOT back (simulating content erased from the record).
    std::fs::write(mirror_dir.join("erased.md"), "to be shredded").unwrap();
    assert!(mirror.mirror_pages(&["kept".into()], "seed").unwrap());

    // A page added to the store after the seed commit…
    store.write_page("added/later", &[sec("s2", "Later", "appears", &[])], &BTreeMap::new()).unwrap();

    assert!(mirror.rebuild().unwrap());
    assert!(mirror_dir.join("kept.md").exists());
    assert!(mirror_dir.join("added/later.md").exists(), "rebuild renders the whole corpus");
    assert!(!mirror_dir.join("erased.md").exists(), "rebuild drops what the record no longer holds");
}

/// End-to-end through the **trait seam**: a curator drains a real proposal and notifies the
/// injected sink with the applied round — proving the `drain_once` wiring without any git in the
/// timing path (a recording sink; `GitMirror` is just one `ChangeSink`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curator_drain_notifies_the_change_sink() {
    use mycelium::{GossipAgent, GossipConfig, NodeId};
    use mycelium_wiki::{CuratorBrain, Wiki, WikiConfig, WikiRole};

    #[derive(Default)]
    struct Recording(Mutex<Vec<AppliedRound>>);
    impl ChangeSink for Recording {
        fn round_applied(&self, round: &AppliedRound) {
            self.0.lock().unwrap().push(round.clone());
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let store = Arc::new(FsStore::open(tmp.path(), "notify").unwrap());
    let sink = Arc::new(Recording::default());

    // One node, pinned curator — this test is about the drain→sink wiring, not the election.
    let agent = loop {
        let port = mycelium::test_util::alloc_port();
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let wiki = Wiki::with_brain(
        Arc::clone(&agent),
        WikiConfig::new("notify").role(WikiRole::Curator),
        Arc::clone(&store),
        CuratorBrain::default().with_change_sink(sink.clone() as Arc<dyn ChangeSink>),
    )
    .await;

    let section = mint_section_id("notify", "minutes/2026-08", 1, 1);
    wiki.propose("minutes/2026-08", section, "Opening", "The meeting opened.", attrs(&[("author", "ivy")]));

    // Structural poll, no fixed sleeps: the drain tick applies the proposal, then notifies.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if !sink.0.lock().unwrap().is_empty() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "sink was never notified");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    {
        let rounds = sink.0.lock().unwrap();
        assert_eq!(rounds[0].group, "notify");
        assert_eq!(rounds[0].pages, vec!["minutes/2026-08".to_string()]);
        assert_eq!(rounds[0].proposals, 1);
    } // guard dropped before the shutdown awaits (clippy: no MutexGuard across await)
    // The store (the truth) holds the applied text the sink was told about.
    let page = store.read("minutes/2026-08").unwrap().unwrap();
    assert!(page.sections.iter().any(|s| s.body.contains("meeting opened")));
    wiki.shutdown().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
