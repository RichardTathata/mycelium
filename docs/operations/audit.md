# Mycelium — Audit & Evidence Operations Runbook

Operator-facing guide to the WS2 tamper-evident audit trail (`compliance`
feature). Concept and API: [`docs/guide/09-security.md`](../guide/09-security.md)
§The audit trail in practice. Architecture invariants: `CLAUDE.md` §Audit trail.

Like RBAC, audit signing requires the TLS identity — `compliance = ["gateway",
"tls"]`. A node with no `GossipConfig::tls` cannot seal signed records; under
`compliance` `agent.audit()` returns `InvalidField` and the event is logged, not
written. Configure TLS on every node that must produce evidence. **A node that
attaches an audit sink without `[tls]` refuses to start** (since 2.20.0,
`an_audit_sink_without_tls_refuses_the_start`): records are sealed with the node
identity, so the sink would receive nothing — `start()` returns `InvalidField`
naming `with_audit_sink` and `[tls]`. Configure `[tls]` or remove the sink.

---

## 1. What gets recorded, and where

| Build | Trail key | Signed | Hash-chained |
|---|---|:-:|:-:|
| default (no `compliance`) | `audit/{ts_unix_nanos}/{node}` | ✗ | ✗ |
| `compliance` | `sys/audit/{node}/{seq:016x}` | ✓ | ✓ |

Each `compliance` record carries: the recording `node_id`, a per-node monotonic
`seq` (genesis = 0), an `hlc` timestamp, the `principal` (who caused the event),
an `action` (`Write`/`Read`/`Invoke`/`Admin`), the `target` resource, an
`outcome` (`Success`/`Denied`/`Error`), optional `detail`, and `prev_hash` (the
SHA-256 content hash of the previous record in this node's stream).

**The chain is per-node.** There is no global chain (that would need a
coordinator). Verify each node's stream independently; the cluster trail is the
union of those streams.

**The head survives a restart with `[persistence]`.** At `start()` — after the identity is
loaded and persisted state replayed, before anything can seal — the node folds its own
persisted stream, verified from genesis (or the newest signed checkpoint, §5) under its
retained keys, and resumes sealing after the last record that verifies (`audit::restore_chain_head`).
**Nothing existing is ever sealed over** (post-360 plan row Q): a record that does not verify is kept
untouched and reported — an `error` log naming the node and the seq range, `GET /stats`
`audit_unverified_records` (a count) and `audit_unverified_from_seq`, the guarantee report's
`audit_unverified` — and a seal skips any slot such a record occupies, linking across it by hash (only
the contiguous run of occupied slots after the head is passed; a far-away record is never followed;
the sequence never wraps). Only `{seq:016x}` keys whose record names that seq are read as records;
anything else under the stream is counted (`audit_malformed_keys`) and ignored. A stream reads
**segment by segment** (`GossipAgent::audit_segments`, `GET /gateway/audit` `segments`): the verified
prefix and the tail linked to it verify; the planted records do not. `audit_prune_to_checkpoint`
prunes only records that verify. Without persistence there
is nothing to recover from and the stream restarts at seq 0 (peers' copies are not consulted). A
stream whose first present record has no anchor — seq 0 deleted by hand with no checkpoint at the
first record — is reported the same way (`audit_verify` names the `SequenceGap`). **A node upgraded
after a rotation on a version before row Q** reports its pre-upgrade records as unverified until its
history is re-anchored; nothing is destroyed. **Rotate-and-revoke before a restart**
([cert-rotation.md](cert-rotation.md) § Compromise remediation): a revoked key verifies nowhere, so
every record it signed reads as unverified after the restart (kept, reported) — seal a checkpoint with
the **new** key and `audit_prune_to_checkpoint()` **before** restarting, so the head anchors at that
checkpoint and the stream verifies.

---

## 2. Querying the trail

```bash
# All streams, verified, most-recent 100 records each:
curl -H 'Authorization: Bearer <audit-token>' \
     'http://NODE:PORT/gateway/audit?limit=100'

# One node's full stream:
curl -H 'Authorization: Bearer <audit-token>' \
     'http://NODE:PORT/gateway/audit?node=10.0.0.7:8080'
```

The endpoint requires the `audit:read` scope (configure a `gateway_scoped_tokens`
entry granting it — see [`rbac.md`](rbac.md) §2). Response shape:

```json
{ "streams": [
  { "node": "10.0.0.7:8080",
    "count": 1422,
    "verified": true,
    "verify_error": null,
    "head_hash": "9f86d081…",
    "records": [ { "seq": 1421, "hlc": 173…, "principal": "10.0.0.3:8080",
                   "action": "Invoke", "target": "orders/place",
                   "outcome": "Success", "detail": "…", "content_hash": "…" } ] } ] }
```

Programmatically: `agent.audit_stream(&node)` (decoded, seq-ordered),
`agent.audit_verify(&node)` (full-stream verification), `agent.audit_stream_nodes()`.

---

## 3. Verifying evidence

`verified: true` means, for that stream: every record's Ed25519 signature checks
against the node's identity key, the sequence is contiguous from genesis, and
every `prev_hash` matches its predecessor's content hash. A `verify_error` names
the **first** offending `seq`:

| `verify_error` | Meaning | Likely cause |
|---|---|---|
| `BadSignature { seq }` | a record's signature failed | the record was edited, or signed by the wrong key |
| `BrokenLink { seq }` | `prev_hash` ≠ predecessor's content hash | a record was edited or removed |
| `SequenceGap { expected, found }` | seq not contiguous | a record is missing or reordered |
| `WrongOwner { seq }` | record claims a different node | a forged or misfiled entry |
| `UnknownSigner` | this node lacks the stream owner's key | owner's `sys/identity/` not yet learned, or unshared CA |

**Any `verified: false` on a stream you did not intentionally prune is a
trust-boundary incident.** Capture the stream, the `verify_error`, and the
offending `seq`; the content hashes let you cite exactly which record changed.

To cite a specific event in an external report, record its `content_hash` — it is
stable for the logical record and changes if any field is altered.

## 3a. A record by its hash (the span's reference)

A span (adopter-surface Y4, [`span-export`](../features/span-export.md)) carries a record's content hash and nothing
of what the record says. The record stays on the node that wrote it, and is read there by that hash, under
`audit:read` (feature `compliance`):

```bash
# The span's mycelium.source says which: mycelium.decision → decision, mycelium.evidence.* → evidence.
curl -H 'Authorization: Bearer <audit-token>' \
     "http://NODE:PORT/gateway/records/evidence/$HASH"
curl -H 'Authorization: Bearer <audit-token>' \
     "http://NODE:PORT/gateway/records/decision/$HASH"
```

`$HASH` is the span's `mycelium.record_sha256`: 64 lowercase hex characters; anything else is `400 bad_hash`. Found
(`200`):

```json
{ "schema": "mycelium.record/1", "sha256": "…", "kind": "evidence", "found": true, "source": "journal",
  "record_b64": "…", "record": { … },
  "verification": { "hash": "matches",
                    "chain": { "state": "referenced", "seq": 41, "linked": true, "reference_content_hash": "…" } } }
```

**Verifying it.** Two checks, the second for evidence only:

1. **The bytes are the record the span named.** Decode `record_b64` and take SHA-256 over it: it is `$HASH`
   (`sha256sum`, or `verification.hash: "matches"`, which the node computed — recompute it rather than trust it).
   `record` is the same bytes parsed, for reading.
2. **An evidence record is the one this node enforced.** `verification.chain` is the reference record in this node's
   signed, hash-linked audit stream whose `journal_sha256` is `$HASH`: `referenced` with its `seq` and `linked` —
   whether it is in the stream's verified segment (§3) — and its `content_hash`, which `GET /gateway/audit` lists;
   `not_referenced` when no reference cites it (the stream pruned to a checkpoint past it, §5). A decision record's
   chain is `not_chained`: the decision trace is best-effort and nothing seals it.

**Where it is read from.** Evidence: the node's evidence journal (`with_evidence_journal`), which loses nothing it
acknowledged. A decision: the node's decision sink (`with_decision_trace`), read a page at a time without waiting for
its lock (`503 sink_busy` if every attempt found it held), then the trace the embedder declared it writes as it
drains the sink (`GossipAgent::with_decision_trace_file`, a `decisions.jsonl`). A read of the sink copies at most 64
records per hold of its lock, so a decision point recording during one drops its record as contended, counted as any
contention is.

**Not found (`404`).** `not_found` is the one meaning the node can give, else `undetermined` with `cannot_rule_out`:

| `not_found` / `cannot_rule_out` | Meaning |
|---|---|
| `not_written` | the node keeps no record of that kind (no journal; no sink and no declared trace) — or, in `cannot_rule_out`, the sink has dropped records (`searched.sink.dropped`, by reason), and a dropped decision was never kept anywhere |
| `evicted` | the record may have been kept and since let go: decisions the sink kept that this read did not search (`searched.sink.not_searched` — drained, or kept after the read) and the declared trace did not hold; evidence a journal cut away when it was opened (`searched.journal.truncated_at_open`) |
| `never_existed` | no record with this hash was kept here — alone only when nothing else is possible. For a decision it covers **this incarnation**: the sink lives in memory, and a record an earlier incarnation never wrote to the declared trace is not known. Each node's journal is its own: a hash from another node's span is `never_existed` here |

---

## 4. Export to an external SIEM / WORM (SOC 2 WS-C)

Attach an `AuditSink` and every sealed record is streamed to your external store on a
background drain task, **off the write path**. The in-cluster hash-chain stays
authoritative; the sink is the long-term / tamper-evident mirror.

```rust
use mycelium::{AuditSink, SignedAuditRecord};

struct SiemSink { /* your client / buffer */ }
impl AuditSink for SiemSink {
    fn export(&self, rec: &SignedAuditRecord) {
        // push rec.encode() to your SIEM/WORM. Runs on the drain task — keep it
        // non-blocking (buffer, or spawn_blocking your own IO).
    }
}

agent.with_audit_sink(std::sync::Arc::new(SiemSink { /* … */ })); // before start(); needs [tls] or start() refuses
```

Semantics: the drain channel is bounded; if your sink can't keep up, the **mirror** copy is
dropped with a `warn!` — never the chain record, which you can always re-export from
`sys/audit/`. So a slow sink degrades to "re-export from the chain", never to lost audit data.

## 5. Retention — checkpoint, export, prune (SOC 2 WS-D)

Audit records are normal replicated KV entries; the trail grows unbounded. Pruning a hash
chain is **not** as simple as deleting old keys: verification runs from genesis, so removing
record 0 makes the whole stream read `SequenceGap`. A **signed checkpoint** fixes this — it
attests a trusted mid-chain boundary, so pruned-below records verify from the checkpoint
instead of genesis.

The scheduled retention flow (run it from a maintenance task, e.g. daily):

```rust
// 1. Records are already mirrored to your WORM/SIEM by the AuditSink (§4).
// 2. Seal a checkpoint at the current boundary:
let (seq, _prev) = agent.audit_checkpoint()?;          // sys/audit-checkpoint/{node}/{seq}
// 3. Prune this node's records below the newest checkpoint (irreversible locally):
let pruned = agent.audit_prune_to_checkpoint();
```

- The checkpoint lives under a **separate** prefix (`sys/audit-checkpoint/`), so pruning
  `sys/audit/` never touches it.
- `audit_verify(node)` now resumes from the newest covering checkpoint automatically; the pruned
  stream still verifies. Full genesis verification remains possible **offline** against the
  exported WORM archive.
- Export **before** you prune — pruning is irreversible on the node. For 7-yr-class HIPAA
  retention the WORM archive is the system of record; the in-cluster trail is the hot window.

Do **not** hand-delete `sys/audit/` keys without a covering checkpoint — verification will fail,
by design (the chain is meant to notice).

---

## 6. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| no `sys/audit/` entries under `compliance` | `GossipConfig::tls` unset → `audit()` errors (and with a sink attached the node does not start at all) | enable TLS; check logs for "set GossipConfig::tls" |
| `/gateway/audit` → 403 | token lacks `audit:read` | grant the scope (or `*`) on the token |
| `verified: false`, `UnknownSigner` | owner's identity key not learned | confirm peering + shared CA; the key arrives via `sys/identity/` gossip |
| `verified: false`, `BrokenLink`/`BadSignature` | records tampered or store hand-edited | treat as an incident; cite the offending `seq` + `content_hash` |
| trail not shrinking | by design — no time-eviction of live keys | export + size the store; see §4 |
| at `start()`: `audit: the persisted stream stops verifying at …` | a record in this node's own persisted stream did not verify (planted, edited, or signed by a key since revoked — see §1 on rotate-and-revoke) — the head resumed after the verified prefix and the next seal overwrote it | treat as an incident; the log names the `seq`; the overwritten bytes survive in `snapshot.bin`/`wal.bin` only until the next periodic snapshot compacts them — copy the persistence directory first |

---

## 7. `/gateway/transparency` — revocation proofs

Key **revocations** get their own tamper-evident surface: a Merkle transparency log over
each node's validated revocation list, served at `GET /gateway/transparency` (scope
`transparency:read`; `compliance` feature). It answers a different question from §2's audit
trail — not "what happened" but "**can I prove this key was revoked fleet-wide, without
trusting the server that told me?**"

Two modes:

```bash
# Head mode (no query) — every node's revocation-log Merkle root + count:
curl -H 'Authorization: Bearer <transparency-token>' \
     'http://NODE:PORT/gateway/transparency'
# → { "nodes": [ { "node": "10.0.0.7:8080", "root": "<64-hex>", "count": 3 }, … ] }

# Inclusion-proof mode (?node=&key=<64-hex of the revoked verifying key>):
curl -H 'Authorization: Bearer <transparency-token>' \
     'http://NODE:PORT/gateway/transparency?node=10.0.0.7:8080&key=<64-hex>'
# → { "node": "…", "revoked_key": "…", "included": true,
#     "root": "<64-hex>", "leaf": "<64-hex>", "index": 1,
#     "proof": [ { "sibling": "<64-hex>", "on_right": true }, … ] }
```

**What it proves.** The inclusion `proof` is the Merkle audit path from the `leaf` (the
revocation record's hash) up to the `root`. The operator verifies it **client-side** —
recompute the root from `leaf` + `proof` and check it equals `root` (the
`transparency::verify_inclusion` function, exposed for exactly this). Because the root is
recomputable on any node from its own gossiped view, a matching proof shows the key was
revoked *and* that the node's revocation log can't have been forged or silently altered —
no trust in the responding server is required. `included: false` means that node has no
validated revocation for the key.

The head `count` is also the cheap fleet-wide sanity check: if nodes disagree on a node's
`root`/`count`, revocations haven't fully propagated yet (or one view is partitioned).

*Code: `src/agent/http/compliance.rs::gw_transparency`, `src/agent/transparency.rs`
(`inclusion_proof`, `revocation_head`, `verify_inclusion`), `src/agent/revocation.rs`.*

---

## 8. Proving a guardrail stopped an agent

When a node runs the `mycelium-guardrails` **Tier-C** invoke gate (feature `compliance`),
every *unauthorized* invocation it blocks is sealed as an `Invoke`/`Denied` record into
**this same** per-node, signed, hash-chained audit trail (§1) — verified caller as
`principal`, the RPC kind as `target`. So "prove agent X was stopped from doing Y" reduces
to reconstructing and re-verifying that chain and citing the sealed denials.

The [`prove_denials` / `narrate_proof`](../guide/16-guardrails.md#proving-a-guardrail-fired)
tool (guide 16) does exactly that — for a compliance officer, the flow is:

```rust
use mycelium_guardrails::{prove_denials, narrate_proof};
// provider = the node that ran the gate; caller = the agent you want to prove was stopped.
let proof = prove_denials(&any_node, provider.node_id(), Some("10.0.0.3:8080"));
for line in narrate_proof(&proof) { println!("{line}"); }
```

It pulls `provider`'s stream, runs the full §3 chain verification (`chain_verified` +
`verify_error`), and lists every matching sealed denial with its citable `content_hash`.
**Any node can run it** — the audit chain gossips fleet-wide, so a neutral third party
proves the denial exactly as the provider would.

**Read the claim honestly** (the tool prints this framing itself): it proves the provider
*tamper-evidently sealed stopping* those specific calls — **provable-stopping**, not a
global "X could not have done Y *anywhere*." The [honest limits](../guide/16-guardrails.md#proving-a-guardrail-fired)
carry straight over from §3: the chain is **per-node**, so absence in one provider's chain
is not proof of absence elsewhere; only *guarded* capabilities that reach the gate seal
denials; and if the chain doesn't verify, the proof is **voided**, not asserted. Verify the
underlying chain with §3 before citing any denial.

*Code: `mycelium-guardrails/src/verify.rs` (`prove_denials`, `narrate_proof`),
`mycelium-guardrails/src/guard.rs` (the sealing gate).*

## 9. Mandate lifecycle — three endings, recorded apart

An appointment can end three ways, and they are **separate events** on purpose:

| Event | Says |
|---|---|
| `RoleExpired { term, at_ms }` | the term reached the end of its window |
| `PermissionWithdrawn { term, by, at_ms }` | the establishing authority took it away — and **who** |
| `OutstandingOperationsInvalidated { epoch, at_ms }` | work authorised under an older epoch is void |

**A term that ran out is not a term that was taken away.** Folding them into one "ended" event
destroys the distinction an auditor most often needs, and only the second carries an actor.

When a fenced write is refused because the holder was replaced, the error is
`MandateRevoked { refname, expected, found }` inside an I/O error — **not a conflict**. If your
tooling classifies it as a conflict it will advise the holder to re-read and retry, which refuses
forever. See [guide 21 · Mandates](../guide/21-mandates.md).

**Strict eligibility — what the operator owns** (2.26.0; only where the embedding application decides
incumbency rules with `mandate::eligibility::ready` over the shipped appointment-stream source,
`mandate::history_source`, feature `tls`). The substrate decides from the appointment stream the appointing
authority publishes in the knowledge layer; three facts about that stream are operational:

- **Publishing.** The authority chains each head from its *own* previous head, one appointment per head.
- **Key revocation.** A reader whose checkpoint is on a head signed under a since-revoked key vouches for
  nothing until it holds a head under a current key; older heads under a revoked key stay attributable.
- **Forks.** A stream the reader has seen fork vouches for nothing; forks are held in memory, so a restart
  forgets them.

A successor held with `eligibility unknown: …` (`NotReadyStrict::HistoryUnknown`) is the source failing
closed, not a refusal: check that this node's reader holds a current-key checkpoint for the stream, that the
appointment records the walk names can be fetched, and that the stream has not forked. `not eligible: …` is a
rule saying no. The rule table and the trust walk are in [guide 21 § Eligibility](../guide/21-mandates.md).

## 10. Evidence export — what a gap looks like to the consumer

Authorisation evidence does **not** live in this gossiped chain. Every decision and execution goes
to a **node-local evidence journal**, fsynced and never gossiped; only a hash-bearing reference
enters the tamper-evident chain. That is deliberate: the chain reaches every node and the evidence
is not for every node. A reader cross-checks what was exported against what the chain attests.

Three things an operator has to get right:

- **Attach a journal, or the node records nothing.** A node with an action evaluator and no journal
  enforces and records nothing. It warns at attach time; treat that warning as a failed deployment.
- **The profile decides what a recording failure does.** `Strict` refuses the dispatch when evidence
  cannot be recorded — enforcement and attribution stand or fall together. `Lenient` proceeds and the
  evidence says it was produced under a profile that does not gate: weaker, and **legible as weaker**.
- **A gap is visible in the cursor, not inferred from silence.** Exporters read the journal through a
  durable cursor. A re-read after a lost cursor re-sends the same record ids with byte-identical
  bodies, which is why each record carries its **own event time** rather than its read time. A
  consumer that receives the same id with a different body must refuse it.

Coverage travels with the evidence as a field. `coverage.complete: false` names the routes the
enforcement point does not see. **Never read silence as an all-clear.**

### Gaps, skipped slots and kept records (post-360 plan row Q)

- **Seqs may jump.** A seal skips a slot that a record the node cannot verify occupies (kept, never sealed over) and
  links across it by hash. The contiguous verifiers — `verify_chain`, `verify_chain_keys`,
  `verify_stream_from_genesis` — require `seq = previous + 1` and report such a jump as `SequenceGap`. **An external
  verifier of an `AuditSink` export should use `verify_linked(records, node, keys, anchor_seq, anchor_prev)`**, which
  follows the hash links (the rule the node verifies itself by) and returns which records verify and which do not.
- **A planted run never stops sealing.** When records of the node it cannot verify occupy the slots after the chain
  head, the seal finds the first free slot outside the audit lock — one ordered pass over the stream; the lock is held
  only for an O(1) check of the head's slot per attempt — seals there, links by hash, and reports the run with the
  other unverified records. Garbage at a key is not a record and occupies nothing. (That rule could overwrite a
  genuine record only on a node downgraded across a change of the audit record's format, which would read its own
  newer records as garbage — an unsupported downgrade.) The look-ahead costs one O(1) lookup per occupied slot, not a
  read of the stream, and scan-then-recheck is retried at most 8 times before the free slot is found under the lock
  (the race reported); a seal never refuses and never loops unbounded.
- **Kept, unverified records stay until an operator removes them.** Neither recovery, sealing nor
  `audit_prune_to_checkpoint` deletes them. To remove them: read the seqs from `GossipAgent::audit_segments(node)`
  (`unverified`), or `GET /gateway/audit` (`segments.unverified_seqs`); keep a copy as evidence; then delete each with
  the embedded API, `agent.kv().delete(mycelium::audit_key(&node, seq))` (the gateway's KV routes refuse the owned
  `sys/` namespaces). Removing them clears the report at the next restart, and frees the slots the next seals skip.
- **A pruned stream whose boundary record was removed** anchors at the newest checkpoint below its first record, so
  numbering resumes at that boundary — not at genesis, among exported and pruned slots.
- **The anchor.** `verify_linked` takes the anchor `(seq, prev_hash)` from the caller: take it from a checkpoint whose
  signature you verified (`SignedAuditCheckpoint`), or `(0, [0; 32])` for genesis.
- **Residual, with `require_identity_proofs` explicitly off (the default until post-360 row S; `deprecations.md` §41):** a key poisoned into a node's identity set can sign an
  audit checkpoint for that node that replaces the genuine one by LWW and so steers where its stream is anchored. This
  is pre-existing in kind (the identity-poisoning residual); a peer's write into `sys/audit-checkpoint/{node}/` now
  trips the `sys/` tripwire, so it is counted.
