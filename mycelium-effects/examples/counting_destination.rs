//! **Counting what a destination refused** — `Counting<D>`, the refusal counter (v2.22.0).
//!
//! ```text
//! cargo run -p mycelium-effects --example counting_destination
//! ```
//!
//! `destination_commit` shows the rung: an effect happens once at a destination you own. This shows
//! the destination's *ledger of outcomes*: a `Counting<D>` wrapper counts every commit and every
//! refusal by kind and by composition leg, so the destination's owner can see — without reading any
//! journal — that someone is presenting effects under another's mandate, or retrying with changed
//! content. The counter is a value; nothing here depends on a metrics stack.
//!
//! | Step | What you watch |
//! |---|---|
//! | 1 | a composed effect that holds commits, and is counted as a commit |
//! | 2 | the same effect replayed is a commit too (the effect is at the destination either way) |
//! | 3 | an effect presented by a principal who is not the mandate's holder: `unauthorised_attribution` |
//! | 4 | an effect under an expired mandate: `unauthorised_authority` |
//! | 5 | the same operation with different content on the plain path: `conflict` |
//!
//! The wrapper delegates `apply_composed` to the inner destination rather than re-deriving it from
//! `apply`, so a composed refusal counts exactly once — a destination that takes its authority inside
//! its transaction keeps doing so.

use mycelium_effects::sqlite::Handler;
use mycelium_effects::{
    AttemptId, ComposedEffect, Composition, Counting, DedupOutcome, Effect, EffectDestination, EffectRefusal,
    Mandate, OperationId, PrincipalId, ResourceAuthority, SqliteDestination, TermId,
};
use std::sync::Arc;

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

fn principal(s: &str) -> PrincipalId {
    PrincipalId::new(s).expect("a principal")
}

fn mandate(holder: &str, valid_until_ms: u64) -> Mandate {
    Mandate {
        holder: principal(holder),
        established_by: principal("coop-board"),
        purpose: "collect and weigh donations".into(),
        scope: "depot-ledger".into(),
        operations: vec!["ledger.record".into()],
        epoch: 3,
        term: TermId::new("term-7").expect("a term"),
        valid_from_ms: 0,
        valid_until_ms,
    }
}

fn composed(op: &str, kg: &str, presented_by: &str, holder: &str, valid_until_ms: u64) -> ComposedEffect {
    let operation = OperationId::new(op);
    let attempt = AttemptId::of(&operation, 1);
    ComposedEffect {
        effect: Effect::new(operation, attempt, kg.as_bytes().to_vec()),
        composition: Composition {
            principal: principal(presented_by),
            operation: "ledger.record".into(),
            mandate: mandate(holder, valid_until_ms),
            origin_domain: None,
        },
    }
}

const NOW: u64 = 5_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("mycelium-effects-counting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let depot = Counting::new(SqliteDestination::open(dir.join("depot.sqlite"), "depot-ledger", ledger_handler())?);
    let authority = ResourceAuthority::new("depot-ledger", 3);

    println!("Counting what a destination refused — Counting<D>");
    println!("=================================================\n");

    // 1. Holds: attributed to the mandate's holder, authorised now.
    let r = depot.apply_composed(&composed("collection-1", "120", "worker-a", "worker-a", 10_000), &authority, NOW)?;
    println!("1. attributed + authorised → {:?}   counts: {:?}", r.dedup, depot.counts());
    assert_eq!(r.dedup, DedupOutcome::Fresh);

    // 2. Replayed: still a commit, as far as the counter is concerned.
    let r = depot.apply_composed(&composed("collection-1", "120", "worker-a", "worker-a", 10_000), &authority, NOW)?;
    println!("2. the same effect again   → {:?}   commits: {}", r.dedup, depot.counts().commits);
    assert_eq!(r.dedup, DedupOutcome::Replayed);

    // 3. Attribution planted missing: worker-b presents an effect under worker-a's mandate.
    let r = depot.apply_composed(&composed("collection-2", "120", "worker-b", "worker-a", 10_000), &authority, NOW);
    println!("3. presented by another   → {}", match &r { Err(e) => e.to_string(), Ok(_) => "committed?!".into() });
    assert!(matches!(r, Err(EffectRefusal::Unauthorised { .. })));

    // 4. Authority planted missing: the mandate expired before NOW.
    let r = depot.apply_composed(&composed("collection-3", "120", "worker-a", "worker-a", 1_000), &authority, NOW);
    println!("4. under an expired mandate → {}", match &r { Err(e) => e.to_string(), Ok(_) => "committed?!".into() });
    assert!(matches!(r, Err(EffectRefusal::Unauthorised { .. })));

    // 5. A conflict on the plain path: same operation, different content.
    let op = OperationId::new("collection-1");
    let r = depot.apply(&Effect::new(op.clone(), AttemptId::of(&op, 2), b"999".to_vec()));
    println!("5. same id, other content  → {}", match &r { Err(e) => e.to_string(), Ok(_) => "committed?!".into() });
    assert!(matches!(r, Err(EffectRefusal::Conflict { .. })));

    let c = depot.counts();
    println!("\nthe ledger of outcomes: {c:?}");
    println!("  commits {}  ·  refusals {}  (attribution {}, authority {}, conflict {})",
        c.commits, c.refusals(), c.unauthorised_attribution, c.unauthorised_authority, c.conflict);
    assert_eq!((c.commits, c.unauthorised_attribution, c.unauthorised_authority, c.conflict), (2, 1, 1, 1));
    println!("\nWhat this does not claim: the counter is a value in this process — the durable trace of a");
    println!("refusal is the provider's evidence record; with the `metrics` feature the same wrapper");
    println!("also emits mycelium_effects_refusals_total{{kind, leg}}.");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
