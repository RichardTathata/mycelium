# Federated domains

## What it is

A [domain](../guide/00-concepts.md#domain) is one fleet under one authority. Federation lets two domains call each other
without merging: a partner's nodes never enter your membership, your `cap/`, `grp/`, `sys/` or `consensus/` namespaces,
your anti-entropy or a quorum, and there is no `federation/` key prefix — a CI sweep enforces its absence. Each side
holds a bilateral trust bundle naming the other's key. The provider's federation edge publishes a signed catalogue
filtered per partner — what that partner may see is what it is granted — and authorises each call on `/a2a` against
its policy, with a credential bound to the caller and the export — and to the body as received only where the
provider's `CallPolicy::require_body_binding` is set, which it is not by default. The consumer's
`FederationClient` fetches the catalogue, meters its slots per partner, and turns a silent gateway into
`DeliveryUnknown`, or a failover where the export is repeatable.

## When to use it, and when not

- **Use it when** two organisations' fleets should invoke named skills of each other's, each keeping its own CA,
  membership and policy, with every call attributed to the calling domain and principal.
- **Not when** you want the fleets to become one: that is one domain under one CA; and not for open discovery between
  strangers, which is [AgentFacts](agentfacts.md) — trust here does not compose, so a grant you hold is not re-exported.

## Turn it on

This feature is turned on in code: the provider side needs `tls` and `a2a`, the consumer side `gateway` and `tls`. The
published node mounts neither `/a2a` nor an edge.

- [`domain_profile`](../reference/configuration.md#domain_profile) `= DomainProfile::Enforced` (or
  `GOSSIP_DOMAIN_PROFILE=enforced`) on every node of a federating domain: `validate()` then requires
  [`tls`](../reference/configuration.md#tls) and refuses
  [`swim_failure_detector`](../reference/configuration.md#swim_failure_detector), which is on by default — set it
  `false`, or the node does not start.
- [`FederationEdge::new`](../../src/federation/edge.rs) with this domain, its exports (A2A skill ids), a `DomainPolicy`,
  a `TrustBundle` and a `CallPolicy`; `.with_signing_key` to sign the catalogue; then `with_a2a().with_federation_edge(edge)`
  before `start()`. `CallPolicy::max_in_flight_per_partner` caps a partner's concurrent calls; set
  `require_body_binding` (default `false`) to refuse a credential that does not bind the body.
- [`FederationClient::new`](../../src/federation/client.rs) on the consumer, with `.with_partner_key` to refuse an
  unsigned or misaddressed catalogue and `.with_tls_pins` to refuse an endpoint that is not the partner's.
- `with_federation_clients` exposes the client to SDKs under `federation:read` and `federation:invoke` — scopes checked
  only in a build with `compliance`; without it any valid bearer reaches those routes. The credential names the local
  caller, never the node.
- To have `start()` refuse a node whose side of the edge is loose, run it under the
  [multi-domain profile](multi-domain-profile.md).
- `three_domains_compose_without_trust_composing`, in [`src/lib_tests.rs`](../../src/lib_tests.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`federated_domains`](../../examples/README.md#the-capability-matrix): the lifecycle in one process — descriptors,
trust, the filtered catalogue, a call, a revocation. CI runs it, and
[`federation_trust_is_not_transitive`](../../examples/README.md#the-capability-matrix); both print each decision and
assert none, so the checks are the tests, `three_domains_compose_without_trust_composing` in
[`src/lib_tests.rs`](../../src/lib_tests.rs) among them. The two-mesh Docker suite,
`make test-federation`, cuts the link at the network layer and asserts the meshes never merge.

## What it guarantees, and what it does not

- The `fed.*` guarantees are the [multi-domain profile](multi-domain-profile.md)'s — for example
  [`fed.non_merger`](../reference/guarantee-catalogue.md#fed.non_merger) and
  [`fed.catalogue_signed`](../reference/guarantee-catalogue.md#fed.catalogue_signed); under the default `dev` profile a
  node reports them and refuses nothing.
- **Not built:** a per-partner cap across gateways — the cap is per gateway, so N gateways allow N × cap, and it bounds
  concurrency, not rate; streaming under a federated credential (calls are unary); `CallPolicy::require_body_binding`
  defaults to `false`, which gives no integrity guarantee against an active attacker; a pool with health memory — retire
  a replaced gateway with `retire_gateway`.
- **Evidence:** the ledger rows
  [two meshes do not merge with the link cut](../operations/what-is-proven.md#two-mesh-federation-does-merge-link) and
  [trust does not compose across three domains](../operations/what-is-proven.md#trust-does-compose-across-three-federated).

## Go deeper

- **Guide:** [17 § Part 2 · Federated domains](../guide/17-federation.md#part-2--federated-domains--who-may-invoke-what).
- **Runbook:** [federation](../operations/federation.md).
- **Design:** [`design/federated-domains.md`](../design/federated-domains.md).
- **Code:** [`src/federation.rs`](../../src/federation.rs) and [`src/federation/`](../../src/federation/edge.rs)
  (`catalog.rs`, `call.rs`, `gateway.rs`, `session.rs`, `edge.rs`, `client.rs`, `pinning.rs`).
