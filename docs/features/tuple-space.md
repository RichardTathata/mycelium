# The tuple space

## What it is

`mycelium-tuple-space` is a pull-based pipeline buffer built on the public API: producers `put` opaque payloads into
named stage lanes, workers `take` from a lane only when they are ready, and `complete` moves an item atomically to the
next lane. Nobody predicts which worker is free — readiness announces itself. One node holds the space as primary,
mirrored to a secondary, with an optional write-ahead log; when the primary dies, the secondary promotes itself by the
same capability-freshness election any provider uses, with no coordinator. An item taken and never completed is
re-queued after a timeout, so delivery is at least once. Keyed puts and takes add joins on a key.

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
  `tuple:write`; a raw route's `tuple.{ns}.{op}` kind needs the family scope.
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
- **Evidence:** the ledger row
  [a failed WAL append is never followed by an acknowledged one](../operations/what-is-proven.md#failed-wal-append-never-followed-acknowledged),
  both companions' WALs included.

## Go deeper

- **Guide:** [07, pipelines](../guide/07-pipelines.md).
- **Runbook:** [companions § mycelium-tuple-space](../operations/companions.md#mycelium-tuple-space--staged-pull-pipelines)
  and [admission control](../operations/admission-control.md).
- **Design:** [`design/exactly-once-effect.md`](../design/exactly-once-effect.md) on why the space does not provide one.
- **Code:** [`mycelium-tuple-space/src/`](../../mycelium-tuple-space/src/lib.rs) (`lib.rs`, `store.rs`, `http.rs`, `rpc.rs`).
