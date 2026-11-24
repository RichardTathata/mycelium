## [2026-11-23] ingest | B1 PR 2: the cancel-forward and its reply on the control lane

The cancel-forward rides the control lane (`rpc_call_ctx_control`); the provider's answer rides it back
(`rpc_respond_control`) only for a request that passed the receive boundary signed as its own sender — a reply's
target is the frame's unauthenticated `sender`, so a refusal goes by the shared path only. Durable knowledge: a cancel
can overtake its call (the provider holds the revocation of an unregistered term and refuses the call `revoked`);
the lane's yield tick follows the smaller of the live and start reconnect backoff; M4's off-reader evidence append
covers `rpc.result`. Evidence: the mints test on main without #824, 28/30 debug (two setup failures before any
cancel) and 10/10 release, the stop 10.8 to 35 ms after the cancel.

Pages touched: none in the wiki beyond this log (the runtime-invariants line from PR 1 stands). Outside it:
`docs/design/control-lane.md`, `docs/features/mandates.md`, `docs/features/signal-mesh.md`, `docs/guide/deprecations.md`
§127, `docs/operations/what-is-proven.md` (`call-cancel-forward-resent`, `call-cancel-behind-collector`).
