# Causal records

## What it is

Two fields a request carries into every record its work produces, and that no record mints — the
[causal fields](../guide/00-concepts.md#causal-records): `correlation`, one key segment that groups the records of one
run, and `parent`, the content hash of the record this work depends on, the same hash an evidence record's
`journal_sha256` cites. A caller puts both under `params._meta`, beside `operation_id` and `mandate` — inside the
application payload, which the caller envelope's signature digests — and the gateway's door, the provider's check and
every execution record carry them as given. `mycelium explain --correlation <id> <files…>` renders one run's records
as one chain across evidence journals and decision traces; `mycelium explain --closure <hash> <files…>` renders the
dependency closure of one record, marking each line captured, reconstructed or unattributable, naming every hole and
stopping at a federation edge.

## When to use it, and when not

- **Use it when** an agent's work is a run of several calls and you want its records — decided, executed, at the
  gateway and at the provider — read as one chain from one id, or when a call acts on a decision and you want the
  record to say so by hash, so a reader can compute what depended on it.
- **Not when** the node attaches no evidence journal: a record's hash is the journal's, so a fleet without journals
  has no closures (the [scaffolds](init.md) attach one). Not for ordering across nodes by time: a record is ordered by
  its own event time where it has one and by file order where it has none, and the reader says which.

## Turn it on

This feature is turned on in code.

- [`causal_fields`](../../src/agent/causal.rs) — reads and validates the two keys under `_meta`; a malformed one is
  refused at the door (`request_malformed`, JSON-RPC `-32602`; `400` at `POST /gateway/kv`), never corrected.
- [`ae_decide`](../../src/agent/http/authz.rs) — the one preflight every door runs: the fields enter the
  [`ActionEnvelope`](../../src/agent/action_evaluator.rs) and thence the [`AeEvidence`](../../src/agent/action_evaluator.rs)
  record, decision and execution alike; `/mcp` forwards them to the provider inside the tool request, where
  [`check`](../../src/agent/provider_enforcement.rs) reads them from the payload.
- [`DecisionRecord`](../../mycelium-core/src/decision.rs) (`correlation`, `parent_hash`) and
  [`WriteReceipt`](../../mycelium-core/src/receipt.rs) (`with_causal`) carry them where a decision point or a write
  has them.
- [`Corpus`](../../src/closure.rs) — the reader behind `mycelium explain --correlation` and `--closure`.
- `the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records`, in
  [`src/agent/http/causal_tests.rs`](../../src/agent/http/causal_tests.rs): a real `/mcp` call with both fields, the
  gateway's and the provider's records read back from the journal carrying them, and a header's copy counted and
  never read.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The CI job `Scaffold` runs the Rust scaffold's call under `_meta.correlation` and reads
its chain back with `mycelium explain --correlation`. The tests: in
[`src/agent/http/causal_tests.rs`](../../src/agent/http/causal_tests.rs),
`the_causal_fields_ride_in_the_payload_to_the_gateway_s_and_the_provider_s_records` and
`a_malformed_causal_field_is_refused_before_policy_and_never_corrected`; in
[`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs),
`a_parent_in_the_envelope_is_counted_not_read_and_one_in_the_payload_is_signed`; in
[`src/closure.rs`](../../src/closure.rs), `a_chain_marks_captured_reconstructed_and_unattributable` and
`a_closure_follows_parents_names_holes_and_stops_at_a_federation_edge`.

## What it guarantees, and what it does not

- A field in the payload reaches every record the decision produces at that point, unchanged; a field carried as a
  header or in the caller envelope is never read, and `GET /stats` `causal_fields_misplaced` counts it by door. A
  `parent` in the payload is covered by the digest the caller envelope's signature binds, so altering it after signing
  is refused as any altered byte is. The record that carries the fields is the one
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md) requires before the dispatch runs.
- The closure is **authoritative over the evidence journal and best-effort over the decision trace**: the journal
  never drops a record; the sink does, counted, and the reader prints those counts beside any hole. A hole is named —
  a journal not given, a record pruned or crypto-shredded, a dropped decision — and never filled. A closure over
  fewer journals than the records name nodes says so in its first line.
- A closure stops at a federation edge: the partner's journals are private and trust does not compose; the edge's
  record is the leaf, printed as a boundary.
- Comparison of two traces ignores the fields: two recordings of one run do not diverge on an id alone.
- **Not built in this release** (the plan row is [`adopter-surface.md`](../plans/adopter-surface.md) X4): the
  **door registry** and the member-side doors it lists (signal and wired emits, `bulk_call`, scatter, mailbox, shard
  and overlay emits, the tuple-space `take`, the blackboard, the stem's activation command) — a member's direct call
  carries the fields only as bytes it does not alter, so the provider reads them from an `mcp.invoke` payload and from
  nothing else; a skill call's payload is its text and carries none, so the provider's record of an `/a2a` call is a
  root; the SDKs' verbs do not yet name the keys; the reason trace's `run_id` is not yet joined; the X2 scaffold's
  chain is read in CI, not asserted across three nodes with plants on each side.

## Go deeper

- Guide: [authorising actions — one id and one parent through the records](../guide/20-authorising-actions.md#one-id-and-one-parent-through-the-records).
- Design: [the action envelope](../design/action-envelope-ae0.md) for the record; the plan row for the placement rule.
- Code: [`src/agent/causal.rs`](../../src/agent/causal.rs), [`src/closure.rs`](../../src/closure.rs),
  [`src/main.rs`](../../src/main.rs).
