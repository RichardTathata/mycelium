# 23 · Knowledge

Chapter 18 was about what an acknowledgement proves. This one is about what a *statement* proves, and
about a system that keeps disagreement instead of resolving it.

It is grounded in [`src/knowledge.rs`](../../src/knowledge.rs) and its three submodules. The design
record is [`docs/design/knowledge-layer.md`](../design/knowledge-layer.md).

Two questions this chapter answers:

- **"Two agents disagree. Which do I store?"** Both. Last-writer-wins would let the later statement
  erase the earlier one, and "the most recent thing anybody said" is not the same as "what is true".
- **"How do I stop an agent citing itself into authority?"** By splitting *recording* from *judging*,
  and by refusing to count a self-assessment.

---

## The honest core: judging is not recording

Four record kinds, and the split between the first three is the whole design:

| Kind | What it is |
|---|---|
| `Claim` | an issuer **asserts** something |
| `Observation` | an issuer **reports what it saw** |
| `Assessment` | an issuer **judges** a claim or observation — a judgement, with an author |
| `AcceptanceDecision` | a reader decides what to do about something |

**Only assessments count as evidence.** A claim is what someone said about themselves; an observation
is a report. Neither is a judgement, and the first two kinds are split from the third precisely so
that a self-issued claim cannot be counted as support for itself.

**And a self-assessment does not count either.** Without that, an agent could assert something, judge
its own assertion favourably, and clear a policy that requires supporting evidence.

The kind is part of the **signed bytes**, so an observation's signature can never authenticate an
assessment. That is the same domain-separation rule the federation objects use: reuse the code, never
the trust.

---

## Six link kinds, and no others

```rust
pub enum LinkKind {
    Supports, Challenges, DerivedFrom, Supersedes, Retracts, Adopts,
}
```

Two carry rules that are easy to miss and load-bearing:

- **`Challenges` is meant to cross issuers.** Disagreement is what the layer preserves, not a
  conflict to be cleaned up.
- **`DerivedFrom` is asserted explicitly and never inferred** from timestamp adjacency. Adjacency is
  an ordering, not a cause, and a system that guessed causation from ordering would build dependency
  chains nobody wrote down.
- **`Retracts` is same-issuer only.** You can withdraw your own statement. You cannot withdraw
  someone else's.

A `RecordId` is content-derived **and names its issuer**, which is what lets self-support be detected
without fetching anything.

---

## Four verdicts, and the pair people collapse

```rust
pub enum Verdict {
    Accepted { supporting, independent },
    Rejected { reasons },                    // never an empty vector
    InsufficientEvidence { have, need, independent, need_independent },
    Conflicted { supporting, challenging },
}
```

**`InsufficientEvidence` is not `Rejected`.** One says *we do not know*; the other says *we looked,
and it is bad*. Collapsing them turns every unknown into a negative, which is the same error chapter
18 spends its length on at the durability layer.

`Rejected` is never returned with an empty reason list, so a rejection always says why.

`Conflicted` is the one that shows the design's intent most clearly: support **and** challenge, both
current, and **not the reader's to silently resolve.** The layer hands you the disagreement rather
than picking a winner.

Independence matters as much as count. The reader's policy names control groups, and `Accepted`
reports both how many assessments supported and how many *independent* groups they came from. Five
assessments from one group is not five independent opinions.

---

## Standing: why a record stopped counting

```rust
pub enum Standing {
    Current,
    Retracted { by },                     // its own issuer withdrew it
    BasisWithdrawn { basis, hops },       // something it was DERIVED FROM was retracted
    Expired { at_ms },
}
```

The middle two are different facts and the distinction is the point:

- **`Retracted`** is the only status that is a judgement *about this record*, and the only one its
  issuer can cause.
- **`BasisWithdrawn`** is a fact about the record's **support**, not a verdict on the record. The
  issuer may well stand by it on other grounds. It carries the *nearest* withdrawn basis and how many
  derivation hops away it is.

Treating a withdrawn basis as a retraction would silently void conclusions their authors still hold.

---

## The semantic gate

```bash
make gate-knowledge
```

Three things the gate establishes are **impossible**, replayed in CI: misleading evidence cannot
silently erase a conflicting observation, cannot refresh expired evidence, and cannot confer
authority.

It also carries a positive control, named for what it guards against:

> `the_gate_is_not_satisfied_by_refusing_everything`

**A resolver that rejects everything looks safe while being useless.** Any gate of this shape needs
that control, or it measures caution rather than correctness.

---

## What this does not establish

- **Not that evidence-aware resolution improves outcomes.** The gate shows three things are
  impossible. Whether reasoning this way produces better decisions is a research question, explicitly
  *not* a deliverable of this axis.
- **Not a truth oracle.** `Conflicted` is a real answer, and the layer returns it rather than
  arbitrating.
- **Not protection against a dishonest issuer** beyond attribution. A signature says who said it, not
  whether they were right.
- **Not last-writer-wins.** Heads live in the gossip medium and records in an authorised store,
  specifically so a later write cannot erase a competing statement.

---

## Where to go next

| You want | Read |
|---|---|
| the decision record: four kinds, the two gates kept apart | [`design/knowledge-layer.md`](../design/knowledge-layer.md) |
| the contract in code | [`src/knowledge.rs`](../../src/knowledge.rs) |
| why an unknown is never a negative, one layer down | [18 · Contracts & receipts](18-contracts-and-receipts.md) |
| the same refusal-typing rule stated generally | [00 · Concepts](00-concepts.md) |
| a curated canon with a single writer, rather than competing claims | the [`mycelium-wiki`](../../mycelium-wiki/) companion and [21 · Mandates](21-mandates.md) |
