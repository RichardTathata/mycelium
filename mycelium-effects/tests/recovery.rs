//! **Effect recovery** (item 1 PR 6): a worker that committed an item's effect and died before
//! acknowledging it is re-delivered, and the second apply is `Replayed` — one effect, one row.
//!
//! The crash is modelled the way the tuple space's own lifecycle probe models it: the primary is
//! shut down with the item in flight and restarted over the same WAL, which re-queues every
//! unacknowledged item with its id intact. No thirty-second lease wait; the same path a real
//! restart takes.
#![cfg(feature = "tuple-space")]

use bytes::Bytes;
use mycelium::{DedupOutcome, GossipAgent, GossipConfig, NodeId};
use mycelium_effects::sqlite::Handler;
use mycelium_effects::tuple_consumer::TupleConsumer;
use mycelium_effects::{Effect, EffectDestination, SqliteDestination, AttemptId};
use mycelium_tuple_space::{TupleConfig, TupleError, TupleRole, TupleSpace};
use std::sync::Arc;
use std::time::Duration;

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

fn business_rows(path: &std::path::Path) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
        .unwrap_or(0)
}

async fn primary(port: u16, wal: &std::path::Path) -> (Arc<GossipAgent>, Arc<TupleSpace>) {
    let agent = Arc::new(GossipAgent::new(
        NodeId::new("127.0.0.1", port).expect("node id"),
        GossipConfig { bind_port: port, health_check_max_jitter_ms: 50, ..Default::default() },
    ));
    agent.start().await.expect("agent start");
    let space = TupleSpace::new(
        Arc::clone(&agent),
        TupleConfig {
            namespace: Arc::from("orders"),
            role: TupleRole::Primary,
            persist: true,
            wal_path: wal.to_path_buf(),
            cap_refresh: Duration::from_millis(300),
            ..Default::default()
        },
    )
    .await
    .expect("tuple space");
    (agent, space)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_that_died_after_the_effect_is_re_delivered_and_replays_it() {
    let dir = std::env::temp_dir().join(format!("effects-recovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wal = dir.join("orders.wal");
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());

    // ── Generation 1: the worker takes the item, commits its effect, and dies before acking. ────
    let taken_id;
    {
        let (agent, space) = primary(mycelium::test_util::alloc_port(), &wal).await;
        let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");

        space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");
        let (id, payload) = space.take("work", Duration::from_secs(5)).await.expect("take");
        taken_id = id;

        // The effect commits — the same path `consume_one` takes — and then the worker dies with
        // the item still in flight: no `ack`.
        let op = consumer.operation_id("work", id);
        let first = destination
            .apply(&Effect::new(op.clone(), AttemptId::fresh(&op), payload.to_vec()))
            .expect("first apply");
        assert_eq!(first.dedup, DedupOutcome::Fresh);
        assert_eq!(business_rows(&db), 1);

        space.shutdown().await;
        agent.shutdown().await;
    }

    // ── Generation 2: the primary restarts over the same WAL; the item is re-delivered. ────────
    {
        let (agent, space) = primary(mycelium::test_util::alloc_port(), &wal).await;
        let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");

        let consumed = consumer
            .consume_one("work", Duration::from_secs(5), Duration::from_secs(5))
            .await
            .expect("the re-delivered item is consumed");
        assert_eq!(consumed.id, taken_id, "the same item, with its id intact");
        assert_eq!(
            consumed.receipt.dedup,
            DedupOutcome::Replayed,
            "the destination remembered the first worker's effect"
        );
        assert_eq!(business_rows(&db), 1, "one effect, two deliveries");

        // And now it is acknowledged: the stage is empty.
        match space.take("work", Duration::from_millis(300)).await {
            Err(TupleError::Timeout) => {}
            other => panic!("the stage should be drained, got {other:?}"),
        }

        space.shutdown().await;
        agent.shutdown().await;
    }
}

/// **The ordering rule, pinned where it matters.** The replay test above would also pass for a
/// consumer that acknowledged *before* committing — the difference shows only when the effect is
/// refused. Then the item must still be in flight, not lost: after a restart over the WAL it comes
/// back, because nothing acknowledged it. A consumer that acked first would have made the item
/// vanish along with its failed effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_effect_leaves_the_item_in_flight_for_re_delivery() {
    let dir = std::env::temp_dir().join(format!("effects-refused-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wal = dir.join("orders.wal");
    let db = dir.join("ledger.sqlite");
    // A destination that refuses everything: the business change never happens.
    let refusing: Handler = Arc::new(|_tx: &rusqlite::Transaction<'_>, _e: &Effect| {
        Err("the ledger is closed".to_string())
    });
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", refusing).unwrap());

    let item_id;
    {
        let (agent, space) = primary(mycelium::test_util::alloc_port(), &wal).await;
        let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");
        item_id = space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");

        let err = consumer
            .consume_one("work", Duration::from_secs(5), Duration::from_secs(5))
            .await
            .expect_err("a refused effect is not consumed");
        assert!(
            matches!(err, mycelium_effects::tuple_consumer::ConsumeError::Effect(mycelium_effects::EffectRefusal::Failed(_))),
            "the refusal is surfaced, not swallowed: {err}"
        );
        assert_eq!(destination.committed_count().unwrap(), 0, "nothing was committed");

        space.shutdown().await;
        agent.shutdown().await;
    }

    // The item was never acknowledged, so the restart re-queues it with its id intact.
    {
        let (agent, space) = primary(mycelium::test_util::alloc_port(), &wal).await;
        let (id, payload) = space
            .take("work", Duration::from_secs(5))
            .await
            .expect("the unacknowledged item is delivered again");
        assert_eq!(id, item_id, "the same item — it was never lost");
        assert_eq!(payload.as_ref(), b"credit 5");
        space.shutdown().await;
        agent.shutdown().await;
    }
}

/// The ordinary path, for contrast: a fresh item is `Fresh`, acknowledged, and gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_item_is_committed_then_acknowledged() {
    let dir = std::env::temp_dir().join(format!("effects-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("ledger.sqlite");
    let destination = Arc::new(SqliteDestination::open(&db, "ledger", ledger_handler()).unwrap());

    let (agent, space) = primary(mycelium::test_util::alloc_port(), &dir.join("orders.wal")).await;
    let consumer = TupleConsumer::new(Arc::clone(&space), Arc::clone(&destination), "orders");

    space.put("work", Bytes::from_static(b"credit 5")).await.expect("put");
    let consumed = consumer
        .consume_one("work", Duration::from_secs(5), Duration::from_secs(5))
        .await
        .expect("consumed");
    assert_eq!(consumed.receipt.dedup, DedupOutcome::Fresh);
    assert_eq!(consumed.receipt.destination, "ledger");
    assert_eq!(business_rows(&db), 1);
    assert!(matches!(space.take("work", Duration::from_millis(300)).await, Err(TupleError::Timeout)));

    space.shutdown().await;
    agent.shutdown().await;
}
