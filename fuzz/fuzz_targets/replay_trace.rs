#![no_main]
//! Replay-trace decoding (§12.6, v3 item 6).
//!
//! A trace is the record a whole-node recording replays from, so what it decodes to *is* the run.
//! `Trace::parse` states its own rule — **a line that does not parse is an error rather than a
//! skip**, because a trace with a hole in it would replay as a different run while looking like the
//! same one. The target asserts exactly that, in the form it can be checked: a trace that parses
//! must re-render and re-parse to itself, so no entry is silently dropped or invented in between.
//!
//! `Trace::parse` is public API of the sim crate, so this target needs no `fuzz_internals` hook —
//! it calls the same function a replay does.
use libfuzzer_sys::fuzz_target;
use mycelium_sim::trace::Trace;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let Ok(trace) = Trace::parse(text) else { return };

    let rendered = trace.to_text();
    let again = Trace::parse(&rendered).expect("a rendered trace must re-parse");
    assert_eq!(trace, again, "a trace did not survive its own round trip");
});
