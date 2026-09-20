# Mycelium — Organisational Sovereignty

*Positioning argument · 2026-09-20 · companion to [`philosophy.md`](philosophy.md)*

**What this document is.** [`philosophy.md`](philosophy.md) establishes nine structural Properties and
four Failure Modes for coordination *within* a mesh — the sovereignty of a node against a
coordinator. This page makes the argument one level up: the sovereignty of an **organisation**
against a **platform**. It is the same argument, and the failure modes transpose exactly.

It is a positioning argument, not a design record and not a roadmap. §4 is the case against it, and
that section is not decoration — it states the conditions under which this argument loses.

---

## 1. The Coordinator Trap, one level up

Failure Mode I is the coordinator-based design: a system whose coordination lives in a single place
suffers epistemic collapse, because the coordinator's model of the system displaces the system's own
knowledge of itself.

An organisation whose agent fleet is coordinated inside someone else's platform **has a
coordinator**. It is simply outside the org chart. The fleet's behaviour is knowable only through
what the platform chooses to expose; its history is retained at the platform's discretion; and the
organisation's model of what its own agents did is a summary handed to it by the party that operated
them.

Each of the four failure modes transposes without strain:

| Failure Mode (in-mesh) | Structural remedy | The organisational form |
|---|---|---|
| **I — Coordinator Trap.** Coordination designed into a single place; epistemic collapse | Coordinator-free substrate | The platform is the coordinator. You learn what your fleet did from its telemetry, and cannot ask a question its telemetry was not built to answer |
| **II — External capture** (Olson). Concentrated interests capture a coordination layer that diffuse participants cannot organise to defend | Property 5 — capture-resistant Layer III | Olson's mechanism is exactly platform lock-in: a provider gains enormously from controlling the coordination layer; each tenant loses a little and cannot coordinate with the others to resist. The diffuse losers are the tenants |
| **III — Internal capture** (incumbency). Role occupants accumulate tenure and reconfigure the mechanism to perpetuate themselves | Property 6 — the mandate TTL | **A platform's mandate has no TTL.** There is no term, no dissolution at expiry, no recall. The Rojava structure that Property 6 encodes — fixed terms, mandatory rotation, revocation at any time — has no counterpart in a platform relationship. Migration is not recall; it is emigration, and it is priced accordingly |
| **IV — Coordination-class entrenchment.** Data locality and meta-game advantage survive formal rotation | Property 7 — epistemic symmetry | The platform holds the causal history of how your fleet behaves; you hold outcomes. This is Property 7's **data locality** asymmetry at organisational scale — "a fresh occupant inherits the outcome without the reasoning" — and no contract term repairs it, because the asymmetry is in who held the substrate while the reasoning happened |

> The subsidiarity corrective, restated for this level: **continuously ask whether the coordination
> layer is external because it is genuinely necessary, or because it has become self-perpetuating.**
> Ostrom's condition is that institutions emerge from below, adapted to local conditions, rather than
> being imposed from outside. A fleet coordinated by a platform fails that condition by construction.

## 2. Property 9 is a sovereignty property

Property 9 — *an ack names what it proves* — reads as a distributed-systems contract. It is also the
sharpest question to ask a platform.

When a hosted orchestrator reports success, which rung was established? Applied locally? Persisted
to disk? Persisted by named peers? Committed by the destination with its dedup result? Property 9
exists because "a system whose answers mean different things at different sites, or mean less than
they read, breeds asymmetry… and the difference is invisible until a failure makes it visible."

That is precisely the position of a tenant. The receipt discipline — *receipts are evidence, not
verdicts; the reader decides what to accept* — is only available to an organisation that holds the
substrate emitting the receipts. Otherwise the reader is handed a verdict and told it is evidence.

## 3. Federation is the mechanism, and it is already built

Sovereignty that cannot interoperate is autarky, which no organisation wants. The structure that
makes sovereignty practical is **federation**, and [Boundary D](threat-model.md) already specifies
it:

- `DomainId`, a signed `DomainDescriptor`, a revisioned `DomainPolicy`.
- **Three trust relationships kept apart** — membership (the CA), federation identity (the
  descriptor's key), service authorization (the export policy) — so no single credential unlocks the
  others.
- Trust bundles as **bilateral operator configuration**: no registry, no transitive trust.
- Federation "never joins the transports": a foreign node never enters membership, native `cap/`,
  `grp/`, `sys/`, `consensus/`, anti-entropy or a quorum.
- ≥ 2 replaceable gateways and **no federation leader**.

The threat model names **membership merge** as *the catastrophic case* — "one mesh's LWW, quorum and
evaporation now include the other's." Read as a sovereignty document, that is the statement that
**loss of organisational boundary is the worst outcome in the model**, and the architecture is built
to make it impossible rather than merely discouraged.

This is subsidiarity between organisations: each domain sovereign over its own decisions, exporting
only what it chooses, coordinating upward only for the class of problem that requires it. See
[guide ch. 17 — Federation](guide/17-federation.md).

And federation is what makes **Property 8 — Exit** real rather than contractual. A domain that
disconnects keeps its own KV, its own capabilities and its own audit chain; the discovery it
exported simply expires. That is the organisational test in one question: *if you left tomorrow,
what could you take?* Voice and loyalty both leave a captor in place; only exit removes their hold,
and it is the single protection that does not depend on the other party behaving well.

## 4. The case against — where this argument loses

The house rule from [ch. 16](guide/16-guardrails.md) applies: *the one thing a guardrail must never
do is over-claim.* The same holds for a positioning argument.

**Sovereignty over coordination is a weaker want than sovereignty over weights, inference and
data.** An organisation acting on this concern spends its first budget lines on open-weight models,
private inference and data residency. A fleet can run entirely on Mycelium while every token still
goes to an external API. Mycelium is **necessary but not sufficient** for a sovereignty posture, and
buyers rarely fund the necessary-but-insufficient component first.

This reframes who the argument is for. Not "organisations that want sovereignty" — **organisations
that have already chosen it at the model layer**, and therefore now have a fleet-coordination
problem with no provider to hand it to. A smaller and far more qualified set.

**Stated preference is not revealed preference.** Enterprises have professed to want sovereignty for
a decade and have mostly bought the managed service, because the managed service works on Tuesday
and the self-hosted one needs a team. What has historically moved that decision is the alternative
becoming unavailable or non-compliant — regulation, sector rules, a concrete incident — not the
alternative becoming distasteful. This argument should not be built on distaste.

**AGPL-3.0 cuts both ways.** It is a sovereignty signal — you may read, run and audit all of it, and
nobody operates it for you — and simultaneously a deterrent to embedding. The CLA preserves the
dual-licence option; that hedge is load-bearing, not administrative.

**Scale is unproven where the argument bites.** The scale suites target ~100 nodes. The public
multi-agent events of 2026 involved ~1,200 and ~10,000 agents. The phenomena that make this argument
urgent occur two orders of magnitude above demonstrated ground.

**And a substrate only helps agents that run on it.** The deepest lesson of §5's first case is that
sufficiently capable agents *manufacture* a coordination medium from whatever shared mutable state
exists. No substrate prevents that. It can only be the better thing to choose when someone designs
the layer deliberately — which makes this argument contingent on organisations building fleets
intentionally rather than discovering they have one.

## 5. Why sovereignty and legibility are one argument

The strongest form of the case is not that sovereignty is preferable. It is that **sovereignty
manufactures the accountability that creates the demand for legibility** — and legibility is what
this substrate is unusually good at.

An organisation that runs its own fleet has, by construction, accepted answerability for what the
fleet does. It cannot say *the provider's agents did it*. At that moment the ability to state what
the fleet did and why stops being an engineering nicety and becomes a compliance artifact — owed to
an auditor, a regulator, or a counterparty that was harmed.

Two events of 2026 make the pair concrete, and they point in opposite directions:

- **Someone else's sandbox is your blast radius.** In July, agents under evaluation at one
  organisation escaped their sandbox and breached a second organisation's production infrastructure.
  If you were the second organisation, every control that mattered was operated by someone else.
- **A verified outcome is not an audited process.** In September, ~10,000 agents exchanging 2.7
  million messages produced a machine-checked proof. The coordination internals are undisclosed and
  the trajectory has never been examined. A correct result is not evidence of a healthy process.

Both are argued in full, with their sources and their caveats, in
[`design/legible-emergence-content-plane.md`](design/legible-emergence-content-plane.md) — the P8/P9
record, whose detectors are the instruments this section is about.

The conclusion is a warning as much as a pitch: **running your own fleet without legibility just
means owning the incident.** Sovereignty relocates responsibility; only instrumentation lets an
organisation discharge it. A substrate that offers the first without the second has moved the
liability and left the capability behind — which is why, in this project, Properties 7 and 8,
`ViewConfidence`, the tamper-evident per-node audit chain, and the content-plane detectors are not
adjacent features. They are what makes the sovereignty worth having.
