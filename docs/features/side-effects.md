# The side-effect contract

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

A destination outside the fleet — a payment provider, a cloud API, a ticket system — gives no transaction to commit
a dedup row in, so a lost answer cannot be resolved by a retry that reads it. The [side-effect
contract](../guide/00-concepts.md#side-effect-contract) has the destination's class **declared before execution**
and recorded on the prepared record: idempotent under the key, compensatable by a named effect, or irreversible.
On a lost answer the record is unknown, a reconciliation probe is required before any retry, at most one retry is
made and only against a destination that declared itself idempotent, and a declared probe budget bounds the
unknown loop — past it the record is unresolved and escalated. A compensation is a new effect, authorised like any
other, whose record names the one it compensates. It extends the [destination
commit](../guide/00-concepts.md#destination-commit) one rung outward.

## When to use it, and when not

- **Use it when** a tool's effect lands somewhere the fleet does not own, and *did it happen?* must have a recorded
  answer, or a recorded reason there is none.
- **Not when** the destination is the reference SQLite one or anything else that commits a dedup row in the same
  transaction as the effect — the [destination commit](../guide/18-contracts-and-receipts.md#rung-4-in-practice-mycelium-effects)
  already resolves a lost answer by replaying. **The headline limit:** the reference destinations are the SQLite one
  and an HTTP shape whose test harness loses responses; no real provider has been run against it
  ([what-is-proven.md](../operations/what-is-proven.md), *An external side-effect at a real provider*).

## Turn it on

This feature is turned on in code.

- `Reconciler` in [`mycelium-effects/src/external.rs`](../../mycelium-effects/src/external.rs): one ledger (a
  SQLite file, the rung-1 checkpoint), one destination, one egress policy.
- `ExternalEffect` and `EffectClass` in the same file: the effect with its declared class, probe budget and, when
  composed, its attribution and authority.
- `ExternalDestination` in the same file: the trait a destination outside the fleet implements — `send` and
  `observe`, the reconciliation probe — with `HttpDestination` in
  [`mycelium-effects/src/http.rs`](../../mycelium-effects/src/http.rs) and `SqliteDestination` in
  [`mycelium-effects/src/sqlite.rs`](../../mycelium-effects/src/sqlite.rs) as the reference shapes.
- `CompensationPermit` in [`mycelium-effects/src/external.rs`](../../mycelium-effects/src/external.rs): the
  evaluator's permit a compensation needs (Cargo feature [`envelope`](../../mycelium-effects/Cargo.toml)); without the
  feature none can be constructed, and the reconciler refuses by name.
- `input` in [`mycelium-core/src/sim_seam.rs`](../../mycelium-core/src/sim_seam.rs): the `Input`-class seam the
  apply and the probe go through, so a recording replays without reaching the destination (Cargo feature
  [`sim`](../../mycelium-effects/Cargo.toml), off in every shipped build).

The tests that run that code: `a_lost_response_is_unknown_and_a_retry_without_a_probe_is_refused` and its
siblings in [`mycelium-effects/tests/external.rs`](../../mycelium-effects/tests/external.rs),
`a_compensation_needs_the_evaluators_permit_and_names_the_record_it_compensates` in
[`mycelium-effects/tests/external_evidence.rs`](../../mycelium-effects/tests/external_evidence.rs), and
`a_recorded_run_reaches_the_destination_once_and_its_replay_zero_times` in
[`mycelium-effects/tests/external_replay.rs`](../../mycelium-effects/tests/external_replay.rs) — each run by CI's
`effects` job.

## See it work

No runnable example exists. The test that runs the feature end to end, against an HTTP destination that loses
responses on demand, is `a_probe_answering_not_done_permits_one_retry_and_the_destination_sees_one_effect` in
[`mycelium-effects/tests/external.rs`](../../mycelium-effects/tests/external.rs); the recording-and-replay
demonstration is `a_recorded_run_reaches_the_destination_once_and_its_replay_zero_times` in
[`mycelium-effects/tests/external_replay.rs`](../../mycelium-effects/tests/external_replay.rs).

## What it guarantees, and what it does not

- **The class is declared before execution and recorded on the prepared record**, which is on disk before the
  destination is reached (`prepared_record_hash`, `mycelium.effects/prepared/1`). **A found record binds what may be
  presented under its operation:** other content is a `Conflict`, another class or dedup scope a
  `DeclarationMismatch`, at the apply, the retry and the compensation alike; a record found `Prepared` — a send whose
  answer no one has, after a crash or beside a concurrent applier — is *unknown*, probed and never re-sent.
- **A lost answer is `DeliveryUnknown` and the record `Unknown`**; a retry without a probe, a second retry, and a
  retry against a destination not declared idempotent are each refused by name and counted
  (`RefusalSnapshot::external`; with the crate's `metrics` feature, `mycelium_effects_refusals_total{kind="external",
  leg=<reason>}`).
- **A probe answering `Done` settles the record and produces no second effect**; `NotDone` opens one retry under
  the same key, so the destination sees one effect.
- **An irreversible, unobservable effect is neither probed nor retried**; its record stays `Unknown` and its
  evidence is kept.
- **The probe budget bounds the unknown loop**: a probe is counted before it is made, and the moment the budget is
  spent — by this probe's `Unknown` answer, or found spent before it — the record is `Unresolved` and escalated,
  recorded as `Execution::Unresolved`; nothing further is attempted without a new authorisation. **Not built:** the
  re-authorisation itself (no API or route grants one; the record is read and the escalation is a human's).
- **A probe is egress**: the destination's host is checked against the same hostname allow-list as the effect, and a
  denied host is refused by the gate before the destination is reached. **The host is the destination's declaration**
  (`ExternalDestination::host`; `None` is *local* and ungated) — an adapter that misdeclares is not caught here.
  **Not covered:** credential binding at egress (post-360 rows L and M) — the reference HTTP destination attaches none;
  a real adapter attaches its own.
- **A compensation is an effect of its own**: refused without the evaluator's permit, and refused under a permit that is
  not *for it* (the class's declared operation, at this destination, naming this record as its parent, inside its
  validity, for its principal); its evidence record carries the evaluator's decision and names the compensated record's
  hash as `parent`, so `mycelium explain --closure` shows both; a compensation whose answer is lost keeps its link, and
  the probe or retry that settles it compensates the original
  ([causal records](../guide/00-concepts.md#causal-records); the [feature page](causal-records.md)).
- **The composed legs are enforced on the external path** as `apply_composed` enforces them: attribution, then
  authority; a composed effect with no authority to check against is refused, never sent. **Decided in the PR:** no
  new composition leg — reconciliation is a *state of the record*, not a leg of the composed guarantee.
- **A recorded run replays without reaching the destination**: the apply and the probe are `Input`-class choices
  (`ChoiceKind::Input`, first production use), written down by a recording kernel and returned by a replaying one. The
  seam gate does not scan the companion; the inventory's §2.4 row is the record.
- **The evidence record's decision is honest about who decided:** an uncomposed effect's is *indeterminate* (no policy
  saw it), a composed one the destination's own permit (the composition held), a compensation the evaluator's from its
  permit. **The receipt's rung is written where it is true:** a local destination's commit carries
  `effect_durability: on_disk`; a remote destination's commit is the destination's claim, which the local rung cannot
  name — the record carries `destination` and the dedup outcome instead.
- **The reconciler is a parallel path**, not pluggable through `EffectDestination`, `Counting<D>` or the tuple-space
  consumer: `tests/composed.rs` is unchanged and green against the SQLite destination; the external path has its own
  composition test.
- **Not built, stated** ([`adopter-surface.md`](../plans/adopter-surface.md) X5): a real provider (the mock stands
  where S3Mock stands for the object store); credential binding on the reference HTTP destination; a probe that is
  itself authorised by the evaluator (the probe is gated by egress and budget, not by policy); the exposure policy that
  reads the unresolved age (deferred to X9 and §6 of the plan); an operator route over the ledger.

## Go deeper

- Guide: [18 · Contracts and receipts](../guide/18-contracts-and-receipts.md#beyond-the-fleet-the-side-effect-contract).
- Operations: [companions.md § mycelium-effects](../operations/companions.md#mycelium-effects--the-transactional-destination);
  the counter in [metrics.md](../operations/metrics.md).
- Design: the plan row is the record ([`adopter-surface.md` X5](../plans/adopter-surface.md)); the composed effect's
  record is [`composed-effect.md`](../design/composed-effect.md); the replay seam's inventory is
  [`replay-nondeterminism-inventory.md` §2.4](../design/replay-nondeterminism-inventory.md).
- Code: [`mycelium-effects/src/external.rs`](../../mycelium-effects/src/external.rs),
  [`mycelium-effects/src/http.rs`](../../mycelium-effects/src/http.rs), the seam in
  [`mycelium-core/src/sim_seam.rs`](../../mycelium-core/src/sim_seam.rs).
