## [2026-11-27] ingest | A mailbox key names its sender; its delete reaches the WAL (engineering review ER1, R7-01 + R7-03)

`deliver_event` wrote `mailbox/{target}/{kind}/{hlc:016x}`, and the stamp was the only thing that kept two events
apart. Two nodes' HLCs are independent, so two idle nodes delivering in one millisecond hand out the same stamp and
write one key: LWW keeps one event, both calls return `true`. The key is now `…/{hlc:016x}/{sender}` (the log route's
shape); one node's HLC never repeats, so the sender is enough salt, and it is deterministic under replay.

Durable knowledge:
- **An HLC stamp is unique per node, not per fleet.** A key that must be unique across writers needs the writer's id
  beside the stamp, after it (so key order stays stamp order). Sibling to check whenever this shape appears: the bare
  `{hlc:016x}` grant request id (`state_machine.rs`), not determined by reading.
- **A drain that skips entries must filter before its emptiness check.** `drain_prefix` loops until its scan is
  empty; an entry it will never consume (a suffix that is neither shape, e.g. kind `a/b`'s under mailbox `a`) must
  leave the scan in the same `retain` as the in-flight filter, or the loop never ends.
- **A prefix scan of `{kind}/` sees every kind that extends it with `/`.** The strict suffix parse (sixteen hex digits,
  then nothing or one segment that parses as a node id) is what keeps mailbox `a` out of kind `a/b`. Refusing `/` in a kind at
  deliver is a follow-up (decision D9′).
- **The mailbox's tombstone is now handed to the WAL**, as its insert was (this is not yet true of every
  tombstone: ER11 enumerates about forty sites, e.g. `state_machine.rs`, `mcp.rs`). The mailbox's
  `tombstone` applied and gossiped but never appended, so a node that exited without `shutdown()` replayed the insert
  and redelivered a consumed event. The rung is *queued*, not synced (`append_try`).
- **Mixed fleets:** an older receiver parses the whole remainder as hex and delivers a salted event with
  `hlc_ts == 0` — upgrade receivers before or with senders.

Pages: `docs/features/mailbox.md`, `docs/guide/deprecations.md` §TBD-ER1, `CHANGELOG.md`, `src/lib.rs` (namespace
table), `docs/wiki/dev/owner-signed-records.md` (the mailbox row's key).
