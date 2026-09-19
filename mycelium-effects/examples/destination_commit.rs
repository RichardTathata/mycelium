//! **The destination commit** — the fourth rung, and the one the substrate never provides.
//!
//! ```text
//! cargo run -p mycelium-effects --example destination_commit
//! ```
//!
//! # The claim this exists to make checkable
//!
//! *At-least-once delivery plus an idempotent merge is exactly-once **effect** — but only the
//! resource can say so.* Chapter 18's first three rungs are the substrate's: applied here, on this
//! node's disk, persisted by named peers. None of them says a business change happened anywhere.
//! This one does, because the dedup row and the business change **commit in the same transaction**.
//!
//! The scenario: a food-rescue co-op recording collected surplus against a depot's ledger. The same
//! collection may be reported twice — a retry after a timeout, a restarted worker, a duplicate
//! delivery — and the depot's total must not move twice.
//!
//! # What it demonstrates by doing, not by asserting
//!
//! | Step | What you watch |
//! |---|---|
//! | 1 | a first apply is `Fresh`, and the ledger moves |
//! | 2 | the **same** operation replayed is `Replayed`, and the ledger does **not** move again |
//! | 3 | the same identity with **different content** is a `Conflict` — refused, first version intact |
//! | 4 | a handler that fails leaves **no dedup row**, so a later retry is `Fresh`, not a false `Replayed` |
//! | 5 | a slow destination answers `DeliveryUnknown`, and the retry that follows resolves it |
//!
//! Step 4 is the one worth reading twice. A destination that wrote its dedup row *outside* the
//! business transaction would record the attempt, fail the change, and then answer a later retry
//! with `Replayed` — reporting an effect that never happened. The whole design is that those two
//! writes cannot come apart.
//!
//! # What it does not demonstrate
//!
//! **Not a crash mid-transaction.** Steps 4 and 5 stage a failing handler and a slow one inside one
//! cooperating process; neither kills it between the business change and the dedup row. That the two
//! survive a power cut together is SQLite's transactional guarantee, relied on here and not tested
//! here.
//!
//! **Not concurrent appliers racing for the same identity.** The destination serialises through its
//! own transaction, and this example applies serially; a real race would need two processes.
//!
//! **Not the tuple-space consumer.** The half that commits the effect *before* acknowledging a
//! pipeline item is behind the `tuple-space` feature and is a different claim: it is about worker
//! death between two steps, not about the destination's own rung.

use mycelium_effects::sqlite::{Handler, SqliteDestination};
use mycelium_effects::{
    apply_within, AttemptId, DedupOutcome, Effect, EffectDestination, EffectRefusal, OperationId,
};
use std::sync::Arc;
use std::time::Duration;

/// The depot's running total of collected surplus, in kilograms.
fn depot_total(db: &SqliteDestination) -> i64 {
    let conn = rusqlite::Connection::open(db.path()).expect("open for reading");
    conn.query_row("SELECT COALESCE(SUM(kg), 0) FROM collections", [], |r| r.get(0))
        .unwrap_or(0)
}

/// The business change: record one collection against the depot's ledger.
fn recording_handler() -> Handler {
    Arc::new(|tx: &rusqlite::Transaction<'_>, effect: &Effect| {
        tx.execute(
            "CREATE TABLE IF NOT EXISTS collections (operation TEXT PRIMARY KEY, kg INTEGER NOT NULL)",
            [],
        )
        .map_err(|e| e.to_string())?;
        let kg: i64 = String::from_utf8_lossy(&effect.payload).parse().map_err(|_| {
            "the payload is not a kilogram figure".to_string()
        })?;
        tx.execute(
            "INSERT OR REPLACE INTO collections (operation, kg) VALUES (?1, ?2)",
            rusqlite::params![effect.operation_id.as_str(), kg],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// A handler that always refuses — the depot's scales are offline.
fn failing_handler() -> Handler {
    Arc::new(|_tx: &rusqlite::Transaction<'_>, _e: &Effect| {
        Err("the depot's scales did not respond".to_string())
    })
}

/// A handler that takes longer than the caller is willing to wait.
fn slow_handler() -> Handler {
    Arc::new(|tx: &rusqlite::Transaction<'_>, effect: &Effect| {
        std::thread::sleep(Duration::from_millis(400));
        tx.execute(
            "CREATE TABLE IF NOT EXISTS collections (operation TEXT PRIMARY KEY, kg INTEGER NOT NULL)",
            [],
        )
        .map_err(|e| e.to_string())?;
        let kg: i64 = String::from_utf8_lossy(&effect.payload).parse().unwrap_or(0);
        tx.execute(
            "INSERT OR REPLACE INTO collections (operation, kg) VALUES (?1, ?2)",
            rusqlite::params![effect.operation_id.as_str(), kg],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn effect(op: &str, attempt: u32, kg: &str) -> Effect {
    let operation = OperationId::new(op);
    let attempt = AttemptId::of(&operation, attempt);
    Effect::new(operation, attempt, kg.as_bytes().to_vec())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("mycelium-effects-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let db = dir.join("depot.sqlite");

    println!("The destination commit — the rung the substrate never provides");
    println!("=============================================================\n");

    // ── 1. A first apply is Fresh, and the ledger moves ──────────────────────────────────────
    let depot = SqliteDestination::open(&db, "depot/southwark", recording_handler())?;
    let receipt = depot.apply(&effect("collection-4471", 1, "180"))?;
    println!("1. first apply        → {:?}, depot holds {} kg", receipt.dedup, depot_total(&depot));
    assert_eq!(receipt.dedup, DedupOutcome::Fresh);
    assert_eq!(depot_total(&depot), 180);

    // ── 2. The same operation replayed is Replayed, and the ledger does NOT move ──────────────
    // A different attempt id on purpose: this is the *same* logical collection, reported again.
    let receipt = depot.apply(&effect("collection-4471", 2, "180"))?;
    println!("2. same op, retried   → {:?}, depot holds {} kg  (unchanged)", receipt.dedup, depot_total(&depot));
    assert_eq!(receipt.dedup, DedupOutcome::Replayed);
    assert_eq!(depot_total(&depot), 180, "a replay must not move the total");

    // ── 3. Same identity, different content → Conflict, first version intact ──────────────────
    match depot.apply(&effect("collection-4471", 3, "260")) {
        Err(EffectRefusal::Conflict { committed_hash, presented_hash, .. }) => {
            println!(
                "3. same op, new figure → Conflict (committed {committed_hash:#x} ≠ presented {presented_hash:#x}); depot still holds {} kg",
                depot_total(&depot)
            );
        }
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(depot_total(&depot), 180, "a refused conflict changes nothing");

    // ── 4. A failing handler leaves NO dedup row, so a later retry is Fresh ───────────────────
    // The load-bearing step. A destination that recorded the attempt outside the business
    // transaction would answer this retry `Replayed` — reporting an effect that never happened.
    let broken = SqliteDestination::open(&db, "depot/southwark", failing_handler())?;
    match broken.apply(&effect("collection-5502", 1, "95")) {
        Err(EffectRefusal::Failed(why)) => println!("4. handler fails      → Failed({why})"),
        other => panic!("expected a failure, got {other:?}"),
    }
    let repaired = SqliteDestination::open(&db, "depot/southwark", recording_handler())?;
    let receipt = repaired.apply(&effect("collection-5502", 2, "95"))?;
    println!(
        "   retry after repair  → {:?}  ← NOT a false Replayed; depot holds {} kg",
        receipt.dedup,
        depot_total(&repaired)
    );
    assert_eq!(receipt.dedup, DedupOutcome::Fresh, "a rolled-back attempt left no dedup row");
    assert_eq!(depot_total(&repaired), 275);

    // ── 5. A slow destination is DeliveryUnknown, and the retry resolves it ───────────────────
    let slow = Arc::new(SqliteDestination::open(&db, "depot/southwark", slow_handler())?);
    let outcome = apply_within(
        Arc::clone(&slow),
        effect("collection-6613", 1, "40"),
        Duration::from_millis(50),
    )
    .await;
    match outcome {
        Err(EffectRefusal::DeliveryUnknown) => {
            println!("5. deadline elapsed   → DeliveryUnknown  (the apply was NOT cancelled)")
        }
        other => panic!("expected unknown, got {other:?}"),
    }

    // The apply was never cancelled, so let it land, then retry the same identity to find out.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let resolver = SqliteDestination::open(&db, "depot/southwark", recording_handler())?;
    let receipt = resolver.apply(&effect("collection-6613", 2, "40"))?;
    println!(
        "   retry resolves it   → {:?}; depot holds {} kg",
        receipt.dedup,
        depot_total(&resolver)
    );
    println!(
        "\n   The unknown became a fact by asking again under the same identity — which is the\n   \
         only way an unknown ever resolves. The substrate could not have told you this."
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
