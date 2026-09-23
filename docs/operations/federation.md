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
let edge = FederationEdge::new(our_domain, exports, CallPolicy::default())
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

Then, from any local client holding a bearer:

```bash
curl -sX POST localhost:7946/gateway/federation/connect -H "Authorization: Bearer $TOKEN" \
     -d '{"domain":"partner.example"}'                         # federation:invoke
curl -s localhost:7946/gateway/federation/partners -H "Authorization: Bearer $TOKEN"   # federation:read
curl -sX POST localhost:7946/gateway/federation/call -H "Authorization: Bearer $TOKEN" \
     -d '{"domain":"partner.example","export":"surplus-food/collection","text":"…"}'
```

```python
fed = agent.federation()                      # mycelium-py ≥ 0.2.5
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

## What this runbook does not cover

- **More than two domains** is item 2's row 11 and is not shipped. TLS on the edge and the SDK
  verbs *are* — see "Encrypting the link" and "Letting local clients call a partner" above, and
  [gateway-tls](gateway-tls.md) for the serving side.
- **Who the caller is inside your own mesh** is [rbac](rbac.md); federation preserves the origin
  principal but does not authorise it for you.
