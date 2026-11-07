# Evidence journal

## What it is

The [evidence journal](../guide/00-concepts.md#evidence-journal) is where an enforcement point records what it
decided and what became of the action. It is node-local, append-only and fsynced, and it never gossips. What
enters the tamper-evident audit chain is a reference carrying the journal record's content hash, so the chain still
covers the evidence while the evidence itself stays on the node until an exporter reads it. It is one profile of the
node-local [journal](../guide/00-concepts.md#journal), not a replay bundle.

## When to use it, and when not

- **Use it when** a node has an [action evaluator](action-authorisation.md) attached: without a journal the evaluator
  enforces but does not record, and warns at attach time.
- **Not when** you want a fleet-wide record: the journal is one node's, and evidence leaves it only through an
  exporter or the by-hash read ([audit § 10](../operations/audit.md#10-evidence-export--what-a-gap-looks-like-to-the-consumer)).

## Turn it on

This feature is turned on in code, in a build with `gateway` and `tls`.

- [`EvidenceJournal::open`](../../src/agent/evidence_journal.rs): open or create the journal at a path, with a
  failure profile.
- [`EvidenceProfile`](../../src/agent/evidence_journal.rs): `Strict` refuses a dispatch whose decision could not be
  recorded; `Lenient` lets it proceed without an acknowledged record, and under `compliance` the chain's reference
  says which: `Unknown` when the journal did not acknowledge in time (the record may exist), `NotEstablished` when
  it reported a failure or its queue was full.
- [`with_evidence_journal`](../../src/agent/mod.rs): attach it before `start()`; a second journal is ignored with a
  warning.
- [`read_evidence_journal_from`](../../src/agent/evidence_journal.rs): a bounded page from a cursor, the exporter's
  read.
- `a_failing_journal_refuses_under_strict_and_is_declared_under_lenient`, in
  [`src/agent/http/ae_seam_tests.rs`](../../src/agent/http/ae_seam_tests.rs): both profiles against a journal whose
  writer accepts nothing.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for the
journal on its own; the examples that attach one do so beside an evaluator. The end-to-end run is a test in
[`src/agent/http/ae_seam_tests.rs`](../../src/agent/http/ae_seam_tests.rs):
`evidence_goes_to_the_journal_and_only_a_reference_gossips`, which reads the journal and the gossiped chain after a
gateway call. Reading one record back by its hash is
`an_evidence_record_is_served_from_the_journal_and_verified_against_the_chain`, in
[`src/agent/http/record_read_tests.rs`](../../src/agent/http/record_read_tests.rs).

## What it guarantees, and what it does not

- **Guarantee:** [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md#ae.recorded_before_dispatch) —
  every decision journalled, fsynced, before dispatch. The promise holds under `EvidenceProfile::Strict`; under
  `Lenient` a dispatch proceeds without the acknowledged record.
- **Not built:** evidence does not survive the loss of the node's disk (the journal is node-local by design), and the
  chain's reference proves a record matches its hash, not that every decision was exported — a gap is visible to
  the consumer, as [audit § 10](../operations/audit.md#10-evidence-export--what-a-gap-looks-like-to-the-consumer)
  describes.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the journal on its own;
  its authority rows rest on it.

## Go deeper

- **Guide:** [20 § Wiring it up](../guide/20-authorising-actions.md#wiring-it-up) and
  [§ The two evidence profiles](../guide/20-authorising-actions.md#the-two-evidence-profiles).
- **Runbook:** [audit § 3a, A record by its hash](../operations/audit.md#3a-a-record-by-its-hash-the-spans-reference)
  and [§ 10, Evidence export](../operations/audit.md#10-evidence-export--what-a-gap-looks-like-to-the-consumer).
- **Design:** [`action-envelope-ae0.md`](../design/action-envelope-ae0.md) §5.
- **Code:** [`src/agent/evidence_journal.rs`](../../src/agent/evidence_journal.rs) (the profile) over
  [`src/agent/journal.rs`](../../src/agent/journal.rs) (the mechanism).
