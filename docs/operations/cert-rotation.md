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

A signed revocation of the old key is validated at every node it reaches and excluded on **every**
verify path — KV writes, identity records, role claims, the audit chain, caller envelopes, knowledge
records **and consensus** — so the old key stops verifying there. **Who may revoke** (post-360 plan
row Q, [`docs/design/revocation-authority.md`](../design/revocation-authority.md)): a revocation
counts only when signed by a key of the node that a **certificate record** anchors
(`sys/identity-cert/{node}/{key}`, a certificate the fleet CA issued naming the node by
`mycelium://<node>`) and that was **issued after** the revoked key's certificate — authority flows only
to the past, so a thief holding an old key cannot revoke the new one. A node with such a record trusts
exactly its anchored, unrevoked keys. Two ways:

- **One call:** `agent.rotate_identity_on_compromise(propagation)` — rotates to a fresh key,
  then revokes the outgoing one *with the new key*. Use this when the current key may be
  compromised.
- **Operator trigger (no code):** `POST /gateway/identity/revoke` (scope `identity:write`,
  `compliance`), body `{"revoked_key":"<64 hex>","reason":"..."}` — after a plain
  `rotate_identity`, revoke the old key over HTTP.

```bash
curl -X POST https://gateway:9443/gateway/identity/revoke \
  -H "authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"revoked_key":"<old-verifying-key-hex>","reason":"suspected compromise"}'
```

**Before restarting a node that revoked its key:** the audit chain's head is recovered at `start()`
by verifying the persisted stream under the retained keys, and a revoked key is excluded there too —
every record it signed fails `BadSignature` and reads as unverified after the restart (kept untouched,
reported as `audit_unverified`, never sealed over). Seal a checkpoint with the **new** key (`agent.audit_checkpoint()`) and prune
(`agent.audit_prune_to_checkpoint()`) **before** the restart, so the head anchors at the checkpoint
([audit.md §1, §5](audit.md)).

**What revocation does not protect against — read before relying on it:**

- **A CA holder.** In the development posture (`auto_cert_dir`) `ca-key.pem` sits beside every node's
  key and rotation needs it: any member, or anyone who compromises a host, can issue a certificate dated
  *now* that outranks every key of any node. A certificate dated past the clock plus the skew bound is
  ignored (`identity_cert_future`), which stops "for ever", not "now". A CA compromise is the same in
  any posture. The guarantee `id.revocation_authority` reports `not_configured` while the CA key is on
  the node.
- **The secure posture cannot rotate** (`rotate_identity` needs the CA key). Recovery there is
  **re-issue only**: `mycelium tls issue --force` a new certificate (`--force` because it replaces the node's key and
  every pin a partner holds for it — [deprecations § 69](../guide/deprecations.md#69-mycelium-tls-issue-refuses-to-replace-a-node-key-it-issued-before---force-replaces-it-tls-ca-and-tls-pin-are-new-unreleased-adopter-surface-y8); it is issued after every earlier one, so the
  new key outranks them), restart the node with it, then `POST /gateway/identity/revoke` the old key.
- **A retired key still verifies until it is revoked.** `rotate_identity` alone is hygiene.
- **Anti-entropy** entries carry no author and no signature, so a revocation cannot reach them. (An unsigned
  `Data` frame is refused at every TLS node since row F2 — `deprecations.md` §40.)
- **A node with a legacy certificate** (issued before row Q, without the URI) has no standing: no
  revocation of its keys counts at upgraded nodes, and keys it revoked before the upgrade verify again
  there until it is re-issued. **Re-issue every node's certificate as part of the upgrade**
  (`deprecations.md` §39). A development-posture node re-issues its own at every start.

**Ownership limit (by design):** only the node itself, holding its *current* key, can revoke
its own keys — the coordinator-free trade-off. A **fully-compromised or offline** node is the
operator's case, below; an earlier version of this paragraph said that mechanism was *not yet
provided*, which stopped being true with member removal (C5, v2.15.0).

**AgentFacts across a rotation.** The facts edge (`/.well-known/agent-facts.json`) re-signs the
document with the node's *current* key on every fetch, so a partner pulling after the rotation
verifies against the new key; a partner that cached the previous document within its `ttl_secs`
still verifies it against the key it was signed with (`SignedFacts::verify`), and the domain board's
fields verify with `verify_any` against the known-key history. Nothing needs re-publishing.

### A member that goes down: restore it with its state, or retire it — never reissue it empty

A node's identity is its `NodeId`, and `mycelium tls issue` can reissue its certificate off-node — but for
a member of a **consensus electorate group** (guide [04 § Discovery is not an
electorate](../guide/04-consensus.md#discovery-is-not-an-electorate)) the identity is not the whole
member. Its acceptor memory — the ballot it promised, to whom, and the value it accepted, per slot — is
on its own disk (`sys/consensus-accepted/{node}/{slot}`, fsynced before a promise or vote leaves since
2.32.0) and is read back once at start by `prewarm_accepted`. **An electorate member must configure
`[persistence]`** — without it nothing is on disk, the node answers anyway, and the hazard below recurs on
every restart. Nothing checks the store is the one the
member had: started under the old `NodeId` over an **empty or older** store, it answers as a fresh
acceptor — it can promise a ballot it had already promised past, or report no acceptance where it had
accepted — and a quorum that counts it can decide a **second value** for the slot. Decide, before
touching the identity:

- **Restore** — same `NodeId`, same certificate or one reissued off-node, **and** its persistence
  directory `{persistence.base_path}/{node_id}/kv/` (`snapshot.bin` + `wal.bin`) at its **latest** state,
  copied quiesced ([deployment.md § Backup & restore](deployment.md#backup--restore)). Safe; nothing to
  migrate. An older copy is the empty-store hazard with a shorter reach — every promise and vote after
  the copy is forgotten. Anti-entropy does not close it: a peer's copy of this node's own record does
  reach a fresh store, but the acceptor's memory is loaded once at start, so the copy is not in the
  running acceptor until the next restart, and the node's first own answer overwrites it by LWW.
- **Retire** — the directory is gone, or you cannot show it is the latest: do **not** reissue the
  identity. Remove the member (below) and count the group one member smaller for good — and leave its
  `grp/` entry alone: the proposer's door refuses every proposal on a group whose live roster differs from
  the electorate's members (`ElectorateMismatch`), so tidying the dead member out of the group stops it.
  Migrating the
  group into a successor — declare the successor naming it, retire it, complete — is the third branch
  ([guide 04 § Discovery is not an electorate](../guide/04-consensus.md#discovery-is-not-an-electorate)),
  while the group still has its majority.
- **Removal forecloses restore.** The ledger is monotonic — there is no un-removal — and a removed
  `NodeId` is dropped at every connection whatever certificate it presents, so a restore after a removal
  is a *new* member under a new identity, and the group has still lost one. Sign the removal only once
  you know the state is gone.
- **A lost majority** is not repaired by any of this: a disaster-recovery genesis forced from the
  survivors' durable records is not built ([what-is-proven.md](what-is-proven.md), the third table).

### Removing a member (an operator's authority, C5)

Self-revocation is a node's act; **removal** is an operator's, and it names *every* key the member
ever held so the removed node cannot come back under an older one. For an electorate member, read the
section above first: removal is permanent, and it forecloses a restore.

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

### Issuing node certificates off-node — the CA key stays with you (`id.ca_key_off_node`)

By default a node re-signs its own certificate at every start with the fleet CA's **private key**, which
must therefore be in its certificate directory (and is minted there when absent). The removal argument
above (C5) rests on the opposite: a node that holds the CA key could mint a removed member a new
identity. The guarantee `id.ca_key_off_node` holds only for a node that starts from a **pre-issued**
certificate and the CA **certificate** alone:

```bash
# on the issuer's host, where ca-cert.pem + ca-key.pem live (never on a node) — minted there once:
mycelium tls ca --out ./fleet-ca          # refused if ./fleet-ca already holds a CA; needs hard links (not FAT/exFAT/some SMB)
mycelium tls issue --ca-dir ./fleet-ca --node 10.0.0.7:8080 --out ./issued   # --force to replace a key issued before
# → ./issued/10_0_0_7_8080.cert.pem and .key.pem
# on the node: copy both plus ./fleet-ca/ca-cert.pem (the certificate only) into its directory, and
[tls]
auto_cert_dir = "/var/lib/mycelium/tls"      # holds ca-cert.pem; no ca-key.pem
cert_pem      = "/var/lib/mycelium/tls/10_0_0_7_8080.cert.pem"
key_pem       = "/var/lib/mycelium/tls/10_0_0_7_8080.key.pem"
```

With `cert_pem` set, `start()` loads the CA certificate and the node's certificate and key, needs no CA
key, looks for none, and **never mints a CA** — a missing `ca-cert.pem` is an error naming the path, and
`cert_pem` without `key_pem` is refused by name. Rotation (§1) then means **re-issuing**: `generate_rotation`
needs the CA key and errors without it, by design. The startup report resolves `id.ca_key_off_node` as
`enforced` only on such a node; `secure-single-domain` **rev 2 (v2.21.0) requires it** — a node under
the profile whose certificate was minted on-node refuses to start, naming `id.ca_key_off_node`
(announced in v2.20.0, one release ahead, per the plan's G12). The issuer's `mycelium` binary needs
`--features tls`; without it `tls ca` and `tls issue` exit 2 saying so. On unix every private key is written `0600`,
every certificate `0644`, and a directory these steps (or a node) create for them `0700`, whatever the umask; a
directory that already exists is left as you made it. `tls ca` holds the `ca.lock` a starting node takes, and a mint
interrupted between its two files leaves `ca-key.pem` without `ca-cert.pem` — the next `tls ca` or `tls issue` names
that state: remove the key (nothing can have been issued from it) and mint again. For a federation partner, `mycelium tls pin
<cert.pem>` prints the hex SPKI SHA-256 of a gateway's certificate — the value the partner's client pins
([federation § Encrypting the link](federation.md#encrypting-the-link-tls-pinning)).

### Authenticated identity — enabling proof enforcement (identity-auth Phase 2/3)

Every TLS node now publishes a signed `sys/identity-proof/{self}` alongside its identity, and
peers **reject** an identity overwrite whose proof doesn't chain to a key they already trust — so
the key-poisoning vector (a forged verifying key injected via `sys/identity`) is closed for any
connected/established peer. Rejections increment `identity_anchor_conflicts` on `/stats`.

**`require_identity_proofs` is `true` by default** (post-360 plan row S; `docs/guide/deprecations.md`
§41). An identity entry this node cannot authenticate is rejected outright rather than tolerated. To
opt out — for a mixed-fleet window, or on a TLS-less development fleet where it is inert anyway:

```toml
require_identity_proofs = false     # or GOSSIP_REQUIRE_IDENTITY_PROOFS=0 (a strict boolean)
```

**What "proof" means here changed in Phase 3b, and it is the difference between safe and not.**
With the flag set, this node accepts the **sealed** record `sys/identity-signed/{node}` — key
history and proof in **one** KV entry — and does *not* accept the older
`sys/identity/` + `sys/identity-proof/` pair. The pair is two gossip messages with no ordering
between them, so a peer requiring proofs could learn the identity first, reject it, and hold **no
key** for that node until the proof arrived. One entry cannot arrive in two parts, so the window is
closed by construction rather than by timing. (The 2026-09-23 attempt to default the flag on was
reverted after a test-fleet leader-election failure that was wrongly connected to it: the flag is
inert without TLS and those nodes configure none; the failure was an election with a quorum of one.)

**What the default closes.** With the flag off, an admitted member could garble another node's
sealed record and proof and write a bare `sys/identity/{node}` key list naming its own key — a valid
frame under its own signature — and every receiver merged the key: it then verified forged audit
history for that node and, for a node still on a pre-2.33 certificate, signed live writes as it.

**Before you upgrade into it.** Since 2.33.0 a proof is accepted only in its tagged form, with or
without this flag, so every node you can learn from gossip already runs **2.32.0 or later** and
proves. The default adds only a node from before **v2.3.0** (no proof at all), which `false`
re-admits. Check:

```bash
# One sealed record per node, or set require_identity_proofs = false until you are.
curl -s localhost:PORT/gateway/kv/keys | grep -c 'sys/identity-signed/'
```

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
