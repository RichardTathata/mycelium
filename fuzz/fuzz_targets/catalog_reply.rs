#![no_main]
//! A signed catalogue reply, parsed by a client before it is verified (§12.6, v3 item 2).
//!
//! The signature covers bytes derived from the parsed fields, so parse stability is what keeps
//! "the reply I verified" and "the reply I acted on" the same object. The Phase-C audit reached
//! this path once already — a replayed reply reinstated a withdrawn export — which is why the
//! decoder in front of it is fuzzed rather than trusted.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::catalog_reply_parse(data);
});
