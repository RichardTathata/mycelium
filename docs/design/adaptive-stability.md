# Adaptive stability — three promises, one ledger, and what uncertainty is allowed to hold back (ADR, item 4 PR 1)

**Status:** adopted 2026-09-18 · **item 4 PR 1** of `docs/plans/v3-contracts-axis.md` §6.2 (decisions D17–D20;
D30 for the ledger). A contract, not an API. It depends on [`contracts-receipts.md`](contracts-receipts.md)
(item 1 — a right is persisted before it is acted on, and "persisted" means what a receipt says it means), reuses
[`scoped-mandates.md`](scoped-mandates.md)'s term identity for an allocation, and is judged under the replay
harness of item 6. It sits *beside* [`action-envelope-ae0.md`](action-envelope-ae0.md): AE decides **may this
action happen**; this record decides **how much of a resource this node may consume, and how sure it must be
before it changes that** — and the two never borrow each other's authority.

> **Posture, once.** The governors that exist today — tuning, membership, opacity, the provisioner — keep their
> decision functions unchanged. This record does not add a controller above them; it names what each one
> promises, gives the promises a strength in a vocabulary that already exists, and adds the one thing none of
> them has: a **ledger of allocated rights that nothing evaporates**. The reviewer's fixture targets are targets,
> not guarantees. Nothing here changes the wire.

## 1. Three promises, kept apart

A governor is asked for three different things, and the substrate has been answering all three with the same
mechanism — evaporating soft state — which is right for exactly one of them.

| Promise | What it means | Strength (D17: the **guardrails tiers**, no new vocabulary) |
|---|---|---|
| **Hard bounds** | a ceiling that *cannot* be exceeded: this many installs, this much of a budget, no more | `HardPrevention` (Tier C) — **only** when backed by **exclusive, durably accounted rights** (§4). Without those it is not a hard bound however it is labelled |
| **Stability objectives** | the fleet settles rather than oscillates: spacing, hysteresis, settling time, and the *combined* behaviour of several loops | `SelfImposedPrevention` (Tier A) — each governor breaks its own loop (§5) |
| **Service objectives** | admission control and fair scheduling — and **rejected work reported beside completions**, because a rejection is a visible outcome, not a silence | `SelfImposedTransition` (Tier B) — enforced at the point where work is admitted |

The membership governor already says the true thing about its own promise: *"bounds are convergence targets,
not guarantees"* (`src/agent/membership_governor.rs`, module doc). This record generalises that honesty. A
self-enforced budget is Tier A. A fleet ceiling **holds only with exclusive rights**, and a document that calls a
convergence target a hard bound is the defect this record exists to prevent.

## 2. The decisive rule — what uncertainty holds back, and what it never holds back

`ViewConfidence` (`src/agent/emergent.rs:52` — `peers_known`, `peers_heard`, `max_staleness_ms`,
`self_degraded`, and since WP5 `staleness_known()`) exists and is not yet acted on. This record makes it
**actionable and per-input**: every governor input carries its own confidence, and a policy predicate decides
whether an action class may proceed on it. The predicate has one asymmetry, and it is the sentence everything
else here serves:

> **Uncertainty holds speculation and routine scale-down. It never holds protective shedding, and it never
> holds rescue from zero capacity.**

Concretely, four action classes:

| Class | Example | Under low confidence (stale, few peers heard, `staleness_known == false`) |
|---|---|---|
| **Speculative scale-up** | pre-install a model because demand *looks* rising | **held** — acting on a guess spends rights on a guess |
| **Routine scale-down** | leave a group because it *looks* over `max` | **held** — the observation may be a partition, and a node that leaves on a partition makes it worse |
| **Protective shed** | go opaque because *this node's* channel is full | **never held** — it reads local state, and local state is never uncertain |
| **Rescue from zero** | join a group whose live count *reads as* 0 | **never held** — the cost of a wrong rescue is one extra member; the cost of a wrong hold is a group with nobody in it |
| **Deficit fill** *(added PR 4a)* | join a group that reads below its **declared** `min` | **never held** — by cost a rescue, not speculation: a wrong fill on a stale undercount is one extra member; a wrong hold on a partition is a group stuck below a bound someone declared |

> **Amendment, 2026-09-18 (PR 4a).** The table above was written with four classes. Wiring the first governor
> showed that its own primary action — the membership governor's join below `min` — fitted none of them: it is
> not speculation (the bound was declared, the deficit observed), and it is not rescue from zero by name. By
> **cost** it is exactly rescue, and the rule is about cost, so a fifth class was added rather than the join
> forced into `SpeculativeScaleUp` and held. The mapping from a membership decision to a class is a pure,
> tested function (`membership_governor::classify`), so this reading is a fact in the suite and not a comment.
> A **drain** — an operator's instruction carried by a fresh intent — is deliberately *no* class: the predicate
> judges inferences from the fleet view, and a drain is not one.

The asymmetry is not caution; it is the shape of the costs. Holding a protective shed on grounds of uncertainty
is how a node with a full channel waits for peers it cannot hear to tell it whether it is allowed to protect
itself. Item 4 PR 2 makes the predicate a pure function and sweeps it (every class × every confidence state),
the way item 5's partition table is swept.

## 3. `ControlSpec`, the flow, and who owns what

Every governor gets a `ControlSpec`: what it observes, what it may actuate, its spacing and hysteresis bounds,
and its profile (§7). Every action goes through one flow with a **stable action id** so that a retry, a replay,
and a reconcile all name the same thing:

**observe → propose → reserve → act → reconcile**

- *observe* reads inputs with their confidence; *propose* applies the governor's existing pure decision;
  *reserve* claims the right (§4) — **before** acting, and persisted before acting; *act* touches the actuator;
  *reconcile* observes the effect and settles the reservation (consumed, released, or `unknown`).
- **One owner per actuator.** Two governors writing one knob is a control loop with two set-points, and it
  oscillates by construction:

| Actuator | Owner |
|---|---|
| `HotParam::{InboundFps, WriterDepth, BulkHandlers}` | the tuning governor (`tuning_governor.rs`; `gate` is its hysteresis) |
| this node's membership of a governed group | the membership governor (`decide` at `membership_governor.rs:100`) |
| this node's opacity flag | the opacity gate (`opacity.rs:297`, pure) |
| an `Installing` reservation and the install it protects | the provisioner (`mycelium-wasm-host/src/provisioner.rs:44`) |

- **One owner per deficit** — the divergence the plan asked to be named here:

| Deficit | Owner | Not the owner |
|---|---|---|
| a group below `min` | membership governor | the provisioner (it installs software, it does not join groups) |
| inbound above what the writer can drain | tuning governor | opacity (opacity *sheds*; it does not re-tune) |
| a required model absent | provisioner, on `require_model`'s demand | the membership governor |
| a queue too deep | **the companion that owns the queue** (its primary, via its depth signal — §6) | any substrate governor |

## 4. The rights ledger — vocabulary and backend (D30's decision, made here)

This is the piece none of today's governors has, and the one D30 says item 4 must decide before the resource
accounting slice can be written: *"the ledger's vocabulary and backend are item 4's decisions, not RA's."*

**Why it cannot live in gossip KV.** Everything a governor reads today is soft state that **evaporates** — a
capability advertisement, a load report, an intent. That is the right shape for an *observation*: a provider
that vanishes should stop being offered. It is the wrong shape for a *right*: an allocation that vanished
because its holder dropped out of discovery is an allocation that will be **issued twice** — once to the holder
that is still, in fact, running, and once to whoever is next. Gossip KV is LWW-resolved, so a right written
there can also be *overwritten* by a later writer with a bigger claim. Item 3 faced the same problem with
records and put only *heads* in the medium; this record does the same.

**The decision.** Strict budgets are **fixed, disjoint, allocated rights**:

- A `Right` is `{ holder, resource, units, allocated_by, term, state }`. `units` are **native** — installs,
  bytes, slots, tokens — never money (the RA slice keeps money out of the hard-bound vocabulary, and this record
  does not put it in). `allocated_by` is an authority, and `term` is item 5's `TermId`: an allocation is an
  appointment, and "the same holder, re-allocated after a gap" must be distinguishable from "never lapsed".
- `state` is counted across **`installing` · `warming` · `serving` · `draining` · `unknown`** — a unit in
  `installing` is as consumed as one in `serving`, and `unknown` is a state, not a zero.
- **Persisted before acting** through item 1's durable write (`set_requiring_sync` / `prepare_write` +
  `commit_prepared`), and a right whose receipt is not `LocalDurability::OnDisk` has not been reserved.
- **Never reclaimed because an owner vanished from discovery.** A right is released by its holder, expires by
  its term, or is revoked by its allocator. Absence from the peer table is none of those.
- **`admission.rejected` is a first-class outcome**, recorded beside completions, so a budget that refuses work
  is visible as a budget that refused work.

**The backend** is a **node-local, append-only, fsynced journal that is never gossiped** — the shape the AE
`EvidenceJournal` already has (`src/agent/evidence_journal.rs`, on item 1's durability contract), not a new
service and not a KV prefix. What gossips is a **bounded signed head**, `rights/head/{holder}`, reserved here
(§11) and used from PR 3: a holder's *claim* of what it holds, checkable against the allocator's own journal,
which is the authority for how many rights exist. Exclusivity is therefore **by allocation, not by consensus** —
the allocator's ledger is where "this many and no more" is true, and a fleet ceiling is the sum of what one
allocator issued.

## 5. Loop-breaking points, named

Two of these already exist and are cited rather than rebuilt; the rest are `ControlSpec` fields.

- **Hysteresis** — the tuning governor's `gate` (`tuning_governor.rs:152`) and the opacity gate's *"clearing
  requires fill to fall a full `hysteresis` below the effective threshold"* (`opacity.rs:297`). Kept.
- **Cooldown** — the membership governor's `membership_cooldown_secs`, made an explicit bounded parameter by
  WP5 (D20), read at start. Kept, and the decision recorded there — live timing intents do not alter it — stands.
- **Spacing** — a minimum interval between two actions on one actuator, per `ControlSpec`. New.
- **Settling** — the reconcile step: a governor does not propose again on an actuator whose last action has not
  been observed to take effect, or to have failed, or to have timed out into `unknown`. New.
- **Combined behaviour** — several loops sharing inputs (opacity reads load; tuning changes throughput; the
  membership governor biases joins toward idle nodes, which changes load) can oscillate together while each is
  stable alone. That is tested **once, under the replay harness** — D19: *the combined-feedback harness is replay
  stage 6* — reusing the governors' pure decision functions, not a bespoke rig. It is item 6 PR 6, and it is
  blocked on this record only in the sense that it needs the `ControlSpec` shape of PR 2.

## 6. The workload probe consumes depth (D18)

Backlog is already measured where work queues: `TupleSpace::depth` (`mycelium-tuple-space/src/lib.rs:1330`),
`Blackboard::depth → BoardDepth` (`mycelium-blackboard/src/lib.rs:664`), and the KV-ring stages. The probe
reads those. It does not add a metric until one of them is shown insufficient, and the companion that owns the
queue owns the deficit (§3).

## 7. Profiles — shadow before enforcement

`legacy` · `observe` · `enforce-local` · `enforce-allocated`, in that order of adoption, the same discipline as
item 7's `gateway_caller_profile` and the AE slice's observe mode:

- **`legacy`** — today's governors, untouched. The default until an operator opts in.
- **`observe`** — the ledger records what *would* have been refused, and the confidence predicate records what
  it *would* have held; nothing changes behaviour. **This is where a deployment lives until its own traces show
  the rules are right.**
- **`enforce-local`** — Tier A budgets on this node's own actuators; the predicate holds actions.
- **`enforce-allocated`** — Tier C ceilings, backed by allocated rights from a named allocator.

**Rollback disables new enforcement admissions and deletes nothing**: a right already held is honoured until it
is released, expires or is revoked, and the journal is never truncated by a profile change.

## 8. Detection, not prevention — except the ledger

The substrate's law holds: Layer I is never taught a higher-layer rule, and every promise in §1 except one is
enforced by the governor on its own node and *detected* elsewhere (tripwires, counters, the diagnose surface).
The exception is the rights ledger, which is prevention by construction: a right that is not in the journal was
not reserved, and an action without a reservation does not proceed. That is the one place a hard bound is hard,
and it is hard because it is **local and durable**, not because it is global.

## 9. What lands next

| PR | Contents |
|---|---|
| **1** *(this record)* | the ADR; `rights/head/{holder}` reserved in the namespace table and `kv_ns` |
| **2** ✓ | `src/control.rs` — `ControlSpec`; the **confidence predicate as a pure function, swept** over every action class × every way a view can be uncertain, taking the real `ViewConfidence`; the four profiles with `Observe` as a distinct `WouldHold` decision; the stable `ActionId`; spacing and settling as pure checks |
| **3** ✓ | `src/control/ledger.rs` on `agent::journal` (the mechanism lifted out of the AE profile in PR 3a, ungated here) — `Right` with its five counted states, **persist-then-apply** (a record that did not reach disk allocates nothing), no method that takes a peer set (discovery loss cannot reach the ledger), `admission.rejected` as a journal record, a fail-closed open over an undecodable journal, and `RightsHead` over `serde_fixint` bytes with `tls`-gated verification |
| **4a** ✓ | **the membership governor through the contract** — `classify` (pure, tested: join at 0 → rescue, join below `min` → deficit fill, leave over `max` → routine scale-down, drain → no class), the predicate per pass on `compute_view_confidence`, `SettleState` observed against the group's membership, the cooldown as `ControlSpec.spacing_ms` (unchanged in meaning), the node's profile as an atomic (`set_control_profile`, default `Legacy` — production behaviour unchanged until an operator opts in) and the `Observe` tripwire `control_would_hold_count`. Its `fastrand`/`Instant`/`sleep` sites routed through the seams: baseline 6 → 1 |
| **4b** ✓ | **the tuning governor and the opacity gate through the contract** — local-input governors, so spacing and settling only; the confidence predicate does not apply to a view that is this node's own. *Tuning:* `gate_at`/`acted_at` (pure in time; `gate`/`acted` read the seam clock) — **reconcile is the knob's readback**: an action is pending until a later `gate` sees `cur` at the applied value (consumed) or the settle timeout passes (`unknown`, counted, warned); a value equal to `cur` is not an action; a change inside `spacing_ms` of the last action is held. `acted` is separate from `gate` so a policy-rejected value runs no clock. Timing set by `start_cluster_tuner` to two ticks each; both `0` by default = the old gate. Counters in `GovernorSnapshot` (`held_by_spacing`, `held_by_settling`, `settled_unknown`) and `ParamSnapshot.pending`. *Opacity:* **only the release is spaced** (`OpacityHint.release_spacing_ms`, default 1 s) — `GoOpaque` is protective shedding, never held (§5's decisive rule); no settle state, because the loop's own input is the effect channel and every tick re-reads it before proposing; tripwire `opacity_releases_spaced`. *Not shown:* the tuner loop end to end (unit-level pins only), and the combined behaviour — PR 6's harness |
| **4c** ✓ | **the provisioner against the rights ledger** — the ledger's first live user, in `mycelium-wasm-host`, on the public API only. `Provisioner::with_install_rights(ledger, holder, resource, signing_key)`: before an `Installing` reservation the round asks `may_admit(holder, resource, reserved + live + 1)` — **a unit in flight is as consumed as one serving** (§4) — and refuses otherwise, counting the refusal (`rights_refusals`) and **recording** it (`admission.rejected`, off the synchronous admission path). The admission reads the ledger under `try_lock`: `provision_round` is synchronous and must not block on a journal write, so a busy ledger is a refusal, not a wait. The head goes into `rights/head/{holder}` as `PublishedRightsHead { head, signature }` on attach and after every refusal, signed with an operator-supplied Ed25519 key — **unsigned means unproven**, and `verify_published_head` says `false` for it. *Decided here:* the provisioner never allocates to itself (the ledger is the allocator's record; an unallocated node is refused every install, visibly); per-install rights and state transitions on the right are not modelled — consumption is the provisioner's own count, checked against the units held. *Not shown:* revocation or expiry of a right while an install is live (the next round refuses, the live install is not torn down — a release is the holder's act, not the ledger's) |
| 5 | admission control at the companions' queues via depth (§6), the example, `docs/operations` runbook, the §6.6 ledger entry for any config struct that gains a field |
| — | the combined-feedback harness is **item 6 PR 6** (D19), on PR 2's `ControlSpec` |

**The gate that matters** is PR 3's: *a right whose holder vanishes from discovery is not reissued* — and its
twin, *a right that is released, expired or revoked is*. Both replayed. A ledger that passed only the first
would be a ledger that never frees anything, which is the failure mode §1 of the knowledge record warns about in
another form: a mechanism that refuses everything looks safe while being useless.

## 10. What this record refuses

- **A coordinator, a controller, or a global scheduler.** Each governor acts on its own node; the ledger is
  local; exclusivity is by allocation.
- **Rights in gossip KV.** Heads only (§4, §11).
- **Reclaiming a right because its holder stopped being heard.** Discovery loss is not release.
- **A money vocabulary in the hard bounds.** Native units; an estimate is a claim in item 3's sense.
- **A new promise-strength vocabulary** (D17), **a new backlog metric** before the depth signals (D18), **a
  bespoke feedback harness** (D19).
- **Any claim that a convergence target is a guarantee.** The membership governor's sentence stands for all of
  them.

## 11. Reservations made at PR 1

`rights/head/{holder}` — in `src/lib.rs`'s namespace table and `kv_ns::RIGHTS_HEAD`. **Heads only.** The ledger
itself is node-local and never enters the medium. Nothing writes the prefix until PR 3.

## Appendix — anchors verified at adoption (2026-09-18)

| Claim | Where |
|---|---|
| `ViewConfidence` fields; `staleness_known()` is a derived accessor (WP5) | `src/agent/emergent.rs:52` |
| the membership decision is pure; bounds are convergence targets | `src/agent/membership_governor.rs:100` (`decide`), module doc |
| the tuning governor gates the auto-tuner with hysteresis; three `HotParam`s | `src/agent/tuning_governor.rs:40`, `:152` (`gate`) |
| the opacity transition is pure, with a clearing hysteresis; a 100 ms ticker | `src/agent/opacity.rs:297`, `:341` |
| the provisioner holds an `Installing` reservation | `mycelium-wasm-host/src/provisioner.rs:44` |
| demand is a declaring-node/provider count, not work | `src/agent/demand.rs` (`demand_snapshot`) |
| the node-local fsynced journal shape, on item 1's durability | `src/agent/evidence_journal.rs:132` |
| the durable write verbs and the receipt that means "on disk" | `mycelium-core/src/kv_handle.rs:163`, `:257`; `receipt.rs:185` |
| the companions' depth signals | `mycelium-tuple-space/src/lib.rs:1330`; `mycelium-blackboard/src/lib.rs:664` |
| the guardrails tiers | `mycelium-guardrails/src/lib.rs` (`Strength::{HardPrevention, SelfImposedPrevention, SelfImposedTransition}`) |
| the cooldown parameter and `staleness_known` shipped (WP5, D20) | `docs/wiki/dev/history.md` → *WP5* |
