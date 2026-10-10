# Positioning — the hero, the sentence, and its three expansions

This file is the **single source** for how Mycelium describes itself. Every front door quotes the
hero and the sentence below verbatim and links here; `scripts/check-positioning.sh` (in `make check`)
fails when a door's copy of either differs, when the front doors' routing structure breaks (§ *The routes and the
five-step path*), or when a link still points at the repository's pre-move account. Edit them **here and nowhere else**. Rationale and the list of
doors: [`docs/plans/proposition-alignment.md`](plans/proposition-alignment.md).

## The hero

<!-- hero -->
**A fleet that grows the capabilities it lacks, under rules it can show it kept.**

## The sentence

<!-- sentence -->
**Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays.**

The hero says what is distinctive in a dozen words: the fleet *grows* — a node that lacks a capability
acquires it — and it does so *under rules it can show it kept*, which is authority checked where the work
runs and a record someone else can read. The sentence makes each claim at the strength the code supports:

- *Embedded library, no coordinator* — the thesis, and the binding wording: a library, not a platform.
- *Generic nodes install signed capabilities when demand goes unmet* — the stem node and demand-driven
  provisioning (v2.17.0–v2.18.0). It is true of a **stem** — a node with a `[hosts]` table — not of every
  node; an SDK agent or a plain `mycelium` node does not install anything.
- *Re-heal what they declared when a provider dies* — presence floors: **what was declared**, not
  self-repair in general.
- *Where you configure an enforcement point* — gateway and provider enforcement need their features
  (`compliance`, `tls`), an attached evaluator and deployment configuration; a route no enforcement point
  fronts inherits nothing, and a declaration alone enforces nothing. Resource fencing has its own contract.
  The default configuration is a development posture — plaintext gossip trusting every peer, an open gateway
  on loopback if `http_port` is set (a non-loopback one refuses to start without a credential unless
  `gateway_allow_unauthenticated` is set), allow-all egress, no profile — and `profile = "secure-single-domain"` makes `start()`
  refuse unless the enforced posture holds.
- *It checks authority before work runs and records what it decided* — mandates and the action seam, with
  the evidence journal attached; an evaluator without a journal enforces and records nothing.
- *A recorded run replays* — replay checks a recording against its captured inputs and choices. It does not
  reproduce external effects, and a decision journal is not a replay bundle. Deliberately not *proves it by
  replay* (the 360 review, 2026-10-02).

It contains no word that [`philosophy.md`](philosophy.md) § *What This Architecture Is Not* forbids.
"Unique", "first" and any new-category claim stay reserved until the source-based prior-art comparison
exists ([`v3-contracts-axis.md`](plans/v3-contracts-axis.md) §13.1).

**Revised again the same day.** An interim wording — *Configured enforcement points check authority and can
record their decisions; supported replay seams make captured executions reproducible* — reached every door in
v2.18.1 (carried, unreviewed, by #474; see the changelog note). Every hedge in it was true, and the bullets
above keep them; three hedges in one clause read as a disclaimer on a front page, so the sentence now says
where the guarantee holds (*where you configure an enforcement point*) and what replay does (*a recorded
run replays*) without qualifying each verb.

**Revised 2026-10-02.** The previous sentence — *Mycelium lets a fleet of AI agents find, authorise and
account for each other's work with no coordinator, and proves it by replay* — said nothing about the fleet
acquiring what it lacks, which v2.17.0–v2.18.0 made true, and its *proves it by replay* claimed more than
replay shows. Its parts are not lost: *find* is the capability layer under *grows*; *authorise* and
*account* are the configured authority and evidence boundaries. The developer teaching pass
also removed the universal *every action* claim and separated evidence from replay coverage.

## The three expansions

Each is one paragraph and opens with the hero and the sentence.

**The visitor's** — README, the GitHub description, crates.io, docs.rs (on the README it sits below the
intent router, under *Is it for you?*, since the router is the first choice a reader makes):

> **A fleet that grows the capabilities it lacks, under rules it can show it kept.** Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays. It is built in three layers — a gossip
> KV store, a signal mesh, and epidemic consensus — with capability discovery across them: no
> broker, no registry, no daemon, no control plane. State converges by gossip; work is claimed, not
> dispatched; roles are discovered, not assigned. It is probably overkill if you want to chain a few
> LLM calls, run one orchestrator that fans out to workers, or coordinate through a database or queue
> you already operate; reach for a workflow engine or a broker there. It earns its keep when many
> agents must coordinate and nobody should be in charge: fleets that partition and heal, edge and
> on-prem meshes, systems where *who is in charge* must be emergent and recallable.

**What "no control plane" means** (realignment repairs A1, decision D2). Not that nothing controls
the fleet — the node runs governors, a provisioner and consensus. It means there is no *separate*
control-plane service to deploy or operate. The definition, used everywhere the phrase appears:
*Control decisions live in participating nodes: enabled governors and provisioners act locally, subject to configured consensus and resource-side authority checks. No separate Mycelium control-plane service is required.* "Enabled" is load-bearing: a minimal embed runs none of them, and a governor
enforces only under the control profile its node is set to.

**The buyer's** — both decks, the engagement kit, the pilot page — the hero and the sentence, then the order in
the plan's §3: what an agent **may** do, checked where the work happens · what it **did**, as records
a third party can audit · a fleet that **fills itself** · how: no coordinator · what is proven and what
is not ([`what-is-proven.md`](operations/what-is-proven.md)).

**The engineer's** — the guide, the integrator on-ramp, the crate doc — the hero and the sentence, then the map of
the contracts axis: *local decision-making · evidence-aware capability selection · scoped authority ·
bounded federation · coordination contracts tested through deterministic replay*, then the layer table.

## The routes and the five-step path

The first choice a reader makes is **by role and intent**, not by audience page. The rule, which
`scripts/check-positioning.sh` (with `scripts/check-front-doors.py`) enforces as structure and link targets, not prose:

- **The README carries the intent router** — between `<!-- router:start -->` and `<!-- router:end -->`,
  directly under the hero, the sentence and one plain-language line, four routes in this order:
  - **Build a fleet** → the developer guide (`docs/guide/README.md`), its tutorials
    (`docs/guide/tutorials/README.md`) and the five steps below the router;
  - **Run a fleet** → the operations door's journey (`docs/operations/README.md#operator-journey`),
    `production-readiness.md` and `observability.md`;
  - **See it work** → four curated examples, chosen on the examples page
    (`examples/README.md#what-do-you-want-to-see`): the mesh, self-provisioning, governed action and
    operational insight;
  - **Check the evidence** → `docs/operations/what-is-proven.md`.

  Below the router, a compact capability compass (`<!-- compass:start -->` … `<!-- compass:end -->`)
  points every row into [`capabilities.md`](capabilities.md); then the five steps.
- **The five steps are the developer's canonical path.** They appear, identical, in three places:
  the developer guide (`docs/guide/README.md`, the path's home for a reader), the README's *Build a fleet*
  route, and the examples page's *Learning path*. The check compares the text of the steps with link
  targets removed, since a relative link differs by directory.
- **The examples page leads with the chooser** — *What do you want to see?*, the same four examples
  as the README's *See it work* route (`<!-- chooser:start -->` … `<!-- chooser:end -->`) — then the
  learning path, then the full capability matrix.
- **The operations door has its own journey** — deploy, readiness, observation, diagnosis and recovery,
  under *Operator journey* — and does not carry the five steps; an operator's first five moves are not a
  developer's.

The five steps' text lives here — edit it here first; the check compares every copy against it:

<!-- path:start -->
1. **`hello_mesh`** — two embedded agents share state by gossip: 30 seconds, no setup.
2. **`hello_capability`** — one node says what it does, another finds it by name and calls it: no registry, no addresses.
3. **Your first stem fleet** — generic hosts discover and install a signed echo component, answer a call, and restore a declared provider floor after graceful removal.
4. **guide 20 with `authority_drain`** — what an agent may do, checked where the work happens, and stopped when its authority lapses.
5. **`what-is-proven.md`** — what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- path:end -->

The [documentation index](README.md) routes all four audiences (prospect, developer, operator, researcher) at greater length. `scripts/check-materials.py` checks that every local link on these pages resolves and that the capability map covers the guide.
