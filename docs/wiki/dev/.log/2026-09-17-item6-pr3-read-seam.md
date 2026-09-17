## [2026-09-17] ingest | item 6 PR 3 — the recovery-read seam, and why a read is checked not supplied

Up: [dev](../dev.md) · record `docs/design/replay-nondeterminism-inventory.md` §2.4, §5 · code
`mycelium-core/src/sim_seam.rs`, `persistence.rs`.

**The asymmetry worth keeping.** A **write**'s bytes are its *request* — the kernel already knows
them, so a replay checks the request and skips the effect entirely. A **read**'s bytes are its
*result*. Putting a snapshot's contents into a line-per-decision trace would make the trace the disk
image, which it is not: the bundle already carries disk images under `initial/`.

So a read is **checked, not supplied**. In replay it really happens, against the restored state, and
the seam compares what came back with what was recorded. That is a divergence check on recovery, and
the failure it catches has a name here: **v2.4.3**, where the WAL tail's read-back mapped an error to
an empty tail and the snapshot truncated acknowledged records away. A harness that supplied the
recorded bytes would have replayed straight past it — it would have *reproduced the run* and
*concealed the bug*, which is the worst thing a replay harness can do.

**Three reads routed:** the snapshot, the WAL, and the WAL tail the snapshot merges. The tail gets
its own stream (`wal.bin#tail`) because *that* read returning different bytes is the v2.4.3 failure
exactly, and it deserves to be legible on its own line rather than mixed in with recovery reads.

**The channel seam, built after all — and the design is the point.** My first instinct was that
wrapping `try_send` and recording the outcome would do. It will not, for a reason worth keeping: a
file write in replay can be *skipped*, because the disk is restored from the bundle; a channel send
**cannot**, because its effect is in-process and the replay is reproducing that process.

So the kernel **decides** and the call site **honours**: `Sent` performs the send, `Full` does not,
and a replayed `Sent` that finds the channel full is a divergence — the replay's channel state has
departed from the recording's — rather than a frame quietly dropped. The test asserts the first half
with an attempt closure that panics if it is called: a replayed drop must not even try.

Per-shard streams (`gossip/shard2`), because a drop on shard 2 and a drop on shard 5 are different
events, and a merged trace could not tell a reader which key stopped propagating.

Routed at `framing::dispatch_gossip_try_send` and at `connection.rs`'s four direct sends, which
bypass that helper and predate it.

**A design flaw of my own, found by the checker.** The first shape took a *closure* wrapping the
caller's `try_send`. That left the `try_send` in the call site, so when `try_send` was added to the
forbidden-call pattern, correctly-routed code failed the check — it could not tell routed from
unrouted. The fix is that **the seam owns the send**: `chan_try_send(stream, tx, msg)`. A seam you
cannot enforce is a seam that erodes, which is the whole reason D12 moved the check forward to PR 3.

**And the check's scope did not match the coverage map's.** §6's forbidden list names clocks, RNG and
storage, but **not channels** — though the coverage map assigns channel fullness to the kernel. With
`try_send` added, fifteen unrouted sends appeared across `persistence`, `signal`, `writer`, `a2a`,
`tasks`, `audit` and `topology`. Baseline **199 across 45 files**: a widening, like the alias fix
before it, where the number went up because the check got honest.

**The earlier note, kept because the reasoning was right even though the conclusion changed.** The
channel seam. The inventory says "capacity and fullness are
kernel state, so 'full' is a schedulable fault" — which means a *simulated* channel, not a wrapper
around a real one. A channel send, unlike a file write, has in-process effects the replay is itself
reproducing: if the recording says "full" the replay must not send, and if it says "sent" it must.
Wrapping `try_send` and recording the outcome cannot deliver that, and improvising something that
looks like it would be worse than leaving it. It needs its own design pass.

**Baseline: 184 sites across 43 files.**

**Gates.** `make check` clean; core 179 without `sim`, 194 with; mycelium 459 and 402.
