## [2026-11-16] ingest | I4, PR 3a: the cancel-forward, the cancel receipt, A2A `tasks/cancel` and MCP `notifications/cancelled`

`docs/plans/identity-and-lifecycle.md` row I4 records PR 3a (*Delivered by PR 3a*) and the split of PR 3 into 3a and
3b (§8, with the H3 decision put to the maintainer).

- The cancel-forward, `sys.call-cancel.v1` (`src/agent/call_cancel.rs`): the gateway sends the revoking checkpoint
  to the provider the call went to; the provider accepts it only from the issuing gateway (`cancel_not_from_issuer`),
  holds the revocation (`ExecutionAuthority::hold_call_revocation`, row 43, one hold — even for a call it has not
  seen, until issue + `GRANT_TTL_CAP_SECS`), sweeps, and answers its own class. A substrate kind: refused at every
  gateway raw route.
- The receipt names its rung (`CancelClass`): `Stopped` only where the agent doing the work promised to stop
  (`Running.confirms`, cleared by `WorkGuard::unconfirmed`) and it ended; `Unbounded` where it made no such promise;
  `DeliveryUnknown` on no answer, never a negative.
- The doors hold a `CallSink` (task-local `CALL_SINK` around the dispatch); a cancel before the mint is caught by
  the dispatch (`cancelled_before_sent`). `/mcp`'s id → term table is a papaya map keyed by principal, session,
  request id.
- A papaya 0.1.9 `retain` on a 65 536-entry map aborted a debug test ("misaligned pointer dereference" in
  `raw/mod.rs` `retain`). **Corrected by #799's review (H1):** `remove_if` played no part — papaya 0.1.9's `retain`
  reads through the tagged pointer, so after any incremental resize its closure sees bytes 4 off the entry (fixed
  in papaya 0.2.x). The table evicts by `iter` + `remove`; the one other papaya `retain` in the workspace,
  `call_issuer::collect`'s on `issued` (#794), is replaced the same way. Rule 3 on `lock-free-and-atomics.md`.
- #799's review, also: a provider that cannot see the work (no enforcement, or an unregistered plain-rule call)
  answers `not_registered`, never `not_running`; holds from cancel-forwards have their own budget
  (`MAX_CALL_HOLDS` 16 384, `MAX_CALL_HOLDS_PER_GATEWAY` 1 024); stop records name their gateway; a pending A2A
  task's re-send starts no second call.
- Pages touched: `lock-order.md` rows 43 and 48, `owner-signed-records.md` (the `call/` row narrowed),
  `operations/what-is-proven.md#call-checkpoint-replay`.
