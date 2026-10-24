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

## Beyond the fleet: the side-effect contract (adopter-surface X5, 2026-10-24)

`src/external.rs` — a destination the fleet does not own gives no transaction to commit a dedup row in, so
the contract moves the decision **before** the effect: the destination's class (`EffectClass`: idempotent ·
compensatable · irreversible) and a `ProbeBudget` are declared on the `ExternalEffect` and recorded on the
prepared record, which the `Reconciler` writes to its own SQLite ledger (`external_effects`) before the
destination is reached. Key facts:

- **A found record binds.** `Ledger::prepare` under one `IMMEDIATE` transaction turns a found `Prepared` row into
  `Unknown` (a crash or a concurrent sender — probed, never re-sent) and re-prepares a `Failed` one; `bind`
  compares dedup scope, content hash and class before any move (`DeclarationMismatch`, `Conflict`); a
  compensation's permit is bound to it (`bind_permit`: operation, resource, parent, validity, principal), and
  a lost compensation keeps `compensated_by` as a link that `complete_compensation` closes when it settles.
- **The rules are the class's.** A lost answer → `Unknown`; a probe (`ExternalDestination::observe`:
  `Done` · `NotDone` · `Unknown`) before any retry; at most one retry, only when idempotent; irreversible is
  neither probed nor retried; the probe is counted before it is made and the budget (count or age) is checked
  before and after it, so the record is `Unresolved` and escalated the moment it is spent
  (`Execution::Unresolved`). The record's decision is honest: indeterminate for an uncomposed effect, the
  destination's own permit for a composed one, the evaluator's for a compensation; `effect_durability` only for
  a local destination's commit. Each forbidden move is an `ExternalRefusal` by name, counted in
  `RefusalCounts::record_external` (`RefusalSnapshot::external`).
- **A compensation is an effect.** `compensate` needs a `CompensationPermit` (the evaluator's preflight,
  `envelope` feature; without the feature none exists — fail closed) and records the new effect with the
  compensated record's hash as `parent`, so `closure::Corpus` shows both. The compensated record becomes
  `Compensated`.
- **A probe is egress.** `gate_egress` runs before every send and probe on `ExternalDestination::host`;
  `None` is local.
- **Replay does not reach the destination.** `send_through_seam` and `probe` go through
  `mycelium::sim_seam::input` — `ChoiceKind::Input`'s first production use, the one seam whose effect is
  suppressed in replay (the inventory §2.4 row). Inside the seam's closure nothing may call another seam:
  the kernel's `RefCell` is borrowed for the call, so the destination's `send` uses only its own timeouts.
- **Evidence.** With `with_evidence(journal, node)` every settled outcome, probe and compensation is an
  `AeEvidence` execution record at enforcement point `destination`, the ledger keeping the journal's hash
  (`evidence_hash`) as the record's identity; without a journal the ledger's `record_hash`
  (`mycelium.effects/prepared/1`) stands.
- **Reference destinations:** `SqliteDestination` (local; probe = the dedup row) and `HttpDestination`
  (`src/http.rs`, std TCP, no redirect followed, mock-grade); the test harness is `tests/mock_http/`.
- **Decided:** no new `CompositionLeg`; reconciliation is a state of the record.
- Tests: `tests/external.rs` (nine exits), `tests/external_evidence.rs` (`envelope`), `tests/external_replay.rs`
  (`sim`, the zero-reach replay). Feature page: `docs/features/side-effects.md`.

## What this crate is not

Not the tuple space's `complete` — the pipeline's own receipt proves the item was acknowledged and
the next stage queued, **not** that a business transaction happened. Not the *in-process* half (the
local fiber runtime, beyond v3 by D34). The `tuple-space` feature adds the consumer that composes the
two: **effect first, acknowledgement second**, so the pipeline's receipt never stands in for the
destination's.
