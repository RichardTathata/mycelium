## [2026-09-17] ingest | item 3 PR 1 — the knowledge-layer ADR, and a namespace reserved before anything needs it

Up: [dev](../dev.md) · record `docs/design/knowledge-layer.md` · plan
`docs/plans/v3-contracts-axis.md` §6.1 · code `mycelium-core/src/signal.rs` (`kv_ns`), `src/lib.rs`.

Second of the three PR-1 ADRs Phase A's exit gate requires (*"item 8 published and cited by items 2/3/5's PR-1
ADRs"*). Item 2's landed earlier today; item 5's remains.

### The load-bearing decision

**Heads in the gossip medium, records in an authorized store.** One sentence carries it: *LWW moves a pointer,
and moving a pointer cannot erase a competing statement.*

Put the records themselves in KV and last-write-wins becomes **last-writer-is-right** — two issuers who
disagree resolve to whichever had the later HLC. That is a clock race, not a truth procedure, and afterwards
the losing statement is simply gone. With heads only, both statements survive and the layer can say *"these two
issuers disagree"*, which is the thing it exists to be able to say.

`knowledge/head/{issuer}/{stream}` is **reserved at PR 1 in both front doors** — `kv_ns::KNOWLEDGE_HEAD` and
the crate-doc namespace table — before any code writes it. Reserving a prefix while it is still free to settle
costs nothing; discovering at PR 4 that something else took it costs a migration.

### The split that gives the layer its name

Four record types, and the one that matters is **assessment separated from observation**. Collapsing them is
how *"three nodes reported an error"* silently becomes *"the service is broken"* — with no one having said so,
and no one accountable for the inference. Records are signed and immutable; there is no edit, only a further
record. An issuer **retracts only its own statements**, because retraction that reached other issuers would
make this a consensus mechanism over truth, which §9 refuses.

The same concern shapes the trace adapter. `mycelium-reason`'s `TraceEvent { hlc, node, kind, detail }` has
**no parent link** — its causal story is HLC *adjacency*, which is an ordering, not a cause. So the adapter
adds `derived_from` links **explicitly** rather than importing adjacency as causation. Manufacturing causal
claims from timestamps would be invisible afterwards, because the resulting graph would look identical to one
someone had actually asserted.

### The honest part of the gate

The layer's headline claim — *substantive disagreement is preserved while useful coordination continues* — is
also its least evidenced, so the record keeps **two gates apart**:

- **semantic**, a Phase D CI condition: misleading evidence cannot erase a conflicting observation, refresh
  expired evidence, or confer authority. Three replayed negative cases. Properties, so they hold or they do
  not.
- **behavioural**, research track: whether evidence-sensitive resolution improves outcomes, measured as errors
  **and opportunity costs**.

The opportunity-cost half is what keeps the experiment honest: **a resolver that rejects everything looks safe
while being useless**, and a metric counting only bad selections would score it perfectly. The experiment can
support a bounded claim; it can never establish that evidence-aware selection is always better, and the record
says so rather than implying otherwise.

### A methodological note on the anchors

Three of §6.1's cited anchors — `blob.rs`, `TraceEvent`, the front-door lists — appeared to be missing when
first searched, and were not. They live in the **`mycelium-reason` companion**, which the search had not
covered.

That is the second time today the "it does not exist" reflex was mine rather than the repository's (the first:
claiming the WAL-merge path had no test, when the coverage was in a different test module). Both times the
correction cost one more search. Worth stating as a habit rather than an incident: **a missing anchor is a
claim, and it needs the same verification as a present one** — the search that found nothing is evidence about
the search first, and about the code second.

### Gates

`make check` clean · core **186** · mycelium **522** (`compliance,a2a`). Docs plus two reservation constants;
no behaviour change.
