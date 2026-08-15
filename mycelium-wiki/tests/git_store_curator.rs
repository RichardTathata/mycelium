//! The Phase-1 hinge, end to end: a **curator over a `GitStore`** — proposals drain into real,
//! scoped git commits. This is the council-wiki substrate shape from
//! `docs/design/transparency-council-substrate.md`: the elected single writer per council group
//! applies agent proposals and every apply is a commit to `councils/<slug>`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_wiki::{mint_section_id, GitStore, GitStoreConfig, Section, Wiki, WikiConfig, WikiRole, WikiStore};

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

/// The Phase-2 gate (`transparency-council-substrate.md` §6.2): **two curators, two councils, one
/// repo** — concurrent applies land in their own subtrees, every commit is scoped to exactly one
/// council, messages carry the right per-council prefix, and the shared branch ref serialises the
/// two store instances without losing either council's write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_council_curators_share_one_repo_without_cross_scope_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let councils = ["norfolk", "evesham"];

    let mut wikis = Vec::new();
    let mut agents = Vec::new();
    let mut stores = Vec::new();
    for slug in councils {
        // Independent write domains need no shared mesh: each council's group has its own curator.
        let agent = loop {
            let port = mycelium::test_util::alloc_port();
            let cfg = GossipConfig { bind_port: port, ..Default::default() };
            let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
            if a.start().await.is_ok() {
                break a;
            }
        };
        let store = Arc::new(GitStore::open(GitStoreConfig::for_group(tmp.path(), slug)).unwrap());
        let wiki = Wiki::new(
            Arc::clone(&agent),
            WikiConfig::new(slug).role(WikiRole::Curator),
            Arc::clone(&store),
        )
        .await;
        agents.push(agent);
        stores.push(store);
        wikis.push(wiki);
    }

    // Concurrent proposals into both councils.
    for (i, slug) in councils.iter().enumerate() {
        let section = mint_section_id(slug, "minutes/2026-08-15", 1, 1);
        wikis[i].propose(
            "minutes/2026-08-15",
            section,
            "Opening",
            format!("The {slug} council opened the meeting."),
            Default::default(),
        );
    }

    // Both curators drain into the SAME repo — poll until both pages are committed truth.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let done = stores.iter().all(|s| s.read("minutes/2026-08-15").unwrap().is_some());
        if done {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "both curators must apply into the shared repo");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Every commit touches exactly ONE council's subtree (the scoped-commit property under
    // cross-instance concurrency), and its message carries that council's prefix.
    let log = git(tmp.path(), &["log", "--name-only", "--format=@%s"]);
    for entry in log.split('@').filter(|e| !e.trim().is_empty()) {
        let mut lines = entry.lines();
        let subject = lines.next().unwrap();
        let files: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();
        assert!(!files.is_empty(), "every commit names its files: {entry:?}");
        let scopes: std::collections::BTreeSet<&str> = files
            .iter()
            .map(|f| {
                f.strip_prefix("councils/").and_then(|r| r.split('/').next()).unwrap_or_else(|| {
                    panic!("commit touched a path outside councils/: {f:?} ({subject:?})")
                })
            })
            .collect();
        assert_eq!(scopes.len(), 1, "a commit crossed council subtrees: {subject:?} → {files:?}");
        let slug = scopes.iter().next().unwrap();
        assert!(
            subject.starts_with(&format!("wiki({slug}): ")),
            "message prefix names the council whose subtree it touched: {subject:?} → {files:?}"
        );
    }

    // Both documents are committed truth in one tree, each in its own subtree.
    for slug in councils {
        let doc = git(tmp.path(), &["show", &format!("HEAD:councils/{slug}/minutes/2026-08-15.md")]);
        assert!(doc.contains(&format!("The {slug} council")), "each council's text is in its own subtree");
    }

    for w in &wikis {
        w.shutdown().await;
    }
    for a in &agents {
        a.shutdown_with_timeout(Duration::from_secs(5)).await;
    }
}

/// The Phase-3 gate (`transparency-council-substrate.md` §6.3), end to end: a curator over a
/// **gated** GitStore. A proposal the validator refuses is **dropped with the findings counted**
/// (never retried — the queue must not wedge), and the curator keeps applying clean proposals
/// afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gate_refused_proposals_are_dropped_and_the_curator_keeps_working() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let script = tmp.path().join("stub-validate.sh");
    std::fs::write(&script, "#!/bin/sh\nfor f in \"$@\"; do\n  if grep -q FORBIDDEN \"$f\"; then echo \"GATE001/forbidden-term: $f\"; exit 1; fi\ndone\nexit 0\n").unwrap();
    let mut cfg = GitStoreConfig::for_group(&repo, "testville");
    cfg.validate_cmd = Some(vec!["/bin/sh".into(), script.to_string_lossy().into_owned()]);
    let store = Arc::new(GitStore::open(cfg).unwrap());

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

    // A proposal the gate refuses…
    let bad = mint_section_id("testville", "minutes/2026-08-15", 1, 1);
    wiki.propose("minutes/2026-08-15", bad, "Opening", "FORBIDDEN claim.", Default::default());

    // …is dropped with the refusal counted (poll the counter, not a sleep).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while wiki.gate_refusals() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the refusal was never recorded");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let refusals_at_drop = wiki.gate_refusals();
    assert_eq!(store.read("minutes/2026-08-15").unwrap(), None, "the refused content never landed");

    // A clean proposal afterwards applies normally — the queue did not wedge.
    let good = mint_section_id("testville", "minutes/2026-08-15", 2, 2);
    wiki.propose("minutes/2026-08-15", good, "Opening", "The meeting opened.", Default::default());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(p) = store.read("minutes/2026-08-15").unwrap() {
            assert!(p.sections.iter().any(|s| s.body.contains("meeting opened")));
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the clean proposal never applied");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The dropped proposal was tombstoned, not retried: the counter stays where the drop left it.
    assert_eq!(wiki.gate_refusals(), refusals_at_drop, "a dropped proposal is never retried");

    wiki.shutdown().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The Phase-4 gate, remote leg: a **worker submits a claim-check reference over the mesh**; the
/// curator fetches the staged batch from its own source, applies it, and replies with the summary.
/// The payload never rides the RPC — only the reference string does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_submits_a_batch_reference_and_the_curator_applies_it() {
    use mycelium_wiki::{CuratorBrain, FsBatchSource, IngestBatch, IngestPage, Section};

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let stage = tmp.path().join("stage"); // the claim-check store (S3 stand-in)
    std::fs::create_dir_all(&stage).unwrap();

    // Stage the batch where only the CURATOR's source can see it.
    let batch = IngestBatch {
        source: "pipeline/run-7".into(),
        pages: vec![IngestPage {
            path: "minutes/2026-08-15".into(),
            attributes: Default::default(),
            sections: vec![Section {
                id: std::sync::Arc::from("d1"),
                heading: "Retrofit approved".into(),
                body: "RESOLVED: the retrofit scheme proceeds.".into(),
                attributes: Default::default(),
            }],
        }],
    };
    std::fs::write(stage.join("run-7.json"), serde_json::to_vec(&batch).unwrap()).unwrap();

    // A meshed pair: curator on A (with the batch source), worker on B (no source, no checkout).
    let (agent_a, agent_b) = {
        let mut pair = None;
        for _ in 0..16 {
            let (pa, pb) = (mycelium::test_util::alloc_port(), mycelium::test_util::alloc_port());
            let mk = |port: u16, boot: u16| {
                let cfg = GossipConfig {
                    bind_port: port,
                    bootstrap_peers: vec![NodeId::new("127.0.0.1", boot).unwrap()],
                    ..Default::default()
                };
                Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg))
            };
            let a = mk(pa, pb);
            if a.start().await.is_err() {
                continue;
            }
            let b = mk(pb, pa);
            if b.start().await.is_ok() {
                pair = Some((a, b));
                break;
            }
            a.shutdown_with_timeout(Duration::from_secs(5)).await;
        }
        pair.expect("could not bind an agent pair")
    };

    let store_a = Arc::new(GitStore::open(GitStoreConfig::for_group(&repo, "testville")).unwrap());
    let curator = Wiki::with_brain(
        Arc::clone(&agent_a),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store_a),
        CuratorBrain::default().with_batch_source(Arc::new(FsBatchSource::new(&stage))),
    )
    .await;
    // The worker holds a read handle to the same repo (E4: git-native readers), but no batch source.
    let store_b = Arc::new(GitStore::open(GitStoreConfig::for_group(&repo, "testville")).unwrap());
    let worker = Wiki::new(
        Arc::clone(&agent_b),
        WikiConfig::new("testville").role(WikiRole::Reader),
        Arc::clone(&store_b),
    )
    .await;

    // Submit the REFERENCE (retry until the curator's capability has gossiped to the worker).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let summary = loop {
        match worker.submit_batch("run-7.json").await {
            Ok(s) => break s,
            Err(e) => {
                assert!(tokio::time::Instant::now() < deadline, "submission never succeeded: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    };
    assert_eq!((summary.applied, summary.refused), (1, 0), "the curator applied the staged batch");

    // The result is committed truth, readable by the worker's own store handle, with provenance.
    let page = store_b.read("minutes/2026-08-15").unwrap().expect("the page is committed truth");
    assert!(page.sections[0].body.contains("retrofit scheme proceeds"));
    let msg = git(&repo, &["log", "-1", "--format=%s"]);
    assert!(msg.starts_with("wiki(testville): "), "scoped provenance survives the remote path: {msg}");

    curator.shutdown().await;
    worker.shutdown().await;
    agent_a.shutdown_with_timeout(Duration::from_secs(5)).await;
    agent_b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// The P6.3 gate: **failover across nodes via the shared remote**. Curator A applies + publishes
/// and dies; curator B — a *fresh clone* on another "node" — promotes, refreshes (pull-on-promote)
/// and continues the same corpus: the companion's litmus ("failover transfers nothing… resumes
/// against the same store") restored for the node-local GitStore. The **un-pushed-tail residual**
/// is asserted too, not hidden: a commit A never published is absent after failover and re-lands
/// by re-applying (at-least-once × idempotent apply).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn curator_failover_resumes_on_a_fresh_clone_via_the_shared_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = tmp.path().join("origin.git");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let remote = origin.to_string_lossy().into_owned();

    // ── Node A: curator, applies a proposal; the drain round publishes to the shared remote. ──
    let agent_a = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let store_a = Arc::new(
        GitStore::open(GitStoreConfig::for_group(tmp.path().join("clone-a"), "testville").with_remote(&remote))
            .unwrap(),
    );
    let wiki_a = Wiki::new(
        Arc::clone(&agent_a),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store_a),
    )
    .await;
    let s1 = mint_section_id("testville", "minutes/2026-08-15", 1, 1);
    wiki_a.propose("minutes/2026-08-15", s1, "Opening", "The meeting opened.", Default::default());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let published = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", "refs/heads/main"])
            .current_dir(&origin)
            .output()
            .unwrap()
            .status
            .success();
        if published && store_a.read("minutes/2026-08-15").unwrap().is_some() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "A never published the round");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // A also commits a LOCAL tail it never publishes (a direct store write outside a drain round),
    // then dies — the residual the design names.
    wiki_a.shutdown().await;
    store_a
        .write_page("unpublished-tail", &[Section {
            id: Arc::from("t1"),
            heading: "Tail".into(),
            body: "Never pushed.".into(),
            attributes: Default::default(),
        }], &Default::default())
        .unwrap();
    agent_a.shutdown_with_timeout(Duration::from_secs(5)).await;

    // ── Node B: a FRESH clone promotes; pull-on-promote adopts the remote head. ──
    let agent_b = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let store_b = Arc::new(
        GitStore::open(GitStoreConfig::for_group(tmp.path().join("clone-b"), "testville").with_remote(&remote))
            .unwrap(),
    );
    let wiki_b = Wiki::new(
        Arc::clone(&agent_b),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store_b),
    )
    .await;
    assert!(wiki_b.is_curator(), "B promoted (refresh succeeded)");
    let page = store_b.read("minutes/2026-08-15").unwrap().expect("THE LITMUS: B resumes A's corpus");
    assert!(page.sections[0].body.contains("meeting opened"));
    assert_eq!(store_b.read("unpublished-tail").unwrap(), None,
        "the un-pushed tail is absent after failover — the named ≤1-round residual, not hidden");

    // The tail re-lands by re-applying (at-least-once × idempotent), and B's round publishes.
    let s2 = mint_section_id("testville", "unpublished-tail", 2, 2);
    wiki_b.propose("unpublished-tail", s2, "Tail", "Never pushed.", Default::default());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if store_b.read("unpublished-tail").unwrap().is_some() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the tail never re-landed on B");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // No divergence: B's head equals the remote head, and the tripwire is quiet.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let local = git(&tmp.path().join("clone-b"), &["rev-parse", "refs/heads/main"]);
        let remote_head = git(&origin, &["rev-parse", "refs/heads/main"]);
        if local.trim() == remote_head.trim() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "B never converged with the remote");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(store_b.push_divergences(), 0, "the tripwire is quiet on an honest remote");

    wiki_b.shutdown().await;
    agent_b.shutdown_with_timeout(Duration::from_secs(5)).await;
}

/// P6.3's refusal path: a curator that cannot refresh (unreachable remote) must **never serve** —
/// a knowingly-stale curator is the data-loss path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_curator_that_cannot_refresh_never_serves() {
    let tmp = tempfile::tempdir().unwrap();
    let agent = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };
    let store = Arc::new(
        GitStore::open(
            GitStoreConfig::for_group(tmp.path().join("clone"), "testville")
                .with_remote(tmp.path().join("no-such-origin.git").to_string_lossy()),
        )
        .unwrap(),
    );
    let wiki = Wiki::new(
        Arc::clone(&agent),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(!wiki.is_curator(), "an unrefreshable curator refuses to serve");

    wiki.shutdown().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
