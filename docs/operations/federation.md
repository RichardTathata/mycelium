# Federation

↑ [operations](README.md) · design record [`federated-domains.md`](../design/federated-domains.md) ·
developer guide [17 · Federation](../guide/17-federation.md)

Federation connects **explicitly exported services** between two independently admitted meshes. It
shipped with its transport in v2.8.0.

## The one invariant

**Federation never joins the transports.** A foreign node never enters your membership, your native
`cap/` `grp/` `sys/` `consensus/` namespaces, your anti-entropy state, or a quorum. There is
deliberately **no `federation/` key prefix**, and a sweep in CI enforces its absence.

If you remember one operational consequence: a partner domain going wrong cannot make your cluster
larger, slower to converge, or differently governed. It can only make calls you granted stop working.

A **domain** is named by its `DomainId`, and admission is the per-node certificate trust root — **not
`cluster_name`**, which remains a cosmetic label with no isolation meaning.

## What the edge exports, and what it never does

| Exports | Never |
|---|---|
| services you named in the policy's grants | anything not granted, and never by wildcard |
| a **filtered** catalogue, per partner | the full export list |
| the calling principal, preserved across the boundary | "the gateway called me" |

`federation.catalog` is a **reserved export name**. A domain that exports a real service under it has
made a naming mistake and the edge refuses to serve it. A call credential cannot fetch a catalogue,
and a catalogue credential cannot make a call.

## Standing a partner up

**Step 0 — the enforced domain profile, on every node.** A federated domain runs
`GossipConfig.domain_profile = DomainProfile::Enforced` (env `GOSSIP_DOMAIN_PROFILE=enforced`;
`mycelium-core/src/config.rs`). The default, `Open`, changes nothing and is what you get if you
skip this: under `Enforced`, `validate()` refuses a node without TLS, and the threat model's
Boundary D assumes every node of the domain is under it. Nothing in the three pieces below
turns it on for you.

Three pieces, in this order.

```rust
// 1. Who you trust. Populate deliberately — there is no registry and no discovery of trust.
let mut bundle = TrustBundle::trusting([(partner_domain.clone(), partner_public_key)]);

// 2. What they may call. Absence is denial: no inheritance, no wildcard, no default-allow.
let policy = DomainPolicy {
    domain: our_domain.clone(),
    revision: 7,
    grants: vec![(partner_domain.clone(), "surplus-food/collection".into())],
};

// 3. The edge itself. `with_federation_edge` CONSUMES the agent and returns it, and must be
//    called BEFORE start() — the catalogue route is merged into the gateway's routes at start.
let edge = FederationEdge::new(our_domain, exports, policy, bundle, CallPolicy::default())
    .with_signing_key(catalogue_signing_key);   // so partners can verify the catalogue
let agent = agent.with_federation_edge(Arc::new(edge));
agent.start().await?;
```

A second `with_federation_edge` call **keeps the first edge and warns**. If you are attaching
conditionally, build the edge you want once rather than layering calls.

**Bump `revision` whenever `grants` changes.** Partners read it, and a catalogue reply carries it;
leaving it stale makes a real change invisible to the far side.

Check `signs_catalogue()` at start-up. An unsigned catalogue reply is refused by a conforming
partner, so a missing signing key looks like a partner-side failure from your logs.

## Letting local clients call a partner (the consumer side)

The edge above is the direction *in*. Calling *out* needs a `FederationClient` per partner, and
attaching it to the node is what turns on `/gateway/federation/*` — and with it the Python and
TypeScript verbs, which have no other way to reach a partner.

```rust
let client = FederationClient::new(
    our_domain.clone(), "svc/discovery", our_signing_key, partner_domain.clone(),
    vec![GatewayEndpoint { id: "gw-1".into(), base_url: "https://gw1.partner.example".into() }],
    /* slots per partner */ 2, Duration::from_secs(60),
)
.with_partner_key(partner_public_key)                       // else the catalogue is only an observation
.with_tls_pins(bundle.tls_pins_for(&partner_domain).to_vec());

let agent = agent.with_federation_clients([Arc::new(client)]);   // before start(), like the edge
```

`with_federation_clients` applies the node's `egress.allow_hosts` to each client: a partner `base_url`
outside the list fails before any byte with `ClientError::Egress` (the enum is `#[non_exhaustive]` —
match with `_` and fail closed). A client built elsewhere opts in with `with_egress` (since 2.20.0).

Then, from any local client holding a bearer:

```bash
curl -sX POST localhost:7946/gateway/federation/connect -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' -d '{"domain":"partner.example"}'                         # federation:invoke
curl -s localhost:7946/gateway/federation/partners -H "Authorization: Bearer $TOKEN"   # federation:read
curl -sX POST localhost:7946/gateway/federation/call -H "Authorization: Bearer $TOKEN" \
     -H 'Content-Type: application/json' -d '{"domain":"partner.example","export":"surplus-food/collection","text":"…"}'
```

```python
fed = agent.federation()                      # mycelium-py ≥ 0.2.4 (the `federation` module ships in 0.2.4)
fed.connect("partner.example")
fed.call("partner.example", "surplus-food/collection", "…")
```

**Three things to get right before you expose this.**

1. **The principal the partner sees is the local caller's**, resolved by your own gateway's auth
   layer — not the node, and not the client's configured principal (that one is used only for
   discovery). On a gateway with **no token model** it is `anonymous`, and your partner's evidence
   will say so. If attribution matters to them, configure tokens *before* connecting the two.
2. **`federation:invoke` is the power to spend your domain's credential.** Grant it to the
   services that need it, not to everything holding a read scope — the split exists for this.
3. **`repeatable` is the caller's statement about their own effect**, and the only thing that lets
   a silent gateway be retried elsewhere. It defaults to false in both SDKs.

The consumer side is independent of the edge: a node may call out without exporting anything, or
export without calling out. `GET /gateway/federation/domain` answers `{"configured": false}` on a
node with no edge, which is the quickest way to tell the two halves apart in a deployment.

## Bounding what a partner can send you

The consumer side has always metered slots per partner (`FederationClient`'s pool bounds what you
send). The provider side does now too, and it is **off unless you set it**:

```rust
let edge = FederationEdge::new(our_domain, exports, policy, bundle, CallPolicy {
    max_in_flight_per_partner: 8,        // 0 (the default) is unlimited
    ..CallPolicy::default()
});
```

A call over the cap is refused with `AtCapacity` and JSON-RPC **-32004**, not queued. Partners
should treat -32004 as *retry shortly* and -32003 as *talk to your operator* — the two are kept
apart precisely so a transient load answer is not read as a standing authority answer.

**Sizing it, and the three things to know before you do.**

1. **The cap is per gateway.** Running N gateways means one partner can have up to **N × cap** in
   flight against your domain. Divide your intended aggregate by the number of gateways you run,
   and accept that a gateway which is down leaves its share unused — there is deliberately no
   cross-gateway counter (it would need either a coordinator or partner names gossiped through
   `sys/`, which the design forbids).
2. **It bounds concurrency, not rate.** A partner making brief calls in a tight loop stays under any
   in-flight cap. If rate is your problem, that is your ingress' job.
3. **Capacity is checked after authority**, so a partner you would refuse anyway cannot occupy a
   slot belonging to one you would admit.

Read the current occupancy with `FederationEdge::in_flight_for(&partner)` — the map holds only
partners with work in flight, so an empty read means idle, not unconfigured.

## A credential is one call (replays)

Every gateway's edge remembers the credentials it has served and refuses one presented again: the same signed
bytes a second time are a **replay**, refused with `Replayed` and JSON-RPC **-32003**, before any slot is taken or
anything is dispatched, and counted as `federation_replays_refused` on `GET /stats`. There is nothing to configure.
A body binding does not do this on its own: the copy of a captured call carries the same body, so before this guard
anyone holding a captured credential — on the path, or a log that kept the header — could send it again inside its
lifetime and the provider would act again. The key is the credential's signed bytes, so an *unbound* credential
presented with a different body is a replay too. An honest client never trips it: `FederationClient` mints every
credential with a strictly later `issued_at_ms` than its last, so two identical calls are two credentials.

What it does not do, and the window it leaves:

1. **Per gateway.** A domain that runs N gateways serves one credential up to N times, once at each — no shared set,
   for the reason the slot cap has none.
2. **In memory.** A restart forgets what was served, so a credential captured before a restart can be served once
   more after it, inside its lifetime — 300 s at most by default (`CallPolicy::max_lifetime`; shorten it to shorten
   the window).
3. **Kept until the credential expires**, plus the 30 s skew tolerance, by the gateway's clock; a wall clock stepped
   back further than the skew tolerance re-opens what it forgot.
4. **Bounded**, 65 536 credentials a partner by default (`FederationEdge::with_replay_window_per_partner`). Full, the
   window forgets the credential expiring first and refuses every credential expiring no later than it as
   `ReplayUndecidable` (JSON-RPC **-32004**, `federation_replays_undecidable`): fail closed, never a replay served. A
   fresh credential is served, so a partner's client retries through it; a rising count means the partner calls faster
   than the window holds, and the fix is a larger window or a shorter lifetime.
5. **Two clients of one domain** with the same principal, minting the identical call in the same millisecond, mint the
   same credential, and the second is refused as a replay. Give each client its own principal.

The catalogue `GET` is not guarded: it changes nothing, and its reply is signed for the asker.

## Encrypting the link (TLS pinning)

Federated calls are signed, not encrypted: without this step an on-path observer reads every call
and every reply. Turning it on is two decisions on each side.

**The partner's side — serve TLS and publish a pin.** Run the gateway behind `gateway_tls`, then
publish the sha256 of the endpoint's `SubjectPublicKeyInfo`:

```bash
# From a certificate file:
openssl x509 -in gateway.pem -pubkey -noout | openssl pkey -pubin -outform der | sha256sum
```

On the node-cert reuse path (`GatewayTlsConfig::default()`) there is no certificate file to read —
the certificate is regenerated at every startup — but the identity key behind it is stable, so
publish `federation::pinning::ed25519_spki_sha256(&identity_key)` instead. The value is the same;
`the_two_ways_to_compute_a_pin_agree` is the test that keeps it so.

**Your side — pin it and require https.**

```rust
bundle.pin_tls(&partner_domain, pin);                     // beside their signing key
let client = FederationClient::new(..)
    .with_tls_pins(bundle.tls_pins_for(&partner_domain).to_vec());
```

Send the pin over a channel that is not the link you are securing (the same call anyone would make
about a signing key), and **verify it with the partner before installing it** — a pin accepted from
whoever is currently answering is not a trust decision, it is a record of who answered.

Once pins are set, an `http://` endpoint for that partner is refused rather than dialled
(`Tls(PlaintextEndpoint)`), so fix the URLs in the same change.

### Rotating a TLS key without a flag day

The pin list is a list for this reason. In order:

1. The partner generates the new key and publishes its pin **before** using it.
2. Every counterparty runs `bundle.pin_tls(&partner, new_pin)` — both pins now verify.
3. The partner swaps its endpoint to the new key. Nothing is refused, on either side.
4. Every counterparty runs `bundle.unpin_tls(&partner, &old_pin)`. **The window is not closed until
   this step runs** — until then the retired key still opens a connection.

Doing 3 before 2 is the outage: every counterparty refuses the endpoint with `Tls(PinMismatch)`, and
the refusal names the digest that arrived, which is the new one. That is the symptom to recognise —
a `PinMismatch` whose `presented` value matches the partner's *new* published pin means step 2 was
skipped, not that anyone is being attacked.

## Rotation (signing keys)

```rust
bundle.rotate(&partner_domain, new_key, overlap_until_ms);
```

Rotation is **overlapping by design**: `acceptable_keys` returns both the old and new key until
`overlap_until_ms` passes. Set the overlap wider than the partner's deployment window, or calls
signed with the old key fail during their rollout.

This is a different key from the TLS pin above and rotates independently: one signs credentials and
catalogues, the other terminates connections, and an operator may well hold them in different
places. Revocation, below, covers both — a revoked partner's pins stop being returned too.

## Revocation

```rust
bundle.revoke(&partner_domain);   // trust side
edge.revoke(&partner_domain);     // grant side
```

Do **both**. Revoking trust stops you accepting their credentials; revoking the grant stops you
serving them. Either alone leaves half the relationship live.

`is_revoked` is the check to assert in a post-change smoke test.

**Revocation is per partner**, and so is everything else on this page. One edge serves any number of
partners from one bundle and one policy: each gets a catalogue filtered to its own grants, each is
authenticated against **its own** key (a partner's key never authenticates a credential naming another
partner), and revoking one leaves the others serving. Gated by
`three_domains_one_edge_and_the_middle_domain_is_not_a_bridge` and four edge tests.

Capacity is per partner too: `max_in_flight_per_partner` (above) holds each partner to its own share of
the edge, so one partner's burst does not refuse another's calls.

## Partitions, and why some answers are "unknown"

A federated call returns one of four things. Two are the interesting ones:

| Outcome | What it means operationally |
|---|---|
| `Completed` | it ran and answered |
| `Refused(..)` | the verifier said no, with a reason — **look at which reason** |
| `DeliveryUnknown { attempted_via, reason }` | a gateway stopped answering. **Nothing was learned** about whether the far side ran it |
| `NoCapacity { partner, slots_per_partner }` | every gateway's slots for that partner were full. Refused, **not queued** |

`DeliveryUnknown` carries the gateways it tried, in order, so you can go and look at the right one.

**Repeatability decides failover.** A `Repeatable` call may be retried through another gateway. An
`AtMostOnce` call is **never failed over** — an unknown outcome stays unknown, because retrying it
elsewhere could run it twice.

A blackholed gateway returns `DeliveryUnknown` **within a bound** rather than hanging. That was a
real defect found by the two-mesh demonstration: the in-process test could not have caught it,
because severing by stopping a gateway produces a refusal, and a refusal fails fast. The client's
defaults are a 5-second connect and a 30-second total; `with_timeouts` changes them.

Retire a gateway you are taking out of service with `retire_gateway`, so in-flight accounting drains
rather than stranding slots.

## Reading a refusal

Refusals are typed so they route you to the right file. Two pairs matter most:

- **`BadSignature` is not `NotPermitted`.** The first says somebody is forging; the second says a
  partner asked for something you chose not to grant. Reading a forgery as a policy gap sends you to
  edit the wrong file.
- **`Replayed` is not `ReplayUndecidable`.** The first is this exact credential served before — someone
  holds a captured call; the second is a partner window full, which a fresh credential resolves.
- **`WrongExport` on a catalogue path** means a credential crossed the catalogue/call boundary,
  which is a partner-side bug rather than a permissions question.

`LifetimeTooLong` is about **your** `CallPolicy`, not their credential: they claimed a lifetime longer
than you accept. Defaults are a 300-second maximum lifetime and 30 seconds of clock-skew tolerance.

- **`Tls(PinMismatch)` is not `DeliveryUnknown`.** A pin mismatch means the handshake never
  completed, so the partner received nothing and even an at-most-once call is safe to retry once the
  cause is fixed. Do not treat it as a possible execution. Check it against the rotation order above
  before assuming an attack.

## Reading a refusal from an SDK

The gateway routes carry the same distinctions the Rust API does, in two fields on every refusal
body, because an SDK caller cannot see `ClientError`:

| Field | Values | What you do with it |
|---|---|---|
| `sent` | `true` / `false` | `false` means the refusal happened at **your** gateway and nothing crossed — safe to retry |
| `delivery` | `none` · `refused` · `completed` · `unknown` | `unknown` is *we cannot say whether it ran*, never a failure |

Both SDKs raise a distinct type for `unknown` (`DeliveryUnknown` in Python,
`DeliveryUnknownError` in TypeScript) precisely so it cannot be caught by accident alongside
ordinary failures and retried.

## Starting under `secure-multi-domain`

`profile = "secure-multi-domain"` (or `GOSSIP_PROFILE`) makes `start()` refuse a node of a federating domain unless
its side of the edge is set the way this runbook describes (adopter-surface Y7). It requires everything
`secure-single-domain` requires ([readiness checklist](production-readiness.md)) and the federation family of the
[guarantee catalogue](../reference/guarantee-catalogue.md):

| Guarantee | On | Met by |
|---|---|---|
| `fed.domain_enforced` | every member | `domain_profile = "enforced"`: TLS required, SWIM off (its datagrams are unauthenticated UDP) |
| `fed.pinned` | a node with clients | `FederationClient::with_tls_pins` on every client — [Encrypting the link](#encrypting-the-link-tls-pinning) |
| `fed.body_bound` | a node with an edge | `CallPolicy::require_body_binding = true` |
| `fed.partner_slots` | a node with an edge | `CallPolicy::max_in_flight_per_partner > 0` — [Bounding what a partner can send you](#bounding-what-a-partner-can-send-you) |
| `fed.caller_named` | a gateway with clients | a gateway credential model, so a call names the local caller, not `anonymous` (`gw.not_open`'s check, restated) |
| `fed.catalogue_signed` | either side | `FederationEdge::with_signing_key` on the edge; `FederationClient::with_partner_key` on every client |
| `fed.partner_egress` | a node with clients | `egress.allow_hosts` permitting every partner endpoint's host — name it exactly; a `.suffix` entry admits every host under it |

A member that attaches neither an edge nor a client reads the six side-specific ones `not_applicable` and starts on
the rest; the start log and `doctor` say in one line that it faces no partner. **Attach the edge and the clients
before `start()`:** under this profile one attached after it is ignored with a warning (counted in `late_attachments`),
because the check at start never saw it. Under `dev` and `secure-single-domain` a late attachment applies as before. Three more are listed `not_verifiable_here` and never counted: `fed.partner_posture` (the partner's
own report, under the same profile, exchanged out of band), `fed.non_merger` (a fact about both domains) and
`fed.clock_agreement` (a credential's window is wall-clock across the edge). Nothing this node checks makes the
partner honest.

The edge and the clients are attached in code, so `mycelium doctor`, which builds a node from a file, reports the
six as `not_applicable`; read the running node's `GET /gateway/guarantees` or `guarantee_report()` after attaching
them, or run `mycelium::doctor::run_attached` — the same doctor, over the node as your program builds it, never
started. A refusal names each one unmet, as `[fed.body_bound: not_configured — …]` after `rev 1:`.

**A fleet whose mesh nodes run under it.** The two-mesh Docker suite (`make test-federation`, CI job `federation`)
starts every mesh node of both domains under this profile: a one-shot issuer per domain mints the CA in a volume no
node mounts and issues each node's certificate into that node's own volume (`mycelium::issue_node_cert`, what
`mycelium tls issue` runs), the gateways serve HTTPS on the node identity and the consumer pins each one's SPKI, the
egress allow-list names the partner's endpoint hosts, and each node carries the authority, audit, persistence and
electorate settings the readiness checklist lists — `examples/federation_node.rs` and the comment at the top of
`docker/docker-compose.federation.yml` say which setting meets which guarantee. The runner reads each node's own
`GET /gateway/guarantees` for the profile met, finds only each node's own key on its filesystem, shows the evaluator
and the body binding refusing calls the catalogue granted, and starts one more node with its CA key on-node, which
refuses to start naming `id.ca_key_off_node` alone. **Its consumer is not a node:** it is a bare `FederationClient`, so
the client side's facts — `fed.pinned`, `fed.caller_named`, `fed.partner_egress` on a node holding clients — read
`not_applicable` on every node there and are shown by the in-process tests only (the two-domain scaffold, Y8, is where
a client node runs under the profile). It is a test fleet: its keys and gateway token are fixtures in the clear.

**A deployment to start from: the two-domain scaffold.** `mycelium init <dir> --template two-domains` writes two
fleets of two nodes — a food bank that exports `pantry/stock` and `pantry/reserve`, a kitchen that exports
`menu/today` — each under this profile, each gateway running an edge and holding a client for the other, with the
operator's steps in `fleet.sh`: `mycelium tls ca` mints each domain's CA, `mycelium tls issue` each node's certificate,
`mycelium tls pin` each gateway's key for the partner's client to pin, and the domain signing keys, named tokens and
egress lists are made or named there. Each gateway's own `GET /gateway/guarantees` reads all seven `fed.*` node facts
`enforced`, the client side's among them; a call names `federation:kitchen.example/token:kitchen/planner` at the food
bank's provider, and the other way `…/token:foodbank/steward` at the kitchen's; a wrong pin, a rewritten body and a
third concurrent call against a cap of two are each refused — CI's `Scaffold` job runs every line
([`templates/two-domains`](../../templates/two-domains/README.md)). Its four nodes share one host, so the separation of
`ca/` from each node's `certs/<node>/` is shown by layout, not enforced by the host.

**A credential presented twice.** The edge checks a federated credential's signature, its body binding, its window
and the grant; it keeps no record of the credentials it has served, so the same credential with the same body,
presented again inside its window (at most `CallPolicy::max_lifetime`, 300 s by default), is served again. **Anyone
who holds a captured credential and its body** — on the path, at a proxy that terminates TLS, in a log that kept a
request — can repeat its effect within that window, not only the partner; an at-most-once effect needs its own
idempotency key at the provider ([side effects](../features/side-effects.md)) until the edge refuses a replay.

## What this runbook does not cover

- **Running a hub** — a node that is a provider to one partner and a consumer of another — works
  and is gated (`three_domains_compose_without_trust_composing`), but this runbook has no section
  on operating one. Two things to carry over meanwhile: trust is **not** transitive (your partner's
  partners are strangers to you, and a call from one is refused before any catalogue is computed),
  and a grant you *hold* is never re-exported by holding it — if a third party should reach it,
  that is a grant you write, deliberately, in your own policy.
- **Streaming under a credential** is refused: federated calls are unary (§5 of the record).
- **Who the caller is inside your own mesh** is [rbac](rbac.md); federation preserves the origin
  principal but does not authorise it for you.
