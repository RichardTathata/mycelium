//! **PR 2's gate** (`docs/design/replay-nondeterminism-inventory.md` §5):
//!
//! > record a run, replay it with one WAL record's bytes changed at equal length, and require a
//! > divergence — a replay harness that passes that unchanged is not detecting divergence, only
//! > re-seeding.
//!
//! Equal length is the whole point. A trace that recorded `len=214` would accept changed content as
//! a faithful replay, and every later claim built on "this bundle reproduces the failure" would be
//! built on a reproduction that is not one. The two WAL records in the inventory's own example
//! (entries 7 and 8) are exactly that pair.

use mycelium_sim::{ChoiceKind, FsOutcome, Kernel, Seams, Sources, Trace};

/// Two WAL records of equal length and different content — the pair the schema was written around.
const RECORD_A: &[u8] = b"wal-record-payload-AAAAAAAAAAAAAAAA";
const RECORD_B: &[u8] = b"wal-record-payload-BBBBBBBBBBBBBBBB";

/// One node's run: draw a nonce, read the clock, append two WAL records.
///
/// `second` is the second record's bytes, so a replay can be handed different content at the same
/// length without changing anything else about the run.
fn run(kernel: &mut Kernel, sources: &mut Sources, second: &[u8]) -> Result<(), mycelium_sim::Divergence> {
    let mut n = Seams::new(kernel, sources, "n1");
    n.rng_u64("nonce")?;
    n.wall_now_ms()?;
    n.fs("wal.bin", "append", RECORD_A, 8192, true, || FsOutcome::Ok(RECORD_A.len()))?;
    n.fs("wal.bin", "append", second, 8192 + RECORD_A.len() as u64, true, || {
        FsOutcome::Ok(second.len())
    })?;
    Ok(())
}

fn record() -> Trace {
    let mut k = Kernel::recording();
    let mut s = Sources::seeded(1234, 1_789_000_000_000);
    run(&mut k, &mut s, RECORD_B).expect("recording never diverges");
    k.trace().clone()
}

/// **The gate.** Same length, different bytes, and the replay must stop.
#[test]
fn a_same_length_different_content_write_is_a_divergence() {
    assert_eq!(RECORD_A.len(), RECORD_B.len(), "the premise of this test");
    let recorded = record();

    // Replay the identical run: no divergence, and the whole trace is consumed.
    let mut k = Kernel::replaying(recorded.clone());
    let mut s = Sources::seeded(1234, 1_789_000_000_000);
    run(&mut k, &mut s, RECORD_B).expect("an identical replay must not diverge");
    assert!(k.is_exhausted(), "an identical replay consumes the whole trace");

    // Now change the second record's *content* at the same length. A harness that only recorded
    // lengths would sail through this.
    let changed: Vec<u8> = RECORD_B.iter().map(|b| if *b == b'B' { b'C' } else { *b }).collect();
    assert_eq!(changed.len(), RECORD_B.len(), "equal length is the whole point");

    let mut k = Kernel::replaying(recorded);
    let mut s = Sources::seeded(1234, 1_789_000_000_000);
    let divergence = run(&mut k, &mut s, &changed).expect_err("changed content must diverge");

    assert_eq!(divergence.actual.kind, ChoiceKind::Fs);
    assert_eq!(divergence.actual.stream, "wal.bin");
    assert_eq!(divergence.seq, 4, "it stops at the write that changed, not at the end");

    // Both sides are printed, and they differ in the digest rather than the length.
    let expected = divergence.expected.as_ref().expect("a recorded entry");
    assert!(expected.request.contains(&format!("len={}", RECORD_B.len())));
    assert!(divergence.actual.request.contains(&format!("len={}", changed.len())));
    assert_ne!(expected.request, divergence.actual.request, "the digests differ");
}

/// The first write is untouched, so the run gets that far before stopping — a divergence names
/// *where* the code departed, which is the question a reviewer has.
#[test]
fn the_divergence_names_the_first_departure_not_merely_that_one_happened() {
    let recorded = record();
    let changed: Vec<u8> = RECORD_B.iter().map(|_| b'Z').collect();

    let mut k = Kernel::replaying(recorded);
    let mut s = Sources::seeded(1234, 1_789_000_000_000);
    let d = run(&mut k, &mut s, &changed).expect_err("diverges");

    // rng, wall, first fs all matched; the fourth decision is where it parted.
    assert_eq!(k.position(), 3, "three decisions replayed cleanly before the divergence");
    let shown = d.to_string();
    assert!(shown.contains("recorded:") && shown.contains("replayed:"), "{shown}");
}

/// A flipped `sync` flag is a different write even with identical bytes — the flag changes what the
/// write *means* for durability, and the receipt ladder turns on it.
#[test]
fn a_flipped_sync_flag_diverges_even_with_identical_bytes() {
    let recorded = record();
    let mut k = Kernel::replaying(recorded);
    let mut s = Sources::seeded(1234, 1_789_000_000_000);

    let mut n = Seams::new(&mut k, &mut s, "n1");
    n.rng_u64("nonce").expect("matches");
    n.wall_now_ms().expect("matches");
    let d = n
        .fs("wal.bin", "append", RECORD_A, 8192, false, || FsOutcome::Ok(RECORD_A.len()))
        .expect_err("sync=false is a different write");
    assert!(d.actual.request.contains("sync=false"));
    assert!(d.expected.unwrap().request.contains("sync=true"));
}

/// A moved offset is a different write. Same bytes at a different place is not the same effect.
#[test]
fn a_moved_offset_diverges() {
    let recorded = record();
    let mut k = Kernel::replaying(recorded);
    let mut s = Sources::seeded(1234, 1_789_000_000_000);

    let mut n = Seams::new(&mut k, &mut s, "n1");
    n.rng_u64("nonce").expect("matches");
    n.wall_now_ms().expect("matches");
    assert!(n
        .fs("wal.bin", "append", RECORD_A, 9999, true, || FsOutcome::Ok(RECORD_A.len()))
        .is_err());
}

/// Replay returns what the *recording* received, not what the replayed code would have computed —
/// including a short write, which is neither success nor failure and is where the durability
/// argument lives.
#[test]
fn replay_supplies_the_recorded_outcome_including_a_short_write() {
    let mut k = Kernel::recording();
    let mut s = Sources::seeded(9, 0);
    {
        let mut n = Seams::new(&mut k, &mut s, "n1");
        n.fs("wal.bin", "append", RECORD_A, 0, true, || FsOutcome::Short(96)).expect("recorded");
    }
    let trace = k.trace().clone();

    let mut k = Kernel::replaying(trace);
    let mut s = Sources::seeded(9, 0);
    let mut n = Seams::new(&mut k, &mut s, "n1");
    let out = n
        .fs("wal.bin", "append", RECORD_A, 0, true, || {
            unreachable!("production must not run during replay")
        })
        .expect("no divergence");
    assert_eq!(out, FsOutcome::Short(96));
}

/// A recording is reproducible from its seed *before* it has been recorded — that is exploration
/// mode. Two runs at the same seed produce the same trace, so a failure found by exploring can be
/// re-found, and then bundled.
#[test]
fn two_runs_at_the_same_seed_produce_the_same_trace() {
    assert_eq!(record().to_text(), record().to_text());

    let mut k = Kernel::recording();
    let mut s = Sources::seeded(4321, 1_789_000_000_000);
    run(&mut k, &mut s, RECORD_B).expect("records");
    assert_ne!(k.trace().to_text(), record().to_text(), "a different seed is a different run");
}
