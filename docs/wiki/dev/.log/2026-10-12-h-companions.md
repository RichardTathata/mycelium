## [2026-10-12] ingest | post-360 row H — the companions' four doors (#609)

- `mycelium-commitment`: the deadline is judged on the offer's **log position** (`hlc::physical_ms` of the HLC
  `append` minted), not the offerer's `offered_at_ms` — an offerer chose whether its own offer was late; `award_of`
  reports only a record naming its requirement and operation; `award_of_verified(requirement, resolve)` reports only
  an award the announcement's declarer signed. Residual: the offerer's node clock within `max_clock_drift_ms`.
- `mycelium-reason` 0.9.0: the blob store bounded (`DEFAULT_BLOB_QUOTA_BYTES` 1 GiB, `open_with_quota`, atomic
  reservation, 507 `quota_exceeded`); a run id one bounded segment at every door (`validate_run_id`, 400
  `bad_run_id`, `TraceRecorder::new` → `Result`, `replay` strict on the key shape) — `victim/x` wrote into
  `victim`'s trace, which `anchor` then sealed.
- Pages: `dev/companions/commitment.md` (bullets + gates), `dev/companions/companions.md` (the blob tier line),
  `operations/companions.md` (both sections), guide 15 and 24, `deprecations.md` §36, the plan's §3.5 H block.
