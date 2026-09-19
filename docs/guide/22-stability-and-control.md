# 22 · Stability & Control

A fleet that reacts to what it sees will sometimes react to what it only *thinks* it sees. This
chapter is about the machinery that decides when not to act, and about the difference between a
budget you enforce and a budget you merely print.

It is grounded in [`src/control.rs`](../../src/control.rs) (the predicate) and
[`src/control/ledger.rs`](../../src/control/ledger.rs) (the rights). The design record is
[`docs/design/adaptive-stability.md`](../design/adaptive-stability.md), and the rollout runbook is
[`docs/operations/control-profiles.md`](../operations/control-profiles.md). The runnable
demonstration is [`examples/control_envelope_viz.rs`](../../examples/control_envelope_viz.rs).

Two questions this chapter answers:

- **"How do I stop governors thrashing on a bad view?"** With a predicate that holds an action when
  the view is not certain enough — but only for the classes of action where holding is the cheaper
  mistake.
- **"How do I turn this on without breaking a running fleet?"** A four-rung profile ladder whose
  second rung acts exactly as before and *records* what it would have held.

---

## The honest core: uncertainty is not a scalar

`assess` does not produce a confidence percentage. It produces the **first reason** the view is not
good enough, in a fixed order:

```rust
pub enum Uncertainty {
    StalenessUnknown,                              // no peer heard in the window
    Stale { max_staleness_ms, bound_ms },
    TooFewHeard { heard, required },
    SelfDegraded,                                  // this observer is itself opaque or shedding
}
```

`StalenessUnknown` is reported before `Stale`, because **a stale figure presupposes there is one**.
And the case worth internalising: **an isolated node is uncertain, not fresh.** A node that has heard
from nobody has a staleness figure that is a placeholder, not an observation. Treating that as
freshness is how an isolated node talks itself into acting on a view of a fleet it cannot see.

---

## Holding is not always the safe choice

Five action classes, and the split between them is by **which mistake costs more**:

| Class | Holds on uncertainty? | Why |
|---|---|---|
| `SpeculativeScaleUp` | yes | spending rights on a guess |
| `RoutineScaleDown` | yes | giving something up because the fleet *looks* oversized |
| `ProtectiveShed` | no | reads local state, and local state is never uncertain |
| `RescueFromZero` | no | a wrong rescue costs one extra member; a wrong hold leaves a group with nobody in it |
| `DeficitFill` | no | a wrong fill on a stale undercount costs one member; a wrong hold on a partition leaves a group stuck below its bound |

The last two are the ones that make the design worth reading. A naive "hold when unsure" rule would
hold precisely when a group had emptied out and nobody was coming.

`DeficitFill` was added after the first four shipped, because the membership governor's own primary
action fitted none of them. The record carries that as a dated amendment rather than quietly widening
an existing class.

---

## The predicate

```rust
pub fn decide(class, view, bound, profile) -> Decision

pub enum Decision {
    Proceed,
    Held(Uncertainty),
    WouldHold(Uncertainty),   // Observe only: act, AND record that enforcing would have held
}
```

`WouldHold` is a distinct variant **so a caller cannot treat it as `Proceed` by forgetting to check a
flag**. That is a deliberate API shape, not an accident: the observe rung is where a deployment lives
while it decides whether the rule is right, and it is worthless if the recording is easy to skip.

The predicate is *derived* rather than hand-listed. A class that holds on uncertainty is held when
the view is uncertain and the profile enforces, recorded as would-hold when the profile observes, and
never consulted under `Legacy`.

---

## The profile ladder

```rust
pub enum Profile { Legacy, Observe, EnforceLocal, EnforceAllocated }
```

| Rung | Behaviour |
|---|---|
| `Legacy` | today's governors, untouched. The predicate is not consulted |
| `Observe` | the predicate runs and its holds are **recorded**; nothing is held |
| `EnforceLocal` | budgets on this node's own actuators; holds are real |
| `EnforceAllocated` | ceilings backed by **allocated rights**; holds are real |

Two safety properties worth copying into your own config handling:

- **An unknown stored byte reads as `Legacy`**, never as an enforcing profile a bit-flip could switch
  on.
- **An unknown profile *name* is refused**, never read as `Legacy`, so a typo in a config file cannot
  silently step the ladder down.

Those two rules point in opposite directions on purpose. A corrupt byte should fail safe; a
misspelled intention should fail loud.

---

## Why a right cannot be soft state

Everything a governor reads from discovery **evaporates** when its owner goes quiet. That is the
right shape for an observation and the wrong shape for a right: an allocation that vanished with its
holder would be **issued twice**.

So rights live in a node-local, append-only, fsynced journal that is **never gossiped**. Only a
bounded, signed `RightsHead` leaves the node.

```rust
pub struct Right {
    holder: PrincipalId,
    resource: String,
    units: u64,            // resource-native units. NEVER money
    allocated_by: PrincipalId,
    term: TermId,          // chapter 21's term identity, reused rather than reinvented
    state: RightState,
    valid_until_ms: u64,
}

pub enum RightState { Installing, Warming, Serving, Draining, Unknown }
```

**Every state counts against the budget.** A unit in `Installing` is as consumed as one in `Serving`,
and `Unknown` is a state rather than a zero — it is what the ledger says after a crash mid-transition
or a reconcile that timed out.

### Three rules the ledger enforces

```rust
pub enum LedgerRefusal {
    NotDurable(JournalError),              // "not recorded, so not done"
    Duplicate { holder, term },            // a term is allocated ONCE
    Unknown { holder, term },
    Rejected { requested, available },
    Encoding(String),                      // a defect, not a runtime condition
}
```

1. **Persisted before acting.** `NotDurable` means nothing happened, and it carries the journal's own
   reason — including *unknown*, which is a different claim from failure.
2. **Never reclaimed because an owner vanished from discovery.** Going quiet is not releasing.
3. **A rejected admission is a recorded outcome.** `admit` writes the rejection *before* it refuses,
   because a rejection nobody recorded is a silence, and a silence is indistinguishable from work
   nobody asked for.

Note the pair: `may_admit` is **pure** and answers the question; `admit` is the one that records.
Reach for the pure one when you are planning and the recording one when you are deciding.

---

## Spacing and settling

A predicate that says yes is not the whole answer. Two more gates sit around it:

- `spacing_allows` — has enough time passed since the last action on this actuator? It saturates, so
  a wall clock that jumped backwards **refuses** rather than wrapping into a very large allowance.
- `may_propose` — has the previous action settled, or is the system still absorbing it?

`ControlSpec` carries the pair `(governor, actuator)`, and the record fixes **one owner per
actuator**. Two governors moving the same actuator is the oscillation source that no amount of
per-governor care will fix.

---

## Run the demonstration

```bash
cargo run --example control_envelope_viz
```

It writes an HTML envelope you can open. What to look for is the difference between a hold and a
would-hold on the same trace, which is the whole argument for the observe rung existing.

A recorded doubt worth knowing about: the project once believed hysteresis was not load-bearing while
the release spacing was on. It is. The earlier reading was schedule coverage, not a real result, and
the correction came with a measured bound — **hysteresis damps a persistent hover, it does not settle
one.** The sweep gained a per-breaker plant so the claim cannot quietly regress.

---

## What this does not establish

- **`EnforceLocal` is not a hard bound.** A hard bound is hard prevention *only* when backed by
  exclusive, durably accounted rights. Without those it is not a hard bound however it is labelled,
  which is why the top rung exists as a separate rung.
- **Nothing here is about money.** Units are resource-native. A bound expressed in money would depend
  on a rate card the node does not hold, at a time it cannot pin, which is a bound it cannot enforce.
- **The ledger is node-local.** It is not a cluster-wide allocator, and the signed head that leaves
  the node is a summary, not the journal.

---

## Where to go next

| You want | Read |
|---|---|
| the decision record: the control spec, rights, and the profile ladder | [`design/adaptive-stability.md`](../design/adaptive-stability.md) |
| how to roll the ladder out on a running fleet | [`operations/control-profiles.md`](../operations/control-profiles.md) |
| the three strength tiers this chapter's "hard bound" sits in | [16 · Guardrails](16-guardrails.md) |
| the term identity the ledger reuses | [21 · Mandates](21-mandates.md) |
| load shedding and opacity, the local-state half | [00 · Concepts](00-concepts.md) |
