## [2026-10-17] ingest | Fleet-wide history scopes (post-360 row K2)

The user decided (plan §5, D7): build both fleet scopes, as named variants of `HistoryScope`, the policy choosing per
rule; per-point stays the default. `FleetConsensus` — exact: N seats, each a consensus slot leased for the window, a
permit claiming one by `exclusive_propose` before admission (read back from the converged record, never assumed from
`Committed`); no quorum ⇒ `Indeterminate`. `FleetTally` — approximate: per-point permit times under `sys/ae-tally/`
(self-owned, added to core's tripwire list) and a heartbeat, summed over self and fresh peers; detection, not
prevention, stated on every decision; stale or unknown coverage (the governors' confidence bound, reused) is not
established. Design points found while building: the enforcement point must learn the counter *before* the evaluator
decides (a new trait method, `fleet_counters`, rather than a field on `Decision`, which has ~40 in-crate literals); the
contract suite binds a fixture's fleet state to whatever counter key the evaluator names, as `RevisionBinding` binds a
derived revision; and gateway and provider must count apart fleet-wide too, or provider enforcement would spend two
seats per call.

Pages: `dev/dev.md` (AE section), `dev/concurrency/lock-order.md` (row 61, held across the claim). Repo: guide 20,
`00-concepts.md` (history scope), `deprecations.md` §53, plan rows K2 + §4 D7 + §5, `src/lib.rs` namespace table,
rule catalogue `ae.preflight` rev 3.

## [2026-10-18] review round 1 | Fleet-wide history scopes (#642)

The independent review found a consensus round held under the striped per-actor gate (and inline in the provider
serve loops), a lease fixed before admission (N+1 within a window), a heartbeat that started only with a permit,
unbounded seat and tally growth, a counter digest that revealed the rule, a peer without a heartbeat counted as zero,
and four smaller gaps. Lessons: a lock that serialises cheap local work must never be held across a network round —
consensus already orders the contenders, so the local gate is released before the claim and the local count taken
back on failure; a lease that starts before admission must be checked at admission; coverage needs every member to
speak, so the heartbeat is every node's, from start; and row A's per-slot sentinel makes a per-actor slot family
permanent, so the bound has to come from the policy (a named actor), not from collection.

## [2026-10-18] review round 2 | Fleet-wide history scopes (#642)

The confirmation review found the provider half of finding 1 still open for rules that require scopes: `may_wait`
built its own scope-less envelope, so the decision whether to wait diverged from the check's. Fixed at the root: the
preflight is split into `ae_decide` (from the real envelope, up to releasing the gate) and `Settling::finish` (claims
and the record), and the provider check returns `Checked::Waiting` only when the decided permit must claim — nothing
decided twice. The deferred set is bounded (64; full, the loop stops draining, so backpressure returns), a panicking
check or one still waiting at loop end is answered `authority_not_established`, and the closed-channel spin is gone.
A graceful leave now writes a departed heartbeat and keeps the entries (peers count them, do not wait, and collect
them once aged), an unreadable entry is unknown, and tally writes take one leaf lock (row 67). Lesson: when a fast
path decides "is this the slow case?", it must decide from the same object the slow path uses, or the two drift on
exactly the fields the shortcut omitted.

## [2026-10-18] review round 3 | Fleet-wide history scopes (#642)

Two more serve loops (`rpc/serve`'s SSE stream and the bridged MCP loop) awaited a claim in place; every loop now
settles a `Checked::Waiting` beside itself (`provider_enforcement::check` is the one deferring entry point). An
abandoned check is aborted, not detached, and a dropped `Settling` takes its provisional permit back. Collection of a
departed node's keys by its peers was removed: a collector cannot distinguish a node's old record from a newer one
it has not yet received, so a foreign delete could erase a returned node's permit (fails open) and trips its `sys/`
tripwire. Instead readers count every enforcing non-peer's entries until they age out — which also closes the
under-count after a crash — and the residue of a node that never returns is stated as bounded. Lesson: in a
gossiped namespace, "delete what looks dead" by a non-owner is unsound unless the delete is conditional on a version
the owner cannot have superseded; absent that, let readers ignore what has aged.

## [2026-10-18] review round 4 | Fleet-wide history scopes (#642)

Residue is collected again, in a form that cannot erase a returned node: a non-owner tombstones a non-peer's tally keys
only past the horizon (longest window among its entries + staleness + drift), each **at the timestamp of the record it
holds** (`ops::kv_delete_at`, renamed `kv_delete_held` in round 5) — under LWW an equal-stamp tombstone removes exactly that record and any later write
beats it. The returning owner re-asserts its own heartbeat and withdraws its own aged entries with local-only writes
(`ops::kv_write_local`) before `start_listener`, so a collector's tombstone reaches it older than what it holds, is not
applied, and the `sys/` tripwire (which counts only applied foreign writes) does not count it. Readers ignore a
non-peer past the horizon, count one only if it is a verified member (`trusted_keys` non-empty, not removed — a
fabricated id counts for nothing), and list nodes from the store's scope index (`sys/ae-tally/` registered as a scoped
prefix) rather than scanning entries. `Settling::finish` disarms the retraction before the journal await (an abort
there is delivery unknown). The `rpc/serve` loop is pull-based. The `/mcp` door-test flake is recorded open with
instrumentation. Lesson (amending round 3's): a non-owner delete *is* sound in a gossiped LWW namespace when it is
stamped at the version observed — it is then conditional on that version by construction.

## [2026-10-18] review round 5 | Fleet-wide history scopes (#642)

No blocking defect; five lows closed. The scope index gained a per-prefix roster (`KvStore::scope_roster`), so
`scopes_under` a registered prefix visits that prefix's scopes only — a tally read no longer walks every consensus
slot; a reader prunes an emptied scope and re-checks the bucket after pruning, so a racing insert (bucket first, roster
second) is never lost. `kv_delete_at` became `kv_delete_held(ctx, key, observed)`: refused unless the live record held
carries the observed stamp, so it cannot write a tombstone newer than the record held. `kv_write_local` is
`#[doc(hidden)]`. A `/mcp` dispatch timeout's diagnostics stay in the operator's log. Without `[tls]`, a non-peer
inside the horizon is unknown coverage (fail closed) rather than ignored; `require_identity_proofs = false` re-opens
fabricated ids; forgery under a dead member's namespace (unsigned entries) is stated as not built.
