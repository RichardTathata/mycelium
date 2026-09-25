# Removing a member (ADR, Boundary H closure plan C5)

**Status:** **proposed** 2026-09-25, for review before any code. Plan:
[`plans/boundary-h-closure.md`](../plans/boundary-h-closure.md) C5 (finding F7). It builds on the issuer-binding ADR
([`knowledge-issuer-binding.md`](knowledge-issuer-binding.md), P1's external-issuer path), the A1 ADR
([`authority-at-execution.md`](authority-at-execution.md), whose revocation view this mirrors), and the confined-fleet
ADR ([`confined-fleet.md`](confined-fleet.md)).

> Posture, once: **revoking what a member may do is not removing the member.** A1, C3 and C4 stop a member's
> *authorised work*. A member that is still a member can still gossip, still be a peer, and still hold whatever
> the mesh gives every member. Removal is the operator saying: *this identity is no longer one of us.*

---

## 1. The problem

Today an operator cannot cut a member off (F7):

- **Identity-key revocation is self-revocation.** `sys/revocation/{node}/…` must be signed by the node's own current
  key: it exists so an honest node can retire a compromised key, and a colluding node will never sign its own.
- **It affects signatures, not traffic.** Revoked keys fail verification; RPCs and gossip from the node are still
  accepted.
- **At the transport, any certificate chaining to the fleet CA is a member.** There is no revocation list or
  deny-list. Removing a member today means rotating the CA and reissuing every other node.

## 2. Decision (proposed)

### 2.1 The act

An operator-signed `MemberRemoval { node, keys, seq, issued_at_ms, reason }`:
- signed by a **configured membership authority** (P1's external-issuer path; `GossipConfig::membership_authorities`);
- naming the node and **every key it is known to hold** (current and retained), so a rotation does not escape it;
- written to `sys/membership/removed/{node}` (a new reserved namespace), and also offered directly to each node
  (`offer_member_removal`), the way revocation checkpoints are, so a colluding member cannot suppress it by refusing
  to forward.

**Monotonic, like A1's revocation view after #402:** a removal, once seen, stands; there is no "un-remove", only a
fresh admission under a **new identity**.

### 2.2 Where it is enforced

| Layer | What a removed member gets |
|---|---|
| RPC receive (`RpcRequestRx`, the MCP loops, the SDK serve stream) | Refused before any serve path, whether it calls directly or its principal arrives through a gateway envelope |
| Gossip ingest | Updates originated by a removed node are not applied |
| Membership / SWIM | Not admitted as a peer; its liveness claims are ignored |
| TLS handshake | A client/server certificate verifier rejects a removed node's identity key, so it cannot rejoin by reconnecting |
| Knowledge (P1) | Its signatures verify as `Removed` (attributable history, never current) |
| A1 / C3 | Its mandates are refused, since the holder is removed, independent of the authority's checkpoints |

### 2.3 The design question: what does "unknown" mean for membership?

A1 fails **closed** when revocation standing is unknown: without a fresh checkpoint, protected work stops. The same
rule applied to membership would mean: a node that has not heard from the membership authority within *F* treats
**every** peer as possibly removed, and stops talking to all of them. An authority outage would then partition the
whole fleet.

Three options:

| Option | Behaviour when the membership view is stale | Cost |
|---|---|---|
| **A. Fail open on staleness** | Keep every peer not yet seen removed. Removals already seen stand (monotonic) | A removal takes effect as fast as it propagates, and a node partitioned from the authority keeps a removed peer until it hears |
| **B. Fail closed** | Refuse every peer until a fresh membership checkpoint arrives | An authority outage longer than *F* stops the mesh |
| **C. Split by layer** (recommended) | Transport, gossip and membership fail **open** (A); **protected work** already fails closed through A1 and C3 | The removed member's *authorised* work stops within A1's bound; its *presence* goes when the removal arrives |

**Recommendation: C.** Protected work is where harm is done, and it already has a closed-failing clock (A1). Presence
(gossip, liveness, being a peer) is what keeps the fleet coordinating through an authority outage. Failing it closed
would turn a single authority fault into a fleet-wide one, which is the Coordinator Trap this substrate exists to
avoid.

### 2.4 The assumption that decides whether removal is real

**A removed member must not be able to mint a new identity the fleet will accept.** If member nodes share the
fleet CA's private key (the `auto_cert_dir` development default), a removed member can issue itself a fresh
certificate and a fresh identity key, and rejoin as a stranger. Removal then only removes a *name*.

So the strict profile requires:
- the CA key held **off** member nodes (an operator-held CA, or a per-node certificate issued by an enrolment
  service);
- admission of a new identity to be an operator act, not something any holder of the CA can do.

The confinement report gains a line for it: `ca_key_on_node`, which the node **can** see (whether the CA's private
key is in its own certificate directory), reported `Present` or `Absent`. `Present` is an unmet strict setting.

## 3. What this does not claim

- **Instant removal.** It takes effect when it arrives: gossip propagation, or the operator's direct offer.
- **Removal of work already running.** In-flight work stops through C10's cancellation, not through removal.
- **Anything about a member whose CA-holding makes rejoining possible.** §2.4's profile is what makes removal mean
  something; without it, removal is a name change.

## 4. Gates (when built)

- A removed member's direct RPC, a gossip update it originates, and a reconnect are each refused; an unremoved
  member is unaffected (the plant).
- A forged removal (unknown authority, bad signature) is ignored.
- A removal seen once stands against a later view that omits it.
- A removal naming a retained key catches a node that rotated after being removed.
- With the membership authority silent past *F*: peers stay (option C), and the removed member's protected work is
  still refused (A1).
- The confinement report flags a CA key on the node.

## 5. Questions for review

1. Option **C** (split by layer), or a different stance on stale membership?
2. Is requiring the CA key off member nodes acceptable for the strict profile, given that the development default
   shares it?
3. Should a removal also **revoke the removed node's outstanding mandates** at every authority, or is "the holder is
   removed" at A1 enough?
