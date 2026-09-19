# mycelium-effects — the transactional destination

↑ [companions](companions.md) · guide: [18 · Contracts & receipts](../../../guide/18-contracts-and-receipts.md) ·
operator: [operations/companions.md](../../../operations/companions.md)

The **fourth receipt rung**, made generic and inspectable. Item 1 names four and keeps them strictly
apart: local application · local sync · replica sync · **destination commit**. The substrate provides
the first three and **never** the fourth, because *at-least-once + idempotent merge = exactly-once
effect* is a rule about the caller's resource, and **a resource is the only thing that can say
whether an effect happened there**.

Design: `docs/design/contracts-receipts.md` §6 (item 1 PR 5). Key facts:

- **One table, one transaction.** `effects_dedup(operation_id PRIMARY KEY, content_hash, attempt_id,
  applied_at_unix_ms)`. The dedup row and the caller's business change commit **together or not at
  all** — that single fact is the whole design, and everything below follows from it.
- **`Fresh` / `Replayed` are facts, not guesses.** Same `operation_id`, same content, any attempt:
  `Replayed`, nothing applied twice — the retry may come from another worker, after a restart, a week
  later. The attempt id may differ; the operation identity is what dedups.
- **Same identity, different content is a `Conflict`.** A retry that changed its mind is not a retry,
  and a destination that quietly took the second version would have two effects under one identity.
  The first version stands.
- **A failed business change leaves no dedup row.** This is the one to hold onto: a destination that
  wrote its dedup row *outside* the transaction would record the attempt, fail the change, and then
  answer a later retry `Replayed` — **reporting an effect that never happened**.
- **A deadline overrun is `DeliveryUnknown`, and the apply is not cancelled.** `apply_within` spawns
  the apply on a blocking thread and times out the *wait*, not the work. It may still commit, which
  is precisely why the answer is unknown rather than failed, and why the resolution is a retry under
  the **same** identity rather than a new one.
- **Why this was justified where a shared tracker was not.** `docs/design/exactly-once-effect.md`
  declined, with evidence, to extract a shared in-flight tracker across the tuple space and the
  blackboard: their clocks and persistence differ. A destination sits *beyond* both and couples
  neither. What it adds is the transaction boundary, the dedup row, and a receipt a third party can
  read.
- **SQLite is `bundled`, quarantined to this crate.** CI and every developer get the same engine with
  no hidden system library, and `-p` builds of the substrate never pay for it.

## The gates

| Claim | Test |
|---|---|
| a same-content retry applies nothing twice | `a_retry_with_the_same_content_is_replayed_and_applies_nothing_twice` |
| a different-content retry is refused and applies nothing | `a_retry_with_different_content_is_a_conflict_and_applies_nothing` |
| **a failed change commits no dedup row** | `a_failed_business_change_commits_no_dedup_row` |
| the two writes are one transaction | `the_business_change_and_the_dedup_row_commit_together` |
| the destination remembers across a reopen | `a_reopened_destination_remembers_what_it_committed` |
| two appliers racing one operation yield exactly one `Fresh` | `two_appliers_racing_on_one_operation_yield_exactly_one_fresh` |

The third and fourth rows are the same property approached from both sides, which is the shape worth
copying: assert what happens when it holds *and* what a reader would see if it stopped holding.

The race test earns its place because the destination's transaction — not a Rust borrow — is what
serialises appliers: `apply` takes `&self` deliberately.

## The demonstration

`cargo run -p mycelium-effects --example destination_commit` — five steps in the food-redistribution
domain: `Fresh`, `Replayed`, `Conflict`, a failed handler leaving no dedup row, and a deadline
overrun the retry then resolves.

**It is a gate, not a display.** Planting the dedup row so it survives a failed business change fails
the example with its own message — *"a rolled-back attempt left no dedup row"*, `Replayed` where
`Fresh` was expected. It was written to close the [onboarding checklist](onboarding-checklist.md)'s
sharpest finding: this crate had no runnable demonstration anywhere.

## What this crate is not

Not the tuple space's `complete` — the pipeline's own receipt proves the item was acknowledged and
the next stage queued, **not** that a business transaction happened. Not the *in-process* half (the
local fiber runtime, beyond v3 by D34). The `tuple-space` feature adds the consumer that composes the
two: **effect first, acknowledgement second**, so the pipeline's receipt never stands in for the
destination's.
