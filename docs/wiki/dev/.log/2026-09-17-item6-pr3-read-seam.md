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

**What I did not build, and why.** The channel seam. The inventory says "capacity and fullness are
kernel state, so 'full' is a schedulable fault" — which means a *simulated* channel, not a wrapper
around a real one. A channel send, unlike a file write, has in-process effects the replay is itself
reproducing: if the recording says "full" the replay must not send, and if it says "sent" it must.
Wrapping `try_send` and recording the outcome cannot deliver that, and improvising something that
looks like it would be worse than leaving it. It needs its own design pass.

**Baseline: 184 sites across 43 files.**

**Gates.** `make check` clean; core 179 without `sim`, 194 with; mycelium 459 and 402.
