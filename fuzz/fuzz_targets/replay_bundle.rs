#![no_main]
//! The replay bundle's other readers (§12.6, v3 item 6) — the two layers `Trace::parse` does not
//! reach.
//!
//! 1. **The recorded outcome.** `Trace::parse` takes a choice's result as the verbatim remainder of
//!    the line and does not interpret it, so `FsOutcome::decode` is a second parse the trace target
//!    never touches. A misparse there is swallowed at the call site and the replayed code is told
//!    the write *failed* when the recording said it succeeded — a silent divergence, which is the
//!    one failure this crate exists to prevent.
//! 2. **The bundle's hand-rolled JSON.** `build.json`, `config.json` and `witness.json` are read by
//!    a `split_once` / `trim_matches` parser and never verified. A reproduction bundle travels with
//!    a bug report, so in practice these are third-party bytes, and the stake is `Build.commit` —
//!    the field a replay uses to decide it is replaying against the same binary.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let _ = mycelium_sim::FsOutcome::fuzz_roundtrip(text);
    let _ = mycelium_sim::bundle::fuzz_object_roundtrip(text);
    let _ = mycelium_sim::bundle::fuzz_build_roundtrip(text);
    // The stronger claim: start from a map the fuzzer controls and assert it survives being
    // written and read back. Round-tripping text alone hides a stably-lossy reader.
    let _ = mycelium_sim::bundle::fuzz_object_fidelity(text);
});
