## [2026-10-16] ingest | shutdown retracts every soft-state family a node keys with its own id

**What changed:** `GossipAgent::shutdown_with_timeout` (`src/agent/lifecycle.rs`) sweeps `svc/{kind}/{self}`,
`tools/{name}/{self}` and `gcap/{group}/{ns}/{name}/{self}` alongside `cap/{self}/` and `req/{self}/`, before
`flush_outbound_before_shutdown`. Recorded as a follow-up under row F in the post-360 plan §3.6 (#611's review) for
`svc/` and `tools/`; `gcap/` was found by enumeration.

**The transferable part — why the sweep, not the tasks:** a soft-state task (`run_kv_persist_task`, the MCP tool
and bridge tasks, the group-membership task) tombstones its key when it exits, and it exits on the shutdown signal.
The signal is sent *after* the outbound flush, and the writers stop on that same signal, so a task woken by it builds
a (signed) tombstone for writers that are already going. Only what is queued before the flush is delivered. So any
family a node keeps alive under its own id must be in the sweep; a handle being live at shutdown is the normal case
(the node stops, the work is not dropped first).

**The enumeration (rule 1):** every caller of `run_kv_persist_task` (`git grep -n "run_kv_persist_task("`) and
every task that tombstones on exit (`git grep -n "kv_delete(\|, true, &ctx.hlc"`). Families and their state:
`cap/`, `req/`, `sys/load/{self}/req/`, `sys/load/{self}/group-req/`, `cap/{self}/locality/self` — already swept;
`svc/`, `tools/`, `gcap/` — swept now; `cap-group/{name}` — excluded on purpose (ownership is not readable from the
key; the niche outlives one member); `grp/{group}/{self}` membership — left as it was (a membership, not an advert).

**Test:** `soft_state_with_a_live_handle_at_shutdown_is_retracted_at_the_peers` (`src/lib_tests.rs`, `tls`; health
checks every 60 s so anti-entropy cannot mask it), seen failing first on all three families 3/3, passing 5/5 after.
