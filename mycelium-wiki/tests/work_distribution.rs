//! The Phase-5 gate (`transparency-council-substrate.md` §6.5): **work distribution assembled from
//! existing companions** — the tuple space supplies council work-leases (at-least-once: a taken
//! item whose worker dies re-queues on lease expiry), the Phase-4 ingest supplies the idempotent
//! apply, and their composition is **exactly-once effect** (`docs/design/exactly-once-effect.md`,
//! the same contract the tuple space and blackboard already use).
//!
//! The kill is deliberately at the **worst point**: the worker dies *after* submitting the batch
//! but *before* acking the lease — so the work is redelivered and **re-submitted in full**, and
//! zero duplicate leaves is earned by the ingest's idempotency, not by the death being convenient.
//! (A death before submit is the easy case; this subsumes it.)
//!
//! Single node: the property under test is lease-redelivery × idempotent apply. The mesh transport
//! leg of submission was proven in `git_store_curator.rs` (Phase 4's remote roundtrip).

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use mycelium::{GossipAgent, GossipConfig, NodeId};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};
use mycelium_wiki::{
    CuratorBrain, FsBatchSource, GitStore, GitStoreConfig, IngestBatch, IngestPage, Section, Wiki,
    WikiConfig, WikiRole, WikiStore,
};

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dead_workers_lease_redelivers_and_the_batch_lands_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let stage_dir = tmp.path().join("stage");
    std::fs::create_dir_all(&stage_dir).unwrap();

    // The staged meeting (what a pipeline worker's extraction produced).
    let batch = IngestBatch {
        source: "pipeline/run-9".into(),
        pages: vec![IngestPage {
            path: "minutes/2026-08-15".into(),
            attributes: Default::default(),
            sections: vec![Section {
                id: Arc::from("d1"),
                heading: "Retrofit approved".into(),
                body: "RESOLVED: the retrofit scheme proceeds.".into(),
                attributes: Default::default(),
            }],
        }],
    };
    std::fs::write(stage_dir.join("meeting-1.json"), serde_json::to_vec(&batch).unwrap()).unwrap();

    let agent = loop {
        let port = mycelium::test_util::alloc_port();
        let cfg = GossipConfig { bind_port: port, ..Default::default() };
        let a = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).unwrap(), cfg));
        if a.start().await.is_ok() {
            break a;
        }
    };

    // The council's curator (Phase 1–4 stack) + the work lanes (the tuple space, 1s lease).
    let store = Arc::new(GitStore::open(GitStoreConfig::for_group(&repo, "testville")).unwrap());
    let wiki = Wiki::with_brain(
        Arc::clone(&agent),
        WikiConfig::new("testville").role(WikiRole::Curator),
        Arc::clone(&store),
        CuratorBrain::default().with_batch_source(Arc::new(FsBatchSource::new(&stage_dir))),
    )
    .await;
    let lanes = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig { role: TupleRole::Primary, worker_timeout_secs: 1, ..Default::default() },
    )
    .await
    .unwrap();

    // A council work item: the claim-check reference, nothing more.
    lanes.put("extract", Bytes::from_static(b"meeting-1.json")).await.unwrap();

    // ── Worker A: takes the lease, submits the batch… and DIES before acking. ──
    let (id_a, payload) = lanes.take("extract", Duration::from_secs(5)).await.unwrap();
    let reference = String::from_utf8(payload.to_vec()).unwrap();
    let s1 = wiki.submit_batch(&reference).await.unwrap();
    assert_eq!((s1.applied, s1.refused), (1, 0));
    let _abandoned = id_a; // never acked — the worker is dead; the lease expires and the item re-queues.

    let commits_after_a = git(&repo, &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert!(store.read("minutes/2026-08-15").unwrap().is_some(), "worker A's submit landed");

    // ── Worker B: polls for the redelivered lease (the re-queue scan ticks ~30s), ──
    //    re-submits the SAME batch in full, and acks.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    let (id_b, payload_b) = loop {
        match lanes.take("extract", Duration::from_secs(2)).await {
            Ok(item) => break item,
            Err(_) => assert!(
                tokio::time::Instant::now() < deadline,
                "the dead worker's lease was never redelivered"
            ),
        }
    };
    assert_eq!(payload_b, payload, "the redelivered item is the same work");
    let s2 = wiki.submit_batch(&reference).await.unwrap();
    assert_eq!(s2.applied, 1, "the re-submit reports success (a no-op re-apply)");
    lanes.ack(id_b).await.unwrap();

    // ── Exactly-once effect: the redelivery left NOTHING behind. ──
    let commits_after_b = git(&repo, &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(commits_after_b, commits_after_a, "zero duplicate commits from the re-submit");
    let page = store.read("minutes/2026-08-15").unwrap().unwrap();
    assert_eq!(page.sections.len(), 1, "zero duplicate leaves");
    assert_eq!(page.sections[0].body, "RESOLVED: the retrofit scheme proceeds.");

    // The acked item is gone — the work is finished, not re-queued again.
    assert!(lanes.take("extract", Duration::from_secs(2)).await.is_err(), "the lane is drained");

    lanes.shutdown().await;
    wiki.shutdown().await;
    agent.shutdown_with_timeout(Duration::from_secs(5)).await;
}
