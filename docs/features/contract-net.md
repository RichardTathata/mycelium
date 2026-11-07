# The contract net

## What it is

`mycelium-commitment` coordinates work by commitment, as five records on the public API. A declarer announces a
requirement with its terms and deadline; participants offer against it — a willingness, not yet an obligation; the
declarer closes the requirement at or after the deadline, naming every offer it holds, and awards it once, by a
deterministic lowest-participant rule over the offers the close names; the awardee reports what happened, or that it
does not know; and anyone may assess whether the requirement was met. An [award](../guide/00-concepts.md#award) is a
receipt-bearing write, a second award is refused rather than written over the first, and offers, closes, awards and
assessments are signed when their author holds a key — an unsigned one verifies as `false`, never true by absence. No
component assigns another participant's obligation: an award only records an offer the participant made.

## When to use it, and when not

- **Use it when** work should go to whoever offers for it, under terms a declarer sets, and the record should show who
  committed to what and whether it was done — pickups offered by drivers and awarded by a hub.
- **Not when** the work should be pulled by whoever is ready ([tuple space](tuple-space.md)) or claimed by content
  ([blackboard](blackboard.md)); and not when you need a fair award — the rule is deterministic, which is a different
  property.

## Turn it on

This feature is turned on in code, from the `mycelium-commitment` crate.

- [`ContractNet::new`](../../mycelium-commitment/src/lib.rs) with the agent and this participant's name; then
  [`announce`](../../mycelium-commitment/src/lib.rs), [`offer_signed`](../../mycelium-commitment/src/lib.rs),
  [`close`](../../mycelium-commitment/src/lib.rs) and [`award`](../../mycelium-commitment/src/lib.rs); the awardee
  [`report`](../../mycelium-commitment/src/lib.rs)s and anyone [`assess`](../../mycelium-commitment/src/lib.rs)es.
- Award on the checked path — [`plan_award_verified`](../../mycelium-commitment/src/lib.rs) over offers verified by a key
  resolver you supply — where a forged offer must not be a candidate.
- [`commit_award_linearizable`](../../mycelium-commitment/src/lib.rs) where two declarers may race: a consensus round on
  the award's slot, so exactly one commits; [`commit_award_under_mandate`](../../mycelium-commitment/src/lib.rs) checks the
  declarer's [mandate](mandates.md) before any write.
- `one_award_per_requirement_with_a_receipt_and_a_second_is_refused_not_overwritten`, in
  [`mycelium-commitment/src/lib.rs`](../../mycelium-commitment/src/lib.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`redistribution_cn`](../../examples/README.md#the-capability-matrix): surplus-food pickups announced by a hub, offered
for by drivers, each awarded exactly once, reported, and assessed by a food-bank auditor who signs. CI runs it.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the award's durability is the rung its
  [durability receipt](durability-receipts.md) names, and no more.
- **Not built:** a single award between racing declarers on the plain path — two declarers racing `award` can both
  win; only the linearizable commit refuses the second; when an offer was really made — the declarer's close decides
  which offers were in time, and `closed_at_ms` is its own claim; protection of the checked path from deletion — a
  member who deletes a named, signed offer removes it from the candidates; participant identity — a signature proves the
  key, not whose it is; exactly-once work, which is the destination's ([destination commit](destination-commit.md)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the contract net on its own;
  `cargo run -p mycelium-commitment --example redistribution_cn` is its CI gate.

## Go deeper

- **Guide:** [24, commitments](../guide/24-commitments.md), including
  [§ What this does not establish](../guide/24-commitments.md#what-this-does-not-establish) and
  [§ Which coordination model?](../guide/24-commitments.md#which-coordination-model).
- **Runbook:** no runbook of its own.
- **Design:** [`plans/v3-contracts-axis.md`](../plans/v3-contracts-axis.md) §6.9 and §13.2.
- **Code:** [`mycelium-commitment/src/lib.rs`](../../mycelium-commitment/src/lib.rs).
