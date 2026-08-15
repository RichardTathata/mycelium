# Git and the group wiki — projection, not substrate

**Status:** decided + built 2026-08-15 (`GitMirror` change sink, feature `git-mirror`).
**Context:** `mycelium-wiki` ([plan](../plans/mycelium-wiki.md) · [concurrent-edit design](wiki-concurrent-edit.md)).
**Driving use case:** the Transparency-Platform council-decisions deployment (one of the two use cases
that shaped the 2026-07-03 control-plane/data-plane pivot), whose premise is *"markdown in a git repo
is the authoritative datastore; git history is the audit trail; Postgres holds derived metrics."*

## The question

The `WikiStore` trait is small and its contract — per-object compare-and-swap over immutable
versioned objects, manifest-last commit point — looks like a natural fit for git (CAS ↔ atomic ref
update, versions ↔ commits). Should the crate ship a **`GitStore: WikiStore`** so a git repo *is* the
group wiki's backing store?

A design review of the obvious adapter (branch-ref CAS; the CAS `u64` carried as a `v:` front-matter
field; push to an operator remote for replication) found it fails Mycelium's own principles on seven
counts. This record keeps those findings, states the decision they force, and defines the eligibility
envelope under which the rejected variant could still be built later.

## The findings against GitStore-as-truth

1. **A global sequencer in the write path (High).** One branch ref totally orders every write across
   the corpus. The store contract exists to be *airtight under a transient dual-curator without
   assuming a single writer* (`store.rs`), with **independent per-section CAS slots**; a ref-CAS
   serialises unrelated pages onto one slot and re-assumes what the contract was built not to assume.
   Same move WS2 rejected for audit ("a global chain would need a sequencer = coordinator"). Worse,
   the cross-machine backstop — the remote rejecting non-fast-forward pushes — delegates the safety
   property to an **external coordinator's configuration**, silently voided by a force-push-permitting
   remote.
2. **Deletion impossible by construction (High).** `FsStore` GCs old versions deliberately (bounded
   disk is a stated design input). Git history cannot be dropped without rewriting it under every
   consumer, and envelope-encrypting blobs for crypto-shred (WS-F) destroys the legible-diff property
   that motivates git in the first place. *Human-auditable diffs and erasable content are mutually
   exclusive in one store.*
3. **The CAS token pollutes the canon (Medium).** A `v:` front-matter field puts concurrency-control
   machinery inside the human-audited document and dirties every diff with version noise — in the
   corpus whose diff legibility was the point.
4. **An unpoliced egress path (Medium).** Pushing the corpus — the concentrated crown jewel — to a
   third-party remote is an outbound path `EgressPolicy` never sees (WS3 enforces it at every
   outbound path *the substrate chooses*).
5. **Layering leak (Medium).** Batching a reconcile round into one commit needs the curator to
   special-case the concrete store type — breaking the pluggability that is the data plane's one job.
6. **The dumb-store property degrades (Low-Med).** Readers would need git tooling + fetch credentials;
   the access broker's `StoreGrant` (location + scoped read grant) has no custody story for git auth.
7. **Concurrency hygiene (Low).** The natural implementation holds a lock across blob/tree/commit
   I/O; raced ref-CAS attempts strand dangling commits; `gix`/libgit2 is a heavy dependency for a
   workspace that counts core's ~48 deps as a virtue.

## The decision — git is a projection the curator emits, not the substrate writes through

Keep the system of record exactly as shipped: the airtight per-object-CAS `WikiStore` (`FsStore` /
`S3Store`), dumb, erasable, read directly by agents. Add a **change sink** on the curator: after each
drain round lands, the curator notifies an injected `ChangeSink`; the shipped `GitMirror` sink renders
the touched pages as pure markdown into a local git worktree, makes **one commit per applied round**
(a real, reviewable unit with proposal provenance in the message), and optionally pushes to an
operator-owned remote.

This is the same shape WS2 audit export already takes (`AuditSink`: the substrate emits, the operator
owns the downstream), and it dissolves the findings rather than mitigating them:

| Finding | How the projection resolves it |
|---|---|
| 1 — sequencer | Git is **not in the write path**. The store keeps independent per-section CAS; the mirror linearises only its own commits, which order nothing. A transient dual-curator at worst produces divergent *mirrors* — detected, not load-bearing. |
| 2 — erasure | The record stays erasable (GC / crypto-shred as designed). The mirror is a **derivation**: erasure = delete the mirror repo and `rebuild()` from the store — rewriting a projection's history is legitimate precisely because nothing coordinates through it. |
| 3 — canon purity | The mirror carries **no CAS tokens** — page path + attributes in front-matter, section ids as comments, prose as prose. Versioning is git's own ancestry. |
| 4 — egress | `GitMirror` takes an optional `EgressPolicy` and **fail-closed** refuses a push (and construction) whose remote host isn't permitted; local-path remotes bypass nothing because they leave no machine. |
| 5 — layering | `ChangeSink` is a trait on the brain; the curator never downcasts. One-round-one-commit falls out of the notification granularity, not a store special case. |
| 6 — dumb store | Agent reads are untouched (direct store reads via the broker). Git credentials exist only where the mirror runs — the curator node — matching the single-writer shape. |
| 7 — hygiene | Zero new deps (the `git` CLI, feature-gated `git-mirror`). The sink `try_lock`s and **skips** under contention (mirroring is an idempotent snapshot; the next round catches up) — no lock ever blocks the drain. Pushes run on a background thread behind an in-flight guard, with a post-push `ls-remote` **divergence tripwire** (detection, not prevention) counted and warned. |

**The honest cost:** git stops being *the* truth and becomes a tamper-evident derivation of it. For a
deployment whose stated premise is "the git repo IS the datastore," that is a real product conversation:
the substrate's answer is that the repo remains the *human* system of engagement (review, blame,
history, PR workflow against the mirror can feed back as proposals) while the *machine* system of
record is the store the CAS protects. Pipelines that today read the repo keep working — they read the
mirror.

### Semantics (what the sink promises, and doesn't)

- **Best-effort, never load-bearing.** A sink failure (dir vanished, git absent, push refused) logs
  and counts; it can never fail or delay an apply. The mirror is *eventually* faithful: every
  successful round re-renders the touched pages from the store's committed state, so any missed round
  is healed by the next one touching those pages, and `rebuild()` heals everything.
- **Each commit is a true snapshot.** The mirror reads the store *at mirror time*, which may already
  include a later round's writes — a commit may be "ahead" of its message's round. No content is ever
  lost; attribution granularity is per-round, not per-write.
- **One commit per drain round** with provenance: `wiki({group}): N proposal(s) by {authors} → {pages}`.
- **Erasure procedure** (operator runbook): erase in the system of record first (store GC /
  crypto-shred per `data-lifecycle-and-erasure.md`), then delete the mirror repo (and remote), then
  `rebuild()` — a fresh history containing only the surviving corpus.

## The eligibility envelope for GitStore-as-truth (not built)

Like the KV-native wiki variant, the rejected adapter is retained as a *conditional* design: it may
only be built for a deployment satisfying **all** of:

- **E1 — public-record corpus:** no per-subject erasure obligation, ever (history is permanent).
- **E2 — single-curator topology:** write concurrency is genuinely 1 (the per-section independence
  the trait promises is forfeited knowingly).
- **E3 — operator-owned, force-push-locked remote** with the ancestry tripwire deployed (the
  external-coordinator trust is explicit and monitored).
- **E4 — reader tooling accepted:** every direct reader carries git tooling + credentials, and the
  broker's grant story is extended to cover their custody.

Absent any one of these, the projection is the answer.

## Verification

`mycelium-wiki/tests/git_mirror.rs` (feature `git-mirror`, real `git` in a tempdir): one commit per
round with byte-exact pure front-matter (no CAS token); history retained across rounds
(`git show HEAD~1:` recovers the prior text); egress fail-closed (a disallowed remote host refuses
construction; a local bare remote round-trips a push); a destroyed mirror never fails `round_applied`;
`rebuild()` regenerates the corpus; and an end-to-end curator test (propose → drain → commit) proving
the `drain_once` wiring. CI runs the feature's tests + clippy in the Wiki job.
