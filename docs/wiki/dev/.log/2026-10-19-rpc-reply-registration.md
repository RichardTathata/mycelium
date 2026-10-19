## [2026-10-19] ingest | A reply registered after its request is emitted can be lost (the #642 `/mcp` door-test flake)

Root cause of `the_mcp_door_counts_into_the_fleet_counter`'s intermittent 30 s timeout. `rpc_call_framed` emitted the
request, then registered for the reply. A provider on the same node receives the request inside `emit_signal`, and on
a multi-threaded runtime its serve task runs on another worker at once. When it answered before the registration, the
reply interceptor dropped the reply. The old comment said this was safe because registration came "before the first
yield", which holds only on one thread. Every requester shared the path, `bulk_call` included. The fix is
`rpc::PendingReply`: register, then emit, then wait, with removal on drop, so a cancelled call no longer leaks its
`rpc_pending` entry.

Seen deterministically by a hook that holds the caller after its emit until the provider has answered. With a 1 ms
pause, the door test failed with CI's exact message. 800 unpaused runs did not reproduce it on macOS.

Found beside it, open: a seat claim that loses one 2 s prepare phase cannot meet the 2 s lease margin
(`SEAT_LEASE_MARGIN_MS` against `CLAIM_PHASE_TIMEOUT`), so under load about 1 run in 200 refuses with "decided too
late". It fails closed.

Pages: `dev/architecture/runtime-invariants.md` (new section), `dev/architecture/hooks.md` (ReplyInterceptor row),
`dev/concurrency/lock-order.md` (row 2). Repo: `docs/plans/post-360-hardening.md` §3.6, CHANGELOG, CLAUDE.md.

## [2026-10-19] ingest | #647 review round 1: the reply guard owns its entry; the third emitter; the SWIM probe

Each finding closed with a test seen failing first. `bulk_call_ctx` runs the same after-emit hook as
`rpc_call_framed` (`rpc::after_emit`, test-only), and its self-dispatch regression timed out on the emit-then-register
order. `test_rpc_timeout` now asserts `rpc_pending` is empty; it failed with `Drop`'s removal disabled.
`PendingReply::register` draws a nonce no live call holds and the entry carries a registration id that `Drop` compares
(`RpcPending`'s value gained the id); a forced-collision test failed on overwrite `(7, 7)` and on removal by nonce
alone. `emit_signal_async` skipped the reply interceptor — not deliberate: the `rpc_pending` fast path (fecd36e1) was
added to `emit_signal` only, and `emit_signal_ordered` copied it later — so a co-located `rpc.result` sent through it was
lost; it now claims as the other two do (test failed with `Err(Timeout)`). A SWIM probe's `pending` entry is a
`PendingProbe` guard; dropping a probe mid-flight left the entry (test failed on the old code).

Pages: `dev/architecture/runtime-invariants.md` (the section, the occurrence count made four),
`dev/architecture/hooks.md` (ReplyInterceptor row: citations to `mod.rs:1252-1295`, the emitters, the fall-through).
