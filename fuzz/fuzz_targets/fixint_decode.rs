#![no_main]
//! The hand-rolled fixed-int codec (§12.6, v3 item 4).
//!
//! `serde_fixint` is the byte layer under the rights ledger's signed documents, and two objects
//! reach it from outside the process: a `LedgerEvent` read back from the ledger on disk, which
//! carries no signature at all, and a `PublishedRightsHead` read from `rights/head/{holder}` in the
//! gossip medium, which any peer can write and which is decoded before it is verified.
//!
//! A 20,000-case mutation sweep over a valid encoding found **no panic** — the decoder checks its
//! remaining length before every read, so the unchecked subtraction in `remaining()` is not
//! reachable from these types. This target exists anyway, for two reasons: that sweep is far weaker
//! than coverage-guided fuzzing, and a hand-rolled binary decoder reading bytes off disk is exactly
//! the shape behind the unbounded-allocation decode DoS that sat uncaught through M2 Run-20.
//!
//! The invariant is round-trip stability rather than byte equality: `from_slice` deliberately
//! tolerates trailing bytes, so a re-encoding need not reproduce the input.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::fixint_decode(data);
});
