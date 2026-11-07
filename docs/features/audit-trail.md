# Audit trail and audit export

## What it is

In a build with `compliance`, the gateway seals each governance action it takes into a per-node, hash-linked audit
chain, signed with the node's identity, so an edited or missing record is detectable. The chain is replicated KV
under `sys/audit/`. An external sink can mirror every sealed record to a SIEM or WORM store, keeping the original
bytes, and a signed checkpoint lets old records be pruned while the remaining chain still verifies. The evaluator's
decisions reach the chain only as references to the [evidence journal](evidence-journal.md).

## When to use it, and when not

- **Use it when** you must show who changed the fleet's governance and that the record was not edited afterwards,
  for example under SOC 2.
- **Not when** you need a record of every call's content: the chain holds governance actions and evidence
  references, not payloads, and a slow sink drops its mirror copy, never the chain record
  ([audit § 4](../operations/audit.md#4-export-to-an-external-siem--worm-soc-2-ws-c)).

## Turn it on

This feature is turned on in code: build with `compliance` (on `mycelium`, which turns on `mycelium-core`'s
`compliance` for the signed-identity surface) and give the node a [`tls`](../reference/configuration.md#tls)
identity, which the records are sealed with.

- [`tls`](../reference/configuration.md#tls): without `[tls]` nothing is sealed.
- [`with_audit_sink`](../../src/agent/mod.rs): attach an [`AuditSink`](../../src/agent/audit.rs) before `start()`;
  a sink on a node with no `[tls]` refuses the start by name.
- [`audit_checkpoint`](../../src/agent/mod.rs) and [`audit_prune_to_checkpoint`](../../src/agent/mod.rs): the
  retention flow; export before you prune, because pruning is irreversible on the node.
- The snippets in [audit § 4](../operations/audit.md#4-export-to-an-external-siem--worm-soc-2-ws-c) and § 5 are not
  run as written; the tests below run the same calls.
- `test_audit_sink_mirrors_sealed_records`, in [`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs):
  sealed records reach the sink in order while the chain still verifies.

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`auditor_questions`](../../examples/README.md#the-capability-matrix): who did this, can you prove the log was not
edited, and can you erase one person, each answered by running the mechanism. CI runs it.

## What it guarantees, and what it does not

- **Guarantees:** [`audit.chain`](../reference/guarantee-catalogue.md#audit.chain) — governance actions at the
  gateway sealed into the tamper-evident chain; [`audit.sink`](../reference/guarantee-catalogue.md#audit.sink) —
  sealed records mirrored to an external sink, original bytes kept.
- **Not built:** the chain is per node and proves a record was not edited, not that a node recorded everything it
  did; the sink's mirror copy can be dropped under back-pressure; full verification from genesis after pruning is
  offline, against the exported archive ([audit § 5](../operations/audit.md#5-retention--checkpoint-export-prune-soc-2-ws-d)).
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row for the chain on its own; the
  [guarantee-report row](../operations/what-is-proven.md#node-says-which-guarantees-enforces-why) covers how a node
  reports these two.

## Go deeper

- **Guide:** [09, security](../guide/09-security.md#three-questions-an-auditor-asks).
- **Runbook:** [audit](../operations/audit.md) (recording, querying, verifying, export, retention, failure modes).
- **Design:** no design record; the SOC 2 audit-gap work is the record, and the
  [shared-responsibility matrix](../operations/shared-responsibility-matrix.md) says which controls are the
  substrate's and which the operator's.
- **Code:** [`src/agent/audit.rs`](../../src/agent/audit.rs) (records, chain verification, checkpoints, the sink
  trait).
