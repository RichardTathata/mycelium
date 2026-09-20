#![no_main]
//! The two signed objects an operator ingests from a partner (§12.6, v3 item 2):
//! `DomainDescriptor` and `DomainPolicy`.
//!
//! Both are parsed before their signatures are checked, and each carries a field a later decision
//! is keyed on — the descriptor's `exports` (what may be offered at all) and the policy's
//! `revision` (the monotonic high-water mark a consumer now enforces). Round-trip stability is the
//! invariant: a field that parses one way and re-renders another decides one thing and is audited
//! as another.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::federation_objects_parse(data);
});
