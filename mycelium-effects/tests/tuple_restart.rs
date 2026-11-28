//! **Effect ids across a tuple space's restart** (ER3, R4-01): a `TupleConsumer` commits each item
//! under `tuple/{namespace}/{stage}/{id}`, so the tuple id is the effect's identity. A space with no
//! id history — a transient one, or a WAL that was deleted — used to start its ids at 0, and a new
//! item after a restart reused an id the destination had already committed: the same payload came
//! back `Replayed` with no write, a changed payload `Conflict` and stuck in flight. A fresh id space
//! now starts from the wall clock, so new work is `Fresh`; a WAL with history keeps its ids, so a
//! real redelivery is still `Replayed`.
//!
//! The restart is modelled in-process, the way `recovery.rs` models it: the space is shut down and a
//! new one built in the same namespace, over the same destination.
#![cfg(feature = "tuple-space")]

use bytes::Bytes;
use mycelium::{DedupOutcome, GossipAgent, GossipConfig, NodeId};
use mycelium_effects::sqlite::Handler;
use mycelium_effects::tuple_consumer::{ConsumeError, TupleConsumer};
use mycelium_effects::{AttemptId, Effect, EffectDestination, SqliteDestination};
use mycelium_tuple_space::{TupleConfig, TupleRole, TupleSpace};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(5);

fn ledger_handler() -> Handler {
    Arc::new(|tx: &rusqlite::Transaction<'_>, e: &Effect| {
        tx.execute_batch(
            "CREATE TABLE IF NOT EXISTS ledger_entries (operation_id TEXT NOT NULL, payload BLOB NOT NULL);",
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO ledger_entries (operation_id, payload) VALUES (?1, ?2)",
            rusqlite::params![e.operation_id.as_str(), e.payload],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn business_rows(path: &Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
        .unwrap_or(0)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("effects-tuple-restart-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One life of the primary: an agent and a space in namespace `orders`.
async fn primary(persist: bool, wal: &Path) -> (Arc<GossipAgent>, Arc<TupleSpace>) {
    let port = mycelium::test_util::alloc_port();
    let agent = Arc::new(GossipAgent::new(NodeId::new("127.0.0.1", port).expect("node id"), {
        let mut cfg = GossipConfig::default();
        cfg.bind_port = port;
        cfg.health_check_max_jitter_ms = 50;
        cfg
    }));
    agent.start().await.expect("agent start");
    let space = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig {
            namespace: Arc::from("orders"),
            role: TupleRole::Primary,
            persist,
            wal_path: wal.to_path_buf(),
            cap_refresh: Duration::from_millis(300),
            ..Default::default()
        },
    )
    .await
    .expect("tuple space");
    (agent, space)
}

async fn stop(agent: Arc<GossipAgent>, space: Arc<TupleSpace>) {
    space.shutdown().await;
    agent.shutdown().await;
}

/// A life that puts `payload`, consumes it to `Fresh`, and stops with nothing in flight.
async fn first_life(persist: bool, wal: &Path, destination: &Arc<SqliteDestination>, payload: &'static [u8]) {
    let (agent, space) = primary(persist, wal).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(destination), "orders");
    space.put("work", Bytes::from_static(payload)).await.expect("put");
    let consumed = consumer.consume_one("work", WAIT, WAIT).await.expect("first life consumes");
    assert_eq!(consumed.receipt.dedup, DedupOutcome::Fresh);
    stop(agent, space).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_work_after_a_transient_restart_is_fresh_not_replayed() {
    let dir = scratch("same-payload");
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());
    first_life(false, &dir.join("unused.wal"), &destination, b"credit 5").await;
    assert_eq!(business_rows(&db), 1);

    // The second life: a new job that happens to carry the same bytes.
    let (agent, space) = primary(false, &dir.join("unused.wal")).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
    space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");
    let consumed = consumer.consume_one("work", WAIT, WAIT).await.expect("consumed");
    assert_eq!(consumed.receipt.dedup, DedupOutcome::Fresh, "new work, not a replay of item {}", consumed.id);
    assert_eq!(business_rows(&db), 2, "two jobs, two business rows");
    stop(agent, space).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_work_after_a_transient_restart_is_not_a_conflict() {
    let dir = scratch("changed-payload");
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());
    first_life(false, &dir.join("unused.wal"), &destination, b"credit 5").await;

    let (agent, space) = primary(false, &dir.join("unused.wal")).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
    space.put("work", Bytes::from_static(b"debit 3")).await.expect("put");
    match consumer.consume_one("work", WAIT, WAIT).await {
        Ok(c) => assert_eq!(c.receipt.dedup, DedupOutcome::Fresh),
        Err(ConsumeError::Effect(e)) => panic!("new work refused, the item left in flight: {e}"),
        Err(e) => panic!("unexpected: {e}"),
    }
    assert_eq!(business_rows(&db), 2);
    let depth = space.local_depth(Some("work")).unwrap();
    assert_eq!(depth.first().map_or(0, |d| d.inflight), 0, "nothing left in flight");
    stop(agent, space).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_wal_does_not_recycle_effect_ids() {
    let dir = scratch("deleted-wal");
    let wal = dir.join("orders.wal");
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());
    first_life(true, &wal, &destination, b"credit 5").await;
    std::fs::remove_file(&wal).expect("the WAL is removed between runs");

    let (agent, space) = primary(true, &wal).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
    space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");
    let consumed = consumer.consume_one("work", WAIT, WAIT).await.expect("consumed");
    assert_eq!(consumed.receipt.dedup, DedupOutcome::Fresh, "a recycled id {} answered for new work", consumed.id);
    assert_eq!(business_rows(&db), 2);
    stop(agent, space).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The control: a WAL with history keeps its ids, so an item committed and not acknowledged before
/// the restart is re-delivered under the same id and the destination answers `Replayed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retained_wal_redelivery_is_still_replayed() {
    let dir = scratch("retained-wal");
    let wal = dir.join("orders.wal");
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());
    let taken_id;
    {
        let (agent, space) = primary(true, &wal).await;
        let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
        space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");
        let (id, payload) = space.take("work", WAIT).await.expect("take");
        taken_id = id;
        let op = consumer.operation_id("work", id);
        let first = destination.apply(&Effect::new(op.clone(), AttemptId::fresh(&op), payload.to_vec())).unwrap();
        assert_eq!(first.dedup, DedupOutcome::Fresh);
        stop(agent, space).await; // no ack
    }
    let (agent, space) = primary(true, &wal).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
    let consumed = consumer.consume_one("work", WAIT, WAIT).await.expect("re-delivered");
    assert_eq!(consumed.id, taken_id, "the same item, with its id intact");
    assert_eq!(consumed.receipt.dedup, DedupOutcome::Replayed);
    assert_eq!(business_rows(&db), 1);
    stop(agent, space).await;
    let _ = std::fs::remove_dir_all(&dir);
}
