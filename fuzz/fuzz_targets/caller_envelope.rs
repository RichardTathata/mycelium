#![no_main]
//! The caller-context **envelope** — one layer past the frame (§12.6, v3 item 7).
//!
//! `split_frame` only classifies. Everything that actually reads the client's envelope happens
//! after it and still **before** the signature check: the JSON decode, the `via` node id, and the
//! two base64 credential fields. Fuzzing the classification alone stops one layer short of the
//! bytes an attacker shapes — and this is the path the Phase-C audit already reached once.
//!
//! The invariant is signing-field stability: the verifier rebuilds the signed message from these
//! parsed fields, so an envelope that parses one way and re-renders another would have one
//! credential verified and a different one acted on.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::caller_envelope_decode(data);
});
