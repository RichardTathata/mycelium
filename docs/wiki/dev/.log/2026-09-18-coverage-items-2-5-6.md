## [2026-09-18] ingest | coverage — items 2, 5 and item 6 PRs 4–5, merged without a wiki trace

Up: [dev](../dev.md) · records `docs/design/{federated-domains,scoped-mandates,replay-nondeterminism-inventory}.md`
· code `src/federation/`, `src/mandate/`, `mycelium-wiki/src/mandate_fence.rs`, `mycelium-core/src/persistence.rs`.

### Why this entry exists

The item 3 ingest ([2026-09-18-item3-knowledge-layer.md](2026-09-18-item3-knowledge-layer.md)) checked the ledger
and found that the same session's earlier work had been merged with no wiki trace: **item 2** (federated domains,
#242, #245–#253) had neither a ledger section nor a log entry; **item 5** (scoped mandates) had only its ADR's
log entry (#244) and nothing for #256–#259 or the three follow-ons #261–#263; **item 6 PR 4** (#241) had a log
entry but no ledger row, and **PR 5** (#260) had neither. That is lint check 4 — merged work with no wiki
trace — applying to my own work. This entry closes it.

### What was added, and where

- [history.md](../history.md) — three ledger sections, one per item, each a paragraph per PR with the decision
  it turned on and what is *not* done stated plainly (item 2: no transport; item 5: the remote hook untested,
  `FsStore` out of strict scope; item 6: the harness bugs replaying found). Also: item 3's ADR now cites #243.
- [security.md](../security.md) → *Federated domains — the trust boundary*: D5/D6/D7/D25 as checked things, the
  canonical-encoding reasoning, and the unbuilt transport.
- [companions/wiki.md](../companions/wiki.md) → *The mandate fence*: what is inside the `update-ref --stdin`
  transaction and the push, `Superseded` ≠ `Conflict`, the mutation-fence gate, the two named exempt sites.
- [testing/testing.md](../testing/testing.md) → *Replay scenarios A and B*, and the discipline they established:
  **verify a gate by breaking the thing it guards, in both directions.**

### The one correction carried into the wiki

Scenario B does **not** discharge D4 — I said it did when #260 merged, then read D4 (*"audit `LockService` under
replay scenario B first; no second fence"*) and retracted it. The audit (#261) found the concern's premise wrong
(`distributed_lock` reads back the converged value; losers hold no token) and **amended the adopted record's §6
with a date** rather than rewriting it. The ledger section says so; a reader of §6 will find the amendment
where the claim was.

### Not done here

No pages for `mycelium-reason` or `mycelium-agentfacts` exist under `companions/`; their knowledge adapters are
on the folder-note. The architecture pages (`dev/architecture/`) do not list the v3 modules (`federation`,
`mandate`, `knowledge`) at all — that is a page-structure question for a lint pass, not a coverage fix.
