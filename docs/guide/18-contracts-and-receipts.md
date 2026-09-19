# 18 · Contracts & Receipts

Chapters 01–04 showed you how to write to the mesh. This one is about what the answer *means*.

It is grounded in [`mycelium-core/src/receipt.rs`](../../mycelium-core/src/receipt.rs), which is the
vocabulary in code, and the design record
[`docs/design/contracts-receipts.md`](../design/contracts-receipts.md). The runnable demonstration is
[`examples/receipt_ladder.rs`](../../examples/receipt_ladder.rs).

Two questions this chapter answers:

- **"My write returned `true`. What did I just learn?"** That it was queued for gossip. Not that it
  reached disk, not that any peer has it, not that it will survive a restart. Worse, `false` does not
  mean the write did not happen: the local store may have been updated with a full gossip channel and
  anti-entropy still to come. The bool names no rung, which is exactly why the receipt verbs exist.
- **"The call timed out. Did it happen?"** Nobody knows, and the API says so rather than guessing.
  A timeout comes back as `DeliveryUnknown`, never as a failure, because a failure is a claim about
  the world that a timeout does not support.

---

## The honest core: a receipt names its rung and nothing above it

There are four rungs. They are not degrees of confidence on one scale. They are different facts, and
**nothing infers a higher rung from a lower one**.

| Rung | Receipt | Establishes |
|---|---|---|
| 1 | `LocalApplication` | the operation was applied to *this node's* store, or superseded under last-writer-wins |
| 2 | `LocalDurability` | *this exact operation* crossed this node's persistence barrier |
| 3 | `ReplicaSync` | named, distinct peers persisted *this exact operation* |
| 4 | `DestinationCommit` | a destination committed the business change and its deduplication result in one transaction |

Applied is not on-disk. On-disk on one node is not replica sync. Three replicas are not a
destination commit. A caller that needs a higher rung asks for it, and is **refused** when the node
cannot establish it, rather than handed a receipt that quietly claims less than it appears to.

---

## Rung 2 has four states, and one of them is a lesson

```rust
pub enum LocalDurability {
    OnDisk,          // written and synced. The one state that establishes durability
    Buffered,        // the log took it; the bytes are in the OS page cache
    Failed(String),  // durability was not established. NOT "the record is absent"
    NotConfigured,   // no persistence here, so nothing was promised and nothing is claimed
}
```

Read `Buffered` precisely. It means the record **survives a process crash and is lost to a power
failure**, until the next sync or snapshot. It is not a softer way of saying `OnDisk`.

That variant was missing from the contract's first draft, which assumed every write either reached
disk or failed. That is true only under a synchronous sync mode. A receipt reporting `OnDisk` in the
buffered case would have claimed a durability the node never established. The gap was found while
implementing the contract, and the fix was a new state rather than a rounded-off one.

Read `Failed` precisely too. The log writes the record before it syncs, so after a failure the bytes
may or may not be on disk and a later replay may restore them. Nothing is promised. Nothing is
denied either.

---

## Identity comes before any of it

Every operation carries a caller-minted `OperationId`, stable across retries, worker replacement and
gateway hops. Each delivery attempt carries an `AttemptId`.

```rust
use mycelium::{OperationId, AttemptId};

// Supply your own when retries must be recognisable as retries.
let op = OperationId::new("depot-plan-2026-09-19");
let attempt = AttemptId::fresh(&op);

// Or mint one when you have none. Unique, but NOT stable across processes.
let op = OperationId::generate(&node_id);
```

**An operation identity is correlation, never authority.** A client supplies one precisely so its
retries can be recognised. Who the caller *is* comes only from the verified caller context, which is
chapter 09's material. Nothing in the system mints a second identity scheme.

The rule that makes identity load-bearing: **the same operation identity carrying different content
is a `Conflict`**, refused rather than applied. An identity that meant two things would make every
receipt about it ambiguous.

---

## Writing, four ways

```rust
// 1. No receipt. The bool says the update was QUEUED FOR GOSSIP — and `false` is
//    ambiguous: the local store may still have been updated (a full channel, with
//    anti-entropy to follow) or the write may have been rejected outright as oversized.
agent.kv().set("depot/plan", bytes.clone());

// 2. A receipt, reporting whatever rung was actually reached.
let receipt = agent.kv().set_with_receipt(&op, "depot/plan", bytes.clone()).await?;
match receipt.local_durability {
    LocalDurability::OnDisk => { /* it is on disk */ }
    LocalDurability::Buffered => { /* survives a crash, not a power cut */ }
    LocalDurability::NotConfigured => { /* this node promised nothing */ }
    LocalDurability::Failed(ref why) => { /* unknown, not absent */ }
    _ => {}
}

// 3. Demand rung 2. Refused if the node cannot establish it.
match agent.kv().set_requiring_sync(&op, "depot/plan", bytes.clone()).await {
    Ok(receipt) => assert!(receipt.local_durability.is_durable()),
    Err(ReceiptError::DurabilityNotEstablished { persistence_configured, reason }) => {
        // The flag distinguishes "this node was never able to" from "the attempt failed".
    }
    Err(other) => return Err(other),
}

// 4. Ask rung 3: which peers hold it. The timeout is the caller's, and peers that
//    do not answer inside it are reported unknown rather than missing.
let receipt = agent.set_with_replica_sync("depot/plan", bytes, Duration::from_secs(2)).await?;
```

Note the shapes. The receipt verbs are **async** and take the operation identity **first**. The
replica-sync form lives on the agent rather than the key-value handle, and it mints its own operation
identity, so its third argument is a timeout and not a replica count.

The fourth form is the one to think carefully about. `ReplicaSync` carries **`persisted_by` and
`missing` as two separate lists**, because a peer that did not answer is not a peer that refused.
A silent peer is unknown, and that is why the call returns a receipt rather than a count.

### The required-sync write is the one place the substrate prevents

Everywhere else Mycelium detects rather than prevents. `set_requiring_sync` is the exception, and it
is admissible only because the caller asked for it as a contract: when durability is not established,
**nothing was applied and nothing was gossiped**, so no reader, subscriber or peer saw the value as a
result of that call.

Read the limit of that carefully, because the first cut of the design claimed more. It does **not**
promise the operation can never become visible here. The write-ahead log writes a record before it
syncs, so a failed sync may leave bytes on disk that a later replay restores. The value can appear
after a restart even though the call reported failure. The recovery outcome is unknown.

### Retrying

```rust
// Right: reuses the operation's HLC stamp.
let receipt = agent.kv().retry_with_receipt(&prior, "depot/plan", bytes).await?;
```

**Never retry by calling `set_with_receipt` again.** That ticks a fresh hybrid logical clock stamp,
so the same logical operation ranks differently under last-writer-wins on each attempt. The retry
verb reuses the original stamp, which is what makes a retry the same operation rather than a
competing one.

---

## When the answer is unknown

```rust
Err(ReceiptError::DeliveryUnknown { established, awaiting }) => {
    // `established` is the receipt as far as it got before the answer stopped coming.
    // `awaiting` names what the caller was still waiting for.
}
```

**No verb in this vocabulary returns "nothing happened",** because no verb can know that. The error
type has no such variant, deliberately. This is the single most load-bearing convention in the
contract, and it is why a blackholed peer is reported `DeliveryUnknown` within a bound rather than
hanging. A call that waits forever has quietly turned an unknown into a hang, which is the same lie
told more slowly.

The consensus surface carries the same rule as `CommitError::DeliveryUnknown`, alongside `Superseded`
and `TopologyUnsatisfied`.

The one error that *is* a clean negative is `ReceiptError::Rejected`, for a write refused before
anything was applied, such as an oversized key and value. It is a statement about the request, not
about the world.

---

## Run the ladder

```bash
cargo run --example receipt_ladder
```

The same write is made four ways against real nodes, and comes back with four different claims. The
example then shuts a peer down and writes again, so you watch a peer that cannot answer reported as
**unknown** rather than failed, and it reopens a persisted node's data directory to read back what
replayed.

**What the example does not demonstrate,** and says so in its own header: neither failure mode that
`Buffered` declines to survive. Its step 8 is a clean shutdown and a reopen, so a buffered record
surviving it shows the write-ahead log replaying and nothing about durability under failure. A kill
and a power cut are both outside one cooperating process. It does not show a peer crashing mid-write,
nor anything about a network partition.

That paragraph is the chapter's real lesson repeated one level up: **say which rung your evidence
reaches.**

---

## The same vocabulary at the gateway and in the SDKs

The receipt states have one set of stable names, shared by the gateway JSON field
`local_durability` and both software development kits:

```
on_disk · buffered · not_configured · failed
```

These strings are pinned by a test, because renaming one is a wire change for every kit. If you are
building a client, match on these rather than on prose.

---

## What a receipt never proves

- **That a lower rung implies a higher one.** This is the whole point, and it is worth re-reading the
  rung table if only one thing from this chapter survives.
- **That an operation did not happen.** Only that a rung was not *established*.
- **That a peer refused.** Silence is unknown. `ReplicaSync` keeps `missing` separate from
  `persisted_by` for exactly this reason.
- **That the business effect landed.** That is rung 4, `DestinationCommit`, which also carries a
  `DedupOutcome` of `Fresh` or `Replayed` so a destination can tell a first delivery from a retry.

---

## Where to go next

| You want | Read |
|---|---|
| the design rationale and the regression floor that pins today's behaviour | [`docs/design/contracts-receipts.md`](../design/contracts-receipts.md) |
| the vocabulary in code, with every variant's doc comment | [`mycelium-core/src/receipt.rs`](../../mycelium-core/src/receipt.rs) |
| what the error types mean across the whole library | [error-handling.md](error-handling.md) |
| who the caller is, as distinct from which operation this is | [09-security.md](09-security.md) |
| replaying a recorded run to see the same receipts again | the [`mycelium-sim`](../../mycelium-sim/) crate and `cargo run -p mycelium-sim --example replay_a_bundle` |
