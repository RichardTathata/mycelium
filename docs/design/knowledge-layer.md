# The knowledge layer — recording, judging, and what evidence is allowed to decide (ADR, item 3 PR 1)

**Status:** adopted 2026-09-17 · **item 3 PR 1** of `docs/plans/v3-contracts-axis.md` §6.1. A contract, not an
API. It cites [`threat-model.md`](../threat-model.md) (item 8) as the boundary model, and depends on
[`contracts-receipts.md`](contracts-receipts.md) (item 1, receipts) and the clock seam from item 6. It sits
*beside* [`action-envelope-ae0.md`](action-envelope-ae0.md): AE decides **may this happen**, this record decides
**what do we know, and how sure are we** — and the two never borrow each other's authority (§6).

> **Posture, once.** PR 1's typed records are an **initial API release, not completion of the knowledge
> contract**. The contract is complete at PR 6 (the demonstration); adapters (PR 7) extend it. Nothing here
> changes the wire. The layer's most attractive claim — *substantive disagreement is preserved while useful
> coordination continues* — is also its least evidenced, and §8 says exactly how much of it is checked.

## 1. Four record types, because judging is not recording

| Record | What it is |
|---|---|
| **claim** | an issuer asserts something |
| **observation** | an issuer reports what it saw |
| **assessment** | an issuer *judges* a claim or observation |
| **acceptance decision** | a reader decides what to do about it |

The split that matters is **assessment from observation**. Collapsing them is how "three nodes reported an
error" silently becomes "the service is broken" with no one having said so and no one accountable for the
inference. Each record is **signed and immutable**; there is no edit, only a further record.

Links are explicit, and there are exactly six: `supports` · `challenges` · `derived_from` · `supersedes` ·
`retracts` · `adopts`.

**An issuer retracts only its own statements.** Retraction is not moderation. A record that could retract
another issuer's statement would make the layer a consensus mechanism over truth, which §9 refuses.

## 2. Heads in the medium, records in a store

The gossip KV namespace carries **bounded, signed discovery heads** and nothing else:

```
knowledge/head/{issuer}/{stream}
```

Reserved at PR 1 in both front doors — `kv_ns::KNOWLEDGE_HEAD` (`mycelium-core/src/signal.rs`) and the
crate-doc namespace table (`src/lib.rs`) — before any code writes it, so the shape is settled while it is still
free to settle.

The records and their evidence live in an **authorized store**. This is the load-bearing decision of the whole
record, and the reason is one sentence: **LWW moves a pointer, and moving a pointer cannot erase a competing
statement.** Put records in KV and last-write-wins becomes last-writer-is-right — two issuers who disagree
resolve to whichever had the later HLC, which is not a truth procedure, it is a clock race.

So: **equivocation is preserved, never HLC-resolved.** The layer can say *"these two issuers disagree"*,
because both statements still exist.

## 3. Evidence-aware resolution wraps `resolve_for_caller`

`src/agent/capability_handle.rs:99` already applies the native gates — `is_fresh`, then the schema-id gate.
Evidence-aware resolution **wraps** it; it does not replace it. The order is fixed:

1. **native gates first** (`is_fresh`, schema) — an entry that fails them never reaches evidence evaluation;
2. **bind to the exact release** being considered;
3. **verify and classify** the evidence for that release;
4. **evaluate a deterministic reader policy**;
5. return **`Accepted` · `Rejected` · `InsufficientEvidence` · `Conflicted`** — *with reasons*;
6. **compose with load and locality** — rank *within* the accepted class, then hand survivors to the reasoning
   router's load/reservation ranking.

Step 6's order is the one that would otherwise be got wrong: evidence decides *which candidates are eligible*,
the router decides *which eligible candidate to use*. Interleaving them would let a well-evidenced but
overloaded provider beat a fine one, or the reverse.

**Evidence never grants what authorization denies.** If `resolve_for_caller` or the AE evaluator says no, no
quantity of supporting evidence changes that. Evidence can only ever *narrow* the set.

## 4. Four rules about what evidence means

- **Competence is contextual. There is no reputation scalar.** "Good at X" does not imply "good at Y", and a
  single number asserts that it does. Aggregated reputation is deferred (§9).
- **Identity is not independence.** Two issuers with different keys may be the same operator, the same
  deployment, or the same upstream. Independence is a **reader-configured control group**, not something the
  layer infers.
- **Missing evidence is uncertainty, not a verdict.** `InsufficientEvidence` is a distinct outcome from
  `Rejected` precisely so a reader can tell "we looked and it is bad" from "we do not know".
- **Refreshing an advertisement never refreshes evidence.** The capability refresh is the evaporation lease —
  it says the provider is *alive*, not that anything is still *true*. Conflating them would let a liveness
  heartbeat launder a stale assessment into a current one.

**Expiry and correction are active**, with a dependency index, so a retraction reaches what was derived from
the retracted record rather than waiting to be noticed.

## 5. Reuse, not re-invention

Three things already exist and are reused rather than duplicated:

- **Signatures** — `mycelium-core/src/tls.rs:474` (`verify_bytes`) is public, and AgentFacts is already Ed25519
  self-signed. No new signing stack.
- **Schema identity** — `schemas/{schema_id}` is the schema **locator**, plus a content digest. **Not a second
  registry.**
- **Confidential evidence addressing** — `mycelium-reason/src/blob.rs` verifies on read behind `llm:read`, and
  the hash is still the credential. That is *why* the plan's opaque-address rule for confidential evidence is
  necessary rather than optional: a guessable address is a readable one.

**Expiry timers run on the replay clock seam** (`sim_seam`, item 6), not `tokio::time`, so an expiry is a
schedulable event a recorded run can reproduce instead of a wait a test has to sit through.

## 6. The trace adapter adds links; it does not import HLC order as causation

`mycelium-reason`'s `TraceEvent { hlc, node, kind, detail }` has **no parent link** — verified at adoption
(`mycelium-reason/src/trace.rs:48`). Its "causal story" is HLC *adjacency*, which is an ordering, not a cause.

The adapter therefore **adds `derived_from` links explicitly**. Treating adjacency as derivation would
manufacture causal claims out of timestamps — exactly the inference §1 split the record types to prevent, and
it would be invisible afterwards because the resulting graph would look identical to one someone asserted.

## 7. What lands next

| PR | Contents |
|---|---|
| **1** *(this record)* | the ADR; `knowledge/` reserved in the namespace table and both front-door lists |
| 2–3 | the typed records and the store — the **initial API release** |
| 4–6 | evidence-aware resolution, expiry/correction, the demonstration (**the contract completes here**) |
| 7 | adapters |

## 8. Two gates, and they are not the same claim

The layer's headline claim gets a **semantic gate** and a **behavioural experiment**, kept apart because they
prove different things.

**Semantic — a Phase D exit condition, in CI.** Three negative cases, replayed: misleading evidence cannot
silently *erase* a conflicting observation, cannot *refresh* expired evidence, and cannot *confer* authority.
These are properties. They either hold or they do not.

**Behavioural — research track (§13), not a v3 deliverable.** Whether evidence-sensitive resolution improves
outcomes under misleading self-advertisements, correlated observers, stale successful histories and
contradictory task-specific results — measured as **errors and opportunity costs**: bad selections, unnecessary
refusals, completion quality, evaluation overhead.

The last of those is the one that keeps the experiment honest: **a resolver that rejects everything looks safe
while being useless**, and a metric that counted only bad selections would score it perfectly. The experiment
can support a *bounded* empirical claim. It can never establish that evidence-aware selection is always better,
and this record does not say it is.

## 9. What this record refuses

- **An aggregated reputation scalar** — deferred, and §4 says why it is not merely unimplemented.
- **Inferred independence** — the reader configures control groups; the layer does not guess.
- **Mandatory LLM judgment** — an assessment may be produced by a model, but nothing in the layer requires one,
  and a model's assessment is a record by an issuer like any other.
- **Consensus over truth** — the layer records and resolves; it does not vote on what is true.
- **Records in the gossip KV namespace** — heads only (§2). This is the invariant the rest depends on.
- **Any claim that disagreement is *resolved*.** It is *preserved*. Coordination continues around it.

## Appendix — anchors verified at adoption (2026-09-17)

| Claim | Where |
|---|---|
| `resolve_for_caller` applies `is_fresh` then the schema gate — the wrap point | `src/agent/capability_handle.rs:99` |
| signature verification is public and already Ed25519 | `mycelium-core/src/tls.rs:474` (`verify_bytes`) |
| blob reads verify, behind `llm:read`; the hash is the credential | `mycelium-reason/src/blob.rs` |
| `TraceEvent` has **no** parent link | `mycelium-reason/src/trace.rs:48` — `{ hlc, node, kind, detail }` |
| `knowledge/` reserved in the code registry | `mycelium-core/src/signal.rs` — `kv_ns::KNOWLEDGE_HEAD` |
| `knowledge/` reserved in the crate-doc table | `src/lib.rs` |
