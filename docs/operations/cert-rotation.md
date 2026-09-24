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
its own keys — the coordinator-free trade-off. A **fully-compromised or offline** node cannot
be force-revoked by a fleet operator without a separate operator-authority mechanism (not yet
provided). Re-issuing the cluster CA remains the heavier fallback for that case.

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

**The rollout argument for making this the default is complete; the default is still off, and the
gap between those two facts is worth your attention before you set it.** Phase 2 shipped in
**v2.3.0 (2026-07-24)** and every TLS node has written `sys/identity-proof/{self}` unconditionally
at startup since, so no node within a supported rolling-upgrade range writes an unsigned identity.
On that reasoning the default was flipped to `true` on 2026-09-23 — and reverted on 2026-09-24,
because a node's identity and its proof are **two separate KV writes**, hence two gossip messages
with no ordering between them.

A peer that learns the identity *before* the proof rejects it and holds no key for that node until
the proof arrives. The key recovers on its own — the identity watcher subscribes to the broader
`sys/identity` prefix precisely so a late proof re-validates its entry — so the window is
**transient**. A decision taken inside it is not: a leader election is one-shot, and a node that
could not verify a peer's signature during the window does not re-run the election afterwards. The
Docker suite showed an intermittent `S12 leader election … Nodes disagree on leader` after twelve
consecutive green runs. Closing the window properly means an atomic identity+proof record, or a
bounded "pending its proof" state that defers rather than rejects — a design change, not a default.

**So: turn it on** if you want the unsigned-mimic residual closed and your fleet does not elect
leaders during bring-up. **Leave it off** if nodes join and elect in the same breath, or if you
genuinely run nodes older than **v2.3.0** (they write no proof and would never join).

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
| rotating away from a compromised key, old signatures still verify | retained-key design (option B) | perform explicit revocation (re-issue CA / rebuild trust), not just rotation |
