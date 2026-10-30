## [2026-10-28] ingest | Y2's replay — a schedule run recorded and replayed by another process

**What landed.** Row Y2's last promise ("the tick through the timer seam, so a recording replays it"): the corpus bundle
`tests/replay-corpus/y2-schedule-run/` (one node holding `close`, a `run_once` decision over missed slots, the gap's
run, the on-time run) and `tests/schedule_replay.rs`, which replays it on a paused clock and compares every schedule
decision on its rule, outcome, reason, effect and every input. CI: `cargo test --features sim --test schedule_replay`.

**The durable lesson.** A recording replayed in its own process shares that process's seeds; a checked-in recording,
replayed by the next run, does not. The first checked-in recording of a whole node found two per-process seeds on the
replay path that every earlier whole-node replay (`decision_trace_replay`, same process) had hidden:

- `framing::shard_hasher` — `RandomState::new()`, so the shard a key maps to (and the channel stream a send is
  recorded on) differed per process. Fixed seeds under `sim` only; pinned by
  `framing::shard_seed_tests::under_sim_a_key_maps_to_the_same_shard_in_every_process`.
- the shutdown sweep (`lifecycle.rs` `shutdown_with_timeout`) tombstoned keys in the store map's iteration order, which
  follows its per-process hasher. Sorted, in every build.

- `GossipConfig::default` set `gossip_shards` from `available_parallelism()` — a **host** read, which the seams gate
  does not scan. Recorded on a 12-core laptop (16 shards), the bundle diverged on CI's 4-core runner (4 shards): shard
  6 masked to 4 is shard 2, exactly the replay's. A constant (`SIM_GOSSIP_SHARDS`) under `sim`. So a checked-in
  recording must be independent of the process *and* the machine: a whole-node replay in the same process on the same
  host hides both.

A recording's node name is part of it too: it is in the keys the shards hash and the persist streams' names, and
`start()` requires the name to be the bind address — so the recorded node binds a fixed port (15946, below
`alloc_port`'s range and the ephemeral floor).

**Seen failing first:** the ticker off the seam → diverged at `sched/close` (3/3); the sweep unsorted → diverged at the
sweep (2/3); the random shard seed → two processes, two maps. Inventory §2.5 rows updated.
