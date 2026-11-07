# Destination commit

## What it is

[Destination commit](../guide/00-concepts.md#destination-commit) is the fourth receipt rung: one effect, applied once,
at a resource you own. The substrate never provides it — only a destination can say whether an effect happened there
— so the `mycelium-effects` crate supplies a reference destination over SQLite whose dedup row, keyed by the caller's
`operation_id`, commits in the same transaction as the business change. A retry with the same content is `Replayed`
and applies nothing; different content under the same id is a `Conflict`; applied through `apply_within`, a deadline
passed is `DeliveryUnknown`, never "nothing happened".

## When to use it, and when not

- **Use it when** a retried, re-delivered or recovered call must change a ledger, a stock count or a booking exactly
  once.
- **Not when** the destination is someone else's service you cannot put a dedup row in: that is the
  [side-effect contract](side-effects.md), which records and reconciles what it cannot make exactly-once.

## Turn it on

This feature is turned on in code, from the `mycelium-effects` crate. Its features: `tuple-space` for a consumer that
commits each item's effect before acknowledging it; `envelope` for building a composed effect from the gateway's
action envelope (it pulls in `gateway` and `tls`); `metrics` for a Prometheus counter of refusals; and `sim`, the replay
seams for the external-effect reconciler, which belongs to the [side-effect contract](side-effects.md).

- [`EffectDestination`](../../mycelium-effects/src/lib.rs): the trait — `apply`, and `apply_composed`, which refuses
  an effect whose attribution or authority leg fails, at the destination.
- [`SqliteDestination`](../../mycelium-effects/src/sqlite.rs): the reference destination; `open` it on a file.
- [`apply_within`](../../mycelium-effects/src/lib.rs): apply with a deadline, an overrun answered `DeliveryUnknown`;
  `apply` alone waits for the destination.
- [`TupleConsumer`](../../mycelium-effects/src/tuple_consumer.rs): under `tuple-space`, a worker that dies between the
  effect and the acknowledgement is re-delivered, and the second apply is `Replayed`.
- [`Counting`](../../mycelium-effects/src/counting.rs): wraps a destination and counts its refusals by kind.
- `a_retry_with_different_content_is_a_conflict_and_applies_nothing`, in
  [`mycelium-effects/src/sqlite.rs`](../../mycelium-effects/src/sqlite.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`destination_commit`](../../examples/README.md#the-capability-matrix): the reference destination applying, replaying
and refusing, run in CI. [`counting_destination`](../../examples/README.md#the-capability-matrix) shows the refusal
counter, and [`composed_commit`](../../examples/README.md#the-capability-matrix) the composed effect enforced at the
destination.

## What it guarantees, and what it does not

- No guarantee or rule id: the receipt is the destination's, and nothing in the
  [guarantee catalogue](../reference/guarantee-catalogue.md) can resolve it for a node.
- **Not built:** exactly-once at a destination that is not transactional with its dedup row; the composed effect's
  domain leg is carried, not re-verified ([`composed-effect.md`](../design/composed-effect.md) §9); `rusqlite` is
  bundled and quarantined to this crate.
- **Evidence:** the ledger rows
  [a composed effect is refused at the destination](../operations/what-is-proven.md#composed-effect-refused-at-destination)
  and [an acknowledgement names its rung](../operations/what-is-proven.md#ack-names-its-rung).

## Go deeper

- **Guide:** [18, contracts and receipts](../guide/18-contracts-and-receipts.md).
- **Runbook:** [companions § mycelium-effects](../operations/companions.md#mycelium-effects--the-transactional-destination).
- **Design:** [`contracts-receipts.md`](../design/contracts-receipts.md), [`exactly-once-effect.md`](../design/exactly-once-effect.md)
  and [`composed-effect.md`](../design/composed-effect.md).
- **Code:** [`mycelium-effects/src/`](../../mycelium-effects/src/lib.rs).
