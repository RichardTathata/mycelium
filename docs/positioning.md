# Positioning — the hero, the sentence, and its three expansions

This file is the **single source** for how Mycelium describes itself. Every front door quotes the
hero and the sentence below verbatim and links here; `scripts/check-positioning.sh` (in `make check`)
fails when a door's copy of either differs, when a funnel's five-step path differs, or when a link still points at the
repository's pre-move account. Edit them **here and nowhere else**. Rationale and the list of
doors: [`docs/plans/proposition-alignment.md`](plans/proposition-alignment.md).

## The hero

<!-- hero -->
**A fleet that grows the capabilities it lacks, under rules it can show it kept.**

## The sentence

<!-- sentence -->
**Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Configured enforcement points check authority and can record their decisions; supported replay seams make captured executions reproducible.**

The hero says what is distinctive in a dozen words: the fleet *grows* — a node that lacks a capability
acquires it — and it does so *under rules it can show it kept*, which is authority checked where the work
runs and a record someone else can read. The sentence makes each claim at the strength the code supports:

- *Embedded library, no coordinator* — the thesis, and the binding wording: a library, not a platform.
- *Generic nodes install signed capabilities when demand goes unmet* — the stem node and demand-driven
  provisioning (v2.17.0–v2.18.0). It is true of a **stem** — a node with a `[hosts]` table — not of every
  node; an SDK agent or a plain `mycelium` node does not install anything.
- *Re-heal what they declared when a provider dies* — presence floors: **what was declared**, not
  self-repair in general.
- *Configured enforcement points check authority and can record their decisions* — mandates
  and the action seam constrain covered routes. A node must attach the applicable evaluator,
  authority and evidence journal; uncovered routes do not inherit the guarantee.
- *Supported replay seams make captured executions reproducible* — replay checks a recording
  against captured inputs. Evidence records and receipts are not automatically replay bundles,
  and arbitrary application executions are not automatically covered.

It contains no word that [`philosophy.md`](philosophy.md) § *What This Architecture Is Not* forbids.
"Unique", "first" and any new-category claim stay reserved until the source-based prior-art comparison
exists ([`v3-contracts-axis.md`](plans/v3-contracts-axis.md) §13.1).

**Revised 2026-10-02.** The previous sentence — *Mycelium lets a fleet of AI agents find, authorise and
account for each other's work with no coordinator, and proves it by replay* — said nothing about the fleet
acquiring what it lacks, which v2.17.0–v2.18.0 made true, and its *proves it by replay* claimed more than
replay shows. Its parts are not lost: *find* is the capability layer under *grows*; *authorise* and
*account* are the configured authority and evidence boundaries. The developer teaching pass
also removed the universal *every action* claim and separated evidence from replay coverage.

## The three expansions

Each is one paragraph and opens with the hero and the sentence.

**The visitor's** — README, the GitHub description, crates.io, docs.rs:

> **A fleet that grows the capabilities it lacks, under rules it can show it kept.** Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Configured enforcement points check authority and can record their decisions; supported replay seams make captured executions reproducible. It is built in three layers — a gossip
> KV store, a signal mesh, and epidemic consensus — with capability discovery across them: no
> broker, no registry, no daemon, no control plane. State converges by gossip; work is claimed, not
> dispatched; roles are discovered, not assigned. It is probably overkill if you want to chain a few
> LLM calls, run one orchestrator that fans out to workers, or coordinate through a database or queue
> you already operate; reach for a workflow engine or a broker there. It earns its keep when many
> agents must coordinate and nobody should be in charge: fleets that partition and heal, edge and
> on-prem meshes, systems where *who is in charge* must be emergent and recallable.

**The buyer's** — both decks, the engagement kit, the pilot page — the hero and the sentence, then the order in
the plan's §3: what an agent **may** do, checked where the work happens · what it **did**, as records
a third party can audit · a fleet that **fills itself** · how: no coordinator · what is proven and what
is not ([`what-is-proven.md`](operations/what-is-proven.md)).

**The engineer's** — the guide, the integrator on-ramp, the crate doc — the hero and the sentence, then the map of
the contracts axis: *local decision-making · evidence-aware capability selection · scoped authority ·
bounded federation · coordination contracts tested through deterministic replay*, then the layer table.

## The five-step path

Every funnel (`README.md`, `docs/guide/README.md`, `examples/README.md`, `docs/operations/README.md`)
shows these five steps, in this order, above anything else it recommends. The check compares the
text of the steps with link targets removed, since a relative link differs by directory.

<!-- path:start -->
1. **`hello_mesh`** — two embedded agents share state by gossip: 30 seconds, no setup.
2. **`hello_capability`** — one node says what it does, another finds it by name and calls it: no registry, no addresses.
3. **Your first stem fleet** — generic hosts discover and install a signed echo component, answer a call, and restore a declared provider floor after graceful removal.
4. **guide 20 with `authority_drain`** — what an agent may do, checked where the work happens, and stopped when its authority lapses.
5. **`what-is-proven.md`** — what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- path:end -->
