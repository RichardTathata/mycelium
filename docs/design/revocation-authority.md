# Revocation authority — who may revoke a node's key, and what a revocation means

**Status:** proposed 2026-10-12, for independent design review before implementation. Post-360 plan row Q
(`docs/plans/post-360-hardening.md`), after the adversarial review of #616 (head `e5c2c27e`) found that #616 got
the reach of revocation right and the authority behind it wrong. This record decides the authority. It changes no
code. Code is canon: every claim about today cites the file it was read from.

## 0. What #616 settled, and what it did not

#616 put one question, "which keys are trusted for node N now?", behind one answer
(`CoreCtx::trusted_keys` / `retain_trusted`, `mycelium-core/src/context.rs`). Every verifier of a node's key reads
it: the signed gossip arm and the identity exemption (`mycelium-core/src/connection.rs`), identity chaining
(`src/agent/helpers.rs`), consensus (`src/consensus.rs`), `known_verifying_keys`, caller envelopes
(`src/agent/gateway_caller.rs`) and knowledge keys (`src/agent/knowledge_keys.rs`). The review found that the
wiring is correct. This design keeps it.

What #616 got wrong is the input to that answer, the set of keys N has revoked. Its rule was that any key the
reader holds for N may revoke any other key it holds for N, and the result is never undone
(`src/agent/revocation.rs`). "Held" includes three kinds of key:

- a key accepted on first sight (trust on first use, TOFU), including one re-accepted from the store after a restart;
- an unproven legacy key (accepted with proofs off);
- a key retired long ago.

Revocation is now strong, because it reaches the KV verifier and is permanent. That strength turned each weak key
into a way to silence N for good. The review's findings 1–3 are that one defect seen from three sides. A
revocation must carry no more authority than the key that signed it, and today nothing ranks keys at all.

## 1. Key standing

Every key held for N gets one **standing**. Standing is computed from records anyone can check, never from an LWW
value as such.

| Standing | How a key gets it | May revoke | May introduce a successor |
|---|---|---|---|
| **Anchored** | (a) A CA-signed certificate binds the key to **N's NodeId**, and the certificate is verified against the fleet CA (`ca-cert.pem`). Or (b) this node completed an mTLS handshake with N's address and the peer presented the key (`record_peer_anchor`, `src/agent/helpers.rs`; harvested on the writer path, `mycelium-core/src/tls.rs` `record_anchor`). | yes | yes |
| **Proved** | Reached from an Anchored key by a chain of valid **succession links** (§2.1). | yes | yes |
| **First-sighted** | Reached by succession links from a self-signed, NodeId-bound genesis record. Only while N has **no** Anchored key in the record set. | yes, but only within its own chain (§2) | yes |
| **None** | Learned only from a bare `sys/identity/{N}` value, or from a v1 sealed record whose proof does not bind N (today's format, `helpers::identity_proof_message` signs the history and not the NodeId). Accepted for verification under rollout tolerance, as today. | **no** | **no** |

Standing is the order `Anchored > Proved > First-sighted > None`.

### Decisions

- **The anchor needs a record that binds the NodeId.** Today's certificate binds only the IP address
  (`generate_node_cert`, `mycelium-core/src/tls.rs`: one `SanType::IpAddress`, with fixed dates of 2024-01-01 to 2099-01-01).
  - Nodes on the same host share an IP, so an IP-only certificate published by gossip would let a co-hosted member
    anchor a key for its neighbour.
  - New certificates therefore carry a URI SAN `mycelium://{ip}:{port}` and a `notBefore` equal to their issuance
    time.
  - Each node publishes its certificate DER at `sys/identity-cert/{N}/{key-hex}`, one key per record. Two
    different facts then never share an LWW slot.
  - A legacy IP-only certificate still anchors, but only through a direct handshake to the address the writer
    dialed. That is local knowledge and is never re-gossiped.
- **An anchor is only as strong as the CA key's custody.** When the fleet CA key sits on every node (the auto-CA
  dev posture, which `rotate_identity` needs today: `generate_rotation` calls `load_existing_ca`), anyone who
  compromises a host can mint an Anchored key for any NodeId. Under `secure-single-domain` the CA key is off-node
  (`id.ca_key_off_node`), and Anchored means what it says. This record states that dependency rather than hiding it.
- **An unproven legacy key has no standing.** It still verifies signatures as today, so a proofs-off fleet keeps
  working. It never revokes, never introduces a successor, and any key of N with standing may revoke it. This closes
  review finding 2's proofs-off scenario at its root: the attacker's `ka`, written into the bare `sys/identity/{C}`,
  can revoke nothing.
- **First sight yields to an anchor.** Once N has an Anchored key in the record set, any key of N whose chain is
  rooted only in a first-sighted genesis stops being trusted. Every verifier consults it like a revoked key, but the
  key is not recorded in `RevokedKeys`, so it regains trust if it is ever chained from the anchor. This is the honest
  reading of TOFU: it stands in for an anchor that has not arrived yet, and it ends when the anchor arrives.

### How standing survives a restart

Certificates, links and revocations are all **self-authenticating records**:

- a certificate is checked against the CA;
- a link is checked against its predecessor key and the NodeId;
- a revocation is checked against the revoker's link (§2).

Standing is therefore a pure function of the set of records. A restarted node recomputes from that set and reaches
the same answer, whatever order the records arrive in (§4). The one thing that cannot be recomputed is a record the
node no longer has. §4 deals with that.

## 2. Who may revoke whom

### 2.1 History order is succession, signed

"Older" has to come from signatures, never from an LWW value.

- Today N's history is the order of keys in the current `sys/identity-signed/{N}` record, which LWW overwrites
  (`helpers::encode_identity_history`, `src/agent/mod.rs` `rotate_identity`). The review's restart probe used
  exactly that weakness: a record holding `[ka, k1, k0]` signed by `k0`.
- **Decision:** the history is a set of **succession links**, one record per link, at
  `sys/identity-link/{N}/{new-key-hex}`.
  - Each link carries `{ node: N, predecessor: P, successor: S }` and a signature by P over
    `mycelium.identity/link/1 ‖ N ‖ P ‖ S`.
  - A **genesis** is the same shape with P = S, signed by S itself.
  - Binding N into the signature means a link made for node A can never be presented as node C's. Today's proof
    can be, because it binds only the history.
  - One record per link means no attacker overwrite can rewrite the order. The worst an overwrite or tombstone can
    do is hide a link, which §4 answers.
- **"S is younger than P" means S is reachable from P by valid links.**
- **Forging the order needs the predecessor's private key.** A thief holding k0 can mint links k0 → kX. It cannot
  mint any link into k0's past, and it cannot make kX an ancestor of k1.

### 2.2 The rules

A revocation is a record at `sys/revocation/{N}/{revoked-key-hex}`. It carries
`{ node: N, revoked: P, revoker: S, successor: S }` and a signature by S under the domain
`mycelium.identity/revoke/1`. **It counts iff all of the following hold:**

1. **Successor revokes predecessor.** S is a descendant of P through valid links. A key can revoke only keys in its
   own past. Consequences:
   - `rotate_identity_on_compromise` works: k1 is k0's successor and revokes k0.
   - A thief holding k0 cannot revoke k1, because k1 is not in k0's past. This closes review finding 3.
2. **The revoker stands at least as high as the revoked key:** `standing(S) ≥ standing(P)`. A first-sighted or
   poisoned key never revokes an Anchored one, which closes review finding 2's proofs-on scenario. This rule has one
   carve-out and one consequence:
   - An Anchored key may revoke any **non-anchored** key of N, descendant or not. This is the owner's tool against a
     thief's fork (rule 4).
   - Between two Anchored keys, the one whose certificate has the later CA-signed `notBefore` may revoke the
     earlier. This is how a CA re-issue recovers (§3). Legacy certificates all carry the fixed `notBefore` and are
     ordered by links only.
3. **A revoked key never revokes, and never introduces a successor after its revocation is in force.** Signatures
   carry no trusted time, so "after" is expressed structurally:
   - A revocation of P by S **designates S as P's successor**.
   - It voids every other link signed by P.
   - The revocations of P signed by those voided successors stop counting.
   - The thief's link k0 → kA, and anything kA later signs, stop counting once k1's revocation of k0 is in the
     set. The owner's link k0 → k1 survives, because it is the designated one.
4. **Ties.** Two revocations of the same P can designate different successors: the owner's k1 and the thief's kA,
   both children of k0.
   - The successor with the **strictly higher standing** wins. The loser's link is void and its revocations of P
     do not count.
   - Rotation mints a CA certificate for the new key, so the owner's successor is Anchored. A thief without the CA
     key can only make Proved or First-sighted successors, so the owner wins.
   - **On equal standing, both successors are void and P stays revoked.** This fails closed: N cannot be
     impersonated, but it is silenced until it is re-anchored (§3). It is counted (`identity_succession_conflicts`)
     and logged by name.
   - An equal-standing tie happens only when the thief can mint whatever the owner can, which means the CA key is
     on-node.
5. **The record is internally consistent.** The path's node and key-hex equal `event.node` and `event.revoked`,
   and the signature verifies under the named `revoker` (one check, §6).

**The review's mutual-revocation case.** k1 revokes k0 and k0 revokes k1. By rule 1 only the successor's
revocation counts: k0 is not k1's descendant. k0 is revoked, and k1 is untouched.

**Legacy (v1) revocations** have no `revoker` or `successor` field. They are read as revoker = successor = the key
of N whose signature verifies, and they are judged by the same five rules. A v1 revocation signed by the raw
"current key" of an LWW value has no standing of its own: it counts only if the key that signed it passes rule 1
against the link set.

### 2.3 The review's scenarios under these rules

- **Finding 1 (P1), the restart bypass.** C rotates on compromise: link k0 → k1, and k1 revokes k0, designating k1.
  The thief writes a sealed `[ka, k1, k0]` signed by k0, or in the new format a link k0 → ka.
  - Live: the link k0 → ka is void because k0's designated successor is k1.
  - After B restarts: the same record set gives the same answer (§4). ka is never trusted, and the thief's write
    as C is refused.
  - A v1 sealed record carrying ka gives ka standing None, so even before the links exist ka cannot revoke or
    chain anything. Under `require_identity_proofs` a v1 record that does not bind N stops being accepted as
    first sight once N is anchored (§1).
- **Finding 2, proofs off (P2).** A garbles C's sealed record, writes `sys/identity/{C}` = ka ‖ k0, and revokes k0
  with ka.
  - ka has standing None, so it may not revoke (rule 2), and it is not k0's successor (rule 1).
  - C keeps k0, and C's writes keep arriving.
  - Residual, unchanged from main: with proofs off ka still *verifies* writes signed as C. That is the documented
    identity-poisoning gap. It gains no new power here.
- **Finding 2, proofs on (P3).** A writes a self-signed record `[ka]` for C; B restarts and sees it first; A
  revokes C's anchored k0.
  - ka is First-sighted at best. While C has an Anchored key it is not even that (§1). So ka cannot revoke k0
    (rule 2) and is not k0's successor (rule 1).
  - A's TOFU record for C is distrusted while C's certificate record is present.
- **Finding 3 (P4), a thief holding a retired key.** After `rotate_identity_on_compromise`, the thief revokes k1
  with k0.
  - k1 is not in k0's past (rule 1), and k0 is revoked (rule 3), so the revocation does not count.
  - `rotate_identity` to k2 still works: link k1 → k2, and k1 is not revoked.
  - The same holds for any leaked retired key, however old.

## 3. Recovery

- **After a compromise the owner rotates, and the old key can never silence it.** The rotation publishes link
  k0 → k1 and k1's certificate record, then revokes k0 designating k1. From then on k0 can sign nothing that
  counts, including a successor, a revocation or a fork (rules 1 and 3). If the thief forked first, the owner's
  Anchored k1 beats the thief's non-anchored kA (rule 4), or revokes it directly (rule 2's carve-out).
- **A CA re-issue recovers a node whose key is lost or contested.**
  - The operator issues a new certificate (`mycelium tls issue`, `issue_node_cert` in `mycelium-core/src/tls.rs`).
    It carries the URI SAN and a `notBefore` later than every earlier certificate for N.
  - The new key k2 is Anchored. It may revoke every non-anchored key of N, and every Anchored key with an older
    `notBefore` (rule 2).
  - No link from the old chain is needed. This is the recovery when the owner never held the successor's
    predecessor key, for example a node restored from its certificate alone under `ca_key_off_node`, where
    `rotate_identity` cannot run because `generate_rotation` needs the CA key.
- **Still requires member removal and a fresh identity:**
  - **A thief holding the CA key.** It can out-issue the owner. Removal (`docs/design/member-removal.md`) is
    signed by the membership authority, not the CA, so it still works.
  - **An Anchored-versus-Anchored tie** where neither certificate is newer (two legacy certificates).
  - A fleet that runs **without a CA**, with TLS off. There is nothing to revoke there.

## 4. Restart

The choice is between two approaches:

- **(a)** Recompute from the store in a fixed point (identities → revocations → re-validate).
- **(b)** Persist `RevokedKeys`.

**Decision: neither alone.** Recompute from the record set, and journal the record set.

- **The computation is a fold over records, not over arrival order.** At start and on every change, a node takes
  the set of valid records it has for N and computes standing, the designated successors and the revoked set,
  stratified so that no cycle exists:
  1. Anchored keys come from certificate records and journaled handshake anchors. Revocations play no part here.
  2. Link reachability and standing come from the links, ignoring revocation.
  3. Revocations are judged by §2's rules against strata 1–2.
  4. Voided links and the losing successors in ties are removed. Trust follows.

  Revocations only point to the past (rule 1), and ties are broken by standing computed before any revocation, so
  this is one pass and not an iteration. **A stored identity record is never trusted before the revocations are
  judged**, because trust is computed after stratum 3, never merged eagerly. This is what removes review finding 1:
  today's `prewarm_peer_keys` merges first and refreshes after (`src/agent/lifecycle.rs`).
- **Why not persist `RevokedKeys` alone.** The revoked set is a *derived* fact. Persisting it would freeze a
  decision without the records that justify it. A later certificate, such as a CA re-issue, has to be able to
  change the outcome (§3), so the inputs must be kept, not just the output.
- **Why journal the records.** LWW lets a member tombstone or overwrite a record while a node is down. A derivation
  from the store alone would then forget a revocation it once honoured.
  - Each node appends every record it has validated (certificate, link, revocation, handshake anchor with the DER
    the peer presented) to a node-local journal: `src/agent/journal.rs` `Journal`, append-only, with torn-tail
    repair.
  - At start it folds the journal together with the store.
  - Per-record namespaces (§1, §2.1) mean an overwrite can only hide a record, never substitute a different fact
    under its name. The journal means a hidden record is not forgotten.
  - A node without persistence recomputes from what the store and gossip deliver. Its residual is stated: a record
    tombstoned fleet-wide while it was down is lost to it, and the owner republishes (each node republishes its own
    certificate and links at start).
- **Anchors trigger a refresh.** `record_peer_anchor` (`src/agent/helpers.rs`, reached from the sink installed in
  `src/agent/lifecycle.rs`) journals the anchor and recomputes N. Handshake anchors are re-harvested on reconnect,
  and the journal carries them across the restart in between (review finding 5).
- **`RevokedKeys` stays as the in-memory cache** that every verifier reads (`mycelium-core/src/removal.rs`). It is
  rebuilt for N by each recomputation and replaced for N as a whole: voided links and tie outcomes have to follow
  the record set. The no-un-revocation guarantee moves from the cache to the journal. A revocation once validated
  never leaves the record set, and no rule ever makes a counted revocation stop counting, because a record that is
  added can only add revocations or void younger links. So no revoked key is ever restored.

## 5. Scope, stated

**These verifiers honour revocation**, all through `retain_trusted`:

- the signed gossip arm and the unsigned identity exemption (`mycelium-core/src/connection.rs`);
- identity chaining, which is replaced by the fold (§4);
- consensus (`src/consensus.rs`);
- `known_verifying_keys`, which covers role claims, the audit chain and capability authorization;
- caller envelopes (`src/agent/gateway_caller.rs`);
- knowledge member keys, and through them member-removal authority (`src/agent/knowledge_keys.rs`).

The module is compiled under `tls`, and issuing a revocation needs `compliance`.

**These do not honour it, and the PR states each:**

- **`mycelium-agentfacts`** verifies against the raw `sys/identity/{node}` value (`mycelium-agentfacts/src/crdt.rs`
  `peer_identity_keys`). That is unfiltered *and* poisonable. **Fix in this row:** the root crate exposes
  `GossipAgent::trusted_identity_keys(&NodeId)` (the same answer), and agentfacts reads it.
- **Unsigned `Data` frames** are applied, with no key to revoke, unless `kv_require_signed_frames` is set. Making
  that refusal unconditional is row F2's job.
- **Anti-entropy entries carry no author** (`docs/threat-model.md`). A revoked key's earlier writes, or a forged
  write, arrive that way without any signature check. Revocation cannot reach what is not signed.
- **The mTLS handshake** refuses removed members only (`refuse_if_removed`, `mycelium-core/src/tls.rs`). A
  certificate whose key N has revoked still completes the handshake. **Decision: not in this row.** With the
  certificate's URI SAN the check becomes possible (`revoked_keys.is_revoked(san_node, key)`), and it is recorded as
  the follow-up. Refusing on the key alone would let a member that claimed someone's key and revoked it refuse that
  key's owner at the handshake.
- **Consensus's fallback** to the raw `sys/identity/{N}` value when `peer_keys` holds nothing (`src/consensus.rs`
  `decode_verify`) is pre-existing. **Decision: removed in this row.** An unknown signer is refused, as in the KV arm.
- **Writes verified before the revocation arrived stay applied.** This is detection, not prevention.

## 6. Cost

`refresh` in #616 rescans every revocation record against every held key on every identity or revocation event
(`src/agent/revocation.rs`). That is O(R·H) per event, R and H are attacker-inflatable, and the path's key-hex is
never compared with the event. **Decisions:**

- **Parse before verifying.** Drop any record whose path node or key-hex differs from its body (rule 5). This costs
  nothing.
- **Name the signer.** A v2 record names its `revoker` or `predecessor`. Verification is one signature check, and
  only if the named key is held for N with standing. An attacker who holds none of N's keys pays O(1) per record
  and never reaches a signature check. A v1 record is tried only against N's keys with standing, which are bounded
  below.
- **Recompute per node, not for the whole fleet.** A change under `sys/identity-cert/{N}/`,
  `sys/identity-link/{N}/` or `sys/revocation/{N}/` recomputes N only. Events within one tick are coalesced through
  the timer seam, so a burst costs one fold per node.
- **Bound the per-node record set.** At most `MAX_IDENTITY_RECORDS_PER_NODE` (256) links and as many revocations
  are considered per node, in key order. Beyond that, records are refused and counted (`identity_records_over_cap`).
  A node legitimately rotating 256 times is not a case to design for. A flood of junk links for N from an attacker
  fails rule 5 or the signer check before it counts toward the cap.

## 7. Test plan

Each probe is written as a test that is **seen failing on `e5c2c27e`** before the fix. Multi-node tests run 12
times. None needs `compliance` unless it issues a revocation through the agent API. The tls-only test signs the
records directly.

| Test | What it shows |
|---|---|
| P1 `a_fork_from_a_revoked_key_is_not_trusted_after_the_verifier_restarts` | Review finding 1. Restart B after the thief's k0-signed record; the thief's write as C is refused, and ka is not in C's trusted set. |
| P2 `an_unproven_key_cannot_revoke_a_members_key` | Proofs off. A's bare `ka ‖ k0` plus a revocation signed by ka leave k0 trusted, and C's writes still arrive. |
| P3 `a_first_sighted_key_cannot_revoke_an_anchored_one` | Proofs on, B restarted. A's self-signed `[ka]` for C neither revokes k0 nor is trusted while C's certificate record is present. |
| P4 `a_retired_key_cannot_revoke_its_successor` | After `rotate_identity_on_compromise`, a k0-signed revocation of k1 does not count, and `rotate_identity` to k2 then succeeds. |
| P5 `a_revocation_survives_a_restart_with_its_record_tombstoned` | B journals the revocation; the record is tombstoned while B is down; after restart k0 is still revoked. |
| `only_the_successors_revocation_counts_in_a_mutual_revocation` | k0 and k1 revoke each other; k0 is revoked and k1 is trusted. |
| `the_higher_standing_successor_wins_a_fork` | Anchored k1 beats Proved kA. On equal standing both are void, the tie is counted and N is not impersonable. |
| `a_ca_reissue_outranks_every_older_key` | A new Anchored key revokes a contested chain and the node's writes verify again. |
| `a_tls_only_node_honours_a_revocation` | `--features tls` without `compliance`. A received revocation refuses a KV write signed by the revoked key. |
| `agentfacts_verifies_against_the_trusted_set` | A field signed by a revoked key does not verify through `mycelium-agentfacts`. |
| `junk_revocation_records_cost_one_parse_each` | Records with a mismatched path and body, or an unheld signer, are dropped before any signature check (counted). Over-cap records are refused. |
| `a_link_for_one_node_is_not_a_link_for_another` | A link signed for A, presented under C's path, is refused (NodeId binding). |

The seven tests already in #616 stay, as regressions. Two of them are expected to change shape, and the PR will say so:

- `a_member_cannot_revoke_another_members_key_by_rewriting_its_identity` is subsumed by P2 and P3.
- `a_key_one_member_claims_and_revokes_stays_trusted_for_its_owner` stays as the per-node check.

## 8. What this changes on the wire and for adopters

- **Wire v12 is unchanged.** The new records are KV values under three `sys/` prefixes:
  - `sys/identity-cert/` and `sys/identity-link/` are new;
  - `sys/revocation/` gains the v2 body.

  An older node ignores the new prefixes and reads a v2 revocation with its old rule. It gets the old, weaker
  answer. That is a mixed-fleet window, written up in `deprecations.md` §39 in the 2.30.0 shape.
- **New certificates carry the URI SAN and an issuance-time `notBefore`.** An existing certificate keeps working
  and anchors by handshake only, until it is re-issued.
- The revocation API is unchanged. `revoke_identity_key` now writes a v2 record naming its successor, which is the
  current key, and refuses a key that is not in the current key's past.

## 9. Open for the design review

1. Should the first-sighted-yields-to-anchor rule (§1) also apply when the anchor is only a journaled handshake
   (local knowledge) rather than a gossiped certificate? Proposed: yes. The local node's own observation is the
   strongest evidence it has.
2. The equal-standing tie (rule 4) fails closed. The alternative, keeping both successors trusted, fails open to
   impersonation. Proposed: closed, with the counter.
3. Whether the handshake refusal (§5) should be pulled into this row once the URI SAN exists. Proposed: follow-up,
   to keep the row's surface reviewable.
