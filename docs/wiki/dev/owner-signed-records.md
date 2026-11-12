# Owner-signed records — a security-bearing KV record is signed by its owner and verified at read

↑ [dev/](dev.md) · siblings: [security](security.md) · [runtime-invariants](architecture/runtime-invariants.md) ·
canon: `src/lib.rs` § KV namespace ownership · `docs/threat-model.md` §5 Boundary H · plan:
[`docs/plans/boundary-h.md`](../../plans/boundary-h.md) §17

**The rule (adopted by the maintainer, 2026-11-12):** *a security-bearing record kept in the gossip KV is signed by
its owner and verified at read.*

This page states the rule, says what it covers and what it does not, and lists — by enumeration, against the code
at `7d4c86a6` — every KV namespace a decision reads and whether the rule holds there. The inventory is the starting
list for the hardening plan. A row that says *meets* cites the code that verifies; a row without such a citation
says *does not hold*.

## Why

Layer I's law is **detection, not prevention** ([runtime-invariants](architecture/runtime-invariants.md)): any
admitted fleet member can write any key, a tombstone included, and LWW accepts it. That is kept deliberately
(`src/lib.rs` — *"Ownership is promise-strength, not mechanism-strength"*). Three facts of the transport follow, and
every row below depends on them:

- **A frame's signature names its sender, not the key's owner.** A live KV write under `[tls]` is signed by its
  writer and a receiver checks only that the signer is the frame's sender
  (`mycelium-core/src/connection.rs:1154`, `if signer != update.sender`). Nothing compares the signer with the node
  segment of the key. A member writes `cap/{victim}/…` under its own valid key and the frame verifies.
- **An anti-entropy entry names no writer.** `SyncEntry` is `key, value, timestamp, is_tombstone`
  (`mycelium-core/src/framing.rs:220`); a `StateResponse` entry is applied by LWW with `Origin::Unknown`
  (`connection.rs:843`, `:867`). The store keeps no writer and no signature at rest (`store.rs:482`). The code says
  so: `mycelium-core/src/config.rs:1724`.
- **The `sys/` tripwire detects, at the owner only.** It counts an applied foreign write to one of fifteen
  self-owned `sys/` prefixes (`connection.rs:46`) on the node the key names (`sys_namespace_violations`); the write
  is still applied, and a node that *reads* the forged record sees no count. An anti-entropy forgery that arrives
  before the owner holds its own record is not counted (`connection.rs:78`).

So a record whose content decides authority, admission, cancellation, retention or audit cannot be trusted because
of where it sits. It must carry its own proof.

**Why it had to be written down.** The rule was rediscovered three times in one week by independent reviews: #757
(a bearer exchanged token could be lifted by a relaying member; fixed by DPoP binding), #766 (I4's window record
could be forged by any member to hasten collection; fixed by binding its signature to the key's gateway segment),
and the grant use marker `grant/…/used/{door}`, which a member can tombstone to re-arm a grant (open, below). An
external critique of 2026-11-12 named this as the single root cause across grants, audit, provisioning and caller
identity.

## It does not contradict "detection, not prevention"

The rule teaches Layer I nothing. No prefix write guard is added to `apply_and_notify`, no key is refused at the
store, forwarding and anti-entropy are unchanged, and a forged record still propagates and is still stored. The rule
constrains **how an upper layer stores and reads its own state**: the reader, which is the enforcement point,
decides whether the bytes it found are a record. Detection stays where it is (tripwires and counters); what
changes is that a decision no longer rests on a detector that may sit on another node.

The gateway's raw KV routes refusing owned namespaces (`refuse_protected_key`, `src/agent/http/authz.rs:1821`) is
not this rule and does not satisfy it: that door stops an HTTP client holding `kv:write`, not a member.

## The four conditions

A record meets the rule when all four hold:

- **(a) Signed by its owner.** It carries a signature under the key of the principal its key path names. Where the
  principal's standing is issued (a node's certificate, a client's certificate), the issuer's signature naming that
  principal is the signature meant.
- **(b) Verified before use, body against key.** The reader verifies the signature, and that the body's
  identifying fields equal the key's segments, before the record moves any decision. A signature alone lets a
  genuine record be moved to another key.
- **(c) A failure is an absence, and is counted.** A record that fails is treated as absent. Absence fails closed,
  or to a safe fallback the design states, and the failure is counted where an operator reads it. This includes the
  tombstone: a reader that permits when the record is *deleted* does not meet (c), whatever the signature.
- **(d) A forged-record test.** A test writes the record as another member would — forged, replayed, tombstoned —
  and shows the reader is not moved. A test that shows the forgery is *counted* while the reader obeys it is not
  this test.

## What is security-bearing

A record is security-bearing when a reader uses it to decide one of:

- **authority** — who may act, under which grant, mandate, role or revocation;
- **admission** — whose key, certificate, membership or caller identity is accepted, and what is refused;
- **cancellation** — that a grant was used, consumed, revoked, cancelled or released;
- **retention** — how long evidence or state is kept before it is collected;
- **audit** — what the record of decisions says happened, and whether it is complete.

## What the rule does not cover

The rule does not say every record is signed.

- **Availability.** A member who can write any key can also flood, tombstone and stall. A record whose worst
  forgery is a refusal, a delay or a retry is not made safe by a signature and is outside the rule. (A reader
  should still fail closed; that is condition (c)'s concern only for records inside it.)
- **Soft state whose worst case is detected and bounded.** Discovery hints, load and pressure signals, advisory
  configuration, diagnostics: unsigned by design, provided the page that owns the record names the detector and the
  bound. *Detected at the owner only, while another node acts on it* is not a bound.
- **Records no decision reads.** Mirrors and announcements (`agent/{node}/task/…`, `grant/…/revoked`).
- **A holder of the fleet CA key, colluders among themselves, and self-assertion.** A node that signs its own
  `sys/role/` claim as `admin` meets (a)–(d); whether a self-signed role should carry authority is Boundary H's
  entitlement question (P2, H3), not this rule.
- **Layer I.** The rule does not ask the store to authenticate entries. Anti-entropy entries that carry their
  origin's signature would be a wire change and a separate decision.

## The pattern, where it already exists

`sys/identity-signed/` and `sys/identity-cert/` · `sys/client-cert/` with its fold (I1) · `sys/revocation/` · signed
revocation checkpoints · signed grants with possession proofs · I5's exchanged token bound to the envelope's
signing key · electorate certificates · `sys/audit/` records · `facts/` · I4's `call/{gw}/{term}/window` record
(specified in `docs/plans/identity-and-lifecycle.md` row I4; no `call/` code is on `main` at `7d4c86a6`).

## The inventory

**How it was enumerated (rule 1).** The owned-namespace table the raw KV write routes use, and the crate doc's
ownership table:

```bash
git grep -n "refuse_protected_key" -- src/            # the doors: kv.rs:47,108,177 · overlay.rs:218 · authz.rs:1821
git grep -n "OWNED_KV_PREFIXES\|APPLICATION_KV_PREFIXES\|OWNED_LOG_STREAMS" -- src/agent/http/authz.rs
sed -n 82,200p src/lib.rs                             # § KV namespace ownership
git grep -n "SELF_OWNED_SYS_PREFIXES" -- mycelium-core/src/connection.rs   # what the tripwire watches
```

For each prefix: the writer, every reader that decides on it, the verifying code, the behaviour on absence, and a
test that forges. Citations are `file:line` at `7d4c86a6`; test names are given so a moved line can be found.
*Inferred* marks an effect traced through the code with no test that shows it.

**Status values.** **Meets** — (a)–(d) hold, residual stated. **Partly** — signed and verified, but a named
condition fails. **Does not hold** — a decision of the kinds above rests on an unsigned or unverified record.
**Outside: availability** — unsigned, and the worst forgery found is a refusal or delay. **Outside: no decision** —
nothing decides on it. **Bounded, not detected** — soft state with a bound and no detector.

### Identity and admission

| Record | (a) signed | (b) verified, body = key | (c) fail = absent, counted | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `sys/identity-signed/{node}` (and the bare `sys/identity/` + `sys/identity-proof/` pair) | The proof chains to a key already held for the node (`src/agent/helpers.rs:592`) | Node from the key path; the bare pair is ignored under the default `require_identity_proofs` | Rejected records merge nothing and are counted; a tombstone removes no key | `an_unsigned_identity_forgery_cannot_make_a_member_sign_as_another` (`src/lib_tests.rs:1351`), `an_identity_tombstone_does_not_erase_a_members_keys` (`:1623`), `a_replayed_pre_rotation_sealed_record_does_not_replace_the_current_one` (`:1655`) | **Meets.** Residual: first sighting of a node with no certificate record accepts a self-signed history (`helpers.rs:596`) |
| `sys/identity-cert/{node}/{key-hex}` | CA-issued certificate naming the node | Key equals the key segment and the URI names the node (`src/agent/revocation.rs:430`) | Treated as absent; only `cert_future` and `over_cap` are counted, a wrong CA, URI or key is dropped silently | `an_owner_republishes_its_overwritten_certificate_record` (`src/lib_tests.rs:2385`), `a_planted_record_does_not_refuse_a_start_whose_own_certificate_record_is_missing` (`:2695`) | **Meets**, (c) counted in part. Collapses where every node holds the CA key (the development posture) |
| `sys/revocation/{node}/{key}[/{revoker}]` | An anchored key of the node issued after the revoked one (`revocation.rs:621`) | Path equals body before any signature check (`revocation.rs:463`) | Ignored; not counted (only `over_cap`, `journal_refused`) | `a_member_cannot_revoke_another_members_key_by_rewriting_its_identity` (`src/lib_tests.rs:1947`), `a_revocation_hidden_while_the_verifier_was_down_still_counts` (`:2260`), `junk_revocation_records_keep_a_fold_bounded` (`:2534`) | **Meets**, (c) uncounted. Residual: without `[persistence]`, a restart while the record is tombstoned forgets it until the owner republishes |
| `sys/client-cert/{principal-hex}/{key-hex}` and its fold | CA-issued certificate naming the principal | Principal and key equal the path (`src/agent/client_cert.rs:310`) | Unknown key ⇒ `KeyUnknown`, closed (`client_key_standing`, `client_cert.rs:376`); wrong CA or principal uncounted | `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it` (`src/agent/client_cert_tests.rs:60`), `junk_records_sorted_ahead_cannot_hide_a_newer_certificate` (`:236`), `a_node_joining_later_reads_a_tombstoned_newer_record_back` (`:364`) | **Meets**, (c) counted in part. Residual (stated in the module): without `[persistence]`, a restart while the newer record is hidden lets the superseded key back |
| `sys/membership/removed/{node}` | A configured membership authority (`auth.verify`, `src/agent/membership_ops.rs:32`) | The key segment is ignored; the signed body names the node (`membership_ops.rs:24`) | A refusal is dropped (`let _ = apply(…)`, `membership_ops.rs:85`); not counted | Forgery only through `offer_member_removal` (`a_removed_member_is_cut_off_and_an_unremoved_one_is_not`, `src/lib_tests.rs:15202`); no KV plant or tombstone test | **Partly.** (c) uncounted, (d) missing. No persistence of the removed set was found, so a tombstone before a restart re-admits the member there until the removal is offered again (*inferred*) |
| `sys/role/{node}` | The node's own key | Claim's node equals the key's, then the signature (`src/agent/rbac.rs:130`, `:133`) | Absent ⇒ no roles ⇒ deny; not counted | `verified_roles_rejects_node_mismatch` (`rbac.rs:236`), `verified_roles_rejects_bad_signature_and_garbage` (`:250`); no replay test | **Partly.** `issued_at_ms` is not checked at read, so an older genuine claim can be replayed (*inferred*). Self-assertion is outside the rule (above) |
| `sys/capauthz/{ns}/{name}` | No | No; the consensus commit that preceded the write is not re-checked at read | Absent or undecodable ⇒ **no policy ⇒ open** (`required_roles`, `src/agent/capauthz.rs:56`–`60`) | None | **Does not hold.** A tombstone removes the role requirement |
| `sys/caller-context/{node}` | No (`b"1"`/`b"2"`) | No (`provider_enforces_context`, `src/agent/gateway_caller.rs:514`) | Not counted at readers | None for a forged or tombstoned marker | **Does not hold.** Tombstoning a provider's marker stops secure gateways dispatching to it (closed); lowering or tombstoning a gateway's makes providers read its bare RPC as the node itself (*inferred*, open) |
| `sys/caller-context-id/{node}` | The node's key, node inside the signed message (`gateway_caller.rs:587`) | Yes, with freshness | Absent or bad ⇒ `false` ⇒ id-less envelopes accepted, counted `without_id` | `only_a_gateways_own_fresh_id_marker_is_honoured` (`src/lib_tests.rs:3853`) | **Partly.** (c): a tombstone fails open — a captured, id-stripped envelope replays inside the freshness window |
| The caller envelope's principal, for a principal with no key (`token:…`, `oidc:…` without exchange) — not a KV record, the same root | The framing node's key only: `via` is the sender and the signer is a trusted key of `via` (`gateway_caller.rs:1216`, `:1239`) | Nothing binds the principal to a key of its own, and nothing checks that `via` is a gateway; `client_admitted` compares strings (`gateway_caller.rs:1590`) | — | `only_a_gateways_own_fresh_id_marker_is_honoured` frames `oidc:idp/alice` from a plain node and the provider admits it | **Does not hold.** Any member holding a node key can assert any key-less principal. Closed for a principal with a client certificate that signs (`cs`, I1) and for `oidc:` actions under I5 |
| I5's exchanged token (carried in the envelope) | IdP-signed, `cnf.jkt` bound to the envelope's signing key (`src/agent/exchanged_token.rs:120`); subject equals the principal (`:125`) | Yes | Refused; `(iss, jti)` admitted once per provider | `each_check_refuses_an_exchanged_token_by_name` (`src/agent/exchanged_token_tests.rs:165`), `the_gateways_signature_over_the_token_is_required` (`:213`) | **Meets**, only where the provider's `[oidc]` sets `exchange_audience` |

### Grants, mandates and cancellation

| Record | (a) signed | (b) verified, body = key | (c) fail = absent, counted | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `grant/{agent}/{request}` (the request) | No | No | No | None | **Does not hold.** The supervisor signs the tool, digest and resource read from this record (`src/agent/http/grants.rs:337`) |
| `grant/…/issued` | The issuer's signature (`verify_signed_by`, `src/mandate/grant.rs:276`) | Holder equals the caller, with a possession proof; the scope is not compared with the path | Not established ⇒ refused; not counted | Revoke path only (`src/agent/client_grant_tests.rs`) | **Meets** (a), (b); (c) uncounted, (d) thin. Under fleet entitlement any member node is an entitled issuer (`grant.rs:190`) — configuration, stated |
| `grant/…/checkpoint` | The issuer's (`src/mandate/authority.rs:259`) | The view is keyed by the signed body | The offer's result is discarded (`let _ =`, `src/agent/gateway_authority.rs:242`); not counted | Unit tests in `mandate/authority.rs`; none at the KV | **Partly.** The genuine sequence-1 record replayed over sequence 2 reads *not revoked* at a node restarted since (*inferred*; the restart half is stated at `grants.rs:866`) |
| `grant/…/consumed` | No (`b"1"`) | No | Absent ⇒ the request is re-honoured (stated: `src/agent/state_machine.rs:696`) | None | **Does not hold.** A tombstone re-honours the grant |
| **`grant/…/used/{door}`** — the use marker | No; the value is a timestamp (`src/agent/http/authz.rs:1460`) | No; only presence is read (`authz.rs:1437`) | **Absent ⇒ unused ⇒ admitted** | None for a hostile tombstone | **Does not hold — open.** A tombstone re-arms the grant at every other node at once, and at the same node once its in-memory claim is swept (`gateway_authority.rs:440`; the claim lives ≤ 60 s, the grant up to 3 600 s) or after a restart. No node-local record of a use exists besides the KV's own WAL |
| `grant/…/refused` | No | No | — | None | **Outside: availability.** A forged refusal stops the agent |
| `grant/…/revoked` | No | — | — | — | **Outside: no decision.** The doors read `/checkpoint` |
| `call/{gw}/{term}/window` (I4) | *Specified:* signed by the issuing gateway's node key, bound to the key's gateway segment (`docs/plans/identity-and-lifecycle.md`, row I4) | *Specified* | *Specified:* absent or forged ⇒ the cap-based floor | *Specified* | **Not on `main`.** No `call/` code at `7d4c86a6`; the row is written to this rule and is checked against it when it lands |
| `mandate/{scope}` | — | — | — | — | **Outside: no decision.** Reserved; no reader or writer found |

### Audit and evidence

| Record | (a) signed | (b) verified, body = key | (c) fail = absent, counted | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `sys/audit/{node}/{seq}` — each record | The node's key | `seq` and `node_id` equal the key (`src/agent/audit.rs:595`); hash link (`audit.rs:271`) | `malformed` / `unverified` counted | `an_audit_record_whose_key_and_body_disagree_is_ignored` (`src/lib_tests.rs:2964`), `a_forged_record_in_the_persisted_audit_stream_does_not_seed_the_chain_head` (`:8033`) | **Meets**, per record |
| The audit chain's **head** | **None.** No signed head and no external anchor; the head is the last record present | — | **A truncated tail, or a wholly deleted chain, verifies `Ok`** (`empty_chain_verifies`, `audit.rs:917`) | None for truncation | **Does not hold.** After the owner restarts, `recover_chain_head` resumes at the truncated head and the loss is invisible in-cluster. `AuditSink` is a mirror that drops when full, not an anchor |
| `sys/audit-checkpoint/{node}/{seq}` | The node's key | `node_id` equals the key's (`audit.rs:539`); `{seq}` is not compared | Skipped; not counted | `a_peers_write_into_a_nodes_audit_stream_trips_the_tripwire` (`src/lib_tests.rs:2993`) | **Partly.** A mid-chain boundary; it covers no tail |
| `audit/{ts}/{node}` (SkillRunner, without `compliance`) | No | No | — | — | **Outside: no decision.** No reader; it is not evidence |
| `sys/ae-tally/{node}/hb` · `…/c/{counter}/{actor}` | **No** — stated (`src/agent/fleet_history.rs:61`) | No | A missing counter entry counts 0 (`fleet_history.rs:452`); a peer whose heartbeat says `enforces: false` is skipped (`:520`) | Future-time forgeries only (`src/agent/fleet_history_tests.rs`) | **Does not hold, stated.** Tombstoning a counter entry, or forging the heartbeat, permits past a fleet limit. Owner-side tripwire only |
| `sys/ae-exposure/{node}` | **No** — stated (`src/agent/exposure.rs:50`) | No | A fresh forged summary listing nothing adds 0; a tombstone reads *no summary* (closed) | Unreadable, future and expired summaries only (`src/agent/exposure_tests.rs`) | **Does not hold, stated.** A forged summary under-counts and permits |

### Consensus, electorates and exclusivity

| Record | (a) signed | (b) verified, body = key | (c) fail = absent, counted | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `consensus/electorate-cert/{group}/…` with its `consensus/committed/electorate/…` record | The certificate carries the electors' signed votes (`certificate_holds`, `src/consensus.rs:2883`) | Slot, value digest and voter-in-electorate; a genesis needs every member it names (`consensus.rs:2276`) | Refused, skipped and counted (`note_refused`, `src/agent/electorate.rs:694`) | `a_forged_electorate_record_is_not_adopted` (`src/lib_tests.rs:16469`), `an_electorate_certificate_verifies_under_tls` (`:16554`) | **Meets** under `[tls]` (without it the check is a shape). Open questions, *inferred, no test*: a genesis naming only its author certifies itself — adoption does not compare the members with the roster; a tombstone after adoption is not re-examined, so a restarted node sees no electorate |
| `consensus/life/{slot}/{ballot}` · `/s` · `/end/…` · `/v/{digest}` — the decision record, sentinel, release marker, value | No | The envelope's slot equals the key's and the value's digest (`src/consensus_life.rs:254`); no signature | Malformed skipped; only a ballot above 2^32 is counted | `row16_a_forged_record_ballot_is_counted` (`src/agent/lock_lifecycle_api_tests.rs:274`) — counts it; the forged record is still the top | **Does not hold.** A planted record above the top names a holder; a marker forged from the top's public fields releases a lock or leadership (*inferred*). The design's stated limit applies: exclusivity is enforced at the resource, by its fencing token |
| `consensus/committed/{slot}` · `consensus/lease/{slot}` (legacy reading, slots with no decision record) | No | No (`legacy_live`, `src/consensus.rs:986`) | A tombstoned or malformed lease reads **permanent** (`consensus.rs:1001`) | `row10_a_legacy_decision_is_released_the_legacy_way` (`lock_lifecycle_api_tests.rs:262`) shows a plain write is read | **Does not hold**, legacy slots only |
| `consensus/ballot/{slot}` · `consensus/decided/{slot}` | No | No | Counted above 2^32 and **still obeyed** (`consensus.rs:1292`) | `a_forged_decided_floor_is_counted_and_still_obeyed` (`src/lib_tests.rs:13718`) | **Outside: availability** (a forged ceiling exhausts the slot). A tombstoned floor is covered by the acceptor's own durable record |
| `sys/consensus-accepted/{node}/{slot}` | No | No (`decode_acceptor`, `consensus.rs:4599`) | A foreign change withholds the slot and is counted at the owner (`src/agent/acceptor_guard.rs:307`) | `test_sys_namespace_tripwire_flags_foreign_self_owned_write` (`src/lib_tests.rs:7767`) — detection | **Does not hold.** Detected at the owner; whether a forged acceptance survives into the owner's repaired record and its next start is *inferred* and needs a test |
| `consensus/trust/{group}/{node}` | No | No | No | None | **Outside: availability** with `use_trust_slices` (a narrowed slice times a proposer out) |
| `sys/topology-override/{group}` | No | No (`consensus.rs:1696`) | No | None | **Does not hold.** `b"true"` disables a hard topology gate for the group |
| `grp/{group}/{node}` | No | No | A roster that disagrees with a declared electorate is refused and counted (`mycelium_electorate_roster_mismatch_total`) | None; tests use a foreign `grp/` write as setup | **Does not hold.** A peer's write flips the named node's own signal boundary (`mycelium-core/src/connection.rs:1070`) and changes a non-electorate group's quorum size |
| `sys/quorum/{kind}/{sender}` | No | No | No; excluded from the tripwire by design (`connection.rs:27`) | None | **Does not hold.** After a restart, forged sender evidence seeds `quorum()` (`src/agent/lifecycle.rs:1146`) |
| `lock/{name}` (raw key) | — | — | — | — | **Outside: no decision.** Locks decide under `consensus/life/` |

### Governance intents and soft state

| Record | Signed, verified | Worst forgery found | Detector | Status |
|---|---|---|---|---|
| `sys/govern/membership/{group}` | No | A forged `drain` makes the named node leave (`src/agent/membership_governor.rs:285`); a huge `min` refuses every election; a future `written_at_ms` never evaporates (`src/agent/intent.rs:57`, *inferred*) | None; the gateway route is audited, a KV write is not | **Does not hold** |
| `sys/govern/timing` · `sys/govern/fleet` | No | Fleet timing within `validate()`'s bounds; the auto-tuner disabled or bounded | None | **Bounded, not detected** — soft state with a bound (the setters' ranges) and no detector, so not yet outside the rule |
| `sys/config/{param}` | No | `writer_channel_depth` on nodes whose policy accepts the advice (`src/agent/cluster_tuner.rs:78`) | None | As above |
| `sys/rate/{observer}/{sender}` | No | Forged observers throttle any sender to its floor (`mycelium-core/src/rate.rs:111`) | `rate_limited_senders` (not a forgery signal) | **Outside: availability** |
| `sys/load/{node}/…` | No | Opacity routing; with the opt-in `count_opaque_as_absent`, a non-electorate quorum shrinks (*inferred*) | `sys/` tripwire, owner only | **Outside: bounded soft state** by default; **does not hold** with `count_opaque_as_absent` |
| `sys/health/{node}` | No | A diagnostic field | — | **Outside: no decision** |
| `sched/{id}/last/{node}` | No | A rolling entry ahead suppresses the schedule (≤ 64 slots); a tombstone allows a repeat run of the slot | Traced reason `last_ahead` | **Outside: availability** for suppression; **does not hold** for the repeat run |
| `sys/tuple/{node}/…/pressure/{stage}` · `tuple/inflight/…` | No | Producers' `put` answers `Backpressure` | `sys/` tripwire, owner only | **Outside: availability** |

### Capabilities, provisioning and what a model is told

| Record | Signed, verified | What decides on it | Worst forgery found | Status |
|---|---|---|---|---|
| `cap/{node}/{ns}/{name}` (every advertisement, `authorized_callers` and `llm-meta` attributes included) | No. The writer chooses its own refresh interval, so a forged entry sets its own lifetime. `filter_protected` (H3, `src/mandate/protected.rs:133`) checks a grant and possession proof bound to (node, ns, name) and **has no caller outside its own tests** | Routing (`resolve_for_caller`, `src/agent/capability_handle.rs:165`; the gateway's `/llm`, A2A and caps routes with no role check), the provisioner's floor and ceiling, schedule election, `mycelium-reason`'s locality for data-class routing | A fake or hidden provider; a forged `locality` on a live node's `llm-meta` routes restricted data off-domain (*inferred*). `authorized_callers` in an ad is a discovery filter only — enforcement is the provider's own configuration | **Does not hold.** No counter for a foreign `cap/` write |
| `req/{node}/{ns}/{name}` | No (`src/agent/demand.rs:31`) | The provisioner's demand-driven install | Forged demand installs a catalogued artifact (bounded by provenance, budget and self-election); a tombstone removes demand | **Does not hold**; no detector |
| `cap/{node}/prov-shed/{band}` | No — a plain advertisement (`mycelium-wasm-host/src/provisioner.rs:1200`) | The presence-ceiling shed's ranking (`provisioner.rs:1240`) | A band held above its ceiling, or shed below it; with forged providers, served only by fakes (*inferred*) | **Does not hold**; `mycelium_artifact_presence_unranked_departures_total` is the one indirect count |
| `cap-group/{group}` · `gcap/{group}/{ns}/{name}/{contrib}` | No; a contributor is not checked against `grp/` | Emergent-group membership and group-level providers (`src/agent/emergent_groups.rs:175`) | A forced join and projected capabilities fleet-wide; inflated presence | **Does not hold** |
| `tools/{name}/{node}` · `skills/{ns}/{name}/{node}/…` | No | The `/mcp` door's tool list and provider choice (`src/agent/http/mcp_bridge.rs:55`); the tool schema and A2A card text a model reads | A provider chosen by the forger; text placed in front of a model | **Does not hold** |
| `prompts/{ns}/{name}` | No; no principal in the key | Read on every invocation of the model skill (`src/agent/llm.rs:365`) | The skill's template rewritten fleet-wide; written once, never re-asserted | **Does not hold** |
| `installable/{ns}/{name}/{hex}` | The publisher's signature over kind, artifact, requirements and `provides` (`verify_provenance`, `mycelium-wasm-host/src/catalog.rs:187`), enforced at `provenance_ok` (`provisioner.rs:538`) | Install | A tombstone erases a line (the librarian re-publishes its own); a withdrawn, validly signed line replays | **Meets** for forgery where `trusted_publishers` is set — `require_provenance_refuses_unsigned_but_installs_trusted_signed` (`provisioner.rs:2110`), `provenance_binds_the_whole_entry_not_just_the_bytes` (`catalog.rs:792`). An empty list verifies nothing (a stem refuses to start that way unless `accept_unsigned`); a rejected line is traced, not counted; no tombstone or replay test |
| `mailbox/{target}/{kind}/{hlc}` | No; the sender is a field the writer sets (`src/agent/mailbox.rs:178`) | Whatever the application decides from the sender | A spoofed sender; deleted messages | **Does not hold** where an application trusts the sender |
| `wiki/{group}/proposal/{id}` | No; `author` is self-declared | The curator applies what decodes; the mandate fence is at the store, not here | A forged author on an applied edit | **Does not hold** for attribution |
| `comp/{node}/{ns}/…` | No | A guest's own working state | Planted or erased guest state | **Does not hold** for a guest that decides from it |
| `svc/` · `agent/{node}/…` · `log/`, `clog/` · `schemas/` · `manifest/` · `ui/` | No | No security reader found: turn and tool budgets are enforced from memory (`src/agent/state_machine.rs:425`), the KV keys are mirrors | A consumer group skipped or replayed (`clog/`) | **Outside: no decision** / availability |

### Companions

| Record | (a) signed | (b) verified, body = key | (c), (d) | Status |
|---|---|---|---|---|
| `facts/{node}/{field}` (`mycelium-agentfacts`) | The node's trusted keys (`read_verified_fields`, `mycelium-agentfacts/src/crdt.rs:108`, `:120`) | The node, through the keys; the `{field}` segment is not compared with the signed field | Dropped, uncounted; `per_field_merge_lww_and_forgery_rejection` (`crdt.rs:221`) | **Partly.** A genuine older field replays within its TTL |
| `cn/{requirement}/award` (`mycelium-commitment`) | The declarer the caller pins (`award_of_verified`, `mycelium-commitment/src/lib.rs:1114`) | Requirement and operation id equal the slot | `award_of_verified_reports_no_award_the_pinned_declarer_did_not_sign` (`lib.rs:1624`); uncounted | **Meets** on the verified path. The plain `award_of` path, and a tombstone of the award, do not |
| `cn/{requirement}` (the announcement) | No | No | Absent ⇒ `NotAnnounced` | **Outside: availability** — the verified path pins the declarer |
| `knowledge/head/…` · `rights/head/{holder}` | Signed where written | No production reader found (`verify_published_head` is called only by tests) | — | **Outside: no decision** yet; the rule applies when a reader lands |

### Layer I

| Record | What holds | Status |
|---|---|---|
| A live `SignedData` frame | Signed by its writer; the signer must be the sender (`mycelium-core/src/connection.rs:1154`, counted `kv_signer_mismatch`); `a_signed_frame_whose_signer_is_not_its_sender_is_refused` (`src/lib_tests.rs:3601`). The key's owner is not compared | **By design** — the rule does not ask Layer I for more |
| An anti-entropy entry | **Names no writer and carries no signature** (`framing.rs:220`, `connection.rs:867`); applied by LWW under mTLS only; stated at `config.rs:1724` | **Does not hold, stated** — and the reason every row above must verify at read. Closing it is a wire change, a separate decision |

### The six items the adoption named, as found

1. **The grant use marker** — unsigned, presence-only, absence admits: *does not hold, open*.
2. **The audit chain** — every record is signed and verified; no signed head or external anchor, and a truncated or
   deleted chain verifies: *does not hold*.
3. **`cap/`, `req/`, `prov-shed/` advertisements** — unsigned, no reader checks a writer; H3's `filter_protected`
   is unwired: *does not hold*.
4. **`sys/ae-exposure/` and the tally** — unsigned, as the plans record; a tombstone or forged summary permits past
   a limit: *does not hold, stated*.
5. **A caller envelope's key-less principal** — bound only to the framing node's key; any member can assert it:
   *does not hold*.
6. **Anti-entropy entries** — name no writer: *does not hold, stated*, by Layer I's design.

## Using this page

- **A new record a decision reads** is designed to (a)–(d), or lands with a row here saying which condition does
  not hold and why.
- **A review** asks the question of every KV read the change makes (`.claude/commands/adversarial-review.md`,
  check 8).
- **A row moves to *meets*** by citing the verifying code and the forged-record test, in the PR that builds them.
- The lint pass re-verifies the citations (`/wiki-lint`, check 1); nothing in CI does.

Log: [`.log/2026-11-12-owner-signed-rule.md`](.log/2026-11-12-owner-signed-rule.md).
