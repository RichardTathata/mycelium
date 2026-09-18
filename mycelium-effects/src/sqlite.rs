//! The SQLite reference destination.
//!
//! One table the crate owns — `effects_dedup(operation_id PRIMARY KEY, content_hash, attempt_id,
//! applied_at_unix_ms)` — and one rule: **the business change and the dedup row are written inside
//! the same `IMMEDIATE` transaction.** `IMMEDIATE` takes the write lock at `BEGIN`, so two appliers
//! racing on the same `operation_id` serialise at the database, and the second one reads the
//! first one's row rather than both applying.
//!
//! The business change is the caller's: a handler `Fn(&Transaction, &Effect) -> Result<(), String>`
//! that runs *inside* the transaction. If it errs, the transaction is dropped — SQLite rolls it back
//! — and no dedup row is left behind, which is what lets a retry start clean.
//!
//! A connection is opened per `apply`. That keeps [`SqliteDestination`] `Sync` (a `rusqlite`
//! connection is not) so it can be shared behind an `Arc` and used from a blocking thread; for a
//! reference destination the cost is noise, and it is documented here rather than hidden behind
//! a pool this crate does not need.

use crate::{Effect, EffectDestination, EffectRefusal};
use mycelium::{DedupOutcome, DestinationCommit};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The caller's business change, run inside the destination's transaction.
pub type Handler = Arc<dyn Fn(&rusqlite::Transaction<'_>, &Effect) -> Result<(), String> + Send + Sync>;

/// A transactional SQLite destination.
pub struct SqliteDestination {
    path:     PathBuf,
    identity: String,
    handler:  Handler,
}

impl SqliteDestination {
    /// Open (or create) the database at `path`, ensure the dedup table exists, and bind the
    /// business handler. `identity` is what every receipt names.
    pub fn open(
        path: impl AsRef<Path>,
        identity: impl Into<String>,
        handler: Handler,
    ) -> rusqlite::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS effects_dedup (
                 operation_id       TEXT PRIMARY KEY NOT NULL,
                 content_hash       INTEGER NOT NULL,
                 attempt_id         TEXT NOT NULL,
                 applied_at_unix_ms INTEGER NOT NULL
             );",
        )?;
        Ok(Self { path, identity: identity.into(), handler })
    }

    /// Where the database lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connect(&self) -> rusqlite::Result<Connection> {
        let conn = Connection::open(&self.path)?;
        // A second applier waits for the first's transaction rather than failing on `SQLITE_BUSY`.
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(conn)
    }

    /// How many dedup rows the destination holds — for inspection and tests.
    pub fn committed_count(&self) -> rusqlite::Result<u64> {
        let conn = self.connect()?;
        conn.query_row("SELECT COUNT(*) FROM effects_dedup", [], |r| r.get::<_, i64>(0)).map(|n| n as u64)
    }
}

fn failed(e: impl std::fmt::Display) -> EffectRefusal {
    EffectRefusal::Failed(e.to_string())
}

impl EffectDestination for SqliteDestination {
    fn identity(&self) -> &str {
        &self.identity
    }

    fn apply(&self, effect: &Effect) -> Result<DestinationCommit, EffectRefusal> {
        let mut conn = self.connect().map_err(failed)?;
        // The write lock now, not at the first write: the dedup *read* below must see any competing
        // applier's committed row, and `DEFERRED` would let two readers both see nothing.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(failed)?;

        let op = effect.operation_id.as_str();
        let existing: Option<i64> = tx
            .query_row(
                "SELECT content_hash FROM effects_dedup WHERE operation_id = ?1",
                params![op],
                |r| r.get(0),
            )
            .optional()
            .map_err(failed)?;

        match existing {
            // Already committed with this content: a replay. Nothing is applied again.
            Some(h) if h as u64 == effect.content_hash => {
                tx.commit().map_err(failed)?;
                Ok(DestinationCommit::new(&self.identity, DedupOutcome::Replayed))
            }
            // Already committed with other content: refused, and the first version stands.
            Some(h) => {
                drop(tx); // rollback
                Err(EffectRefusal::Conflict {
                    operation_id: effect.operation_id.clone(),
                    committed_hash: h as u64,
                    presented_hash: effect.content_hash,
                })
            }
            // Fresh: the business change, then the dedup row, one commit.
            None => {
                if let Err(e) = (self.handler)(&tx, effect) {
                    drop(tx); // rollback: whatever the handler wrote goes with it
                    return Err(EffectRefusal::Failed(e));
                }
                let applied_at_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                tx.execute(
                    "INSERT INTO effects_dedup (operation_id, content_hash, attempt_id, applied_at_unix_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![op, effect.content_hash as i64, effect.attempt_id.as_str(), applied_at_unix_ms],
                )
                .map_err(failed)?;
                tx.commit().map_err(failed)?;
                Ok(DestinationCommit::new(&self.identity, DedupOutcome::Fresh))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply_within;
    use mycelium::{AttemptId, OperationId};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mycelium-effects-{name}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("effects.sqlite")
    }

    /// The business change: one row per applied effect, in a table the handler creates on demand.
    fn ledger_handler() -> Handler {
        Arc::new(|tx: &rusqlite::Transaction<'_>, e: &Effect| {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS ledger_entries (operation_id TEXT NOT NULL, payload BLOB NOT NULL);",
            )
            .map_err(|e| e.to_string())?;
            if e.payload == b"boom" {
                return Err("the business change refused this payload".into());
            }
            tx.execute(
                "INSERT INTO ledger_entries (operation_id, payload) VALUES (?1, ?2)",
                params![e.operation_id.as_str(), e.payload],
            )
            .map_err(|e| e.to_string())?;
            // A handler that wrote its row and *then* failed: the row must go with the transaction.
            if e.payload == b"write-then-fail" {
                return Err("failed after writing".into());
            }
            Ok(())
        })
    }

    fn dest(name: &str) -> SqliteDestination {
        SqliteDestination::open(temp(name), format!("ledger-{name}"), ledger_handler()).unwrap()
    }

    fn effect(op: &str, attempt: u32, payload: &[u8]) -> Effect {
        let op = OperationId::new(op);
        Effect::new(op.clone(), AttemptId::of(&op, attempt), payload.to_vec())
    }

    fn business_rows(d: &SqliteDestination) -> i64 {
        d.connect()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM ledger_entries", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// **The contract's first line.** The same operation retried — another attempt, another
    /// worker, later — is `Replayed`, and the business change happened exactly once.
    #[test]
    fn a_retry_with_the_same_content_is_replayed_and_applies_nothing_twice() {
        let d = dest("replay");
        let first = d.apply(&effect("op-1", 1, b"credit 5")).unwrap();
        assert_eq!(first.dedup, DedupOutcome::Fresh);
        assert_eq!(first.destination, "ledger-replay", "the receipt names the destination");

        let again = d.apply(&effect("op-1", 2, b"credit 5")).unwrap();
        assert_eq!(again.dedup, DedupOutcome::Replayed);
        assert_eq!(business_rows(&d), 1, "one effect, however many attempts");
        assert_eq!(d.committed_count().unwrap(), 1);
    }

    /// **Same identity, different content is a conflict**, and the first version stands.
    #[test]
    fn a_retry_with_different_content_is_a_conflict_and_applies_nothing() {
        let d = dest("conflict");
        d.apply(&effect("op-1", 1, b"credit 5")).unwrap();
        let err = d.apply(&effect("op-1", 2, b"credit 50")).unwrap_err();
        match err {
            EffectRefusal::Conflict { committed_hash, presented_hash, .. } => {
                assert_eq!(committed_hash, mycelium::content_hash("op-1", b"credit 5", false));
                assert_eq!(presented_hash, mycelium::content_hash("op-1", b"credit 50", false));
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        assert_eq!(business_rows(&d), 1, "the second version was not applied");
    }

    /// **A failed business change leaves no dedup row**, so a later retry is `Fresh` — not a false
    /// `Replayed` that would make the effect vanish.
    #[test]
    fn a_failed_business_change_commits_no_dedup_row() {
        let d = dest("failed");
        let err = d.apply(&effect("op-1", 1, b"boom")).unwrap_err();
        assert!(matches!(err, EffectRefusal::Failed(_)));
        assert_eq!(d.committed_count().unwrap(), 0, "no dedup row for a change that did not happen");

        let retry = d.apply(&effect("op-1", 2, b"credit 5")).unwrap();
        assert_eq!(retry.dedup, DedupOutcome::Fresh, "the retry starts clean");
        assert_eq!(business_rows(&d), 1);
    }

    /// **The two commit together or not at all.** A handler that wrote its row and then failed
    /// leaves neither the row nor the dedup entry — the transaction is the unit.
    #[test]
    fn the_business_change_and_the_dedup_row_commit_together() {
        let d = dest("atomic");
        let err = d.apply(&effect("op-1", 1, b"write-then-fail")).unwrap_err();
        assert!(matches!(err, EffectRefusal::Failed(_)));
        assert_eq!(business_rows(&d), 0, "the written row was rolled back with the transaction");
        assert_eq!(d.committed_count().unwrap(), 0);
    }

    /// The dedup row is durable: a reopened destination still knows what it committed.
    #[test]
    fn a_reopened_destination_remembers_what_it_committed() {
        let path = temp("reopen");
        {
            let d = SqliteDestination::open(&path, "ledger", ledger_handler()).unwrap();
            assert_eq!(d.apply(&effect("op-1", 1, b"credit 5")).unwrap().dedup, DedupOutcome::Fresh);
        }
        let d = SqliteDestination::open(&path, "ledger", ledger_handler()).unwrap();
        assert_eq!(d.apply(&effect("op-1", 2, b"credit 5")).unwrap().dedup, DedupOutcome::Replayed);
        assert_eq!(business_rows(&d), 1);
    }

    /// Two appliers racing on one operation serialise at the database: exactly one is `Fresh`.
    #[test]
    fn two_appliers_racing_on_one_operation_yield_exactly_one_fresh() {
        let d = Arc::new(dest("race"));
        let mut handles = Vec::new();
        for attempt in 1..=8u32 {
            let d = Arc::clone(&d);
            handles.push(std::thread::spawn(move || d.apply(&effect("op-1", attempt, b"credit 5")).unwrap().dedup));
        }
        let outcomes: Vec<DedupOutcome> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let fresh = outcomes.iter().filter(|o| **o == DedupOutcome::Fresh).count();
        assert_eq!(fresh, 1, "one applier applied; the rest replayed: {outcomes:?}");
        assert_eq!(business_rows(&d), 1);
    }

    /// **A timeout is `DeliveryUnknown`, never "nothing happened".** The slow first attempt commits
    /// after the caller stopped waiting, and the retry that resolves it comes back `Replayed`.
    #[tokio::test]
    async fn a_timed_out_apply_is_unknown_and_a_retry_resolves_it() {
        let slow: Handler = Arc::new(|tx: &rusqlite::Transaction<'_>, e: &Effect| {
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS ledger_entries (operation_id TEXT NOT NULL, payload BLOB NOT NULL);",
            )
            .map_err(|e| e.to_string())?;
            std::thread::sleep(Duration::from_millis(300));
            tx.execute(
                "INSERT INTO ledger_entries (operation_id, payload) VALUES (?1, ?2)",
                params![e.operation_id.as_str(), e.payload],
            )
            .map_err(|e| e.to_string())?;
            Ok(())
        });
        let d = Arc::new(SqliteDestination::open(temp("timeout"), "ledger", slow).unwrap());

        let first = apply_within(Arc::clone(&d), effect("op-1", 1, b"credit 5"), Duration::from_millis(20)).await;
        assert_eq!(first, Err(EffectRefusal::DeliveryUnknown), "the deadline passed; the fate is unknown");

        // The first attempt is still running on its blocking thread and will commit.
        tokio::time::sleep(Duration::from_millis(600)).await;
        let retry = d.apply(&effect("op-1", 2, b"credit 5")).unwrap();
        assert_eq!(retry.dedup, DedupOutcome::Replayed, "the retry resolves the unknown");
        assert_eq!(business_rows(&d), 1, "and the effect happened exactly once");
    }
}
