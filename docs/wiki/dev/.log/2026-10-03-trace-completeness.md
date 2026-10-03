## [2026-10-03] ingest | the trace's last three gaps

**What:** `ActivationCtx` + `RuntimeCtx::{trace, install_token}` (activation and probe recorded under the
install token); the node binary's bundle carries the trace and coverage manifest; `mycelium-wasm-host`
`sim` feature with the draw-count test; `tests/decision_trace_replay.rs` — the comparison, run.

**Durable knowledge:** the operation id the plan asked for was already there — the install token — and the
link was a matter of carrying it into the hook; `parent_seq` was the wrong tool for a chain whose records
come from different tasks. And the draw-count claim is cheapest to prove at the decision point itself: the
provisioner's one draw, sixteen times, under the kernel, with and without a sink — not through a live mesh
whose demand registration the kernel's timers do not drive.
