## [2026-11-21] ingest | §9 O11, PR 1: group commit in the KV WAL's writer; a failed `Sync` poisons

`docs/wiki/dev/architecture/runtime-invariants.md` § Persistence gains invariant 6 (group commit) and a note under
the apply-then-append window on why #602's caller-side batching does not apply; its regression-floor sentence no
longer names `floor_observe_counts_any_update_at_or_after_write_ts` as current (PR 4a replaced it with
`observe_counts_only_this_exact_payload`; drift found by O11's scoping). Plan: `docs/plans/identity-and-lifecycle.md`
§9 O11 (PR 1 closed on evidence; the maintainer's decisions of 2026-11-21) and §9 O12 (new, not built); ledger row
`what-is-proven.md#call-cancel-behind-collector`; `deprecations.md` §125; replay inventory §2.3.

- **The mechanism.** `wal_writer_task` takes an append, drains what is already on the channel behind it
  (`sim_seam::chan_drain`; at most 4 096 appends or ≈ 4 MiB of keys and values; no window — it never waits), and
  `commit_batch` writes every frame in one `write_all`, syncs once if any member needs it, and only then answers each
  member. `Sync`, `TriggerSnapshot`, `Shutdown` and the test poison end a batch; a `Sync` is answered by its batch's
  sync. A failed encode, write or sync fails, counts and poisons for every member. The threshold snapshot runs at the
  batch's end.
- **Why it is not #602's group commit.** That batched on the caller's side and widened the apply-to-append window;
  this batches behind the channel, and `begin_inbound`'s window still closes at the enqueue.
- **What it changes for a caller.** No ack's meaning (the `floor_*` pins and the golden fixtures unchanged). A
  required-sync write told `DurabilityNotEstablished` may have a whole frame on disk that replays after a restart —
  now also when a later record of its batch tore (`kv_set_requiring_sync`'s doc says so). A failed `Sync` now
  poisons (an `fsync` that fails once can succeed later without the data), and goes through `fs_sync_data`.
- **Replay.** The drain count is a recorded `chan wal/drain` choice, recorded only when the drain took something, so a
  lone append's trace is unchanged. No checked-in trace needed re-recording (by reading and by running the corpus
  tests): scenario A drives `wal_append` directly, and the `x9`/`y2` corpora have no WAL effect.
- **Not in it:** the connection reader under `flush` still awaits each record's sync (O11's PR 2, which with the
  `flush` soak closes I4's exit 14); the journals' writers (O12); `PEER_WRITE_STALL_DEFAULT_MS`, to re-measure after
  PR 2.
