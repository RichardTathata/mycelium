# Authority at execution (ADR, Boundary H item A1)

**Status:** **adopted and implemented** 2026-09-25 (`src/mandate/authority.rs`;
`ReferenceEvaluator::allowances_without_mandate` in `src/agent/action_evaluator.rs`). Plan:
[`plans/boundary-h.md`](../plans/boundary-h.md) §7 A1 (rev 0.4). It builds on the
[scoped-mandates ADR](scoped-mandates.md) (`ResourceAuthority`), the [AE2 ADR](action-envelope-ae2.md) (tiers), and
the [issuer-binding ADR](knowledge-issuer-binding.md) (P1 verification).

> Posture, once: **stopping new admissions is not stopping admitted work**, and **expiry is not revocation**.
> Authority is re-established at every point where work acts, never inherited from the moment it was admitted.

---

## 1. The problem

Letting a mandate expire stopped **new admissions** wherever a mandate was checked. It did not stop work **already
admitted**:
- a task admitted a second before expiry ran on;
- a queued item dequeued an hour later ran on;
- a retry ran on;
- delegated work could outlive its parent.

A revocation that never reached a disconnected reader was also indistinguishable from "nothing revoked". In
the Hugging Face counterfactual, this was the one thing standing between "stops new work" and "stops the fleet".

## 2. Decision: the five-part contract

| # | Rule | Where |
|---|---|---|
| 1 | Every protected operation requires an established mandate | `ExecutionGate::check` → `NoMandate`; `ReferenceEvaluator::allowances_without_mandate` lists policy holes |
| 2 | Missing or unverifiable authority prevents execution | Unknown revocation standing → `RevocationUnknown` (denied) |
| 3 | Validity is checked **at the execution boundary** | `ExecutionGate::check` re-runs `ResourceAuthority::check` (epoch, scope, window, operation) every time. The gate declares the resource's **AE2 tier**, and `ExecutionGate::strict` refuses an **Advisory** resource |
| 4 | Queued, retried and delegated work keeps the requirement | Call `check` at dequeue and at every retry. `AuthorizedWork::new` clamps to the mandate's window, and `delegate` can never extend the parent's |
| 5 | Long-running work declares a continuation policy | `Continuation::ReauthorizeAt { interval }` or `RunToCompletion { max_duration, enforced }`. `StopContract::drain_bound` gives a bound, or `Unbounded` with the reason: an unenforced duration, an undeclared cancellation latency, or an unconfirmable stop |

**Two measurements, kept apart.** `drain_report` gives **T_admit** (expiry to the last admission) and
**T_drain** (expiry to the last *confirmed* stop). A stop that was requested or acknowledged but never confirmed is
counted as **unconfirmed**, never as stopped.

## 3. Decision: expiry and revocation

**The clock model.** *s* bounds each clock's deviation from real time, so any two clocks differ by at most 2*s*.
Expiry is local: a reader admits while its clock is at or before `valid_until_ms`, so admissions stop within *s* of
the real expiry instant.

**Revocation needs an authoritative freshness mechanism.**
- `RevocationCheckpoint { authority, scope, seq, issued_at_ms, revoked }`, signed by the establishing authority and
  verified through P1. It is issued at least every *I*, **even when nothing is revoked**.
- **The freshness predicate, exactly:** fresh iff `(a − r) ≤ 2s ∧ (r − a) ≤ F − 2s`, where *a* is `issued_at_ms`
  and *r* is the reader's clock.
  - *Safety:* accepted ⇒ real age ≤ *F*.
  - *Liveness:* real age ≤ *F* − 4*s* ⇒ accepted.
- **The profile refuses to start** unless *F* > 4*s* and *I* + *D* ≤ *F* − 4*s* (`FreshnessPolicy::validate`).
- **Replay protection:** the reader retains the newest `seq` per `(authority, scope)`. A lower or equal `seq` is
  `Replayed`, and cannot refresh freshness.
- **Silence is not evidence.** No checkpoint, or no fresh one, means `Unknown`, which is denied. Under a partition
  longer than *F* − 2*s*, protected work stops. That is the intended failure direction.
- **Future-dated:** `a − r > 2s` is refused as a clock fault or forgery.
- **A revocation, once seen, stands** regardless of later freshness: revocation is monotonic.
- **Scope binding:** a checkpoint for scope X says nothing about scope Y.

**Nothing here reads a clock.** Every `now_ms` is the caller's, so the contract replays deterministically and both
clock extremes are testable exactly.

## 4. What this does not claim

- **That any resource uses it.** A1 is the contract a resource applies at its effect boundary by calling
  `ExecutionGate::check`. Wiring it into specific resources (the wiki's mandate fence, gateway tool dispatch through
  AE2) is per-resource work and not done here. A resource that does not call it keeps "expiry stops new admissions"
  only.
- **Durable state.** The revocation view and its retained `seq` are in memory. A restarted reader starts at
  `Unknown`, which fails closed.
- **Timing by deployment.** T_admit and T_drain are measured by the caller's clock. Their *logic* is tested here;
  measuring them in a deployment is a follow-up.
- **The Cedar adapter.** `allowances_without_mandate` checks the reference evaluator only. The private Cedar adapter
  needs the same check over its own policy.

## 5. Gates

`mandate::authority::tests` (16) and `a1_policy_tests` (1):
- the profile refuses bad parameters and advisory resources;
- safety at the extreme that understates age;
- liveness at the extreme that overstates age;
- a future-dated checkpoint is refused;
- a replayed checkpoint does not refresh freshness;
- silence and partition deny;
- a checkpoint for one scope does not refresh another;
- a revoked appointment is denied;
- a forged checkpoint is not accepted;
- a protected operation without a mandate is refused;
- queued and retried work is refused after expiry;
- delegated work cannot outlive its parent;
- work is clamped to its mandate's window;
- a re-authorising task fails its first check after expiry;
- drain bounds per continuation, and unbounded classes;
- the drain report keeps requested, acknowledged and confirmed apart;
- policy holes (an allowance without a mandate, including through `*`) are reported.
