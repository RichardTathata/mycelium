## [2026-11-22] ingest | A gateway re-sends an unanswered cancel-forward (I4, §9 O13)

The maintainer's decision of 2026-11-22 on #824's finding, built: option A. `call_cancel::forward` sends an attempt at
once and again every 500 ms until any attempt is answered (each waits 5 s for its own answer, so they overlap), then
on a back-off from 1 s to 30 s, at most 24 attempts; none past the call's window (`valid_until_ms` + 2·skew) or once
the node shuts down. The receipt is settled as before — the first answer within 5 s, else `DeliveryUnknown` — through
a `settle` callback, so a later answer is counted and never revises it. `GET /stats` `call_cancel_forwards` and two
counters are the evidence for the deferred option B (a lane of its own for control signals).

Durable knowledge from the build:
- **Where #824's forward was lost** (one probe, five runs): at `run_gossip_shard`'s hand-off to the peer writer, its
  channel full — not at the shard queue (`emit_signal` returns `true` there) nor in the writer's backoff. When it was
  handed on it arrived about 1 s later, in order behind the connection's gossip: a re-send fixes the drop, not the
  delay.
- **A pending forward held the shutdown**: `shutdown_with_timeout` drains the task set, and a forward waiting out its
  5 s timeout held it. The forward now watches `shutdown_tx` and drops its attempts.
- **A repeat is idempotent at the provider**: `hold_call_revocation` finds the window held and the offer is a no-op;
  the answer comes from the stop record.

Pages: the plan's §9 O13 (and O10's pointer to it), `docs/operations/what-is-proven.md` (`call-cancel-forward-resent`),
`docs/features/mandates.md`, `docs/operations/metrics.md`, `CHANGELOG.md`.

**From #826's review, the same day:**
- **Behind the gateway's own flood a re-send turns lost into late**, and late is often more than *F*: on #824 with
  this code the stop was inside *F* in 5 of 10 runs (every receipt `Stopped`, none by anti-entropy). §9 O13 stays
  open, pending option B.
- **One question, one wait:** a provider joins a repeat of a forward to the answer already in progress for the same
  `(issuer, scope)` (`call_cancel::serve`). Without it a re-send took a confirm-wait permit, or, finding none, was
  answered `stop_requested` at once and settled the receipt ahead of the first attempt's waited `stopped`.
- **One forward per term at the gateway** (`forward_once`): join while unsettled, replace once settled.
- **A test that orders an acknowledgement against a poll of the provider races** whenever the forward's answer
  releases the acknowledgement; assert the receipt's rung inside the cancel's own task instead.
- **A burst test must prove its burst held the disk, per round** (the confirm round): the revoking checkpoint is in
  the gateway's KV only once synced, so the first checkpoint read just after the stop proves the write was pending;
  an overshot calibration is inconclusive and re-run larger, never a pass.
- **In-progress entries in papaya maps get a drop guard**, so a panic cannot leave an entry that later callers join.
