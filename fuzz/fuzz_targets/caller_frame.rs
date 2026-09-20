#![no_main]
//! The caller-context frame, classified from bytes a gateway client controls (§12.6, v3 item 7).
//!
//! An RPC payload is `[8-byte nonce][frame][application bytes]`, and this classification is the
//! first read of it — before any signature check. The target asserts **byte conservation**: every
//! input byte is accounted for exactly once and none is invented, whichever way the payload is
//! classified. That is the property the raw-emission guard rests on; a client that could make
//! envelope bytes reappear as application input would walk past it.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = mycelium::fuzz_internals::caller_frame_classify(data);
});
