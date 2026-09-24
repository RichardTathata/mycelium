# 17 — Federation: public discovery and federated domains

Federation is **two edges**, and conflating them is the mistake this chapter exists to prevent.

| | **Public discovery** (part 1) | **Federated domains** (part 2) |
|---|---|---|
| Question | *what does this domain say about itself?* | *who may invoke what, between partners?* |
| Who may read | **anyone** — un-gated by design | one named partner, per grant |
| Trust | the **fetcher's** decision, from a self-signed document | **bilateral**, from a trust bundle you populate deliberately |
| Shape | one well-known document | a credential bound to *this call*, and a filtered catalogue |
| Crate surface | `mycelium-agentfacts` | `mycelium::federation` |

The boundary between them is a decision, not an accident:

> **Public discovery is what a domain says about itself, verifiable by any fetcher. Federation is
> who may invoke what, between partners.**

So federation adds **no second well-known document**, no registry and no trust-registry service. A
`DomainDescriptor`'s public subset is serialised as an AgentFacts profile through the existing
serialiser — they are one model, not two.

Operators run part 2 from [operations/federation.md](../operations/federation.md).

---

## Part 1 · Public discovery — AgentFacts

*Un-gated by design: anyone may pull this, and trust is theirs to decide.*

### Concept

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

### Serve your facts (mount the edge)

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

### Pull and verify another domain

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

### The trust model — self-certified, no issuer authority

- **The signature is the node's identity.** The document embeds the Ed25519 `identity_pubkey`; a
  fetcher verifies against *that*. There is no CA, no registry, no issuer to trust — trust is the
  fetcher's decision. Two domains with separate auto-CAs discover each other without sharing a root.
- **`cluster` field** — carries the node's `cluster_name` ([chapter 13](13-cluster-topology.md)) so
  a fetcher can tell *which environment* a node belongs to. Omitted when unset.
- **Optional revocation head (WS-D / D3).** Surface `agent.revocation_head()` (the `compliance`
  feature) in `FactsOptions::revocation` so a fetcher can check "is this domain's key set current?"
  and verify inclusion proofs against the live `/gateway/transparency` endpoint — still no new trust
  authority. The substrate-shaped facts stay independent of the feature (pass it in, or don't).

### The multi-author domain board

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

### Run it

The two-domain demo runs the whole arc — advertise → pull → verify → route → tamper-fails — with
two clusters that never peer:

```bash
cargo run -p mycelium-coop-examples --bin federation_facts
```

## Part 2 · Federated domains — who may invoke what

*Bilateral, credentialed and policy-revisioned. Nothing here is public, and nothing is discovered.*

Part 1 was the **discover-me** edge. This is the **invoke** edge, and the one invariant to carry
into it is that **federation never joins the transports**: a partner's node never enters your
membership, your native `cap/` `grp/` `sys/` `consensus/` namespaces, your anti-entropy state or a
quorum. That is why there is deliberately no `federation/` key prefix, and a sweep in CI enforces its
absence. Design record: [`design/federated-domains.md`](../design/federated-domains.md).

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
| `federation::pinning` (`tls`) | the transport's **confidentiality** half: TLS anchored on a key pinned in the trust bundle, because this design has no CA to anchor it on |

Serve federated calls from a node:

```rust
let edge = Arc::new(FederationEdge::new(
    alpha,                                   // this domain
    ["invoice.submit", "invoice.status"],    // exports = A2A skill ids
    policy,                                  // who is granted what
    TrustBundle::trusting([(beta, beta_key)]),
    CallPolicy::default(),
).with_signing_key(alpha_signing_key));         // sign catalogues under alpha's key
let agent = GossipAgent::new(id, cfg).with_a2a().with_federation_edge(edge); // before start()
```

Call one from another domain:

```rust
let client = FederationClient::new(beta, "svc/billing", beta_signing_key, alpha,
    vec![GatewayEndpoint { id: "gw-1".into(), base_url: "https://alpha-gw-1:8443".into() }],
    /* slots per partner */ 4, /* catalogue freshness */ Duration::from_secs(60))
    .with_partner_key(alpha_public_key)   // require the catalogue to be alpha's, issued to us
    .with_tls_pins(bundle.tls_pins_for(&alpha).to_vec()); // and require the endpoint to be alpha's
let granted = client.connect().await?;                       // the catalogue is the grant
let reply = client.call("invoice.submit", text, Repeatability::AtMostOnce).await?;
```

### Bounding what a partner sends you

Budgets used to run one way. `FederationClient`'s pool meters slots per partner on the **consumer**
side — what you send — and the provider side had no counter at all, which the Phase-C audit named
and left as a decision. The decision is the mirror of what the other side already does:

```rust
CallPolicy { max_in_flight_per_partner: 8, ..CallPolicy::default() }   // 0 = unlimited
```

Over the cap is `CallRefusal::AtCapacity` and JSON-RPC **-32004** — refused, not queued, and kept
distinct from -32003 (`NotPermitted`) because *"too busy right now"* and *"you may not do this"* are
different answers that call for different responses. The slot is an **RAII guard**, so it returns on
a reply, on a later refusal, or on an unwind; a release a handler had to remember is one it would
miss.

Two honest limits, both consequences of having no coordinator: the cap is **per gateway** (N
gateways ⇒ N × cap in aggregate, because a shared counter would mean partner names gossiped through
`sys/`, which [D7](../design/federated-domains.md) forbids), and it bounds **concurrency, not rate**.

### Calling a partner from Python or TypeScript

The Rust client above is the whole trust story: it holds your domain's signing key, mints
credentials and checks the partner's catalogue and TLS pin. None of that belongs in an SDK
process, so the SDKs do not re-implement it — they drive **your own node**, which does.

Attach a client per partner and the gateway grows the consumer verbs:

```rust
let agent = GossipAgent::new(id, cfg)
    .with_federation_clients([Arc::new(client)])    // before start(), like the edge
```

| Route | Scope | What it does |
|---|---|---|
| `GET /gateway/federation/domain` | `federation:read` | this node's own domain, exports and policy revision |
| `GET /gateway/federation/partners` | `federation:read` | link state and last-granted catalogue, per partner |
| `GET /gateway/federation/catalog/{domain}` | `federation:read` | the last observation — **no network** |
| `POST /gateway/federation/connect` | `federation:invoke` | go and fetch the catalogue; bring the link up |
| `POST /gateway/federation/call` | `federation:invoke` | invoke an export |

```python
fed = agent.federation()
fed.connect("alpha.example")
reply = fed.call("alpha.example", "invoice.submit", text)        # repeatable=False by default
```

```ts
const fed = agent.federation();
await fed.connect("alpha.example");
const reply = await fed.call("alpha.example", "invoice.submit", text);
```

**The principal is the caller's, and the body cannot say otherwise.** The credential names the
principal your gateway's auth layer resolved for *that request* — item 7's rule carried one
boundary further out. A gateway that minted every credential under its own configured principal
would be the confused deputy again, one domain wider: the partner's evidence would record a
service account for work it never asked for. On a gateway with no token model the principal is
`anonymous`, which is honest and rarely what you want in a partner's records.

**A refusal answers two questions, not one.** `sent` says whether any byte crossed; `delivery` is
`none` · `refused` · `completed` · `unknown`. Both SDKs raise a *distinct type* for `unknown`
(`DeliveryUnknown` / `DeliveryUnknownError`) so it cannot be swept up with ordinary failures and
retried — it means the call may already have run. That is the receipt vocabulary of
[chapter 18](18-contracts-and-receipts.md), at a domain boundary.

### Encrypting the link: the pin is the anchor

Everything above is **signed and not encrypted**. The credential binds the origin domain, the
principal, the export, the validity window and (since 2.12.0) the request body — so an on-path
attacker cannot alter a call — but it can still *read* every call and every reply.

TLS closes that, and TLS needs a trust anchor. There is no X.509 anywhere in this design: partner
trust is one Ed25519 key per partner that an operator chose bilaterally. So the anchor is **the
bundle** — the same decision, extended to the transport — rather than the public Web PKI (whose root
is far larger than the bilateral one) or an exchange of private CAs (more machinery, and a CA signs
*any* name).

```rust
// The partner publishes a pin: sha256 of its endpoint's SubjectPublicKeyInfo.
//   openssl x509 -in gw.pem -pubkey -noout | openssl pkey -pubin -outform der | sha256sum
// A partner reusing its node identity key (GatewayTlsConfig::default(), where the certificate is
// regenerated at every startup) computes it from the key instead:
let pin = federation::pinning::ed25519_spki_sha256(&partner_identity_key);

bundle.pin_tls(&alpha, pin);                                  // beside the signing key
let client = FederationClient::new(..).with_tls_pins(bundle.tls_pins_for(&alpha).to_vec());
```

The field is a **list**, for the reason key rotation is a list: one pin makes a TLS key change a flag
day. The partner publishes the next pin, every counterparty adds it (`pin_tls`), the partner swaps,
the old one is dropped (`unpin_tls`).

Two refusals come with it, and both mean *nothing was sent*:

| Refusal | What happened |
|---|---|
| `Tls(PinMismatch { gateway, presented })` | The endpoint presented a key this domain does not pin. Either the partner rotated without publishing, or that endpoint is not the partner. `presented` is the digest that arrived — compare it against what the partner says, never paste it in. |
| `Tls(PlaintextEndpoint { gateway, base_url })` | Pins are configured and the URL is `http://`. Refused rather than downgraded: silently dialling plaintext would give you the ceremony of pinning and none of the confidentiality. |

Neither is a `DeliveryUnknown`, and that distinction is the point of a separate variant: a
`DeliveryUnknown` says *it may have run*, which bars an at-most-once caller from ever retrying. A
failed handshake sent nothing, so the call is safe to retry once the pin or the endpoint is fixed.

**What pinning does not do.** It does not authenticate the *caller* — that stays the credential,
deliberately, because a second authentication model at the same edge is the drift v2.4.1 and v2.4.2
were spent removing. It does not check the certificate's name, chain or expiry, because there is no
authority here to check them against, and a name checked against nothing reads like validation while
proving nothing. And it says nothing about an attacker holding the partner's private key.

A call before `connect` is refused with no HTTP (`Link(Down)`); an export the catalogue never named is
refused with no HTTP (`Resolve`); a credential the partner does not trust, or one minted for another
export, is refused *at the partner's gateway* with no dispatch. The provider is handed
`federation:beta.example/svc/billing` — never "the gateway".

Run it:

```bash
cargo run --example federated_domains --features tls
```

The example walks the whole lifecycle and prints what each step decided *and why*.

### Three domains — the question two cannot ask

Two domains cannot ask the question that decides how a federation grows:

> Alpha trusts Beta. Beta trusts Gamma. **Does Alpha trust Gamma?**

```bash
cargo run --example federation_trust_is_not_transitive --features tls
```

The answer is **no**, three times over, and each is an assertion rather than a paragraph: a trust
bundle does not compose (gamma's perfectly good credential is `UnknownDomain` at alpha); a grant you
hold is not a grant you can re-export (beta cannot mint a credential naming gamma without gamma's
key); and the per-partner budget is pairwise, so gamma saturating its slots cannot refuse beta's
calls — a refusal there says `AtCapacity`, which is **transient load**, never `NotPermitted`, which
is a standing answer about authority that a retry will not change.

**Why this is the right default, not a limitation.** Transitive trust is how one compromised partner
becomes everybody's compromise: if alpha inherited beta's trust list, beta adding a partner would
silently grant that partner access to alpha — an authority decision alpha's operator never made and
cannot see. The cost is real and worth stating: federations grow by **pairs**, not by transitivity.
That is the price of an authority surface an operator can enumerate.

### The release gate, and what it does not claim

**The release gate, and what of it is met.** PR 8 (2026-09-18) put the first bytes across; PR 9 (same day)
ran the gate's whole choreography over them in one process — every node under the enforced profile,
two meshes under two CAs, the only gateway lost and replaced, every link severed while both meshes kept
gossiping and committing, the grant changed mid-partition and visible on reconnect, authority issued
before the partition honoured to its expiry — with non-merger asserted from the membership tables, the
consensus namespace and each node's connection table (`connected_peers`) before, during and after
(`src/lib_tests.rs` → `the_release_gates_choreography_over_the_transport`). Two things to know when you
run this for real: a replaced gateway should be **retired** (`FederationClient::retire_gateway`) — the
pool keeps no health memory, so a dead gateway left listed costs every at-most-once call a
`DeliveryUnknown`; and the edge is plain HTTP in *that* test — in production it is whatever the
gateway serves, so run it behind `gateway_tls` and pin it (above;
`a_pinned_federation_link_talks_only_to_the_key_the_bundle_names` is the same edge over real TLS,
with two gateways differing only in their TLS key). Every attempt is bounded (5 s to connect, 30 s in total;
`with_timeouts` to change them), because a partner whose network is blackholed never refuses and an
unbounded client could not report the `DeliveryUnknown` the contract promises. The catalogue is signed under the domain's key and bound to the
partner it was issued to; a client given the partner's key (`with_partner_key`) refuses an unsigned,
forged or misaddressed one and leaves the link down. PR 10b closed the gate's last caveat: the same
choreography runs in Docker with one container per node and the link cut for real
(`make test-federation`). **Row 11 is closed:** the SDK verbs (above), a hostile network between
domains ("Encrypting the link"), and more than two domains —
`three_domains_compose_without_trust_composing` runs a chain (alpha grants to beta, beta grants to
gamma, alpha and gamma strangers) and pins what two domains could not state: **trust does not
compose** (gamma is refused at alpha's auth layer before any catalogue is computed), a grant you
hold is **not re-exported** by holding it, a catalogue is filtered *per asker* with three askers to
tell apart, non-merger holds **pairwise** over all three meshes, and a hub's slots are per partner
so a busy neighbour is not a denial of service on everyone else. Streaming under a
credential is refused (federated calls are unary); `examples/federated_domains.rs` still runs the
lifecycle in one process and says so. The invocation edge **is A2A** (D5) with domain-bound origin
credentials — not a second call protocol, because two invocation edges with different auth models is
the drift v2.4.1 and v2.4.2 were spent removing.

---

## Where next

**Part 1 — public discovery**

- [00 · Concepts](00-concepts.md#why-a2a--mcp--agentfacts-are-not-the-same-thing) — the A2A vs MCP
  vs AgentFacts distinction (native mesh call vs external tool call vs cross-domain discovery).
- [08 · A2A interop](08-a2a-interop.md) — the *call-me* side (LangChain / AutoGen on the mesh).
- [09 · Security](09-security.md) — the Ed25519 node identity that does the self-certifying.
- Operators: [observability → Viewing AgentFacts](../operations/observability.md#viewing-agentfacts).

**Part 2 — federated domains**

- [operations/federation.md](../operations/federation.md) — the runbook: standing a partner up,
  overlapping key rotation, revoking **both** halves, and reading a refusal.
- [`design/federated-domains.md`](../design/federated-domains.md) — what a domain is, the three
  trust relationships kept apart, and the four things federation refuses to become.
- [18 · Contracts & receipts](18-contracts-and-receipts.md) — why a silent gateway is
  `DeliveryUnknown` rather than a failure, and what an at-most-once call may not do about it.
- [20 · Authorising actions](20-authorising-actions.md) — federation preserves the origin principal;
  deciding what that principal may *do* is a separate question.
