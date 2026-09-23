# dev/architecture/contracts — what an acknowledgement proves

↑ [architecture/](architecture.md) · record
[`docs/design/contracts-receipts.md`](../../../design/contracts-receipts.md) · guide
[18 — contracts & receipts](../../../guide/18-contracts-and-receipts.md) · the destination rung's
crate: [companions/effects.md](../companions/effects.md)

Canon: `mycelium-core/src/receipt.rs` (the vocabulary), `mycelium-core/src/kv_handle.rs` (the
verbs), `mycelium-core/src/ops.rs` (`kv_set_with_receipt`, `kv_set_requiring_sync`),
`src/agent/kv_quorum_ext.rs` (`set_with_replica_sync`), `src/agent/kv_quorum.rs` (the tracker and
its floor tests).

## Why this page exists

The substrate's acknowledgements used to be `bool`s whose meaning lived in folklore. The evidence
that this was not a style question is in the record's §1: the snapshot/WAL race fixed in v2.4.2 was
deterministic on a single-thread runtime, shipped in every release with persistence, and invisible
to ~800 tests — it was found by an external probe. `set_with_min_acks` counting *any newer* peer
write as an acknowledgement was a second instance of the same family, still live at the time.

**The common thread is not a bug. It is that mechanisms had contracts nobody had written down.**
This page is the shape of the fix as it exists in code; the argument for it is the record.

## The four rungs, and the rule about them

| Rung | Type | Establishes | Asked for by |
|---|---|---|---|
| 1 | `LocalApplication` | applied to *this node's* store, or superseded under LWW | `set_with_receipt` |
| 2 | `LocalDurability` | *this exact operation* crossed this node's persistence barrier | the receipt's field; demanded by `set_requiring_sync` |
| 3 | replica sync | named, distinct **peers** hold this exact operation on disk | `GossipAgent::set_with_replica_sync` |
| 4 | destination commit | a destination committed the business change and its dedup result in one transaction | `mycelium-effects` — never the substrate |

**Nothing infers a higher rung from a lower one**, and the ordering is not a confidence scale —
they are different facts. Applied is not on-disk; on-disk here is not on-disk there; three replicas
are not a destination commit. A caller that needs a higher rung **asks**, and is refused when this
node cannot establish it, rather than handed a receipt that quietly claims less than it looks like.

Rung 4 is the one the substrate structurally cannot provide, and saying so is the point: only the
resource that performed an effect can report whether it happened there.

## `LocalDurability` has four states and one lesson

`OnDisk` · `Buffered` · `Failed(String)` · `NotConfigured` (`mycelium-core/src/receipt.rs`).

- **`Buffered` was missing from the contract's first draft**, which assumed a write either reached
  disk or failed — true only under `SyncMode::Flush` or an explicit `append_sync`. A receipt saying
  `OnDisk` in the buffered case would have claimed a durability the node never established. The gap
  was found *while implementing the contract*, which is the argument for writing contracts down in
  the first place.
- **`Failed` is not "the record is absent".** The log writes before it syncs, so the bytes may or
  may not be there and a replay may restore them. Nothing is promised in either direction.
- **`NotConfigured` is the state the old `persisted: true` hid**: no persistence here, so nothing
  was promised and nothing is claimed. Two very different operational facts had one value.

## A timeout is `DeliveryUnknown`, never a negative

This is a hot invariant, and it is the rule most easily eroded by a convenience. A peer that does
not answer is **unknown** — unreachable, mid-restart, or already holding a newer value all look
identical from here. The write was applied and gossiped regardless and is **not rolled back**, so
retrying on the strength of a timeout is retrying something that already happened.

The same rule crosses a domain boundary as `delivery: "unknown"` on the federation routes, and both
SDKs raise a *distinct exception type* for it so it cannot be caught alongside ordinary failures
([security](../security.md) → federated domains).

## The floor is how a meaning gets changed

`src/agent/kv_quorum.rs::floor_tests` and the golden on-disk fixtures under
`tests/fixtures/persistence/` (replayed by
`persistence.rs::golden_fixture_replays_every_released_on_disk_format`) are **executable pins of
what today's acknowledgements mean**. The discipline they exist for is visible in their own
history: PR 1 pinned the `>=` propagation rule, and **PR 4a changed that meaning by changing the
pin** — `observe_counts_only_this_exact_payload` replaced
`floor_observe_counts_any_update_at_or_after_write_ts`, in the open, in a diff.

So the rule for this codebase is: *a PR changes an ack's meaning by changing a pin.* A change that
alters what a caller may conclude and leaves the floor untouched has not been reviewed — it has
been missed.

**A format change adds a fixture directory; it never edits one.** Every released on-disk format
must keep replaying, which is what makes the second pin a compatibility claim rather than a
snapshot of today.

## Where the contract shows up outside core

- **The gateway** renders the receipt beside the old bool (`src/agent/http.rs::commit_json`) —
  `persisted` is exactly what it always was, and `local_durability` is *added* beside it, never
  derived back into it.
- **Both SDKs** carry the vocabulary (`CommitResult.local_durability` / `localDurability`) and now
  explain it, not only carry it (their READMEs, §12.2).
- **`set_with_min_acks` is `#[deprecated]`** and cannot succeed on today's substrate; the verb that
  does what its name says is `GossipAgent::set_with_replica_sync`, which **asks** each peer whether
  it holds the exact operation rather than watching gossip go by.
