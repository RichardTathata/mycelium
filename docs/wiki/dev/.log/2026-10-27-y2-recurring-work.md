# 2026-10-27 — Y2: declared recurring work

- `[[schedule]]` (`src/capability_config.rs`) and `mycelium::schedule` (`src/schedule.rs`): one holder per schedule by
  rendezvous on the ring `sched/{id}` among live candidates that match `holders` and run it; the holding is an
  advertisement (`schedule/{id}.holder`) that evaporates — the lease; no lock, so a partition has two holders.
- Every run carries `sched/{id}/{slot}` as its idempotency key; one effect per slot only at a deduplicating destination
  (X5's reconciler), shown by `two_holders_under_a_partition_give_one_effect_at_the_destination`.
- A missed run is `sched.missed` (skip · run late ≤ 64 · run once), traced with its inputs; runs are `sched.run`.
- New owned KV namespace `sched/` (the last slot run); refused on the raw KV routes; `deprecations.md` §67.
- The loop runs in the embedding application: `units/declare` refuses the section (422) and a stem refuses to start.
