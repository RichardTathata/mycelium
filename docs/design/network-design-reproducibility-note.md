# Discussion note — reproducible topology: what a network-design result does and does not imply for us

**Status:** 💬 **discussion note, not a decision and not a plan.** 2026-09-20. Circulated for the
team's view. Nothing here is proposed for implementation yet; §6 is a list of questions, not a
backlog. If the answer to §6.1 is "we already handle that," this note should be closed.

**Source.** J. van der Kolk, C. Glover, A.-L. Barabási, *Design Principles for Reproducible
Networks*, arXiv:2609.03852v1 [cond-mat.dis-nn], 3 September 2026, 116pp.

---

## 1. What the paper does

It asks when **local constraints on components guarantee a unique global structure**. A *design set*
D is (node types, a binding matrix **O** saying which types may connect to which, a capacity vector
**C** giving each node's number of connections). Assembly is **unigraphical** when D admits exactly
one final network — and crucially, when it does, that outcome is *"independent of the dynamics of
assembly."* Any process that respects the constraints and settles produces the same graph.

Where a design set does *not* fix a unique outcome, they identify a second route: **guided
assembly**, in which temporal ordering decomposes construction into unigraphical steps.

The empirical result is the part worth our attention. Across 3,618 real reproducible systems
(protein complexes, molecules, circuits, robots, furniture, image fragments): **813 assemble
unigraphically. 2,805 do not.** Roughly **22% get reproducibility from constraint; 78% get it from
ordering.**

## 2. What I read, and what I did not

Read (~25 of 116pp):

- Main text: abstract, introduction, the design-set formalism, the Unigraphical Design Theorem and
  its specificity parameter ψ.
- **SI S3.1.3** — the UDT for semi-specific design sets, including Theorem 7 and its proof sketch.
- **SI S4 / S4.1** — guided assembly: the relaxation of the saturation assumption, the design
  protocol and assembly-tree definitions, Proposition 8 (every tree is guided-assemblable),
  Proposition 10 (cycles), and the 2-core/appendage decomposition.

**Not read:**

- **SI S3.2** — the Mixed Integer Linear Programming formulation for design sets the analytic
  conditions cannot settle (needed for ~8% of their cases).
- **SI S5.3** — the diversity–redundancy boundary. This is the section on trading component variety
  for structurally interchangeable parts while retaining unique assembly. It is probably the closest
  to our concerns of anything I have not read, and anyone picking this up should start there.
- SI S1 (datasets), S2, S5.1–5.2, S6 (the 3D-printing experiment).
- The full proofs of Theorems 1–6 and Propositions 9, 11–15.

**Verified in our code:** `advertise_capability` and `advertise_roles` carry no count limits; no
`max_roles` / `max_caps` equivalent exists; `MembershipIntent{min,max}` bounds group population, not
per-node role count; `max_peers`, `max_connections` and `gossip_fanout` bound *transport* degree.
**Not verified:** whether role accumulation is constrained indirectly somewhere I did not look —
see §6.1, which is the question this note most needs answered.

## 3. The homology test, applied

Because this is a cross-domain import, it was run against the criteria in *Heterogeneous Local
Knowledge Systems* (zenodo.20813058) rather than adopted on resemblance.

| Criterion | Result |
|---|---|
| **Independence of derivation** | **Strong pass.** Derived from unigraph theory, tested on proteins and circuits, with no contact with distributed systems, Hayek, Ostrom or coordination theory. On HLKS's own reasoning this is a fifth independent derivation, and the independence is the evidence. |
| **Shared problem-class under stated assumptions** | **Fails for the live mesh; holds in a bounded regime.** HLKS assumes state-change rates δ bounded away from zero — that assumption is what makes a coordinator's estimate stale. This paper assumes a terminal state (saturation, relaxed in S4 to *maximality*: no further compatible edge can be added). **A system with δ > 0 never reaches one.** The frameworks contradict each other on the load-bearing variable. They coincide only over a *stated observation period with stated membership* — a capability group between membership changes, a federation edge, a deployment's 2-core. We already have that vocabulary in the P8/P9 record. |
| **Structural identity, stated in what respect** | **Mostly holds, one empty row.** Node type ↔ advertised capability. Binding matrix **O** ↔ capability filters and boundary admission. Final states ↔ maximal configuration over an observation period. Specificity ψ ↔ how tightly predicates constrain binding. Automorphism condition ↔ role-assignment ambiguity. **Capacity vector C ↔ nothing.** See §4.1 — the empty row is the finding. |
| **Mechanisms must not transfer** | **Passes only under discipline.** HLKS's rule is *"the pattern recurs; the mechanisms do not transfer."* We may claim the constraint recurs. We may not import unigraphs, assembly trees or the MILP formulation as our machinery, any more than our substrate implements a price mechanism because Hayek was right. |

## 4. Three candidates for discussion

Each is justified below **in our own terms**. The paper is what made them visible; it is not the
argument for them. If any of these is worth doing, it should be arguable without citing Barabási at
all — and each is.

### 4.1 Capability degree is unbounded, and that is a coordinator forming by accretion

The empty row in the identity table is not a mapping failure. It is a gap in the architecture.

We bound transport degree (`max_peers`, `max_connections`, `gossip_fanout`). Nothing bounds how many
**roles** a node accumulates. One node may hold blackboard primary, tuple-space curator, gateway and
N skills at once, and no structural rule prevents it.

That node is a coordinator. Nobody declared it; it arrived by accretion. And **Property 5 has no
teeth at the topology layer** — capture resistance is defined over the binding-commitment mechanism,
not over role concentration. Olson's argument does not require a ballot if one node ends up holding
everything that matters, and Property 6's mandate TTL rotates *who* holds a role without bounding
*how many* roles one holder may accumulate.

`MembershipIntent{min,max}` bounds group population. There is no `RoleIntent{max}`.

**This is the candidate I would act on**, and the one most likely to be already-answered by someone
who knows the capability system better than I do.

### 4.2 There is no assembly-ordering concept

Layer III is justified in the docs as *for operations requiring linearisable cross-node agreement* —
a consistency argument. The 22/78 result suggests ordering is also the dominant route to
**reproducible structure**, which is a different claim and a stronger one.

We have no notion of a bring-up order: which capability groups must form before which, so that the
resulting topology is determined rather than incidental. Bootstrap ordering today is emergent.

The independent justification is ordinary operational experience: bootstrap order decides what you
get, and we currently neither state it nor check it. Whether the answer is a new primitive or a
documented discipline is a real design question this note cannot settle.

### 4.3 No notion of expected topology — weakest, noted for completeness

We can say what the mesh *is* (`/gateway/fleet`, the P1–P9 detectors) and what is pathological about
it. We cannot say what it *should be* given a configuration. That is the "staging and prod have
identical configs and behave differently" gap.

Ranked lowest because it pulls against adaptivity — a mesh with one legal topology is brittle, and
losing a node could leave *zero* legal topologies rather than a degraded one — and because the
bounded-regime constraint confines it to a stated observation period.

## 5. Explicitly out of bounds

- **Aiming for a unigraphical mesh.** It is the minority route (22%) even in nature, and uniqueness
  fights adaptivity.
- **Computing a design set over the live mesh.** The homology test forbids it outside a stated
  observation period, and we have no capacity vector to compute it from.
- **A P10 built on UDT machinery**, or Theorem 7 as a detector. That is mechanism transfer. Role
  ambiguity may still be worth naming as a hazard — but on its own merits, not because a theorem in
  another field says so.

## 5a. Answers — questions 1, 2 and 3, resolved 2026-09-24

**Q1: is role accumulation constrained anywhere I did not look?** *No.* Four places were checked:
`advertise_roles` takes an unbounded iterator and writes one signed claim; `advertise_capability`
has no count limit and no `max_roles` equivalent exists in the workspace; `MembershipIntent{min,max}`
bounds a **group's population**, never a node's role count; and `InstallRights::admit_install` — the
only mechanism that counts a per-node allocation and refuses — bounds *artifact installs*, not roles,
and only under `EnforceAllocated`. §4.1 stands.

**And it was worse than the note supposed, in a way the note could not have seen.** The absence of
*prevention* is not the problem — "detection, not prevention" is this substrate's law, so a missing
cap is expected. What should have existed is a tripwire, and **none of the seven pathologies could
see this one**: P2 watches role *churn* (a node calmly holding everything produces none) and P6
watches coverage *gaps* (here every capability has a provider, merely the same one). Both are the
orthogonal axis. A concentrated fleet read as **perfectly healthy by every measurement we had**.

Worse still, concentration was not an accident waiting to happen but the **default behaviour**: the
tuple space, the blackboard and the wiki each elect by *lowest candidate node id wins*, and the same
rule over the same candidates returns the same winner. A fleet where every node runs every companion
put every single-writer job on one node, deterministically, on first election and again after every
restart.

**Q2: accepted threat, or gap?** *Gap* — and now closed on both halves:

- **Detection:** **P10, "coordinator by accretion"** (`docs/design/legible-emergence-taxonomy.md`) —
  the share of live single-writer roles held by one node, hysteresis-confirmed, on `/stats` as
  `role_concentration_pct` and in the event ring. It carries a **partition guard**: a node that has
  lost sight of its peers sees only its own roles, which is the pathology's exact shape, so the
  reading is withheld below two visible holders. Absence of evidence is not evidence.
- **Mitigation:** `mycelium::election` — rings now order candidates by `hash(ring, node)`
  (rendezvous), so different rings pick different winners, and a failover spreads instead of moving
  every role to the next-lowest id **as a block**.

**Q3: would a `RoleIntent{max}` be coherent?** Not needed, and the note's instinct that it "fights
the capability system's premise" is right — a cap is an *assignment* mechanism in a system whose
premise is discovery. Rendezvous achieves the spread **without anyone assigning anything**: it is
still a pure, total, deterministic ordering every node computes from data it already holds. Nothing
is allocated; the answer is derived, as before.

**What is deliberately *not* claimed.** Rendezvous is a spread, **not a bound**: three rings over
three nodes still leaves ~11% chance one node wins all three and ~78% chance somebody holds two. It
makes concentration unlikely, not impossible — which is exactly why P10 stays. Mitigate the cause,
keep the ability to see the residue.

**Questions 4 and 5 remain open** (bootstrap ordering as an unwritten discipline; SI S5.3 on
diversity–redundancy). This section answers what the code could answer.

## 6. Questions for the team

1. **Is role accumulation constrained anywhere I did not look?** If a node cannot in practice hold
   blackboard primary and gateway and tuple-space curator simultaneously, §4.1 dissolves and this
   note is mostly closed. This is the question that matters most.
2. If it is *not* constrained — is an unbounded-role node a threat we already accept knowingly (in
   which case it belongs in the threat model's §4, "what the substrate does NOT defend against"), or
   a gap?
3. Would a `RoleIntent{max}` be coherent with the governor model, or does bounding role count fight
   the capability system's premise that roles are discovered rather than assigned?
4. Is bootstrap ordering already a discipline somewhere — a runbook, a convention — that simply is
   not in the architecture docs?
5. Does anyone want to read SI S5.3 (diversity–redundancy) and report back? It is the section most
   likely to say something useful about capability redundancy that this note has missed.

---

*Note prepared with AI assistance (Claude Opus 5). The paper reading, the homology test and the code
checks in §2 were done as described; the §2 statement of what was not read is exhaustive to the best
of my knowledge and should be treated as the confidence bound on everything above.*
