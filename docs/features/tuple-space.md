# The tuple space

## What it is

`mycelium-tuple-space` is a pull-based pipeline buffer built on the public API: producers `put` opaque payloads into
named stage lanes, workers `take` from a lane only when they are ready, and `complete` moves an item atomically to the
next lane. Nobody predicts which worker is free — readiness announces itself. One node holds the space as primary,
mirrored to a secondary, with an optional write-ahead log; when the primary dies, the secondary promotes itself by the
same capability-freshness election any provider uses, with no coordinator. An item taken and never completed is
re-queued after a timeout, so delivery is at least once. Keyed puts and takes add joins on a key: a key need not be
unique — items under one key are claimed in arrival order (a re-queued one at the back, on the primary that re-queued
it: a requeue is not logged, so after a restart or a failover a key's items are in id order), and workers parked on one
key are served in arrival order and counted in the stage's `waiters`. One limit, in a lane or under a key: an item whose
hand-off to a worker failed (the worker had just timed out) is re-queued at the front, so two such hand-offs at once may
come back reversed relative to each other.

## When to use it, and when not

- **Use it when** work arrives faster than it is done, workers come and go, and a stage should be drained by whoever is
  ready rather than dispatched by a scheduler.
- **Not when** the work must happen exactly once at its destination: at-least-once delivery needs an idempotent effect,
  which is a [destination commit](destination-commit.md); and not when no secondary and no WAL are configured, where a
  primary crash loses everything.

## Turn it on

This feature is turned on in code, from the `mycelium-tuple-space` crate; its `gateway` feature adds the
`/gateway/tuple/*` routes, mounted with `with_http_routes`, and `metrics` its series.

- [`TupleSpace::new`](../../mycelium-tuple-space/src/lib.rs) with a `TupleConfig`: the namespace, the role (primary,
  secondary or auto), and `persist` with a `wal_path` for durability.
- [`put`](../../mycelium-tuple-space/src/lib.rs), [`take`](../../mycelium-tuple-space/src/lib.rs),
  [`complete`](../../mycelium-tuple-space/src/lib.rs) and [`ack`](../../mycelium-tuple-space/src/lib.rs); keyed:
  [`put_keyed`](../../mycelium-tuple-space/src/lib.rs) and [`take_by_key`](../../mycelium-tuple-space/src/lib.rs).
- [`http_router`](../../mycelium-tuple-space/src/http.rs) (`gateway`): the routes, under `tuple:read` and
  `tuple:write`; a raw route's `tuple.{ns}.{op}` kind needs the family scope. Scopes are checked only where the node is built with
  `compliance`: the crate depends on `mycelium` without default features, so the scope wall is your build's, and
  without it any valid bearer reaches the routes ([what-is-proven](../operations/what-is-proven.md#companion-s-rpc-kind-needs-family)).
- Settings to size: `worker_timeout_secs` (default 300), `high_watermark` (500), `mirror_payload_limit` (1 MiB) and
  `cap_refresh` (10 s) ([companions § mycelium-tuple-space](../operations/companions.md#mycelium-tuple-space--staged-pull-pipelines)).
- `rpc_roundtrip_two_nodes`, in [`mycelium-tuple-space/tests/rpc_roundtrip.rs`](../../mycelium-tuple-space/tests/rpc_roundtrip.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`llm_pipeline`](../../examples/README.md#the-capability-matrix): two LLM workers drain a two-stage donation pipeline,
each pulling from the deepest pending lane. CI runs it. That a secondary promotes when the primary goes is
`succession_chain_late_secondary_joins_promoted_primary`, in
[`mycelium-tuple-space/tests/failover.rs`](../../mycelium-tuple-space/tests/failover.rs).

## What it guarantees, and what it does not

- No guarantee or rule id of its own; a take, claim or emit door can hold work under the
  [exposure policy](exposure-policy.md).
- **Not built:** exactly-once delivery; a payload above `mirror_payload_limit` is not live-replicated and relies on the
  WAL; the first promotion on a fresh cluster can take about 100 s (the orphan grace); one owner per `wal_path`, and a
  WAL with a corrupt middle record refuses the open rather than truncating.
- **An item's id is not reissued, under one assumption** (ER3). A space with no id history — transient, or a WAL that is
  missing or empty — starts its ids at the wall clock (ms × 1024), so a restarted space does not reuse the ids of its
  previous life, which [destination commit](destination-commit.md) keys effects on; a WAL with history fences at its
  highest id + 1 and needs no clock. **The limit:** two fresh starts of one namespace whose clocks read within
  *N*/1024 ms of each other (*N* the ids the earlier one issued) can overlap — a clock stepped back, or a device with
  no real-time clock booting at a repeatable time; in one process the bump is 1 024 per store, so a store that issued
  more than 1 024 ids and is recreated within milliseconds can overlap, and *N* stores created in one process put the
  last base about *N* ms ahead of the clock. Ids stay below 2^53
  ([the ledger row](../operations/what-is-proven.md#fresh-id-space-not-reissued)).
- **A promoted mirror does not reissue an id the primary acked** (ER6-T, the fence half). A mirror fences its ids past
  every id a replicated or replayed record names — an `Ack`, a `Take`, a `Complete`'s old id — not only a `Put`'s, and
  a transient primary's last state chunk ends with a lone `Take` naming the last id it issued — a record that changes
  no liveness when a mirror applies it; a mirror that persists the fence just after the item's own `Put` may requeue
  that item once on a restart (at-least-once). **Not built** (ER6-T's
  other forms, to be reproduced before they are ranked): a mirror away for an ack the primary then compacted away is
  never told of it; a transient primary's state chunk carries no acked ids; a restarted mirror's index is not seeded
  from its WAL ([the ledger row](../operations/what-is-proven.md#board-failover-retires-acked)).
- **Evidence:** the ledger row
  [a failed WAL append is never followed by an acknowledged one](../operations/what-is-proven.md#failed-wal-append-never-followed-acknowledged),
  both companions' WALs included.

## Go deeper

- **Guide:** [07, pipelines](../guide/07-pipelines.md).
- **Runbook:** [companions § mycelium-tuple-space](../operations/companions.md#mycelium-tuple-space--staged-pull-pipelines)
  and [admission control](../operations/admission-control.md).
- **Design:** [`design/exactly-once-effect.md`](../design/exactly-once-effect.md) on why the space does not provide one.
- **Code:** [`mycelium-tuple-space/src/`](../../mycelium-tuple-space/src/lib.rs) (`lib.rs`, `store.rs`, `http.rs`, `rpc.rs`).
