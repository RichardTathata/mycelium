# Revocation authority — who may revoke a node's key, and which of its keys verify

**Status:** rev 3, 2026-10-12, **implemented** on PR #616 (post-360 plan row Q, `docs/plans/post-360-hardening.md`).

**History.**
- **Rev 1 (`9209d44e`)** was narrowed by a design review.
- **Rev 2 (`c17fcd0f`)** was the reviewer's minimal sound set.
- **Rev 3** folds in the confirming review's three named changes. It then simplifies one step further, where the
  implementation showed a rev-2 mechanism had nothing left to do (§2.1). That step is called out below so the
  reviewer can check it.

The code is canon:
- `src/agent/revocation.rs` holds the fold;
- `mycelium-core/src/removal.rs` (`KeyStanding`) and `mycelium-core/src/context.rs` (`trusted_keys`) hold what every
  verifier reads;
- `mycelium-core/src/tls.rs` (`cert_identity`, `verify_member_cert`, `generate_node_cert`) handles the certificates.

## 0. What #616 settled, and kept

Every verifier of a node's key reads one answer, `CoreCtx::trusted_keys` / `retain_trusted`:
- the KV `SignedData` arm and the unsigned identity exemption (`connection.rs`);
- identity chaining (`helpers.rs`);
- consensus (`consensus.rs`);
- `known_verifying_keys`, which covers roles, audit and capability authorization;
- caller envelopes (`gateway_caller.rs`);
- knowledge keys (`knowledge_keys.rs`);
- since rev 3, `mycelium-agentfacts` too.

What #616 got wrong was the input to that answer. Any key the reader held for a node could revoke any other, for
ever. Rev 3 replaces that input.

## 1. Standing comes from certificate records, and nothing else

**Certificate record.**
- Stored at `sys/identity-cert/{N}/{key-hex}`. The value is the DER of a certificate that:
  - the fleet CA issued, and that is valid now (`NodeTls::verify_member_cert`, judged at the wall clock through the
    seam);
  - names N by a `mycelium://{N}` URI;
  - has the key its path names as its subject public key info.
- One record per key, so two facts never share an LWW slot.
- It proves itself, so it is exempt from the unsigned-frame refusal, like the sealed identity record. That is how
  first contact reaches a peer that does not hold N's key yet (`connection::certificate_record_proves_itself`).

**Issuance time.**
- `generate_node_cert` writes the issuance time into the certificate's **serial number**, in milliseconds.
- **Why not `notBefore`, as rev 2 said:** `notBefore` is whole seconds, so a start and a rotation in the same second
  would tie. And a certificate dated *now* would be refused at the handshake by a peer whose clock is behind. So
  `notBefore` keeps its fixed 2024 date.
- A certificate whose issuance is later than `wall_now_ms` plus the skew bound gives no standing. It is counted
  (`identity_cert_future` on `/stats`, with a warning naming the node) and re-judged at the next fold.
  - The skew bound is `max_clock_drift_ms`, or 300 s when that is 0.
  - `wall_now_ms` is used rather than `decision_now_ms`, because a peer frame can push the HLC forward without bound.

**Anchored versus legacy.**
- A node with a valid record is **anchored**. Its trusted keys are **exactly its anchored keys that it has not
  revoked**. None of the following verifies for it:
  - a bare key;
  - a poisoned key;
  - a first-sighted key;
  - a v1-only key;
  - a legacy IP-only handshake anchor.
- A node with no valid record is **legacy**. It is judged from `peer_keys` exactly as before row Q, and **no
  revocation of its keys counts** (§4).
- **One root.** A node's anchored keys are ordered by issuance. Two certificates issued in the same millisecond are
  siblings. A self-signed first-sighted genesis gives no standing, and an anchored node does not trust it.
- **Saturation.** S* is the node's newest in-window anchored key overall. A saturated node (§3) trusts S* alone. A
  node with no anchored key cannot be saturated: only an anchored signer makes a revocation count.

## 2. Who may revoke whom

### 2.1 Successor-only, by certificate

A revocation of key P for node N, signed by S, **counts iff** S is an anchored key of N, and either:
- P is not anchored, or
- S's certificate was issued **strictly after** P's.

Authority flows only to the past:
- A thief holding an old key cannot revoke a newer one. This answers #616 review finding 3.
- A key with no standing cannot revoke anything. This answers P1 and P2.
- A node that rotates outranks every older key. `rotate_identity` mints the new key's certificate through
  `generate_rotation` → `generate_node_cert`, now with the URI SAN.
- A CA re-issue outranks every older key too.

**What rev 3 cuts beyond the confirming review: succession links, Proved, next-hop designation and ties.**
- Links existed to give an uncertified successor standing. Nothing in the tree produces one:
  - every rotation mints a certificate;
  - in the secure posture rotation cannot run at all, because `generate_rotation` needs the CA key.
- With every successor certified, two revocations of one key never disagree about a successor. "S is newer than P"
  is a fact of S's and P's certificates, so nothing needs designating or tie-breaking.
- A revocation signed by a key that is itself later revoked still counts against older keys. Authority only flows
  backwards, so this restores nothing and silences nothing new.

### 2.2 The record

A revocation record:
- **Path:** `sys/revocation/{N}/{P-hex}/{S-hex}`, keyed `(P, S)`, so a thief's later write or a tombstone never
  replaces the owner's (critical attack 1).
- **Body:** the **v1 `SignedRevocation` bytes unchanged**, followed by a marker and a trailer. The trailer carries the
  revoker, the certificates of S and of P (when held), and S's signature over the event and all of these, under
  `mycelium.identity/revoke/2`.
- **Self-contained:** the fold judges it from the certificates it carries. Hiding N's certificate records cannot stop
  it counting at a late joiner (P5, attack 4).
- **2.32 nodes:** a 2.32 node decodes the prefix (`serde_fixint::from_slice` ignores trailing bytes, `serde_fixint.rs`)
  and judges it by its old rule.
- **v1 records** at the old path `sys/revocation/{N}/{P-hex}` are judged by §2.1. The signer is whichever anchored key
  of N the record's signature verifies under.
- **Checked before any signature:** the path's N and P, and S for v2, must equal the body's. Otherwise the record is
  dropped.

## 3. The fold, the journal and hiding

**The fold** (`revocation::refresh`):
- **Runs:**
  - at start, after the identity prewarm and after the owner adopts its own revocations;
  - on any change under `sys/identity*` or `sys/revocation/`;
  - and every `max(60 s, health_check_interval_secs)` on the timer seam.
- **Computes**, for each node with records:
  1. its anchored keys, from the store's certificate records, the certificates carried by its revocations, and the
     anchored keys this process has already judged;
  2. then which revocations count;
  3. then its standing, published to `CoreCtx::key_standing` as a whole.
- Nothing is trusted before revocations are judged: a verifier reads only the published standing.
- The fold is single-flight. A call while one is running makes that one run again.
- **Anchored keys this process has judged are kept for its life.** Hiding a certificate record does not demote a node
  in-process.

**Restart, hiding, and the journal:**
- Every revocation this node counts is appended to a node-local journal: `{persistence}/identity/revocations.journal`,
  using `src/agent/journal.rs` (fsynced, with torn-tail repair). The fold reads it at start.
- So a revocation hidden while the node was down still counts after its restart. The records carry their
  certificates, so the anchors come back with them.
- The journal is bounded: 64 counted revocations per node and 65 536 in all.
- **When a node's budget is full, that node is saturated and fails closed:** it trusts only S*. The saturation is
  journaled and counted (`revocation_journal_refused`).
- A node without persistence has no journal. Its residual is stated in §6.

**The owner's side.**
- `sys/identity-cert/` and `sys/revocation/` join `SELF_OWNED_SYS_PREFIXES`, so the tripwire counts a peer's write to
  the owner's records.
- The owner republishes its certificate record and its revocations:
  - when it sees them changed under those prefixes;
  - periodically;
  - with a fresh HLC, so it wins LWW.

## 4. Upgrade: re-anchoring, and the window

- **Dev posture.** A development-posture node re-issues its own certificate, with the URI SAN, for its current key at
  every start. `load_or_generate` re-signs with the on-node CA, as it always did. At start the node then adopts its own
  revocations:
  - each v1 record it can show it wrote (signed by a key in its own history, revoking a key in that history, never
    its current key) is re-issued as a v2 record under its current, anchored key;
  - it counts by §2.1 (the revoked key is older, or has no standing).
- **Secure posture.** The operator's required re-issue (`mycelium tls issue`) does the same once the node starts with
  the new certificate.
- **The window, stated.** Until N is re-anchored, no revocation of N's keys counts at an upgraded node, so **keys N
  revoked before the upgrade verify again there**. A 2.32 node still applies its old rule. The guarantee
  `id.revocation_authority` reports `not_configured` until the node runs a URI-SAN certificate issued off-node.
- **Two key sets: live and history** (the final review's finding 1). After N is re-anchored, a key of N that
  never had a URI-SAN certificate (one retired before the upgrade) no longer verifies anything **live** for N: a
  write, a role claim, a caller envelope, a consensus message. **History** verifies under a separate, verify-only
  set — every key held for N minus the keys N revoked (`CoreCtx::history_keys`) — read by audit-chain recovery,
  verification and pruning; knowledge records keep attribution under it, though not present standing. So a
  chain sealed under a pre-upgrade key still verifies and its genesis is never re-sealed. The owner republishes the
  certificate record of every key it holds, not only its current one.

## 5. Scope

**These verifiers honour revocation and standing:** all of §0's. Consensus no longer falls back to the raw
`sys/identity/{signer}` value: a signer with no trusted key is refused.

**Not reached:**
- unsigned `Data` frames, until `kv_require_signed_frames` or row F2;
- anti-entropy entries, which carry no author;
- writes verified before the revocation arrived (detection, not prevention);
- the mTLS handshake, which refuses removed members only. The per-node check, which needs the URI SAN this row
  introduces, is a recorded follow-up.

## 6. Limits, stated where operators read

The following limits are stated in the `rotate_identity_on_compromise` / `revoke_identity_key` docs, in the runbook
(`docs/operations/cert-rotation.md`), in the guarantee report (`id.revocation_authority`) and in the threat model:

- **No protection against a CA holder.**
  - In the development posture `ca-key.pem` sits beside every node's key, and rotation needs it. Any member, or a
    host-level thief, can issue a certificate dated now that outranks every key of any node.
  - The future bound stops "for ever", not "now".
  - CA compromise is the same in any posture.
- **In the secure posture rotation cannot run.** Recovery is re-issue only.
- **Retired keys still verify until revoked.** A rotation alone is hygiene.
- **Unsigned `Data` and anti-entropy are not covered.**
- **A node with a legacy certificate is not protected.** No revocation of its keys counts at upgraded nodes, and its
  keys are judged as before row Q, including TOFU and proofs-off poisoning.
- **A node without persistence** forgets, across a restart, a revocation hidden fleet-wide while it was down, until
  the owner republishes it.

## 7. Cost

- **Parse before verifying.** A path that disagrees with its body is dropped with no signature check.
- **Bounded work per revoked key.**
  - At most 8 records per revoked key are considered: those naming the newest-issued anchored revoker first, then v1
    records.
  - A v1 record is tried against at most 16 anchored keys.
- **Bounded work per node.** At most 16 anchored keys per node, newest issuance first. Records beyond the bounds are
  skipped and counted (`identity_records_over_cap`), and never by key bytes an attacker chooses (attack 8).
- **Records for a key nobody holds.** A revocation of a key with no standing that is not held for the node is judged
  like any other. It costs one CA chain check per carried certificate and at most 8 records per key. That is the
  bound.
- **Not done, and stated:** the fold walks every node with records on each change (coalesced by single-flight). It
  does not recompute only the node that changed. The work is bounded per node by the caps above.

## 8. Test plan, as built

The failing-first runs were on a `git archive` of `e5c2c27e`, with its own target dir and touched sources. Results:

| Test | Status | On `e5c2c27e` |
|---|---|---|
| `a_poisoned_identity_key_neither_revokes_nor_signs_for_an_anchored_member` | fails first | "POISON: a key A wrote into C's identity revoked C's key at B" |
| `a_first_sighted_key_after_a_restart_cannot_revoke_an_anchored_member` | fails first | the first-sighted key was trusted for C |
| `a_fork_from_a_revoked_key_is_not_trusted_after_the_verifier_restarts` | fails first | "RESTART: a key chained through C's revoked key is trusted for C after B restarted" |
| `a_retired_key_cannot_revoke_its_successor_and_rotation_continues` | fails first | "THIEF: a retired key revoked its successor at B" |
| `a_revocation_hidden_while_the_verifier_was_down_still_counts` | fails first | "HIDDEN: after B restarted, the hidden revocation no longer counted" |
| `an_owner_republishes_its_overwritten_certificate_record` | fails first | no certificate record |
| `mycelium-agentfacts` `a_field_signed_by_a_key_written_into_the_raw_identity_does_not_verify` | fails first | "a key written into the raw identity entry signed a verified field" |
| `a_tls_only_node_honours_a_revocation` | **passes** there | #616 had already moved the module under `tls`. The review's finding 7 was missing coverage, which this test now provides. |
| `a_legacy_ip_only_certificate_anchors_nothing` | guard; passes there | certificate records did not exist |
| `a_certificate_issued_in_the_future_outranks_nothing` | guard | uses the new counter, so it does not compile there |

#616's tests stay as regressions. Unit tests cover:
- the v2 layout read by a v1 reader;
- the path-versus-body check;
- `KeyStanding`'s per-node standing.

## 9. The final review's round (named fixes, 2026-10-13)

The final review of `38da0472` found two high defects and four smaller ones; each is fixed and tested (seen failing
on `38da0472`):

1. **History needs its own key set** — §4's "Two key sets" above; `history_keys` for audit recovery, verification
   and pruning; the owner republishes the certificate record of every key it holds.
2. **A re-issue must never un-revoke.** A key ranks by its **first** (lowest) issuance seen, never a later
   re-issue's; a counted (and journaled) revocation stays counted — no later fold un-counts it; and a node starting
   with a key its own records show revoked (a key file restored from a backup, `key_pem` reloading a retired key)
   **refuses to start**, by name. A node keeps the certificate record already stored for its current key rather than
   overwriting it with a re-issue.
3. **The fold's cost is bounded before any check:** a revocation record carrying more than two certificates is
   refused at parse; judged certificates are cached by SHA-256 (at most 4 096, cleared when full); a revoked key must
   be one held for the node, at most 32 revoked keys per node (anchored first) and 8 records per key (those naming an
   already-anchored revoker first, then by the issuance their carried certificate claims) are considered before any
   CA or signature check. Stated bound, tested: a fold over five records carrying 2 000 certificates each takes under
   250 ms (3.6 s before).
4. **A clock stepped back** cannot mint a successor that ranks below its predecessor: a rotation issues at
   `max(wall_now_ms, newest own issuance + 1)` (`generate_rotation_after`).
5. `anchors_seen` and the never-uncounted set merge through papaya `compute`; `adopt_own_revocations` adopts only
   records signed by the node's current or anchored keys, reading its history from its own sealed record (never the
   raw `sys/identity/{self}`); repeated `revoke_key` for one `(P, S)` keeps the first record, and the journal takes one
   entry per `(P, S)` path.
