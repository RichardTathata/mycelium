# The blackboard

## What it is

`mycelium-blackboard` is shared working memory for agents: agents post typed facts — attributes and a payload — and
others react to the facts whose attributes match a predicate. Reading is unconditional and concurrent; consuming is
competitive. When several agents' predicates match a finite fact, they race for an atomic claim and exactly one wins;
the winner acks the fact when done, and a claim not acked within its timeout is re-queued, so a winner that dies does
not lose the fact. Where the tuple space routes work by lane, the blackboard routes by content: the predicate is the
capability attribute-filter grammar. Like the tuple space, it runs as a primary mirrored to a secondary, with an
optional write-ahead log and failover with no coordinator.

## When to use it, and when not

- **Use it when** agents should take up work by what it is — a surplus here, a deficit there — and each fact must be
  acted on by one of them.
- **Not when** facts must expire: a fact lives until it is claimed and acked, so an undrained board grows without bound;
  and not when the work is a staged pipeline, which the [tuple space](tuple-space.md) fits better.

## Turn it on

This feature is turned on in code, from the `mycelium-blackboard` crate; its `gateway` feature adds the
`/gateway/bb/*` routes, mounted with `with_http_routes`, and `metrics` its series.

- [`Blackboard::new`](../../mycelium-blackboard/src/lib.rs) with a `BoardConfig`: the namespace, the role, and `persist`
  with a `wal_path`.
- [`post`](../../mycelium-blackboard/src/lib.rs), [`read`](../../mycelium-blackboard/src/lib.rs),
  [`claim`](../../mycelium-blackboard/src/lib.rs), [`ack`](../../mycelium-blackboard/src/lib.rs) and
  [`release`](../../mycelium-blackboard/src/lib.rs), with a [`Predicate`](../../mycelium-blackboard/src/lib.rs) built
  from `eq` and `present`.
- [`http_router`](../../mycelium-blackboard/src/http.rs) (`gateway`): the routes under `board:read` and `board:write`; a
  raw route's `blackboard.{ns}.{op}` kind needs the family scope.
- `claim_timeout_secs` (default 300) re-queues an in-flight claim; it is not eviction
  ([companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool)).
- `two_claims_over_one_finite_fact_exactly_one_wins`, in [`mycelium-blackboard/src/store.rs`](../../mycelium-blackboard/src/store.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`microgrid`](../../examples/README.md#the-capability-matrix): a community microgrid's agents post and claim energy facts;
CI runs it, and [`microgrid_viz`](../../examples/README.md#the-capability-matrix) is the browser view. That an
in-flight claim survives a primary's failover is `inflight_claim_survives_primary_failover`, in
[`mycelium-blackboard/tests/board_failover.rs`](../../mycelium-blackboard/tests/board_failover.rs).

## What it guarantees, and what it does not

- No guarantee or rule id of its own; a claim door can hold work under the [exposure policy](exposure-policy.md).
- **Not built:** exactly-once processing — one claimer wins, but a re-queued claim can be processed again, so the effect
  should be idempotent ([destination commit](destination-commit.md)); expiry or eviction of facts; durability of the
  last ~1 s at a clean shutdown, which does not fsync the WAL ([companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool)).
- **Evidence:** the ledger row
  [a failed WAL append is never followed by an acknowledged one](../operations/what-is-proven.md#failed-wal-append-never-followed-acknowledged),
  both companions' WALs included.

## Go deeper

- **Guide:** no guide chapter; the [examples README](../../mycelium-blackboard/examples/README.md) walks the microgrid.
- **Runbook:** [companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool).
- **Design:** [`plans/mycelium-blackboard.md`](../plans/mycelium-blackboard.md).
- **Code:** [`mycelium-blackboard/src/`](../../mycelium-blackboard/src/lib.rs) (`lib.rs`, `store.rs`, `wal.rs`, `http.rs`).
