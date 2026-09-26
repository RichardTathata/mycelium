# 24 · Commitments — the Contract Net

Chapter 14 covers two ways work finds a worker: the **tuple space** routes by position, the
**blackboard** routes by content. This is the third. The contract net routes by **negotiation** — a
requirement is announced, participants offer, one is awarded.

It is grounded in [`mycelium-commitment`](../../mycelium-commitment/). The runnable demonstration is
[`redistribution_cn`](../../mycelium-commitment/examples/redistribution_cn.rs).

Two questions this chapter answers:

- **"When would I negotiate rather than just queue work?"** When the participants know something the
  announcer does not — their own capacity, cost or suitability — and you want that expressed as an
  offer rather than guessed at by a dispatcher.
- **"What stops two nodes both believing they won?"** The award is a **receipt-bearing operation**
  under a stable operation identity, and one requirement takes exactly one award.

---

## The honest core: an award is a receipt, not a write

```rust
pub struct Awarded { award: Award, receipt: WriteReceipt }
pub struct AwardedLinearizable { award: Award, commit: CommitReceipt }
```

**The award is never a key-value write alone.** It carries chapter 18's `WriteReceipt` under
`operation_id = cn/{requirement}/award`, so a retry of the award is recognisable as a retry, and its
durability is exactly what the receipt says — no more.

The linearizable form makes **two** statements rather than one: the cluster agreed, *and* this node's
own durability is whatever `commit.local_durability` reports. Those are different facts and the type
keeps them apart.

An award also names the offer it accepted, by that offer's position in the log. That is the
mechanical form of a rule worth stating plainly: **no component assigns another participant's
obligation.** An award points at something the participant volunteered.

---

## Five records, one mechanism each

| Record | Where it lives |
|---|---|
| `Announcement` | a key-value head, declarer-owned |
| `Offer` | appended to the requirement's offer log |
| `Award` | a key, written **with a receipt** |
| `Report` | appended to the report log |
| `Assessment` | appended to the assessment log, signed |

Time enters through the caller, so every decision is **pure in time** — which is what makes these
runs replayable under chapter 19.

---

## The award rule is pure, so the award can be checked

```rust
pub enum AwardRule { LowestParticipant }   // lowest participant id, ties on the earlier offer
```

`choose` is a pure function of the offers. **Every reader of the streams reaches the same
conclusion**, which is what lets an award be *checked* rather than trusted. A rule that consulted
anything outside the offer log would make the award an assertion.

### And a pure rule over forgeable inputs checks nothing

An offer names its participant in a **field**. Until 2026-09-23 nothing bound that field to anyone,
so a member able to append to the stream could post an offer naming somebody else — and because
`LowestParticipant` is deterministic, every reader checking the award against the offers would agree
the award was **correct**. The purity of the rule is what made the forgery invisible: the check
confirms the rule was applied, not that the inputs were real.

So an offer is signed when its participant holds a key, and a declarer that cares awards from the
verified set:

```rust
driver.offer_signed("pickup-9", "2 km", Some(&driver_key), now)?;   // the participant signs

let candidates = hub.offers_verified("pickup-9", |p| directory.get(p).copied());
let award = hub.plan_award_from("pickup-9", AwardRule::LowestParticipant, now, candidates)?;
```

`offers_verified` checks each offer against the key of the participant **the offer names**, so an
offer signed by its forger fails — which is the attack worth stopping, not the careless one. Passing
`offers()` instead gives you the old behaviour unchanged: provenance is the declarer's decision, not
the crate's policy. Awards sign symmetrically (`Award::signed`, `verify_award`), because an award
names a declarer too.

**What a verifying signature buys, precisely.** It proves the holder of that key made the record.
Whether the key belongs to the participant it names is the identity layer's question, and under the
default configuration (`require_identity_proofs` off) that layer is weaker than the signature looks
— see [09 · Security](09-security.md). And nothing stops the forged offer being *written*; it stops
it being *awarded*. That is refusal at the decision point, not prevention at the medium, which is
the substrate's standing posture.

---

## Refusals are visible states, not retries

```rust
pub enum CommitmentRefusal {
    NotAnnounced,
    NoOffers,
    AlreadyAwarded(Box<Award>),   // the enum is #[non_exhaustive] (2.13.0): match with a `_` arm that fails closed
    Receipt(ReceiptError),
    AwardUnknown { ballots_tried },
    NotCommitted(String),
    Mandate(MandateRefusal),
    MandateNotTheAcceptors { holder, acceptor },
    Encoding(String),
}
```

Three worth reading closely:

- **`NoOffers`** — *nobody offered before the deadline.* That is a **visible state, not a retry**. An
  empty market is information about the market.
- **`AlreadyAwarded`** returns the existing award. One requirement, one award; a second is refused
  and the first is **never overwritten**.
- **`AwardUnknown`** — the round reached no commit, so the award **may or may not** have committed
  elsewhere. Not *no award*. A retry resolves it as `AlreadyAwarded` or as a fresh commit. This is
  chapter 18's `DeliveryUnknown` rule, one layer up.

---

## Unknown is a first-class outcome here too

```rust
pub enum Outcome { Fulfilled, Failed(String), Unknown }
```

`Unknown` is *the participant cannot say* — a timeout, a lost acknowledgement. **The honest report.**
A contract net whose only outcomes were success and failure would force every participant to lie once
per ambiguous run.

---

## An unsigned record verifies as false

Anyone may assess, not only the declarer, and an assessment names its assessor. Verification returns
`false` for an unsigned assessment — **unproven, not forged**, and specifically not true-by-absence.
The same rule now holds for offers and awards; unsigned stays legal, because a single-tenant mesh
whose members are trusted equally has nothing to prove to itself, and what is never legal is reading
an unsigned record as proof.

That is the difference between "we have no proof this was assessed" and "this was assessed and the
proof failed", and a reader has to be able to tell them apart.

---

## The mandate join

The acceptor's own mandate is checked **before any write**. A stale holder's award is refused as
`Superseded { installed, presented }` — the resource has moved to a later epoch than the one the
mandate was minted under. See [21 · Mandates](21-mandates.md) for why that is not a conflict.

`MandateNotTheAcceptors` is separate: the mandate presented names somebody else as holder. Presenting
a valid mandate that is not yours is a different mistake from presenting your own expired one, and
the two refusals say so.

---

## Run it

```bash
cargo run -p mycelium-commitment --example redistribution_cn
```

Surplus food needs collecting; depots offer; one is awarded; it reports; the outcome is assessed.
This example is a gate in CI, not just a demonstration.

---

## Which coordination model?

| Model | Routes by | Reach for it when |
|---|---|---|
| **tuple space** (ch. 14) | position | work is uniform and a worker takes the next item |
| **blackboard** (ch. 14) | content | workers compete for facts matching a predicate |
| **contract net** (here) | negotiation | participants know their own cost or capacity and should say so |

The contract net costs a round trip and a deadline that the other two do not. Reach for it when the
offer carries information, and not merely to make a queue feel deliberate.

---

## What this does not establish

- **Not identity.** The companion does not mint participant identity, and `offers_verified` takes a
  **resolver** rather than reaching into the mesh precisely because a participant may be a person or
  a vehicle rather than a node. It checks a signature against a key you supply; where that key came
  from, and whether it really is that participant's, is the identity layer's question. Authority
  lives in the gateway's caller context and in mandates; the contract net records who said what, and
  now lets a declarer check that the saying was theirs.
- **Not exactly-once effect.** The award is durable to the rung its receipt names. Making the *work*
  exactly-once is the destination's job — chapter 18's rung 4.
- **Not fairness.** `LowestParticipant` is deterministic, which is a different property from fair. It
  is the tuple space's primary-election rule, chosen so the award can be checked.

---

## Where to go next

| You want | Read |
|---|---|
| what the award's receipt actually proves | [18 · Contracts & receipts](18-contracts-and-receipts.md) |
| why the acceptor's mandate is checked first | [21 · Mandates](21-mandates.md) |
| the other two coordination models | [14 · Patterns & pitfalls](14-patterns-and-pitfalls.md) |
| the companion's own docs | [`mycelium-commitment`](../../mycelium-commitment/) |
