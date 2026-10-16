## [2026-10-16] ingest | shutdown closes the soft-state gate before it sweeps

**What changed:** `mycelium_core::kv_persist::SoftStateGate`, held as `CoreCtx::soft_state_gate`. Every re-assert of
a soft-state family the shutdown sweep retracts writes through `gate.write(..)` — `run_kv_persist_task`, the
emergent-group task's `gcap/` and requirement-opacity writes, `write_opacity_key`, the load-opacity task's go-opaque
write — and `shutdown_with_timeout` calls `gate.close()` first. Closes the residual #627's review found (finding 1):
a re-assert between the sweep and the signal undid the sweep (20/20 shutdowns at a 5 ms refresh).

**The transferable part:** a lock-free "stop, and wait for whoever is already inside" is a two-variable handshake
and needs `SeqCst` on both sides (the writer's increment-then-read-flag against `close`'s set-flag-then-read-count).
Acquire/Release does not order a store before a later load of a different variable. Recorded in the memory-ordering
policy ([lock-free-and-atomics](../concurrency/lock-free-and-atomics.md)). No lock, so no lock-order row.

**Enumeration (rule 1):** writers of a swept family that run after shutdown begins — every caller of
`run_kv_persist_task`, plus `git grep -n "false, &ctx.hlc"` over `src/agent/` for non-tombstone writes in long-lived
tasks: `emergent_groups.rs` (two closures), `capability_ops.rs::write_opacity_key`, `opacity.rs` (go-opaque). MCP
`tools/` keys are written once at registration, not re-asserted. Tombstone writes are left ungated (they agree with
the sweep).

**Tests:** `a_fast_reassert_does_not_undo_the_shutdown_sweep` (`src/lib_tests.rs`, five shutdowns at a 5 ms refresh;
10/10 shutdowns kept the `svc/` advert at the peer with `close()` toggled off, 15/15 retracted with it — the `cap/`
half never reproduced at 5 ms), `kv_persist::gate_tests`. Upgrade note `docs/guide/deprecations.md` §42.
