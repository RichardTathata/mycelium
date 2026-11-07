# AgentFacts

## What it is

[AgentFacts](../guide/00-concepts.md#agentfacts) are a node's public, self-signed description of itself — its advertised
capabilities, its Ed25519 identity key, an optional locality and its public endpoints — served as JSON-LD at
`GET /.well-known/agent-facts.json`. A fetcher in another domain verifies the signature against the key the document
embeds; there is no shared authority to consult, so two domains under separate CAs discover each other without sharing a
root, and trust stays the fetcher's decision. A second route, `/.well-known/agent-facts/domain.json`, serves the whole
domain's board: every node publishes per-field signed facts that gossip inside the domain, and a field is listed only if
it verifies against a key the serving node trusts for its author now — a revoked key's fields drop out. A tampered copy
fails verification: detection, not prevention.

## When to use it, and when not

- **Use it when** agents in other organisations should find what your fleet offers — a neighbouring co-op discovering
  your `route/optimize` — with no registry between you.
- **Not when** the other domain should be allowed to call you: that is [federation](federation.md), bilateral and
  credentialed; AgentFacts grant nothing, and are public by design.

## Turn it on

This feature is turned on in code, from the `mycelium-agentfacts` crate, on a node with a `tls` identity — without one
the route answers `503`.

- [`agent_facts_router`](../../mycelium-agentfacts/src/http.rs) with [`FactsOptions`](../../mycelium-agentfacts/src/lib.rs)
  (`endpoints`, `locality`, `ttl_secs`, an optional `revocation` head), mounted with `with_http_routes` before `start()`.
  The routes sit outside the `/gateway` scope wall: no token reaches them and none is needed.
- [`publish_field`](../../mycelium-agentfacts/src/crdt.rs) to add a signed field to the domain board;
  [`read_verified_fields`](../../mycelium-agentfacts/src/crdt.rs) and [`domain_facts`](../../mycelium-agentfacts/src/crdt.rs)
  to read it verified.
- On the fetching side, [`SignedFacts::verify`](../../mycelium-agentfacts/src/lib.rs); [`claim`](../../mycelium-agentfacts/src/knowledge.rs)
  turns a verified document into a [knowledge-layer](knowledge-layer.md) claim issued by its key.
- `a_tampered_document_produces_no_record`, in [`mycelium-agentfacts/src/knowledge.rs`](../../mycelium-agentfacts/src/knowledge.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`federation_facts`](../../examples/README.md#the-capability-matrix): two clusters that never peer — one advertises,
the other pulls, verifies, routes its overflow there, and a tampered copy fails. CI runs it.

## What it guarantees, and what it does not

- No guarantee or rule id of its own.
- **Not built:** trust — a valid signature says which key wrote the facts, not that the key belongs to who you think,
  nor that the capabilities work; revocation for an outside fetcher of a single node's document, beyond the optional
  revocation head it can check against the node's transparency endpoint; a document is a snapshot, re-pulled after
  `ttl_secs`.
- **Evidence:** the ledger row
  [a key a node has revoked stops verifying](../operations/what-is-proven.md#key-node-has-revoked-stops-verifying),
  whose tests include the domain board's: a field signed by a key written into the raw identity record does not verify.

## Go deeper

- **Guide:** [17 § Part 1 · Public discovery — AgentFacts](../guide/17-federation.md#part-1--public-discovery--agentfacts).
- **Runbook:** no runbook of its own; [companions](../operations/companions.md).
- **Design:** no design record; the guide is the record.
- **Code:** [`mycelium-agentfacts/src/`](../../mycelium-agentfacts/src/lib.rs) (`lib.rs`, `http.rs`, `crdt.rs`,
  `knowledge.rs`).
