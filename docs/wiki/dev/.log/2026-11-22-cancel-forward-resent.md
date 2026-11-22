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
