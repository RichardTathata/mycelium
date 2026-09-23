## [2026-09-23] ingest | the budget runs both ways — the Phase-C audit's last finding, decided

Up: [dev](../dev.md) · pages touched: [security](../security.md) (federated domains),
[concurrency/lock-order](../concurrency/lock-order.md) (row 40) · record
`docs/design/federated-domains.md` · code `src/federation/call.rs`, `src/federation/edge.rs`,
`src/agent/a2a.rs` · docs guide 17, `docs/operations/federation.md`, `CHANGELOG.md`.

The second of the axis' two open design decisions. The first (commitment provenance) is its own
entry; the two are unrelated in subject and identical in shape — *a rule that existed on one side of
a boundary and not the other*.

## What was actually exposed

The audit's sentence was *"the per-partner budget is enforced consumer-side only (the edge has no
slot accounting)"*, and it is easy to read that as a flood risk it is not. An anonymous flood is
already stopped: `/a2a` refuses a request whose federation credential does not verify. What is not
stopped is a **partner**, because a partner's credentials are by construction minted by *them* — so
the threat is a compromised or simply buggy partner holding this gateway's whole capacity, and no
amount of credential checking touches it.

Put plainly: `GatewayPool` bounds what we send a partner, and until now nothing bounded what a
partner sends us. One edge, one budget concept, one side of it implemented.

## Why not the M7 shape

The obvious reach is `mycelium-core`'s `rate` module — **shared observation, local decision** —
applied to partner domains instead of mesh peers. It is better in one respect: it catches a partner
fanning out across several of our gateways, which a per-gateway cap cannot.

It loses on two:

- **It puts foreign domain names into `sys/`.** Federation state is otherwise confined to gateway
  nodes; `sys/fed-rate/{observer}/{partner}` would tell **every node in the mesh** which partners
  exist and how hard each is calling. D7 — *foreign state never enters the gossip medium* — was
  written for exactly this, and the widening is larger than the mechanism.
- **It cannot be built first.** M7's own design is a per-peer limit *first* (`max_inbound_frames_per_sec`)
  and the aggregate view second; the decider has nothing to clamp without it. So the per-gateway cap
  is the **prerequisite** for the shared-observation version, not a competitor to it. If a
  deployment ever needs the aggregate, this is the thing it will be built on.

## The mechanism, and the three decisions inside it

`CallPolicy::max_in_flight_per_partner` (0 = unlimited) and `FederationEdge::admit` →
`PartnerSlot`. Three choices worth keeping:

1. **The release is a `Drop`, not a method.** A release a handler has to remember is one it misses —
   on an early return, on a later refusal, on an unwind — and a leaked slot is a partner permanently
   short of capacity **with nothing saying so**. That is the worst failure shape available here: it
   degrades silently and looks like the partner's problem.
2. **Capacity is asked after authority.** A caller we would refuse on authority must not be able to
   occupy a slot, or an unauthorised partner could exhaust an authorised one's allowance — turning
   an access-control refusal into a denial of service against the partner who was in the right.
3. **-32004, not -32003.** A capacity refusal is transient and a retry resolves it; `NotPermitted`
   is standing and a retry will not. Folding them together tells a partner to go and ask their
   operator about a grant they already hold — the same reasoning that keeps `BadSignature` and
   `NotPermitted` apart one layer down.

Lock-order **row 40**, never nested with row 38: `authorize` takes and releases `trust` before
`admit` is called, and the in-flight map is untouched entirely when the cap is 0.

## The gate that had to be concurrent

The unit test proves the counter. It cannot prove the cap *applies*, and "a mechanism nothing wires"
is the failure this project has found in its own gates twice this month. So the second gate drives
**two concurrent calls through a real gateway** — against a provider that blocks until the test
releases it, because with an instant provider the first call finishes before the second arrives and
the cap is never consulted. Removing the `admit` call from the `/a2a` path makes it fail, which was
checked rather than assumed.

## What is still asymmetric, stated rather than implied

The count is **one gateway's**: N gateways ⇒ N × cap in aggregate. And it bounds **concurrency, not
rate** — a partner making brief calls in a tight loop stays under any in-flight cap. Rate is the
operator's ingress. The asymmetry between the two sides of the edge is now smaller, not gone, and
the record says which part remains.
