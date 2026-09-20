#![no_main]
//! A trust bundle, as an operator's configuration is read into the process (§12.6, v3 item 2).
//!
//! The weakest of the trust edges — a bundle is bilateral operator configuration, with no registry
//! and deliberately no way for a third party to add an entry. It is fuzzed anyway, because it is
//! the object that decides which keys are acceptable at all: a misparse here is not a refused call,
//! it is the wrong answer to *who do we trust*.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::trust_bundle_parse(data);
});
