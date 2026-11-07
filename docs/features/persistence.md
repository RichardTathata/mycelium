# Persistence

## What it is

With a `[persistence]` table a node keeps its KV store and its consensus acceptor memory on disk, as a write-ahead
log and a periodic snapshot, and a restart recovers both. A write is applied to the store and then handed to the
log, never the reverse. What a write's acknowledgement means is the sync mode: under `Flush` the record is
`fdatasync`ed first, while the other modes buffer. Consensus commits and leases are synced in every mode. A node
refuses to start over persisted state it cannot read, and a failed append stops the writer until a snapshot has
repaired the log, so an acknowledged record never sits behind a torn one.

## When to use it, and when not

- **Use it when** a node's data or its votes must survive a restart: an electorate member needs it, and the secure
  profile requires `Flush`.
- **Not when** you need replication to survive the loss of the node's disk: persistence is local, and other nodes
  hold copies only as gossip and anti-entropy deliver them ([durability receipts](durability-receipts.md)).

## Turn it on

Set the table. The two-domain scaffold's [`kitchen-gw.toml`](../../templates/two-domains/nodes/kitchen-gw.toml), which
CI's two-domains job starts, sets `[persistence]` with `sync_mode = "Flush"`.

- [`persistence`](../reference/configuration.md#persistence): `base_path`, under which the node writes
  `{node_id}/kv/{wal.bin,snapshot.bin}`; `sync_mode` (`Flush`, `Async` the default, or `Os`, which the writer treats
  as `Async`); `snapshot_wal_threshold` and `snapshot_interval_secs`, which bound replay time; `on_unreadable`
  (`refuse`, the default, or `quarantine`, which moves unreadable files aside and starts).
- A directory that cannot be created, or one another process owns, refuses the start by name.
- `a_torn_wal_tail_is_repaired_at_start_before_the_first_acknowledged_append`, in
  [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for it;
[`receipt_ladder`](../../examples/README.md#the-capability-matrix) shows what each sync rung means to a write. The
end-to-end run is a test in [`mycelium-core/src/persistence.rs`](../../mycelium-core/src/persistence.rs):
`a_failed_append_poisons_the_writer_until_a_snapshot_truncates_the_torn_tail`.

## What it guarantees, and what it does not

- **Guarantees:** [`persist.configured`](../reference/guarantee-catalogue.md#persist.configured) — a restart recovers
  the KV and acceptor memory; [`persist.sync_mode`](../reference/guarantee-catalogue.md#persist.sync_mode) — a WAL
  acknowledgement is a durability claim, only under `Flush`;
  [`persist.unreadable_refused`](../reference/guarantee-catalogue.md#persist.unreadable_refused) — no start over
  state the node could not read.
- **Not built:** directory fsync is assumed honoured (ext4, XFS, btrfs); on macOS `fsync` does not force the drive's
  write cache, so a laptop is development only; a plain `set` under `Flush` logs a writer failure rather than
  returning it, and a receipt is the way to ask ([deployment § Persistence modes](../operations/deployment.md#persistence-modes)).
- **Evidence:** the ledger rows
  [persisted state is repaired before the first acknowledged append](../operations/what-is-proven.md#persisted-state-repaired-before-first-acknowledged)
  and [a failed WAL append is never followed by an acknowledged one](../operations/what-is-proven.md#failed-wal-append-never-followed-acknowledged).

## Go deeper

- **Guide:** [18, contracts and receipts](../guide/18-contracts-and-receipts.md), for what an acknowledgement proves.
- **Runbook:** [deployment § Persistence modes](../operations/deployment.md#persistence-modes),
  [§ Choosing a sync mode](../operations/deployment.md#choosing-a-sync-mode-with-the-receipt-contract-in-hand) and
  [§ Backup and restore](../operations/deployment.md#backup--restore).
- **Design:** the persistence invariants in
  [runtime-invariants § Persistence](../wiki/dev/architecture/runtime-invariants.md#persistence-the-store-is-never-behind-the-wal-and-a-snapshot-never-discards-a-wal-record);
  the contract is [`contracts-receipts.md`](../design/contracts-receipts.md).
- **Code:** [`mycelium-core/src/persistence.rs`](../../mycelium-core/src/persistence.rs) (the WAL, snapshots, replay,
  repair).
