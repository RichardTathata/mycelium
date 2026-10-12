# Revocation authority: who may revoke a node's key, and what a revocation means

**Status:** rev 2, 2026-10-12. This is the reviewer's minimal sound set, with every fix from the design review of
rev 1 (`9209d44e`) folded in. It is for a confirming design review before implementation. It belongs to post-360
plan row Q (`docs/plans/post-360-hardening.md`) and follows #616's adversarial review of head `e5c2c27e`. It changes
no code. Code is canon: every claim about today cites the file it was read from.

**What rev 2 drops from rev 1:**
- First-sighted as a standing.
- "An anchored key may revoke any non-anchored key".
- Designation of the final successor.
- One revocation slot per revoked key.
- An unbounded journal of every record.

**What rev 2 adds:**
- Next-hop designation.
- Revocations keyed `(P, S)` and self-contained.
- A bounded `notBefore`.
- The legacy rules for a fleet without URI-SAN certificates.
- A mixed-fleet encoding that 2.32 nodes still honour.

## 0. Settled by #616, and kept

Every verifier of a node's key reads one answer, `CoreCtx::trusted_keys` / `retain_trusted`
(`mycelium-core/src/context.rs`). The verifiers are:
- the signed gossip arm and the identity exemption (`mycelium-core/src/connection.rs`);
- identity chaining (`src/agent/helpers.rs`);
- consensus (`src/consensus.rs`);
- `known_verifying_keys`;
- caller envelopes (`src/agent/gateway_caller.rs`);
- knowledge keys (`src/agent/knowledge_keys.rs`).

The review found this wiring correct. This record replaces what feeds `CoreCtx::revoked_keys`. That is #616's rule,
under which any key the reader holds for N revokes any other, for ever (`src/agent/revocation.rs`). It also replaces
how stored identities are merged at start (`src/agent/lifecycle.rs` `prewarm_peer_keys`).

Definitions used throughout:
- **N** is a node, identified by its NodeId.
- **P** is the predecessor, the key being revoked.
- **S** is the successor, the key signing the revocation.

## 1. Key standing: from certificate records only

| Standing | Source | Authority |
|---|---|---|
| **Anchored** | A record at `sys/identity-cert/{N}/{key-hex}` holding a certificate DER. It verifies against the fleet CA (`ca-cert.pem`), carries a URI SAN `mycelium://{ip}:{port}` equal to N, has this key as its SPKI, and has `notBefore ≤ decision_now_ms + SKEW`. | May revoke and may introduce a successor (§2). |
| **Proved** | Reached from an Anchored key of N by succession links (§2.1) that the fold has not voided (§3). | Same as Anchored, ranked below it. |
| **None** | Every other key the reader holds for N. That covers trust on first use (TOFU), a v1 sealed record without a certificate, a bare `sys/identity/{N}` value under proofs off, and a key anchored only by a handshake with a **legacy IP-only certificate**. | **Verification only**, exactly as today. Its rights to revoke are limited by §2.2: it can revoke only within its own legacy succession and can never revoke an Anchored or Proved key. |

**Decisions:**

- **URI-SAN certificate records are the only source of standing.**
  - Today's certificates bind only the IP (`generate_node_cert`, `mycelium-core/src/tls.rs`: one
    `SanType::IpAddress`, with the fixed dates 2024-01-01 to 2099-01-01).
  - Nodes on one host share that IP, so an IP-only certificate cannot name a node. A member that squats a
    neighbour's port while it is down presents a valid certificate for the shared IP (review attack 5).
  - So a legacy handshake anchor (`record_peer_anchor`, `src/agent/helpers.rs`) gives the key **verification only**.
  - New certificates (`generate_node_cert`, `issue_node_cert`) carry the URI SAN and a `notBefore` equal to their
    issuance time.
- **`notBefore` is bounded, so time cannot be forged forward** (review attack 2).
  - A certificate with `notBefore > decision_now_ms + SKEW` is ignored. It is not refused for ever: it is re-judged
    on the next fold.
  - `decision_now_ms` is `Hlc::decision_now_ms` (`mycelium-core/src/hlc.rs`). `SKEW` is `max_clock_drift_ms`, or
    300 s when that is 0.
  - A certificate dated a century ahead therefore outranks nothing.
  - What the bound cannot stop is stated in §7: a CA holder minting a certificate dated *now*.
- **Two genesis roots for one NodeId** (review gap 10):
  - **Several Anchored certificates for N are ordered by `notBefore`.**
    - A later one is a **certificate successor** of every earlier one (§2.1).
    - Equal `notBefore` makes siblings. Siblings are both trusted, and neither is the other's successor.
  - **Self-signed (TOFU) geneses for N are honoured only while N has no URI-SAN certificate record.**
    - They have standing None, under today's rule: the first seen establishes N, and later ones must chain.
    - Once N has a certificate record, a key rooted only in a self-signed genesis is not trusted.
    - This closes review scenario P3's impersonation as well as its revocation: A's self-signed `[ka]` for C is
      ignored while C's certificate record is held.
    - A fleet without URI-SAN certificates keeps today's TOFU, which is stated in §7.

## 2. Who may revoke whom

### 2.1 Succession

**Succession links.** A link is a record at `sys/identity-link/{N}/{S-hex}` with body `{node: N, pred: P, succ: S}`.
It is signed by P over `mycelium.identity/link/1 ‖ N ‖ P ‖ S`, so it is bound to N and can never be replayed as
another node's.

**Legacy links.** A v1 sealed record's proof is a signature by P over a history that starts with S
(`helpers::identity_proof_message`, rotation in `src/agent/mod.rs` `rotate_identity`). It is read as a legacy link
P → S, used **only for N**: it counts only when it sits at N's own path (`sys/identity-signed/{N}`) or is carried
inside a revocation for N. This is what keeps revocations made before the upgrade counting (§6).

**Certificate succession.** Between two Anchored keys of N, the one whose certificate has the later `notBefore` is a
successor of the other. This is how a CA re-issue recovers a node (§5) without the old key's signature.

**What "S descends from P" means.** S is reachable from P by links, legacy links or certificate succession.
Forging a hop needs P's private key, or the CA key for a certificate hop. Nothing is read from an LWW value's order.

### 2.2 The revocation rule

A revocation of P by S, for N, counts iff **either**:

- **(a) Successor revokes predecessor.** S descends from P through a path that is not void, and
  `standing(S) ≥ standing(P)`.
- **(b) Cleanup of a key with no standing.** `standing(P) = None` and `standing(S)` is Anchored or Proved.

In both cases S must not itself be revoked, and the record must be internally consistent (§2.3, §8).

**Reconciling (a) with P1.** Rule (b) is the review's P1 fix: a key that is not P's descendant may revoke a
None-standing key only if it is Anchored or Proved. A free self-signed genesis has standing None and is not C's
descendant, so it revokes nothing. Rule (a) still lets a None-standing key revoke its own None-standing predecessor.
That case needs P's own signature on the hop, and it is what a legacy fleet's `rotate_identity_on_compromise` relies
on (§6).

**Next-hop designation** (review attack 3).
- A counted revocation of P by S **designates the next hop**: the first key after P on the path from P to S. On
  k0 → k1 → k2 where k2 revokes k0, the designated key is k1, not k2.
- Every other hop out of P is void, and so is everything that descends only through a void hop.
- So k0 → k1 survives a multi-hop revocation, and a thief's fork k0 → kA dies.

**Ties.** Two counted revocations of one P can designate different next hops, for example the owner's k1 and a
thief's kA, both children of k0. Then:
- The next hop whose subtree contains the signer with **strictly higher standing** wins. The others are void.
- **On equal standing, every designated hop is void and P stays revoked.** This fails closed: N cannot be
  impersonated through either branch.
- A tie is **transient** while either signer's certificate could still arrive (review attack 7).
  - The fold still fails closed during a tie.
  - The alarm (`identity_succession_conflicts`, a warning naming N) is raised only after the tie has persisted for
    a grace period: twice `health_check_interval_secs`, and at least 60 s.
  - Republishing (§4) usually ends a tie that hiding caused before the grace runs out.

**The review's mutual-revocation case.** k0 and k1 revoke each other. Only k1 descends from k0, so only k1's
revocation counts. k0 is revoked, and k1 is designated and trusted.

### 2.3 The revocation record: keyed `(P, S)`, self-contained

- **Path.** `sys/revocation/{N}/{P-hex}/{S-hex}`. Two revocations of one key never share a slot (critical review
  attack 1): a thief's later write, or a tombstone of the owner's record, cannot replace the owner's revocation
  everywhere.
- **Body.** The v1 bytes (§6), then a trailer carrying:
  - the **path** from P to S, as v2 links, legacy proofs, or certificate hops;
  - the **certificates** that establish the standing of S and P (Anchored, or the Anchored root of a Proved path);
  - a v2 signature by S over `mycelium.identity/revoke/2 ‖ N ‖ P ‖ S ‖ sha256(path ‖ certificates)`.
- **Judged from its own contents.** It is judged against the fold's view of N with no separate record needed. A
  member who hides N's link or certificate records (review attack 4) cannot stop the revocation from being judged
  at a late joiner. That answers P5's late-joiner gap.

## 3. The fold

At start, and whenever a record for N changes, the node computes N's trust in one stratified pass. Nothing is
trusted before revocations are judged.

1. **Anchored keys** come from valid, in-window URI-SAN certificate records, both stored ones and those carried in
   revocations. Revocations play no part here.
2. **Descent and standing** come from links, legacy links and certificate succession, ignoring revocation. Proved
   is computed here.
3. **Revocations** are judged by §2.2 against strata 1 and 2. They come from the store, and from the journal (§4)
   for those this node has counted before.
4. **Designation.** Void hops and tie losers are removed, along with whatever descends only through them.
5. **Trust.** The trusted keys of N are every key held for N that is not revoked, not void, and not a TOFU-rooted
   key of a certificate-anchored N. That set is published to `CoreCtx::revoked_keys` for N.

Revocations point only to the past (rule (a)), and ties are broken by standing fixed in stratum 2, so there is no
cycle.

**A record of any version signed by a revoked or void key contributes nothing** (review P2). That covers a v1 sealed
record, a v2 link, a v2 or v1 revocation, and a legacy proof. It applies even when the record was signed before the
revocation, because signatures carry no time. The owner's own pre-revocation history is the one exception that
matters, and it does not need an exception. The designated hop P → next is carried inside the counted revocation's
path, so the successor is reached through the revocation itself, not through P's other records.

**The guarantee, stated so it is true** (review finding 9):
- A key that a counted revocation names as P is never trusted again by this node. That holds as long as the
  revocation stays countable: it is in the store, or in the journal, or the journal is saturated for N (§4).
- A key **voided** as a losing branch can regain trust when a stronger-standing revocation of the same P arrives
  and designates its branch. That is a tie being resolved, not a revocation undone.
- A key that was never revoked can lose trust when a certificate record for N arrives (TOFU yields to the anchor),
  or when a revocation arrives.

## 4. Hiding, republishing and the journal

**The tripwire** (review attack 4). `sys/identity-cert/`, `sys/identity-link/` and `sys/revocation/` join
`SELF_OWNED_SYS_PREFIXES` (`mycelium-core/src/connection.rs`). A peer's write that changes a record under the
owner's NodeId is counted, and the owner is woken.

**The owner republishes:**
- its certificate record, its links and its counted revocations;
- at start, every `max(60 s, health_check_interval_secs)` on the timer seam, and at once when the tripwire fires
  for one of those prefixes.

A republished record carries a fresh HLC, so it wins LWW over a hiding write.

**A bounded journal of counted revocations only** (review attack 6).
- **What it holds.** Each revocation this node has counted, as its self-contained bytes, appended to a node-local
  journal (`src/agent/journal.rs` `Journal`: append-only, with torn-tail repair). Nothing else is journaled.
  Certificates and links are re-derived from the store, and the owner republishes them.
- **At start.** The journal joins stratum 3, so a revocation hidden while the node was down still counts.
- **Bounds.** 64 counted revocations per N, and 65 536 in total.
- **When N's budget is full, fail closed.**
  - The node journals one `Saturated(N, S*)` entry, where S* is the newest Anchored key of N by `notBefore` among
    the counted revocations' signers.
  - While N is saturated, the fold trusts only N's Anchored keys whose `notBefore` is at or after S*'s, and their
    Proved descendants.
  - Only an extraordinary number of N's own key changes, or a thief, fills N's budget. Either way the safe answer
    is to trust N's newest anchor only.
- **When the total budget is full**, the same rule applies to every N with a revocation that cannot be journaled.
  That node logs, raises a `revocation_journal_full` counter, and reports it in the guarantee report.
- **Without persistence.** A node without persistence has no journal. Its residual is stated in §7: what was
  hidden fleet-wide while it was down is re-learned when the owner republishes.

## 5. Recovery

- **The owner rotates, and the old key can never silence it.**
  - `rotate_identity_on_compromise` publishes link k0 → k1 and k1's certificate record, then a revocation of k0 by
    k1 that carries both.
  - k0 is not a descendant of k1, so nothing k0 signs can revoke k1 (rule (a)). Every record k0 signs contributes
    nothing (§3), including a fork, a link or a revocation.
  - A thief's earlier fork kA loses the tie, because k1 is Anchored and kA is at most Proved, unless the thief holds
    the CA key (§7).
  - Review finding 3 is answered: rotating on to k2 works (k1 → k2).
- **Re-issue.** The operator issues a certificate with a later `notBefore` (`mycelium tls issue`,
  `issue_node_cert`). The new key is a certificate successor of every older Anchored key of N, so it can revoke
  them, and through rule (b) every None key of N.
- **In the secure posture, re-issue is the only recovery.** `rotate_identity` needs the CA key
  (`generate_rotation` → `load_existing_ca`, `mycelium-core/src/tls.rs`), which `id.ca_key_off_node` keeps off the
  node. In that posture links come only from a dev-posture history or from a thief. A thief's links give at most
  Proved, which the re-issued Anchored key outranks.
- **Recovery still needs member removal and a fresh identity:**
  - when a CA holder is the adversary (§7);
  - on an Anchored-versus-Anchored tie with equal `notBefore`;
  - in a fleet whose certificates were never re-issued with the URI SAN, where re-issue *is* the fix.

  Removal (`docs/design/member-removal.md`) is signed by the membership authority, not the CA.

## 6. Mixed fleet and upgrade

**v1 decoders still honour v2 records.**
- A v2 revocation's bytes are the **v1 `SignedRevocation` unchanged**: `RevocationEvent { node_id, hlc,
  revoked_key: P, reason }` and S's v1 signature over it. The trailer of §2.3 follows, behind a magic marker.
- `serde_fixint::from_slice` (`mycelium-core/src/serde_fixint.rs`) does not check for trailing bytes, so a 2.32
  node decodes the prefix and judges it by its old rule.
- Its old rule scans `sys/revocation/{N}/` by prefix (`revocation_nodes`, `src/agent/revocation.rs`), so the
  `(P, S)` path is found. An owner that rotated has S as its raw current key, so a 2.32 node honours the owner's
  revocation as before.
- No v2 field is ever inserted before the signature. The review's finding was that appending fields to
  `RevocationEvent` broke the v1 decode.

**Revocations made before the upgrade keep counting.**
- A pre-upgrade revocation lives at the old path `sys/revocation/{N}/{P}` with a v1 body. An upgraded node judges it
  by §2.2, with S as the signer whose v1 signature verifies.
- S descends from P through the **legacy links** derived from N's v1 sealed records (§2.1). In particular,
  `rotate_identity_on_compromise`'s rotation record `[k1, k0]` signed by k0 gives the legacy link k0 → k1.
- So a previously revoked key does not regain trust on upgrade.
- If the rotation record has since been overwritten (a thief's later sealed record), the legacy link is lost.
  **On upgrade the owner re-publishes each of its v1 revocations as a v2 `(P, S)` record** carrying the legacy proof
  from its own persisted store or journal.
- An upgraded verifier that counted a v1 revocation journals it (§4), so the count does not depend on the old slot
  surviving.

**Re-issuing certificates is an upgrade step.**
- Until N has a URI-SAN certificate record, every key of N has standing None.
- N's revocations are then judged by rule (a) among None keys, which is legacy succession with next-hop designation.
- A fork from a stolen key becomes an equal-standing tie and fails closed, so N is silenced, not impersonated.
- The upgrade notes (`deprecations.md` §39) **require** re-issuing every node's certificate with the URI SAN. They
  state that a node with a legacy certificate is not protected against that silencing.

## 7. Scope and limits, stated where operators read

The implementation PR states each of these in the `rotate_identity_on_compromise` and `revoke_identity_key` docs
(`src/agent/mod.rs`), in the runbook (`docs/operations/cert-rotation.md` §Compromise remediation), in the guarantee
report (a guarantee `id.revocation_authority`, reported `not_configured` when the CA key is on the node or when any
known node lacks a URI-SAN certificate), and in the threat model (`docs/threat-model.md`):

- **No protection against a CA holder.**
  - In the dev posture the CA key sits beside the node key (`ca-key.pem`, `mycelium-core/src/tls.rs`), and rotation
    needs it.
  - A host-level thief, or any member with the CA key, mints an Anchored key dated *now* for any node and wins every
    tie.
  - The `notBefore` bound prevents only "for ever".
  - A CA compromise is the same in any posture.
- **In the secure posture rotation cannot run.** Recovery is re-issue only (§5).
- **Retired keys still verify until revoked.** A rotation without a revocation is hygiene, not containment
  (`docs/operations/cert-rotation.md` §2).
- **What revocation does not reach:**
  - unsigned `Data` frames, which are applied unless `kv_require_signed_frames` is set (unconditional refusal is row
    F2);
  - anti-entropy entries, which carry no author (`docs/threat-model.md`);
  - writes verified before the revocation arrived (detection, not prevention);
  - the mTLS handshake, which refuses removed members only (`refuse_if_removed`). The per-node handshake check
    needs the URI SAN and is a recorded follow-up.
- **A node with a legacy certificate is not protected:**
  - it has no standing, so a thief's fork silences it (fails closed);
  - TOFU and proofs-off poisoning remain for it, as today.
- **Fixed in this row:**
  - `mycelium-agentfacts` verified against the raw `sys/identity/{node}` value (`mycelium-agentfacts/src/crdt.rs`
    `peer_identity_keys`); it moves to `GossipAgent::trusted_identity_keys(&NodeId)`;
  - consensus's fallback to the raw value when `peer_keys` is empty (`src/consensus.rs` `decode_verify`) is
    removed.
- **A node without persistence** forgets, across its restart, a revocation hidden fleet-wide while it was down,
  until the owner republishes it.

## 8. Cost

#616's `refresh` rescans every revocation against every held key on each event (`src/agent/revocation.rs`). That
is O(R·H), and both are attacker-inflatable.

- **Parse before verifying.**
  - A record is dropped unless its path's N, P and S equal its body's.
  - A link is dropped unless its path's S equals its body's.
  - A certificate record is dropped unless its path's key equals its SPKI.
  - None of these checks verifies a signature.
- **Name the signer.** A record names its signer (P for a link, S for a revocation). It is verified once, and only
  if that key is held for N. A v1 revocation is tried only against N's keys that descend from its P.
- **Recompute per node.** A change under N's prefixes recomputes N only. Events within one tick are coalesced on the
  timer seam.
- **Bound by policy, never by key hex** (review attack 8). Per N, the fold considers:
  - certificate records newest-first by `notBefore`, at most 16;
  - links reachable from those Anchored keys or from N's TOFU root, at most 64, breadth-first in descent order;
  - per predecessor P, at most 8 revocations, preferring signers by standing and then `notBefore`.

  Records beyond the bounds are skipped and counted (`identity_records_over_cap`). Junk a thief writes under fresh
  key hex is never reachable from an anchor, so it is never considered.

## 9. The review's questions, answered as the reviewer did

1. **Does a legacy handshake anchor grant authority?** No. It grants verification only (§1). An IP-only
   certificate cannot name a node on a shared host.
2. **Should an equal-standing tie fail closed or open?** Closed, with the alarm held back for a grace period, so a
   transient tie (a certificate not yet arrived) is not reported as an attack (§2.2).
3. **Should the handshake refusal be pulled into this row?** No, it is a follow-up. It needs the URI SAN, which this
   row introduces, and the row's surface stays reviewable.

## 10. Test plan

Each test is written against the behaviour and **seen failing on `e5c2c27e`**. Where a record format is new, the
test fails there because the behaviour is absent. Multi-node tests run 12 times. Tests that need no
revocation-issuing API run under `tls` alone.

| Test | Review item |
|---|---|
| `a_self_signed_genesis_cannot_revoke_a_key_without_standing` | P1, rule (b) |
| `a_record_signed_by_a_revoked_key_contributes_nothing_live_and_after_restart` (a v1 sealed `[ka, k1, k0]` signed by k0) | finding 1, P2 |
| `a_far_future_certificate_outranks_nothing` | attack 2 |
| `a_retired_key_cannot_revoke_its_successor_and_rotation_continues` (with a legacy certificate, and with a URI-SAN one) | finding 3, P4 |
| `a_late_joiner_judges_a_revocation_whose_links_and_certificate_are_hidden` | P5, attack 4 |
| `a_thiefs_revocation_does_not_replace_the_owners` (two `(P, S)` records for one P; the thief writes last) | critical attack 1 |
| `a_multi_hop_revocation_designates_the_next_hop` (k0 → k1 → k2; k2 revokes k0; k1 stays trusted) | attack 3 |
| `a_hidden_certificate_is_republished_and_the_tripwire_counts_it` | attack 4 |
| `a_legacy_handshake_anchor_cannot_revoke` (two nodes on 127.0.0.1) | attack 5 |
| `a_saturated_journal_trusts_only_the_newest_anchor` | attack 6 |
| `a_transient_tie_fails_closed_without_an_alarm_and_a_persistent_one_is_counted` | attack 7 |
| `the_cap_is_not_steered_by_key_hex` | attack 8 |
| `a_voided_branch_is_restored_by_a_stronger_revocation_and_a_revoked_key_never_is` | finding 9 |
| `a_certificate_anchored_node_ignores_a_self_signed_genesis` | gap 10, P3 |
| `only_the_successors_revocation_counts_in_a_mutual_revocation` | rule (a) |
| `a_v2_revocation_decodes_and_counts_at_a_v1_reader` (decode with the 2.32 struct; judge by the old rule) | mixed fleet |
| `a_pre_upgrade_revocation_still_counts_after_upgrade` (v1 record at the old path; legacy link from the rotation record) | mixed fleet |
| `a_tls_only_node_honours_a_revocation` | #616 review finding 7 |
| `agentfacts_verifies_against_the_trusted_set` | scope |
| `consensus_refuses_a_signer_it_holds_no_keys_for` | scope |
| `junk_records_cost_one_parse_each` | cost |

#616's seven tests stay as regressions. `a_member_cannot_revoke_another_members_key_by_rewriting_its_identity` is
kept: under rev 2, A's key has standing None and does not descend from k0.
