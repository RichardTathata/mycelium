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

## Rotation

```rust
bundle.rotate(&partner_domain, new_key, overlap_until_ms);
```

Rotation is **overlapping by design**: `acceptable_keys` returns both the old and new key until
`overlap_until_ms` passes. Set the overlap wider than the partner's deployment window, or calls
signed with the old key fail during their rollout.

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

## What this runbook does not cover

- **TLS on the federation edge itself** is item 2's row 11 and is not shipped. The edge today rides
  the gateway's own transport posture — see [gateway-tls](gateway-tls.md).
- **More than two domains**, a hostile network, and the SDK verbs are also row 11.
- **Who the caller is inside your own mesh** is [rbac](rbac.md); federation preserves the origin
  principal but does not authorise it for you.
