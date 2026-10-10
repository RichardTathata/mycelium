## [2026-10-10] ingest | tripwires on consensus records a peer should not write

- `SELF_OWNED_SYS_PREFIXES` (`mycelium-core/src/connection.rs`) grows from five to eight:
  `sys/consensus-accepted/`, `sys/identity-signed/`, `sys/identity-proof/` — each written only by its own node
  (grep of their writers: `persist_acceptor` via `accepted_key(&self.task_ctx.node_id, …)`; `lifecycle.rs` start
  and `mod.rs` rotation for the identity pair). `flag_foreign_sys_write`'s `{prefix}{owner}/…` parse fits all
  three (a slot with `/` in it still has the owner as its first segment). Counted on the receive path's three
  apply sites (anti-entropy, `Data`, `SignedData`).
- `consensus/decided/{slot}` is not self-owned, so its tripwire is in Layer III: `ConsensusEngine::decided_floor`
  (every floor reader goes through it) counts a floor more than `DECIDED_FLOOR_ANOMALY_MARGIN = 2^32` above the
  highest ballot this node has observed for the slot (ballot key, acceptor promise) — once per slot, in
  `TaskCtx::decided_floor_anomaly_slots` (a `papaya::HashSet`, no lock-order row); surfaced as
  `SystemStats::consensus_decided_floor_anomalies`, `/stats` and `mycelium_consensus_decided_floor_anomalies_total`.
  Refusal unchanged.
- Found on the way, not changed: the proposer's draw `max(ballot key, floor) + 1` overflows at a `u64::MAX` floor.
- Pages: `dev/security.md` (the tripwire item); ops `metrics.md`, `observability.md`.
