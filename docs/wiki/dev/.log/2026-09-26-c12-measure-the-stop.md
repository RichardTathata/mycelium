## [2026-09-26] ingest | closure plan C12: the stop, measured on a live node

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §10 · code
`examples/authority_drain.rs`.

`drain_report` had only ever been fed numbers a test made up. The example runs a real node, admits calls under a
mandate, revokes it, and takes T_admit and T_drain from the node's own records: the handler's entry times and the
authority's stop records. First run: T_drain 25 ms against a declared bound of 300 ms (s + the sweep interval +
declared cancellation and confirmation), nothing admitted after the revocation, no unconfirmed stops.

Two findings. An MCP tool loop serves one call at a time, so "calls admitted before the revocation" means "the call
that was running"; the rest queue and meet the revocation at admission, which is exactly T_admit. And a Rust member
could not compute the arguments digest its own possession proof needs; it was crate-private while both SDKs exported
it.

Open: the same measurement inside the confined-fleet kind cluster, which needs a node image built in that job.
