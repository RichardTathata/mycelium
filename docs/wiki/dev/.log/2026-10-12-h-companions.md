## [2026-10-12] ingest | post-360 row H — the companions' four doors (#609)

- `mycelium-commitment`: lateness is decided by the **declarer's close** (`close`, `Close` at `cn/{r}/close` naming
  every offer by log position and SHA-256; `NotClosed` before one). The PR's first head judged on the HLC in the
  offer's log key; its review showed a key segment is offerer-written data, not a clock (backdated to
  `0000000000010000`, an award won a minute late). Residual: the declarer decides; a late or partial close is visible
  and, signed, attributable. Reads filtered to their own requirement; `award_of_verified` takes a pinned declarer;
  `plan_award_verified` / `commit_award_linearizable_verified` the checked path; `SlotHoldsAnotherRecord`,
  `UnprovenAwardHolds`.
- `mycelium-reason` 0.9.0: the blob store bounded **per digest** (`DEFAULT_BLOB_QUOTA_BYTES` 1 GiB, `open_with_quota`,
  507 `quota_exceeded`, lock-order row 58; `.tmp-*` removed at open); a run id one bounded segment at every door
  (`validate_run_id`, 400 `bad_run_id`, `TraceRecorder::new` → `Result`, `replay` strict on the key shape).
  Validated, not owned.
- Pages: `dev/companions/commitment.md`, `dev/companions/companions.md`, `dev/concurrency/lock-order.md` (row 58),
  `operations/companions.md`, guides 15 and 24, `deprecations.md` §36 (+ index row), the plan's §3.5 H block.
