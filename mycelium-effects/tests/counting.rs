//! Zero-gaps Z6 (D6): a destination's refusals are **countable**, by kind and by leg, beside the
//! evidence journal's landing — the one `~` cell the doc-coverage matrix carried since run 18.
//! Written before `Counting` existed and seen failing to compile; the counts describe the
//! wrapper, not the other way round.

use std::sync::Arc;

use mycelium_effects::sqlite::Handler;
use mycelium_effects::{
    AttemptId, ComposedEffect, Composition, Counting, DedupOutcome, Effect, EffectDestination, EffectRefusal, Mandate,
    OperationId, PrincipalId, ResourceAuthority, SqliteDestination, TermId,
};

fn ledger_handler() -> Handler {
    Arc::new(|tx: &rusqlite::Transaction<'_>, effect: &Effect| {
        tx.execute("CREATE TABLE IF NOT EXISTS ledger (operation TEXT PRIMARY KEY, kg INTEGER NOT NULL)", [])
            .map_err(|e| e.to_string())?;
        let kg: i64 = String::from_utf8_lossy(&effect.payload).parse().map_err(|_| "not a figure".to_string())?;
        tx.execute("INSERT OR REPLACE INTO ledger (operation, kg) VALUES (?1, ?2)", rusqlite::params![effect.operation_id.as_str(), kg])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn destination(tag: &str) -> SqliteDestination {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!("mycelium-counting-{tag}-{}-{}.sqlite", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_file(&path);
    SqliteDestination::open(&path, "depot-ledger", ledger_handler()).unwrap()
}

fn principal(s: &str) -> PrincipalId { PrincipalId::new(s).unwrap() }

fn mandate(holder: &str, valid_until_ms: u64) -> Mandate {
    Mandate {
        holder: principal(holder),
        established_by: principal("coop-board"),
        purpose: "collect and weigh donations".into(),
        scope: "depot-ledger".into(),
        operations: vec!["ledger.record".into()],
        epoch: 3,
        term: TermId::new("term-7").unwrap(),
        valid_from_ms: 0,
        valid_until_ms,
    }
}

fn composed(op: &str, kg: &str, principal_name: &str, holder: &str, valid_until_ms: u64) -> ComposedEffect {
    let operation = OperationId::new(op);
    let attempt = AttemptId::of(&operation, 1);
    ComposedEffect {
        effect: Effect::new(operation, attempt, kg.as_bytes().to_vec()),
        composition: Composition { principal: principal(principal_name), operation: "ledger.record".into(), mandate: mandate(holder, valid_until_ms), origin_domain: None },
    }
}

const NOW: u64 = 5_000;

#[test]
fn a_refused_composed_effect_is_counted_by_leg_and_a_commit_is_counted_too() {
    let d = Counting::new(destination("legs"));
    let authority = ResourceAuthority::new("depot-ledger", 3);

    // Holds: one commit, one replay — both commits as far as the counter is concerned.
    let first = d.apply_composed(&composed("collection-1", "120", "worker-a", "worker-a", 10_000), &authority, NOW).unwrap();
    assert_eq!(first.dedup, DedupOutcome::Fresh);
    d.apply_composed(&composed("collection-1", "120", "worker-a", "worker-a", 10_000), &authority, NOW).unwrap();

    // Attribution planted missing: presented by worker-b under worker-a's mandate.
    let r = d.apply_composed(&composed("collection-2", "120", "worker-b", "worker-a", 10_000), &authority, NOW);
    assert!(matches!(r, Err(EffectRefusal::Unauthorised { .. })), "{r:?}");
    // Authority planted missing: the mandate expired before NOW.
    let r = d.apply_composed(&composed("collection-3", "120", "worker-a", "worker-a", 1_000), &authority, NOW);
    assert!(matches!(r, Err(EffectRefusal::Unauthorised { .. })), "{r:?}");
    // A conflict on the uncomposed path: same operation, different content.
    let op = OperationId::new("collection-1");
    let r = d.apply(&Effect::new(op.clone(), AttemptId::of(&op, 2), b"999".to_vec()));
    assert!(matches!(r, Err(EffectRefusal::Conflict { .. })), "{r:?}");

    let c = d.counts();
    assert_eq!(c.commits, 2, "{c:?}");
    assert_eq!(c.unauthorised_attribution, 1, "{c:?}");
    assert_eq!(c.unauthorised_authority, 1, "{c:?}");
    assert_eq!(c.conflict, 1, "{c:?}");
    assert_eq!(c.refusals(), 3, "{c:?}");
    assert_eq!(d.identity(), "depot-ledger", "the wrapper is transparent");
}
