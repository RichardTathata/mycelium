# Federated domains — what a domain is, and what federation is allowed to be (ADR, item 2 PR 1)

**Status:** adopted 2026-09-17 · **item 2 PR 1** of `docs/plans/v3-contracts-axis.md` §5 (decisions D5, D6, D7,
D25). A contract, not an API. It cites [`threat-model.md`](../threat-model.md) (item 8) as the boundary model
every claim here is stated against, and sits beside [`contracts-receipts.md`](contracts-receipts.md) (item 1,
for `DeliveryUnknown` and the acknowledgement ladder) and [`action-envelope-ae0.md`](action-envelope-ae0.md)
(the AE slice, for who-may-do-what at a gateway).

> **Posture, once.** Every guarantee below is stated at the strength it has. Federation is an **edge between
> meshes**, never a widening of one. The library ships mechanism; the isolation an operator gets is the
> deployment's claim, not the library's (§10). Nothing here changes the wire — v1 federates over HTTPS at
> gateways, which is a correction to our own earlier record (§11).

## 1. What a domain is

A **domain** is one independently admitted gossip mesh. It owns, separately from every other domain:

- its **membership** (who is in the mesh),
- its **replication and dissemination** (what gossips to whom),
- its **policy** (what is allowed inside it),
- its **electorate** (who votes in consensus).

Admission is the existing per-node CA trust root — `mycelium-core/src/tls.rs` builds a `RootCertStore` per node
(l. 410) — not a name. **`cluster_name` remains a cosmetic label** with no isolation meaning, exactly as
`CLAUDE.md` already states; a domain is emergent from *admission*, and `DomainId` is what names it.

## 2. What federation is not

Federation connects **explicitly exported services** between meshes. It never joins the transports. Concretely,
a foreign node never enters:

| | Why it matters |
|---|---|
| membership | a foreign node in `peers` is a node the failure detector, fan-out and quorum sizing all count |
| the native `cap/` · `grp/` · `sys/` · `consensus/` namespaces | these are the medium; foreign state in them is indistinguishable from local state afterwards |
| anti-entropy state | the Merkle digest is a claim about *this* mesh's contents |
| a quorum | consensus is decided by an electorate, and the electorate is the domain |

**This is the whole invariant.** Everything else in this record is a consequence of it.

## 3. Three trust relationships, kept apart

Collapsing any two of these is how federation becomes a second membership system.

1. **Membership trust** — may this node be *in* my mesh? Per-node CA root, as today.
2. **Federation identity** — is this partner who it says it is? The signed `DomainDescriptor`, verified
   bilaterally.
3. **Service authorization** — may this partner invoke *this* export? The revisioned `DomainPolicy`, evaluated
   at the gateway.

A partner that authenticates (2) is not thereby authorized (3), and is never admitted (1).

## 4. Identity and policy objects

- **`DomainId`** — the stable name of a domain.
- **`DomainDescriptor`** — signed; carries the domain's identity and its **exported** surface. Its *public*
  subset is an AgentFacts profile (§7), not a second well-known document.
- **`DomainPolicy`** — **revisioned**, so "which rules were in force" is answerable after the fact rather than
  inferred. A policy revision is the unit an evidence record names.

Foreign observations — what a partner's catalogue said, and when — are **attributable per-gateway
observations**, held in the companion's cache. They are not facts about the fleet; they are things a particular
gateway saw at a particular time, and the type says so.

## 5. D5 — the invocation edge is A2A, not a second protocol

**Decided: the federation call *is* A2A, with domain-bound origin credentials. There is no
`POST /federation/v1/call`.**

The plan (§5) left this open: *"Either the federation call is A2A with domain-bound origin credentials, or the
ADR states why `POST /federation/v1/call` must exist."* This record takes the first branch, for three reasons.

1. **The edge already exists and is already enforced.** `src/agent/a2a.rs` serves
   `GET /.well-known/agent.json` and `/a2a`, and both dispatch paths run the AE preflight under
   `gateway:a2a` (v2.6.0). A second edge starts at zero on authentication, authorization, evidence and rate
   limiting.
2. **Two invocation edges with different auth models is the drift we just cleaned up.** v2.4.1 and v2.4.2 were
   both spent putting merged routers and unguarded endpoints behind gateway auth. Adding a second call surface
   would re-create the shape those releases removed, in the one place where the caller is by construction not
   ours.
3. **A capability invocation maps onto an A2A skill without loss.** The resolver turns a `RemoteCapability` into
   a skill on the partner's gateway; `origin` is preserved to the provider through the federation-aware adapter
   rather than being flattened into "the gateway called me".

**What would reopen it.** If a federation call needs a property A2A cannot express — streaming semantics
incompatible with `tasks/sendSubscribe`, or a request shape that cannot carry the domain-bound credential
without abusing a field — that is an argument for a second protocol, and it belongs here as a revision rather
than in code as a fait accompli.

## 6. D6 — reuse the cryptography, never the trust

`src/agent/oidc.rs` already owns the verification machinery: `jsonwebtoken`, a JWKS cache, and an explicit
algorithm allowlist (`ALLOWED_ALGS`, l. 32 — RS/ES/PS families, no `none`, no mixed-family list). Federation
**shares that code** and **shares none of its trust policy**.

Federation objects carry their own **issuer, audience, object type and authority rules**, so a token valid for
one purpose is never valid for another by passing through the same verifier. This is the rule the register
states as *"reuse the code, never the trust"* (D6), and it is the reason the two must not share a `Validation`
construction path.

Source-side egress is the **existing** `EgressPolicy::allow_hosts` extended, not a new rule — one place where an
operator says where this node may talk to.

## 7. D25 — the NANDA boundary

`mycelium-agentfacts` is already Mycelium's NANDA edge: self-certified, publicly fetchable, pulled rather than
pushed, deliberately un-gated, with NANDA's field names isolated in one serializer. `FACTS_PREFIX` is `facts/`
(`mycelium-agentfacts/src/crdt.rs`, l. 24).

Federation must not become a second discovery edge:

- the `DomainDescriptor`'s **public subset is an AgentFacts profile** through the existing serializer —
  **no second well-known document**;
- **trust bundles are bilateral operator configuration** — no registry, no trust-registry service;
- item 3's assessments reach AgentFacts `certification` only through **explicit export projections**.

The line, stated once: **NANDA is what a domain says about itself, verifiable by any fetcher. Federation is who
may invoke what, between partners.** If NANDA later standardises attestations or cross-registry federation, the
projection adapts — edges are adopted, not competed with.

## 8. D7 — no `federation/` KV prefix, as a checked invariant

Foreign observations live in the companion's cache and **never** in the gossip KV namespace. There is no
`federation/` prefix, and there must not be one.

Verified at adoption: no `"federation/"` literal exists in `src/` or `mycelium-core/src/`. The registry of
legitimate prefixes is `kv_ns` (`mycelium-core/src/signal.rs`, l. 860).

**The check now exists.** `scripts/check-kv-namespaces.sh` fails the build if a forbidden prefix literal
appears in production code, naming the file, the line and the record that forbids it; it runs in `make check`
and in CI. The plan said *"the wiki lint's namespace sweep checks it"* — **it did not**, as of this ADR being
written, and an invariant nothing runs is a sentence. Test code is exempt for the same reason it is in the
forbidden-call check: a test naming the prefix to prove it absent is doing its job. Both directions were
verified by planting a violation in production code (caught, with the citation) and in a test module (correctly
ignored). **This is the invariant most likely to be violated by accident**, because
putting a foreign observation in KV is the shortest path to making it visible everywhere — which is precisely
the thing §2 forbids.

## 9. The enforced v1 profile

**SWIM is off in the enforced profile.** The reason is specific and verified rather than precautionary: SWIM's
control datagrams are **unauthenticated UDP**. `mycelium-core/src/swim.rs` signs nothing — the only occurrence
of the string `sign` in that module is the word *signal*, in a doc comment. A membership protocol that any
host on the path can forge is not one to expose at a federation boundary.

The profile is therefore: TLS between admitted nodes, SWIM disabled, federation over HTTPS at gateways, and
separate CA roots per domain.

## 10. Gateways, failure, and partition

- **At least two replaceable gateways, with no federation leader.** A leader would be a new coordination
  dependency, which is the thing the substrate exists not to need.
- **Failover only for repeatable exports.** An export that is not safe to retry is not failed over; the caller
  gets `DeliveryUnknown` (item 1's vocabulary — a timeout is never a negative).
- **Fixed per-gateway quota slots**, so one partner cannot consume another's budget through a shared gateway.
- **On disconnect:** discovery **expires**, calls fail **explicitly**, and authority already issued lasts only
  to its stated expiry. Nothing is silently extended.
- **On reconnect:** discovery refreshes **before** new work is admitted, and **the meshes never merge** — the
  §2 invariant holds across the whole partition/reconnect cycle, not just at steady state.

**Process isolation between domains is the example deployment's claim, not the library's.** The library gives a
domain its own admission, namespaces and electorate; it does not give it its own address space. Every document
about federation holds this line.

## 11. Corrections to our own record

**"Domains are the one item likely to touch the wire" — withdrawn.** v1 federates over HTTPS at gateways, with
separate CA roots and SWIM off. The gossip wire is untouched; `WIRE_VERSION` stays 12. The only protocol change
anywhere on this axis remains item 2's *deferred* authenticated, domain-bound SWIM/handshake, which is not in
v1 and is not decided here.

**"There is a `federation_facts.rs`" — withdrawn** (already corrected in the plan on 2026-09-13). The starting
point is the `mycelium-agentfacts` crate and [guide 17](../guide/17-federation.md).

## 12. What this record refuses

- **A federation leader**, an election among gateways, or any single point that must be up for two healthy
  domains to keep working.
- **A trust registry or TRS.** Trust bundles are bilateral operator configuration. "Trust is the fetcher's
  decision" holds on both sides of the boundary.
- **A second public descriptor.** One well-known document, one serializer, one place NANDA's field names live.
- **A `federation/` KV prefix**, now or later (§8).
- **Merging on reconnect.** A partition that heals is two domains that can talk again, not one domain.
- **Any claim about process isolation on the library's behalf** (§10).

## 13. What lands next

Per §5's sequence, and gated in that order:

| PR | Contents |
|---|---|
| **1** *(this record)* | the domain ADR, the enforced profile, the two-mesh harness |
| 2 | identity, trust bundles, typed policy, test vectors |
| 3 | filtered catalogs + the remote resolver |
| 4 | authenticated unary calls + the provider adapter |
| 5 | two-gateway operation, budgets, outcomes |
| 6 | partition / reconnect, revocation, rotation |
| 7 | example, SDKs, diagnostics, docs |
| **8** *(2026-09-18)* | the transport's first arm: `federation/edge.rs` (the credential as one header on `/a2a`; authenticate at the auth layer, authorise in the handler; `GET /federation/catalog`), `federation/client.rs` (link → resolver → pool → HTTP), and the two-mesh test that re-runs PR 1's never-merged assertions *after* a call has crossed |
| 9 | the gate's choreography over that transport: sever every link and keep working locally, change permissions mid-partition, reconnect; the *traces* leg of the proof; a signed catalogue reply |

**Release gate** (§5): the two-mesh demonstration — discover, invoke, lose a gateway, sever every link, keep
working locally, change permissions mid-partition, reconnect — and prove **from membership tables, consensus
state and traces** that the meshes never merged. Not from a narrative: from the three places that would show it
if they had.

**Where the gate stands after PR 8.** Two of the three places are checked with bytes crossing: the membership
tables and the native namespaces (`lib_tests.rs` → `a_federated_call_crosses_and_the_meshes_still_never_merge`,
which also plants a forged, a tampered, an ungranted and a revoked credential and counts that none reached the
provider). *Discover* and *invoke* are done; *lose a gateway* is done on the consumer side (a silent gateway is
`DeliveryUnknown` or a failover by repeatability, over real connection refusals). Not done: the traces leg, and
the partition choreography. The gate is not claimed.

## Appendix — anchors verified at adoption (2026-09-17)

| Claim | Where |
|---|---|
| SWIM control datagrams are unauthenticated | `mycelium-core/src/swim.rs` — signs nothing |
| intra-domain admission is a per-node CA root | `mycelium-core/src/tls.rs:410` (`RootCertStore`) |
| the A2A edge exists and is enforced | `src/agent/a2a.rs` — `/.well-known/agent.json`, `/a2a` |
| OIDC owns the verifier and its allowlist | `src/agent/oidc.rs:24,32` (`jsonwebtoken`, `ALLOWED_ALGS`) |
| the egress rule already exists | `EgressPolicy::allow_hosts` (`mycelium-core/src/config.rs:202`; used in `mcp.rs`, `llm.rs`) |
| AgentFacts is the public descriptor | `mycelium-agentfacts/src/crdt.rs:24` (`FACTS_PREFIX = "facts/"`) |
| no `federation/` KV prefix exists today | no such literal in `src/` or `mycelium-core/src/` |
| the KV namespace registry | `mycelium-core/src/signal.rs:860` (`kv_ns`) |
