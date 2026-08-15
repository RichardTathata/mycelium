# ingest — GitStore built: council-substrate Phase 1 (2026-08-15)

**Shipped:** `GitStore: WikiStore` (feature `git-store`, `mycelium-wiki/src/git_store.rs`, zero new
deps) — the git-as-truth store, legitimate **only inside the E1–E4 envelope** of
`design/wiki-git-store.md`; Phase 1 of `design/transparency-council-substrate.md`. CI Wiki job
gained the `git-store,control-plane` steps; companion page + plan + CHANGELOG updated.

**As-built decisions worth remembering:**
1. **One page = one markdown file** (front-matter manifest, marker-delimited sections with visible
   headings) — an FTT leaf is a real document; bodies round-trip **byte-exactly** (char-exact
   parser; two reserved sequences refused at write, never mangled — the boundary off-by-one was
   caught by the round-trip test on first run).
2. **CAS tokens = content hashes (FNV-64 of the serialized section/manifest), never in the
   document** — resolves the F3 canon-purity critique outright, and turns out to be the *right*
   CAS for merge-based writers: conflict ⟺ the reconcile base is no longer the committed content.
   Equality-only (unordered; A→B→A re-yields A's token — safe, the base equals committed content);
   the FsStore suite's version-ordering assertion adapts to inequality. Crucially it preserves
   **per-section CAS independence inside a single file**: a commit changing section X leaves Y's
   bytes and token unchanged, so a concurrent Y-writer just rebuilds on the new head and retries
   the ref.
3. **Plumbing against a private temporary index** (`hash-object → read-tree → update-index →
   write-tree → commit-tree`) landed by **atomic `update-ref <new> <old>`** — a true branch-head
   CAS. The caller's real index/staging is never touched; each commit carries exactly the written
   path (FTT's scoped-commit rule becomes the mechanism). One internal mutex per instance; the
   ref-CAS is the cross-instance backstop (gated by a real two-instance race test).
4. **Reads at HEAD, never the worktree** (FTT's own "measure from a named commit" rule); the
   worktree is synced post-commit for humans/validators.

**Gates:** `tests/git_store.rs` — the FsStore contract mirrored (20 tests: round-trip,
manifest-authoritative reads incl. orphan visibility in `read_versioned`, CAS conflicts on both
slots, the idempotent-append exactly-once proof at git-commit volume, two-instance ref-CAS race)
+ git-specific (history recoverable via `git show HEAD~1:`, scoped commits + message prefix, no
empty commits, unborn branch = empty store, worktree ≡ HEAD, no version tokens in documents);
`tests/git_store_curator.rs` — the **hinge**: a curator over GitStore drains a proposal into
scoped, prefixed commits. Phases 2–5 (group wiring · validator lint · bulk ingest · work
distribution) remain open in the design record.
