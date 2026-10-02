## [2026-10-02] ingest | the activation runner drains stderr while the command runs

**What:** `mycelium-wasm-host/src/activation.rs` `run` read a child's piped stderr only after exit; a
command writing more than a pipe buffer blocked and was reported as a timeout (360 review F3). Now a
reader thread drains it concurrently, keeping a 4 KiB tail; the tail is awaited at most 1 s after exit,
so a descendant holding the pipe cannot hang the result. Three tests, two seen failing first
(1 MiB of stderr: `did not finish within 10s`).

**Durable knowledge:** a piped child stream must be drained concurrently or not piped at all — a pipe
buffer is a deadlock bound, and the demo's quiet scripts never reached it, so the stem suite passed.
