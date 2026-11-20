## [2026-11-17] ingest | I4, PR 3b: the node's own stop at the bridged MCP loop and the SDK serve stream

`docs/plans/identity-and-lifecycle.md` row I4 records PR 3b (*Delivered by PR 3b*), the maintainer's decision (C) of
2026-11-16; `deprecations.md` §116.

- **Each agent promises its own behaviour.** At a door where another agent does the work (a bridged MCP server, an
  SDK subscriber) the node's stop is its own: it stops waiting, answers `cancelled`, tells the other agent
  (`notifications/cancelled`; a `cancel` event to a stream that said it reads them) and discards the late reply. A
  stop there is recorded **confirmed only on the other agent's own word** — an SDK subscriber's `rpc/respond`
  `"stopped": true` after it declared `stops_on_cancel`. Nothing else the node sees (a result, silence, a closed
  stream) is a confirmation. A bridged server cannot declare through the node, so the bridged loop keeps refusing a
  cancellable rule's call.
- **Waiting on a lapse without acknowledging it**: `WorkGuard::stop_asked` / `Admission::stop_asked` — the node's
  seeing the request is not the work's acknowledgement (`cancelled()` records one).
- **A streamed call's admission is owned by a watcher task** (`src/agent/http/served_call.rs`), found by
  `rpc/respond` in a papaya table; lock-order row 46 (`ProviderBudget::parked`) retired. The receipt's wait reads the
  stop record once confirming registrations reach zero, since one can end unconfirmed.
- **A new SSE event is sent only to a stream that asked for it** (`cancel_events`): an older SDK reads every event as
  a request.
