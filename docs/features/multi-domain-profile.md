# The multi-domain profile

## What it is

`secure-multi-domain` is a [guarantee profile](../guide/00-concepts.md#guarantee-report) for a node of one of
several federating [domains](../guide/00-concepts.md#domain): `start()` refuses unless every guarantee
`secure-single-domain` requires holds, and the federation family too. The family is ten guarantees. Seven are facts this
node can check, and three only the partner could show, so the report lists those three and never counts them. A node
that faces no partner, with no federation edge and no client attached, reads the side-specific ones `not_applicable`,
as a node with no gateway reads the gateway's, and the start log and `doctor` say in one line that the node faces no
partner. No side is required: a member of a federating domain need not face a partner itself.

## When to use it, and when not

- **Use it when** a node serves federated calls (an edge), calls a partner (clients), or is a member of a domain
  that federates, and you want it refused at start unless its side of the edge is pinned, body-bound, metered,
  attributed, signed and confined to the partner's hosts.
- **Not when** you need to know the partner is honest: nothing here checks the partner's posture, non-merger or clock
  agreement, which are `not_verifiable_here` ([federation runbook](../operations/federation.md#starting-under-secure-multi-domain)).
  Also not for `mycelium doctor nodes/<node>.toml` alone: the edge and the clients are code attachments, which a node
  built from a file never has — `mycelium::doctor::run_attached` is the same doctor over the node as a program builds it.

## Turn it on

This feature is turned on in code.

- [`profile`](../reference/configuration.md#the-fields): `"secure-multi-domain"`, or `GOSSIP_PROFILE`.
- [`domain_profile`](../reference/configuration.md#the-fields): `"enforced"` on every member, with SWIM off and `[tls]`.
- [`with_federation_edge`](../../src/agent/mod.rs): the edge, built with `FederationEdge::new` and a
  `CallPolicy` that sets `require_body_binding` and `max_in_flight_per_partner`.
- [`with_signing_key`](../../src/federation/edge.rs): the edge signs the catalogue it serves.
- [`with_federation_clients`](../../src/agent/mod.rs): one client per partner, each with its pins and the partner's key.
- [`with_tls_pins`](../../src/federation/client.rs): the partner's TLS key pinned from the trust bundle.
- [`with_partner_key`](../../src/federation/client.rs): a catalogue is relied on only once it verifies under it.
- [`egress`](../reference/configuration.md#the-fields): `allow_hosts` names every partner endpoint's host.
- [`SECURE_MULTI_DOMAIN`](../../src/agent/guarantee.rs): the required set, rev 1. Attach the edge and the clients
  **before** `start()`: under this profile — and only this one — one attached after it is ignored with a warning, and
  counted.
- `a_node_providing_every_requirement_starts_under_the_multi_domain_profile`, in
  [`src/lib_tests.rs`](../../src/lib_tests.rs): attaches an edge and a client set as above to a node meeting every
  other requirement, and the node starts under the profile with every `fed.*` node fact enforced.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature, and the suite's harness binary is not a row either. A scaffold does:
`mycelium init <dir> --template two-domains` writes two fleets of two nodes, each its own domain under this profile,
calling each other across one edge ([`templates/two-domains`](../../templates/two-domains/README.md)); CI's
`Scaffold` job runs it — the calls, each gateway's report, and the refusals under a wrong pin, a rewritten body and
a full slot count. `make test-federation` runs the
mesh nodes of two domains under this profile in Docker ([`examples/federation_node.rs`](../../examples/federation_node.rs),
[`docker/docker-compose.federation.yml`](../../docker/docker-compose.federation.yml),
[`tests/integration/run_federation.sh`](../../tests/integration/run_federation.sh): legs 0, 2 and 10 read the
profile). The per-guarantee runs are tests in [`src/agent/guarantee.rs`](../../src/agent/guarantee.rs),
one per required federation guarantee, each dropping one setting from a node that has every other and showing `start()`
refuses naming that guarantee and no other in the family: `multi_domain_refuses_naming_fed_domain_enforced`,
`multi_domain_refuses_naming_fed_pinned`, `multi_domain_refuses_naming_fed_body_bound`,
`multi_domain_refuses_naming_fed_partner_slots`, `multi_domain_refuses_naming_fed_caller_named`,
`multi_domain_refuses_naming_fed_catalogue_signed` and `multi_domain_refuses_naming_fed_partner_egress`. A late attachment left inert is
`a_federation_attachment_after_start_is_inert_under_the_multi_domain_profile`, in
[`src/lib_tests.rs`](../../src/lib_tests.rs). `doctor`'s
rendering of the family is `doctor_prints_the_federation_family_under_the_multi_domain_profile`, in
[`src/doctor.rs`](../../src/doctor.rs).

## What it guarantees, and what it does not

- **Required, on this node:** [`fed.domain_enforced`, `fed.pinned`, `fed.body_bound`, `fed.partner_slots`,
  `fed.caller_named`, `fed.catalogue_signed` and `fed.partner_egress`](../reference/guarantee-catalogue.md#the-descriptors),
  with the twenty of `secure-single-domain` rev 5.
- **Listed, never counted:** [`fed.partner_posture`, `fed.non_merger` and `fed.clock_agreement`](../reference/guarantee-catalogue.md#the-descriptors).
  Nothing this node checks makes the partner honest.
- `fed.caller_named` is `gw.not_open`'s check restated for the federation routes; `fed.partner_egress` holds for a
  `.suffix` entry, which admits every host under it, so name the partner's host exactly to confine calls to it.
- `fed.partner_slots` bounds what one gateway carries: a domain running N gateways admits N times the cap
  ([federation § Bounding what a partner can send you](../operations/federation.md#bounding-what-a-partner-can-send-you)).
- **Its mesh nodes run under it, across real processes:** the two-mesh Docker suite (`make test-federation`, CI job
  `federation`) starts every mesh node of both domains under this profile — certificates issued off-node, one volume
  per node, HTTPS gateways the consumer pins, the authority, audit, persistence and electorate settings — reads each
  node's `GET /gateway/guarantees` for the profile met, shows the evaluator and the body binding refusing, and starts
  one node with its CA key on-node, which is refused naming `id.ca_key_off_node`
  ([federation § Starting under `secure-multi-domain`](../operations/federation.md#starting-under-secure-multi-domain)).
  **Not its consumer:** that is a bare `FederationClient`, not a node, so the client side's `fed.pinned`,
  `fed.caller_named` and `fed.partner_egress` as a node's facts are not shown there.
- **Both sides run under it, in the two-domain scaffold** (Y8): each gateway runs an edge and holds a client for the
  other, and its own `GET /gateway/guarantees` reads all seven `fed.*` node facts `enforced` while it runs —
  `fed.pinned`, `fed.caller_named` and `fed.partner_egress` among them (CI job `Scaffold`). Four processes on one host,
  not containers.
- **A replayed credential is refused at every edge**, under any profile — the same signed bytes served once a gateway,
  in memory, until they expire; per gateway, and forgotten at a restart
  ([federation § A credential is one call](../operations/federation.md#a-credential-is-one-call-replays)). No guarantee
  id: it is not a setting, so there is nothing for the report to resolve.
- `mycelium doctor <file>` cannot report the edge's or the clients' facts; `doctor::run_attached` can, over the
  program's own attachments, and the scaffold's `doctor` is that. The upgrade
  notes are [deprecations § 66](../guide/deprecations.md#66-a-third-guarantee-profile-secure-multi-domain-ten-fed-guarantees-in-every-report-guaranteereportunresolved-lists-only-what-applies-unreleased-adopter-surface-y7).

## Go deeper

- **Runbook:** [federation § Starting under `secure-multi-domain`](../operations/federation.md#starting-under-secure-multi-domain)
  and the [readiness checklist](../operations/production-readiness.md).
- **Guide:** [17 · Federation](../guide/17-federation.md).
- **Design:** [`federated-domains.md`](../design/federated-domains.md); the plan row is Y7 of
  [`adopter-surface.md`](../plans/adopter-surface.md), decision D25 (rev 0.13 adds `fed.domain_enforced` as the seventh required guarantee, agreed with the maintainer 2026-10-26).
