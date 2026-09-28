# Positioning — the one sentence, and its three expansions

This file is the **single source** for how Mycelium describes itself. Every front door quotes the
sentence below verbatim and links here; `scripts/check-positioning.sh` (in `make check`) fails when a
door's copy differs, when a funnel's five-step path differs, or when a link still points at the
repository's pre-move account. Edit the sentence **here and nowhere else**. Rationale and the list of
doors: [`docs/plans/proposition-alignment.md`](plans/proposition-alignment.md).

## The sentence

<!-- sentence -->
**Mycelium lets a fleet of AI agents find, authorise and account for each other's work with no coordinator, and proves it by replay.**

*Find* is discovery, the first half of the story. *Authorise* is mandates and the action seam, the
second half. *Account* is receipts and evidence. *No coordinator* is the thesis. *Proves it by replay*
is the method, and the one claim that needs the seams to make. It contains no word that
[`philosophy.md`](philosophy.md) § *What This Architecture Is Not* forbids. "Unique", "first" and any
new-category claim stay reserved until the source-based prior-art comparison exists
([`v3-contracts-axis.md`](plans/v3-contracts-axis.md) §13.1).

## The three expansions

Each is one paragraph and opens with the sentence.

**The visitor's** — README, the GitHub description, crates.io, docs.rs:

> Mycelium lets a fleet of AI agents find, authorise and account for each other's work with no
> coordinator, and proves it by replay. It is an embedded Rust library in three layers — a gossip
> KV store, a signal mesh, and epidemic consensus — with capability discovery across them: no
> broker, no registry, no daemon, no control plane. State converges by gossip; work is claimed, not
> dispatched; roles are discovered, not assigned. It is probably overkill if you want to chain a few
> LLM calls, run one orchestrator that fans out to workers, or coordinate through a database or queue
> you already operate; reach for a workflow engine or a broker there. It earns its keep when many
> agents must coordinate and nobody should be in charge: fleets that partition and heal, edge and
> on-prem meshes, systems where *who is in charge* must be emergent and recallable.

**The buyer's** — both decks, the engagement kit, the pilot page — the sentence, then the order in
the plan's §3: what an agent **may** do, checked where the work happens · what it **did**, as records
a third party can audit · a fleet that **fills itself** · how: no coordinator · what is proven and what
is not ([`what-is-proven.md`](operations/what-is-proven.md)).

**The engineer's** — the guide, the integrator on-ramp, the crate doc — the sentence, then the map of
the contracts axis: *local decision-making · evidence-aware capability selection · scoped authority ·
bounded federation · coordination contracts tested through deterministic replay*, then the layer table.

## The five-step path

Every funnel (`README.md`, `docs/guide/README.md`, `examples/README.md`, `docs/operations/README.md`)
shows these five steps, in this order, above anything else it recommends. The check compares the
text of the steps with link targets removed, since a relative link differs by directory.

<!-- path:start -->
1. **`hello_mesh`** — two embedded agents share state by gossip: 30 seconds, no setup.
2. **`hello_capability`** — one node says what it does, another finds it by name and calls it: no registry, no addresses.
3. **the co-op `provisioning` demo** — a fleet fills an unmet need itself: a node pulls, verifies and serves a capability nobody deployed, and re-heals when it dies.
4. **guide 20 with `authority_drain`** — what an agent may do, checked where the work happens, and stopped when its authority lapses.
5. **`what-is-proven.md`** — what CI proves on every merge, what is demonstrated with its bound stated, and what is not yet shown.
<!-- path:end -->
