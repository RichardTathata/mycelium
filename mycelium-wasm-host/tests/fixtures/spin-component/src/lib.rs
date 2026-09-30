//! A capability component that never returns: the guest for the D19 gate (an agent-published
//! entry that loops is stopped at its fuel budget; the same bytes under the operator's key run
//! unmetered — so the operator's test never invokes this one). Built out-of-band for
//! wasm32-wasip2 like the echo fixture; regenerate with build.sh.

wit_bindgen::generate!({
    world: "capability-component",
    path: "../../../wit",
});

use exports::mycelium::host::capability::{Guest, Request, Response};

struct Component;

impl Guest for Component {
    fn handle(req: Request) -> Response {
        // A loop the optimiser cannot fold: it consumes fuel until the host stops it.
        let mut x: u64 = req.payload.len() as u64;
        loop {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            if x == 0 {
                break;
            }
        }
        Response { payload: x.to_le_bytes().to_vec(), error: None }
    }
}

export!(Component);
