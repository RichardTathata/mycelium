## [2026-10-19] ingest | The shutdown flush waits for the wire: outbound frames counted (`a_fast_reassert` CI failure)

`a_fast_reassert_does_not_undo_the_shutdown_sweep` failed on `main` at 98a311fc: B kept A's `svc/` key in round 2 of 5.

**Not a regression.** No earlier CI failure of the test exists outside #628's own fail-first run. Measured under
24–32 parallel test processes, it failed 5 times in 496 runs on `main` and 3 times in 464 on v2.35.0. Failures kept
`svc/` or `cap/` alike, so neither the service advert nor the soft-state gate was the cause.

**Mechanism**, from a probe on a failing run:
1. `flush_outbound_before_shutdown` saw no queue (`saw_queue false`, `dropped 0`).
2. It sent the signal.
3. A's writer to B was stopped mid-write with an empty queue.

The sweep's tombstone had been dequeued into the writer's hand, where reading queue emptiness cannot see it.

**First fix and its review (#650).** The writer finished its batch after the signal. The independent review found
three holes:
- Nothing awaited that drain. The `mycelium` binary returns from `main` straight after `shutdown()`.
- The connect arm still broke with a frame in hand.
- A frame between a shard worker's dequeue and its hand-off was still invisible.

**Fix that landed.** `mycelium-core/src/outbound.rs` counts every outbound frame (`Unsent`) until it is on the wire
or dropped:
- **Producers** raise the count before they enqueue. They send through `GossipTxs` and `WriterTx`, which expose no
  raw sender.
- **A shard worker** raises it per writer hand-off before its `Held` releases its own copy.
- **A writer** lowers it after its socket flush, or when the frame is dropped by any path: oversize, backoff, a
  failed write or connect, eviction, shutdown, or an abort (`Held` and `CountedRx` release on drop).
- **The flush** waits, bounded at 250 ms, for zero.

The post-signal drain was removed, because nothing awaited it.

**Tests**, each seen failing first. They run in a strict CI step.
- `a_frame_a_writer_holds_at_shutdown_still_reaches_the_peer` (write pause): failed 10/10 before any fix.
- `…_is_written_before_shutdown_returns` (A on its own runtime, `shutdown_background()`): failed 10/10 on the first
  fix.
- `a_frame_in_a_slow_connect_at_shutdown_still_reaches_the_peer` (the writer evicted before the sweep by
  `TEST_BEFORE_SWEEP`, connect paused): failed 10/10 on the first fix.
- `a_frame_a_shard_worker_holds_at_shutdown_still_reaches_the_peer` (`TEST_SHARD_PAUSE_MS`): failed 18/20 on the first
  fix.
- `frames_dropped_to_a_dead_peer_do_not_hold_shutdown`: failed with the backoff drop's release removed.
- `an_undeliverable_frame_holds_shutdown_only_for_the_bound`: a frame held in a five-second connect does not hold
  `shutdown()` past the bound.
- `writer`'s backoff test also asserts the count returns to zero.

**Entry points checked:**
- every writer of `svc/`;
- the persist task's ungated closures;
- the sweep's `kv_delete`;
- every producer into a gossip shard or a writer queue (the compiler enumerates them now);
- every exit of `run_peer_writer` (recv, connect, write, backoff, oversize, idle, eviction, abort) and of the shard
  loop;
- #642's `depart()` and `reassert_own`;
- #647's `emit_signal_async`.

Pages: `dev/architecture/runtime-invariants.md` (§ Shutdown retracts through the sweep). Repo: CHANGELOG (Fixed),
`docs/guide/deprecations.md` §54, `docs/plans/post-360-hardening.md` §3.6.
