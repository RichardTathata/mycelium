# Selective recovery

<!-- A feature page links to its sources and copies nothing from them. See README.md in this directory. -->

## What it is

[Selective recovery](../guide/00-concepts.md#selective-recovery) is what a fleet does about one effect whose fate it
does not know: it probes before it retries, reads the **dependency closure** of the unknown record across the
fleet's journals — every record that names it, or names one that does, and no record that does not — and recovers
each member by its declared class, each as a new record that names what it recovers. It is the adopter-surface
plan's decisive demonstration: the [side-effect contract](../guide/00-concepts.md#side-effect-contract)'s classes,
the [causal records](../guide/00-concepts.md#causal-records) and their closure reader, and the
[fault classes](../guide/00-concepts.md#test-faults) composed into one run, the `recovery` class of
`mycelium test --faults`.

## When to use it, and when not

- **Use it when** a declaration's fleet will carry effects outside it and you want to know, before it does, what
  this fleet does when an acknowledgement is lost, work has started on it elsewhere, and a node restarts in between.
- **Not when** the question is a real provider or more than one destination: the class's destination is one mock in
  the test's own process, the closure is read offline from journal files, and nothing weighs a closure's size
  ([what-is-proven.md](../operations/what-is-proven.md), *Selective recovery*; the plan's §6).

## Turn it on

This feature is turned on in code.

- [`run`](../../src/faults/recovery.rs) — the `recovery` class: three nodes of the declaration's fleet with a
  reconciler, an evidence journal and the evaluator each, one mock destination that loses answers once per key; the
  lost acknowledgement, the dependents carrying their parent, the independents, the crash and restart over the WAL,
  the dark node; then the recovery and its ten checks.
- [`Lossy`](../../src/faults/recovery.rs) — the destination: a payload beginning `lose:` is applied and not
  answered on its first send, `vanish:` neither applied nor answered; every send and probe counted.
- [`apply_caused`](../../src/effects/external.rs) — an effect applied with X4's fields, so a dependent's record names
  the record it depends on; [`leave`](../../src/effects/external.rs) — the irreversible class's recovery verb;
  [`claim_retry`](../../src/effects/external.rs), [`claim_probe`](../../src/effects/external.rs),
  [`claim_compensation`](../../src/effects/external.rs) and [`claim_leave`](../../src/effects/external.rs) — each
  move's check and change in one step under the ledger's lock, so two moves racing on one record cannot both send.
- [`persistence`](../reference/configuration.md#the-fields) — the class needs `[persistence]` and a `gateway` + `tls`
  build; it reads `not_configured` or `not_in_build` otherwise, and fails the run only when asked for by name.
- `the_recovery_class_recovers_the_closure_by_class_and_leaves_independent_work`, in
  [`src/faults.rs`](../../src/faults.rs): the live form against a persisted three-node fleet, every check held.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs `mycelium test --faults` against the Rust scaffold's
declaration with `[persistence]`, `recovery` among the classes (`recovery: passed`), and the one-hour job runs it on
the published binary as its step 5 from the first release that carries the class. The seamed form is
`the_checked_in_recovery_bundle_replays_here_and_matches_a_fresh_recording` in
[`tests/faults_recovery_replay.rs`](../../tests/faults_recovery_replay.rs): the recovery phases recorded under the
replay kernel into the corpus bundle `tests/replay-corpus/x9-selective-recovery/`, replayed in CI with the destination
reached **zero times in those phases** (the seven sends that set the scene are made live in both forms) and every
check made again on the replay.

## What it guarantees, and what it does not

- **Probe before retry, one effect.** The restarted node's WAL replays a write made before the crash
  ([`persist.configured`](../reference/guarantee-catalogue.md), [`persist.sync_mode`](../reference/guarantee-catalogue.md)),
  its ledger — a journal, folded at open — finds the record *unknown*, a retry without a probe is refused by name, the
  probe finds the effect done, and the destination was **reached once** for it (it deduplicates, so the class counts
  the sends that reached it, not only the applications) — the contract's own rule, run after a crash.
- **The closure, and what it cannot see.** Over the journals the fleet can reach the reader names every record whose
  parent chain reaches the unknown one, and no independent one. With a node dark its journal is withheld, the reader
  says *INCOMPLETE* and names the hole — and **a hole hides what hangs below it**: a record two steps down through
  the withheld journal is outside the closure until the node returns.
- **Recovery by class, each as a new record:** an idempotent dependent is probed and *resumed* by one retry (of two
  concurrent retries one is refused before anything is sent); a compensatable dependent is probed and never retried;
  a committed dependent whose premise did not happen is *compensated* under the evaluator's permit
  ([`ae.preflight`](../reference/rule-catalogue.md); the reconciler records the permit's decision before the
  compensation is sent — its own ordering, not the gateway's guarantee, which a fleet with no gateway resolves
  `not_applicable` and the class does not list); an irreversible dependent is *left*, unresolved and escalated. The
  rule is the contract's: *unknown + idempotent → probe, then retry if not done; unknown + compensatable → probe,
  never retry (found not done it ends `not_done`: nothing at the destination, a new operation is the caller's);
  committed on a premise that did not happen → compensate; unknown + irreversible → leave.* Each move's check and
  change are one step under the ledger's lock, so two moves racing on one record cannot both send.
- **Independent work is untouched**: outside the closure, its ledger states unchanged, no new record citing it.
- **Explained from the records**: `mycelium explain --closure <hash> <journals> --fleet 3` renders every recovery
  decision; the class keeps the rendering beside a kept run (`--keep`, `closure.txt`).
- **The seamed form replays without reaching the destination**: the reconcilers' sends and probes are `Input`-class
  choices, so the corpus bundle replays against a fresh fleet with the destination reached zero times; the fleet's
  own gossip and the nodes' runtimes are outside the kernel (the inventory's §3.1.1), so what the bundle holds is
  the recovery's decisions, not the fleet's schedule.
- **Not built, stated** ([`adopter-surface.md`](../plans/adopter-surface.md) X9): a real destination; more than one
  destination; a closure read at runtime (it is read from files, after the fact); any policy over the closure's size
  or age (the plan's §6, Y5); a composed compensation (`compensate` takes no resource authority, so one is refused
  rather than sent unchecked). The partition is one node gone dark, in process (D6); the seamed form's kernel sees the
  recovery phases only — the fleet's three nodes and their gossip are outside it.

## Go deeper

- Guide: [18 · Contracts and receipts](../guide/18-contracts-and-receipts.md#beyond-the-fleet-the-side-effect-contract);
  the principle in [the philosophy](../philosophy.md) (*Rollback → compensation as a new promise*).
- Operations: [what-is-proven.md](../operations/what-is-proven.md); the fault classes' page
  [test-faults.md](test-faults.md); the [side-effect contract](side-effects.md); [causal records](causal-records.md).
- Design: the plan row is the record ([`adopter-surface.md` X9](../plans/adopter-surface.md)); the replay seam's
  inventory is [`replay-nondeterminism-inventory.md` §2.4](../design/replay-nondeterminism-inventory.md).
- Code: [`src/faults/recovery.rs`](../../src/faults/recovery.rs), [`src/effects/external.rs`](../../src/effects/external.rs),
  [`src/closure.rs`](../../src/closure.rs), [`tests/faults_recovery_replay.rs`](../../tests/faults_recovery_replay.rs).
