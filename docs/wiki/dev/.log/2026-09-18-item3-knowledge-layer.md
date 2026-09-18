## [2026-09-18] ingest | item 3 — the knowledge layer, complete at PR 7

Up: [dev](../dev.md) · record `docs/design/knowledge-layer.md` · plan `docs/plans/v3-contracts-axis.md`
§6.1 · code `src/knowledge/{store,resolution,correction,gate}.rs`, `mycelium-reason/src/knowledge.rs`,
`mycelium-agentfacts/src/knowledge.rs`, `examples/knowledge_layer.rs`.

### What shipped

PRs 4–7 today (#264–#267) on top of PRs 1–3 (the ADR; #254; #255). The contract completes at PR 6 per the
record's own status wording; PR 7 extends it with adapters. Ledger entry: [history](../history.md) → *item 3*.

### What the wiki now says, and where

- [history.md](../history.md) — the ledger section, one paragraph per PR with the decision each turned on.
- [companions/companions.md](../companions/companions.md) — the two adapters, on the `mycelium-reason` and
  `mycelium-agentfacts` bullets (there are no per-crate pages for those two companions; the folder-note is
  where their surface is recorded).
- [wiki.md](../../wiki.md) — the plan-of-record row said rev 1.6 / D1–D32; it is rev 1.12 / D1–D39. Fixed
  (doc-vs-code drift, lint check 1).

### The three things worth carrying forward

1. **Withdrawing a basis is not withdrawing the conclusion.** A `Retracts` link is same-issuer only, but a
   retraction must *reach* records other issuers derived from it. Those become `BasisWithdrawn`, never
   `Retracted` — a fact about support, not a verdict — because issuer A has no standing over issuer B's
   record. Collapsing the two lets anyone silently invalidate anyone's conclusions by retracting something they
   cited: erasure wearing a correction's clothes.
2. **A gate made of refusals needs a positive control inside it.** All three of §8's negative cases pass
   against a resolver that refuses everything — verified by planting one. The controls are part of the gate,
   not beside it.
3. **Adjacency is not derivation, structurally.** The trace adapter's batch form has no parameter through
   which links could be supplied. A helper that "helpfully" chained a run by HLC order would produce a graph
   indistinguishable from an honest one — so the rule is enforced by the shape of the API, not by a comment.

### Corrections made on the way

- A "cycle terminates" test in PR 5 was a DAG: a content-addressed id cannot contain a link that depends on
  itself, so a genuine cycle only arrives from a forged index. The test now builds one by hand and says so.
- The record says expiry timers run on the replay clock seam. They run on nothing — expiry is a pure function
  of the reader's clock and the record's timestamp. Stated rather than claiming to use a seam the code does
  not touch.
- The AgentFacts "still not evidence" test excluded the claim for two reasons (kind *and* no link); it now
  carries a `supports` link so the kind is the only thing excluding it — and a kind swap fails exactly it.

### Coverage gap, named rather than carried

The ledger has no section for **item 2** (federated domains, `src/federation/`), **item 5** (scoped mandates,
`src/mandate/`, incl. the D4 audit that amended the adopted record's §6), or **item 6 PRs 4–5** (the
WAL/snapshot scenario has a log entry but no ledger row; scenario B has neither). All are merged on `main`.
That is lint check 4 (merged work with no wiki trace) applying to this session's own work; it is a separate
coverage ingest, not folded into this one.
