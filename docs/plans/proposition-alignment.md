# Proposition alignment: one sentence, one proof page, one path (plan)

**Status:** ✅ **executed 2026-09-28** (rev 0.2 — P1–P5 in one PR, #434, the same day the plan was written; the
A12 line corrected: performance *is* micro-benchmarked, what is missing is a figure under load). A documentation and
positioning plan; no code changes except one check script. Follows a step-back review of the story as told by the front doors, made after
[`design-time-tooling.md`](design-time-tooling.md) reached rev 0.8.

> Posture, once: the system is one thing and the front doors tell it three ways. A buyer meets *mesh
> runtime* on the README, *no coordinator* in the FAQ and *evidence-aware authority* in the deck, and
> a reviewer finds the proof gaps stated honestly but in six places. This plan picks one sentence, puts
> the gaps on one page, shortens the path, and adds the check that stops the drift coming back.

---

## 1. What the review found (each line read, 2026-09-28)

| # | Artifact | What it says now | Verdict |
|---|---|---|---|
| A1 | `README.md` line 3 | *A broker-less mesh runtime for AI agent fleets* | altitude 1 of 3; no authority or evidence in the first paragraph |
| A2 | `docs/guide/faq.md` §Is Mycelium for me | *coordination at scale, without a single point of control* — and the best paragraph in the repo on who should **not** use it | altitude 2; the sweet-spot text belongs on the README |
| A3 | `docs/plans/v3-contracts-axis.md` §13.1 | *a distributed coordination substrate for autonomous agents, combining local decision-making with evidence-aware capability selection, scoped authority and bounded federation; its coordination contracts are tested through deterministic replay* | altitude 3; the truest sentence, in a plan nobody outside reads |
| A4 | `Cargo.toml` `description` | *Broker-less gossip substrate for AI agent fleets: KV store, signal mesh, consensus, capability discovery* | a parts list; crates.io shows it |
| A5 | `src/lib.rs` line 1–3 | *gossip substrate for adaptive AI agent systems … provides two primitives* | **wrong**: three layers plus capabilities; docs.rs shows it |
| A6 | GitHub repository description | *Epidemic gossip mesh — KV store, signal/boundary layer, bulk transfer (Layers 1-4)* | **stale by a year**: predates consensus, capabilities and every release; the first line a visitor reads |
| A7 | `mycelium-py/README.md`, `mycelium-ts/README.md` line 2 | *SDK for the Mycelium gossip mesh* linking `github.com/RichardEko/mycelium` | **dead account link** (repo moved to `RichardTathata` 2026-07-31); "gossip mesh" |
| A8 | `docs/publications/customer-pitch.html` `<title>` | *Coordination for applications and AI agents, without a coordinator* | altitude 2; the authority/evidence story is inside, not on the door |
| A9 | `docs/publications/presentation.html` `<title>` | *Adaptive Substrate for Services and AI Agent Fleets* | a fourth phrasing |
| A10 | `ROADMAP.md` status line | one paragraph of ~1,800 words, which itself says it *had fallen five releases behind* and points at `CHANGELOG.md` | a status line that is a wall is not read; the pointer is right, the wall should go |
| A11 | `CLAUDE.md` §What this is; `docs/wiki/wiki.md` | *an embedded, broker-less Rust library — a three-layer substrate* | consistent with each other; contributor-facing; keep, quote the sentence |
| A12 | `docs/operations/production-readiness.md`, `shared-responsibility-matrix.md`, `docs/analysis/ratings.md` Run 62, `v3-contracts-axis.md` §10, `CLAUDE.md` §Active work, `docs/publications/README.md` ledger | the proof gaps — no named production deployment; the nightly scale runner never green (V1); performance measured only as hot-path micro-benchmarks on one machine (rev 0.1 said *never benchmarked*, which was wrong — `tuning.md` §Performance baselines); AE4's four joint cloud runs need a counterparty; the self-audit floor at 7/7/7 for the same three dimensions across many runs — each stated honestly **in one of six places** | no single page a reviewer or buyer can read |
| A13 | `docs/guide/README.md` (25 chapters), `examples/README.md` §Recommended paths, `docs/operations/README.md` | three good funnels that do not agree on the first five steps | the path from *is this for me* to a running fleet is long |

**What this means.** Nothing here is false except A5, A6 and A7. The defect is that a true story is told
in four sentences, its limits in six places, and its start in three funnels.

---

## 2. The sentence (decided here)

**D1 — one canonical sentence, in one file, quoted verbatim everywhere else.** `docs/positioning.md`
holds it, with its three expansions; every front door quotes the sentence exactly and links the file;
a script fails the gate when a front door's copy differs. The candidate, to be edited once in that
file and nowhere else:

> **Mycelium lets a fleet of AI agents find, authorise and account for each other's work with no
> coordinator, and proves it by replay.**

Three expansions travel with it, each one paragraph, each quoting the sentence first:

- **the visitor's** (README, GitHub description, crates.io, docs.rs): the sentence, then *an embedded
  Rust library, three layers — gossip KV, signal mesh, epidemic consensus — with capability discovery
  across them; no broker, registry, daemon or control plane; state converges by gossip, work is claimed
  not dispatched, roles are discovered not assigned*; then the FAQ's *probably overkill if* paragraph,
  moved here because it is the most useful thing a visitor can read;
- **the buyer's** (both decks, the engagement kit, the pilot page): the sentence, then the order in §3;
- **the engineer's** (guide, building-on, crate doc): the sentence, then A3's clause list as the map of
  the axis, then the layer table.

*Why this sentence.* Find (discovery, the first half of the story), authorise (mandates and the action
seam, the second half), account (receipts and evidence), no coordinator (the thesis), proves it by
replay (the method, and the one claim a competitor cannot make without the seams). It contains no
word the philosophy's *What This Architecture Is Not* forbids. "Unique" and "first" stay reserved, as
§13.1 of the axis plan requires, until the prior-art comparison exists.

**D2 — one title for both decks**, the sentence, with the audience in the subtitle.

---

## 3. The buyer's order (decided here)

**D3 — authority and evidence first, emergent capacity second, the substrate as the how.** The deck
today opens on coordination without a coordinator and reaches authority late. A buyer deploying agents
fears what an agent might do and whether they could show what it did; that is the door. The order:

1. **What an agent may do, checked where the work happens** — mandates, the action seam, revocation
   that stops running work, the bypass matrix (v2.15.0).
2. **What it did, as records a third party can audit** — receipts that name their rung, the evidence
   journal, the NovusLens record kinds, replay.
3. **A fleet that fills itself** — declarations, the catalogue, presence policies, the stem topology
   (`design-time-tooling.md` §13), including agent-authored functions under fuel and a second signature
   (§17 there).
4. **How: no coordinator** — the three layers, the philosophy in four lines, the FAQ's *why not X*.
5. **What is proven and what is not** — §4's page, on a slide, dated.

The engineer deck keeps its current order (substrate first) and gains slide 5.

---

## 4. The proof page (decided here)

**D4 — one dated page, `docs/operations/what-is-proven.md`, beside the shared-responsibility matrix,
linked from the README, both decks and `production-readiness.md`.** Three sections, every line with
its evidence and its date, nothing paraphrased:

- **Proven in CI on every merge** — the bypass matrix; the golden persistence fixtures; the replay
  corpus; the fails-first regression floor; the fuzz targets and their reachability registry; the
  coop and AFN smokes; the Docker cluster suites. Each names the workflow step.
- **Demonstrated, with the bound stated** — the authority drain measured against its class's bound;
  the ten-council contention run; the 100-node scale suite *when it runs*; the two-mesh federation
  suite; the four CLI demonstrations that found defects.
- **Not yet shown, and what would show it** — no named production deployment (NovusLens is a design
  partner consuming records; the council corpus is envelope-qualified, not live); V1, the nightly scale
  runner, never green in its window; performance never benchmarked (`benches/` exists, no recorded
  run); AE4's four joint AWS/GCP runs; S4's real buckets; the self-audit floor at 7/7/7 (Modularity,
  Performance, Operational Readiness / Developer Experience) for many runs, with the ledger's count of
  scores later proven wrong.

The page is the single place `CLAUDE.md` §Active work, the axis plan §10, `ratings.md` and the
publications ledger link for *the gaps*; they keep their own detail and stop restating the list. It is
refreshed at every release (a `RELEASING.md` step) and by `/publication-lint`.

---

## 5. The path (decided here)

**D5 — one recommended path, five steps, the same on every funnel.** README, guide README, examples
README and operations README each show the same five links in the same order and nothing else above
the fold: *hello_mesh* (30 s) → *hello_capability* (the value) → the coop `provisioning` demo (the
fleet filling itself) → guide 20 + `authority_drain` (what an agent may do) → `what-is-proven.md`.
Everything else stays where it is, one link down. The FAQ keeps its routing tables and gains the
sentence.

---

## 6. The check (decided here)

**D6 — `scripts/check-positioning.sh`, in `make check`.** It reads the sentence from
`docs/positioning.md` and fails when any of these does not contain it verbatim: `README.md`,
`docs/guide/faq.md`, `docs/guide/README.md`, `docs/guide/building-on-mycelium.md`,
`docs/operations/README.md`, `src/lib.rs` (first doc paragraph), both decks' opening slide,
`mycelium-py/README.md`, `mycelium-ts/README.md`, `CLAUDE.md`, `docs/wiki/wiki.md`. It also fails on
`RichardEko/` anywhere outside `docs/wiki/dev/history.md` and the git log. The GitHub description and
the two `Cargo.toml` descriptions cannot be gated by a script over the tree; `RELEASING.md` gains the
two lines that set them (`gh repo edit --description`, and the `description` field diffed against the
file) and `/publication-lint` reads them.

---

## 7. Phases and exit gates

| Phase | Deliverable | Exit gate |
|---|---|---|
| **P1** | `docs/positioning.md` with the sentence and three expansions; `scripts/check-positioning.sh` in `make check`, initially **red** on every front door it names | the script lists exactly A1–A11's files as failing, so the gate is seen failing before any door is edited |
| **P2** | The doors: README (sentence + visitor expansion + the FAQ's overkill paragraph + the five-step path), FAQ, guide README, building-on, operations README, `lib.rs` (three layers, not two), `CLAUDE.md`, `wiki.md`, both SDK READMEs (sentence, `RichardTathata` links), `Cargo.toml` descriptions, the GitHub description via `gh repo edit` | the script is green; `gh repo view --json description` prints the sentence; `cargo doc` renders the new first paragraph; `/wiki-lint`'s front-door check passes |
| **P3** | The decks: one title (D2), the buyer order (D3), the proof slide; `docs/publications/README.md`'s ledger gains the entry *four titles, one sentence (2026-09-28)* | `/publication-lint` run 4 finds no cross-artifact inconsistency on the sentence, title or status claims; both PDFs regenerated |
| **P4** | `docs/operations/what-is-proven.md` (D4); links from README, decks, `production-readiness.md`, `shared-responsibility-matrix.md`; `CLAUDE.md` §Active work, axis plan §10 and the publications ledger shortened to cite it; `RELEASING.md` step to refresh it | every gap named in A12's six places appears on the page with a date and a link, and each of the six places links the page (grep); `/doc-coverage` gains a row *what is proven* with WHY and both HOW·Ops cells Clear by opening the page |
| **P5** | The path (D5) on all four funnels; the examples README's *Recommended paths* reduced to the five steps above the fold | the four funnels' first five links are byte-identical (a test in the positioning script); a newcomer following them reaches a running provisioning demo with no other page opened (walked once, recorded in the log) |

P1 and P2 are one PR (the script must be red then green in the same change). P3 and P4 are one PR
each. P5 is small and can ride with P2. Total: three PRs, two to three days, no code but the script.

*Executed 2026-09-28 in one PR (#434) rather than three, the script seen red on all seventeen doors and
paths before any edit; the GitHub description set with `gh repo edit`; the decks reordered; the proof
page written; every funnel carrying the path. The plan's own §1 line about performance was the one
finding the execution corrected.*

---

## 8. Decision register

| # | Decision | Alternatives declined, and why |
|---|---|---|
| D1 | One sentence in one file, quoted verbatim, gated | Harmonising by hand (it drifted four ways with everyone trying); a sentence per audience (that is the defect) |
| D2 | One deck title | Keeping two (a buyer who sees both sees two products) |
| D3 | Authority and evidence lead the buyer story | Leading with *no coordinator* (true, but it answers a question the buyer has not asked yet); leading with the stem fleet (not built) |
| D4 | One proof page, dated, linked from every place that used to restate it | Six honest places (each true, none complete; a reviewer assembles the list, and assembles it against us) |
| D5 | One five-step path on every funnel | Three funnels each with a good path (a newcomer meets three first steps) |
| D6 | A script in `make check` | A lint people remember to run (`/publication-lint` exists and the four titles still happened) |

---

## 9. Not claimed

The sentence does not make the gaps smaller; the proof page makes them findable. A single title does
not make the two decks say the same thing inside; `/publication-lint` §5 still does that. And a
recommended path is a recommendation: the guide keeps its 25 chapters and the examples their matrix,
because depth is the product's strength and this plan only fixes where it starts.
