# Log overlay

## What it is

An append-only log per named stream, kept in the [gossip KV](gossip-kv.md): each entry is a key
`log/{stream}/{hlc:016x}/{node}`, so it replicates, persists and is repaired like any other key, and a stream is
read in `(hlc, key)` order — the stamp first, then the writer's node id, so two nodes appending in one millisecond
both keep their entry and every node orders them the same way. A stream is read three ways: a snapshot scan, a
live subscription, and a group consumer that keeps a shared position.

## When to use it, and when not

- **Use it when** a fleet needs an ordered record many nodes append to and read — events, a reasoning trace,
  proposals — and every reader may see it eventually, in an order every node agrees on.
- **Not when** each item must be taken by exactly one of many workers: a group consumer is single-active, and a
  work queue is the [tuple space](tuple-space.md)'s job (the trap is named in the
  [runtime invariants](../wiki/dev/architecture/runtime-invariants.md)).

## Turn it on

This feature is turned on in code: every node has it, from `agent.kv()`.

- `append`, `scan_log`, `compact_log`, `subscribe_log` and `subscribe_log_group` on
  [`KvHandle`](../../mycelium-core/src/kv_handle.rs); the live subscription's cursor is
  [`LiveCursor`](../../mycelium-core/src/log_cursor.rs), the group's position
  [`LogPosition`](../../mycelium-core/src/log_cursor.rs).
- Over HTTP, `/gateway/overlay/log/append`, `/log/scan`, `/log/compact`, `/log/subscribe` (SSE) and
  `/log/group/subscribe` (SSE, a consensus claim) on the [HTTP gateway](http-gateway.md)
  ([`src/agent/http/overlay.rs`](../../src/agent/http/overlay.rs)); the SDKs' `append`, `scan_log` / `scanLog`,
  `subscribe_log` / `subscribeLog` and `subscribe_log_group` / `subscribeLogGroup` call them. A stream under `cn/`,
  `wiki/` or `reason/` belongs to a component and is refused at every HTTP route and SDK verb; the in-process
  `KvHandle` methods do not refuse it.
- `test_subscribe_log_receives_live_append`, in [`src/agent/kv_handle_tests.rs`](../../src/agent/kv_handle_tests.rs):
  an entry appended after the subscription opened is delivered.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`three_node_demo`](../../examples/README.md#the-capability-matrix), whose `overlay` node serves the log routes to the
Python SDK and to the overlay scenarios under `tests/overlay/` (S11 the group consumer's failover, S13 a shared
reasoning log).

## What it guarantees, and what it does not

- **A live subscription delivers every entry at or after `since` once** (engineering review ER8). The backlog and
  each later batch are in `(hlc, key)` order; an entry that arrives late — a peer's append at the same stamp, or
  one stamped below the last delivered — is delivered when it arrives, so across batches the order is arrival
  order and a stamp can be lower than the one before. `since` is a floor, not a resume token
  ([language bridges § a dropped stream](../guide/10-language-bridges.md)). The subscription ends when its
  receiver is dropped, its HTTP client disconnects, or the node shuts down.
- **A live subscription's memory** is one key suffix per live entry of the stream at or after `since`;
  `compact_log` shrinks it. Nothing caps it.
- **A group consumer** delivers each entry after its persisted `(hlc, key)` position, recording the position as the
  entry is handed on; in process it is best-effort (an LWW claim), at the gateway single-active (a consensus claim)
  ([guide 04 § consumer groups](../guide/04-consensus.md)).
- **Not built:** a group consumer does not deliver an entry that replicates after its position has passed it
  ([the ledger row](../operations/what-is-proven.md#log-group-skips-a-late-row)); no SSE log route sends an event
  `id:` or reads `Last-Event-ID`, so a dropped stream is re-opened by the client; a log entry is unsigned — any
  admitted member can write or tombstone one, as for any key of the [gossip KV](gossip-kv.md).

## Go deeper

- **Guide:** [04, the ordered durable log](../guide/04-consensus.md) and
  [10, language bridges](../guide/10-language-bridges.md).
- **Design:** no design record; the [runtime invariants](../wiki/dev/architecture/runtime-invariants.md) hold the
  group consumer's contract, and the module doc of
  [`mycelium-core/src/log_cursor.rs`](../../mycelium-core/src/log_cursor.rs) the two cursors.
- **Code:** [`mycelium-core/src/kv_handle.rs`](../../mycelium-core/src/kv_handle.rs),
  [`mycelium-core/src/log_cursor.rs`](../../mycelium-core/src/log_cursor.rs),
  [`src/agent/http/overlay.rs`](../../src/agent/http/overlay.rs).
