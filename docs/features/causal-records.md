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

- A field in the payload reaches every record the decision produces at that point, unchanged, and the record names
  the node that enforced (`node`). A field carried as a header is never read and is counted once per request against
  the door that saw it — `/mcp`, `/a2a`, the two LLM doors, the federation call and `POST /gateway/kv` — and one in
  the caller envelope against `caller_envelope`, after its signature held (`GET /stats` `causal_fields_misplaced`, a
  map by door). A `parent` in the payload is covered by the digest the caller envelope's signature binds, so altering
  it after signing is refused as any altered byte is. `POST /gateway/rpc/call` runs no preflight and reads no `_meta`:
  it carries the bytes to the provider unaltered, and refuses the protected kinds. The record that carries the fields is the one
  [`ae.recorded_before_dispatch`](../reference/guarantee-catalogue.md) requires before the dispatch runs.
- The closure is **authoritative over the evidence journal and best-effort over the decision trace**: the journal
  never drops a record; the sink does, counted, and the reader prints those counts beside any hole. A hole is named —
  a journal not given, a record pruned or crypto-shredded, a dropped decision — and never filled. A closure over
  fewer journals than the records name nodes says so in its first line.
- A closure stops at a federation edge: the partner's journals are private and trust does not compose; the edge's
  record is the leaf, printed as a boundary.
- Comparison of two traces ignores the fields: two recordings of one run do not diverge on an id alone.
- **The provider's parent is the gateway's decision.** `/mcp` forwards, as the `parent` of the `tools/call` it
  sends, the content hash of its own decision record; the caller's parent stays on the gateway's record. So the
  provider's record cites the gateway's, and a closure read from the two journals chains them
  (`tests/causal_closure.rs`: three nodes, every dependent record named and no independent one, shown by a plant on
  each side; a saturated decision sink on the provider shows a cited dropped hash as a hole with the sink's counts,
  the journal-side closure unchanged).
- **The door registry** — [`scripts/causal-doors.txt`](../../scripts/causal-doors.txt), gated by
  [`check-causal-doors.py`](../../scripts/check-causal-doors.py) in `make check` and CI — lists every door that
  starts governed work and how the fields travel through it: `record+fwd` (`/mcp`), `record` (`/a2a` on either
  generation's `_meta`, the LLM doors, the federation call, the provider check, the public `preflight`), `bytes` (a
  member's `rpc_call`, `bulk_call`, `scatter_gather`: the application bytes pass unaltered and the provider reads an
  `mcp.invoke` payload), `refused` (`/gateway/rpc/call`, `scatter`: protected kinds), `none` (signal, mailbox, shard
  and overlay emits: no record is written for them). Each row names the test that shows it, or states the gap: the
  LLM stream door, the federation call, the routed public door, `bulk_call` and `scatter_gather` are listed untested.
  A door the scanner finds without a row fails the gate; a planted one fails its self-test by name.
- The replay bundle carries `decisions.jsonl` as bytes, so the two fields ride through it by construction; the
  comparison ignores them. `POST /gateway/kv` echoes the fields on its receipt, and the SDKs carry them:
  `mycelium-py` 0.2.13 (`A2aClient.send`/`send_subscribe` and `set` take `correlation=`/`parent=`, `KvReceipt`
  echoes them) and `mycelium-ts` 0.2.7 (`SendOptions`, `set(key, value, { correlation, parent })`). A traced reason
  run's `run_id` is its `correlation` (`mycelium-reason` 0.11.0).
- **Not built** (the plan row is [`adopter-surface.md`](../plans/adopter-surface.md) X4): the provider writes no
  execution record and no receipt is journaled, so "executed, with the receipt's rung" is not read off a chain; a
  skill call's payload is its text and carries none, so the provider's record of an `/a2a` call is a root; the
  stem's activation command does not yet take the install decision's hash as its parent; the tuple-space `take` and
  the blackboard write no record and are outside the registry's scan.

## Go deeper

- Guide: [authorising actions — one id and one parent through the records](../guide/20-authorising-actions.md#one-id-and-one-parent-through-the-records).
- Design: [the action envelope](../design/action-envelope-ae0.md) for the record; the plan row for the placement rule.
- Code: [`src/agent/causal.rs`](../../src/agent/causal.rs), [`src/closure.rs`](../../src/closure.rs),
  [`src/main.rs`](../../src/main.rs).
