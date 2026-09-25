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
- **A revocation, once seen, stands** regardless of later freshness, and regardless of later checkpoints that omit
  it: revocation is monotonic. The view keeps every revoked term per `(authority, scope)` apart from the newest
  checkpoint. It kept only the newest until 2026-09-25, when the wiki store's tests found that a later checkpoint
  omitting a term reinstated it.
- **Scope binding:** a checkpoint for scope X says nothing about scope Y.

**Nothing here reads a clock.** Every `now_ms` is the caller's, so the contract replays deterministically and both
clock extremes are testable exactly.

## 4. What this does not claim

- **That every resource uses it.** A1 is the contract a resource applies at its effect boundary by calling
  `ExecutionGate::check`. The **gateway** applies it (§6), and so does the **wiki's git store** (§7). Other
  resources (provider admission, the `FsStore`) apply it only when wired, and until then keep "expiry stops new admissions" only.
- **Durable state.** The revocation view and its retained `seq` are in memory. A restarted reader starts at
  `Unknown`, which fails closed.
- **Timing by deployment.** T_admit and T_drain are measured by the caller's clock. Their *logic* is tested here;
  measuring them in a deployment is a follow-up.
- **The Cedar adapter.** `allowances_without_mandate` checks the reference evaluator only. The private Cedar adapter
  needs the same check over its own policy.

## 6. Wired at the gateway (2026-09-25)

**The gap.** AE1 gave the action envelope a slot for the enforcement point's finding about a mandate, and the
gateway always filled it with `None`, because it held nothing it could verify. A policy rule that **requires** a
mandate could therefore never be satisfied at the gateway: it always answered `Indeterminate`.

**The wiring.**
- `GossipAgent::with_execution_authority(ExecutionAuthority)` attaches A1's `ExecutionGate` and P2's
  `GrantVerifier`. `offer_revocation_checkpoint` feeds the revocation view.
- In `ae_preflight`, the one function all three doors (`/mcp`, `/a2a`, federation calls) go through, a grant
  presented in `params._meta.mandate` is assessed:
  1. the grant's **holder must be the authenticated caller**, as the auth layer resolved it, never what the request
     asserts;
  2. P2 checks it: issued, entitled by configuration, current, and possessed. The possession proof is bound to
     **this call** by `possession_request(operation, resource, arguments_digest)`, which binds the operation, the
     resource before `@`, and the arguments digest;
  3. A1's gate checks it at dispatch: the resource's epoch, scope, window and operation, and fresh revocation
     standing;
  4. the result is bound as `Established`, `Refused` or `Unknown`, and the envelope's `not_after_ms` is clamped to
     the mandate's window.
- The operation a grant enumerates is `{operation}:{resource before @}`, e.g. `skill.invoke:skill:depot/dispatch`.
  The resolved provider after `@` is not something a caller can know in advance, so neither the grant nor the proof
  binds it.
- **No authority attached: exactly the behaviour before.**

**SDK parity.** `mycelium-py` and `mycelium-ts` accept `mandate=` on `A2aClient.send`/`stream`, and export
`arguments_digest` and `mandate_request_bytes`, so a holder can compute exactly what to sign. The SDKs do not sign.
**Golden vectors** (the canonical arguments digest, and the request bytes) are pinned identically in the gateway's
Rust tests and both SDKs.

**Gates.**
- `gateway_authority::tests`: no presentation binds nothing; a valid grant is established; a grant held by someone
  else is not; a proof for other arguments does not carry; no fresh revocation view means not established; a
  superseded appointment is refused; a revoked one is not established; both golden vectors.
- `test_boundary_h_a1_gateway_establishes_mandates_end_to_end`, through the real `/a2a` door with a bearer token
  and a mandate-requiring policy:
  - no grant → not established, and **the skill is never reached**;
  - a valid grant → established, permitted, dispatched;
  - after a signed revocation checkpoint → refused, and the skill is not reached again.
- `mycelium-py/tests/test_mandate.py` and `mycelium-ts/tests/mandate.test.ts`: the same vectors.

**Not claimed.**
- The gateway is a route-level enforcement point (AE0 §7). An effect reached without passing through it is outside
  this, which is H7's job.
- The member-key view is the live identity view only under `compliance`. Without it, holders and authorities must
  be configured external issuers.

## 7. Wired at the wiki's git store (2026-09-25)

**The gap.** The wiki's mandate fence (`mycelium-wiki::mandate_fence`, scoped-mandates ADR §5) puts one check inside the
git transaction: is the appointment ref still the one this curator was configured with? That stops a **superseded**
curator. It cannot stop the others, because git has no clock it trusts:
- a curator whose mandate has **expired**;
- a curator **revoked** by a checkpoint, when nobody has moved the appointment ref yet;
- a curator that has **heard nothing** from its authority for longer than the freshness bound.

All three kept writing. The queue drained, compare-and-swap retries retried, and a publish pushed commits made earlier
under an authority that had since lapsed.

**The wiring.**
- **The seam** is `mandate_fence::WriteAuthority`, a one-method trait in the wiki's data plane, which has no Mycelium
  dependency. `GitStoreConfig::authority` holds one, and `None`, the default, is today's behaviour exactly.
- **Where it is asked.** `GitStore` asks it:
  - on **every commit attempt**, after the commit object is built and immediately before the `update-ref`
    transaction;
  - before **every push attempt**, including after a splice.

  So a retry, a round drained long after its proposals were queued, and a publish of earlier commits are each checked
  afresh. A commit made under authority does not carry that authority to the shared remote.
- **The implementation** is `ExecutionGateAuthority` (feature `execution-authority`): A1's `ExecutionGate` over the
  curator's mandate, which must enumerate `wiki.write`, plus a `RevocationView` fed by `offer_checkpoint`. The
  deployment supplies `now_ms`, so nothing reads a clock.
- **The refusal names itself.** `WikiError::authority_refused` is neither a conflict (retrying will not help) nor a
  gate refusal (the content is not at fault). The curator therefore **leaves the proposals queued** for a curator
  that does hold authority, and stops the round.
- **It composes with the fence.** The time check is made just before the transaction, and the fence's `verify`
  holds through it. Neither alone is the contract.

**Gates** (`mycelium-wiki/tests/git_store_authority.rs`):
- with no checkpoint, nothing is written, and no commit reaches any ref;
- the plant: with present authority, the same write lands;
- an expired mandate writes nothing, even with a fresh checkpoint;
- a revoked curator writes nothing, and a later checkpoint that omits the revocation restores nothing;
- silence past the freshness bound stops writes (at the bound it still writes, one millisecond past it does not), and
  the next checkpoint restores them;
- a superseded epoch writes nothing;
- a commit made under authority is not published after revocation, and the remote never receives it;
- end to end through the curator, proposals stay queued, not dropped, while there is no present authority, and land
  once the checkpoint arrives.

**Not claimed.**
- `FsStore` has no `WriteAuthority` seam yet.
- The window between the check and the ref transaction is bounded by local git subprocess time. It is covered for
  appointment moves by the fence's `verify`, and for expiry and revocation it is the same check-then-act distance
  any A1 resource has. The ADR's *s* absorbs a clock read. It does not absorb an unbounded stall, and no bound on
  subprocess time is claimed.
- The remote's side, which is whether it honours `--atomic` and whether a pre-receive hook re-checks, is unchanged
  from §5 of the scoped-mandates ADR.

## 5. Gates

`mandate::authority::tests` (17) and `a1_policy_tests` (1):
- the profile refuses bad parameters and advisory resources;
- safety at the extreme that understates age;
- liveness at the extreme that overstates age;
- a future-dated checkpoint is refused;
- a replayed checkpoint does not refresh freshness;
- silence and partition deny;
- a checkpoint for one scope does not refresh another;
- a revoked appointment is denied, and a later checkpoint that omits it does not reinstate it;
- a forged checkpoint is not accepted;
- a protected operation without a mandate is refused;
- queued and retried work is refused after expiry;
- delegated work cannot outlive its parent;
- work is clamped to its mandate's window;
- a re-authorising task fails its first check after expiry;
- drain bounds per continuation, and unbounded classes;
- the drain report keeps requested, acknowledged and confirmed apart;
- policy holes (an allowance without a mandate, including through `*`) are reported.
