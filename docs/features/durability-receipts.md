# Durability receipts

## What it is

A [receipt](../guide/00-concepts.md#receipt) says what a write's acknowledgement proves, by rung, and nothing above
it: local application, local sync, replica sync, and destination commit, which only a destination can give. Every
receipt is keyed by a caller-minted [`operation_id`](../guide/00-concepts.md#operation-id), stable across retries,
with an attempt id per try; a retry through a retry verb with different content under the same id is a conflict. A timeout answers
[`DeliveryUnknown`](../guide/00-concepts.md#delivery-unknown) — the rungs reached so far, then unknown, never a
negative. Local sync has four states, and [`Buffered`](../guide/00-concepts.md#buffered) — survives a process crash,
lost to a power failure — is a different claim from on-disk, not a weaker one.

## When to use it, and when not

- **Use it when** a caller must know whether a write is on this node's disk, or on other nodes' disks, before it
  acts on it — rather than reading a `bool` that says the write was queued.
- **Not when** you need to know a business effect happened: that is rung 4, a [destination commit](destination-commit.md).

## Turn it on

This feature is turned on in code: the receipt methods are on every node's KV handle; what they can establish
depends on [persistence](persistence.md) and its sync mode.

- [`set_with_receipt`](../../mycelium-core/src/kv_handle.rs): the write and its [`WriteReceipt`](../../mycelium-core/src/receipt.rs),
  whose [`LocalDurability`](../../mycelium-core/src/receipt.rs) is the rung-2 state.
- [`set_requiring_sync`](../../mycelium-core/src/kv_handle.rs): the one write the substrate refuses unless it is
  synced.
- Retrying: [`retry_with_receipt`](../../mycelium-core/src/kv_handle.rs) and
  [`retry_requiring_sync`](../../mycelium-core/src/kv_handle.rs) reuse the prior receipt's stamp, so the retry ranks
  as the same operation, and refuse different content under its id as a `Conflict`;
  [`prepare_write`](../../mycelium-core/src/kv_handle.rs) then [`commit_prepared`](../../mycelium-core/src/kv_handle.rs)
  (or `commit_prepared_requiring_sync`) mint the identity before anything is sent, so it survives the caller's own
  restart if the caller persists the prepared token. Calling `set_with_receipt`
  again is not a retry: it ticks a fresh stamp and the write ranks differently.
- [`set_with_replica_sync`](../../src/agent/kv_quorum_ext.rs): rung 3 — which peers have persisted the record,
  with the ones that did not answer kept apart as missing.
- [`persistence`](../reference/configuration.md#persistence): without it rung 2 is `NotConfigured`.
- `a_receipt_names_its_rung_and_nothing_above_it`, in [`mycelium-core/src/receipt.rs`](../../mycelium-core/src/receipt.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`receipt_ladder`](../../examples/README.md#the-capability-matrix): one write climbing the rungs, each receipt saying
what it proves, run in CI.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: rung 2's meaning under each sync mode is
  [`persist.sync_mode`](../reference/guarantee-catalogue.md#persist.sync_mode), on the [persistence](persistence.md) page, and the regression floor pins today's `bool`s and `persisted`.
- **Not built:** a receipt never proves that a lower rung implies a higher one, that an operation did not happen,
  that a silent peer refused, or that the business effect landed
  ([guide 18 § What a receipt never proves](../guide/18-contracts-and-receipts.md#what-a-receipt-never-proves)).
  Rung 3 since row R8 of the [external review plan](../plans/external-review-2026-10-31.md) counts a record a peer
  has applied but not yet handed to its WAL as not persisted; what R8 left is no per-record tracking (a busy peer
  answers `Failed` for any record while any inbound record is in flight, and is asked again) and the same
  apply-to-append window for a node's own local writes.
- **Evidence:** the ledger row
  [an acknowledgement names its rung and nothing above it](../operations/what-is-proven.md#ack-names-its-rung).

## Go deeper

- **Guide:** [18, contracts and receipts](../guide/18-contracts-and-receipts.md).
- **Runbook:** [deployment § Choosing a sync mode with the receipt contract in hand](../operations/deployment.md#choosing-a-sync-mode-with-the-receipt-contract-in-hand).
- **Design:** [`contracts-receipts.md`](../design/contracts-receipts.md).
- **Code:** [`mycelium-core/src/receipt.rs`](../../mycelium-core/src/receipt.rs) and
  [`mycelium-core/src/kv_handle.rs`](../../mycelium-core/src/kv_handle.rs);
  [`src/agent/replica_sync.rs`](../../src/agent/replica_sync.rs) for rung 3.
