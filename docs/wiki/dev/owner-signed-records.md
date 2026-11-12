# Owner-signed records — a security-bearing KV record is signed by its owner and verified at read

↑ [dev/](dev.md) · siblings: [security](security.md) · [runtime-invariants](architecture/runtime-invariants.md) ·
canon: `src/lib.rs` § KV namespace ownership · `docs/threat-model.md` §5 Boundary H · plan:
[`docs/plans/boundary-h.md`](../../plans/boundary-h.md) §17

**The rule (adopted by the maintainer, 2026-11-12):** *a security-bearing record kept in the gossip KV is signed by
its owner and verified at read.*

This page states the rule, says what it covers and what it does not, and lists — by enumeration, against the code
at `7d4c86a6` — every KV namespace a decision reads and whether the rule holds there. The inventory is the starting
list for the hardening plan. A row's **Meets** cites the code that verifies and the test that forges; anything less
is **Partly** or **Does not hold**.

## Why

The posture is **detection, not prevention** ([runtime-invariants](architecture/runtime-invariants.md) § *Layer III
posture: detection, not prevention*; `CLAUDE.md`'s hot invariant *never teach Layer I a higher-layer law*): any
admitted fleet member can write any key, a tombstone included, and LWW accepts it. That is kept deliberately
(`src/lib.rs` — *"Ownership is promise-strength, not mechanism-strength"*). Three facts of the transport follow, and
every row below depends on them:

- **A frame's signature names its sender, not the key's owner.** A live KV write under `[tls]` is signed by its
  writer, and a receiver checks only that the signer is the frame's sender
  (`mycelium-core/src/connection.rs:1154`). Nothing compares the signer with the node segment of the key: a member
  writes `cap/{victim}/…` under its own valid key and the frame verifies.
- **An anti-entropy entry names no writer.** `SyncEntry` is `key, value, timestamp, is_tombstone`
  (`mycelium-core/src/framing.rs:220`); a `StateResponse` entry is applied by LWW with `Origin::Unknown`
  (`mycelium-core/src/connection.rs:867`). The store keeps no writer and no signature at rest
  (`mycelium-core/src/store.rs:482`). The code says so: `mycelium-core/src/config.rs:1724`.
- **The `sys/` tripwire detects, at the owner only.** It counts an applied foreign write to one of fifteen
  self-owned `sys/` prefixes (`mycelium-core/src/connection.rs:46`) on the node the key names
  (`sys_namespace_violations`); the write is still applied, and a node that *reads* the forged record sees no
  count. An anti-entropy forgery that arrives before the owner holds its own record is not counted
  (`mycelium-core/src/connection.rs:82`).

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

- **(a) Signed by its owner.** It carries a signature under the key of the principal its key path names
  (`sys/role/{node}`: the node). **Where the path names no principal, or names the subject of the record and not
  its author** — a grant names its agent, a removal names the removed node, a certificate record names the node it
  certifies, a catalogue line names an artifact — **the owner is the authority the reader is configured to trust
  for that record**: the grant's entitled issuer, a membership authority, the fleet CA, a trusted publisher. The
  record carries that authority's signature, and the authority is pinned by the reader's configuration, never read
  from the record or from another unsigned key.
- **(b) Verified before use: the signature, the body against the key, and that it is current.** The reader verifies
  the signature, and that the body's identifying fields equal the key's segments, before the record moves any
  decision — a signature alone lets a genuine record be moved to another key. And it checks that the record is the
  current one: a sequence, an issue time or a supersession rule read from inside the signed body and compared with
  what the reader already holds, so an older genuine record cannot be replayed over a newer one.
- **(c) A failure is an absence, an absence fails closed, and the failure is counted.** A record that fails is
  treated as absent. Absence fails closed, or to a safe fallback the design states. The failure is counted where an
  operator reads it. A reader that permits when the record is *deleted* does not meet (c), whatever the signature —
  and a record whose *presence* cancels (a use marker, a revocation) needs its deletion to be as hard as its
  forgery.
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
  forgery is a refusal, a delay or a retry is not made safe by a signature and is outside the rule.
- **Soft state whose worst case is detected and bounded.** Discovery hints, load and pressure signals, advisory
  configuration, diagnostics: unsigned by design, provided the page that owns the record names the detector and the
  bound (the status **Outside: bounded soft state**). *Detected at the owner only, while another node acts on it*
  is not a bound, and a bound with no detector is the status **Bounded, not detected**, which is not yet outside.
- **Records no decision reads.** Mirrors and announcements (`agent/{node}/task/…`).
- **A holder of the fleet CA key, colluders among themselves, and self-assertion.** A node that signs its own
  `sys/role/` claim as `admin` is correctly signed; whether a self-signed role should carry authority is Boundary
  H's entitlement question (P2, H3), not this rule.
- **Layer I.** The rule does not ask the store to authenticate entries. Anti-entropy entries that carry their
  origin's signature would be a wire change and a separate decision.

## The pattern, where it already exists

`sys/identity-signed/` and `sys/identity-cert/` · `sys/client-cert/` with its fold (I1) · `sys/revocation/` · signed
revocation checkpoints · signed grants with possession proofs · I5's exchanged token bound to the envelope's
signing key · electorate certificates · `sys/audit/` records · `facts/` · I4's `call/{gw}/{term}/checkpoint`
(verified at read since I4's PR 1a; **Partly**, see its row) and its `window` record (specified in
`docs/plans/identity-and-lifecycle.md` row I4; written and read from I4's PR 2).

## The inventory

**How it was enumerated (rule 1).** The owned-namespace table the raw KV write routes use, the application
prefixes beside it, and the crate doc's ownership table:

```bash
git grep -n "refuse_protected_key" -- src/            # the doors: kv.rs:47,108,177 · overlay.rs:218 · authz.rs:1821
git grep -n "OWNED_KV_PREFIXES\|APPLICATION_KV_PREFIXES\|OWNED_LOG_STREAMS" -- src/agent/http/authz.rs
sed -n 82,200p src/lib.rs                             # § KV namespace ownership
git grep -n "SELF_OWNED_SYS_PREFIXES" -- mycelium-core/src/connection.rs   # what the tripwire watches
```

For each prefix: the writer, every reader that decides on it, the verifying code, the behaviour on absence, and a
test that forges. Citations are `file:line` at `7d4c86a6`, each re-read for this page; test names are given so a
moved line can be found. *Inferred* marks an effect traced through the code with no test that shows it.

**Status** — the last column of every table, one of:

| Status | Meaning |
|---|---|
| **Meets** | (a), (b), (c) and (d) all hold |
| **Partly** | signed and verified, and a named condition fails |
| **Does not hold** | a decision of the kinds above rests on an unsigned or unverified record |
| **Bounded, not detected** | soft state with a bound and no detector, or one detected at the owner only while another node acts on it — not yet outside the rule |
| **Outside: bounded soft state** | soft state whose worst case is detected and bounded, the detector and the bound cited in the row. No row is here yet |
| **Outside: availability** | unsigned, and the worst forgery found is a refusal or a delay |
| **Outside: no decision** | no reader decides on it |
| **Outside: Layer I** | a property of the store or the transport, which the rule does not ask for more |
| **Untraced** | a reader exists and what decides on its output was not followed; to be traced before the row is planned from |
| **Not on `main`** | specified, not built |

*Stated* is used in one sense on this page: the code or a plan already says the condition does not hold, and the
row cites where.

### Identity and admission

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `sys/identity-signed/{node}` (and the bare `sys/identity/` + `sys/identity-proof/` pair) | A key already held for the node (`src/agent/helpers.rs:592`) | Node from the key path; the bare pair is ignored under the default `require_identity_proofs` (`src/agent/helpers.rs:504`); a pre-rotation record does not replace the current one | Rejected records merge nothing; a tombstone of the bare key removes no key | `an_unsigned_identity_forgery_cannot_make_a_member_sign_as_another` (`src/lib_tests.rs:1351`), `an_identity_tombstone_does_not_erase_a_members_keys` (`src/lib_tests.rs:1623` — tombstones `sys/identity/`, not the sealed record), `a_replayed_pre_rotation_sealed_record_does_not_replace_the_current_one` (`src/lib_tests.rs:1655`) | **Partly.** (a): first sighting of a node with no certificate record accepts a self-signed history (`src/agent/helpers.rs:596`). (d): no test tombstones the sealed record itself |
| `sys/identity-cert/{node}/{key-hex}` — as read by verifiers | The fleet CA (the path names the subject) | Key equals the key segment and the URI names the node (`src/agent/revocation.rs:430`) | Treated as absent; only `cert_future` and `over_cap` are counted — a wrong CA, URI or key is dropped silently | `a_certificate_issued_in_the_future_outranks_nothing` (`src/lib_tests.rs:2325`), `a_legacy_ip_only_certificate_anchors_nothing` (`src/lib_tests.rs:2352`) | **Partly.** (c) counted in part. (a) collapses where every node holds the CA key (the development posture) |
| `sys/identity-cert/{self}/…` — as read by the owner for its next rotation | — | **None.** `own_newest_issuance` parses each record's issue time with no CA, key or URI check (`src/agent/revocation.rs:827`–`835`) | — | None | **Does not hold.** A planted far-future certificate under a node's own prefix sets the floor for its next rotation, whose certificate verifiers then ignore as `cert_future`: denial of rotation (*inferred*) |
| `sys/revocation/{node}/{key}[/{revoker}]` | An anchored key of the node issued after the revoked one (`src/agent/revocation.rs:621`) | Path equals body before any signature check (`src/agent/revocation.rs:463`) | Ignored; not counted (only `over_cap`, `journal_refused`) | `a_member_cannot_revoke_another_members_key_by_rewriting_its_identity` (`src/lib_tests.rs:1947`), `a_revocation_hidden_while_the_verifier_was_down_still_counts` (`src/lib_tests.rs:2260`), `junk_revocation_records_keep_a_fold_bounded` (`src/lib_tests.rs:2534`) | **Partly.** (c) uncounted; and without `[persistence]`, a restart while the record is tombstoned forgets the revocation until the owner republishes (absence fails open there) |
| `sys/client-cert/{principal-hex}/{key-hex}` and its fold | The fleet CA (the path names the subject) | Principal and key equal the path (`src/agent/client_cert.rs:310`); the newest issuance supersedes | Unknown key ⇒ `KeyUnknown`, closed (`src/agent/client_sig.rs:496`); a wrong CA or principal is uncounted | `a_newer_certificate_supersedes_the_older_key_and_an_overwrite_cannot_lower_it` (`src/agent/client_cert_tests.rs:60`), `junk_records_sorted_ahead_cannot_hide_a_newer_certificate` (`src/agent/client_cert_tests.rs:236`), `a_node_joining_later_reads_a_tombstoned_newer_record_back` (`src/agent/client_cert_tests.rs:364`) | **Partly.** (c) counted in part; and (stated in the module) without `[persistence]`, a restart while the newer record is hidden lets the superseded key back |
| `sys/membership/removed/{node}` | A configured membership authority (`src/agent/membership_ops.rs:32`) | The key segment is ignored; the signed body names the node (`src/agent/membership_ops.rs:24`) | A refusal is dropped (`let _ = apply(…)`, `src/agent/membership_ops.rs:85`); not counted. The removed set is in memory (`mycelium-core/src/removal.rs:21`) | Forgery only through `offer_member_removal` (`a_removed_member_is_cut_off_and_an_unremoved_one_is_not`, `src/lib_tests.rs:15202`); no KV plant or tombstone test | **Partly.** (b): the key segment is not compared with the body. (c) uncounted and absence after a restart fails open (*inferred*: a tombstone before a restart re-admits the member there until the removal is offered again); (d) missing. `with_membership_authorities` has no caller outside tests, so a stock binary never reads the prefix |
| `sys/role/{node}` | The node's own key | Claim's node equals the key's, then the signature (`src/agent/rbac.rs:130`, `src/agent/rbac.rs:133`). **Currency is not checked**: `issued_at_ms` is not compared at read | Absent ⇒ no roles ⇒ deny; not counted | `verified_roles_rejects_node_mismatch` (`src/agent/rbac.rs:236`), `verified_roles_rejects_bad_signature_and_garbage` (`src/agent/rbac.rs:250`); no replay test | **Partly.** (b): an older genuine claim, under the current key or any rotated and unrevoked one, replays (*inferred*). (c) uncounted. Self-assertion is outside the rule (above) |
| `sys/capauthz/{ns}/{name}` | No | No; the consensus commit that preceded the write is not re-checked at read | Absent or undecodable ⇒ **no policy ⇒ open** (`src/agent/capauthz.rs:56`–`60`) | None | **Does not hold.** A tombstone removes the role requirement |
| `sys/caller-context/{node}` | No (a one-byte marker) | No (`src/agent/gateway_caller.rs:514`, `src/agent/gateway_caller.rs:602`) | Not counted at readers | None for a forged or tombstoned marker | **Does not hold.** Tombstoning a provider's marker stops secure gateways dispatching to it (closed). Tombstoning or lowering a *gateway's* marker removes its promise of envelopes, so a member's bare frame naming that gateway as sender is read as the gateway node (next row) |
| The bare RPC's `sender` — not a KV record, the same root | **Nothing.** A `WireMessage::Signal` carries no signature (`mycelium-core/src/connection.rs:874`) | An unframed request maps to `RequestPrincipal::Node(req.sender())` (`src/agent/gateway_caller.rs:1198`–`1203`, `src/agent/gateway_caller.rs:1581`) unless the sender's marker promises envelopes | — | None | **Does not hold.** Any member can act as any node that does not promise envelopes. Not every RPC receive site was traced |
| `sys/caller-context-id/{node}` | The node's key, the node inside the signed message (`src/agent/gateway_caller.rs:587`) | Yes, with freshness | Absent or bad ⇒ id-less envelopes accepted, counted `without_id` | `only_a_gateways_own_fresh_id_marker_is_honoured` (`src/lib_tests.rs:3853`) | **Partly.** (c): a tombstone fails open — a captured, id-stripped envelope replays inside the freshness window |
| The caller envelope's principal, for a principal with no key (`token:…`, `oidc:…` without exchange) — not a KV record, the same root | The framing node's key only: `via` is the sender (`src/agent/gateway_caller.rs:1216`) and the signer is a trusted key of `via` (`src/agent/gateway_caller.rs:1239`) | Nothing binds the principal to a key of its own, and nothing checks that `via` is a gateway; `client_admitted` compares strings (`src/agent/gateway_caller.rs:1590`) | — | `only_a_gateways_own_fresh_id_marker_is_honoured` frames `oidc:idp/alice` from a plain node, and the provider admits it | **Does not hold.** Any member holding a node key can assert any key-less principal. Closed for a principal with a client certificate that signs (`cs`, I1) and for `oidc:` actions under I5 |
| I5's exchanged token (carried in the envelope) | The IdP (the configured authority) | `cnf.jkt` equals the envelope's signing key (`src/agent/exchanged_token.rs:120`); the subject equals the principal (`src/agent/exchanged_token.rs:125`); `(iss, jti)` admitted once per provider | Refused by name | `each_check_refuses_an_exchanged_token_by_name` (`src/agent/exchanged_token_tests.rs:165`), `the_gateways_signature_over_the_token_is_required` (`src/agent/exchanged_token_tests.rs:213`) | **Meets**, only where the provider's `[oidc]` sets `exchange_audience` |

### Grants, mandates and cancellation

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `grant/{agent}/{request}` (the request) | No | No | No | None | **Does not hold.** The supervisor signs the tool, digest and resource read from this record (`src/agent/http/grants.rs:337`); its `expires_at_ms` also decides how long an un-consumed request is re-honoured (`src/agent/state_machine.rs:593`) |
| `grant/…/issued` | The entitled issuer (`src/mandate/grant.rs:276`) | Holder equals the caller, with a possession proof; assessed at the agent (`src/agent/state_machine.rs:659`); the scope is not compared with the path | Not established ⇒ refused; not counted | On the revoke path only (`src/agent/client_grant_tests.rs:334`) | **Partly.** (b): the scope is not compared with the path. (c) uncounted, (d) thin. Under fleet entitlement any member node is an entitled issuer (`src/mandate/grant.rs:190`) — configuration, stated |
| `grant/…/checkpoint` | The issuer (`src/mandate/authority.rs:259`) | The view is keyed by the signed body; sequence applies within a process. A grant-scope checkpoint is exempt from the start rule (`src/mandate/authority.rs:281`), so a restarted node takes whatever copy the KV holds | The offer's result is discarded (`src/agent/gateway_authority.rs:242`); not counted | Unit tests in `src/mandate/authority.rs`; none at the KV | **Partly.** (b) currency: the genuine sequence-1 record replayed over sequence 2 reads *not revoked* at a node restarted since (*inferred*; the restart half is stated at `src/agent/http/grants.rs:866`) |
| `grant/…/consumed` | No (`b"1"`) | No | Absent ⇒ the request is re-honoured (stated: `src/agent/state_machine.rs:696`), for as long as the unsigned request record's `expires_at_ms` says (`src/agent/state_machine.rs:587`, `src/agent/state_machine.rs:593`) | None | **Does not hold.** A tombstone, with a rewritten request expiry, re-honours the grant up to its own validity |
| **`grant/…/used/{door}`** — the use marker | No; the value is a timestamp (`src/agent/http/authz.rs:1460`) | No; only presence is read (`src/agent/http/authz.rs:1437`) | **Absent ⇒ unused ⇒ admitted** | None for a hostile tombstone | **Does not hold — open.** A tombstone re-arms the grant at every other node at once, and at the same node once its in-memory claim is swept or after a restart: the claim lives at most 60 s (`src/agent/http/authz.rs:1134`, `src/agent/gateway_authority.rs:384`–`392`, `src/agent/gateway_authority.rs:440`), the grant up to 3 600 s. No node-local record of a use exists besides the KV's own WAL. **For the plan:** the gateway legitimately tombstones this marker when a call is refused before dispatch (`src/agent/http/authz.rs:716`), so a signed marker needs a signed release |
| `grant/…/revoked` | No | No | **Present ⇒ the revoke route answers `already_revoked`** (`src/agent/http/grants.rs:808`) and the status route reports `revoked` (`src/agent/http/grants.rs:706`) | None | **Does not hold.** A planted marker stops a real revocation — the checkpoint is never written — and the grant stays live while its status reads revoked |
| `grant/…/refused` | No | No | — | None | **Outside: availability.** A forged refusal stops the agent |
| `call/{gw}/{term}/window` (I4) | *Specified:* the issuing gateway's node key, bound to the key's gateway segment (`docs/plans/identity-and-lifecycle.md`, row I4) | *Specified* | *Specified:* absent or forged ⇒ the cap-based floor | *Specified* | **Not on `main`.** Nothing writes or reads the `window` record yet (I4's PR 2; the `call/` family's checkpoint is the row below); the row is checked against this rule when it lands |
| `call/{gw}/{term}/checkpoint` (I4, PR 1a) | The issuing gateway's node key — the checkpoint's `authority` (`RevocationView::offer`, `src/mandate/authority.rs:250`) | The body's authority and scope equal the key's gateway and term, then the signature, **whatever the sequence** (`take_kv_copy`, `src/agent/gateway_authority.rs:308`). **Currency is checked only against this process's own view**: sequence applies within a process, a `call:` checkpoint is exempt from the start rule, and nothing is kept across a restart | Unparsed, misplaced or not the gateway's ⇒ treated as absent (the maintainer's decision, 2026-11-12) and counted `call_checkpoints_refused`. An older genuine copy over a newer one held ⇒ counted `call_checkpoints_stale`; no value where a copy was read ⇒ counted `call_checkpoints_missing`. A revocation already seen stays seen | `a_forged_misplaced_replayed_or_tombstoned_copy_of_a_calls_checkpoint_does_not_move_the_reader` (`src/agent/call_mandate_tests.rs`): forged at a new and at the held sequence, misplaced, unparsed, tombstoned, the genuine first checkpoint over the revoking one | **Partly.** (b) currency: a genuine older checkpoint replayed over the revoking one, or a tombstone, reads *not revoked* at a provider that has not yet read the revocation — one admitting later, one restarted, or one already running the call, where last-writer-wins drops the revocation behind the replay (pinned by the test's last two steps). (c) deletion is not as hard as forgery. A replay is counted `stale` only at a node that already holds the revocation. A provider the replay reaches before the revocation counts nothing and keeps running the call: one admitting later, one restarted, or one already running the call. A tombstone is counted `missing` wherever a verified copy had been read. Bound: the mandate's window (≤ `GRANT_TTL_CAP_SECS`, 3 600 s). To close it: the cancel-forward RPC that does not go through the KV (I4's PR 3, the primary cancel path) and the issuing gateway re-asserting its key (PR 2). No gateway writes the record yet ([ledger](../../operations/what-is-proven.md#call-checkpoint-replay)) |
| `mandate/{scope}` | — | — | — | — | **Outside: no decision.** Reserved; no reader or writer found |

### Audit and evidence

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `sys/audit/{node}/{seq}` — each record | The node's key | `seq` and `node_id` equal the key (`src/agent/audit.rs:595`); the hash link to the previous record (`src/agent/audit.rs:271`) | `malformed` / `unverified` counted | `an_audit_record_whose_key_and_body_disagree_is_ignored` (`src/lib_tests.rs:2964`), `a_forged_record_in_the_persisted_audit_stream_does_not_seed_the_chain_head` (`src/lib_tests.rs:8033`) | **Meets**, per record |
| The audit chain's **head** | **None.** No signed head and no external anchor; the head is the last record present | — | **A truncated tail, or a wholly deleted chain, verifies `Ok`** (`empty_chain_verifies`, `src/agent/audit.rs:917`) | None for truncation | **Does not hold.** After the owner restarts, the chain resumes at the truncated head and the loss is invisible in-cluster. `AuditSink` is a mirror that drops when full, not an anchor |
| `sys/audit-checkpoint/{node}/{seq}` | The node's key | `node_id` equals the key's (`src/agent/audit.rs:539`); `{seq}` is not compared | Skipped; not counted | None for a forged checkpoint (`a_peers_write_into_a_nodes_audit_stream_trips_the_tripwire`, `src/lib_tests.rs:2993`, is a tripwire test on the stream) | **Partly.** (b): `{seq}` is not compared with the body. (c) uncounted, (d) missing. A mid-chain boundary; it covers no tail |
| `audit/{ts}/{node}` (SkillRunner, without `compliance`) | No | No | — | — | **Outside: no decision.** No reader; it is not evidence |
| `sys/ae-tally/{node}/hb` · `…/c/{counter}/{actor}` | **No** — stated (`src/agent/fleet_history.rs:61`) | No | A missing counter entry counts 0 (`src/agent/fleet_history.rs:452`); a peer whose heartbeat says `enforces: false` is skipped (`src/agent/fleet_history.rs:520`) | Future-time forgeries only (`a_forged_future_time_dates_nothing`, `src/agent/fleet_history_tests.rs:2028`) | **Does not hold, stated.** Tombstoning a counter entry, or forging the heartbeat, permits past a fleet limit. Owner-side tripwire only |
| `sys/ae-exposure/{node}` | **No** — stated (`src/agent/exposure.rs:50`) | No | A tombstone reads *no summary* (unknown coverage, closed); a fresh forged summary is read as the node's | Unreadable, future and expired summaries only (`src/agent/exposure_tests.rs`) | **Does not hold, stated.** A forged summary under-counts and permits; one marking an out-of-view owner `departed` holds its digests fleet-wide (the module's own words) |

### Consensus, electorates and exclusivity

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `consensus/electorate-cert/{group}/…` with its `consensus/committed/electorate/…` record | The electors' signed votes (`certificate_holds`, `src/consensus.rs:2883`) — **but the electors are the member set the record itself names** (`src/consensus.rs:2276`); no configured authority | Slot, value digest and voter-in-electorate (`src/consensus.rs:2886`, `src/consensus.rs:2891`) | Refused, skipped and counted (`src/agent/electorate.rs:694`) | `a_forged_electorate_record_is_not_adopted` (`src/lib_tests.rs:16469`), `an_electorate_certificate_verifies_under_tls` (`src/lib_tests.rs:16554`) | **Partly.** (a): a genesis naming only its author certifies itself — `check_genesis` accepts any sorted, non-empty member set at epoch 1 (`src/agent/electorate.rs:495`–`521`) and adoption checks the certificate against that set. Confirmed from the code; no test. Also *inferred*: a tombstone after adoption is not re-examined, so a restarted node sees no electorate. Without `[tls]` the check is a shape |
| `consensus/life/{slot}/{ballot}` · `/s` · `/end/…` · `/v/{digest}` — the decision record, sentinel, release marker, value | No | The envelope's slot equals the key's (`src/consensus_life.rs:254`), the value's digest, and the marker's identity against the top record — all computable by any member; no signature | Malformed skipped; only a ballot above 2^32 is counted | `row16_a_forged_record_ballot_is_counted` (`src/agent/lock_lifecycle_api_tests.rs:274`) — counts it; the forged record is still the top | **Does not hold.** A planted record above the top names a holder; a marker forged from the top's public fields releases a lock or leadership (*inferred*). The design's stated limit applies: exclusivity is enforced at the resource, by its fencing token ([runtime-invariants](architecture/runtime-invariants.md) points back here) |
| `consensus/committed/{slot}` · `consensus/lease/{slot}` — the reading of **every slot with no decision record** | No | No (`legacy_live`, `src/consensus.rs:993`, reached from `src/consensus.rs:986`) | A tombstoned or malformed lease reads **permanent** (`src/consensus.rs:1001`, `src/consensus.rs:1004`) | `row10_a_legacy_decision_is_released_the_legacy_way` (`src/agent/lock_lifecycle_api_tests.rs:262`) shows a plain write is read | **Does not hold.** On any never-decided slot a plain write reads as a live, permanent decision — through `consensus_get` (`src/agent/consensus_handle.rs:128`) and the gateway's lock and leader routes (`src/agent/http/overlay.rs:771`, `src/agent/http/overlay.rs:795`, `src/agent/http/overlay.rs:831`) |
| `sys/consensus-accepted/{node}/{slot}` · `consensus/ballot/{slot}` · `consensus/decided/{slot}` — an acceptor's memory and the shared floors | No | No. `prewarm_accepted` inserts what decodes (`src/consensus.rs:4588`–`4605`); a floor above 2^32 is counted and **still obeyed** (`src/consensus.rs:1292`) | A foreign change to the acceptor's own record withholds the slot and is counted at the owner, and the **merged** record — forged values included — is rewritten under the owner's key (`src/agent/acceptor_guard.rs:264`–`270`); a tombstone is ignored by the guard (`src/agent/acceptor_guard.rs:306`) | `a_forged_decided_floor_is_counted_and_still_obeyed` (`src/lib_tests.rs:13718`); `test_sys_namespace_tripwire_flags_foreign_self_owned_write` (`src/lib_tests.rs:7767`) — detection | **Does not hold.** A forged promise or acceptance survives into the owner's next start; a tombstone or a lower overwrite of the durable record is undetected, and it is what a tombstoned shared floor falls back to |
| `consensus/trust/{group}/{node}` | No | No (`src/consensus.rs:1898`–`1903`) | Absent ⇒ **no vote filter** on a non-electorate group (`src/consensus.rs:534`–`539`) | None | **Does not hold** on non-electorate groups with `use_trust_slices`: a tombstoned slice lets votes from outside it count. Within an electorate the roster still filters |
| `sys/topology-override/{group}` | No | No (`src/consensus.rs:1696`) | No | None | **Does not hold.** `b"true"` disables a hard topology gate for the group **Outside: no decision** reader: `GET /gateway/govern`'s `topology_overrides` (`src/agent/http/govern.rs`) reports the groups whose value is `true`, as the gate reads it. |
| `grp/{group}/{node}` | No | No | A roster that disagrees with a declared electorate is refused and counted (`mycelium_electorate_roster_mismatch_total`) | None; tests use a foreign `grp/` write as setup | **Does not hold.** A peer's write flips the named node's own signal boundary (`mycelium-core/src/connection.rs:1070`) and changes a non-electorate group's quorum size |
| `sys/quorum/{kind}/{sender}` | No | No | No; excluded from the tripwire by design (`mycelium-core/src/connection.rs:27`) | None | **Does not hold.** Forged sender evidence is read live by `quorum_persistent` (`mycelium-core/src/kv_handle.rs:482`–`500`) and seeds `quorum()` at start (`src/agent/lifecycle.rs:1146`) |
| `sys/load/{node}/…` | No | No | No | The tripwire test only | **Bounded, not detected** by default (opacity routing; the `sys/` tripwire counts at the owner only). **Does not hold** with `count_opaque_as_absent`: `count_opaque_members_ctx` counts **entries**, not nodes (`src/agent/opacity.rs:113`–`124`), and the quorum is `raw_members − opaque_count`, floored at 1 (`src/agent/consensus_handle.rs:240`–`242`) — several forged keys under one member take a non-electorate quorum to 1 |
| `lock/{name}` (raw key) | — | — | — | — | **Outside: no decision.** Locks decide under `consensus/life/` |

### Governance intents and soft state

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `sys/govern/membership/{group}` | No | No; freshness is `now − written_at_ms` saturating (`src/agent/intent.rs:57`), so a future stamp never evaporates (*inferred*) | **Absent, undecodable or stale ⇒ the declared electorate minimum is 0** (`src/agent/helpers.rs:100`–`110`), and only an empty roster then refuses | None | **Does not hold.** A tombstone removes a group's partial-view floor; a forged `drain` makes the named node leave (`src/agent/membership_governor.rs:285`); a huge `min` refuses every election. The gateway route is audited, a KV write is not |
| `sys/govern/timing` | No | No | Values outside `1..=3600` and `1..=300` are ignored (`src/agent/timing_governor.rs:100`, `src/agent/timing_governor.rs:105`) | None | **Bounded, not detected** — the clamp at read is the bound |
| `sys/govern/fleet` · `sys/config/{param}` | No | No | **No range at read**: a fleet intent sets the master switch, floors and ceilings as given (`src/agent/tuning_governor.rs:374`–`391`); `sys/config/` advice is applied through the governor's gate and the local policy (`src/agent/cluster_tuner.rs:78`) | None | **Does not hold** as soft state: neither bounded nor detected. A local pin wins |
| `sys/rate/{observer}/{sender}` | No | No | No | None | **Outside: availability.** Forged observers throttle any sender to its floor (`mycelium-core/src/rate.rs:111`) |
| `sys/health/{node}` | No | — | — | — | **Outside: no decision** (a diagnostic field) |
| `sched/{id}/last/{node}` | No | No (`src/schedule.rs:356`–`374`) | An entry equal to the current slot suppresses the run as `already_ran`, one ahead as `last_ahead`, up to 64 slots (`src/schedule.rs:265`–`271`); absent ⇒ the slot runs | None; the planted-entry tests read it as clock skew | **Does not hold** for the repeat: a tombstone re-runs the current slot, and with `run_late` up to 64 slots (*inferred*). Suppression is availability **Outside: no decision** reader: `GET /gateway/schedules/last` (`src/agent/http/schedules.rs`) reports a gateway client's own schedule's entries as stored, without the loop's period, phase or ahead rules. |
| `sys/tuple/{node}/…/pressure/{stage}` · `tuple/inflight/…` | No | No (`mycelium-tuple-space/src/lib.rs:620`–`633`) | — | None | **Outside: availability.** Producers' `put` answers `Backpressure`; a future stamp is fresh for good |

### Capabilities, provisioning and what a model is told

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `cap/{node}/{ns}/{name}` — every advertisement, `authorized_callers` and `llm-meta` attributes included | No (`src/agent/capability_handle.rs:93`). `filter_protected` (H3, `src/mandate/protected.rs:133`) checks a grant and a possession proof bound to (node, ns, name) and **has no caller outside its own tests** | No reader checks a writer; the role check on `resolve_for_caller` uses the node in the key (`src/agent/capability_handle.rs:171`), and `resolve_filter_at` has none (`src/agent/capability_ops.rs:228`) — it serves the gateway's `/llm` route (`src/agent/http/llm_bridge.rs:143`). The writer chooses its own refresh interval, so a forged entry sets its own lifetime | No counter for a foreign `cap/` write | Only `filter_protected`'s unit tests | **Does not hold.** A fake or hidden provider for routing, the provisioner's floor and ceiling and schedule election; a forged `locality` on a live node's `llm-meta` (`mycelium-reason/src/route.rs:451`) routes restricted data off-domain (*inferred*). `authorized_callers` in an ad is a discovery filter only — enforcement is the provider's own configuration |
| `cap/{node}/locality/self` | No | Decoded into the `peer_localities` cache from whichever write won (`mycelium-core/src/store.rs:811`–`817`) | A malformed value is warned | None | **Bounded, not detected** — biases forwarding peer order and locality ranking (`src/agent/tasks.rs:441`, `src/agent/wiring.rs:229`); the hard topology gate reads localities from signed votes (`src/consensus.rs:4034`–`4047`), not this key |
| `req/{node}/{ns}/{name}` | No (`src/agent/demand.rs:31`) | No | — | None; `demand_snapshot_counts_unique_demand_and_providers` counts a foreign write | **Does not hold.** Forged demand installs a catalogued artifact (`mycelium-wasm-host/src/provisioner.rs:1065`; bounded by provenance, budget and self-election); a tombstone removes demand |
| `cap/{node}/prov-shed/{band}` | No — a plain advertisement (`mycelium-wasm-host/src/provisioner.rs:1200`) | No (`mycelium-wasm-host/src/provisioner.rs:1240`) | — | None | **Does not hold.** A band held above its ceiling, or shed below it; with forged providers, served only by fakes (*inferred*). `mycelium_artifact_presence_unranked_departures_total` is the one indirect count |
| `cap-group/{group}` · `gcap/{group}/{ns}/{name}/{contrib}` | No; a contributor is not checked against `grp/` | No (`src/agent/emergent_groups.rs:175`) | A tombstoned definition makes every emergent member leave and tombstone its own `grp/` entry (`src/agent/emergent_groups.rs:253`–`259`) | None | **Does not hold.** A forced join and projected capabilities fleet-wide; a tombstone shrinks a non-electorate group's roster, and so its quorum |
| `tools/{name}/{node}` | No | No: the first key wins for the tool list (`src/agent/http/mcp_bridge.rs:55`), for the provider a call goes to (`src/agent/http/mcp_bridge.rs:93`), and for the node a grant request names (`tool_provider`, `src/agent/http/grants.rs:560`–`563`) | — | None | **Does not hold.** A forged entry puts the forger's node into the request a supervisor signs, and its text in front of a model |
| `skills/{ns}/{name}/{node}/…` | No | No | — | None | **Does not hold.** The tool schema and A2A card text a model reads |
| `prompts/{ns}/{name}` | No; no principal in the key | Read on every invocation of the model skill (`src/agent/llm.rs:365`); its presence also decides that an A2A skill is refused as a prompt skill (`src/agent/a2a.rs:686`–`694`) | Absent ⇒ `template_not_found` | None | **Does not hold.** The skill's template rewritten fleet-wide; written once, never re-asserted. The A2A reader is availability only |
| `installable/{ns}/{name}/{hex}` | A trusted publisher (`verify_provenance`, `mycelium-wasm-host/src/catalog.rs:187`) over kind, artifact, requirements and `provides` — **`size_bytes` is outside the signature** (`mycelium-wasm-host/src/catalog.rs:154`–`168`) and gates the install budget (`mycelium-wasm-host/src/provisioner.rs:603`–`604`) | Enforced at `provenance_ok` (`mycelium-wasm-host/src/provisioner.rs:538`); the key is not compared with the body; a withdrawn, validly signed line replays. A stem's staging loop fetches every catalogued line of a declared kind by its size hint before any provenance check (`mycelium-wasm-host/src/stem.rs:505`–`518`) | A rejected line is traced, not counted; a tombstone erases a line (the librarian re-publishes its own). An empty `trusted_publishers` verifies nothing (`mycelium-wasm-host/src/provisioner.rs:286`; a stem refuses to start that way unless `accept_unsigned`) | `require_provenance_refuses_unsigned_but_installs_trusted_signed` (`mycelium-wasm-host/src/provisioner.rs:2110`), `provenance_binds_the_whole_entry_not_just_the_bytes` (`mycelium-wasm-host/src/catalog.rs:792`); none for a tombstone or a replay | **Partly.** (a) one deciding field unsigned; (b) key and currency; (c) uncounted; (d) forgery only |
| `mailbox/{target}/{kind}/{hlc}` | No; the sender is a field the writer sets (`src/agent/mailbox.rs:178`) | No | — | None | **Does not hold** where an application trusts the sender |
| `wiki/{group}/proposal/{id}` | No; `author` is self-declared (`mycelium-wiki/src/agent.rs:497`) | No | — | None | **Does not hold** for attribution. The mandate fence is at the store, not here |
| `comp/{node}/{ns}/…` | No | No | — | None | **Does not hold** for a guest that decides from its own state |
| `ckpt/…` · `ckptw/…` · `strands/…` — application prefixes (`APPLICATION_KV_PREFIXES`, `src/agent/http/authz.rs:1786`) | No | No | — | None | **Does not hold** for an agent that resumes from its checkpoint or session: a member can rewrite it, and so can any HTTP client holding `kv:write` — these prefixes are not refused at the raw routes |
| `svc/` · `agent/{node}/…` · `clog/` · `schemas/` · `manifest/` · `ui/` | No | — | — | — | **Outside: no decision** / availability. Turn and tool budgets are enforced from memory (`src/agent/state_machine.rs:425`), the KV keys are mirrors; a forged `clog/` cursor makes a consumer group skip or replay |

### Companions and log streams

| Record | (a) signed by | (b) verified: body = key, current | (c) failure and absence | (d) forged-record test | Status |
|---|---|---|---|---|---|
| `facts/{node}/{field}` (`mycelium-agentfacts`) | The node's trusted keys (`mycelium-agentfacts/src/crdt.rs:120`) | **The key is discarded** (`mycelium-agentfacts/src/crdt.rs:115`) and the map is filled by the signed body's field name; currency is a TTL only | Dropped, uncounted | `per_field_merge_lww_and_forgery_rejection` (`mycelium-agentfacts/src/crdt.rs:221`) | **Partly.** (b): an older genuine field copied to another key under the node's prefix overrides the current value without touching the owner's slot, within the TTL (*inferred*) |
| `cn/{requirement}` — the announcement head (`mycelium-commitment`) | No (`mycelium-commitment/src/lib.rs:615`) | No | Absent ⇒ `NotAnnounced` | None | **Does not hold.** The declarer's own signed close takes `deadline_ms` and the digest from this unsigned head (`mycelium-commitment/src/lib.rs:708`–`732`) |
| `log/cn/{requirement}/offers` · `closes` · `reports` · `assessments` | Closes: the declarer, where the caller passes its key; offers, reports and assessments as written | `select_close` takes the earliest close by its own `closed_at_ms`, never by stream key, and verifies under the given key (`mycelium-commitment/src/lib.rs:802`); offers, reports and assessments are read as they decode (`mycelium-commitment/src/lib.rs:672`, `mycelium-commitment/src/lib.rs:1140`, `mycelium-commitment/src/lib.rs:1176`) | — | None named here | **Partly** for closes (verified only when the caller pins a key); **does not hold** for the unverified readers. `select_close` decides which offers were in time |
| `cn/{requirement}/award` | The declarer the **caller pins** (`award_of_verified`, `mycelium-commitment/src/lib.rs:1114`) | Requirement and operation id equal the slot (`mycelium-commitment/src/lib.rs:1101`) | Dropped, uncounted | `award_of_verified_reports_no_award_the_pinned_declarer_did_not_sign` (`mycelium-commitment/src/lib.rs:1624`) | **Partly.** (c) uncounted; the plain `award_of` path (`mycelium-commitment/src/lib.rs:1093`) verifies nothing, and a tombstone of the award is untested |
| `log/reason/{run}/{node}/…` (`mycelium-reason` traces) | No | No (`mycelium-reason/src/trace.rs:240`) | — | None | **Untraced.** The replay reads what decodes; what decides on a replayed trace was not traced |
| `log/wiki/{group}/proposals` (durable wiki proposals, `src/mandate/restart.rs:122`) | No: the record carries a self-declared `proposer` and `term` and no signature (`Proposal`, `src/mandate/restart.rs:131`) | No reader outside tests: `Proposal::decode` is called by `src/lib_tests.rs` only | — | None | **Outside: no decision** until a reader exists. A curator that accepts from this stream must verify the proposer against the mandate, or the row becomes *does not hold*. Not readable or writable through the gateway's log routes (`OWNED_LOG_STREAMS`) |
| `rights/head/{holder}` | The holder's key **when one is held, otherwise an empty signature** (`mycelium-wasm-host/src/provisioner.rs:84`–`93`) | `verify_published_head` is called only by tests | — | — | **Outside: no decision** yet |
| `knowledge/head/{issuer}/{stream}` | — | — | — | — | **Outside: no decision.** Reserved; never written to the KV |

### Layer I

| Record | What holds | Status |
|---|---|---|
| A live `SignedData` frame | Signed by its writer; the signer must be the sender (`mycelium-core/src/connection.rs:1154`, counted `kv_signer_mismatch`); `a_signed_frame_whose_signer_is_not_its_sender_is_refused` (`src/lib_tests.rs:3601`). The key's owner is not compared | **Outside: Layer I** |
| An anti-entropy entry | **Names no writer and carries no signature** (`mycelium-core/src/framing.rs:220`, `mycelium-core/src/connection.rs:867`); applied by LWW under mTLS only; stated at `mycelium-core/src/config.rs:1724` | **Does not hold, stated** — and the reason every row above must verify at read. Closing it is a wire change, a separate decision |

### The six items the adoption named, as found

1. **The grant use marker** — unsigned, presence-only, absence admits: *does not hold, open*.
2. **The audit chain** — every record is signed and verified; no signed head or external anchor, and a truncated or
   deleted chain verifies: *does not hold*.
3. **`cap/`, `req/`, `prov-shed/` advertisements** — unsigned, no reader checks a writer; H3's `filter_protected`
   has no caller: *does not hold*.
4. **`sys/ae-exposure/` and the tally** — unsigned, as the plans record; a tombstone or forged summary permits past
   a limit: *does not hold, stated*.
5. **A caller envelope's key-less principal** — bound only to the framing node's key; any member can assert it, and
   a bare frame's `sender` is bound to nothing: *does not hold*.
6. **Anti-entropy entries** — name no writer: *does not hold, stated*, by Layer I's design.

The rows above that bound a claim on `docs/operations/what-is-proven.md` are on that page's § *Not yet shown*,
dated 2026-11-12: these six items' (the use marker, audit truncation, the tally and exposure, H3, the key-less
principal), the self-certified genesis, and a forged decision record against the lock claim. That page was read
for each *Does not hold* and *Partly* row; it makes no claim about member removal, so the restart case of
`sys/membership/removed/` has no row there. The check was by reading, not by a script.

## Using this page

- **A new record a decision reads** is designed to (a)–(d), or lands with a row here whose Status says which
  condition does not hold.
- **A review** asks the question of every KV read the change makes (`.claude/commands/adversarial-review.md`,
  check 8).
- **A row's Status moves to Meets** by citing the verifying code and the forged-record test, in the PR that builds
  them.
- The lint pass re-verifies the citations (`/wiki-lint`, check 1); nothing in CI does.
- **Where the rows are planned:** [`docs/plans/external-critique-2026-11-12.md`](../../plans/external-critique-2026-11-12.md)
  (proposed 2026-11-12) groups every *Does not hold*, *Partly* and *Bounded, not detected* row into rows CR2–CR5
  and CR7, and says which need a KV value change, an on-disk change or a wire change. A row built there moves
  its Status here. **From its review (#773): a signature does not meet (c) for a record whose *presence* cancels**
  — a member's tombstone or garbage value planted before the genuine record wins LWW, and the owner's own write
  still returns `Ok` (`src/agent/state_machine.rs:981`–`983` discards a `Superseded` receipt). Such a record
  needs a node-local durable copy read first, *anything but never-written reads present*, and a check of the
  write's receipt.

Log: [`.log/2026-11-12-owner-signed-rule.md`](.log/2026-11-12-owner-signed-rule.md).
