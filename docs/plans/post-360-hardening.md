# Post-360 hardening — the next batch, and consensus as a protocol, not a service

**Status:** adopted 2026-10-10, rev 1.4 (2026-10-12: Φ closed). **R0 is being cut as v2.32.0 (2026-10-11)**, which delivers rows **G, P3, C, D,
P1, A (with C1, C2), B (as narrowed 2026-10-11) and P2 (without migration, as decided 2026-10-11)** — each closed on
evidence in §3.5. Open:
P5, E, F, H, I, J, and R0's private re-pin (P2's migration — close, enumerate, carry, complete — delivered after 2.32.0, §3.5;
Φ delivered 2026-10-12, #607, §3.5). It follows the 2026-10-09 360 review and the eleven
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
| **Φ** — *delivered 2026-10-12 (#607), §3.5* | Philosophy: "protocol, not service" | a paragraph in the corrected-litmus section stating §2's line, and the deployment rule that no shape may make a named node set the place agreement happens; the wiki's runtime-invariants page cites it | — |

### 3.2 · The external review's items (2026-10-10)

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **P1** — *delivered 2.32.0 (#598), §3.5* | Refuse an exposed, unauthenticated gateway | `start()` refuses by name when the gateway binds a non-loopback address with no credential model (no token, no token table, no `[oidc]`), unless an explicit insecure opt-in is set; the opt-in warns once at start; the guarantee report and the secure profile reflect it; loopback development is unchanged | R0 |
| **P2** — *delivered 2.32.0 without migration (#601), §3.5* | A consensus electorate is a governed group | safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities; versioned electorates with joint-consensus transitions are recorded as protocol work for a later plan, not built here. **Two tensions P2 must resolve** (found while recording the decision, `docs/design/consensus-electorate.md`): (a) *governed is not fixed* — the opt-in `MembershipGovernor` exists to resize a governed group, a membership intent lapses after 5 minutes without re-publication, and the electorate floor after 30 s, so P2 must define an electorate group that does not resize while a slot is open (or exclude the governor from electorate groups); (b) the current §7 profile names voters by node identity (`declare_trust(group, &[NodeId])`), which D1 rules out as the *name* of an electorate — decide whether trust slices stay underneath as the vote filter. The gateway-only governance boundary (an embedded `join_group` or a `grp/` write still moves a governed group) is part of P2's scope. **The maintainer's decisions (2026-10-11), recorded on #601:** after its round-4 review, **freeze membership** — an electorate group's members never change once its genesis is decided (the live one-member steps with a drain before each, rounds 2–3, were removed); and after its round-5 review, **P2 ships in 2.32.0 without migration** — changing an electorate's members (retire into a successor that carries its decisions, built in rounds 4–5) is not built in that release; **delivered after it** as a migration that terminates — close, enumerate, carry, complete — with the default moving by retirement (`docs/design/consensus-electorate.md` §8.3, §4.2). | R0 |
| **P3** — *delivered 2.32.0 (#596), §3.5* | `#![deny(unsafe_code)]` in `mycelium-core` | the one production site (`erasure.rs`'s volatile key wipe) replaced by `zeroize` (already in the tree) or carried as a single scoped `#[allow]` with its justification; test-only `set_var` sites scoped likewise | — |
| **P5** | Split `src/agent/http.rs` (8,394 lines) by route family | behaviour-preserving; one shared authorisation-and-evidence layer every family goes through; the cross-door tests (the C7 bypass matrix, the public-surface check, the scope table) unchanged and green; the lock-order table and `required_scope` still complete | R0; before row E |

### 3.3 · The batch

| Row | Scope | Promises | Depends on |
|---|---|---|---|
| **A** — *delivered 2.32.0 (#600), §3.5* | Locks and leases (with C1, C2) | a released lock whose tombstone was garbage-collected is not re-committed to its old holder; a reopened lease does not adopt the expired holder's value before the decided floor arrives; a late COMMIT does not re-stamp a released lock fleet-wide; **C1** `elect_leader` is lease-based by default with a release path (permanence stays available, opt-in); **C2** acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on | #585, #591 merged |
| **B** — *delivered 2.32.0 as narrowed (#602), §3.5* | Core resource bounds | a handshake and idle timeout on the gossip accept path, and read-progress bounds with a 1 KiB/s floor — **the accept path is bounded against silent and slow connections; an attacker that keeps talking can hold the connection slots it obtained from free permits**, mitigated by mTLS (a non-member fails the handshake within `handshake_timeout_ms`) and an opt-in per-source cap (narrowed by the maintainer's decision, 2026-10-11, after review rounds 3–5 of #602 found connection preemption could displace honest peers); connect, write and flush timeouts on the outbound writer, and no unbounded pile of anti-entropy replies behind a stalled peer; one stalled SSE or serve subscriber cannot veto admission of its kind node-wide; the signal log is bounded per sender-chosen kind; a stopped agent leaks no task context (`RpcRequestRx` ends at shutdown, as its doc says) | R0 |
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

**P2's row, checked at the cut (2026-10-11):** the decision record's *What is enforced today* (§3) carries a
*Since P2* block — the electorate requirement, identity pinning and freezing, nothing resizing an electorate group, the
cross-group refusal, the profile — and its §4.1 table, which still read *not built* for P2, now reads *delivered in
2.32.0, without migration* (fixed at the cut); `what-is-proven.md` moved "Agreement across an electorate change" out of
the third table, adding the gated row to the first and "Changing an electorate's members — not built" to the third
(its *unreleased* marker moved to 2.32.0 at the cut); `threat-model.md` §7 names the electorate group and
`consensus_require_electorate` as the supported profile; guide 04 § *Discovery is not an electorate* states the
requirement and the frozen membership; the FAQ's electorate answer likewise; `runtime-invariants.md` § Layer III has
the *Since P2* paragraph — all in #601 except the two markers fixed here.

**C1/C2's row, checked at the cut (2026-10-11):** the decision record `consensus-electorate.md` (*Roles and outputs
decay* — "rows C1, C2 — **delivered**, PR #600", and its §4.1 table); philosophy's "A protocol, not a service" paragraph
("post-360 plan rows C1 and C2, delivered" — no *not built* left for them; P2's *not built* stayed, correctly, until #607 struck it once the migration had landed, #606); guide 04
(`elect_leader` "**Leased** (30 s by default, since 2.32.0)", `elect_leader_with`, `release_leadership`);
`runtime-invariants.md` § *Acceptor memory* ("shrinks to a node-owned floor once its decision is over", C2's exact
condition, the code named) — all done in #600.

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

**A — Locks and leases, with C1 and C2 (#600; design record `docs/design/lock-lifecycle.md`).** The interleaving tests
withhold real frames with the test-only `test-frame-filter` (enabled in CI's default lib run and in `make check-full`).
- *A released lock whose tombstone was garbage-collected is not re-committed to its old holder* — the decision record
  ends by a release marker no record write can remove (`src/consensus_life.rs`); `row01_a_released_lock_is_not_recommitted_after_collection`
  (`src/agent/lock_lifecycle_tests.rs`), and `growth_a_thousand_acquisitions_leave_constant_keys`.
- *A reopened lease does not adopt the expired holder's value before the decided floor arrives* —
  `row02_a_reopened_lease_does_not_adopt_the_expired_value`; an adopter re-commits the holder's window, never its own
  (`row22_an_adoption_keeps_the_holders_lease`); a legacy value adopted in flight is wrapped in a lease
  (`f1_a_legacy_released_permanent_is_not_resurrected`).
- *A late COMMIT does not re-stamp a released lock fleet-wide* — `CommitTerm` carries the whole envelope and an upgraded
  learner ignores the legacy `Commit`; `row03_a_late_commit_does_not_revive_a_released_lock`,
  `row07_a_legacy_restamp_of_committed_is_not_read`, `row23_a_late_commit_does_not_displace_a_permanent_decision`.
  **Stated, not closed:** an *older* reader still reads the legacy keys the old way until it is upgraded.
- **C1** *`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)* —
  `DEFAULT_LEADER_LEASE` (30 s), `release_leadership` (`src/agent/consensus_handle.rs`), `elect_leader_with(group,
  LeaderTerm::Permanent)`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}`;
  `c1_elect_leader_is_leased_by_default_with_a_release_path`, `c1_a_dead_leaders_lease_lapses_and_a_new_election_succeeds`,
  `row08_a_release_ends_every_renewal_of_its_lineage`. **Not built:** an SDK release verb (row G's follow-up).
- **C2** *acceptor state is collected once its decision or lease is over, without weakening the promise a live slot
  depends on* — `run_acceptor_collector` / `collect_finished` / `shrink_acceptor` (`src/consensus.rs`);
  `c2_acceptor_state_shrinks_to_a_floor_once_its_decision_is_over`, `c2_the_shrink_rechecks_the_promise_inside_its_compare_and_set`,
  `c2_collection_is_bounded_per_pass`, `row27_collection_keeps_a_node_owned_floor`. **Not built:** collection on a node
  that proposes without a listener.

**B — Core resource bounds, as narrowed by the maintainer on 2026-10-11 (#602).**
- *A handshake and idle timeout on the gossip accept path, and read-progress bounds with a 1 KiB/s floor* —
  `handshake_timeout_ms`, `inbound_idle_timeout_secs`, `peer_read_stall_timeout_ms`, `peer_min_rate_bytes_per_sec`
  (`mycelium-core/src/config.rs`, `stall.rs` `StallGuard`); `idle_sockets_that_never_speak_are_closed_and_a_new_peer_connects`,
  `a_one_byte_trickle_is_closed_by_the_rate_floor`, `one_byte_then_silence_is_closed_with_the_floor_off`,
  `a_frame_that_stalls_mid_body_is_closed_and_counted_apart`, `a_large_first_frame_trickling_steadily_is_read_past_the_handshake_bound`
  (`src/resource_bounds_tests.rs`); `a_header_claiming_a_large_frame_does_not_reserve_it_upfront` (`framing.rs`).
- *The accept path is bounded against silent and slow connections; an attacker that keeps talking can hold the slots it
  obtained from free permits, mitigated by mTLS and an opt-in per-source cap* — the residual is stated on
  `max_connections` in `configuration.md`, in the CHANGELOG and on `what-is-proven.md`'s third table; the cap:
  `max_connections_per_source`, `the_per_source_cap_closes_a_second_connection_from_one_address`. The mTLS mitigation is
  the existing handshake under `handshake_timeout_ms`; no new test asserts a non-member's handshake failing in time.
- *Connect, write and flush timeouts on the outbound writer* — `WriterTiming` (`mycelium-core/src/writer.rs`: connect +
  TLS under `handshake_timeout_ms`, writes and flush under `peer_write_stall_timeout_ms`, 1 140 s, from the measured
  760 s worst chunk, `worst_case_chunk_apply_with_an_fsync_wal`, `#[ignore]`d, a measurement);
  `a_stalled_write_times_out_and_the_queue_drains`, `a_peer_that_never_reads_does_not_block_eviction` / `…_shutdown`,
  `a_slow_reader_that_keeps_reading_receives_a_large_frame`. **No dedicated test** for the connect timeout itself
  (code: `writer.rs`, the `Connect … timed out` branch).
- *No unbounded pile of anti-entropy replies behind a stalled peer* — one reply in flight per peer;
  `a_peer_that_never_reads_gets_one_anti_entropy_reply_at_a_time`.
- *One stalled SSE or serve subscriber cannot veto admission of its kind node-wide* — `fill_ratio` is the least-full
  work-bearing subscriber, SSE routes are taps (`mycelium-core/src/signal.rs`); `a_stalled_subscriber_does_not_stop_admission_for_another`,
  `a_kind_whose_every_worker_is_full_sheds_while_a_tap_reads`, `a_fast_tap_does_not_hide_a_saturated_worker`.
- *The signal log is bounded per sender-chosen kind* — 4096 kinds, 1024 senders per kind; `the_signal_log_stays_bounded_under_random_kinds`,
  `a_kind_first_seen_after_a_flood_is_still_tracked`, `a_query_pin_expires`; its `sys/quorum/` twin:
  `a_random_kind_flood_writes_no_evidence`, `one_kinds_evidence_is_bounded_by_the_sender_cap` (a rate, not a total —
  the keys are not collected, `deprecations.md` §30).
- *A stopped agent leaks no task context (`RpcRequestRx` ends at shutdown)* — `a_stopped_agent_frees_its_task_context`
  (`src/agent/rpc.rs`).
- **Removed in review, not built:** connection preemption and WAL group commit (rounds 3–5); a write-side rate floor
  (deliberately — the residual is on `what-is-proven.md`).

**P2 — A consensus electorate is a governed group, without migration (#601; decision record §8).**
- *Safety-sensitive consensus requires a governed group* — `consensus_require_electorate` refuses a proposal flagged
  `safety_sensitive`, or in `lock/`, `leader/`, `consistent/`, `capauthz/`, `electorate/` or a commitment award,
  outside an electorate group: `ConsensusResult::ElectorateNotGoverned`, 403 `electorate_not_governed`
  (`src/agent/electorate.rs` `SAFETY_SLOT_FAMILIES`, `src/consensus.rs`); `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`,
  `the_exclusive_slot_families_are_safety_sensitive_by_definition`, `an_exclusive_outcome_at_the_gateway_needs_an_electorate_group`.
- *Whose membership moves only through a governance route* — **as decided, it does not move at all**: a genesis
  (`declare_electorate`, `POST /gateway/govern/electorate`, `govern:write`) is decided by every member it names, and a
  changed roster is `MembershipFrozen` (`409 membership_frozen`); `a_swap_without_a_migration_is_refused` (asserts
  `MembershipFrozen`). An undecided genesis is superseded only by the live roster
  (`an_undecided_genesis_is_superseded_by_one_naming_the_live_roster`, `a_genesis_supersedes_only_the_epoch_before_it_with_a_majority_of_it`).
  **Delivered after 2.32.0: changing an electorate's members by a migration that terminates** — declare the successor
  naming the group, retire the group into it, complete the carry — and the default moving by retirement
  (`docs/design/consensus-electorate.md` §8.3; `a_value_chosen_in_a_retired_group_is_carried_by_its_successor` and
  the eleven tests beside it; `after_the_completion_the_successor_survives_the_predecessors_departure_and_its_own_restart`
  is the acceptance test). **Not built:** disaster recovery of a group that lost its majority.
- *The secure profile checks it* — `cons.safety_profile` (rev 2), required by `secure-single-domain` **rev 4**;
  `the_consensus_safety_profile_resolves_on_the_electorate_requirement`, `the_secure_profiles_required_set_is_pinned`.
- *The electorate is named as a group, never as node identities* — `ElectorateDecl { group, epoch, members, … }`
  decided on `electorate/{group}/{epoch}`; the members are the vote filter, pinned by identity: a roster that differs
  is `ElectorateMismatch` (`electorate_roster_mismatches`); `votes_from_outside_the_electorate_are_not_counted`,
  `an_electorate_is_established_not_inferred`, certificates `a_forged_electorate_record_is_not_adopted`,
  `an_electorate_certificate_verifies_under_tls`.
- *Versioned electorates with joint-consensus transitions recorded as a later plan, not built here* — recorded (decision
  record §4.2, "not built, not scheduled").
- *Tension (a), governed is not fixed* — resolved by freezing; the governor and the emergent watcher leave an electorate
  group alone, `/gateway/mesh/group` refuses it 403 `governed_group`: `the_governor_and_the_watcher_leave_an_electorate_group_alone`.
- *Tension (b), trust slices as the name* — decided: the member set is the vote filter, a trust slice may narrow it and no
  longer names it (decision record §3).
- *The gateway-only boundary (an embedded `join_group` or `grp/` write)* — **detected, not prevented**: such a write
  makes the roster differ from the member set, and every proposal to the group is refused `ElectorateMismatch` and
  counted. Layer I still accepts the write (detection, not prevention).
- *The exclusive default set once* — `a_second_default_is_refused_by_name`, `two_defaults_fail_closed`,
  `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused`; cluster-scoped exclusive slots fail closed while
  the default cannot be named: `a_tagged_cluster_proposal_for_an_exclusive_slot_fails_closed`; a cross-group proposal
  decides neither: `a_cross_group_proposal_does_not_decide_for_an_electorate_group`, `…_an_exclusive_slot`.
  `leader/{g}` and `electorate/{g}/…` only under `Group(g)`: `slot_names_group` at the door and the acceptor
  (`src/consensus.rs`) — **no test proposes `leader/{g}` under another group**; the scoping is code-only.
- **Stated limits:** two defaults declared concurrently within gossip lag stop the exclusive verbs fleet-wide with no
  escape in this release; a lost majority stops the group (for the default, the cluster-scoped exclusive verbs); genesis
  assumes the members agree on the roster; certificates are unauthenticated without `[tls]`.

**Φ — Philosophy: "protocol, not service" (#594 the first paragraph; #607 its full statement; docs only).**
- *A paragraph in the corrected-litmus section stating §2's line* — `docs/philosophy.md` § *The corrected litmus*, the
  blockquote **A protocol, not a service**: "a *protocol* that every member of a group runs — whichever nodes are in
  the group run it, roles form per ballot, the coalition dissolves when the decision completes, its state is ordinary
  keys and signals, and Layers I and II know nothing of it. It is never a *service* that a named node provides" (§2's
  two bullets, and D1).
- *The deployment rule that no shape may make a named node set the place agreement happens* — the same paragraph,
  bold: "no shape — a profile, a configuration default, a unit file, a governance route, a document — may make a named
  node set the place where agreement happens", with how an electorate is named instead (a group, members fixed at
  genesis, changed only by a migration that terminates) and what 2.32.0 enforces of it (P2, C1, C2) and what came
  after (the migration, #606). No *not built* remains in the paragraph: C1/C2's was struck in #600 and P2's here, the
  migration having landed.
- *The wiki's runtime-invariants page cites it* — `docs/wiki/dev/architecture/runtime-invariants.md` § *Layer III
  posture*, the paragraph **Discovery is not an electorate; consensus is a protocol, not a service**: one sentence
  restating the rule with a relative link to `philosophy.md#the-corrected-litmus`.
- **Stated, not a promise of this row:** the rule is a document's; the code that keeps it is P2's, closed above.

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
