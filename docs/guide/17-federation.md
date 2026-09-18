# 17 — Federation: cross-domain discovery with self-certified AgentFacts

## Concept

Two **separate domains** — separate clusters, separate auto-CAs, they do **not** peer — still need
to discover each other's capabilities. A2A (chapter 08) is *call-me* interop between agents already
in reach; **AgentFacts is *discover-me* across a trust boundary**. A neighbouring co-op with
overflow it can't route discovers your `route/optimize` capability the way a NANDA-style quilt does:
it **pulls your AgentFacts at the edge** (`GET /.well-known/agent-facts.json`) — a self-signed
JSON-LD document — and **verifies the signature itself**.

There is **no shared trust authority**. The facts are self-certified by your node's Ed25519
identity, and trust is the *fetcher's* decision. That is exactly what lets two domains with
different CAs federate discovery without federating their trust roots — see
[00 · Concepts → Why A2A / MCP / AgentFacts are *not* the same](00-concepts.md#why-a2a--mcp--agentfacts-are-not-the-same-thing).
This is the `mycelium-agentfacts` crate.

```mermaid
sequenceDiagram
    participant B as domain B (coop-b)<br/>separate cluster + CA
    participant A as domain A (coop-a)<br/>edge gateway
    A->>A: advertise route/optimize; mount agent_facts_router
    B->>A: GET /.well-known/agent-facts.json
    A-->>B: self-signed JSON-LD {capabilities, identity_pubkey, sig}
    B->>B: verify signature against the embedded key (no shared CA)
    B->>B: read capability list → route overflow to A
    Note over B: a tampered copy fails verify() — detection, not prevention
```

## Serve your facts (mount the edge)

The edge runs **dark** — nothing is published until you mount the router and start the node. Mount
it **before** `start`, and give it the facets the substrate doesn't itself know (your public edge
URLs, an optional jurisdiction, the publish TTL):

```rust
use mycelium_agentfacts::{agent_facts_router, FactsOptions};

let opts = FactsOptions {
    endpoints: vec![format!("http://{host}/.well-known/agent-facts.json")],
    locality:  Some("southwark".into()),   // jurisdiction / zone the facts carry
    ttl_secs:  30,                          // the quilt re-pulls after this (Cache-Control max-age)
    ..Default::default()                    // revocation: None (see below)
};
agent.with_http_routes(agent_facts_router(agent.clone(), opts));
agent.start().await?;
```

This mounts **two public routes**, deliberately *outside* the `/gateway` scope wall (AgentFacts are
meant to be publicly fetchable and cryptographically verified, never token-gated):

| Route | What it returns |
|---|---|
| `/.well-known/agent-facts.json` | **this** node's freshly built, whole-document-signed facts (its own advertised `cap/{self}/…`, identity pubkey, locality, endpoints, TTL) |
| `/.well-known/agent-facts/domain.json` | the converged **multi-author** board — every node's per-field-signed facts as gossiped intra-domain (see below) |

> **Requires a `tls` node identity.** AgentFacts are self-certified, so the signing key *is* the
> node's Ed25519 identity. Without one the route returns `503` (the node is up but has nothing to
> self-certify) — not a silent empty doc. The identity is auto-generated on first start (chapter
> 09); you don't touch a CA toolchain.

## Pull and verify another domain

The fetcher needs nothing from you but the URL. It reconstructs the signed document and checks the
signature against the **embedded** public key — the whole point: no issuer to consult.

```rust
let body = reqwest::get(&a_facts_url).await?.text().await?;
let v: serde_json::Value = serde_json::from_str(&body)?;
let signed = mycelium_agentfacts::SignedFacts {
    document:       v["document"].clone(),
    alg:            "ed25519",
    public_key_b64: v["public_key_b64"].as_str().unwrap().to_string(),
    signature_b64:  v["signature_b64"].as_str().unwrap().to_string(),
};

assert!(signed.verify());                                  // verifies against its OWN embedded key
let caps = signed.document["capabilities"].as_array().unwrap();
if caps.iter().any(|c| c["id"] == "route/optimize") {
    // discovered + verified across a trust boundary → route overflow here
}
```

Tampering is caught, not prevented (the substrate posture): flip any field and `verify()` returns
`false`.

```rust
let mut forged = signed.clone();
forged.document["jurisdiction"] = serde_json::json!("forged-zone");
assert!(!forged.verify());   // the signature no longer covers the document
```

## The trust model — self-certified, no issuer authority

- **The signature is the node's identity.** The document embeds the Ed25519 `identity_pubkey`; a
  fetcher verifies against *that*. There is no CA, no registry, no issuer to trust — trust is the
  fetcher's decision. Two domains with separate auto-CAs discover each other without sharing a root.
- **`cluster` field** — carries the node's `cluster_name` ([chapter 13](13-cluster-topology.md)) so
  a fetcher can tell *which environment* a node belongs to. Omitted when unset.
- **Optional revocation head (WS-D / D3).** Surface `agent.revocation_head()` (the `compliance`
  feature) in `FactsOptions::revocation` so a fetcher can check "is this domain's key set current?"
  and verify inclusion proofs against the live `/gateway/transparency` endpoint — still no new trust
  authority. The substrate-shaped facts stay independent of the feature (pass it in, or don't).

## The multi-author domain board

`/.well-known/agent-facts.json` is one node's view. `/.well-known/agent-facts/domain.json` is the
**converged CRDT board**: every node PUSHes per-field-signed facts intra-domain, they gossip, and
any one node can serve the whole domain's verified facts — each field independently checkable. This
ties PUSH → PULL: a quilt fetches *one* edge and sees the *whole* domain.

```rust
use mycelium_agentfacts::{publish_field, read_verified_fields, domain_facts};

publish_field(&agent, "jurisdiction", serde_json::json!("southwark"));  // signed by this node
let mine   = read_verified_fields(&agent, &node_id, 30_000);            // this node, verified
let board  = domain_facts(&agent, 30_000);                              // every node's verified facts
```

Each field is verified with `verify_any(&pubkeys)` against the domain's known identity keys, so a
forged or stale field simply doesn't appear on the board.

## Run it

The two-domain demo runs the whole arc — advertise → pull → verify → route → tamper-fails — with
two clusters that never peer:

```bash
cargo run -p mycelium-coop-examples --bin federation_facts
```

## Where next

- [00 · Concepts](00-concepts.md#why-a2a--mcp--agentfacts-are-not-the-same-thing) — the A2A vs MCP
  vs AgentFacts distinction (native mesh call vs external tool call vs cross-domain discovery).
- [08 · A2A interop](08-a2a-interop.md) — the *call-me* side (LangChain / AutoGen on the mesh).
- [09 · Security](09-security.md) — the Ed25519 node identity that does the self-certifying.
- Operators: [observability → Viewing AgentFacts](../operations/observability.md#viewing-agentfacts)
  — pulling and inspecting the served documents from the ops side.

---

## The other half: who may *invoke* what (v3 item 2)

Everything above is the **discover-me** edge: AgentFacts, self-certified, pulled by anyone, and
deliberately un-gated. `docs/design/federated-domains.md` adds the **invoke** edge beside it, and
the boundary between them is a decision (D25), not an accident:

> **NANDA is what a domain says about itself, verifiable by any fetcher. Federation is who may
> invoke what, between partners.**

So federation does **not** add a second well-known document, a registry or a trust-registry service.
A `DomainDescriptor`'s public subset is an AgentFacts profile through the existing serializer; trust
bundles stay bilateral operator configuration.

### What exists today

`mycelium::federation` — the contract, and the first arm of its transport:

| Module | What it decides |
|---|---|
| `federation` | `DomainId`, the signed `DomainDescriptor`, the revisioned `DomainPolicy`, the `TrustBundle` (with rotation and revocation) |
| `federation::catalog` | what a partner may **see** (filter, then publish) and what this gateway has **observed**, with expiry |
| `federation::call` | the credential bound to *this call*, and the provider adapter that preserves `origin` |
| `federation::gateway` | per-partner budgets, failover only for repeatable exports, `DeliveryUnknown` |
| `federation::session` | the partition/reconnect state machine — work refused until discovery refreshes |
| `federation::edge` (`tls`) | the provider side of the transport: the credential's wire form (`PresentedCall`, one header on `/a2a`), `FederationEdge` — authenticate at the auth layer, authorise in the handler — and the filtered catalogue on `GET /federation/catalog` |
| `federation::client` (`gateway` + `tls`) | the consumer side: `FederationClient` drives link → resolver → pool → HTTP, and turns a silent gateway into `DeliveryUnknown` or a failover by repeatability |

Serve federated calls from a node:

```rust
let edge = Arc::new(FederationEdge::new(
    alpha,                                   // this domain
    ["invoice.submit", "invoice.status"],    // exports = A2A skill ids
    policy,                                  // who is granted what
    TrustBundle::trusting([(beta, beta_key)]),
    CallPolicy::default(),
));
let agent = GossipAgent::new(id, cfg).with_a2a().with_federation_edge(edge); // before start()
```

Call one from another domain:

```rust
let client = FederationClient::new(beta, "svc/billing", beta_signing_key, alpha,
    vec![GatewayEndpoint { id: "gw-1".into(), base_url: "https://alpha-gw-1:8443".into() }],
    /* slots per partner */ 4, /* catalogue freshness */ Duration::from_secs(60));
let granted = client.connect().await?;                       // the catalogue is the grant
let reply = client.call("invoice.submit", text, Repeatability::AtMostOnce).await?;
```

A call before `connect` is refused with no HTTP (`Link(Down)`); an export the catalogue never named is
refused with no HTTP (`Resolve`); a credential the partner does not trust, or one minted for another
export, is refused *at the partner's gateway* with no dispatch. The provider is handed
`federation:beta.example/svc/billing` — never "the gateway".

Run it:

```bash
cargo run --example federated_domains --features tls
```

The example walks the whole lifecycle and prints what each step decided *and why*.

### What does not exist yet

**The release gate, and what of it is met.** PR 8 (2026-09-18) put the first bytes across; PR 9 (same day)
ran the gate's whole choreography over them in one process — every node under the enforced profile,
two meshes under two CAs, the only gateway lost and replaced, every link severed while both meshes kept
gossiping and committing, the grant changed mid-partition and visible on reconnect, authority issued
before the partition honoured to its expiry — with non-merger asserted from the membership tables, the
consensus namespace and each node's connection table (`connected_peers`) before, during and after
(`src/lib_tests.rs` → `the_release_gates_choreography_over_the_transport`). Two things to know when you
run this for real: a replaced gateway should be **retired** (`FederationClient::retire_gateway`) — the
pool keeps no health memory, so a dead gateway left listed costs every at-most-once call a
`DeliveryUnknown`; and the edge is plain HTTP in the test — in production it is whatever the gateway
serves, so run it behind `gateway_tls`. Still to build: the Docker two-mesh suite (process isolation and
a real network severance are its claim); a signed catalogue reply; SDK verbs. Streaming under a
credential is refused (federated calls are unary); `examples/federated_domains.rs` still runs the
lifecycle in one process and says so. The invocation edge **is A2A** (D5) with domain-bound origin
credentials — not a second call protocol, because two invocation edges with different auth models is
the drift v2.4.1 and v2.4.2 were spent removing.

**The release gate is not met.** The record asks for a two-mesh demonstration that proves *from
membership tables, consensus state and traces* that the meshes never merged. `two_meshes_never_learn_each_other`
is the harness for it and currently proves the easy half — two meshes that share no bootstrap peer
stay separate. It becomes the real test once there is an edge for them to leak across.
