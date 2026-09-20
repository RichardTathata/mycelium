# AE2 — enforcement at the resource, and what its strength depends on (ADR, v3 contracts axis AE slice)

**Status:** adopted 2026-09-20 · **AE2** of `docs/plans/v3-contracts-axis.md` §6.8, Phase D. Builds on
[AE0](action-envelope-ae0.md) (the contract), **AE1** (the envelope binds a scoped mandate — shipped in
v2.10.0), item 5's `ResourceAuthority` fence, item 4's rights for budgeted actions, and item 6's replay
harness. A contract, not an API: the code lands as a resource-side enforcement point, with the policy
adapter staying in the private companion. No wire change, no KV prefix, no daemon.

> Posture, once: every guarantee below is stated at the strength it has (rule 6). **AE2's central claim is
> that this strength is not uniform** — it is a function of the protected resource's own atomicity, and an
> enforcement point that cannot say which tier it is in has not met this record.

---

## 1. What AE-T and AE1 left open, precisely

AE-T enforces at the **gateway**, as a route-level preflight, and says so in every artefact it ships
(`coverage.complete: false`). AE1 gave the envelope a verified mandate binding and put a fence ahead of
policy, so a refused mandate cannot be laundered into a permit by any evaluator.

Both share one limit, and it is the whole of AE2's reason to exist:

> **A preflight decides at time T0 about an effect that happens at T1.**

Everything AE2 must close lives in that gap. A process that reaches the resource without traversing the
gateway is outside AE-T's guarantee entirely; and even one that *does* traverse it was authorised against a
world that may have changed by the time the effect lands.

## 2. The enforcement point is the resource, not the route

AE2 moves the decision to the component that owns the effect. Concretely, in Mycelium terms, that is the
**provider** — the process holding the resource and serving the request, which is also the only place that
can make the check and the effect happen together.

This is not a relocation of the same guarantee; it is a different and stronger one, and the difference has a
name: the gateway can refuse a *request*, the resource can refuse an *effect*.

**The gateway's preflight is retained, not replaced.** It is cheap, it refuses the easy cases early, and it
is where evidence of a refusal-before-dispatch is generated. AE2 adds a second check at the resource. Two
checks are not redundancy here — they answer different questions at different times.

## 3. The check/use race, and the rule that closes it

Item 5 already settled this for the wiki store, and AE2 generalises it. From
[chapter 21](../guide/21-mandates.md):

```text
start
verify <mandate-ref> <expected>
update <content-ref> <new> <old>
prepare
commit
```

> Checking the mandate and *then* writing would leave a window in which a concurrent appointment lands
> between the two. So the verify is inside the transaction, which means the mandate is checked **through
> commit**.

**AE2's rule.** The resource re-checks `ResourceAuthority::check` — epoch first — **inside whatever
transaction the effect uses**, and refuses rather than downgrading when the resource offers no such
transaction. The envelope's `MandateBinding` from AE1 is the *preflight's* finding; it is evidence of what
was decided, never a substitute for the check at the effect. An enforcement point that trusts the binding
instead of re-checking has reintroduced the race it was built to close.

## 4. Strength is a function of the resource's atomicity

This is the part this record exists to fix in place. Three tiers, declared per enforcement point, in the
guardrails vocabulary:

| Tier | The resource offers | Strength | What the operator is told |
|---|---|---|---|
| **Transactional** | a transaction the fence check can sit inside (git `--atomic` + `--force-with-lease`; a CAS; a lease with a fencing token) | *HardPrevention* | the effect cannot land under a superseded mandate |
| **Serialised** | no transaction, but a single owner that orders check and effect with nothing interleaved | *SelfImposedPrevention* | the window is closed by the owner's own discipline, and a second writer voids it |
| **Advisory** | neither — an external API with no boundary we control | *Detection only* | **never labelled prevention**; the effect may land after revocation and the record must say so |

§6.8 is explicit about the third: *"For an external API without such a boundary, state the weaker guarantee
and use item 1's adapter contract; never label it hard prevention."* The failure this prevents is an
adopter reading one word — "enforced" — across three materially different situations.

**A resource that cannot name its tier does not get a strict profile.** Fail closed, as item 5 already does:
*"a remote that does not honour atomic updates is not a supported strict-profile remote, and the write is
refused rather than downgraded."*

## 5. The five behaviours, and what each must show

AE2's gate is that these **replay deterministically** (item 6), not merely that they pass once. Each names
the wrong answer it exists to prevent:

| Behaviour | The effect must be | The wrong answer it prevents |
|---|---|---|
| **check/use race** | refused — a mandate superseded between preflight and commit does not land | a permit issued at T0 honoured at T1 |
| **duplicate attempts** | applied once — same `operation_id`, a second `attempt_id` is idempotent | one intent, two effects |
| **stale holder** | refused as `Superseded`, by name | a refusal that reads as a transport error and invites retry |
| **timeout** | `DeliveryUnknown`, exposure **retained** | a timeout read as "no effect", so a new attempt is issued against rights that were never released |
| **partition** | refused or unknown, never silently permitted | a partitioned resource deciding it is authoritative |

Two are borrowed rather than reinvented, which is the point: **duplicate attempts** is item 1's
`operation_id`/`attempt_id` idempotence, and **timeout** is RA's rule that a new attempt needs new rights
and never recycled ones. AE2 adds no parallel ledger.

## 6. What replays, and why that is the gate

A test that passes once tells you the code did the right thing on one interleaving. The five above are all
*races*, so the interleaving is the thing under test, and item 6's harness is what makes a particular one a
file rather than a memory.

Each behaviour ships as a **recorded bundle** with a witness assertion, so a fix can be shown to address the
recorded failure rather than a re-imagined one. Per item 6's own rule, a bundle with an unknown witness
cannot prove its own fix.

## 7. Where the mandate fence should finally live

AE1 implements the fence separately in the reference evaluator and in the Cedar adapter, and says at both
call sites that this is a property of the **contract** rather than of either adapter. That duplication was
accepted deliberately: until a resource-side enforcement point exists, there is nothing to hoist it into.

**AE2 is that enforcement point**, and it is where the fence moves — checked once, before any evaluator is
consulted, so a replaceable evaluator cannot forget it. An adapter keeping its own copy is then belt-and-
braces rather than the only barrier.

## 8. What this record refuses

- **A uniform "enforced" claim.** Three tiers, declared, or no strict profile.
- **Trusting AE1's binding at the effect.** It is the preflight's finding, and re-checking is not optional.
- **A second identity, ledger or receipt vocabulary.** Item 1's identities and receipts, item 5's
  `MandateRefusal`, item 4's rights — AE2 composes, it does not mint.
- **Calling a passing test a closed race.** Without a replayed bundle, a race test is an anecdote.
- **Blocking on a policy engine.** The fence is not a policy input; an unreachable evaluator yields
  `Indeterminate`, which the secure profile refuses — it never permits.

## 9. What lands next

AE2's implementation (the protected-service enforcement point and the selected policy adapter) in the
private companion; the five recorded bundles; then **AE3**, which is the evidence half — correlated
declaration, decision, execution and outcome records, and what an honest gap looks like when the effect's
outcome is unknown.
