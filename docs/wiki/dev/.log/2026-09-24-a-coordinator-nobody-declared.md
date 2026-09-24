## [2026-09-24] ingest | a coordinator nobody declared — P10, and the rule that was producing one

Up: [dev](../dev.md) · pages touched: [companions](../companions/companions.md) · records
`docs/design/legible-emergence-taxonomy.md` (P10), `docs/design/network-design-reproducibility-note.md`
(§5a answers its own §6) · code `src/election.rs`, `src/agent/emergent.rs`,
`src/agent/capability_ops.rs`, the three companions' elections.

A discussion note asked whether role accumulation is constrained anywhere. Answering it took an hour
and produced two findings, neither of which was the one the note expected.

## Finding 1 — it was not detected, which is the part that mattered

*Not prevented* was the expected answer and is not a defect: **detection, not prevention** is the
law here, so a missing `max_roles` is the design working. What should have existed is a tripwire.

None of the seven pathologies could see this one, and the reason is worth keeping because it
generalises: **P2 watches churn, P6 watches gaps, and concentration is the orthogonal axis.** A node
calmly holding every single-writer job produces *no* churn (nothing changes hands) and *no* gap
(everything has a provider — the same one). So the fleet reads as **perfectly healthy by every
measurement that exists**, right up until the node it all depends on goes away.

The general form, for the next detector someone writes: *a catalogue of pathologies is not a
catalogue of axes.* Seven detectors looking at two axes leave every other axis unwatched, and
nothing in the catalogue says which axes it covers.

## Finding 2 — it was the default, not an edge case

The tuple space, the blackboard and the wiki each elect by **lowest candidate node id wins**. Each
election is individually correct, and deterministic *on purpose* — that is what lets every reader
reach the same conclusion with no coordinator, so an outcome can be **checked rather than trusted**.

The same rule over the same candidates returns the same winner. So a fleet where every node runs
every companion put every single-writer job on one node — not eventually, not by drift, but on the
first election and again after every restart. And losing that node moved every role at once, **as a
block**, to the next-lowest id, so the load did not spread; it relocated.

## What landed

- **P10** (`detect_role_concentration`): the share of live `.primary` / `.curator` advertisements
  held by one node, hysteresis-confirmed, on `/stats` as `role_concentration_pct` and in the event
  ring. The gauge carries the share **whether or not it trips** — an operator watching 40 → 55 has a
  warning that a boolean withholds until it is already true.
- **The partition guard**, which is the subtle part: a node that has lost sight of its peers sees
  only its own roles, *which is the pathology's exact shape*. Without a floor of two visible
  holders, the first thing a partitioned node does is accuse itself of being a coordinator, and the
  operator is sent to the one place the problem is not.
- **`mycelium::election`**: rendezvous ordering (`hash(ring, node)`), so rings spread.

## The rollout, which was the actual difficulty

Changing an election rule is not changing an implementation detail. The companions' safety rests on
**every node computing the same answer** — the wiki's sentinel states it outright: *"the lowest
always sees itself as lowest and stays; every other steps down."*

Deploy a new rule node-by-node and that argument breaks: an old node and a new one each compute a
different winner, each believes it is the winner, and **neither resigns**. Not a transient — a
*stable* two-holder state lasting as long as the rollout, in the one place the design has no
coordinator to break the tie. (Section-CAS underneath keeps it to a conflict storm rather than
corruption. The sentinel exists because two curators happened once already.)

So the rule is **negotiated from the candidate set**: each candidate advertises what it can compute
(`election_rule`, an ordinary capability attribute — no new gossip, no new prefix), and every
elector takes the **minimum across live candidates**. A ring is only as new as its oldest member;
one old candidate pins the whole ring; the flip happens by itself when the last one leaves.

**What that does not remove**, stated because it would be easy to imply otherwise: nodes see the
candidate set converge at slightly different moments, so there is a window where two disagree about
the rule. It is bounded by *capability convergence* — seconds — instead of by the rollout, and it is
the same transient the sentinel already handles. A flag day's window is hours.

## Two smaller things worth keeping

**The fix is a spread, not a bound.** Three rings over three nodes still leave ~11% chance one node
wins all three and ~78% chance somebody holds two. Rendezvous is a large-numbers tool and the small
fleet is where it underdelivers — which is exactly why P10 stays. *Mitigate the cause, keep the
ability to see the residue.*

**`cap/` must be decoded with the fallback.** The first draft of P10 used `CapEntry::decode` alone
and would have silently skipped entries in the older bare-`Capability` encoding — reading as *that
role is not held* rather than *this reader is too new to parse it*. The codebase had the fallback
inline in three places and nothing named it; it is now `decode_cap_entry`, with the reason attached.
