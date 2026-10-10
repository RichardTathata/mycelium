# Post-360 hardening — the next batch, and consensus as a protocol, not a service

**Status:** adopted 2026-10-10, rev 1.1. **R0 is being cut as v2.32.0 (2026-10-10)**, which delivers rows **G, P3, C, D
and P1** — each closed on evidence in §3.5. Open: Φ, P2 (#601), P5, A with C1/C2 (#600), B (#602), E, F, H, I, J, and
R0's private re-pin. It follows the 2026-10-09 360 review and the eleven
PRs that addressed its must-do list (#582–#592, private #28); those are recorded in the CHANGELOG and are not
repeated here. Rows close on evidence (verification policy rule 2): a row is closed by quoting each promise next to
the code or test that delivers it, and a promise that is not delivered is written *not built*.

## 1 · Why one plan

After the 360 review's must-do list was fixed, three sources were left with open items: the review's own lower-tier
findings, the independent adversarial reviews of the fix PRs (each recorded a stated limit or a follow-up), and an
external review of 2026-10-10. This plan puts them in one order, and records one decision about consensus that
shapes several rows (§2).

Every row follows the standing rules: the regression test is written first and seen to fail on the unfixed code;
every entry point that reads or writes the changed thing is enumerated, with the grep; an independent adversarial
review runs before merge; a behaviour change carries a CHANGELOG entry and, where a client can notice, an upgrade
note.

**Out of scope, by decision:** the 100-node nightly scale runner (V1 in [`what-is-proven.md`](../operations/what-is-proven.md)).
It needs a machine rather than a commit, and is deferred, not dropped.

## 2 · Consensus is a protocol, never a service

The question (2026-10-10): is a full Paxos at Layer III counter to the philosophy? **No.** [`philosophy.md`](../philosophy.md)
§ *Anderson — More Is Different* already allows it: an emergent layer adds laws of its own level — "ballots,
quorums, roles, listeners, an explicit lifecycle — protocol machinery is what protocols are made of" — and may hold
transient broken-symmetry states. Coase's firm is the analogy: coordinators emerge where coordination pays and
dissolve when it stops paying. The 2.30.0 prepare phase and the durable acceptor record are inside those bounds.

The constraint is the corrected litmus's *may never* column, not how rich the protocol is. The line this plan draws
explicitly:

- **A consensus protocol is legitimate.** Whichever nodes participate in a group run it; roles form per ballot; the
  coalition dissolves when the decision completes; its state is ordinary keys and signals; Layers I and II know
  nothing of it.
- **A consensus service is the coordinator trap.** A fixed tier of nodes that everyone else must reach in order to
  agree — the etcd or ZooKeeper shape — is "symmetry breaking baked into the laws": permanent, privileged and
  load-bearing. No deployment shape, configuration default or documentation may make a named node set the place
  agreement happens.

Four places strain that line today, and the rows in §3 address each:

| Strain | Why it crosses or approaches the line | Row |
|---|---|---|
| `elect_leader` commits permanently, with no lease and no release path; a dead leader is reported for ever | a role that escapes evaporation | C1 |
| acceptor memory is never collected (and since #585 is durable) | an output that does not decay ("mandate TTL applies to decisions too") | C2 |
| item 2, a fixed electorate for safety-sensitive consensus, could harden into "these nodes are the consensus nodes" | a designed-in privileged node set | P2 |
| the forged-floor finding tempts a Layer I write guard on `consensus/` | the substrate learning a Layer III law | held: #591 added tripwires above, no guard below |

## 3 · The rows

### 3.1 · Before the batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **R0** — *being cut: v2.32.0* | Release **2.32.0** once the merge train (#582–#592) is merged | the release runbook (`RELEASING.md`), step 2b on the branch released from; upgrade notes checked as one set (the deprecation windows from #585 and #587, `SystemStats`/`ConsensusResult` fields, `/api/tuple`, `rpc/respond`, `mycelium-reason` 0.8.0, the three SDK versions); the private companion re-pinned with `COMPATIBILITY.md` updated | the train |
| **Φ** | Philosophy: "protocol, not service" | a paragraph in the corrected-litmus section stating §2's line, and the deployment rule that no shape may make a named node set the place agreement happens; the wiki's runtime-invariants page cites it | — |

### 3.2 · The external review's items (2026-10-10)

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **P1** — *delivered 2.32.0 (#598), §3.5* | Refuse an exposed, unauthenticated gateway | `start()` refuses by name when the gateway binds a non-loopback address with no credential model (no token, no token table, no `[oidc]`), unless an explicit insecure opt-in is set; the opt-in warns once at start; the guarantee report and the secure profile reflect it; loopback development is unchanged | R0 |
| **P2** | A consensus electorate is a governed group | safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities; versioned electorates with joint-consensus transitions are recorded as protocol work for a later plan, not built here. **Two tensions P2 must resolve** (found while recording the decision, `docs/design/consensus-electorate.md`): (a) *governed is not fixed* — the opt-in `MembershipGovernor` exists to resize a governed group, a membership intent lapses after 5 minutes without re-publication, and the electorate floor after 30 s, so P2 must define an electorate group that does not resize while a slot is open (or exclude the governor from electorate groups); (b) the current §7 profile names voters by node identity (`declare_trust(group, &[NodeId])`), which D1 rules out as the *name* of an electorate — decide whether trust slices stay underneath as the vote filter. The gateway-only governance boundary (an embedded `join_group` or a `grp/` write still moves a governed group) is part of P2's scope. | R0 |
| **P3** — *delivered 2.32.0 (#596), §3.5* | `#![deny(unsafe_code)]` in `mycelium-core` | the one production site (`erasure.rs`'s volatile key wipe) replaced by `zeroize` (already in the tree) or carried as a single scoped `#[allow]` with its justification; test-only `set_var` sites scoped likewise | — |
| **P5** | Split `src/agent/http.rs` (8,394 lines) by route family | behaviour-preserving; one shared authorisation-and-evidence layer every family goes through; the cross-door tests (the C7 bypass matrix, the public-surface check, the scope table) unchanged and green; the lock-order table and `required_scope` still complete | R0; before row E |

### 3.3 · The batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **A** | Locks and leases (with C1, C2) | a released lock whose tombstone was garbage-collected is not re-committed to its old holder; a reopened lease does not adopt the expired holder's value before the decided floor arrives; a late COMMIT does not re-stamp a released lock fleet-wide; **C1** `elect_leader` is lease-based by default with a release path (permanence stays available, opt-in); **C2** acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on | #585, #591 merged |
| **B** | Core resource bounds | a handshake and idle timeout on the gossip accept path; connect, write and flush timeouts on the outbound writer, and no unbounded pile of anti-entropy replies behind a stalled peer; one stalled SSE or serve subscriber cannot veto admission of its kind node-wide; the signal log is bounded per sender-chosen kind; a stopped agent leaks no task context (`RpcRequestRx` ends at shutdown, as its doc says) | R0 |
| **C** — *delivered 2.32.0 (#597), §3.5* | Companion WALs (tuple space, blackboard) | compaction is crash-durable (temp file and directory fsynced); a torn or undecodable record poisons the writer and is refused at open rather than silently dropping every later record; one owner per file | R0 |
| **D** — *delivered 2.32.0 (#599), §3.5* | WASM execution limits | epoch interruption bounds a guest call; guest code runs off the runtime's worker threads; a trap does not recompile the component on every call | R0 |
| **E** | Gateway remainder | client-supplied timer periods (`interval_secs`) bounded; the log-group subscribe keeps the core's `(hlc, key)` tiebreak the mailbox deletes an entry only after the client has it (today it tombstones on entering the 256-slot channel, so a dropped stream loses queued events — #595's review); prompt keys cannot alias across `ns`/`name` (`prompts/{ns}/{name}` with a `/` in either); and saves the group offset only after the entry is sent (today it saves first, so an entry in flight when a stream drops is never re-sent — found by row G); the caller envelope binds freshness | P5 |
| **F** | Wire and crypto, one MINOR with a window | a TLS node refuses unsigned `Data` frames; the removal check reads a peer's key structurally, not by byte-scanning the certificate; KV `SignedData` is domain-tagged; each change carries a mixed-fleet window and an upgrade note in the 2.30.0 shape; the #585 and #587 windows close in the same MINOR | R0, ideally its own release |
| **G** — *delivered 2.32.0 (#595), §3.5* | SDKs | Python whole-second timeouts and a non-truncating tuple `take`; path segments escaped; TypeScript `SupersededError`; one `scatterGather` default, documented; SSE reconnect is designed with a gateway resume point or recorded as not built | R0 |
| **H** | Companions | the commitment deadline judged on the log's clock, not the offerer's; `award_of` verifies before it reports; the reason blob store bounded; caller-chosen `run_id` validated | R0 |
| **I** | Private repo P3s | RA5 correction chains and competing corrections; attempt keys carry the operation id; a lock-order table; the outbox directory fsynced at create; Cedar policies schema-validated at load; the CI pin check refuses `[patch]`, `rev =` and multi-line tables | R0's re-pin |
| **J** | Hygiene | the sim-seam gate's test-skip bug (the one the KV gate fixed in #591); every feature and bench linted or built by some CI step; the two CI jobs on `stable` pinned; unreferenced public items deprecated, not removed (nothing is removed on the 2.x line); only the duplicates that disagree are unified (`hex_line`'s whitespace, `now_ms` copies outside the seam) | — |

### 3.4 · Documentation follow-through

A row is not closed until the documents that describe its subject say what the code now does. Several pages record a
position as *planned* or *not built* (the consensus electorate decision record `docs/design/consensus-electorate.md`,
the proof ledger, the threat model, the guide, the FAQ, the wiki); each must be flipped in the same PR that delivers
the change, or the row stays open. In particular:

| When this lands | Update |
|---|---|
| **P2** (governed electorate required) | the decision record's *enforced today* section; `what-is-proven.md`'s "Agreement across an electorate change" row (move what is now gated); `threat-model.md` §7; `guide/04-consensus.md` § *Discovery is not an electorate*; the FAQ answer; the wiki runtime-invariants page |
| **C1** (leased leadership) and **C2** (acceptor state decays) | the decision record and philosophy's "protocol, not service" paragraph (strike *not built*); `guide/04-consensus.md` on `elect_leader`; `runtime-invariants.md` § *Acceptor memory* |
| **P1** (exposed-gateway refusal) | README's and `positioning.md`'s default-posture sentence; `production-readiness.md`; `configuration.md`; the guarantee catalogue golden |
| **F** (wire and crypto MINOR) | `deprecations.md` (close the #585/#587 windows); the threat model's Boundary A; `what-is-proven.md`'s KV `SignedData` residual |
| **every row** | the CHANGELOG entry, a dated wiki `.log/` entry, and any `what-is-proven.md` row the change moves between tables |

The next `doc-coverage` run after each MINOR checks this table: a page still saying *not built* for a delivered row is a
calibration entry.

**P1's row, checked at the cut (2026-10-10):** README's default-posture sentence (§ *Security*: "off loopback it refuses
to start without a credential, unless `gateway_allow_unauthenticated` says otherwise") and `positioning.md`'s ("a
non-loopback one refuses to start without a credential unless `gateway_allow_unauthenticated` is set") — done in #598;
`production-readiness.md` (rev 3's `gw.exposed_closed`, the refusal naming `http_addr`, the opt-in) — done, its
*unreleased* markers moved to 2.32.0 at the cut; `configuration.md` (`http_addr`, `gateway_auth_token`,
`gateway_allow_unauthenticated`, both token tables) — done, likewise re-marked; the guarantee catalogue golden
(`docs/reference/guarantee-catalogue.md` and `.json` carry `gw.exposed_closed`; regenerated and gated by the catalogue
test) — done. Every row: the CHANGELOG entries and dated wiki `.log/` entries landed with each PR.

### 3.5 · Delivered in 2.32.0 — each promise beside its evidence

Read against the merged code on `main` at `6b675720`, not against the PR descriptions.

**G — SDKs (#595, with #583 underneath).**
- *Python whole-second timeouts* — `mycelium-py/src/mycelium/_pool.py` `whole_seconds` (rounds up, never below 1,
  refuses negative/non-finite/past-`u64` by name); `mycelium-py/tests/test_sdk_edges.py`
  `test_a_mesh_timeout_is_sent_in_whole_seconds_rounded_up`, `test_wiki_ingest_sends_whole_seconds`,
  `test_a_timeout_the_gateway_cannot_read_is_refused_by_name`, `test_a_lease_or_ttl_is_sent_in_whole_seconds`.
- *A non-truncating tuple `take`* — `test_a_tuple_take_does_not_truncate_its_park` (Python);
  `mycelium-ts/tests/sdk_edges.test.ts` "tuple take(%p) parks for %p whole seconds" (TypeScript).
- *Path segments escaped* — `test_a_caller_supplied_value_stays_one_path_segment`,
  `test_a_segment_that_cannot_travel_is_refused_before_any_request`, `test_update_prompt_escapes_its_segments`;
  the TypeScript "keeps %p one path segment" and "a server-issued handle or guard id is escaped too";
  `langgraph-checkpoint-mycelium/tests/test_blob_path.py` (the blob id that redirected the bearer, dot and
  non-string ids read as `corrupt`).
- *TypeScript `SupersededError`* — `mycelium-ts/src/agent.ts` `export class SupersededError`; "%s that lost throws
  SupersededError" and its plant "%s: other refusals are not SupersededError".
- *One `scatterGather` default, documented* — `mycelium-ts/src/agent.ts` `min_ok: options.minOk ?? 1` with 10 s, its doc
  comment, both READMEs; "scatterGather waits for one reply by default…" and the Python
  `test_scatter_gather_waits_for_one_reply_by_default`.
- *SSE reconnect designed with a resume point, or recorded as not built* — **recorded as not built**: guide 10
  § (line 355, "A dropped stream is not resumed — not built: the gateway has no resume point"),
  `mycelium-py/README.md` and `mycelium-ts/README.md` say the same and what each stream loses. The resume point
  itself is gateway work (row E).

**P3 — `#![deny(unsafe_code)]` in `mycelium-core` (#596).**
- *The attribute in core* — `mycelium-core/src/lib.rs:14`; also every companion library (`-sim`, `-blackboard`,
  `-wiki`, `-wasm-host`, `-agentfacts`, `-reason`, `-guardrails`, `-effects`, `-commitment`, the coop library);
  the root crate and `-tuple-space` already had it.
- *The one production site replaced by `zeroize`* — `mycelium-core/src/erasure.rs` (`dek.zeroize()`, no
  `write_volatile` left); `destroy_wipes_the_key_it_removes`, and `install_key_wipes_the_key_it_replaces` for the
  replaced-key path the PR found.
- *Test-only `set_var` sites scoped* — the workspace's only `#[allow(unsafe_code)]`s are
  `mycelium-core/src/config.rs` `set_test_env` (inside `#[cfg(test)] mod tests`, takes the `env_test_lock()` guard)
  and one statement in `mycelium-wasm-host/src/stem.rs`'s test module, each with its `SAFETY:` comment
  (`grep -rn 'allow(unsafe_code)' --include='*.rs'`). Outside the promise and stated: `examples/conway.rs` still uses
  `unsafe`.

**C — Companion WALs (#597).**
- *Compaction is crash-durable (temp file and directory fsynced)* — tuple space
  `compaction_syncs_the_directory_after_the_rename`, blackboard `compaction_syncs_the_temp_file_and_the_directory`;
  and beyond the promise, compaction folds the log under the WAL lock
  (`a_compaction_between_append_and_apply_keeps_the_acknowledged_put` / `_post`) and keeps the id high-water mark
  (`a_restart_never_reuses_an_acked_id`).
- *A torn or undecodable record poisons the writer and is refused at open rather than silently dropping every later
  record* — a failed append poisons (`a_failed_append_never_strands_a_later_acknowledged_put` / `_post`; on a
  secondary, `a_poisoned_mirror_repairs_and_keeps_every_later_record`); a whole frame that does not decode, with data
  after it, refuses the open with the file untouched (`a_corrupt_middle_record_refuses_the_open_and_leaves_the_file`);
  a frame the file ends inside is still a crash's torn tail and is truncated (`a_valid_kind_followed_by_zeros_is_a_torn_tail`).
  **Not built, stated in the CHANGELOG:** a length prefix corrupted to run past the end of the file still reads as a
  torn tail (the format has no checksum), and there is no `on_unreadable = "quarantine"` counterpart.
- *One owner per file* — `mycelium::OwnershipLock` on `<wal>.lock` (`mycelium-tuple-space/src/store.rs`,
  `mycelium-blackboard/src/wal.rs`); `a_second_owner_of_the_wal_is_refused` in both crates.

**D — WASM execution limits (#599).**
- *Epoch interruption bounds a guest call* — `mycelium-wasm-host/src/host.rs` `cfg.epoch_interruption(true)`,
  `DEFAULT_CALL_DEADLINE` (5 s), `with_call_deadline`, `[hosts].call_deadline_ms`;
  `mycelium-wasm-host/tests/e2e.rs` `a_guest_call_past_its_deadline_is_stopped_by_name`.
- *Guest code runs off the runtime's worker threads* — `runtime.rs` (`spawn_blocking` around each call, and install's
  compile, instantiation and `describe`); `provisioner.rs`
  `a_long_guest_call_does_not_block_another_task_on_a_current_thread_runtime`.
- *A trap does not recompile the component on every call* — a trapped instance is replaced from the install's compiled
  component (`WasmHost::compiles()`); `a_trapping_guest_called_repeatedly_compiles_once`.

**P1 — Refuse an exposed, unauthenticated gateway (#598).**
- *`start()` refuses by name when the gateway binds a non-loopback address with no credential model, unless an explicit
  insecure opt-in is set* — `src/agent/lifecycle.rs` (`GatewayExposure::Refused` → `InvalidField { field: "http_addr" }`,
  naming the credentials and the opt-in), `guarantee::gateway_exposure` / `gateway_credential_model`;
  `src/lib_tests.rs` `an_exposed_gateway_with_no_credential_refuses_to_start` (`0.0.0.0` and `::`, and blank tokens
  refused at `validate()`: `mycelium-core/src/config.rs` `a_blank_gateway_token_is_refused_in_every_form`,
  `src/agent/http.rs` `resolve_token_never_matches_an_empty_presented_bearer`).
- *The opt-in warns once at start* — `lifecycle.rs` (`GatewayExposure::Waived` → one `tracing::warn!` in `start()`);
  the opt-in starting and classifying as `Waived` is tested
  (`an_exposed_gateway_starts_with_a_credential_on_loopback_or_with_the_opt_in`); **the warning itself is not asserted
  by a test.**
- *The guarantee report and the secure profile reflect it* — `gw.exposed_closed` (`guarantee.rs`), required by
  `secure-single-domain` **rev 3**; `the_secure_profiles_required_set_is_pinned`,
  `the_secure_profile_names_the_unauthenticated_opt_in`, and the report states in the start test above.
- *Loopback development is unchanged* — the `127.0.0.1` case of the start test (`not_applicable`) and
  `loopback_is_127_slash_8_and_colon_colon_1_and_nothing_else`.

## 4 · Decisions taken

| # | Decision | Recorded |
|---|---|---|
| D1 | Consensus is a Layer III protocol and never a service (§2) | 2026-10-10 |
| D2 | Unreferenced public items are deprecated, not removed | 2026-10-10 |
| D3 | No blanket dedupe; only disagreeing duplicates are unified | 2026-10-10 |
| D4 | SSE reconnect and the wire/crypto changes are features with windows, not patches | 2026-10-10 |
| D5 | The scale runner is deferred | 2026-10-10 |
| D6 | P5 lands before row E, so gateway fixes are written once | 2026-10-10 |

## 5 · Open questions

- Should a consensus `Timeout` result carry its cause (today only a metric label)? Doc-coverage run 23, code gap 1.
- Should the MCP bridge and LLM backend clients get timeouts? PR #590's review.
- Versioned electorates with joint-consensus transitions: a later plan, under D1.

## 6 · Recorded limits that stay limits

Single-decree safety rather than a proof of the whole protocol; DNS rebinding outside the egress claim; a member that
*signs* consensus messages at the ballot ceiling; a forgery that reaches a node only through anti-entropy before it
holds its own record.
