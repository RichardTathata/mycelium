## [2026-10-27] ingest | Y4: the record a span names, read by its hash

Y4's last promise, split out of #684 by Richard (2026-10-26): `GET /gateway/records/decision/{sha256}` and
`GET /gateway/records/evidence/{sha256}`, `audit:read`, feature `compliance` (`src/agent/http/compliance.rs`). The
answer carries the exact bytes (`record_b64`); evidence is verified against the reference record in the node's audit
stream that cites its hash. The sink is read by `DecisionSink::try_read_after` (`try_lock`, 64 copies per hold —
lock-order row 53), a declared trace by `closure::find_in_trace` (`GossipAgent::with_decision_trace_file`). *Not
found* is `never_existed`, `not_written`, or `undetermined` with `cannot_rule_out`. Pages: [security](../security.md)
§ WS2; `operations/audit.md` §3a; `operations/rbac.md`; `features/span-export.md`; guide 19; the inventory's
`closure.rs` row.
