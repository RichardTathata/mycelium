# Council-substrate hardening (Phase 6) — closing the toy-scale → council-scale gaps

**Status:** planned 2026-08-15, not started. **Origin:** the same-day step-back critique of the
five-phase build ([design record](../design/transparency-council-substrate.md)): the mechanisms are
sound and their gates honest **at meeting scale**, but six named gaps stand between "mechanisms
built" and "can serve the real 391-council corpus" — and calling the remainder "deployment" was an
overclaim. The meta-lesson joins the session's earlier ones: **a green gate at toy scale is not
evidence at deployment scale** (this session's third instance of the class: `make check-full` ≠ CI;
a written fuzz gate ≠ a validated sweep; a 3-page repo gate ≠ a 5,741-file corpus).

Ownership legend: **[M]** Mycelium-side · **[FTT]** deployment-side · **[D]** decision recorded here.

## P6.1 — Batch commits + batch gate (Gaps 4 + 6) [M] — ✅ BUILT 2026-08-15

Per-page commits violate FTT's crash invariant (*"the repository only ever holds whole meetings"*)
and run the write gate per page — hours against their 38–90 s validator.

- Add a **default trait method** `WikiStore::write_pages(&[…]) -> Result<()>` (default = the
  existing per-page loop, so `FsStore`/`S3Store` semantics are untouched — additive, no breakage).
- `GitStore` overrides it: N blobs into one private index → **one tree, one commit** per batch;
  `apply_batch` calls it once per batch, so a per-meeting batch = a per-meeting commit — FTT's
  boundary-commit granularity restored, commit count back to their measured budgets.
- The write gate runs **once per batch** with the *full candidate file list* as argv (their
  `check-pages.sh` already takes a file list — the shapes meet). **Semantics change, recorded:** a
  gate refusal refuses the **whole batch** (nothing commits) — whole-meeting atomicity, matching
  FTT's abort-before-sync model; Phase 4's per-page-skip behaviour remains only on the per-page
  path. *Gate:* a batch with one invalid page commits nothing; a clean batch is exactly one commit;
  the byte-identical-tree test updated (tree identity unchanged; granularity asserted 1/batch).
  *As built:* `PageWrite` + `WikiStore::write_pages(pages, label)` default method;
  `GitStore::commit_files` (N blobs → one tree → one commit, tree-equality no-op skip) +
  `gate_check_batch` (all candidates placed, ONE run with the full argv list, restore either way);
  `apply_batch` rides `write_pages`, so an ingest batch = one `batch({source}) — N page(s)` commit.
  The stub validator became a file-LIST validator — which itself caught the contract change: the
  single-file stub silently passed a bad batch (checked only $1). Gates green:
  `ingest_is_byte_identical_to_the_serial_writer_and_lands_as_one_commit` +
  `a_gate_refusal_refuses_the_whole_batch_atomically`; exactly-once + curator suites unchanged.

## P6.2 — The read plane: `cat-file --batch` (Gap 5) [M] — ✅ BUILT 2026-08-15

`read` spawns `rev-parse` + `git show` per call; `query`/`list_pages` stack that per file →
~10,000+ process spawns for one query over Edinburgh's 5,741 files.

- One persistent `git cat-file --batch` child per store (spawned lazily, restarted on death);
  `load_at` feeds `{sha}:{path}` lines and reads framed replies. `ls-tree -z` once per
  `list_pages`. Head resolved **once per operation**, not once per page-read.
- *Gate (green):* `the_read_plane_scales_without_per_page_process_spawns` — a 600-page corpus;
  **measured: `list_pages` + `query` in 330 ms** (~0.55 ms/page; extrapolated ≈3 s over
  Edinburgh.s 5,741 files, vs tens of minutes pre-fix). All 24 contract tests unchanged + green.
  *As built:* persistent `CatFile` child (lazy spawn, respawn-once-on-death, Drop-reaped);
  `read_blob` replaces the per-read `rev-parse`+`git show` spawns; `query`/`list_pages` share ONE
  head resolve + ONE `ls-tree` via `page_files_at`; bonus write-side win: the per-file
  `update-index` loop became one `--index-info` stdin splice (N files, one subprocess).

## P6.3 — Failover topology: pull-on-promote, push-per-round (Gap 2) [M] + [D] — ✅ BUILT 2026-08-15

The checkout is node-local, so the companion's litmus ("failover transfers nothing") does not hold:
a ring-promoted curator on another node has a stale clone. **Decision [D]: option (a)** — the E3
remote is the shared truth; per-node clones sync at the role boundaries. (Rejected: shared-FS
checkout — git-on-NFS; curator pinning — surrenders emergent failover, kept only as a deployment
fallback via `WikiRole::Curator`.)

- `WikiStore` gains two **default no-op** methods (additive): `refresh()` — *bring the local view
  up to date with the shared backing* (no-op for inherently-shared Fs/S3) — and `publish()` —
  *make local writes visible to other nodes' views*.
- `GitStore` (with a configured `remote`): `refresh` = fetch + reset-to-origin (reads-at-HEAD only
  move forward); `publish` = push with one pull-rebase retry, plus the **divergence tripwire**
  ported from `GitMirror` (post-push `ls-remote` check, counted + warned, never auto-fixed).
- Curator wiring: `become_curator` calls `refresh()` **before serving** (a refresh failure refuses
  the curatorship — a knowingly-stale curator is the data-loss path); the drain loop calls
  `publish()` after each applied round (rounds are small — per-round push is FTT's
  "commit as you go, push at the end" adapted to a long-running writer).
- **Named residual:** a dead curator's *un-pushed* tail (≤ one round) is lost to the promoted
  curator and re-lands via proposal re-drain / batch resubmission — the same at-least-once +
  idempotency contract as everything else; stated, not hidden.
- *Gate (green):* `curator_failover_resumes_on_a_fresh_clone_via_the_shared_remote` — A applies,
  the round publishes to a bare origin, A dies with a deliberately-unpublished local tail; B (a
  FRESH clone) promotes, refresh adopts the remote head, **reads A.s corpus (the litmus)**, the
  tail is asserted ABSENT (the named residual, not hidden) and re-lands via re-apply; B converges
  with the remote, tripwire quiet. Plus `a_curator_that_cannot_refresh_never_serves` (the refusal
  path). *As built:* `refresh`/`publish` are default no-op `WikiStore` methods (additive; Fs/S3
  untouched); `GitStoreConfig.remote` + `with_remote`; refresh = ls-remote check (empty origin =
  valid fresh start) + fetch + update-ref adopt + best-effort worktree restore; publish = push with
  a **worktree-free `merge-tree --write-tree` two-parent merge retry** (disjoint councils merge
  cleanly; a same-path conflict surfaces) + the ls-remote divergence tripwire
  (`GitStore::push_divergences`); the curator refuses the curatorship on refresh failure and
  publishes best-effort per applied round + after each ingest. Requires git ≥2.38 for the
  merge path (merge-tree --write-tree).

## P6.4 — Write contention: backoff + topology guidance + a measured run (Gap 3) [M]

The single-branch ref-CAS gives ~12 commits/s **global** when many curators share one checkout, and
`write_with`'s 16-loss retry turns contention into spurious `Conflict`s.

- Add jittered backoff to the ref-retry loop and raise the bound (contention should queue, not
  error). *Note:* P6.3's topology dissolves most of the problem — one clone per curator node means
  **no local ref contention at all**; the serialization moves to per-round `publish()` (push-rebase),
  which is per-round, not per-commit. P6.1 cuts commit volume ~5× further.
- Document the topology rule: co-locating many councils' curators over ONE checkout re-introduces
  the shared-ref ceiling — prefer clone-per-group (cheap: worktrees of one object store are a
  follow-up if disk matters).
- *Gate:* a 10-council contention run (10 curators, concurrent per-meeting batches) — zero spurious
  failures, throughput measured and **recorded here** before any 391-council claim is made anywhere.

## P6.5 — The entity-format codec (Gap 1) [M trait + FTT impl] — *the substrate-fidelity gap*

`GitStore` renders the mycelium page format; the council-wiki corpus is `entity-type`-first
kebab-case front-matter, labeled link lines, `decisions.md` row-stores — **their real validator
refuses every page this store writes today**, and the byte-identical gate proved self-consistency,
not fidelity to their pipeline's output.

- **[M]** Extract the codec into a `PageFormat` trait (`render`/`parse` over manifest + sections);
  `GitStoreConfig.format` defaults to the built-in format. Contract: byte-exact round-trip,
  property-tested over both bundled impls.
- **[FTT]** `CouncilWikiFormat` implements their entity schema — it encodes *their* contract, so it
  lives with their validator as its gate (their fixture-vault conformance test is the model: render
  through the real codec, validate with the real `validate.js`, assert zero errors). Note the happy
  structural fit: their `decisions.md` row-store (N rows, `#N` anchors) maps naturally to one page
  with N sections.
- *Gate (M-side):* the trait + round-trip properties + all existing tests green under the default
  format. *Gate (FTT-side):* their conformance test over `CouncilWikiFormat`.

## P6.6 — Lower tier [M]

- Wrap the ingest responder's apply in `spawn_blocking` (sync git I/O off the async workers).
- `submit_batch` timeout becomes a parameter (default 60 s); document **batch = one meeting** as
  the sizing contract, not a convention.
- `councils/councils.md` (the sibling index outside any group scope): regenerable index — stays
  pipeline/FTT-owned for now; noted, not built.

## FTT-side list (for their tracker, not ours)

`S3BatchSource` (impl of `BatchSource` over their bucket) · the real validator as the batch gate
command (wrapping exit-2-warnings → 0) · `CouncilWikiFormat` + conformance gate (P6.5) · the
councillor-GDPR line in the DPIA (envelope E1) · council-scale runs — which double as **Paper 1's
production work-distribution case study**.

## Sequencing

P6.1 → P6.2 (both small, independent) → P6.3 (the big one) → P6.4 (measurement after 6.1+6.3 change
the constants) → P6.5 (parallel-izable; the FTT half can start once the trait lands). No phase
claims council-scale readiness until P6.4's measured run is recorded here.
