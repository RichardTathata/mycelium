# Mycelium — Hot Certificate / Identity Rotation Runbook

Operator guide to rotating a node's Ed25519 TLS/identity key **without cluster
disruption** (WS5; `tls` feature). Concept: the same key is the node's mTLS cert
key *and* its signing/identity key (`sys/identity/{node}`), so a rotation swaps
both at once. See [`../guide/09-security.md`](../guide/09-security.md).

---

## 1. What rotation does

`GossipAgent::rotate_identity(propagation)`:

1. Generates a new key + a fresh node cert **signed by the existing cluster CA**
   (the CA is *not* rotated), persisted to disk.
2. Publishes `sys/identity/{self}` = `new ‖ old` (the raw `32×N`-byte key history),
   so peers' retained key sets accept both. **NOTE (2026-07-15):** this entry is
   written **unsigned** — a plain Layer-I KV value, *not* signed by the old key (that
   was the original design intent; it was never implemented). Peers accept the new key
   because the KV is unauthenticated and accumulated — the identity-poisoning gap tracked
   in [`docs/design/identity-authentication.md`](../design/identity-authentication.md)
   (CFT-not-BFT: a compromised admitted node can inject a key; out of the base threat model).
3. Waits `propagation` for that to gossip cluster-wide.
4. Atomically cuts over the active key + rustls configs. New gossip signatures
   and **new** TLS handshakes use the new key immediately; **existing**
   connections keep their CA-trusted session (no listener restart, no dropped
   frames).

```rust
// node started with cfg.tls = Some(..)
let new_vk = agent.rotate_identity(std::time::Duration::from_secs(10)).await?;
```

Pick `propagation` ≥ a few gossip rounds (your `health_check_interval_secs` ×
2–3) so peers learn the new key before the cutover. Over-shooting is harmless;
under-shooting just means a brief window where peers catch up via anti-entropy
(verification still converges — see §3).

---

## 2. Retained-key verification (why historical records still verify)

Identity keys rotate, but records signed earlier must stay verifiable. So
`peer_keys` holds a **set** of keys per node, **accumulated** across rotations
(option B): every verify path — inbound `SignedData`, consensus votes, role
claims, and the audit chain — tries all keys for the signer. A node's audit
stream therefore verifies end-to-end even though records before and after a
rotation are signed by different keys.

- The `sys/identity/{node}` value stores the **full** key history (`32 × N` bytes:
  current first, then every prior key), so verification survives **any** number of
  rotations and restarts — historical records always have their signing key
  available. The entry grows 32 bytes per rotation (rotations are rare).
- **Compromise caveat:** a retired key remains *accepted for verification* — good
  for history, but it means rotating away from a **compromised** key does not by
  itself stop the attacker's old signatures from verifying. Rotation is **hygiene**;
  compromise response needs explicit **revocation**.

### Compromise remediation — rotate *and* revoke (SOC 2 WS-B)

A signed revocation of the old key is validated cluster-wide and excluded on **every**
verify path — role claims, the audit chain, **and consensus** — so the old key stops
verifying everywhere. Two ways:

- **One call:** `agent.rotate_identity_on_compromise(propagation)` — rotates to a fresh key,
  then revokes the outgoing one *with the new key*. Use this when the current key may be
  compromised.
- **Operator trigger (no code):** `POST /gateway/identity/revoke` (scope `identity:write`,
  `compliance`), body `{"revoked_key":"<64 hex>","reason":"..."}` — after a plain
  `rotate_identity`, revoke the old key over HTTP.

```bash
curl -X POST https://gateway:9443/gateway/identity/revoke \
  -H "authorization: Bearer $TOKEN" \
  -d '{"revoked_key":"<old-verifying-key-hex>","reason":"suspected compromise"}'
```

**Ownership limit (by design):** only the node itself, holding its *current* key, can revoke
its own keys — the coordinator-free trade-off. A **fully-compromised or offline** node is the
operator's case, below; an earlier version of this paragraph said that mechanism was *not yet
provided*, which stopped being true with member removal (C5, v2.15.0).

**AgentFacts across a rotation.** The facts edge (`/.well-known/agent-facts.json`) re-signs the
document with the node's *current* key on every fetch, so a partner pulling after the rotation
verifies against the new key; a partner that cached the previous document within its `ttl_secs`
still verifies it against the key it was signed with (`SignedFacts::verify`), and the domain board's
fields verify with `verify_any` against the known-key history. Nothing needs re-publishing.

### Removing a member (an operator's authority, C5)

Self-revocation is a node's act; **removal** is an operator's, and it names *every* key the member
ever held so the removed node cannot come back under an older one.

1. **Before you need it:** every node attaches the operator authorities it will honour —
   `agent.with_membership_authorities(authorities, external)` (`src/agent/membership_ops.rs`) —
   and the CA key lives **off every node** (`ca_key_off_node` in the confinement report reports
   *unmet* otherwise, because a node that can mint a removed member a new identity defeats the
   removal).
2. **Build and sign** a `MemberRemoval { node, keys, seq, issued_at_ms, reason }` over its
   `canonical_bytes()` with the operator's key (`src/membership.rs`), producing a
   `SignedMemberRemoval`.
3. **Offer it** to any live node: `agent.offer_member_removal(&signed)`. It is written under
   `sys/membership/removed/{node}` (monotonic by `seq`; a lower sequence is refused) and gossips.
4. **Verify:** `agent.removed_members()` on a few nodes, or
   `curl -s localhost:PORT/gateway/kv/keys | grep 'sys/membership/removed/'`; a connection
   presenting any removed key is refused at the TLS handshake.

**Not covered:** gateway bearer tokens the removed member held are a separate revocation
(`rbac.md`); and a CA key that *is* on a node can still mint a fresh identity — which is why step 1
is a precondition, not advice. Design record: `docs/design/member-removal.md`.

### Authenticated identity — enabling proof enforcement (identity-auth Phase 2/3)

Every TLS node now publishes a signed `sys/identity-proof/{self}` alongside its identity, and
peers **reject** an identity overwrite whose proof doesn't chain to a key they already trust — so
the key-poisoning vector (a forged verifying key injected via `sys/identity`) is closed for any
connected/established peer. Rejections increment `identity_anchor_conflicts` on `/stats`.

**`require_identity_proofs` is `false` by default — an operator opt-in.** Set it, and an
*unsigned* identity entry (one mimicking a pre-Phase-2 node) is rejected outright rather than
tolerated:

```toml
require_identity_proofs = true      # or GOSSIP_REQUIRE_IDENTITY_PROOFS=1
```

**What "proof" means here changed in Phase 3b, and it is the difference between safe and not.**
With the flag set, this node accepts the **sealed** record `sys/identity-signed/{node}` — key
history and proof in **one** KV entry — and does *not* accept the older
`sys/identity/` + `sys/identity-proof/` pair.

That is not pedantry. The pair is two gossip messages with no ordering between them, so a peer
requiring proofs could learn the identity first, reject it, and hold **no key** for that node until
the proof arrived. The key recovers on its own (the identity watcher subscribes to the broader
`sys/identity` prefix, so a late proof re-validates its entry) — but a one-shot decision taken
inside that window, such as a leader election, would not.

**No deployment has been observed hitting this**, and one claim that it had was wrong: an attempt to
make the flag default-on (2026-09-23, reverted 2026-09-24) coincided with an intermittent
`S12 leader election … Nodes disagree on leader` in the test fleet, and the two were connected in
the write-up. They cannot be: the flag is inert without TLS and those nodes configure none. Treat
the window as a hazard removed on principle, not a bug you have been living with.

One entry cannot arrive in two parts, so the window is closed by construction rather than by timing.

**The precondition for turning it on** is therefore the ordinary one: **every node in the fleet
must run a release that writes the sealed record.** A node on an older release publishes only the
pair, and this node will refuse it — correctly, but you will have removed a healthy peer. Check
before you set it:

```bash
# One sealed record per node, or you are not ready.
curl -s localhost:PORT/gateway/kv/keys | grep -c 'sys/identity-signed/'
```

**Leave it off** while any node predates the sealed record, and while any node predates **v2.3.0**
(those write no proof at all).

**What this does *not* give you, and it is worth being exact.** *Proofs required* is **not**
*identity authenticated*. First sighting of a node you have never seen is still **trust on first
use**: a self-signed entry is accepted, because there is nothing established to chain it to. An
admitted-but-hostile member can therefore still introduce a key for a node nobody has met yet.

What closes *that* is an **anchor** — a direct, CA-validated connection, which records the peer's
real key (Phase 1b) and after which an unchained key is rejected **and counted** in
`identity_anchor_conflicts`. The two mechanisms are complementary: proofs close the unsigned-mimic
residual, anchors close the first-sighting one. If your topology means some node pairs never connect
directly, that pair's first sighting is the window, and `identity_anchor_conflicts` on `/stats` is
where a later contradiction shows up.

---

## 3. Verify a rotation went cleanly

```bash
# The identity entry grows by one 32-byte key per rotation (current ‖ priors).
# Peers' audit verification of this node must stay green across the rotation:
curl -s -H 'Authorization: Bearer <audit-token>' \
     "http://PEER:PORT/gateway/audit?node=<rotated-node>" | jq '.streams[0].verified'
# → true   (the chain spans the rotation; the peer holds both keys)
```

Expectations during/after rotation:
- **No dropped frames**, no connection loss (existing sessions persist; new ones
  use the new cert).
- Peers' `audit_verify(rotated_node)` stays `true` across the rotation.
- A peer that was offline during the window picks up the new key via
  anti-entropy on reconnect (the retained set means the old key still verifies
  any backlog signed before the cutover).

---

## 4. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `rotate_identity` → `InvalidField { field: "tls" }` | node has no `GossipConfig::tls`, or no cluster CA on disk | enable tls; rotation requires an established CA (`ca-cert.pem` + `ca-key.pem` in `auto_cert_dir`) |
| peers briefly reject the node's new-key frames | cutover happened before the new key gossiped | increase `propagation`; verification self-heals via anti-entropy once the key arrives |
| `sys/identity/{node}` entry growing over time | by design — the full key history is retained (32 B/rotation) so old signatures verify | none needed; rotations are rare. If ever a concern, prune keys older than your audit-retention horizon |
| rotating away from a compromised key, old signatures still verify | retained-key design (option B) | remove the member (§ Removing a member) or, as the heavier fallback, re-issue the CA / rebuild trust), not just rotation |
