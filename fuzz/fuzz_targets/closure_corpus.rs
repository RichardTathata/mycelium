#![no_main]
//! The closure reader (adopter-surface X4): `mycelium explain --correlation` / `--closure` over bytes an operator
//! hands it — a trace file is text anyone can write.
//!
//! The invariant asserted: rendering a chain and a closure **terminates and visits each record once**, under a
//! hostile file too — a cycle of parents, a parent that is its own record, a hash that is not a hash. A reader
//! that looped or doubled a record would print a closure that is not one.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::closure_corpus_render(data);
});
