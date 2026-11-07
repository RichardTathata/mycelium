# The knowledge layer

## What it is

The knowledge layer keeps what a fleet knows as signed, immutable records of four kinds — claims, observations,
[assessments](../guide/00-concepts.md#assessment) and acceptance decisions — joined by six link kinds. A record's id is
derived from its content and names its issuer, so there is no edit, only a further record. A reader resolves a
question by its own policy: only assessments count as evidence, an issuer's own record never supports itself, two
issuers in one control group are one voice, and a record whose issuer's key was revoked, or which was retracted,
stops counting. Competing statements survive side by side — heads live in the gossip medium and records in an
authorised store, so a later write cannot erase an earlier one — and `Conflicted` is a real answer.

## When to use it, and when not

- **Use it when** agents' findings must stay attributable and correctable — a basis retracted, and what was built on it
  told — and a reader needs to know whether something is supported, not only who said it last.
- **Not when** you want the layer to decide what is true: it attributes and classifies; it is not a truth oracle, and a
  signature says who said it, not whether they were right.

## Turn it on

This feature is turned on in code, in a build with `tls` (`mycelium::knowledge` verifies Ed25519 signatures).
`mycelium-reason`'s `knowledge` feature adds an adapter that turns its reasoning traces into observation records.

- [`KnowledgeRecord`](../../src/knowledge.rs), with its [`RecordKind`](../../src/knowledge.rs) and
  [`LinkKind`](../../src/knowledge.rs).
- [`classify`](../../src/knowledge/resolution.rs) with a [`ReaderPolicy`](../../src/knowledge/resolution.rs): a reader's
  verdict on a question.
- [`DurableKnowledgeStore`](../../src/knowledge/durable.rs): the records on the node-local journal; the semantic gate is
  [`src/knowledge/gate.rs`](../../src/knowledge/gate.rs).
- [`observations`](../../mycelium-reason/src/knowledge.rs) (`mycelium-reason`, `knowledge`): trace events as observation
  records, with `derived_from` links only where the caller asserts them.
- `two_issuers_in_one_control_group_are_not_two_independent_voices`, in
  [`src/knowledge/resolution.rs`](../../src/knowledge/resolution.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`knowledge_layer`](../../examples/README.md#the-capability-matrix): claims and observations arrive, an assessment
judges them, a reader resolves by its policy, a basis is retracted and the correction reaches what was built on it,
and a competing statement survives. CI runs it, and `make gate-knowledge` runs the semantic gate in its own step.

## What it guarantees, and what it does not

- No guarantee or rule id of its own: the knowledge layer registers nothing in the
  [guarantee catalogue](../reference/guarantee-catalogue.md); a record signed by a key revoked after it was stored stops
  counting ([node identity](node-identity.md) for revocation).
- **Not built:** evidence that resolving this way improves outcomes — a research question, not a deliverable;
  protection against a dishonest issuer beyond attribution; last-writer-wins, deliberately
  ([guide 23 § What this does not establish](../guide/23-knowledge.md#what-this-does-not-establish)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the knowledge layer; its CI
  evidence is the semantic gate (`make gate-knowledge`) and the resolution tests, among them
  `a_key_revoked_after_storage_removes_the_record_from_the_count`.

## Go deeper

- **Guide:** [23, knowledge](../guide/23-knowledge.md).
- **Runbook:** [companions](../operations/companions.md).
- **Design:** [`design/knowledge-layer.md`](../design/knowledge-layer.md).
- **Code:** [`src/knowledge.rs`](../../src/knowledge.rs) and [`src/knowledge/`](../../src/knowledge/resolution.rs);
  the trace adapter in [`mycelium-reason/src/knowledge.rs`](../../mycelium-reason/src/knowledge.rs).
