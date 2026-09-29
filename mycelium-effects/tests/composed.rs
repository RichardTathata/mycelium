//! The **enforced** composition (`docs/design/composed-effect.md` §9): a destination commits a
//! composed effect only when it is attributed and authorised at this resource, now. Planted leg
//! by leg — every refusal leaves **no row**, and the one that holds commits and replays exactly as
//! an uncomposed effect does.
//!
//! Written before the check existed and observed to fail (every planted leg committed), so the
//! gate describes the enforcement rather than the other way round.

use std::sync::Arc;

use mycelium_effects::sqlite::Handler;
use mycelium_effects::{
    check_composition, AttemptId, ComposedEffect, Composition, CompositionLeg, DedupOutcome, Effect,
    EffectDestination, EffectRefusal, Mandate, OperationId, PrincipalId, ResourceAuthority, SqliteDestination, TermId,
};

fn ledger_handler() -> Handler {
    Arc::new(|tx: &rusqlite::Transaction<'_>, effect: &Effect| {
        tx.execute("CREATE TABLE IF NOT EXISTS ledger (operation TEXT PRIMARY KEY, kg INTEGER NOT NULL)", [])
            .map_err(|e| e.to_string())?;
        let kg: i64 = String::from_utf8_lossy(&effect.payload).parse().map_err(|_| "not a figure".to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO ledger (operation, kg) VALUES (?1, ?2)",
            rusqlite::params![effect.operation_id.as_str(), kg],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

fn rows(path: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row("SELECT COUNT(*) FROM ledger", [], |r| r.get(0)).unwrap_or(0)
}

fn destination(tag: &str) -> SqliteDestination {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "mycelium-composed-{tag}-{}-{}.sqlite",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    SqliteDestination::open(&path, "depot-ledger", ledger_handler()).unwrap()
}

fn principal(s: &str) -> PrincipalId {
    PrincipalId::new(s).unwrap()
}

fn mandate(holder: &str, scope: &str, ops: &[&str], epoch: u64, valid_until_ms: u64) -> Mandate {
    Mandate {
        holder: principal(holder),
        established_by: principal("coop-board"),
        purpose: "collect and weigh donations".into(),
        scope: scope.into(),
        operations: ops.iter().map(|s| s.to_string()).collect(),
        epoch,
        term: TermId::new("term-7").unwrap(),
        valid_from_ms: 0,
        valid_until_ms,
    }
}

fn composed(op: &str, kg: &str, composition: Composition) -> ComposedEffect {
    let operation = OperationId::new(op);
    let attempt = AttemptId::of(&operation, 1);
    ComposedEffect { effect: Effect::new(operation, attempt, kg.as_bytes().to_vec()), composition }
}

fn good() -> Composition {
    Composition {
        principal: principal("worker-a"),
        operation: "ledger.record".into(),
        mandate: mandate("worker-a", "depot-ledger", &["ledger.record"], 3, 10_000),
        origin_domain: None,
    }
}

const NOW: u64 = 5_000;

#[test]
fn a_composed_effect_that_holds_commits_once_and_replays() {
    let d = destination("holds");
    let authority = ResourceAuthority::new("depot-ledger", 3);
    let first = d.apply_composed(&composed("collection-1", "120", good()), &authority, NOW).unwrap();
    assert_eq!(first.dedup, DedupOutcome::Fresh);
    let again = d.apply_composed(&composed("collection-1", "120", good()), &authority, NOW).unwrap();
    assert_eq!(again.dedup, DedupOutcome::Replayed, "same operation, same content: one effect");
    assert_eq!(rows(d.path()), 1);
}

/// Each leg planted missing: the refusal names the leg, and **no row** exists afterwards.
#[test]
fn every_planted_leg_is_refused_by_name_and_nothing_is_applied() {
    let d = destination("legs");
    let authority = ResourceAuthority::new("depot-ledger", 3);

    let cases: Vec<(&str, Composition, CompositionLeg, &str)> = vec![
        (
            "someone else's mandate",
            Composition { principal: principal("worker-b"), ..good() },
            CompositionLeg::Attribution,
            "attributed to \"worker-b\" but the mandate is held by \"worker-a\"",
        ),
        (
            "operation not enumerated",
            Composition { operation: "ledger.delete".into(), ..good() },
            CompositionLeg::Authority,
            "does not enumerate \"ledger.delete\"",
        ),
        (
            "superseded epoch",
            Composition { mandate: mandate("worker-a", "depot-ledger", &["ledger.record"], 2, 10_000), ..good() },
            CompositionLeg::Authority,
            "superseded",
        ),
        (
            "wrong scope",
            Composition { mandate: mandate("worker-a", "another-ledger", &["ledger.record"], 3, 10_000), ..good() },
            CompositionLeg::Authority,
            "scope",
        ),
        (
            "expired",
            Composition { mandate: mandate("worker-a", "depot-ledger", &["ledger.record"], 3, 4_000), ..good() },
            CompositionLeg::Authority,
            "outside the mandate's window",
        ),
    ];
    for (i, (why, composition, leg, needle)) in cases.into_iter().enumerate() {
        let op = format!("collection-{i}");
        match d.apply_composed(&composed(&op, "120", composition), &authority, NOW) {
            Err(EffectRefusal::Unauthorised { leg: got, reason }) => {
                assert_eq!(got, leg, "{why}: wrong leg");
                assert!(reason.contains(needle), "{why}: reason {reason:?} should name {needle:?}");
            }
            other => panic!("{why}: must be refused as Unauthorised, got {other:?}"),
        }
        assert_eq!(rows(d.path()), 0, "{why}: nothing may be applied");
        // A refusal leaves no dedup row either: a later, authorised attempt is Fresh, not Replayed.
    }
    let ok = d.apply_composed(&composed("collection-0", "120", good()), &authority, NOW).unwrap();
    assert_eq!(ok.dedup, DedupOutcome::Fresh, "a refused attempt left no dedup row");
}

/// An epoch installed later at the resource supersedes an effect authorised under the earlier one —
/// the resource is where exclusivity is enforced (v2.14.0's limit, on the tin).
#[test]
fn installing_a_newer_epoch_supersedes_the_mandate_at_the_resource() {
    let d = destination("epoch");
    let mut authority = ResourceAuthority::new("depot-ledger", 3);
    d.apply_composed(&composed("collection-1", "120", good()), &authority, NOW).unwrap();
    assert!(authority.install(4));
    match d.apply_composed(&composed("collection-2", "80", good()), &authority, NOW) {
        Err(EffectRefusal::Unauthorised { leg: CompositionLeg::Authority, reason }) => {
            assert!(reason.contains("installed epoch 4"), "{reason}");
        }
        other => panic!("must be superseded, got {other:?}"),
    }
    assert_eq!(rows(d.path()), 1);
}

#[test]
fn the_pure_check_agrees_with_the_destination() {
    let authority = ResourceAuthority::new("depot-ledger", 3);
    assert!(check_composition(&good(), &authority, NOW).is_ok());
    let foreign = Composition { origin_domain: Some("partner.example".into()), ..good() };
    assert!(check_composition(&foreign, &authority, NOW).is_ok(), "the domain leg is carried, not checked here");
}
