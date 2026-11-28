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
- **Not when** facts must expire: a fact lives until it is claimed and acked, so an undrained board grows without bound
  unless `BoardConfig::high_watermark` is set (by default it is not), which then refuses a `post` as `Backpressure`;
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
  raw route's `blackboard.{ns}.{op}` kind needs the family scope. Scopes are checked only where the node is built with
  `compliance`: the crate depends on `mycelium` without default features, so the scope wall is your build's, and
  without it any valid bearer reaches the routes ([what-is-proven](../operations/what-is-proven.md#companion-s-rpc-kind-needs-family)).
- `high_watermark` (default none) refuses a `post` above that many available facts, counted in `BoardStats::rejected`;
  `claim_timeout_secs` (default 300) re-queues an in-flight claim; neither is eviction
  ([companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool)).
- `two_claims_over_one_finite_fact_exactly_one_wins`, in [`mycelium-blackboard/src/store.rs`](../../mycelium-blackboard/src/store.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`microgrid`](../../examples/README.md#the-capability-matrix): a community microgrid's agents post and claim energy facts;
CI runs it, and [`microgrid_viz`](../../examples/README.md#the-capability-matrix) is the browser view. That an
in-flight claim survives a primary's failover is `inflight_claim_survives_primary_failover`, in
[`mycelium-blackboard/tests/board_failover.rs`](../../mycelium-blackboard/tests/board_failover.rs); that a completed
fact stays completed, and its id unused, across one is `a_returning_mirror_retires_facts_completed_during_its_absence`
and `a_new_mirror_preserves_the_highest_allocated_fact_id_on_promotion`, in the same file.

## What it guarantees, and what it does not

- No guarantee or rule id of its own; a claim door can hold work under the [exposure policy](exposure-policy.md).
- **Not built:** exactly-once processing — one claimer wins, but a re-queued claim can be processed again, so the effect
  should be idempotent ([destination commit](destination-commit.md)); expiry or eviction of facts; durability of the
  last ~1 s at a clean shutdown, which does not fsync the WAL ([companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool)).
- **A fact's id is not reissued, under one assumption** (ER3): a board with no id history (transient, or a missing or
  empty WAL) starts its ids at the wall clock (ms × 1024), a WAL with history at its highest id + 1. **The limit**, as
  for the [tuple space](tuple-space.md): fresh starts within *N*/1024 ms of each other can overlap, the in-process bump
  is 1 024 per board, and *N* boards created in one process put the last base about *N* ms ahead of the clock
  ([the ledger row](../operations/what-is-proven.md#fresh-id-space-not-reissued)).
- **A failover does not bring back, or reuse, what was completed** (ER6). The primary remembers the ids it acked (up
  to 100 000 in memory, the newest 16 384 through a compaction and a reopen) and sends them, with the id it issues
  next, on the snapshot a mirror syncs from. The mirror discards a fact the primary names acked — one completed while
  the mirror was away, restored from its own WAL — refuses a replicated `Post` of an acked id (counted,
  `Blackboard::mirror_refused_acked`), and fences its ids past the primary's next one, so a promoted mirror never
  issues an id the old primary already did. **Positive evidence, never absence:** a fact the snapshot lacks and does
  not name acked is kept and counted (`Blackboard::mirror_unvouched`). A running mirror compares a digest of its facts
  with the primary's every three `cap_refresh` and re-syncs after four consecutive mismatched probes — within about two minutes of the first mismatch at the default `cap_refresh` of 10 s (4 probes × 3 × `cap_refresh`, the first mismatch seen up to one probe interval after the loss)
  (`Blackboard::mirror_digest_resyncs`), so a lost replicated `Post` or `Ack` is repaired without a restart, under
  steady writes too. Replication still in flight at probe time also mismatches, and no sync cures that: a sync that
  retired nothing and refetched nothing issued before the mismatches began doubles the threshold, up to 32 probes;
  a sync that repairs something resets it to 4. **The bounds:** a lost `Post` or `Ack` is repaired within 4 probes
  (about 2 minutes at the default `cap_refresh`) when the threshold is at rest, and within 32 (about 16 minutes)
  after a back-off; on a board whose replication is always behind the probe, the steady cost is one needless
  whole-board snapshot per mirror every 32 probes (about 16 minutes). An `Ack` in flight during a sync counts as a
  repair and resets the threshold. An id the primary holds live that the mirror refuses as acked (its `Ack` was lost before an old primary died)
  is counted in `mirror_refused_acked` at each full sync and folded into the mirror's digest, so it does not start
  syncs. Against a primary older than this probe, whose answer is the whole board, a mirror probes once in ten.
  **Frame headroom:** the trailer is up to about 1.2 MB (100 000 runs of 12 bytes) of the snapshot's 10 MiB frame;
  a mirror refuses a trailer that names more ids than a board remembers, or a next id above 2^53, and then applies
  nothing from that snapshot. **A healed split-brain:** two primaries of one namespace that issued the same ids
  (ER3-F1: fresh starts in one millisecond) can retire each other's facts — the other primary's trailer names an id
  acked that is this mirror's own live fact. **Not built, and older than ER6:** a mirror does not check that a
  `replicate` or a snapshot answer comes from the primary — any member that reaches the RPC kinds can forge one. **Not
  built:** an ack older than the tombstone memory (forgotten, so such a fact on a mirror that was away longer is kept
  as unvouched and can be served again after a promotion); an ack lost within the last probe interval, which a
  promotion in that window does not see; and unvouched facts on a returning node, kept by design (at-least-once)
  ([the ledger row](../operations/what-is-proven.md#board-failover-retires-acked)).
- **Evidence:** the ledger row
  [a failed WAL append is never followed by an acknowledged one](../operations/what-is-proven.md#failed-wal-append-never-followed-acknowledged),
  both companions' WALs included.

## Go deeper

- **Guide:** no guide chapter; the [examples README](../../mycelium-blackboard/examples/README.md) walks the microgrid.
- **Runbook:** [companions § mycelium-blackboard](../operations/companions.md#mycelium-blackboard--shared-fact-pool).
- **Design:** [`plans/mycelium-blackboard.md`](../plans/mycelium-blackboard.md).
- **Code:** [`mycelium-blackboard/src/`](../../mycelium-blackboard/src/lib.rs) (`lib.rs`, `store.rs`, `wal.rs`, `http.rs`).
