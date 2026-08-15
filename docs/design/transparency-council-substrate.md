# Transparency Platform on Mycelium — the council-wiki substrate design

**Status:** designed 2026-08-15, **not built** — the phased build list is §5.
**Companion records:** [`wiki-git-store.md`](wiki-git-store.md) (the GitStore eligibility envelope this
deployment satisfies — the first that does) · [`wiki-concurrent-edit.md`](wiki-concurrent-edit.md) ·
[`../plans/mycelium-wiki.md`](../plans/mycelium-wiki.md) (council decisions is UC2, one of the two
driving use cases behind the 2026-07-03 control-plane/data-plane pivot).
**External context:** the Transparency Platform's own engineering wiki
(`Transparency_Platfrom/docs/wiki/dev/pipeline/council-wiki.md` and
`wiki-datastore-prior-art.md`) — read those pages before revising this record; the facts below about
that system were verified against them on 2026-08-15.

## 1. The deployment, in five facts

The Transparency Platform (FTT) tracks UK council climate decisions. Its **council-wiki** is a git
repo of markdown that **is the datastore** — the DB is derived from it (`PDF → wiki → reviewer →
DB`). Five facts drive everything here:

1. **Git is load-bearing, not cosmetic.** Per-meeting boundary commits are the restore mechanism
   (a prune once destroyed 1,527 leaves; `git checkout <before-sha>` is the recovery), run-id tags
   join DB rows to wiki commits, and git-refusal semantics (`git mv` refuses to clobber) are
   first-order safety properties.
2. **The per-council write domain is already their invariant, enforced by hand.** Commits are scoped
   `councils/<slug>` (never `-A`), validator runs scope per council, reports are council-keyed —
   and agent sessions must run one at a time because nothing coordinates them (their docs record
   measurements corrupted by "another agent's uncommitted edits").
3. **The pipeline is deterministic, model-free code** (their #1226): download → extract → interpret
   → match. Compute dominates; the serial write phase costs ~1.4 s/meeting. Writes must be
   **byte-identical to a serial run** (folder/slug/index minting in candidate order).
4. **Everything resumes via skip-if-exists** — work items are idempotent by construction.
5. **A standalone Node validator** gates structure at choke points (pipeline stages, hooks, CI),
   blocking on errors only.

## 2. The decision

**Mycelium is adopted as the coordination fabric for both of FTT's scaling problems — distributed
pipeline compute and concurrent agent writers — while git remains the datastore, unchanged.** The
shape is a **control-plane adoption over a `GitStore` data plane**: mycelium-wiki supplies curator
election + failover, the proposal path, reconcile, the access broker and MCP tools; the store the
curator writes is the council-wiki repo itself, so every git mechanism FTT hardened (restore points,
tags, hooks, the validator, scoped commits) survives verbatim.

This is **not** the shipped store-as-truth + `GitMirror` shape. That shape was assessed first and
rejected for this corpus: `FsStore` keeps no history (recovery — the property FTT paid blood for —
would be lost), choke-point gating would move after-the-fact, and the ETL volume doesn't belong in
an evaporating KV proposal queue. Inverting to git-as-truth resolves all three — at the cost named
in the envelope below, accepted knowingly.

## 3. The two planes (the correction that shaped this record)

An earlier draft of this thinking said "Mycelium adds nothing to the pipeline." That was wrong — it
conflated the write plane with the compute plane:

- **The write plane cannot parallelise within a council** — git's index lock and the
  byte-determinism rule both forbid it. One writer per council is a *requirement*. That writer is
  the **curator**.
- **The compute plane parallelises at two levels**: across councils (391 disjoint subtrees,
  embarrassingly parallel) and per-PDF within a council (what FTT's local thread pool does today,
  distributed across machines).

```
                        council group (e.g. "norfolk")

  workers ──► pull work items ──► fetch + extract PDFs ──► stage results in S3
  (any node)   (tuple-space,       (CPU/network; the        (content-addressed,
                idempotent leases)  dominant cost)            claim-check)
                                                                  │ reference over RPC
  agents ────► propose edits ────────────────────────────► CURATOR (single writer per council)
  (reviewer,    (wiki.propose via MCP/gateway,              applies in deterministic candidate
   gardener)     membership-gated, no checkout)             order · runs the Node validator as a
                                                            pre-apply lint · commits per meeting,
                                                            scoped councils/<slug> · ring-failover
```

The two roles compose instead of fighting: the single writer the pipeline *needs* is the same
curator the agent-edit story needs. One structure serves both, and one substrate replaces both a job
queue (for compute) and the by-hand serialization (for agents) — with no broker or scheduler to
operate.

**Payload rule (non-negotiable):** extraction outputs never ride gossiped KV. Workers stage results
in S3 (FTT already runs a content-addressed `PdfStore`) and hand the curator a **reference**. KV
carries work-item leases and the small agent-edit proposal queue only.

## 4. Group-per-council: their manual invariant, made structural

| FTT does by hand today | Mycelium provides structurally |
|---|---|
| commit scope `councils/<slug>`, never `-A` | group = council slug; the curator's apply commits that scope only |
| one agent session at a time | proposals queue + per-leaf CAS + reconciler merge; applies serial per council |
| one pipeline process at a time | per-council work leases; N councils proceed in parallel |
| no failover of any role | curator election + ring-failover per group |
| validator at choke points, after the write | validator as a **pre-apply** curator lint (errors refuse, warnings pass) |
| agents need a repo checkout | `wiki.propose` over MCP/gateway; broker gates membership |

"Set of councils" scoping is the same mechanism: a group is a label, so a shard (region, batch) is
just a coarser label; a `CapabilityGroupDef` filter can auto-assign shards. Cross-council writers
(catalog publish) join multiple groups.

## 5. The eligibility envelope, checked (why GitStore is now justified)

[`wiki-git-store.md`](wiki-git-store.md) rejected `GitStore: WikiStore` as the general answer and
retained it behind four conditions. Council-wiki is the first deployment satisfying them:

| Condition | Council-wiki |
|---|---|
| **E1** public-record corpus, no per-subject erasure | **Mostly yes** — council minutes are public record. ⚠️ Councillor pages are personal data and git history is permanent; FTT already lives with this in pure git (no regression), but adopting locks it in → **state it in their DPIA** |
| **E2** single-curator topology (per-section CAS independence knowingly forfeited) | **Yes by design** — one curator per council is the point; their scoped-commit rule already serialises per-council writes |
| **E3** operator-owned, force-push-locked remote + ancestry tripwire | **Yes** — `TathataSystems/council-wiki` with existing hooks/CI |
| **E4** readers carry git tooling | **Yes** — every current consumer is already a repo reader |

The original F1 objection (a branch ref = a global sequencer) softens to a **named, accepted cost**:
N curators pushing one branch serialise at *push* granularity with pull-rebase-retry — fine at
agent-edit + per-meeting-commit rates, and no worse than today's fully-serialised sessions.
Escape hatch if push contention ever bites: per-council branches + a merge queue.

## 6. Build list (Mycelium side, phased)

1. **`GitStore: WikiStore`** — the envelope build. Mapping: **page = leaf file** (one section),
   identity = path (theirs already); CAS = per-file version check + scoped commit + ref-retry;
   apply = commit to `councils/<slug>` with FTT's message/provenance conventions. Size ≈ `fs.rs`
   (~320 lines) + the contract-test harness parameterised over `impl WikiStore` (run `fs::tests`
   against it). *Gate:* the existing store contract suite green on `GitStore`, plus
   history-retention and scoped-commit tests.
2. **Group-per-council wiring** — `WikiConfig::group` = slug → store scope; a shard variant.
   Small; mostly a worked example + conventions. *Gate:* two curators on two councils in one repo,
   concurrent applies, no cross-scope commits.
3. **Node-validator pre-apply lint** — a `SemanticLinter`/gate impl that shells FTT's
   `validation/validate.js --start councils/<slug>` over the curator's working tree and refuses the
   apply on errors (warnings pass — their blocking rule). *Gate:* an edit violating an entity
   contract is refused with the validator's finding attached to the proposal outcome.
4. **Bulk-ingest claim-check path** — a curator RPC accepting "apply this meeting's extracted
   leaves" as an S3 reference (never KV payloads); the curator fetches, writes in candidate order,
   commits per meeting. Wiring over existing RPC/bulk primitives, not invention. *Gate:* a
   simulated worker submits a meeting; the commit is byte-identical to the serial pipeline's.
5. **Work-distribution assembly** — tuple-space lanes for council leases + per-PDF fan-out,
   leaning on FTT's skip-if-exists idempotency (what makes at-least-once redistribution safe by
   construction). Mostly assembling existing companions; doubles as the **production case study
   the Paper 1 work-distribution experiment lacks**. *Gate:* kill a worker mid-council; the lease
   evaporates and another node completes it with zero duplicate leaves.

**FTT-side agreements (theirs, not ours):** a curator **maintenance mode** while a council is bulk
regenerated; hooks/CI stay for human edits; the E1 DPIA note.

## 7. What this design does *not* use, and what stays

- **`GitMirror` — not used here.** Git is truth, so there is nothing to mirror. It stays in the
  crate as the answer for store-as-truth deployments — and building it produced the envelope that
  justifies Phase 1, so it was the road *to* this design, not a dead end.
- **`FsStore` / `S3Store` — unchanged**, the general-purpose data plane for every other wiki
  deployment.
- **The KV-native wiki variant — stays dead**, reconfirmed: this corpus (146k files per large
  council) is the strongest counterexample to putting a corpus in gossiped KV.

## 8. Inherited invariants (break any of these and the design is void)

- Writes per council are **serial and byte-identical** to a serial run (candidate order).
- **Per-meeting boundary commits** + `pipeline/<run_id>` tags survive exactly as FTT specified.
- Commit scope is the council slug (+ its named sibling index files), never broader.
- Work items are **idempotent** (skip-if-exists) — the property that makes distribution safe.
- The validator **blocks on errors only**; warnings never block.
- Extraction payloads never enter gossiped KV.

## 9. Open questions (for the build, not the decision)

- Curator ↔ pipeline handoff during a full regen: quiesce protocol and who owns the boundary
  commits during maintenance mode.
- Whether Phase 3 shells `node` per apply (their measured scoped run is ~38–90 s — too slow per
  drain) or batches per drain round with a dirty-set scope; likely the latter.
- One repo checkout per curator node vs a shared filesystem checkout; the former is simpler and
  matches E4.
- Push cadence: per drain round vs interval — FTT's own rule ("commit as you go, push at the end")
  suggests interval, with the divergence-tripwire idea from `GitMirror` ported to the store's push
  path.
