## [2026-10-10] ingest | core resource bounds (post-360 hardening row B)

- Five bounds, each failing first: a handshake / first-frame / idle bound on the gossip accept path
  (`handshake_timeout_ms`, `inbound_idle_timeout_secs`); connect and write timeouts on the outbound
  writer (`peer_write_timeout_ms`) with shutdown and eviction interrupting a blocked write; one
  anti-entropy reply in flight per peer (`bounds::ReplySlot`); admission fill as the least-full
  subscriber, so a stalled one drops its own signals; the sender log capped per kind and in kinds;
  `RpcRequestRx::recv` ends at shutdown, so a stopped agent frees its `TaskCtx`.
- Four counters on `SystemStats` and as metrics.
- Pages touched: `dev/architecture/runtime-invariants.md` (new section); `dev/concurrency/lock-order.md`
  row 18 (the sender log's inner type); outside the wiki, `docs/reference/configuration.md`,
  `docs/operations/{metrics,tuning}.md`, `docs/threat-model.md` §4, the replay inventory §2.4.
