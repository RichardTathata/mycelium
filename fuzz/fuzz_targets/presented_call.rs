#![no_main]
//! A presented federation credential's header value (§12.6, v3 item 2).
//!
//! Entirely partner-controlled, carried on `/a2a`, and parsed *before* the credential is
//! authenticated. The target asserts **parse stability** — the verifier rebuilds the canonical
//! signing bytes from the parsed fields, so a header that parses one way and re-renders another
//! would verify a different credential from the one actually presented.
//!
//! It also asserts that a *present* header never parses as an absent one: the module's own rule is
//! that a client which tried to present a credential and failed must not be anonymised.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::presented_call_parse(data);
});
