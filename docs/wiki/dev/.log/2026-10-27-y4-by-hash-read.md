## [2026-10-27] ingest | Y4: the record a span names, read by its hash

Y4's last promise, split out of #684 by Richard (2026-10-26): `GET /gateway/records/decision/{sha256}` and
`GET /gateway/records/evidence/{sha256}`, `audit:read`, feature `compliance` (`src/agent/http/compliance.rs`), #692.
The answer carries the exact bytes (`record_b64`); evidence is verified against the reference record in the node's
audit stream that cites its hash, a verifying one preferred. Decisions are read from the sinks registered with the
route through a tap per reader (`TapSlot::RecordIndex`, lock-order row 72) into a bounded index
(`src/agent/record_index.rs`, row 73) — after #692's review found the first version's `try_lock` reads made decision
points drop records — and from a declared trace (`closure::find_in_trace`). *Not found* is always `undetermined` for a
decision (a sink the route was never given is not searched, `not_searched`); evidence's `never_existed` is scoped to
the journal as opened. Pages: [security](../security.md) § WS2; [lock-order](../concurrency/lock-order.md) rows 72–73;
`operations/audit.md` §3a; `operations/rbac.md`; `features/span-export.md`; guide 19; the inventory's `closure.rs` row.
