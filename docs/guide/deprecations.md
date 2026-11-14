# Deprecations — what is on the way out, and what to do about it

**Nothing on this page is removed on the 2.x line.** Every entry is a *replacement added beside the
old thing*, with removal deferred to the `3.0.0` removal ledger. The ledger exists so that a major
release, if one ever happens, removes only things that were announced — and the announcement is this
page, on the day of deprecation rather than the day of removal.

The authoritative ledger is [`docs/plans/v3-contracts-axis.md`](../plans/v3-contracts-axis.md) §6.6.
This page is its adopter-facing half: what to change, and whether the compiler will tell you.

## The one compatibility rule

The substrate ships compatible additions on 2.x, by Rust's definition: **a public field or type never
changes shape**. When a verb's answer gains meaning, the new representation is added *beside* the old
one and the old one goes on this page. Read the entry, adopt the replacement, keep compiling.

## Will the compiler tell me?

Honest answer: **usually not**. Only four entries carry `#[deprecated]` (1, 3, 37 and 40), so for the rest this page is
the notice.

| # | What | Since | Replacement | Compiler warns? |
|---|---|---|---|---|
| 1 | `system_propose` | 2.1.0 | `cluster_propose` | **Yes** — `#[deprecated]` |
| 2 | `cluster_name` as an isolation boundary | — | `DomainId` (item 2) | No — it stays as a display label |
| 3 | the inferred `>=` ack in `set_with_min_acks` | — | exact-identity (`content_hash`) ack | **Yes** — `#[deprecated]` *(resolved 2026-09-15)* |
| 4 | `ConsensusResult::Committed { persisted: bool }` | 2.4.2 | `cluster_propose_receipt` / `group_propose_receipt` | No |
| 5 | node-as-caller gateway dispatch (`GatewayCallerProfile::Legacy`) | 2.5.0 | `GatewayCaller` (item 7) | No — a `warn!` at startup |
| 6 | `BoardConfig` not `#[non_exhaustive]` | 2.8.0 | `..Default::default()` | No — it breaks at 3.0.0 |
| 7 | `GossipConfig` (and sibling config structs) not `#[non_exhaustive]` | — | `..Default::default()` | No — it breaks at 3.0.0 |
| 8 | exhaustive `match` on `RecordKind` | 2.10.0 | add a `_` arm | **Yes** — `#[non_exhaustive]` from 2.10.0 |
| 9 | exhaustive `match` on `Execution` | 2.10.0 | add a `_` arm that reads as **unknown**, not as *nothing ran* | **Yes** — `#[non_exhaustive]` from 2.10.0 |
| 10 | `FederationEdge::authorize(presented, export, now_ms)` | 2.12.0 | `authorize(presented, export, **body**, now_ms)` | **Yes** — it will not compile |
| 11 | `mesh:read` / `mesh:write` admitted on the serve routes | 2.15.0 (window closed 2.18.2) | a token holding `mesh:serve` | No — `403 {"required_scope": "mesh:serve"}` at the route |
| 12 | `POST /gateway/kv` without `value_b64` | 2.14.0 | send `value_b64` always (`""` for empty) | No — HTTP 400 |
| 13 | exhaustive `match` on `TracePolicy` / the federation `ClientError` | 2.20.0 | a `_` arm that fails closed | No — a `_` arm takes the new variant silently |
| 14 | a two-argument `with_entry_activation` closure | 2.21.0 | a three-argument closure (`&ActivationCtx` third) | **Yes** — the arity is in the trait bound |
| 15 | exhaustive `match` on `WalMsg`; a second owner or a failed startup snapshot | 2.23.0 | a `_` arm; `OwnershipLock::acquire` before replay | For the `match`, **yes**; the refusals are `InvalidField` at `start()` |
| 16 | outbound clients following an unchecked redirect | 2.23.0 | `.with_egress(agent.egress_policy())`; list the endpoint host for an object store | No — the redirect fails at request time |
| 17 | the live timing setters returning `()` | 2.25.0 | `set_health_check_interval_secs(30)?` | **Warns** — `unused_must_use`, an error under `-D warnings` |
| 18 | `govern_timing` returning `bool`; `sys/` or `consensus/` keys on the KV doors | 2.26.0 | `govern_timing(…)?`; the route that owns the key | `unused_must_use` for the verb; HTTP 400 / 403 for the routes |
| 19 | the raw KV routes writing an owned namespace | 2.27.0 | the route that owns the namespace | No — HTTP 403 `protected_key` (typed in both SDKs) |
| 20 | `/gateway/mesh/group` on a governed group | 2.29.0 | `/gateway/govern/group` (`govern:write`) | No — HTTP 403 `governed_group` |
| 21 | consensus without a prepare phase (a mixed fleet) | 2.30.0 | upgrade a quorum of each group's acceptors; handle `Superseded` | No — wire behaviour; `Timeout` until a quorum is upgraded |
| 22 | an alert keyed on one consensus-timeout `reason`; a second stop signal that waits; an untyped lost write | 2.31.0 | match the new reason set (§22); allow the grace period; catch `SupersededError` | No — a metric label, an exit code, a Python subclass |
| 23 | a bare (untagged) identity proof or consensus signature | 2.32.0 (accepted for one release) — **refused since 2.33.0** (§35) | the tagged form every 2.32 node writes — upgrade the fleet to 2.32 before 2.33 | No — refused; the two counters are removed (§35) |
| 24 | a group proposal from a node outside the group's roster | 2.32.0 | join the group first | No — `#[non_exhaustive]` enums gain `NotAMember`; HTTP 403 `not_a_member` |
| 25 | protected kinds on the SSE doors; `rpc/respond` for a request not yours; another identity's A2A task; `/api/tuple`; a policy-free LLM door | 2.32.0 | the decision trace or evidence journal; one bearer per serve loop; the creating bearer; `/gateway/tuple/overview` | No — HTTP 403 / 404 and JSON-RPC `-32004`; `SystemStats` and `RouteError` grow (an exhaustive literal or `match` breaks) |
| 26 | an open gateway on a non-loopback `http_addr` | 2.32.0 | a credential (`gateway_auth_token`, a token table or `[oidc]`), a loopback `http_addr`, or `gateway_allow_unauthenticated = true` | No — `start()` refuses, `InvalidField { field: "http_addr" }` |
| 27 | an unbounded, unconfined hosted WASM call; a `[hosts]` table with no `trusted_publishers` | 2.32.0 | `[hosts].call_deadline_ms`; `with_emit_kinds`; `accept_unsigned = true` on purpose | Partly — `WasmHostError`, `InvocationOutcome`, `ConfinementError` gain variants; `HostsDecl`, `StemOptions`, `RuntimeCtx` gain fields |
| 28 | a WAL append after a failed one; a corrupt companion WAL record; `sync_mode = "os"` under the secure profile; two new consensus-timeout reasons | 2.32.0 | read the receipt; move a corrupt WAL aside; `sync_mode = "flush"`; alert on the new reasons | No — `Err`/refusal at open/start; `SystemStats` gains fields (an exhaustive literal breaks) |
| 29 | fractional SDK timeouts; raw path segments; TypeScript `scatterGather` waiting for every target | 2.32.0 (`mycelium-py` 0.2.10, `mycelium-ts` 0.2.4, checkpointer 0.3.3) | whole seconds; pass `minOk: targets.length` to wait for all | No — values rounded up, bad ones refused by name |
| 30 | `sys/quorum/` evidence from every node that relays a kind | 2.32.0 | subscribe to the kind (a worker) or query it — `quorum_persistent(kind, window)` pins it for the window | No — fewer keys under `sys/quorum/` |
| 31 | an exclusive outcome (lock, leader, consistent write) on the whole cluster or an undeclared group, under the secure profile | 2.32.0 | declare an electorate group (`declare_electorate`, one `exclusive_default`), sized for its lifetime — its members are fixed at genesis | Partly — new `ConsensusResult`/`CommitError`/`ConsistencyError` variants (a `_` arm must fail closed); otherwise `start()` refuses rev 4 without `consensus_require_electorate`, and proposals are refused by name |
| 32 | an accept path, a writer and a signal log with no bound | 2.32.0 | `handshake_timeout_ms`, `inbound_idle_timeout_secs`, `peer_read_stall_timeout_ms`, `peer_write_stall_timeout_ms`, `peer_min_rate_bytes_per_sec`, `max_connections_per_source` | Partly — `GossipConfig`, `SystemStats` and `CoreCtx` gain fields; `WriterTiming` replaces a `Duration` in three `writer` functions; `validate()` refuses an idle timeout at or below the writer's |
| 33 | a permanent `elect_leader`; a lock or leadership read from the three legacy keys | 2.32.0 (legacy keys still written; they stop at a later, governed MINOR) | renew by calling again, or `elect_leader_with(group, LeaderTerm::Permanent)`; `release_leadership`; upgrade every node | No — a leadership lapses after 30 s; an upgraded proposer times out until a quorum is upgraded |
| 34 | "declare a new group and move the work yourself" as the way to change an electorate's members; a default that cannot move | 2.33.0 | `declare_successor_electorate` → `retire_electorate` → `complete_migration` (`POST /gateway/govern/electorate` with `predecessors` / `retire_to` / `complete`; `drop_predecessor` for one that never retires) | Partly — `ElectorateDecl` and `Retirement` gained fields, `ElectorateError` gained `Retired` (`#[non_exhaustive]`; a `_` arm must fail closed); otherwise refusals by name |
| 35 | an unsigned KV `Data` frame at a TLS node; a bare KV `SignedData` signature; a signed frame whose signer is not its sender; a certificate whose key was read by a byte-scan, or with no Ed25519 key | 2.33.0: unsigned and bare accepted for one release, counted; the scan removed — **refused since 2.34.0** (§40) | the tagged form every upgraded node writes — upgrade the fleet to 2.33 before 2.34 | No — counted (`kv_unsigned_frames_refused`, `kv_untagged_signatures_refused`, `kv_signer_mismatch`); `GossipConfig` gains a field (an exhaustive literal breaks) |
| 36 | an award without a close; an offer, report or assessment read from another requirement's stream; `award_of_verified` against the writable head; an unbounded blob store; any string as a run id | 2.33.0 (`mycelium-reason` 0.9.0) | `close(requirement, key, now_ms)` then award; `award_of_verified(requirement, declarer, resolve)`; `open_with_quota`; one bounded segment per run id | Partly — `TraceRecorder::new` returns `Result`, `award_of_verified` gained a parameter, `CommitmentRefusal` gained variants (`#[non_exhaustive]`); otherwise refusals by name (`NotClosed`, 507, 400) |
| 37 | nineteen public items (eighteen names) nothing in the repository references | 2.33.0 (deprecated, not removed) | none needed; open an issue to keep one — it is undeprecated in the next MINOR | **Yes** — `#[deprecated(since = "2.33.0")]` |
| 38 | a fractional, zero or past-an-hour `interval_secs`/`lease_secs`; a prompt `ns`/`name` that is empty or holds `/`; a caller envelope older than 300 s plus the drift bound; `GatewayPreflight::is_inert` | 2.33.0 (`mycelium-py` 0.2.11, `mycelium-ts` 0.2.5) | whole seconds in `1..=3600`; a one-segment prompt name; a fresh envelope (keep a provider's backlog under 300 s) | Partly — `PromptSkillError` gained `InvalidName`, `CallerError` `Stale`, `is_inert` warns; otherwise HTTP 400 `timer_out_of_range` / `invalid_prompt_name`, `stale` |
| 39 | a revoked key verifying at a KV, identity, consensus, envelope or knowledge verifier; a revocation signed by any key the node held; a certificate without `mycelium://<node>` | 2.33.0 | re-issue every node's certificate (`mycelium tls issue`, or a development node's next start); rotate before `identity/revoke` | Partly — `CoreCtx` and `RotationMaterial` gained fields; otherwise behaviour (a pre-row-Q certificate has no standing until re-issued; 422 on revoking the current key) |
| 40 | `kv_require_signed_frames` (an unsigned KV frame or a bare KV signature accepted at a TLS node); the counters `kv_unsigned_frames_accepted` / `kv_untagged_signatures` | 2.34.0 — §35's window closed | nothing: a TLS node refuses both whatever the setting says; read `kv_unsigned_frames_refused` / `kv_untagged_signatures_refused`; upgrade from 2.33.x, not 2.32 | **Yes** for code naming the field — `#[deprecated]`; otherwise no — a no-op, warned at `start()` when set; the two counters stay at zero |
| 41 | an identity this node cannot authenticate merged by default (a bare `sys/identity/{n}` key list, a legacy pair without a sealed record); `GOSSIP_REQUIRE_IDENTITY_PROOFS` read leniently | 2.34.0 (post-360 row S) | nothing from 2.33.x (2.3.0–2.31.x peers are unlearnable since 2.33.0 either way); `require_identity_proofs = false` re-admits only a node from before 2.3.0 | No — behaviour: a proofless (pre-2.3.0) peer's key is not learned (counted, `identity_anchor_conflicts`); a `GOSSIP_REQUIRE_IDENTITY_PROOFS` value other than `1`/`true`/`TRUE`/`yes`/`0`/`false`/`FALSE`/`no` (empty included) refuses by name |
| 42 | a soft-state re-assert written after shutdown began (between the sweep and the signal) | 2.34.0 | nothing — shutdown closes `CoreCtx::soft_state_gate` before its sweep | Only for an exhaustive struct literal of `CoreCtx` (it gained `soft_state_gate`) |
| 43 | a capability, requirement or group-provides `ns`/`name` that is empty, holds a `/`, or is `locality`/`self`; a group name holding a `/` | 2.34.0 | rename it: one segment each, and not the locality pair | No — behaviour: 400 at `capability/advertise` (`invalid_capability_name`) and `units/declare`; a node config fails to load; in process the handle writes nothing and a warning names it |
| 44 | `mycelium_core::tls::load_or_generate` / `generate_rotation` / `generate_rotation_after` without the key standing; `NodeStanding` without `counted`; TLS session resumption on the mesh | 2.35.0 (post-360 row Q's follow-up) | pass `Arc::clone(&ctx.key_standing)` (or `Default::default()` outside an agent) after the removed set | **Yes** for a direct caller of the three functions — one more argument — and an exhaustive `NodeStanding` literal; otherwise no — a reconnect is a full handshake, and a certificate whose key its node revoked is refused |
| 45 | a gateway caller envelope without an id (`n`), target (`o`) and v2 signature (`sn`) — a pre-row-E gateway's | 2.35.0 (post-360 row E) — accepted and counted for one release from a gateway without a fresh, signed `sys/caller-context-id/` marker | upgrade every gateway node; watch `caller_envelopes_without_id` (`GET /stats`) stop rising on every provider; a raw-signal loop calls `receive_gateway_caller` once per signal | No — behaviour: an envelope is served once, at its target; a replay (repeated, misaddressed, id stripped) is dropped unanswered and counted (`caller_envelopes_replayed`); two subscribers verifying one kind on one node no longer both serve; a bridged MCP call whose context fails verification is refused `-32022` |
| 46 | the A2A card at `/.well-known/agent.json`; the pre-0.2 `/a2a` methods `tasks/send` and `tasks/sendSubscribe`, and `skillId` as a send's parameter | 2.35.0 (post-360 row N) — served beside the replacements for one release | read the card at `/.well-known/agent-card.json`; send with `SendMessage` / `SendStreamingMessage` (or 0.3's `message/send` / `message/stream`), the skill in `metadata.skillId` and a mandate in `metadata._meta` | No — behaviour: the old path and methods answer as before; a task created by `message/send` answers `tasks/get` in 0.3's shapes; the refusal for another identity's task names `GetTask`/`CancelTask` too |
| 47 | an exhaustive `Rule { .. }` or `ContractCase { .. }` literal; an evaluator that wraps another and forwards only `evaluate` | 2.35.0 (post-360 row K) | build a `Rule` with `Rule::new` and its builders, and give a `ContractCase` `history: None` (or a history); a wrapping evaluator forwards `history_request` and `evaluate_with_history` | Yes for the literals (a new field); no for a wrapper — it compiles, and its limited rules decide `Indeterminate` |
| 48 | an exhaustive struct literal of `GossipConfig`, `OpenAiBackend` or `ServeDecl`; a `{:?}` of a `ServeDecl` showing its `api_key`; Z2's `api_key_env` refusal naming the variable | 2.35.0 (post-360 row L) | `..Default::default()` (or the new fields: `egress_credentials`, `credentials`, `credential`); read the section name in the refusal | **Yes** for an exhaustive literal — it will not compile; otherwise no — `Debug` prints `api_key: Some("<redacted>")`, and the refusal names the `[[serve]]` section and the field, not the variable |
| 49 | a unit file with a key or section the loader does not know — at the top level or in an `[[activation]]` | 2.35.0 (post-360 row M) | correct the spelling, or remove the key | No — behaviour: the file is refused at load, naming the key (`units/declare` answers 400, `wire-check` and a stem refuse it); it was silently dropped |
| 50 | a trace consumer or alert keyed on `prov.probe`'s `probe_failed` for a confined re-probe; a probe that replaces one of its own declared paths on every run | 2.36.0 (post-360 row M's follow-up, #641; `prov.probe` rule revision 3) | read the three new refusal reasons — `confinement_unavailable`, `confinement_failed` and `confinement_changed` — beside `probe_failed`, which now means only that the probe failed; move a step that replaces a declared path into the activation `command` | No — behaviour: a confined re-probe re-resolves a replaced path, a refused re-resolution is recorded by its own reason, and a probe that replaces its own declared path fails as `confinement_changed` ([§50](#50-provprobe-is-rule-revision-3-a-confined-re-probe-re-resolves-and-three-new-refusal-reasons-2360-post-360-plan-row-ms-follow-up-641)) |
| 51 | an exhaustive `ConfineDecl { .. }` literal; an exhaustive `match` on `activation_sandbox::Refusal`; a confined command relying on the stem's environment; two `[[activation]]`s for one `ns/name` | 2.36.0 (post-360 rows L and M, the activation egress proxy, #644) | `..Default::default()` for `ConfineDecl` (or `egress: None, env: Vec::new()`); a `_` arm, or `Refusal::EgressUnavailable`; name a needed variable in `[activation.confine] env`; merge duplicate activations | **Partly** — the literals and the `match` fail to compile; the behaviour changes do not ([§51](#51-an-activation-egress-proxy-confinedecl-gained-two-fields-and-profile-one-refusal-a-variant-2360-post-360-plan-rows-l-and-m)) |
| 52 | an exhaustive `Rule { .. }` literal; an evaluator adapter held to the contract suite that cannot express `PolicyClause::ForbidClassAt`; a `mycelium-reason` caller matching `RouteError` exhaustively or building `Routed`; a client relying on the façade answering `403` at the first refused provider | 2.36.0 (post-360 row R) — also: an `llm-meta` ad from a gateway client is `403 reserved_namespace`; `GET /gateway/rpc/serve/{kind}` for `llm.invoke`, `mcp.invoke`, `skill.invoke`, `rpc.result` or `bulk.result` is `403 protected_kind`, for a kind the node serves in process `403 served_in_process`, and an in-process registration closes client streams on its kind; a classifying router routes nothing without an evaluator; a catalogue entry providing `llm-meta/…` is never installed; on a classifying router a caller with no bound floor cannot declare below the default class, and the header given twice or to a router that classifies nothing is `400` | `Rule::new` and its builders; decline the five `route_*` cases in `declared_limits` or express them; a `RouteError::Unclassifiable` arm; read `refused` / `data.refused_destinations` | Yes for the literals and the `match`; no for the façade — a refused provider is now passed over for the next, and `403` comes only when every candidate is refused |
| 53 | an exhaustive `Rule`, `HistoryLimit`, `HistoryView`, `HistoryInput` or `mycelium-core` `KvStore` literal; an evaluator that wraps another and forwards only `evaluate`, `history_request` and `evaluate_with_history`; a fleet-consensus-limited request's position in an `rpc_rx` stream under provider enforcement | 2.36.0 (post-360 row K2; `ae.preflight` is catalogue revision 4) | `Rule::at_most(..)` and `.scoped(scope, counter_id)`; `fleet: Vec::new()` (and `seats: Vec::new()`) in a view or input literal; a wrapper forwards `fleet_counters` too | **Yes** for the literals (a new field); `HistoryScope` was already `#[non_exhaustive]`, so a `_` arm takes the new scopes; no for a wrapper — it compiles, and its fleet-limited rules decide `Indeterminate`; no for the order — behaviour |
| 54 | `mycelium_core`'s `CoreCtx::gossip_txs`, `WriterEntry::tx`, `writer::get_or_spawn_writer` / `request_state`, `run_peer_writer`, and `framing::dispatch_gossip_send` / `dispatch_gossip_try_send` / `gossip_shard_fill` in their old shapes | 2.36.0 (the shutdown-sweep fix, #650) | `GossipTxs::new(senders, Unsent::default())`; pass `gossip_txs.unsent()`; wrap a shard receiver in `CountedRx::new(rx, unsent)` | **Yes** — every use is a type error; code that only uses `GossipAgent` is unaffected ([§54](#54-mycelium-cores-outbound-path-is-counted-its-channel-types-changed-shape-2360-the-shutdown-sweep-fix)) |
| 55 | a fleet-scoped limit (`HistoryScope::FleetConsensus` / `FleetTally`) over more than 1 024 seats, or over a window of zero or more than 24 hours | 2.37.0 (row K2's bounds) | a per-point limit, or a fleet limit within `MAX_FLEET_SEATS` / `MAX_FLEET_WINDOW_MS`; check a policy at load with `check_fleet_bounds` or `FleetCounter::try_new` | **No** — it builds, and every request its rule covers decides `Indeterminate`; **yes** for a `ContractCase` or `Report` literal (a new field) ([§55](#55-a-fleet-limit-is-bounded-at-most-1-024-seats-a-window-of-at-most-24-hours-2370-post-360-plan-row-k2s-bounds)) |
| 56 | a `FleetTally` rule reading a member's entries this node holds without its heartbeat as zero, or counting a forged (future) time as a permit | 2.37.0 (row K2, #656) | nothing to migrate: such a member is unknown coverage while its entry is inside the window plus the staleness and drift bounds | **No** — a tally rule can decide `Indeterminate` where it admitted before, for at most that window ([§56](#56-a-members-tally-entries-without-its-heartbeat-are-unknown-coverage-not-zero-2370-post-360-plan-row-k2)) |

**Entry 10 is the loud kind**, and deliberately so: a signature change, caught by the compiler, not
a behaviour change to discover at runtime. You cannot authorise a federated call without saying
which body it is — see §10 below.

Entries 6 and 7 are the inverse of the usual case: nothing is deprecated *today*, but a future
`#[non_exhaustive]` will break one specific pattern, so the pattern is worth abandoning now.
**Entry 8 is that same case already done**: `RecordKind` was marked `#[non_exhaustive]` in 2.10.0
*before* AE0 §5's remaining record kinds arrive, so their arrival costs nothing. Add the `_` arm
once; that is the whole of it.

---

## 1. `system_propose` → `cluster_propose`

Renamed 2026-07-10 so the consensus verb matches its scope (`SignalScope::Cluster`). The old name is
a `#[deprecated]` alias with identical behaviour; the gateway still accepts `"system"` on the wire.

```rust
// before
agent.consensus().system_propose(slot, value, config).await;
// after — same arguments, same result
agent.consensus().cluster_propose(slot, value, config).await;
```

**Unrelated, and a common confusion:** `system_stats()` is *not* this. It reports node-local runtime
state and is not a scope.

## 2. `cluster_name` is a label, not a boundary

`cluster_name` never provided isolation. A Mycelium cluster is emergent from network reachability —
peer exchange plus CA admission — so two nodes with *different* `cluster_name`s that can reach each
other and pass admission will gossip, and two with the *same* name that cannot will not.

If you were relying on `cluster_name` to keep deployments apart, that separation does not exist. The
mechanism that does is a **federated domain** ([chapter 17](17-federation.md)): a `DomainId`, a trust
bundle, and per-partner authorisation — with the invariant that federation never joins the
transports.

```rust
// not a boundary: a display label that happens to differ
cfg.cluster_name = Some("prod".into());
// the boundary: separate meshes, explicit trust between them
let domain = DomainId::new("prod.example")?;
```

`cluster_name` keeps working as a display name. What is deprecated is **reading isolation into it**.

## 3. The inferred `>=` acknowledgement — resolved

`set_with_min_acks` used to count acknowledgements by inference. It now matches on exact identity
(`content_hash`), so an ack names the write it is for. **Resolved 2026-09-15**: no migration flag was
needed, because the success path a flag would have preserved turned out to be unreachable. See
[chapter 18](18-contracts-and-receipts.md) and `docs/design/contracts-receipts.md` §1a.

## 4. `Committed { persisted: bool }` → the receipt verbs

A `bool` cannot distinguish *"the bytes are on disk"* from *"the write was queued and nothing is
promised"*, and the field's own documentation has to spend a paragraph saying so. Ask for a receipt
instead, which names its rung and nothing above it.

```rust
// before — one bit, and `false` does NOT mean "absent from the WAL"
if let ConsensusResult::Committed { persisted, .. } = result {
    if persisted { /* ... */ }
}
// after — a typed rung
let receipt = agent.consensus().cluster_propose_receipt(slot, value, config).await;
```

The field still works and still means what it documents. Two things to keep in mind while you have
it: add `..` to any exhaustive `Committed` destructure (it gained fields in 2.4.2), and never read
`false` as "absent from the WAL" — it means durability was *not established*, which is a different
claim. [Chapter 18](18-contracts-and-receipts.md) is the full ladder.

**Why the compiler does not warn here, costed rather than asserted** *(2026-09-22)*. It *could*:
Rust does support `#[deprecated]` on a struct-variant field, and it warns on both construction and
destructuring — checked, not assumed, because the first draft of this note claimed the opposite and
was wrong.

The cost is that the field is **still produced by this crate**, in about fifty places across ten
files. Marking it would mean an `#[allow(deprecated)]` beside each one — `make check` runs
`-D warnings` — so the substrate would be suppressing a warning about itself, permanently, and every
future internal use would need the same. That is a lot of noise to buy a nudge for the one pattern
already spelled out above.

So it is left unmarked **deliberately**, and this paragraph is the reason. It is a judgement about
signal-to-noise, not an oversight, and it is worth revisiting if the internal uses shrink — the
day this crate stops producing the field is the day marking it costs nothing.

## 5. Node-as-caller gateway dispatch → `GatewayCaller`

Before item 7, every gateway-originated dispatch ran under the **node's** identity, so a provider's
`authorized_callers` saw the gateway node and never the HTTP client — a confused deputy. A provider
now receives a verified `GatewayCaller`: the originating principal, the gateway acting for it, and
the scopes granted for that request.

The old behaviour survives only under an explicit profile, for a rolling-upgrade window:

```toml
# deprecated: dispatch as the node. Logged at warn! on every start.
gateway_caller_profile = "legacy"
```

**Migration:** upgrade providers first (each writes `sys/caller-context/{self}` when it enforces),
then drop the `legacy` profile. A secure-profile gateway refuses to dispatch to a provider that has
not published the marker, rather than silently falling back. Note that `authorized_callers` now
judges the **client principal**, not the gateway node — so an entry naming a node id stops matching,
which is the point. Operational detail: [`operations/rbac.md`](../operations/rbac.md).

## 6 & 7. Exhaustive struct literals over config structs

`GossipConfig`, `BoardConfig` and the operator-constructed config structs beside them are **not**
`#[non_exhaustive]` today. Every release adds config fields, and each addition silently breaks an
exhaustive struct literal — 2.5.0, 2.7.0, 2.8.0, 2.19.0 (`GossipConfig.profile`), 2.20.0
(`PersistenceConfig.on_unreadable`, which has no `Default`, so the literal must list it; `StemOptions.trace`)
and 2.21.0 (`RuntimeCtx.trace` + `install_token`) each did. Marking them `#[non_exhaustive]` at
`3.0.0` trades that series of unannounced breaks for one announced one.

```rust
// breaks on any release that adds a field
let cfg = GossipConfig { bind_port: 7000, bootstrap_peers: peers, /* ...every field... */ };
// unaffected, now and after 3.0.0
let mut cfg = GossipConfig::default();
cfg.bind_port = 7000;
cfg.bootstrap_peers = peers;
```

The `Default` + assignment pattern is the supported one. Same for `BoardConfig` and `..Default::default()`.

---

## What this page does not cover

Wire-format compatibility, which is versioned separately (`mycelium-core/src/framing.rs`) and
described under [rolling upgrades](../operations/deployment.md#rolling-upgrades). Companion crates
version on their own lines and carry their own notes.

## 10. `FederationEdge::authorize` takes the request body

The credential's signature covered the origin domain, the principal, the export and the validity
window — **and nothing about the payload**. An attacker on the path between two domains could
rewrite a call's body, leave the credential header untouched, and the receiving gateway would accept
the altered call as authentic, then run the authorisation preflight and record an evidence decision
about the attacker's text.

```rust
// before — authorises a caller and an export, and nothing about what was asked
edge.authorize(&presented, skill_id, now_ms)?;
// after — the bytes as they arrived, before parsing
edge.authorize(&presented, skill_id, &raw_body, now_ms)?;
```

**Why a break rather than a second method.** An `authorize_with_body` beside the old one would have
left the *insecure* call still compiling, still public, and still the shorter name. The point of the
change is that you cannot authorise a federated call without saying which body it is, and only the
signature can enforce that.

**Pass the bytes as received**, not a re-serialisation: two encodings of the same JSON are the same
request and different bytes, so digesting a parsed-then-re-encoded body compares your serialiser
against your partner's. `POST /a2a` reads the body as `Bytes` and parses afterwards for exactly this
reason.

**You also choose when to require it.** `CallPolicy::require_body_binding` defaults to `false` so a
partner that predates the binding keeps working — and that default gives **no integrity guarantee
against an active attacker**, because an absent binding is indistinguishable from a stripped one.
Turn it on once every partner has upgraded. `BodyNotBound` means upgrade the partner;
`BodyMismatch` means someone is on the path.

## 11. `mesh:read` / `mesh:write` admitted on the serve routes — one release (2.15.0) — **removed in 2.18.2**

**What changes.** Registering to serve an RPC kind — `GET /gateway/rpc/serve/{kind}` and
`/gateway/rpc/respond` — requires the new scope **`mesh:serve`**. From 2.15.0 a token holding
`mesh:read` or `mesh:write` was still admitted there, **with a warning in the gateway log**. The
window was promised for one release and stayed open through 2.18.1; **2.18.2 closes it** — such a
token now gets `403 {"required_scope": "mesh:serve"}`.

**Why.** Serving and calling were one scope, so a token that could serve could also reach the
raw call routes; separating them is what lets a skill server hold *only* `mesh:serve` (closure plan
C1, `src/agent/http/authz.rs`).

**Migration.** Reissue every serving client's token with `mesh:serve` (a named token,
[rbac.md §1](../operations/rbac.md)); grep the gateway log for the warning to find the ones you
missed. A caller that only *calls* needs no change.

## 12. `POST /gateway/kv` without `value_b64` is 400 (2.14.0)

**What changes.** The gateway KV write requires `value_b64`. A body without it answers **400
`missing 'value_b64'`** and mutates nothing; before 2.14.0 it stored an **empty value** and answered
`{"ok": true}`, so a client bug looked like success.

**Why.** A write that succeeds at writing nothing is the silent failure the receipt vocabulary exists
to remove; `""` is the explicit way to write an empty value.

**Migration.** Send `value_b64` always (`""` for empty). `mycelium-py` ≥ 0.2.4 and `mycelium-ts`
≥ 0.1.1 already do; a raw HTTP client is the one to check.

## 13. `#[non_exhaustive]` enums gain variants — 2.20.0 (`TracePolicy::Partial`, `ClientError::Egress`)

**What changes.** Two enums that were already `#[non_exhaustive]` gained a variant: `TracePolicy`
(`mycelium_core::rule`) gained `Partial(&'static str)` — a decision point traced in part, the missing
part named — and the federation `ClientError` gained `Egress` — a partner `base_url` the node's
`egress.allow_hosts` denies, refused before any byte is sent.

**Will the compiler tell me?** Only if your `match` was already exhaustive without a `_` arm, in which
case it never compiled against a `#[non_exhaustive]` enum from another crate. A `_` arm compiles and
silently takes the new variant.

**Migration.** The same rule as `CallRefusal` (§9): a `_` arm over a refusal **fails closed** — treat an
unrecognised `ClientError` as *not delivered*, never as success. For `TracePolicy`, `Partial` reads as
*instrumented, with a stated gap*; `mycelium rules` prints the gap beside the entry.

## 14. `BlobRuntime::with_entry_activation` takes a three-argument closure (2.21.0)

**What changes.** The activation hook's closure is `Fn(&InstallableEntry, &Path, &ActivationCtx) ->
Result<Option<Arc<AtomicBool>>, String>` (`mycelium-wasm-host/src/runtime.rs`); a two-argument closure no
longer compiles. `ActivationCtx { install_token, trace }` carries the install token and the decision trace,
so `prov.activation` and `prov.probe` record under the install that placed the blob. `RuntimeCtx` gained
`trace` and `install_token` — an exhaustive literal breaks; use `..`. `prov.probe` is catalogue rev 2
(new reason `probe_failed`: a failing *initial* probe is an activation error, never a live install).

**Will the compiler tell me?** Yes — the closure's arity is in the trait bound.

**Migration.** Ignore the context: `|entry, path, _ctx| …`. Use it to record: `_ctx.trace.as_ref()`.

## 15. `WalMsg` gained `HoldOwnership`; a second owner and a failed startup snapshot refuse the start (2.23.0)

**What changes.** `mycelium_core::persistence::WalMsg` has a new variant, so an exhaustive `match` on it no
longer compiles. Behaviour changes with it: `start()` takes a lock on `wal.bin.lock` and refuses a second
agent on the same `{base_path}/{node_id}`, refuses if the startup snapshot that repairs a torn WAL tail
fails, and `WalHandle::shutdown` now waits for the writer to exit.

**Will the compiler tell me?** For the `match`, yes. For the refusals, no — they are `InvalidField {
field: "persistence" }` at `start()`; `docs/operations/deployment.md` § *Persistence start refusals*.

**Migration.** Add a `_` arm. A direct embedder of `replay` + `spawn_wal_writer` takes
`OwnershipLock::acquire` before replay, hands it to `WalHandle::hold_ownership`, and awaits
`trigger_snapshot()` before its first append — the two functions' rustdocs say so.

## 16. Outbound clients follow no unchecked redirect (2.23.0)

**What changes.** Every outbound client the substrate builds re-checks each redirect hop against
`egress.allow_hosts` (at most five, never https → http). `OpenAiBackend::new` follows **no** redirect;
attach the node's policy to follow allowed ones. An `s3://` / `gs://` library under a non-empty allow-list
is gated on the **endpoint host it dials**, not the bucket.

**Will the compiler tell me?** No — a redirect that used to be followed now fails at request time.

**Migration.** `.with_egress(agent.egress_policy())` on each — since 2.26.0 all four (`OpenAiBackend`,
`OllamaProbe`, `HttpLibrarySource`, `FederationClient`) take the policy by reference or by value. For object
stores list the endpoint
(`docs/operations/artifacts.md` § *Remote blob stores*).

## 17. The live timing setters return `Result` (2.25.0)

**What changes.** `set_health_check_interval_secs` and `set_reconnect_backoff_secs` return
`Result<(), GossipError>` and refuse a value above 3600 / 300, changing nothing.

**Will the compiler tell me?** It warns (`unused_must_use`) where a call ignores the result — an error under
`-D warnings`.

**Migration.** `agent.set_health_check_interval_secs(30)?;` — or `.expect(…)` where the value is a constant
you know is in range.

## 18. `govern_timing` returns `Result`; `with_egress` takes either form (2.26.0)

**What changes.** `GossipAgent::govern_timing` returns `Result<bool, GossipError>` and refuses an intent above
3600 / 300, or one governing neither setting. The three `POST /gateway/govern/*` routes answer **400** for the
same, and for a body that is not an object, an unknown field, a field that is not the type it names (`"30"` for
a number, `"true"` for a boolean), a `target` that is not a node-id string, and — membership — a missing `min`.
The KV doors (`POST`/`DELETE /gateway/kv`, `POST /gateway/kv/quorum`, `POST /gateway/overlay/consistent/set`)
answer **403** `protected_key` for any key under `sys/` or `consensus/` except `sys/topology-override/{group}`.
Every client's `with_egress` now takes `impl Into<EgressPolicy>`: a reference or a value.

**Will the compiler tell me?** For `govern_timing`, it warns (`unused_must_use`) where the result is ignored, an
error under `-D warnings`. A `with_egress` call with an `EgressPolicy` or a `&EgressPolicy` compiles unchanged; one that relied on deref coercion (`&Arc<EgressPolicy>`, `&Box<…>`) or inference (`with_egress(Default::default())`, `with_egress(x.into())`) now needs the type spelled — `&*arc`, `EgressPolicy::default()`.

**Migration.** `agent.govern_timing(30, 5, None)?;`. An HTTP client that relied on `"30"` being accepted sends
`30`; one that released governance by publishing zeros stops publishing (the intent lapses with its lease); one
that wrote a `sys/` or `consensus/` key through the KV routes uses the route that owns it — the governance
routes for `sys/govern/`; for a member removal, `GossipAgent::offer_member_removal` (the record is verified on
ingest, so only the door changes); the rest are written only by the substrate.

## 19. The raw KV routes write application keys only; the topology override has a route (2.27.0)

**What changes.** `POST`/`DELETE /gateway/kv`, `POST /gateway/kv/quorum` and `POST /gateway/overlay/consistent/set`
refuse **403** `protected_key` for every key in a namespace the substrate or a companion owns (`src/lib.rs` § KV
namespace ownership) — 2.26.0 refused `sys/` and `consensus/`; 2.27.0 refuses the rest, with
`sys/topology-override/` no longer excepted. The LangGraph checkpointer's `ckpt/`/`ckptw/` rows, the mesh
manifest (`manifest/`), the schema registry (`schemas/`) and an application's `agent/{node}/provision/…` report
are unaffected, as is every key outside the table. The log routes refuse a stream under `cn/`, `wiki/` or
`reason/` (403 `protected_stream`).

**Will the compiler tell me?** No — it is an HTTP status. The 403's `message` names the route to use where the namespace has one.
The SDKs raise it typed: `ProtectedKeyError` (`.key`) and `ProtectedStreamError` (`.stream`), carrying that
message (`mycelium-py` 0.2.7, where each is also the `httpx.HTTPStatusError` it raised before; `mycelium-ts` 0.2.2).

**Migration.** Use the route that owns the namespace: `POST /gateway/govern/topology-override` (`govern:write`) for
the topology escape hatch; `/gateway/prompts/{ns}/{name}` for prompt templates; `/gateway/overlay/log/append` for
logs; `POST /gateway/mesh/group` on the node that joins a group (`DELETE /gateway/mesh/group?group=G` to leave);
`POST /gateway/capability/advertise` (and `DELETE /gateway/capability/{handle_id}`) for capabilities;
`POST /gateway/units/declare` for requirements; `/gateway/mailbox/deliver`;
`/gateway/artifacts/publish` for catalogue lines; `/gateway/overlay/lock/acquire` for locks. A namespace with no
gateway route of its own is written by the substrate or the companion that owns it.

## 20. A governed group's membership moves through a governance route (2.29.0)

**What changes.** `POST`/`DELETE /gateway/mesh/group` (`mesh:write`) refuse **403** `governed_group` for a group under a
live membership intent (`POST /gateway/govern/membership`): its members are the roster and quorum its elections count,
so changing them is governance. `POST`/`DELETE /gateway/govern/group` (`govern:write`) does it, for any group. Every
membership change through `/gateway/govern/group` is audited (or counted on `/stats` as `governance_unaudited`); a plain group's `mesh:write`
join is audited, not counted, and a join or leave that changes nothing is not recorded. `POST /gateway/units/declare`
refuses a `[[group]]` whose name is governed (403 `governed_group`): its filter decides whom the governor can elect.
"Governed" lasts while the intent is live — re-published within `MEMBERSHIP_INTENT_TTL_MS` (5 min) — and an intent
`target`ed at one node governs the group fleet-wide. A group with no membership intent is unaffected.

**Will the compiler tell me?** No — it is an HTTP status; the 403's `message` names the route.

**Migration.** A client that joins or leaves a governed group over HTTP uses `/gateway/govern/group` with a
`govern:write` token. The membership governor's own moves (the embedded API) are unaffected.

## 21. Consensus asks before it proposes: a mixed fleet times out rather than commits (2.30.0)

**What changes.** A proposer now runs a prepare phase — `Prepare`, answered by `PrepareAck` — before it proposes,
and carries the highest-ballot value its promise quorum reports. An acceptor older than 2.30.0 ignores `Prepare`,
so **an upgraded proposer cannot assemble a promise quorum until a quorum of the group's acceptors is upgraded**:
its proposals time out (`ConsensusResult::Timeout`) rather than commit. A proposer older than 2.30.0 keeps working
against upgraded acceptors, without the guarantee. The acceptor's durable record (`sys/consensus-accepted/`) now
carries the promise: an upgraded node reads the old record, and a node **downgraded** below 2.30.0 reads the new
one as no record — it forgets what it promised and accepted for in-flight slots.

Three storage changes ride with it. The acceptor's record is **kept** after a slot commits (it was removed, which
dropped promises), so `sys/consensus-accepted/{node}/` holds one record per slot this node has voted on;
`consensus/ballot/{slot}` is no longer tombstoned at commit, so ballots rise across a leased slot's successive
decisions; and a commit writes `consensus/decided/{slot}`, the ballot it was decided at.

A proposal that ends by committing **another caller's** value — adopted in the prepare phase from what a quorum had
already accepted — now returns `ConsensusResult::Superseded`, not `Committed`; `consistent_set` returns
`Err(Superseded)` and `set_capability_authz_via_consensus` applies nothing. Before, it returned `Committed` with the
other value, and callers that ignored the value acted on their own.

The acceptor's record now carries an accepted value of up to 4 KiB, gossiped cluster-wide: a value proposed to a group
and never committed is readable outside it. A slot left **undecided** by a proposal that timed out before the
upgrade keeps a 40-byte record that names its value only by digest; while such acceptors hold the top acceptance in a
promise quorum, a proposal of any other value for that slot is refused (`Timeout`) rather than risk overwriting it.

**Will the compiler tell me?** No — it is wire behaviour; the API is unchanged.

**Migration.** Upgrade a group's nodes before relying on consensus from upgraded proposers, or expect timeouts
while fewer than a quorum are upgraded — the same shape as 2.14.0's value-bound votes. Do not downgrade a node
mid-slot.

## 22. Consensus timeout reasons say who answered; a second stop signal exits; a lost write is typed (2.31.0)

**What changes.**
- **`mycelium_consensus_timeouts_total{reason}`** is labelled by what *other* acceptors answered on the last attempt:
  `no_voters` (none — a partition, or mid-upgrade every acceptor older than 2.30.0), `promise_short` (some
  promised, too few), `contended` (another proposer is ahead), `blocked` (the slot's top acceptance is known only by
  digest), `quorum_short` (some voted, too few). `contended` and `blocked` are new. A partition inside a group
  proposal used to read `quorum_short` (the proposer's own vote counted) and now reads `no_voters`; a refusal used
  to read `no_voters` or `quorum_short` and now reads `contended`.
- **A second SIGINT/SIGTERM during shutdown exits at once** with `128 + signal` (130, 143) — the node binary,
  `mycelium-stem` and the long-running examples (`mycelium::shutdown::ShutdownSignal`). A shutdown that hung used
  to wait for SIGKILL. The first signal still runs the orderly shutdown, including one that arrives during
  startup.
- **`mycelium-py` 0.2.8:** `consistent_set`, `cross_group_propose`, `distributed_lock` and `elect_leader` raise
  `SupersededError` for a 409 `superseded` — still an `httpx.HTTPStatusError`. Python only: the TypeScript SDK
  still rejects with its generic error.

**Will the compiler tell me?** No — a metric label, an exit code and a Python subclass.

**Migration.** An alert keyed on a single `reason` value should match the new set — for "consensus is
stalling", `reason=~"no_voters|promise_short|quorum_short"`; `contended` is contention, not failure. A supervisor
that sends a second signal to hurry a stop now gets an immediate exit with 130/143 rather than a wait; send one and
allow the grace period if the orderly shutdown matters (the stem's trace and its withdrawals, unsynced
persistence). Existing
`except httpx.HTTPStatusError` handlers keep catching a lost write; catch `SupersededError` to tell it apart.

## 23. Identity proofs and consensus signatures are domain-tagged: the bare form is accepted for one release (2.32.0) — closed in 2.33.0 (§35)

> **Closed.** The allowance below held for 2.32.0 only. From the release after it (§35) a bare identity proof is
> refused whatever `require_identity_proofs` says, a bare consensus signature does not verify, an acceptor no longer
> answers in the bare form, and the counters `identity_untagged_proofs` / `consensus_untagged_signatures`
> (`mycelium_identity_untagged_proofs_total`, `mycelium_consensus_untagged_signatures_total`) are removed — nothing in
> the SDKs or the shipped dashboards read them. **Upgrade a 2.31 fleet to 2.32 before going further**: a 2.31 node and
> a node past 2.32 cannot verify each other's proofs or consensus messages at all.

**What changes.** An identity proof is a signature over `mycelium.identity/proof/1 ‖ len ‖ history`, and a
consensus payload's signature is over `mycelium.consensus/msg/1 ‖ len ‖ bytes`; before 2.32.0 both were bare
signatures over the bytes themselves, which let one be presented as the other (CHANGELOG, 2.32.0 § Fixed). The
frames are unchanged — wire **v12** — so this is a rolling-upgrade allowance, not a wire bump:

- **A 2.32 node accepts the bare form of both for one release**, and counts each acceptance
  (`mycelium_identity_untagged_proofs_total`, `mycelium_consensus_untagged_signatures_total`; `GET /stats`
  `identity_untagged_proofs`, `consensus_untagged_signatures`; a warning per identity proof). These count
  **validations, not peers**: the identity watcher re-validates every record on every `sys/identity*` change, so
  one un-upgraded peer raises the identity counter repeatedly. The identity allowance holds **only while
  `require_identity_proofs` is off**. With the flag on, a bare proof is refused like an unsealed one: that flag
  already requires the whole fleet to write proofs, and a bare signature over 32-aligned bytes is what any other
  signing path could have produced.
- **Consensus, 2.31 proposer → 2.32 acceptors: works.** A 2.32 acceptor answers in the signature form of the
  request it verified, so a bare `Prepare`/`Propose` gets a bare `PrepareAck`, `Promise`/`Nack` or vote, which the
  2.31 proposer verifies; its rounds, lease renewals included, complete.
- **Consensus, 2.32 proposer → 2.31 acceptors: times out.** A 2.31 node verifies bare only, so it drops a 2.32
  proposer's `Prepare`/`Propose`/`Commit` as *bad signature*; until a quorum of the group's acceptors is upgraded,
  an upgraded proposer times out (`ConsensusResult::Timeout`) rather than commits, and a 2.31 learner takes the
  committed value from anti-entropy rather than the COMMIT. The same shape as §21.
- **Identity, at a 2.31 verifier: a 2.32 peer's proof does not verify.** The tagged proof takes the *present but
  not verifying* branch: the keys are **not merged**, `identity_anchor_conflicts` increments, and the identity
  watcher re-raises it on every `sys/identity*` event for every 2.32 peer. Expect an un-upgraded node's
  `identity_anchor_conflicts` to **climb for the whole window** — it is not a poisoning signal there. A 2.32
  peer's handshake key still enters that node's `peer_keys` through the TLS anchor (`record_peer_anchor`), so
  directly connected peers stay verifiable; a 2.32 peer's **rotated** keys do not enter until the node upgrades,
  so do not rotate a 2.32 node's identity while 2.31 nodes must verify it.

**Will the compiler tell me?** No — it is signing behaviour; the API is unchanged. `sign_with_identity` is
unchanged too, but its doc now states the contract it always needed: the caller tags its own message.

**Migration.** Upgrade the fleet before relying on consensus from upgraded proposers, with
`require_identity_proofs` off until every node is on 2.32.0 (the flag's own rule), and without rotating a 2.32
node's identity while 2.31 nodes remain. Watch the two counters on upgraded nodes: a value still rising means a
peer still signs the old way; and read `identity_anchor_conflicts` on un-upgraded nodes as the window's noise
until they are upgraded. **The allowance closes in the next MINOR**, after which a bare signature is refused
everywhere.

## 24. `ConsensusResult`, `CommitError` and `ConsistencyError` gain `NotAMember` (2.32.0)

**What changes.** A group proposal from a node that is not in the group's roster is refused by name —
`ConsensusResult::NotAMember { slot, group }`, `CommitError::NotAMember { slot, group }`,
`ConsistencyError::NotAMember { group }`, and **403 `not_a_member`** from `POST /gateway/overlay/elect` and every
gateway route that reaches a group proposal — where it used to run, counting the proposer's own vote toward a
quorum drawn from a roster it was not in (CHANGELOG, 2.32.0 § Fixed). `cluster_propose` is unaffected.

**Will the compiler tell me?** Only if you matched without a `_` arm — all three enums are `#[non_exhaustive]`
(§13), so an exhaustive `match` inside this crate's dependants already needed one. **That arm must fail closed:**
a refusal read as a commit is the class of bug `ElectorateUnavailable` ended, one door over.

**Migration.** Join the group before proposing to it — `mesh().join_group(..)`, `POST /gateway/mesh/group`, or
`/gateway/govern/group` (`govern:write`) for a governed group. A client that elected a leader for a group its
node had not joined was never electing anything the group's members agreed to.

## 25. The gateway's doors answer only what they were asked by whom (2.32.0)

**What changes.**
- **The signal SSE streams refuse a protected kind.** `GET /signals/{kind}` and `GET /gateway/signal/sse/{kind}`
  answer `403 {"error": "protected_kind"}` for `mcp.invoke`, `skill.invoke`, `llm.invoke` and anything in
  `protected_rpc_kinds` — the body the raw routes have sent since 2.15.0. They used to stream every such request's
  frame (caller envelope, carried mandate, nonce) to any `mesh:read` holder. They refuse `rpc.result` and
  `bulk.result` the same way: a reply nobody claimed (late, or misrouted) used to stream to them.
- **`POST /gateway/rpc/respond` answers only a request your own `rpc/serve` stream delivered**: another principal's
  request, a nonce never streamed, or one older than the gateway's 300 s window is `403 {"error":
  "unserved_request"}` and nothing is emitted. The first answer is delivered; **a repeat from the same principal is
  `200 {"ok": true, "duplicate": true}`** and dropped (counted, `/stats` `rpc_respond_duplicates`) — so replicated
  serve loops under one bearer, which all receive each request, keep running. Any `mesh:serve` holder used to be
  able to answer any in-flight call by nonce. The 300 s window is the gateway's RPC ceiling: every gateway door
  waits at most that long (`/gateway/llm/call`'s `timeout_ms` is now clamped to it); an **in-process**
  `rpc_call` to an SDK-served kind with a longer timeout gets no reply past 300 s. On the mesh side a reply that
  carries a pending call's nonce from a node the call was not sent to is ignored and counted
  (`SystemStats::rpc_reply_sender_mismatches`, `/stats`); it used to consume the pending call and time the caller
  out. The sender is written by the emitter, so this closes the accidental case — a stray or late reply — not a
  peer that writes the target's id as its sender.
- **`mycelium-reason` 0.8.0: the façade routes act as the HTTP client.** `POST /gateway/reason/route` and
  `/gateway/reason/v1/chat/completions` carry the resolved principal to the provider and run the gateway's action
  preflight; a denial is `403` (`{"error": "policy"}` / OpenAI `permission_error`). Under the secure profile a
  provider without a caller-context marker at the gateway is failed over rather than called as the node.
- **An A2A task belongs to the identity that created it.** `tasks/get` and `tasks/cancel` answer only the bearer's
  principal that sent the task, and a `tasks/send` / `tasks/sendSubscribe` under an id another identity owns is
  refused — `-32004` — after any federation authorisation and before anything is dispatched (the id is reserved
  atomically, so two senders racing one new id cannot both run). An anonymous caller's task is answered only on the
  response (or stream) that created it. Any client used to be able to read or cancel any task by its caller-chosen
  id. `-32001` (not found) and `-32004` (another identity's) stay distinct: a send under a taken id must be refused
  anyway, so hiding existence on `get` would buy nothing.
- **`mycelium-tuple-space`: `GET /api/tuple` is `GET /gateway/tuple/overview`**, behind `tuple:read`; the old path
  answers 404. It sat outside the gateway's auth boundary and answered without a bearer.
- **`/gateway/llm/call` and `/gateway/llm/stream` run the action evaluator**: a denial is `403 policy` on `/call`
  and an in-stream `{"type": "error", "error": "policy"}` on `/stream`. Inert without an evaluator attached.
- **The OIDC key refresh is single-flight and rate-limited.** A token whose `kid` the node has not seen forces at
  most one JWKS refresh per 30 s (`JWKS_REFRESH_COOLDOWN`), and the fetch has a 10 s deadline; inside the cooldown
  such a token is refused without a fetch. After an IdP key rotation, tokens under the new key can therefore be
  refused for **up to 30 s** if an unknown `kid` was seen just before.

**Will the compiler tell me?** `SystemStats` gained a field, so an exhaustive struct literal breaks, and an
exhaustive `match` on `mycelium_reason::RouteError` needs the `Refused` arm; otherwise no — HTTP status codes and
JSON-RPC error codes.

**Migration.** A reader subscribed to a protected kind should read the decision trace (`mycelium explain`) or the
evidence journal instead; nothing in the SDKs subscribed to one by default. An SDK agent that serves and responds
under one bearer (every SDK does) needs nothing — replicas included: the losing replica's answer is accepted as a
duplicate. One that split serving and responding across two credentials must use one. Alert on
`rpc_reply_sender_mismatches > 0`: it names a misbehaving or misrouted peer (not every forger — see above). An A2A client that polls
`tasks/get` sends it under the same bearer as its `tasks/send`; an anonymous one reads the result it was already
given. A dashboard reading `/api/tuple` reads `/gateway/tuple/overview` with a `tuple:read` bearer.

## 26. A gateway off loopback needs a credential (2.32.0)

**What changes.** A node whose gateway binds a **non-loopback** `http_addr` — `0.0.0.0`, `::`, a LAN or pod
address — with **no credential model** (no `gateway_auth_token`, no `gateway_named_tokens` /
`gateway_scoped_tokens`, no `[oidc]`) **refuses to start**: `InvalidField { field: "http_addr" }`, the reason naming
the credential settings this build honours and the opt-in. It used to start and serve every gateway route to anyone
who could reach the port. Loopback is `127.0.0.0/8`, `::1` and an IPv4-mapped loopback; development on the default
`127.0.0.1` is unchanged. **A blank token anywhere is refused at `validate()`**, by the field's name: an empty or
whitespace-only `gateway_auth_token` (including `GOSSIP_GATEWAY_AUTH_TOKEN=""`), or a `token` in any
`gateway_scoped_tokens` / `gateway_named_tokens` entry — on every address and in every profile; such a node used to
start and, over HTTP/2, admit an empty bearer. Under
`profile = "secure-single-domain"` the profile is now **rev 4** (rev 3 added `gw.exposed_closed`, rev 4 adds `cons.safety_profile` — §31). Rev 3's addition, `gw.exposed_closed` (with `gw.not_open` at rev 2), refuses one node rev 2 admitted — a gateway whose only credential was a blank `gateway_auth_token`, which `gw.not_open` rev 1 read as enforced (and which `validate()` now refuses in every profile) — and otherwise names the waiver when `gateway_allow_unauthenticated` is set.

**Will the compiler tell me?** No — a start-time refusal. `GossipConfig` gained a field
(`gateway_allow_unauthenticated`), so an exhaustive struct literal breaks; `..Default::default()` is unaffected.

**Migration.** Check before upgrading: a node with `http_addr = "0.0.0.0"` (or `GOSSIP_HTTP_ADDR`) and no
credential, and any blank token (an empty env variable included — unset it instead). Set `gateway_auth_token` (or, in a `compliance` build, a token table or `[oidc]`) — the fix — or bind
`http_addr = "127.0.0.1"` behind a proxy. Where the network in front of the port really is the boundary (a private
Docker network, a demo), set `gateway_allow_unauthenticated = true` or `GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED=1`;
the node then warns once at start, and its guarantee report reads `gw.exposed_closed: not_configured`. The
environment variable is a strict boolean: a value other than `true/false/1/0/yes/no` is refused.

## 27. Hosted WASM components are bounded in time and memory, confined on the mesh, and installed only from named publishers (2.32.0)

**What changes.**
- **Every guest call and instantiation has a wall-clock deadline**, `DEFAULT_CALL_DEADLINE` (5 s), metered or not and
  whoever signed the entry; past it the call ends `WasmHostError::DeadlineExceeded { deadline_ms }` /
  `InvocationOutcome::DeadlineExceeded` (counter outcome `deadline_exceeded`). Set it per host with
  `[hosts].call_deadline_ms` or `WasmHost::with_call_deadline` (`0` is refused by name; `with_call_deadline(None)`
  removes it). The serve loop and install run the guest on `spawn_blocking`; a trapped instance is re-instantiated
  from the compiled component rather than recompiled.
- **A component's linear memory is capped** at `DEFAULT_MEMORY_LIMIT_BYTES` (256 MiB; `HostState::with_memory_limit`
  per instance): a grow past it is answered no, and an initial allocation past it refuses instantiation.
- **`mesh.emit` is confined**: a component emits only under `comp/{namespace}/…` or a kind the host listed
  (`HostState::with_emit_kinds`), and never a protected kind (`mcp.invoke`, `skill.invoke`, `llm.invoke`, the node's
  `protected_rpc_kinds`). A refused emit is dropped, logged and counted
  (`mycelium_wasm_host_emits_refused_total{reason}`) — the WIT signature has no error channel.
- **A `[hosts]` table with no `trusted_publishers` refuses to load** (`hosts.trusted_publishers`); it used to install
  anything any peer published. `[hosts] accept_unsigned = true` keeps the old behaviour on purpose, and warns once.
- **A shadow-lane `tool/*` proposal is not an MCP tool** until it is accepted (it used to appear in `tools/list`).
- **The staging pull is bounded before its first byte** by the stage's ceiling (`DEFAULT_MAX_STAGE_BYTES`, 64 GiB;
  `StemOptions::max_stage_bytes`) and the entry's `size_bytes` hint; a failed stage backs off.

**Will the compiler tell me?** Partly. `WasmHostError` and `InvocationOutcome` each gain a variant and neither is
`#[non_exhaustive]`, so an exhaustive `match` needs the arm; `ConfinementError` gains `ProtectedKind`, `ForeignKind`
and `MalformedNamespace`, and `HostState::emit` now returns `Result`; `HostsDecl` (`call_deadline_ms`), `StemOptions`
(`max_stage_bytes`) and `RuntimeCtx` (`shadow`) gain fields, so an exhaustive struct literal breaks
(`..Default::default()` is unaffected). The deadline, the memory cap, the emit refusal and the `[hosts]` refusal are
runtime behaviour.

**Migration.** A guest call that legitimately runs past 5 s: raise `call_deadline_ms`. An embedder calling
`Instance::invoke` (still synchronous) from async code: move it to `spawn_blocking`, as the serve loop now does. A
component that emitted a kind outside its namespace: list the kind with `with_emit_kinds` if it is not protected work
(code-only for now — no unit-file field yet). A unit file with `[hosts]` and no publishers: add the publisher keys, or
`accept_unsigned = true` if unsigned installs are what you mean.

## 28. Durability: a failed append is not followed, a corrupt companion record refuses the open, and consensus records its answers first (2.32.0)

**What changes.**
- **A failed KV WAL append poisons the writer** until a snapshot repairs the file: every later append is answered
  `Err` (`LocalDurability::Failed` on the receipt path) or counted in `dropped_appends()`, and `/health` carries a
  `persistence` block (`wal_refusing_appends`, `reason`, `dropped_appends`). It used to acknowledge the next record
  behind a torn frame, and the next restart refused (or quarantined) the WAL.
- **The companion WALs (`mycelium-tuple-space`, `mycelium-blackboard`)** poison the same way; a corrupt record with
  data after it **refuses the open** (`InvalidData`, naming the file and the byte) — so the node does not become
  primary, and an Auto-mode node withdraws its candidacy — where every later record used to be truncated silently;
  a second store on one `wal_path` is refused (`WouldBlock`), with a `<wal_path>.lock` file beside each WAL; a
  secondary now syncs and compacts its WAL too; a compacted WAL may end in a lone `Ack` (older builds replay it as
  a no-op).
- **`persist.sync_mode` reads `not_configured` under `sync_mode = "os"`**, which is buffered like `async`, so a
  `secure-single-domain` node running it refuses to start.
- **The audit chain continues across a restart** with `[persistence]`: the head resumes after the longest verified
  prefix of the node's own stream, where a restarted node used to re-seal seq 0 over its genesis record. An exporter
  sees sequence numbers continue rather than restart.
- **Consensus records an acceptor's promise and vote before it answers** (a forced `fdatasync` per answer); one that
  cannot is not sent. `mycelium_consensus_timeouts_total{reason}` gains **`unrecorded`** (this node could not make its
  answer durable) and **`ballot_exhausted`** (the slot's decided floor is at the ballot ceiling — a forged floor,
  counted on `consensus_decided_floor_anomalies`).

**Will the compiler tell me?** `SystemStats` gains three fields (`consensus_decided_floor_anomalies`,
`consensus_ballot_space_exhausted`, `rpc_reply_sender_mismatches`), so an exhaustive struct literal breaks; the rest is
runtime behaviour.

**Migration.** Treat a `Failed` receipt as *not durable* and alert on `wal_refusing_appends`. A companion WAL that
refuses its open holds a corrupt record before its last: move the file aside to start empty (there is no
`on_unreadable = "quarantine"` counterpart), and do not delete a `.lock` file while a node runs. A secure-profile node
on `sync_mode = "os"`: set `flush`. An alert keyed on a consensus-timeout `reason` (§22): add `unrecorded` — read the
node's `/health` `persistence` block when it fires — and `ballot_exhausted`.

## 29. The SDKs send whole seconds and one path segment per value; TypeScript `scatterGather` waits for one reply (2.32.0)

**What changes** (`mycelium-py` 0.2.9 → 0.2.10, `mycelium-ts` 0.2.3 → 0.2.4, `langgraph-checkpoint-mycelium` 0.3.2 →
0.3.3).
- **Every client takes a scheme** (`scheme="https"` / `{ scheme: "https" }`, default `http`) and Python and the
  checkpointer a CA file (`ca_file=`); **the checkpointer sends a bearer** (`token=`, then `MYCELIUM_GATEWAY_TOKEN`)
  and needs `kv:read`, `kv:write`, `llm:read` **and `llm:write`**. Additive.
- **Timeouts, leases and TTLs travel as whole seconds, rounded up** (never below 1; a tuple `take`'s `0` stays the
  poll) — the gateway reads an integer, so a fraction used to be replaced by the route's default, refused 422, or
  truncated to a zero-second park. A negative, non-finite or past-`u64` value is refused by name (`ValueError` /
  `Error`) before any request.
- **A caller-supplied path segment is percent-encoded as one segment**, and a segment that cannot travel as one —
  `.`, `..`, empty, a lone surrogate — is refused before any request; the checkpointer reads such a row's blob id as
  `corrupt` (`IncompleteCheckpoint`) and never fetches it.
- **TypeScript:** a lost consistent write throws `SupersededError` (`status` 409); **`scatterGather` with no `minOk`
  waits for one reply and 10 s**, the gateway's and the Python SDK's default, where it waited for every target and
  5 s; `scatterGather([])` with no `minOk` now rejects with a `TimeoutError` at once (the gateway's 504) where it
  resolved `[]`.
- **A mailbox stream is at-most-once across a drop** (the gateway tombstones each event as it enters the stream's
  queue); the docs said at-least-once. No SSE stream resumes after a drop — the gateway has no resume point.

**Will the compiler tell me?** No — the TypeScript `SupersededError` still extends `Error`, and the rest is values
and runtime refusals.

**Migration.** A TypeScript caller that relied on hearing from every target passes `{ minOk: targets.length }`, and
one that may pass an empty target list passes `{ minOk: 0 }` or skips the call. A caller that passed a fractional
timeout gets the next whole second. A caller whose ids legitimately contain `/` now reaches the route it named; one
that used `.` or `..` as an id must choose another. A checkpointer behind a token-protected gateway sets `token=`
(or the environment variable) and grants `llm:write`.

## 30. Quorum evidence is written by nodes that care about a kind (2.32.0)

**What changes.** `sys/quorum/{kind}/{sender}` — the evidence `quorum_persistent`, `last_signal_persistent` and a
`watch`'s fallback read — used to be written by **every** node that admitted a signal of that kind, for every
sender, at up to one write per `(kind, sender)` per second. A sender chooses both the kind and the sender id it
puts in a frame, so a flood of invented kinds or senders became KV keys appended to the WAL, gossiped to the fleet
and never collected (row B of the post-360 hardening plan; #602's reviews). Now a node writes evidence for a kind
only when:
- a local **worker** subscribes to it (`signal_rx`, `rpc_rx`, a serve stream — not an SSE tap), or
- it was **asked about** recently: `quorum`, `quorum_for_group` and `last_signal` pin the kind for one
  `signal_window_secs`; `quorum_persistent(kind, window)` pins it for `max(window, signal_window_secs)`;
  `last_signal_persistent` for one `signal_window_secs`. A `watch` renews its pin on every poll.

Per kind, a node writes evidence for at most 1024 senders at a time (`SIGNAL_LOG_MAX_SENDERS_PER_KIND`): a
further sender is skipped and counted (`gossip_quorum_evidence_skipped_total`) until a slot's last write is older
than `signal_window_secs`. That bounds a **rate, not a total** — about 1024 new evidence keys per kind per signal
window; the keys are not collected, and a reader's own window (`quorum_persistent(kind, window)`) may be longer
than the signal window, so older keys can still count there. Forged senders on a kind this node works on add keys
at that rate. Across kinds, at most 4096 distinct `(kind, sender)` writes a second. A kind with no worker whose pin
has lapsed forgets its senders at the next GC tick.

**Who notices.**
- **An observer outside a group.** A node reading `quorum_persistent("k", …)` for a kind its *members* emit used to
  see evidence written by every member that relayed `k`. Members with no worker on `k` and no query of it now write
  nothing; the evidence comes from nodes that subscribe to or ask about `k` — including the observer itself, from its
  first query on (its own reads pin the kind).
- **A poller slower than its window.** A reader polling `quorum_persistent` less often than its pin lasts sees a gap
  between pins; the pin lasts at least the window it asks about, so polling within that window keeps it.
- **More than 1024 senders on one kind within the signal window**, or more than 4096 distinct `(kind, sender)`
  evidence writes in one second: the excess is not recorded.

**Will the compiler tell me?** No — fewer keys under `sys/quorum/`.

**Migration.** A node that must contribute evidence for a kind should subscribe to it (a worker), or call
`quorum_persistent(kind, window)` / `last_signal_persistent(kind)` at least once per window. Keys written before the
upgrade stay; nothing collects `sys/quorum/`.

## 31. A safety-sensitive proposal needs an electorate group, whose members are fixed at genesis (2.32.0)

**What changes.** Post-360 plan row P2 (`docs/design/consensus-electorate.md` §8). With `consensus_require_electorate`
(env `GOSSIP_CONSENSUS_REQUIRE_ELECTORATE`; required by `profile = "secure-single-domain"` **rev 4**, guarantee
`cons.safety_profile`), a lock, a leader election, a consistent write, a capability-authz policy, a commitment award or
any proposal flagged `safety_sensitive` is refused `ElectorateNotGoverned` unless an **electorate group** decides it.
An electorate is a genesis decided by every member it names, and its membership is **fixed at genesis**: declaring a
changed roster is `MembershipFrozen`. In 2.32.0 changing an electorate's members was not built; it is now a migration
(§34). In 2.32.0 the fleet's exclusive default is set once, at genesis (a second is refused by name), and cannot move;
since §34 it moves by the holder's retirement.

**Will the compiler tell me?** Partly. `ConsensusResult` (`#[non_exhaustive]`), `CommitError` and
`ConsistencyError` gain `ElectorateNotGoverned`, `ElectorateStale` and `ElectorateMismatch` — a `_` arm must fail
closed. `ConsensusConfig` and `GossipConfig` gained fields (an exhaustive literal breaks). The rest is refusals by name.

**Migration.** Under the secure profile: set `consensus_require_electorate = true` (rev 4 refuses `start()`
otherwise), declare the electorate groups from their rosters (`POST /gateway/govern/electorate`), and mark one
`exclusive_default` for the cluster-scoped verbs — once, only once every node runs this release. **Size each
electorate for the failures it must survive for its whole life**: a dead member lowers its fault tolerance, and a lost
majority stops it — including, for the default group, the cluster-scoped exclusive verbs. A `lock/`, `consistent/`,
`capauthz/` or commitment-award slot proposed on any group but the fleet's default is refused, as is `leader/{g}` on
any group but `g`.

**The surface, in one place** (added at the 2.32.0 cut):
- **API:** `GossipAgent::declare_electorate(group, exclusive_default)` decides a genesis (eight tries, 250 ms doubling
  to 4 s) and returns the electorate unchanged on a group that has one; `ElectorateError::MembershipFrozen` for a
  changed roster, `ElectorateError::Refused` for a precondition no retry changes (a door refusal, a second default).
  `ConsensusHandle::exclusive_propose` / `exclusive_propose_receipt` decide a cluster-scoped exclusive slot in the
  fleet's `exclusive_default` group — `distributed_lock`, `LockService`, `consistent_set`, the lock, consistent and
  log-claim routes, `mycelium-commitment`'s linearizable award and `set_capability_authz_via_consensus` now go there.
  `consensus_electorate` only restates the default; a disagreeing setting is refused.
- **HTTP:** `POST /gateway/govern/electorate` (`govern:write`, audited when something is decided); `409
  membership_frozen`, `409 refused`; `403 electorate_not_governed`; an `ElectorateMismatch` / `ElectorateStale`
  answer on the consensus routes.
- **Counters:** `mycelium::electorate_records_refused` (a record written straight into the store, not adopted) and
  `mycelium::electorate_roster_mismatches` (a roster that differs from the member set by identity — the trace an
  embedded `join_group` or a `grp/` write leaves on an electorate group).
- **Wire v12, appended:** `PrepareIn`, `ProposeIn`, `StaleElectorate`; an acceptor answers only its own electorate.
- **Rolling upgrade:** an electorate whose members are not all on 2.32.0 times out rather than commits, and a pre-P2
  node still proposes the exclusive verbs to the whole cluster — mark an `exclusive_default` only once every node runs
  2.32.0.
- **Stated limits (2.32.0):** changing an electorate's members, or moving the default, is not built in this release
  (both are §34 now); a lost majority is unrecoverable; **two default declarations made at once, within gossip lag, stop the exclusive verbs
  fleet-wide with no escape in this release** (group-scoped work outside the exclusive families keeps running);
  genesis assumes its members agree on the roster; without `[tls]` the certificates are unauthenticated.
- **The profile, in two steps this release:** `secure-single-domain` was rev 2 in 2.31.0; **rev 3** adds
  `gw.exposed_closed` (§26) and **rev 4** adds `cons.safety_profile` (this section). A 2.32.0 node under the profile
  needs both.

## 32. The transport is bounded: silent, slow and stalled peers are closed; the residual is stated (2.32.0)

**What changes** (row B of the post-360 plan, as narrowed by the maintainer on 2026-10-11).
- **Accept path:** the TLS accept, then the first byte of the first frame, must each arrive within
  `handshake_timeout_ms` (10 s); an established connection silent for `inbound_idle_timeout_secs` (300 s; `0` =
  never) is closed. A frame being received must move a byte every `peer_read_stall_timeout_ms` (60 s) and keep
  **1 KiB/s** after it (`peer_min_rate_bytes_per_sec`). A header no longer reserves the frame size it claims.
- **Outbound writer:** connect + TLS bounded by `handshake_timeout_ms`; a frame being sent must move a byte every
  `peer_write_stall_timeout_ms` — **1 140 s (19 min) by default**, 1.5 × the measured worst chunk a `flush` receiver
  applies while not reading; eviction and shutdown interrupt a blocked write. A peer has at most one anti-entropy
  reply in flight.
- **Signals:** admission rolls against the least-full work-bearing subscriber (SSE routes are taps and never count),
  so one stalled subscriber no longer vetoes its kind; the signal log keeps at most 4096 kinds and 1024 senders per
  kind; `sys/quorum/` evidence as in §30. `RpcRequestRx::recv` ends at shutdown.

**The residual, plainly:** the accept path is bounded against silent and slow connections; **an attacker that keeps
talking can hold the connection slots it obtained from free permits**, up to `max_connections`. Mitigations: mTLS (a
non-member fails the handshake within `handshake_timeout_ms`) and the opt-in `max_connections_per_source` (default
off; exact IPs, so an IPv6 /64 is not covered). On the write side there is no rate floor: a peer that reads one byte
per stall window parks its own writer and its reply for as long as it keeps doing so.

**Will the compiler tell me?** Partly. `GossipConfig` gains the six fields above, `SystemStats` eight counters
(`inbound_connections_timed_out`, `inbound_frames_stalled`, `inbound_per_source_refused`, `outbound_stalls`,
`anti_entropy_replies_skipped`, `signal_handler_drops`, `signal_log_kinds_evicted`, `signal_log_kinds_refused`) and
`CoreCtx` the public `transport_bounds` — an exhaustive struct literal breaks; `..Default::default()` is unaffected.
`mycelium_core::writer::{run_peer_writer, get_or_spawn_writer, request_state}` take a `WriterTiming` (no longer
`Copy`) where they took the idle `Duration`, and `ConnContext::writer_idle_timeout` is `writer_timing`; the `bytes`
requirement is `1.9`.

**Migration.**
- `validate()` refuses `inbound_idle_timeout_secs` at or below a non-zero `writer_idle_timeout_secs`; a fleet whose
  writers never idle-close (`writer_idle_timeout_secs = 0`) sets `inbound_idle_timeout_secs = 0`.
- A joiner bootstrapping from many peers over a thin shared link needs senders × 1 KiB/s: set
  `peer_min_rate_bytes_per_sec = 0` there.
- A fleet with no `flush` nodes can lower `peer_write_stall_timeout_ms` well below 19 minutes.
- On a plaintext mesh, or where one address may hold many connections, set `max_connections_per_source`; alert on
  `inbound_connections_timed_out`, `inbound_frames_stalled` and `outbound_stalls`.
- Opacity hints and local emit shedding read `SignalHandlers::fill_ratio` as the least-full work-bearing subscriber.

## 33. Leadership is leased, and a lock, a lease and a leadership end in one decision record (2.32.0)

**What changes** (row A of the post-360 plan, with C1 and C2; design record `docs/design/lock-lifecycle.md`).
- **`elect_leader` and `elect_leader_receipt` lease 30 s by default** (`DEFAULT_LEADER_LEASE`). Calling again
  renews and keeps the lineage; **`release_leadership(group)`** (new, async, durable) steps down, a renewal in flight
  included; `elect_leader_with(group, LeaderTerm::Permanent)` is the opt-in for the old behaviour. Over HTTP,
  `POST /gateway/overlay/elect` leases by default and takes `ttl_secs` or `"permanent": true`; `DELETE
  /gateway/overlay/elect/{group}` steps down (`200`, `404 not_leader`, `500 release_unrecorded`). Before 2.32.0 a
  leadership was permanent and a dead leader was reported for ever.
- **What Paxos decides is a decision envelope** — the value with its window, lineage, original proposer and fencing
  token — written under `consensus/life/{slot}/{ballot}` and read by the highest ballot; a release writes a marker.
  `consensus/decided/` is a refusal floor only. A lock's token is now the decision's token (still monotonic across
  holders when the prior decision's record reached the new proposer).
- **`LockGuard` has a deadline:** `expires_at_ms()`, `deadline()` and `is_expired()`; a guard is never issued past
  its deadline, and the gateway answers **409 `expired_before_grant`** where it would have been. A lock's lease is
  its TTL rounded up.
- **A legacy value adopted in flight is wrapped in a lease**, never made permanent — the adopter's lease, or **30 s**
  when the adopter asked for permanence: a 2.31 permanent leadership released the legacy way is not resurrected by the
  next upgraded proposal. The cost: a slot decided once by a 2.31 node and adopted while its decision is still in
  flight can take a different value after that 30 s wrap ends (`lock-lifecycle.md` §7) — **decide a commit-once
  permanent slot after the upgrade completes**. A slot committed permanently before the upgrade has no decision
  record and is read, and released, the legacy way.
- **Acceptor state is collected** once its decision or lease is over (a collector task beside each listener,
  bounded per pass): the slot shrinks to a node-owned, fsynced floor; lease markers go after their expiry plus the
  drift bound, permanent markers never.
- **Wire v12, five appended consensus variants** (`PrepareTerm`, `PrepareAckTerm`, `ProposeTerm`, `PromiseTerm`,
  `CommitTerm`) and an acceptor-record tag `0x03`. An upgraded node still writes the legacy keys
  (`consensus/committed/`, `lease`, `decided`) and the legacy `Commit`, which older nodes read the old way.

**Will the compiler tell me?** Only in part. `LeaderTerm` is new and `#[non_exhaustive]`; `LockGuard`'s fields are
private, `ConsensusListenerHandle` and `TaskCtx` gained private fields — nothing an embedder constructs. The lease
itself is runtime behaviour: code that elected once and assumed it for ever compiles unchanged and loses the
leadership after 30 s.

**Migration.**
- **A caller that holds leadership** renews by calling `elect_leader` again well inside the lease (every ~10 s), or
  asks for `LeaderTerm::Permanent` / `"permanent": true` on purpose; step down with `release_leadership` / `DELETE
  /gateway/overlay/elect/{group}` rather than by stopping. The SDKs expose no release verb yet (row G); their
  `elect_leader` now returns a 30 s leadership — call it again to renew.
- **A lock holder** checks `is_expired()` / `deadline()` before acting and fences the resource on the token; a
  client that sees 409 `expired_before_grant` retries.
- **Upgrade every node.** An upgraded proposer times out rather than commits until a quorum of its acceptors is
  upgraded (they ignore `ProposeTerm`), as with §21. During the window an older reader still reads the legacy keys,
  with their old failure modes (a late COMMIT can re-stamp a released lock there), and an adoption of a value first
  proposed by an older node takes the adopter's window.
- **Tests that withhold frames** build with `mycelium`'s `test-frame-filter` feature (`mycelium-core/test-frame-filter`);
  nothing shipped enables it.

**Not built:** an SDK `release_leadership` verb; collection on a node that proposes without a listener; releasing
a permanent decision whose original proposer is gone for good (design §5).

## 34. An electorate's members change by a migration that terminates (2.33.0)

**What changes.** Post-360 plan row P2's follow-up (`docs/design/consensus-electorate.md` §8.3). 2.32.0 said *declare a
new group and move the work yourself — nothing decided in the old group is carried* and *the default cannot move*.
Now who decides changes by a **migration** in three governance acts, each a consensus decision: declare the
**successor** first, naming its predecessors (`declare_successor_electorate`, `{"group": "G2", "predecessors":
["G1"]}`) — it decides nothing until each has retired to it; **retire** the group into it
(`retire_electorate`, `{"group": "G1", "retire_to": "G2"}`) — a strict majority of G1 decides, every member closes G1
at its promise, and the fleet's default passes with it when held; **complete** (`complete_migration`, `{"group":
"G2", "complete": true}` on a member of G2) — the carry set is enumerated from a strict majority of G1's acceptor
memory, every slot is carried with one ballot of G2 (the decision envelope whole: a lock keeps its holder's window, a
released lock stays released), and `electorate/G2/carried/G1` is decided, after which G2 decides anything without G1
and G1's members may be removed. Before the completion G2 decides nothing outside the carried slots, at the door and
at every acceptor. Two or more predecessors is a merge. A predecessor that cannot retire into G2 any more (its
retirement to another successor is visible) is **dropped** (`drop_predecessor`, `{"group": "G2", "drop_predecessor":
"P"}`; a G2-majority decision); `"force": true` asserts, outside the protocol's guarantee, that P's majority is gone for
good — audited, refused without it while P may still retire, and a false assertion ends as a detected conflict on the
carry, never a prevented one. A group retires only once every predecessor is complete or dropped — checked by every
acceptor at the prepare, before it closes. A carry that finds another value already decided above the carried one
reports the slot as a `conflict` in the completion and counts it on `commit_conflicts`; the completion still decides. `GET /gateway/govern` shows `predecessors`,
`retired_to` and each predecessor's `migration` phase.

**Will the compiler tell me?** Partly. `ElectorateDecl` gained `predecessors` and `Retirement` is new (an exhaustive
literal of `ElectorateDecl` breaks; `..Default::default()` is unaffected); `ElectorateError` (`#[non_exhaustive]`)
gained `Retired`, and `MigrationState` / `MigrationPhase` are new. `AcceptorSlot` gained `groups` (crate-private).
The rest is refusals by name.

**Migration.** Nothing to do for a fleet that never migrates: a genesis without predecessors is byte-for-byte
2.32.0's, and the acceptor record of a slot never claimed on an electorate group is unchanged. To migrate: every
member of the retired group and of the successor runs this release — a 2.32.0 member ignores the four appended
variants (`CarryManifestRequest`, `CarryManifestPage`, `CarryPrepare`, `CarryPropose`), so a predecessor needs a strict
majority of upgraded members to be enumerated and a successor with an un-upgraded member times out its carry ballots;
a 2.32.0 node decodes a successor's genesis without its predecessors and computes another digest, so it never answers
the successor and the successor never answers it. Migrate while the group **has its majority**: a group that lost
it cannot be retired or enumerated, and its repair is not built. Configure `[persistence]` on every member (§8.3's
restore-or-retire rule): the carry set is complete because the group tag is in the durable acceptor record.

**The surface, in one place:**
- **API:** `GossipAgent::declare_successor_electorate(group, &[predecessors])`, `retire_electorate(group, successor)
  -> Retirement`, `complete_migration(group) -> Vec<MigrationState>` (idempotent; `NotDecided` names the slot a carry
  stopped at, retried with the bounded backoff), `drop_predecessor(group, predecessor, force) -> PredecessorDrop`,
  `migration_state(group)`, `electorate_retirement`, `electorate_completion`, `electorate_drop`;
  `ElectorateError::Retired { successor }`.
- **HTTP:** `POST /gateway/govern/electorate` with `predecessors`, `retire_to`, `complete` or `drop_predecessor` (`govern:write`; each
  decision audited, an undecided retirement attempt too); `409 retired`, `409 refused` (a successor that does not name
  the retiree, a predecessor retired elsewhere, a retirement with an incomplete carry, a retirement that drops or
  claims the default wrongly), `409 not_decided` (a record not visible here yet; a carry that could not finish, naming
  the slot). `GET /gateway/govern` lists each electorate's `predecessors`, `retired_to`, `migration`.
- **Counters:** `mycelium::electorate_waiting_refusals` (an acceptor refusing while a migration is not visible to
  it), `electorate_retirements_undecided`, `migration_slots_carried`, `migration_completions`,
  `migration_manifest_pages`; metrics `mycelium_electorate_waiting_refusals_total`,
  `mycelium_electorate_retirements_undecided_total`, `mycelium_electorate_slots_carried_total`,
  `mycelium_electorate_migration_completions_total`, `mycelium_electorate_manifests_total`.
- **Wire v12, appended:** `CarryManifestRequest`, `CarryManifestPage`, `CarryPrepare`, `CarryPropose`; the
  retirement, the completion and the drop are ordinary `PrepareIn`/`ProposeIn` on `electorate/{g}/retire`,
  `electorate/{g}/carried/{p}` and `electorate/{g}/drop/{p}`, certified under `consensus/electorate-cert/{g}/…`. A
  2.32.0 member of a retired group votes for its retirement **without closing**: with one such member a value can be
  chosen after the retirement and safety rests on the carry set (enumerated from the upgraded members) and the
  conflict tripwire; with two, no majority of manifests can be gathered — stuck, not unsafe. Migrate once every member
  is upgraded.
- **On disk:** a slot claimed on an electorate group writes its acceptor record with tag `0x04` — the 2.32.0 record
  plus the group names — a few bytes per slot per node, never removed.
- **Stated limits:** a group that lost its majority cannot migrate (DR not built); the carry is one ballot per slot,
  sequential; a value a restarted predecessor member holds only by digest and no decision record resolves stops the
  carry at that slot, named; the claim is for slots decided only in the electorate (an ordinary slot also proposed
  under another scope is not enumerated); two default declarations within gossip lag are escaped by a merge.

## 35. Wire and crypto: a TLS node can refuse unsigned KV frames, KV signatures are domain-tagged, a certificate's key is read structurally, and §23 closes (2.33.0) — window closed in 2.34.0 (§40)

> **Closed.** The two allowances below held for 2.33.0 only. From the release after it (§40) a TLS node refuses an
> unsigned KV `Data` frame and a bare KV `SignedData` signature whatever `kv_require_signed_frames` says; the setting
> is a retired no-op (accepted, warned at `start()` when set, `#[deprecated]`); `kv_unsigned_frames_accepted` and
> `kv_untagged_signatures` stay on `GET /stats` at zero and their metrics are no longer emitted. **Upgrade a 2.32
> fleet to 2.33 before going further**: a 2.32 node's KV writes reach a node past 2.33 only by anti-entropy. The
> anti-entropy residual below is restated in §40, not closed.

Post-360 plan row F. Wire **v12** unchanged — no frame gains or loses a byte; what changes is which frames a node
accepts and what a signature is over.

**What changes.**
- **KV `SignedData` is domain-tagged.** A node signs `mycelium.kv/update/1 ‖ u32 len ‖ canonical` rather than the
  canonical update bytes bare (`framing::kv_signing_message`), beside the identity proof's and the consensus
  payload's tags (§23). **One release:** an upgraded receiver verifies the tagged form first and still accepts the
  bare 2.32 form, counted (`GET /stats` `kv_untagged_signatures`, `mycelium_kv_untagged_signatures_total`). Frames,
  not peers.
- **A signed frame's signer must be its sender.** A `SignedData` frame whose `signer` is not the update's `sender` is
  dropped and counted (`kv_signer_mismatch`, `mycelium_kv_signer_mismatch_total`, a warning naming both and the key).
  Before, the signature was checked against `signer`'s key and the update applied as `sender`'s, so a member could
  write as anyone under its own valid signature. No window: no version of Mycelium sends such a frame.
- **Every write a TLS node originates is signed.** Twenty-four write sites — membership, mailboxes, state machines,
  opacity, the gateway's log/KV/offset routes, MCP tombstones, the soft-state advertiser, and the connection
  handler's quorum evidence (`sys/quorum/…`) — built a bare `Data` frame and never signed, even under `[tls]`. They
  now go through `framing::local_kv_wire_msg`, and `scripts/check-signed-dispatch.sh` refuses a new one.
- **A TLS node can refuse an unsigned `Data` frame.** Before, the frame was applied whatever the node's posture, so a
  member could write under another member's `sender` hash by not signing. **One release:**
  `kv_require_signed_frames` (`GOSSIP_KV_REQUIRE_SIGNED_FRAMES`) defaults to `false`; an unsigned frame is applied
  and counted (`kv_unsigned_frames_accepted`; `mycelium_kv_unsigned_frames_total{outcome="accepted"}`). Set it and
  the frame is dropped and counted (`kv_unsigned_frames_refused`, `outcome="refused"`). The guarantee report
  resolves it as `mesh.signed_writes` (`enforced` / `not_configured`); `secure-single-domain` does not require it
  this release. Plan row F2 — the next MINOR — refuses unconditionally. Without `[tls]` the setting is inert and
  `start()` warns so. **Exempt:** one unsigned frame only — a sealed identity record
  (`sys/identity-signed/{n}`, not a tombstone) that authenticates itself: its proof verifies under a key in its own
  history and, when the receiver already holds keys for `n`, under one of those (counted, `kv_identity_exempt_frames`,
  and noted at most once a minute per connection). A node publishes it first, unsigned, before its TLS identity is
  installed — first contact rests on it — and then publishes the legacy `sys/identity/{n}` and proof **signed**. Every
  other identity frame (a bare record, a proof alone, garbage, a tombstone) is refused under the flag like any
  unsigned frame. The first cut (#611's first fix round) exempted any identity frame whose `sender` named its key — and
  `sender` is the writer's to choose; the second review reproduced impersonation and silencing through it. A record must also **contain every key the receiver already holds or has stored for `n`** — a node's
  history only grows — so a genuine pre-rotation record replayed with a fresh timestamp is refused, whatever the flag
  says, and counted (`kv_identity_stale_refused`; #611's third review, N4). What remains: **first sighting is trust on
  first use** — with nothing held or stored for `n`, the first self-proving record is taken (the CA anchor of a direct
  connection is what closes it); and with `require_identity_proofs` off a member that has already written a key into
  `n`'s stored legacy record (under its own signature) makes `n`'s later unsigned sealed records stale here — they then
  arrive signed or by anti-entropy. The same refusal meets a legitimate node that rotated and then restarted
  without persistence (its start-time history is only its current key) or was re-keyed from scratch: its sealed record
  is counted stale where peers hold older keys for it, and the warning names that observation, not an attack. Peers
  that already hold its current key lose nothing; its signed writes still verify. **It is bypassable through
  anti-entropy** (below).
- **A peer certificate's key is read from its `SubjectPublicKeyInfo`, structurally** (`tls::ed25519_key_from_cert_der`),
  not by scanning the DER for the Ed25519 SPKI prefix. The scan returned the first match anywhere — a serial number,
  a name, an extension — so a node holding the CA key could mint a certificate that anchors as another member and
  passes a removal of its own key. Local; no window.
- **An identity tombstone removes no keys.** Before, one tombstone of `sys/identity/{n}` — unsigned, or signed by any
  member as itself — made every receiver's identity watcher drop `n`'s keys, after which `n`'s signed writes were
  dropped as from an unknown signer. Now a node's keys leave only through membership removal (the ledger) or
  accumulate through a verified rotation. A node is also found by its sealed record alone, not only by the legacy one.
- **§23 closes.** The bare identity proof and the bare consensus signature are refused; the two counters are gone.

**Mixed fleet, direction by direction** (the 2.30.0 shape, §21):
- **A 2.32 node's KV writes at an upgraded node: accepted, counted** — its `SignedData` as the bare form, its
  agent's unsigned `Data` frames as unsigned (refused instead if the upgraded node sets `kv_require_signed_frames`).
- **An upgraded node's KV writes reach a 2.32 peer only by anti-entropy, until that peer is upgraded.** Upgraded
  nodes sign tagged at once — the 2.32.0 shape, in which #585's proofs were tagged from the first upgraded node —
  and a 2.32 verifier checks the bare bytes only, so it drops an upgraded peer's `SignedData`, **does not forward
  it**, and **logs a `warn` for every frame it drops** (`SignedData … failed Ed25519 verification`). Every write an
  upgraded node originates — the twenty-four kinds above included — reaches a 2.32 node by the next anti-entropy
  exchange (`health_check_interval_secs`, 10 s by default), hop by hop, rather than by gossip. Expect latency, not
  loss, and a noisy 2.32 log: **upgrade the fleet promptly**, not over days. An upgraded node's own legacy identity
  record is among these writes now (signed), so a 2.32 node that is not directly connected to it learns its key by
  anti-entropy; a directly connected one has it from the TLS handshake. Put plainly: **a 2.32 peer finds an upgraded
  node only by anti-entropy unless it dialed that node itself, and until then the upgraded node's consensus messages
  to it go unverified** (dropped as from an unknown signer).
- **An upgraded node requiring signed frames drops a 2.32 peer's agent writes until that peer is upgraded** — set
  `kv_require_signed_frames` once every peer is past 2.32.
- **A 2.31 node and an upgraded node do not verify each other's identity proofs or consensus messages** (§23 is
  closed): upgrade through 2.32. Upgrading straight from 2.31 leaves a 2.31 node **worse off than one with no proof at
  all**: its bare identity proof is refused, so its key never enters an upgraded peer's `peer_keys`, every
  `SignedData` it sends is then dropped as from an unknown signer, and its consensus messages are dropped — where a
  node publishing no proof is still accepted on first use while `require_identity_proofs` is off. Upgrade to 2.32
  first.

**Will the compiler tell me?** Only for an exhaustive `GossipConfig` literal (a new field). The rest is wire
behaviour.

**Migration.** Upgrade from 2.32, not from 2.31. Watch `kv_untagged_signatures` and `kv_unsigned_frames_accepted` on
upgraded nodes: a value still rising means a peer still signs the old way or not at all. When both stop rising
everywhere, set `kv_require_signed_frames = true`. Alerts on `mycelium_identity_untagged_proofs_total` or
`mycelium_consensus_untagged_signatures_total` should be removed.

**Not covered.** Anti-entropy: a `StateResponse` entry carries no author and no signature and is applied by LWW, so a
member can still introduce a value for any key through a state reply — the residual the threat model's Boundary A
states; the `sys/` tripwire does not count an entry for a key the victim holds nothing under. **With
`require_identity_proofs` off** (the default until row S, §41), the identity of another member is still writable by a member under
its **own** signature: a signed write of `sys/identity/{v}` from M is a valid frame (signer = sender = M), and an
unproven legacy history is merged on first use — the anchor tripwire counts it (`identity_anchor_conflicts`), it
does not prevent it. `require_identity_proofs` is what closes that; a replay of a genuine sealed record re-asserts
only keys its node published (keys accumulate). Role claims, audit
records and revocation events are still signed over untagged canonical bytes (`what-is-proven.md`).

## 36. Companion doors are bounded and checked: a blob quota, a one-segment run id, an award that names its requirement (2.33.0)

**What changes.** Post-360 plan row H (#609). `mycelium-reason` 0.9.0: `FsBlobStore::open` is **bounded at 1 GiB**
(`DEFAULT_BLOB_QUOTA_BYTES`; `open_with_quota(dir, bytes)` for another, `u64::MAX` for none, `0` refused) and
`PUT /gateway/reason/blob` answers **507 `quota_exceeded`** when a put would pass it — a blob already held still
answers 200, and the 413 `too_large` per-blob ceiling is unchanged. A caller-chosen **`run_id` is one bounded segment**
(non-empty, ≤ 128 bytes, ASCII letters, digits and `- _ . : @ +`, not `.` or `..`): `GET /gateway/reason/trace/{run_id}`
and `POST /gateway/reason/route` answer **400 `bad_run_id`**, the façade `400 invalid_request_error` / `bad_run_id`
(`param: "run_id"`); `TraceRecorder::new` returns `Result<_, RunIdError>`; `replay` reads only the key shape a recorder
writes; run ids are validated, runs are not owned. `mycelium-commitment` (on the 2.x train): the declarer **closes**
a requirement (`close(requirement, key, now_ms)`, at or after its deadline, appended to `cn/{requirement}/closes`) and
an award counts only the offers the close names — **no award before a close** (`NotClosed`); a close too large for one
KV write is `CloseTooLarge`; the blob quota charges each file at least one 4 KiB block; offers, reports and assessments are read only for their own
requirement; `award_of` reports only a record naming its requirement and operation; `award_of_verified(requirement,
declarer, resolve)` takes the declarer the caller trusts; `plan_award_verified` and
`commit_award_linearizable_verified` are the checked path — which a member can still steer by deletion: a close names offers by log position and hash, and the award re-reads them from the stream — so a member who deletes or rewrites another participant's named offer removes it from the candidates, and the award does not show it (the close and `close_sha256` are untouched). **Follow-up, not built:** carry each named offer's signed bytes inside the close and award from that copy.

**Will the compiler tell me?** For `TraceRecorder::new` yes (a `Result` where a value was), and for
`award_of_verified` (a `declarer` parameter), and for an exhaustive `Award` literal (`close_sha256`). `CommitmentRefusal`
gained variants (`#[non_exhaustive]`; a `_` arm must fail closed). The rest is runtime: an award without a close is refused `NotClosed`; a 2.32.0 client sending a run id
with `/` gets 400 where it got a trace of another run; a client that stored past 1 GiB gets 507 where it got 200.

**Migration.** Open the store with the quota your node needs (`BLOB_QUOTA_BYTES` on `reason_node`; a store holding
more than its quota at open keeps every file and refuses only *new* puts). Run ids: fold `/` and whitespace at the
client (`reheal_node` folds a model name's `/`). Awards: the declarer calls `close(requirement, key, now_ms)` once
the deadline has passed on its clock, then awards as before; a declarer with keys signs the close and plans with
`plan_award_verified(requirement, rule, now_ms, declarer, resolve)`, and keeps the `Close` that `close` returns — an
overwritten close is recovered by re-asserting it (`close` again on the same handle, `reassert_close(&kept)` after a
restart), which never admits an offer the first close did not.
The Python SDK and the LangGraph checkpointer are unchanged — a 507 or 400 surfaces as `httpx.HTTPStatusError` with
the route's body.

## 37. Nineteen public items (eighteen names) nobody references are deprecated, not removed (2.33.0)

**What changes** (post-360 plan row J, PR #608). A sweep of every `pub fn` / `const` / `struct` / `enum` / `type`
/ `trait` in the library crates against every file in the repository — `src/`, every `mycelium-*` crate, `tests/`,
`examples/`, both SDKs' sources and docs, and `docs/` — found twenty items (nineteen names — `with_max_tokens` is two methods) whose only occurrence in the tree is
their own definition (the grep for each is in the PR). The private companion repository was searched the same way:
it calls one of them, `AeEvidence::audit_outcome` (`compliance`), so that one is **not** deprecated — the note below
would be false for it, and its consumer builds with `-D warnings`. The other nineteen items (eighteen names) are used in neither. Each carries `#[deprecated(since = "2.33.0", note =
"…")]` with the same note: *no caller, test, SDK or document in the repository references it … kept on the 2.x line
— open an issue if you use it and it will be undeprecated.* Nothing is removed; the 2.x line removes nothing.

`mycelium_core::MeshHandle::group_quorum_prehashed` · `mycelium_core::receipt::…::with_replica_sync` ·
`mycelium_core::kv_ns::{MANDATE, CONSUMER_LOG, LOCK}` · `mycelium_effects::Counting::with_counts` ·
`mycelium_sim::bundle::…::path_for` · `mycelium_wasm_host::MeshArtifactSource::cached_len` ·
`Provisioner::disable_resource_policy` · `Provisioner::decision_trace` · `mycelium_wiki::…::with_semantic_lint` ·
`mycelium_wiki::lint::…::with_max_tokens` and `reconcile::…::with_max_tokens` · `RequestPrincipal::is_client` · `AgentStateMachine::set_task_id` ·
`mycelium::federation::{verify_descriptor_rotating, verify_policy}` (`tls`) ·
`FederationClient::with_credential_lifetime` · `mycelium::knowledge::store::…::issuers_about`.

A twentieth candidate, `Preflight::is_inert` (`GatewayPreflight::is_inert`, now in `src/agent/http/authz.rs`), was left as it was until that file's split (row P5) and
row E, which touch the file; row E deprecated it with the same note (§38).

**Will the compiler tell me?** Yes — every use site of a deprecated item warns, and a crate built with
`-D warnings` fails on it; that is the whole mechanism, and it is why the items are deprecated rather than removed.

**Migration.** None is needed for an item you do not call. For one you do: the three `kv_ns` constants are the
namespaces' *vocabulary*, not the namespaces — `lock/`, `clog/` and `mandate/` are live and owned, their rows in
`src/lib.rs` § KV namespace ownership unchanged, and the owners write the literals (a `grep -rn '"lock/'` finds
them); write the literal, as they do. For a builder knob (`with_replica_sync`, `with_counts`, `with_semantic_lint`,
`with_max_tokens`, `with_credential_lifetime`) or an accessor (`cached_len`, `decision_trace`, `is_client`,
`issuers_about`), keep calling it — the warning is the request to say so on an issue, after which
the attribute comes off in the next MINOR. `verify_descriptor_rotating` / `verify_policy` have `verify_descriptor`
and `TrustBundle::key_for` beside them; `set_task_id` has the `"current"` default it documents; `path_for` is
`root.join(name)`; `disable_resource_policy` is `Provisioner::with_resource_policy` with a permissive policy, or
the default.

**Not built:** an automated sweep in CI. The grep above is a one-off (`scripts/` holds no copy); rustc's `dead_code`
cannot see an exported item, so the next sweep is a session's work, not a gate.

## 38. The gateway remainder: timer periods bounded, a dropped stream loses less, prompt names, a fresh envelope (2.33.0)

**What changes** (post-360 plan row E). Six things a gateway client or a provider can see:

- **`interval_secs` and `lease_secs` are bounded** on `POST /gateway/capability/advertise` and
  `POST /gateway/units/declare`: absent means the default (30 s; no lease), present must be a whole number of
  seconds in `1..=3600` (`MAX_TIMER_PERIOD_SECS`), and anything else answers **400 `timer_out_of_range`** naming
  the field. Before, any `u64` was a period, `0` was silently `1`, and a value that was not a whole number (`0.5`,
  `"30"`) was silently 30 s. A unit file's `[[capability]] ttl_secs` is held to the same range by
  `NodeCapabilityConfig::validate()`, so a stem or `units/declare` refuses `ttl_secs = 0` (a 1 ms re-assert loop)
  or `> 3600` by name.
- **A gateway advert says how often it is refreshed.** `capability/advertise` now gossips a `CapEntry` carrying
  `refresh_interval_ms`, as an in-process advert always has, instead of a bare `Capability` every reader timed with
  the legacy 60 s default. An advert refreshed every 600 s no longer reads as stale between beats; one refreshed
  every second evaporates 3 s after its refresher stops, not 3 minutes. Every reader already decodes both forms.
- **The mailbox stream deletes an event when it hands it on.** `GET /gateway/mailbox/{kind}` tombstoned each event
  as it entered its 256-slot channel; it now does so as the stream hands the event to the HTTP connection. A dropped
  stream loses only what the connection (and the kernel's socket buffers) had already accepted — not received: that
  is all HTTP lets the gateway see — and the next stream delivers the rest. Two streams of one kind on one node share
  one in-flight set, so neither hands out an event the other holds; what a dropped stream held un-acknowledged goes
  to the other. The in-process `ServiceHandle::open_mailbox` is unchanged (it hands a plain receiver, and still
  deletes on entering its channel).
- **The log-group subscribe orders by `(hlc, key)` and records its position after the send.** Of two entries with
  one HLC, `GET /gateway/overlay/log/group/subscribe` and `KvHandle::subscribe_log_group` delivered one and skipped
  the other for ever; both now deliver both, in the core's order. The gateway records the group's position as the
  stream hands an entry on, not before queueing it — an entry queued (up to 64) or in flight at a drop is the next
  holder's to send. The position is kept at a new key, `clog/{stream}/{group}/cursor` (`{hlc:016x}/{node}`), beside
  the old `clog/{stream}/{group}/offset`, which is still written in its old format but **one below** the entry's HLC:
  the two writes may reach a peer in either order, and a node that has only the offset — an older node, or one whose
  cursor write has not arrived — then re-delivers that entry and its same-HLC siblings rather than skipping one. `GET /gateway/overlay/log/scan` and `…/log/subscribe` order same-HLC entries by key too.
- **A prompt name holds no `/` and is not empty.** `prompts/{ns}/{name}` made `(a/b, c)` and `(a, b/c)` one key. The
  three `/gateway/prompts/{ns}/{name}` routes and `POST /gateway/llm/call` answer **400 `invalid_prompt_name`**
  (a `%2F` in a path segment arrives decoded, so it is refused too), `/gateway/llm/stream` sends an
  `invalid_prompt_name` error event, and in process `register_prompt_skill`, `update_prompt` and
  `call_prompt_skill` return `PromptSkillError::InvalidName`, `get_prompt` returns `None`, `delete_prompt` deletes
  nothing, and `list_prompts` skips a key no valid pair builds.
- **A caller envelope older than 300 s plus the drift bound is refused.** `gateway_caller::verify` — every provider's
  receive path — reads the envelope's signed issue time: older than `CALLER_CONTEXT_MAX_AGE_MS` (300 s, the longest
  any gateway door waits) plus `max_clock_drift_ms`, or further ahead than `max_clock_drift_ms`, is
  **`CallerError::Stale`** (code `stale`). With `max_clock_drift_ms = 0` — *no drift bound*, as everywhere else —
  the age bound is the 300 s alone, and an envelope may be at most 300 s ahead. Before, the time was signed and never read.
- **A `[[serve]]` whose `ns` or `name` is empty or holds a `/` is refused at load** by `validate()` (a served skill is a
  prompt skill; it loaded, then failed `InvalidName` on every reconcile tick).
- `GatewayPreflight::is_inert` is `#[deprecated(since = "2.33.0")]` (§37's note; nothing in the repository or the
  private companion calls it).

**Will the compiler tell me?** Partly. `PromptSkillError` gained `InvalidName` and is not `#[non_exhaustive]`, so an
exhaustive `match` on it needs the arm; `CallerError` gained `Stale` (it is `#[non_exhaustive]`; your `_` arm already
denies). A call to `is_inert` warns. The rest is refusals by name.

**Migration.** A client sending `interval_secs` or `lease_secs` as a fraction, `0` or past an hour: send whole
seconds in range (`mycelium-py` and `mycelium-ts` now round `interval_secs` up, as they did leases, and refuse a
period of 0 or past 3600 s locally). The bound is these two routes' and a unit file's: `overlay/lock/acquire` and
`overlay/elect`'s `ttl_secs` and `rpc/call`'s `timeout_secs` still clamp. A prompt skill
whose `ns` or `name` holds a `/` never resolved (a `cap/` key with a `/` in its name was already refused as
malformed); rename it. The 300 s window is the gateway's: no gateway door waits longer. An **in-process** caller
(`rpc_call`, `rpc_call_with_mandate`) names its own timeout, and the check runs when the provider reads the request
from `rpc_rx`, not at send — so a node-own request (a secure-profile gateway wraps its own RPCs in a self envelope)
that waits behind a provider backlog longer than 300 s plus the drift bound is refused `stale` though its caller
still waits. Keep such a provider's queue shorter than that, or time the caller out first.
Nothing to do for the log group in a mixed fleet: an older node writes only the offset, and an upgraded reader takes
the offset whenever it is ahead of the cursor. **What a drop still loses**, stated: a mailbox event or log-group
entry the connection had accepted when it dropped — the gateway has no resume point (guide 10).

## 39. Revocation reaches every verifier, and only a newer certificate of a node can revoke its keys (2.33.0)

**What changes** (post-360 plan row Q; decision record `docs/design/revocation-authority.md` rev 3):

- **Every verifier of a node's key honours revocation**: KV `SignedData` (refused, `kv_revoked_key_refused`), identity
  records and the unsigned identity exemption, consensus, role claims and the audit chain, caller envelopes, knowledge
  records and `mycelium-agentfacts`. Consensus no longer falls back to the raw `sys/identity/{signer}` value: a signer
  with no trusted key is refused.
- **Certificates now name their node and their issuance.** `generate_node_cert` (start in the development posture,
  `rotate_identity`, `mycelium tls issue`) adds a URI SAN `mycelium://<node>` and writes the issuance time (ms) as the
  serial; `notBefore`/`notAfter` are unchanged. Each node publishes its certificate at
  `sys/identity-cert/{node}/{key-hex}`.
- **A node with a certificate record trusts exactly its anchored keys it has not revoked.** No other key verifies as
  it — not a key from a bare or poisoned `sys/identity/`, a first-sighted record, a legacy handshake anchor, or a
  retired key that never had a URI-SAN certificate. **What still reads as unverified** once the node is re-anchored:
  anything *live* signed by such a key — a KV write, a role claim (`roles_of`), a caller envelope, a consensus
  message, and a knowledge record's present standing (it stays attributable). **History still verifies**: the audit
  chain is recovered and verified under every key the node held minus the keys it revoked, so a chain sealed under a
  pre-upgrade key keeps verifying and its genesis is never re-sealed. Anything signed by a **revoked** key reads as
  unverified everywhere, history included.
- **Who may revoke:** a revocation of key P counts only when signed by an anchored key of the node issued after P's
  certificate (or P has no standing). A thief holding an old key cannot revoke a newer one; a CA re-issue outranks
  every older key.
- **Revocations are written at `sys/revocation/{node}/{P-hex}/{S-hex}`** as the v1 bytes followed by a trailer (the
  revoker, the certificates, a second signature). A 2.32 node reads the v1 prefix and applies its old rule. Records at
  the old path are still read, under the new rule.
- **Only `{seq:016x}` keys whose record names that seq are read as audit records** (anything else under
  `sys/audit/{node}/` is counted, `audit_malformed_keys`, and ignored); **a seal skips a slot an unverified record
  occupies** and never wraps (an exhausted sequence refuses the seal); a peer's write into `sys/audit/{node}/` or
  `sys/audit-checkpoint/{node}/` trips the `sys/` tripwire; **`audit_segments`** (and `segments` on
  `GET /gateway/audit`) reads a stream segment by segment; `audit_unverified_records` counts records;
  `audit_prune_to_checkpoint` prunes only records that verify. **Seqs may now jump** over a skipped slot: the public
  contiguous verifiers (`verify_chain`, `verify_chain_keys`, `verify_stream_from_genesis`) report that as
  `SequenceGap` — they are unchanged, nothing is removed on 2.x — and an external verifier of an `AuditSink` export
  should use the new `verify_linked`, which follows the hash links. A planted run of any length no longer stops sealing: the seal finds the first free slot outside the audit lock and reports the run. Kept, unverified records stay until an operator removes them (`docs/operations/audit.md`). A
  pruned stream whose boundary record was removed anchors at the newest checkpoint below its first record.
- **An audit stream is never sealed over.** A node whose own persisted audit records do not verify at start (a planted
  record, or — after upgrading a node that rotated on a version before row Q, whose rotation record is signed by the
  prior key — its pre-upgrade records) keeps them untouched, resumes numbering after the last of them, and reports
  them: an error log naming the seq range, `GET /stats` `audit_unverified_records` / `audit_unverified_from_seq`, the
  guarantee report's `audit_unverified`. `audit_verify` keeps saying the stream holds records that do not verify.
  **Nothing is destroyed;** the pre-upgrade prefix reads as unverified until the node's history is re-anchored.
  (Before, the next seal overwrote the first record that did not verify.)
- **A node refuses to start with a key its own records show revoked** — a key file restored from a backup, or
  `key_pem` naming a key a rotation retired: `start()` returns `GossipError::InvalidField { field: "tls", .. }` and
  logs it by name. Start it with its current key, or have the operator issue a new one.
- **A node cannot revoke the key it signs with**, nor a key that is not its own: `revoke_identity_key` returns
  `GossipError::InvalidField { field: "revoked_key", .. }`, and `POST /gateway/identity/revoke` answers **422**.
- **New on `GET /stats`:** `identity_cert_future`, `identity_records_over_cap`, `revocation_journal_refused`. **New
  guarantee:** `id.revocation_authority`. **New file:** `{persistence}/identity/revocations.journal`.

**Will the compiler tell me?** Only for an exhaustive struct literal of `CoreCtx` (it gained `key_standing`) or of
`RotationMaterial` (it gained `node_cert_der`). The rest is behaviour.

**Migration — required.** **Re-issue every node's certificate** so it carries the URI SAN: a development-posture
node (`auto_cert_dir` with the CA key) does it itself at its next start; a node on a pre-issued certificate needs
`mycelium tls issue` and a restart with the new certificate. **The window, until a node is re-anchored:** no revocation
of its keys counts at upgraded nodes, so keys it revoked before the upgrade **verify again there**, and its keys are
judged as before row Q (TOFU, proofs-off poisoning). At start a re-anchored node re-issues each of its own v1
revocations under its current key, so they count again. A script calling `POST /gateway/identity/revoke` with the
current key gets 422: rotate first. **What is not covered:** a holder of the fleet CA key (every node in the
development posture) can issue a certificate that outranks any node's key; the secure posture cannot rotate and
recovers by re-issue only; a retired key verifies until revoked; unsigned `Data` and anti-entropy; the mTLS handshake
did not refuse a revoked key (it does since §44); a node without persistence forgets, across a restart, a revocation hidden fleet-wide
while it was down, until the owner republishes it; a holder of the CA key can make a node refuse to start (loud,
recovered by a re-issue); junk certificates for many nodes can churn the bounded certificate cache (a fold then costs
on the order of a second).

## 40. The refusal is unconditional: a TLS node refuses unsigned KV frames and bare KV signatures, and `kv_require_signed_frames` is retired (2.34.0) — §35's window closes

Post-360 plan row F2, the MINOR after row F. Wire **v12** unchanged — no frame gains or loses a byte; what changes is
that §35's one-release allowances end.

**What changes.**
- **A TLS node refuses an unsigned KV `Data` frame, whatever `kv_require_signed_frames` says.** Dropped, not
  forwarded, counted (`GET /stats` `kv_unsigned_frames_refused`, `mycelium_kv_unsigned_frames_total{outcome="refused"}`).
  The two records that authenticate themselves are still taken, as in 2.33.0: a sealed identity record
  (`sys/identity-signed/{n}`) whose proof verifies and chains to what the receiver holds, and a CA-verified
  certificate record (`sys/identity-cert/{n}/{key}`, row Q) — both `connection::self_authenticating_identity`, counted
  (`kv_identity_exempt_frames`). A stale or revoked sealed record is refused as before.
- **A bare (pre-2.33) KV `SignedData` signature is refused.** A receiver verifies the `mycelium.kv/update/1`-tagged
  form only. A signature that would have verified in the bare form is told apart on the failure path and counted
  (`GET /stats` `kv_untagged_signatures_refused`, `mycelium_kv_untagged_refused_total`), and named at most once a
  minute per connection in a warning that says the peer is on 2.32 — so an operator can find it. That warning has its
  own per-connection gate, so a 2.32 peer's stream of bare signatures does not use up the minute's warning for a
  signer mismatch or a revoked key on the same connection. **An unsigned refusal is logged at `debug` only**, and
  counted: every write a 2.32 peer's agent originates is an unsigned `Data` frame, so a warning per refusal (or even
  per connection-minute, for every such connection) would flood the log for the whole upgrade; the counter is the
  signal, and a bare signature's warning already names the 2.32 peer.
- **A node's locality advertisement is signed.** `start()` wrote `cap/{self}/locality/self` before the TLS identity
  was installed, so the frame left unsigned and every TLS peer would refuse it; it now follows the install, and the
  only writes before it are the two self-authenticating records (#624's review). In a two-node run the start-time
  frame reached the peer by gossip neither before nor after the move — it is dispatched before any connection exists
  — so the entry arrives by anti-entropy either way; no end-to-end test pins the order.
- **`kv_require_signed_frames` is a no-op** (`GOSSIP_KV_REQUIRE_SIGNED_FRAMES` too). It is still accepted, so a 2.33
  configuration that set it loads and starts; `start()` warns once when it is `true`. An explicit `false` cannot be
  told from the default and is **not** an opt-out — nothing restores the window. The field is
  `#[deprecated(since = "2.34.0")]`; removal is on the 3.0.0 ledger, like everything on this page.
- **Row F's window counters stop rising.** `GET /stats` `kv_unsigned_frames_accepted` and `kv_untagged_signatures`
  are kept, at zero, so a 2.x dashboard does not break on a missing key; nothing raises them.
  `mycelium_kv_unsigned_frames_total{outcome="accepted"}` and `mycelium_kv_untagged_signatures_total` are no longer
  emitted. Read the refusals instead: `kv_unsigned_frames_refused` and the new `kv_untagged_signatures_refused`.
- **`mesh.signed_writes` is rev 2** and resolves `enforced` on `[tls]` alone (rev 1 resolved on the setting), and it
  now names the bare signature too. **`secure-single-domain` is rev 5** and requires it. It resolves on exactly what
  `mesh.tls` does, so rev 5 admits every node rev 4 admitted; it is required by name so the profile says the refusal is
  part of it.
- A node without `[tls]` signs nothing and refuses nothing — the plaintext posture is unchanged.

**Mixed fleet, direction by direction** (the 2.30.0 shape, §21):
- **2.33.x ↔ 2.34: works.** A 2.33 node signs every write it originates in the tagged form (row F), so nothing it
  sends is refused here; a 2.34 node sends nothing a 2.33 node would refuse.
- **A 2.32 node's KV writes at a 2.34 node: refused at gossip.** Its agent's unsigned `Data` frames and its bare
  `SignedData` frames are both dropped, counted (`kv_unsigned_frames_refused`, `kv_untagged_signatures_refused`) and
  not forwarded. They still arrive **by anti-entropy** — a `StateResponse` entry is not refused (below) — so the
  effect is latency, not loss, as §35 described for the opposite direction; but a 2.32 node's legacy identity record
  is among them, so a 2.34 node that has not dialled it learns its key only by anti-entropy or from its sealed record.
- **A 2.34 node's KV writes at a 2.32 node: as §35 says for 2.33** — dropped as a bad signature, by anti-entropy only.
- **So a fleet that mixes 2.32 and 2.34 replicates KV state by anti-entropy alone, in both directions.** That is the
  flip §35 announced. **Upgrade from 2.33.x, not from 2.32**: take every node to 2.33 first, check that
  `kv_untagged_signatures` and `kv_unsigned_frames_accepted` have stopped rising on every node, then upgrade.

**Will the compiler tell me?** For code that names `kv_require_signed_frames` — a struct literal or an assignment —
**yes**: a deprecation warning, an error under `-D warnings`. `TransportBounds` gained a field
(`kv_untagged_refused`); it derives `Default`, so only an exhaustive literal breaks. The rest is wire behaviour.

**Migration.** Upgrade from 2.33.x. Remove `kv_require_signed_frames` from configuration files and
`GOSSIP_KV_REQUIRE_SIGNED_FRAMES` from environments (it does nothing, and the start-up warning says so). Move alerts
from `kv_unsigned_frames_accepted` / `kv_untagged_signatures` (and their metrics) to `kv_unsigned_frames_refused` /
`kv_untagged_signatures_refused`: after the upgrade a rising refusal count means a peer still on 2.32, or a member
forging. A deployment under `profile = "secure-single-domain"` needs nothing new — rev 5's addition is met wherever rev
4's `mesh.tls` was.

**Not covered.** **Anti-entropy, restated rather than closed:** a `StateResponse` entry carries no author and no
signature and is applied by LWW, so a member can still introduce a value for any key — another member's included —
through a state reply, and the `sys/` tripwire does not count an entry for a key the victim holds nothing under
(`docs/threat-model.md` Boundary A). Closing it needs entries that carry their originator's signature, verified on
receipt as the `SignedData` arm verifies — a wire change with its own window, not built. **With
`require_identity_proofs` off** (an explicit opt-out since row S, §41), another member's legacy identity is still writable under the writer's own signature
(detected by the anchor tripwire, not prevented). **First sighting of a sealed record stays trust on first use.** **A refused frame still consumes its nonce**
(pre-existing, unchanged): a receiver records a `Data` frame's nonce in the early-dedup path and a `SignedData` frame's
before it checks the signature, so a member relaying an unsigned or badly signed copy of another's frame ahead of the
genuine one makes that receiver drop the genuine frame as a duplicate — one hop suppressed until the next anti-entropy
exchange, not a forged write.
Role claims, audit records and revocation events are still signed over untagged canonical bytes
(`what-is-proven.md`).

## 41. Identity proofs are required by default (2.34.0, post-360 plan row S)

**What changes.** `require_identity_proofs` defaults to **`true`**. A TLS node now accepts a peer's identity only
from its **sealed** record (`sys/identity-signed/{node}`, the history and its proof in one entry), whose proof
verifies and chains to the keys already held for that node (first sighting stays trust on first use). A bare
`sys/identity/{node}` key list, or the legacy `sys/identity/` + `sys/identity-proof/` pair without a sealed record,
is refused and counted (`identity_anchor_conflicts`) rather than merged. A configuration file that does not name
the field gets `true` (the field now carries its own serde default; it used to resolve to `bool`'s `false`), and
`GOSSIP_REQUIRE_IDENTITY_PROOFS` is a strict boolean — `true`/`false`/`1`/`0`/`yes`/`no`; anything else refuses
the configuration by name (`InvalidField { field: "require_identity_proofs" }`), because a typo used to read as
the opt-out. The guarantee `id.proofs_required` resolves `enforced` on a TLS node that says nothing.

**Why.** It closes the identity-poisoning residual rows F and Q stated: with proofs off, a member could garble
another node's sealed record and proof and write a bare key list naming its own key — a valid frame under its own
signature — and every receiver merged that key into the victim's set. The key then reached the verify-only history
set (forged audit history, a steered checkpoint anchor) and, for a node on a legacy certificate (one with no URI SAN,
so no standing), verified live writes as that node. The 2.14.0 attempt to make this the default was reverted after a
Docker leader-election failure that was wrongly blamed on it (the flag is inert without TLS, and that suite runs
none); the ordering window the revert named is closed by the sealed record (one entry, written since 2.14.0).

**A mixed fleet — row S cuts off nobody 2.33.0 did not already cut off, except a node from before 2.3.0.**
Since 2.33.0 (§35, row F) a proof is accepted only in its tagged form, in either posture
(`helpers::identity_proof_message`; 2.32.0 is the first release that writes it):

- a **2.32.0 or later** peer writes a tagged proof in its sealed record and is learned exactly as before;
- a **2.3.0–2.31.x** peer writes an untagged proof, and has not been learned from gossip since 2.33.0 **with or without
  this flag** — only the certificate key a *direct* connection anchors at the handshake; this release changes nothing
  for it, and `false` does not re-admit it;
- a peer from **before 2.3.0** writes no proof at all. 2.33.0's default accepted it on first use; this default refuses
  it, so an upgraded node holds no gossip-learned key for it and drops its relayed writes as from an unknown signer.
  It is the **one** node `require_identity_proofs = false` re-admits;
- a node without `[tls]` publishes and reads no identity: the setting is inert there, nothing warns or refuses, and
  the guarantee report names `[tls]` as what is missing.

An un-upgraded 2.33 peer itself still runs with proofs off unless configured, so the residual stays open *at it*.

**The environment variable is strict.** `GOSSIP_REQUIRE_IDENTITY_PROOFS` accepts exactly `1`, `true`, `TRUE`, `yes`
(on) and `0`, `false`, `FALSE`, `no` (off). Everything else is refused at configuration time by name — including
`True`/`False`, `Yes`/`No`, `on`/`off` and an **empty** value (for example an unset `${VAR}` in a compose file), all
of which the old parse read silently as off. This matches `GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED`, which also refuses
an empty value; unset the variable to take the default.

**Will the compiler tell me?** No — it is behaviour. `GossipConfig::default()` changed one field's value.

**Migration.** Nothing, for a fleet upgrading from 2.33.x: every peer it can learn already proves. To keep a node from
before 2.3.0 for a window, set `require_identity_proofs = false` (or `GOSSIP_REQUIRE_IDENTITY_PROOFS=0`) on the
upgraded nodes, knowing it reopens the residual above; remove it once that node is upgraded. A TLS-less development
fleet needs nothing. **Not closed:** first sighting of a node never seen
is trust on first use with or without proofs (anchors close it); anti-entropy entries carry no author.

## 42. Shutdown closes the soft-state gate before it sweeps (2.34.0)

**What changes.** `GossipAgent::shutdown` tombstones the node's own soft state (`cap/`, `req/`, `svc/`, `tools/`,
`gcap/`, `sys/load/{self}/…`) before it flushes and signals its tasks. Those tasks ran until the signal, so a
re-assert tick between the sweep and the signal wrote a swept key live again under a newer HLC, and the flush
delivered that instead: a peer kept routing to the stopped node until the entry's TTL. #627's review measured 20/20
shutdowns at a 5 ms refresh, and 0/10 at 50 ms and above. Shutdown now closes a gate every soft-state re-assert
passes through (`CoreCtx::soft_state_gate`, a `kv_persist::SoftStateGate`). After that no task writes a re-assert,
and one already under way finishes before the sweep.

**Will the compiler tell me?** Only for an exhaustive struct literal of `CoreCtx`, which gained `soft_state_gate`.
`..` or a constructor is unaffected. Code that builds its own `CoreCtx` initialises the field with
`Arc::default()`.

**Migration.** None for a node. A companion or embedder that writes its own soft state in a periodic loop, re-asserting a key it
expects shutdown to retract, should put that write through `ctx.soft_state_gate.write(|| …)`, as
`run_kv_persist_task` does. Build the payload first and put only the apply and the send inside: `shutdown` waits
for whatever is inside, so it must not block or await. A panic inside is safe, because the in-flight count is
restored on unwind.

## 43. A capability name is one key segment, and `locality`/`self` is reserved (2.34.0)

**What changes** (#613's review, finding 6). A capability's namespace and name are two segments of
`cap/{node}/{ns}/{name}`, and of `req/{node}/{ns}/{name}` and `gcap/{group}/{ns}/{name}/{node}`. Three kinds of name
are now refused, where before they were accepted:
- **An empty `ns` or `name`, or one holding a `/`.** It built a key no reader parses: `parse_cap_key` refuses it, so
  the advert was invisible to every resolver and logged as malformed on every scan.
- **`ns = "locality", name = "self"`.** That key is the node's own locality entry, `cap/{node}/locality/self`. An
  advert overwrote it with a `Capability`.
- **A group name holding a `/`.** It is one segment of `cap-group/{group}` and of `gcap/…`.

Where each is refused:
- `POST /gateway/capability/advertise` answers **400 `invalid_capability_name`**.
- The unit-file loader (`NodeCapabilityConfig::validate`) refuses the file. So `POST /gateway/units/declare` answers
  400, and a node configuration fails to load. This covers `[[capability]]`, `[[requirement]]`, a group's
  `[[group.provides]]` and the group's name.
- In process, `advertise_capability`, `declare_requirement` and `define_capability_group` log a warning and return a
  handle that writes nothing. Their signatures are unchanged.
- A `[[serve]]` named `locality`/`self` fails to load. `register_prompt_skill` returns `PromptSkillError::InvalidName`
  for that pair, as it already did for a `/`.
- The emergent-group projector skips, with a warning, any `cap-group/` definition whose group name or `provides`
  name fails the check. Such a definition can still come from a 2.33 peer, or from state persisted before the
  upgrade, until the fleet is upgraded.

**Not checked at load:** a catalogue manifest's `provides`. One holding a `/` still installs, and its advert is
refused in process with a warning.

The check is `mycelium::capability_name_problem` (and `capability_group_name_problem`), which is public, so a
caller can ask before it advertises.

**Will the compiler tell me?** No. It is a change in behaviour.

**Migration.** A name holding a `/` never reached a reader, so nothing that worked stops working. Rename it to one
segment: for a model id such as `org/model`, use a separator other than `/`. Pick another name than
`locality`/`self` for the capability.

## 44. A revoked key's certificate is refused at the handshake, and the mesh does not resume TLS sessions (2.35.0, post-360 plan row Q's follow-up)

**What changes.** Both sides of a mesh TLS handshake now refuse a certificate whose Ed25519 key its owner revoked by a
revocation this node counted — judged while the key was anchored for the owner (a certificate record naming the node)
and kept for good after (across a restart, through the journal under `[persistence]`, 64 per node) (`NodeStanding::counted`, in `CoreCtx::key_standing`). The owner is the node the certificate's
`mycelium://{node}` URI names, or, for a certificate issued before row Q with no URI, the node that counted a
revocation of its key. Before,
the handshake judged the fleet CA chain and the removal ledger only, so the holder of a revoked key and its
certificate stayed a connected peer. And the mesh's rustls configs no longer resume sessions: a resumed TLS 1.3
session re-runs no certificate verifier, so a removed member (closure plan C5) or a revoked key could reconnect on a
session cached before the change.

**Will the compiler tell me?** Only if you call `mycelium_core::tls::load_or_generate`, `generate_rotation` or
`generate_rotation_after` directly: each takes `standing: Arc<KeyStanding>` after `removed`. A `GossipAgent` passes
its own. `NodeStanding` gained a field, `counted`, so an exhaustive struct literal of it breaks; nothing else changes
shape.

**Migration.** A direct caller passes the agent's `key_standing` (or `Default::default()` where no fold runs, which
judges no revocation). Every reconnect between mesh nodes is now a full handshake: one more Ed25519 signature and
verification each way, per connection, not per frame.

**Not changed:** a connection already open when a revocation is counted stays open, and its signed frames are refused
as before; a key that never had a certificate record naming its node is not refused at the handshake (its signatures
are); a provisional revocation (of a key whose certificate record has not reached this node) refuses no handshake.

## 45. A caller envelope is served once, at the provider it was signed for (2.35.0, post-360 plan row E)

**What changes.** A gateway's caller envelope gains three fields: `n`, an id unique per gateway (its HLC at framing);
`o`, the node it is addressed to; and `sn`, an Ed25519 signature over the v1 fields, `n` and `o` under
`mycelium:gateway-caller:v2`. The v1 signature `sig` is still produced, so a provider on 2.34 or earlier verifies a new
gateway's envelope as before. A provider on this release:
- refuses an envelope addressed to another node (`misaddressed`);
- admits each `(gateway, issue time, id)` once inside the freshness window (300 s plus `max_clock_drift_ms`), refusing a
  second receipt (`replayed`);
- refuses an id-less signed envelope from a gateway whose `sys/caller-context-id/` marker it holds — signed by a key it
  trusts for that gateway and issued within the freshness window plus two refresh periods (12 minutes at the default
  drift bound); every `tls` node on this release re-asserts its own every minute — as a replay with the id stripped
  (`id_stripped`);
- refuses an `n` without its `o` or `sn` as `malformed`, and a changed `n` or `o` fails the signature.

The three replay refusals are **dropped without an answer** and counted (`GET /stats` `caller_envelopes_replayed`). The
MCP bridge now verifies the caller context at its receive boundary and refuses a failed one with `-32022`, as the native
tool loop does; before, it verified only under provider enforcement.

**The window.** An id-less envelope from a gateway without a fresh marker — 2.34 or earlier — verifies in the v1 form,
is served and, if signed, is counted (`caller_envelopes_without_id`; an unsigned mesh counts nothing). Once every gateway node is upgraded and its marker has
gossiped, stripping is refused fleet-wide; the next release refuses every envelope without an id.

**Rolling a gateway back to 2.34** stops its marker being re-asserted; until the last one lapses (up to 12 minutes at
the default drift bound) upgraded providers drop its id-less envelopes unanswered, so its callers time out for that
long. Nothing needs deleting: the marker is never tombstoned, it lapses.

**Will the compiler tell me?** No. `GatewayCaller` gained `envelope_id` and `CallerError` gained `Replayed`,
`Misaddressed` and `IdStripped` (with `CallerError::unanswered`); both are `#[non_exhaustive]`, so nothing outside the
crate breaks. New: `GossipAgent::receive_gateway_caller`.

**Migration.**
- Upgrade gateway nodes, then confirm `caller_envelopes_without_id` stops rising on every provider.
- A loop that reads raw signals itself (`mesh().signal_rx(kind)` + `RpcRequest::from`) and called `gateway_caller`
  should call `receive_gateway_caller` **once per signal** instead, and drop the request unanswered when the error's
  `unanswered()` is true. `gateway_caller` stays stateless and checks no window.
- **Two subscribers verifying one kind on one node** — two `rpc_rx(kind)`, two serve streams, two MCP tools of one name —
  each receive every signal; now the first to read a request admits it and the others drop it as a replay. Before, all
  of them served it. Give each its own kind.
- A caller that sends one framed envelope twice on purpose — none in this repository does — must frame each call afresh.
- An envelope is about 150 bytes larger, so a mandate within that distance of the 8 KiB bound now answers
  `caller_context_too_large`.

**Not changed or not covered:** an unsigned mesh keeps no window (its envelopes can be forged outright); a provider forgets
its window on restart; a member that tombstones a gateway's id marker at a provider reopens the id-less path there until
the gateway's next re-assertion, at most a minute (the `sys/` tripwire counts it); after an eviction floor (past about 109 calls a second, sustained, from one gateway to one
provider), a gateway whose clock moves back across a restart has its calls dropped until its clock passes the floor; a
provider whose own clock steps back may serve an envelope it had already pruned, once; with `max_clock_drift_ms = 0`, a
gateway 60–300 s ahead of a provider has its marker ignored there, so a stripped envelope is served (fail-open) where
`verify` still accepts its envelopes.

## 46. A current A2A client reaches `/a2a`: the card at `agent-card.json`, the 1.0 and 0.3 methods, a skill in `metadata.skillId` (2.35.0, post-360 plan row N)

**What changes.** The agent card is served at `/.well-known/agent-card.json`, where A2A 0.3 and 1.0 clients read it, and
lists `/a2a` once per protocol version (`supportedInterfaces`: JSON-RPC `1.0`, then `0.3`). `/a2a` answers A2A 1.0's
`SendMessage`, `SendStreamingMessage`, `GetTask` and `CancelTask` and 0.3's `message/send` and `message/stream` in each
generation's shapes. A 0.3 / 1.0 send names its skill in `metadata.skillId` (the message's or the request's; the two
must agree), or reaches the skill the embedder declared with `GossipAgent::with_a2a_default_skill`, or is refused
`-32602`; its first part is read as text, or its `data` re-serialised as compact JSON — keys sorted, an integer beyond 64
bits or a decimal passed through an `f64`, and a mandate's possession proof digests that form (anything else is `-32005`); a mandate and an operation
id go in the request's `metadata._meta`, where `tasks/send` takes them in `params._meta`. The task id is minted by the
node, and a message naming a `taskId` is refused `-32004`: a task here is complete when its send returns. An unknown
skill is `-32602` on the new methods (0.3 and 1.0 give `-32001` to *task* not found) and stays `-32001` on `tasks/send`.

**The window.** `/.well-known/agent.json`, `tasks/send`, `tasks/sendSubscribe` and the `skillId` parameter answer as
before for this release. `tasks/get` and `tasks/cancel` are also 0.3's names and stay; a task they read answers in the
shapes of the method that created it, so a pre-0.2 client sees pre-0.2 shapes for its own tasks. A `tasks/send` naming the id of a task a 0.3 or 1.0 method created is refused `-32004`
rather than re-sent over it.

**Will the compiler tell me?** No. New: `GossipAgent::with_a2a_default_skill`. The `-32004` message for another
identity's task now names `GetTask` and `CancelTask` as well — a client matching that text, rather than the code,
needs the new wording.

**Migration.**
- An A2A client: read the card at `agent-card.json` and use `SendMessage` (or the SDK's client built from the card);
  put the skill in `metadata.skillId` unless the node declares a default, and a mandate in `metadata._meta`.
- An embedder whose callers are standard A2A clients (Strands Agents, `a2a-sdk`): declare the default skill, since they
  send no skill.
- `mycelium-py`'s and `mycelium-ts`' `A2aClient` still use the old path and methods; they move before those are
  removed.

**Not changed or not covered:** push notifications, `ListTasks`, `SubscribeToTask`, the extended card, and the HTTP+JSON
and gRPC bindings are not served; a task's `history` is not kept; `A2A-Version` is not checked (the card says which
versions `/a2a` speaks).

## 47. An evaluator can decide on what an actor has already done (2.35.0, post-360 plan row K)

**What changes.** `ActionEvaluator` gains two methods with defaults: `history_request` (the window of the actor's history
the evaluator needs; `None` by default) and `evaluate_with_history` (what the seam now calls; by default it calls
`evaluate`), so an evaluator that decides on the request alone is unchanged. A gateway with an evidence journal keeps
each actor's recent decisions from it — its own, node-local (`HistoryScope::EnforcementPoint`) — and gives them to an
evaluator that asks; a decision on them carries the history it read on its evidence record (`history`). The reference
evaluator's `Rule::at_most(count, window_ms)` expresses *at most three posts in ten minutes*; the fourth is a `Deny`. A
history that cannot be read — no journal, an unreadable one — is `History::Unavailable`, and a limited rule decides
`Indeterminate` on it, never as though the actor had done nothing. `preflight` is unchanged; `preflight_with_history`
is new.

**Whose history.** This enforcement point's, by decision (plan §5): an actor that spreads its calls over N gateways can
be admitted up to N times a per-gateway limit. The scope is named on every history decision and evidence record.

**Will the compiler tell me?** For two struct literals: `Rule` and `ContractCase` each gained a field (`limit`,
`history`) — build a `Rule` with `Rule::new` and its builders, and give a `ContractCase` `history: None`.
`AeEvidence` is `#[non_exhaustive]` and gained `history`, absent when no history was read, so a record without one
serialises exactly as before. **Not for a wrapper**: an evaluator that wraps another and forwards only `evaluate`
compiles, and hides the inner evaluator's history request, so every limited rule behind it decides `Indeterminate` —
closed, but not what was meant. Forward `history_request` and `evaluate_with_history`.

**Migration.** None for an evaluator without history rules. To limit by history: an evaluator implements both methods
(see `ReferenceEvaluator` and the contract suite's `TableEvaluator`), and the gateway needs an evidence journal
(`with_evidence_journal`) — without one, every history it is asked for is unavailable.

**An adapter held to the contract suite** (`ae_contract::run`) meets five new cases (`history_*`). One that cannot
express `PolicyClause::AllowAtMost` must decline them and list them in `declared_limits`, or `Report::conformant` fails.
The private companion's Cedar adapter passes all sixteen cases on its v2.35.0 re-pin (private #32, `5e807a2`), none
declined. `AeEvidence`'s schema stays `mycelium.ae/evidence/1`: the
new `history` field is additive and absent when no history was read.

## 48. Credentials attached at egress: three structs gained fields and `CredentialBinding` is new, a `ServeDecl`'s `Debug` hides its key, Z2's refusal names the section (2.35.0, post-360 plan row L)

**What changes.** Row L adds credential bindings (`docs/operations/crown-jewel.md` § Credentials attached at egress).
Additive for a running node — no binding configured, nothing changes — but three public structs gained fields:
`GossipConfig.egress_credentials`, `OpenAiBackend.credentials` and `ServeDecl.credential`; `CredentialBinding` is new
in this release, so no existing literal of it can break. Two outputs change: `ServeDecl`'s `Debug` (hand-written now)
prints `api_key: Some("<redacted>")` and the new `credential` field, where the derived one printed the literal key; and
the stem's refusal for an unset `api_key_env` variable (zero-gaps Z2) names the `[[serve]]` section and the field, no
longer the variable — a value pasted into `api_key_env` would have been echoed into a start-time log line.

**Will the compiler tell me?** **Yes** for an exhaustive struct literal of any of the three — it will not compile.
No for the two outputs.

**Migration.**
- A struct literal: add `..Default::default()` (`GossipConfig`), or the new field (`credentials: None`,
  `credential: None`); `OpenAiBackend::new` is unaffected.
- A test or a log parser matching a `ServeDecl`'s `Debug` or the `api_key_env` refusal's text: match the section name
  (`[[serve]] llm/<name>`) and `api_key_env`; the variable is whatever the unit file says.
- A binding to `localhost` (any spelling, a trailing dot included) or to any IP literal must set `port` — new, so nothing written before it breaks.

## 49. A unit file with an unknown key is refused, not read without it (2.35.0, post-360 plan row M)

**What changes** (#636's review, finding 3 and its follow-up). The unit-file loader dropped any key it did not know.
A misspelt confinement table — `[activation.confined]` inside an `[[activation]]`, or `[activations.confine]` at the
top level — therefore loaded as **no profile**, and the activation ran unconfined without a word; a misspelt
`[[capabilty]]` declared nothing. `NodeCapabilityConfig` (the top level), `[[activation]]` and `[activation.confine]`
now refuse an unknown key by name. The other sections keep their tolerance.

Where it is refused: `NodeCapabilityConfig::from_toml_str` / `load_from_file`, so `POST /gateway/units/declare`
(400), `mycelium wire-check` and `mycelium-stem --units`.

**Will the compiler tell me?** No. It is a change in behaviour.

**Migration.** Every unit file in this repository loads unchanged. A file of yours that the loader now refuses held a
key that was never read: correct its spelling or remove it.

## 50. `prov.probe` is rule revision 3: a confined re-probe re-resolves, and three new refusal reasons (2.36.0, post-360 plan row M's follow-up, #641)

**What changes.** A confined `[[activation]]`'s re-probe used to fail when a declared path was replaced, so the
install was withdrawn and reinstalled. It now re-resolves the profile, repeating every check, and runs the probe under
the fresh one. The activation's initial probe follows the same rule. `prov.probe` in the rule catalogue goes from
revision 2 to 3 and gains three refusal reasons:
- `confinement_failed`: re-resolution refused, for example because the declared path is now a symlink or is missing;
- `confinement_unavailable`: the platform can no longer apply the profile;
- `confinement_changed`: a declared path changed around each of two probe runs, so the verdict is not trusted.

Before this change, every re-probe refusal was recorded as `probe_failed`.

**Will the compiler tell me?** No. It is a change in recorded reasons and in behaviour.

**Migration.**
- A trace consumer or alert keyed on `prov.probe`'s reasons should add the three new ones.
- `probe_failed` now means the probe itself failed, not its confinement.
- A probe that replaces one of its own declared paths on every run now fails as `confinement_changed`. Move that step
  into the activation `command`, which may replace them.

## 51. An activation egress proxy: `ConfineDecl` gained two fields and `Profile` one, `Refusal` a variant (2.36.0, post-360 plan rows L and M)

**What changes.** An `[activation.confine]` table may name `[activation.confine.egress]` — hosts an activation reaches
by name through an egress proxy its stem runs for it, and the credential bindings that proxy attaches
(`docs/reference/unit-file.md` § `[activation.confine.egress]`). Additive for a unit file: one without the table runs
as before. Three public shapes grew: `mycelium::ConfineDecl` gained `egress: Option<EgressDecl>` and `env: Vec<String>`;
`mycelium_wasm_host::activation_sandbox::Profile` gained `env` (it has a crate-private field, so only this crate
builds one); and `activation_sandbox::Refusal` gained `EgressUnavailable`, recorded as `prov.activation`'s
`egress_proxy_unavailable` (the rule's revision 3). The refusal for a hostname in `connect` now also points at
`egress`.

**Behaviour changes** (#644's review). **Every confined command and probe starts with a cleared environment** —
`PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`, and the names in the new
`[activation.confine] env` (also a `ConfineDecl` field) — where a 2.35.0 confined command inherited the stem's
whole environment; the guarantee `stem.activation_confined` is revision 2 and says so. A stem refuses to start
when a binding's value could reach a confined command: an `env` entry naming a binding's `value_env`, an `env`
entry whose value is a binding's value, or a binding's `value_file` the command could read. Two `[[activation]]`s
for one `ns/name` are refused at load. `stem.activation_egress` reads `not_configured` for a `localhost:<port>`
`connect` entry.

**Will the compiler tell me?** **Yes** for an exhaustive `ConfineDecl` literal and an exhaustive `match` on
`Refusal`. No for the refusal's text, or for the behaviour changes.

**Migration.**
- A `ConfineDecl` literal: add `..Default::default()` or `egress: None, env: Vec::new()`.
- A `match` on `Refusal`: add an arm for `EgressUnavailable` (its `reason()` is `egress_proxy_unavailable`) or a `_`.
- A test matching `connect`'s hostname refusal: match `hostname`; the message now names `[activation.confine.egress]`
  as the way to name a host.
- A confined command that read a variable from the stem's environment: name it in `[activation.confine] env` (not a
  binding's `value_env`; to use a credential, grant it in `[activation.confine.egress] credentials`).
- A unit with two `[[activation]]`s for one capability: merge them into one.
- A binding's `value_file` refused as readable: move it outside the placement root and every confined `read`/`write`
  path.

## 52. The evaluator sees where a routed call would go and what it carries (2.36.0, post-360 plan row R)

**What changes.** A routing door can tell the gateway's preflight the destination it is deciding on and the request's
data class (`GossipAgent::gateway_preflight_routed`, `RouteFacts`). `ActionEnvelope` gained `destination` and
`data_class` and `AeEvidence` gained `route` — both `#[non_exhaustive]`, both `None` from every existing door, so a
record without a route serialises exactly as before and every existing policy decides as it did. The reference
evaluator's `Rule` gained `data_class` and `locality` (builders `for_class`, `at_locality`). `mycelium-reason` 0.10.0
uses all of it: each candidate's preflight sees its declared locality and the request's class, and **a refused candidate
is passed over for the next** — before, the façade answered `403` at the first refused provider.

**Will the compiler tell me?** **Yes** for an exhaustive `Rule { .. }` literal (two new fields) and, in
`mycelium-reason`, for an exhaustive `match` on `RouteError` (`Unclassifiable`) or a `Routed { .. }` literal (`data_class`,
`refused`). No for the façade's behaviour.

**Migration.**
- `Rule`: build it with `Rule::new` and its builders.
- An evaluator adapter run against `ae_contract::run` meets five new cases (`route_*`) and a new clause,
  `PolicyClause::ForbidClassAt`. One that cannot express it must decline the five and list them in `declared_limits`,
  or `Report::conformant` fails. The private companion's Cedar adapter expresses it and passes the five on its v2.36.0
  re-pin (private #33, `0d654fd`), none declined.
- A client of the façade: a `403` now means every candidate was refused; `data.refused_destinations` names each with
  its locality and reason. A `200` may name providers passed over (`refused`). A request carrying
  `x-mycelium-data-class` to a node whose router classifies nothing is now `400` (it was ignored), with or without an
  evaluator.
- A gateway client writing any `llm-meta` ad (`capability/advertise`, `units/declare`) is refused
  `403 reserved_namespace` — with or without `locality`: a client ad on that key replaced the serving node's own.
  Serve a model with `mycelium_reason::serve_model` on the node.
- `GET /gateway/rpc/serve/{kind}` refuses `llm.invoke`, `mcp.invoke` and `skill.invoke` (and the reply kinds
  `rpc.result`, `bulk.result`) with `403 protected_kind`. Those kinds fan out to every receiver on a node, so a client
  serve stream for one received every such request the node was sent and could answer it first. No SDK or example
  served them this way; an **operator-listed** protected kind (`protected_rpc_kinds`) is still servable, checked by
  provider enforcement.
- `GET /gateway/rpc/serve/{kind}` also refuses any kind this node already serves in process (`403
  served_in_process`, naming it — a hosted component's `cap.invoke` kind, an embedder's `rpc_rx`), and a client stream
  holding a kind is **closed** — a terminal `evicted` event, counted as `signal_client_evictions` — when an in-process
  receiver registers for it; a transient listener (`signal_once`, a request's reply) does neither. Serve a kind either
  in process or over client streams, not both. The SDKs raise `ProtectedKindError` for either refusal and
  `ServeEvictedError` on eviction (`mycelium-py` 0.2.12, `mycelium-ts` 0.2.6). `mycelium-core`'s `SignalHandlers`
  gains `register_client`, `register_transient` and `client_evictions`; `SystemStats` gains `signal_client_evictions`
  (an exhaustive struct literal breaks).
- A classifying router (`reason_router_classified`) refuses every request `403 authority_not_established` while no
  action evaluator is attached: attach it (and the journal) before `start()`. `GatewayPreflight::is_inert` is
  undeprecated — deprecated in 2.33.0 as unreferenced (§38), it now has its caller.
- A catalogue entry whose `provides` is an `llm-meta/…` ad is never installed (`prov.eligible`, reason
  `reserved_namespace`, catalogue revision 2).
- **The default class is a floor.** On a classifying router (`reason_router_classified`), a caller with no floor bound
  is held at the default class: a declaration may raise it, not lower it. A deployment that relied on clients
  declaring `public` binds those principals lower (`floor_for(principal, "public")`) or lowers the default
  (`unclassified_as`).
- A policy written with `for_class` or `at_locality` refuses `llm.invoke` through `/gateway/llm/call` and
  `/gateway/llm/stream` (they establish neither) — by design; route classified calls through the façade.

**Not claimed.** The class is declared, never detected; the locality is the provider's declaration, never observed; a
provider re-checking with its own evaluator sees neither (a class rule there decides not established).

## 53. A history-limited rule may count across the fleet: two new scopes, and five structs gained a field (2.36.0, post-360 plan row K2)

**What changes.** `HistoryScope` gains `FleetConsensus` (exact: a counter of seats agreed by consensus, each permit
claiming one before it is admitted; `Indeterminate` without a quorum) and `FleetTally` (approximate: the sum of every
enforcement point's gossiped tally; available without a quorum, detection not prevention, not established when its
coverage is not). A rule chooses with `Rule::at_most(n, w).scoped(scope, counter_id)` — the counter id is opaque, the
policy's own, and what other members see; without `scoped` it counts at the enforcement point, as before. The
reference evaluator does not establish an exact fleet limit over `*`. `ActionEvaluator` gains `fleet_counters`
(default: none), which names the counter a decision on an envelope would rely on; the enforcement point gives each
counter's state to the evaluator in `HistoryView::fleet` and, on a permit, claims a seat in every consensus counter
before admitting it — after releasing the per-actor history gate. `ae.preflight` is catalogue revision 4.

**Behaviour, for every node built with the `gateway` and `tls` features:** it writes a heartbeat under
`sys/ae-tally/{node}/hb` from `start()` (every third of `control_max_staleness_ms`, at most every ten seconds), whether
or not any policy uses the tally, and at a graceful shutdown leaves a last heartbeat marked departed. It tombstones a
non-peer's tally keys once that node's heartbeat is past the horizon, each at the timestamp of the record it holds, and
before its own connections come up re-asserts its own heartbeat and withdraws its own aged entries locally. Without
`[tls]`, a tally rule decides `Indeterminate` while a non-peer enforcing node is inside the horizon (its membership
cannot be verified); with `require_identity_proofs = false`, a fabricated id's entries can count. Tally entries are not
signed (not built): forgery under a dead member's namespace is not detected. Under provider enforcement, a protected request the evaluator puts under a
fleet-consensus counter is checked beside the serve loop, so `rpc_rx` and the MCP tool loop yield it when its claim
completes rather than strictly in arrival order; every other request is checked inline, as before, and handlers still
run one at a time.

**Will the compiler tell me?** For five struct literals: `Rule` gained `counter`, `HistoryLimit` gained `scope`,
`HistoryView` gained `fleet`, `HistoryInput` gained `fleet` and `seats`, and `mycelium-core`'s `KvStore` gained
`scope_roster` (build a store with `KvState::new` or `KvState::with_scoped_prefixes`, never a literal) — build a rule with `Rule::new` and its
builders, and give a view or an input literal `fleet: Vec::new()` (and `seats: Vec::new()`). `HistoryScope` was
already `#[non_exhaustive]`, so a match outside this crate already has a `_` arm — make it fail closed. `AeEvidence`'s
schema stays `mycelium.ae/evidence/1`: `history.fleet` and `history.seats` are absent when empty, so a record without
a fleet count serialises exactly as before; on a fleet decision `history.scope` names the fleet scope. **Not for a
wrapper**: an evaluator that wraps another and does not forward `fleet_counters` compiles, and hides the inner
evaluator's counters, so every fleet-limited rule behind it decides `Indeterminate`.

**Migration.** None for a policy without fleet-scoped limits. To count across the fleet, both scopes need an evidence
journal (`with_evidence_journal`), as the per-point count does — the fleet count travels in the history view. The exact
scope also needs a consensus listener on the voting nodes and, under `consensus_require_electorate`, a declared
`exclusive_default` electorate. The tally scope needs every member built with `gateway` **and** `tls`, and is not
established on a node that has heard from fewer than `control_min_peers_heard` peers.

**A seat is released a window after its permit is admitted; the lease is the crash fallback.** The holder releases
the seat one window after admission, timed on its monotonic clock, so a saturating actor gets close to `n` per window
at any window length. The lease is the window plus `SEAT_LEASE_MARGIN_MS`, 19 225 ms. That is the longest a claim is
allowed to take: four ballots each waiting out both 2 s phases, the convergence poll, and a 2 s allowance for local
work that is not a timeout. Only when the holder crashes or shuts down first does a used seat stay held for the window
plus that margin. Before this release was cut the margin was 2 s, which refused every claim that needed a second
ballot, and nothing released a seat early. The bound holds as stated: at most `n` per window, never more.

**Head-of-line limit.** A serve loop holds at most 64 checks that are settling a seat claim. With 64 waiting — under a
lost quorum, each for up to 2 s per phase × 4 ballots per seat — the loop stops taking requests, so every request
behind them waits too, uncovered calls included: the channel's own bound then pushes back on senders. A fleet that
puts a high-rate kind under an exact limit should expect that while a quorum is lost.

**Upgrade order.** Tally rules fail closed — `Indeterminate` — until every member runs this release built with both
features: an older member, or one built without them, publishes no heartbeat and is unknown coverage. Deploy the
release fleet-wide before a policy names `HistoryScope::FleetTally`.

**An adapter held to the contract suite** (`ae_contract::run`) meets six new cases (`fleet_*`) and a new clause,
`PolicyClause::AllowAtMostAcross` (with a `counter` id). One that cannot express it must decline the cases and list
them in `declared_limits`, or `Report::conformant` fails. The private companion's Cedar adapter expresses it and passes
the six on its v2.36.0 re-pin (private #33, `0d654fd`), none declined.
The suite binds a fixture's fleet state to whatever counter the evaluator names, so an adapter's own counter keys are
held to the same cases.

## 54. `mycelium-core`'s outbound path is counted: its channel types changed shape (2.36.0, the shutdown-sweep fix)

**What changes.** A graceful shutdown now waits, bounded at 250 ms, until every frame the node handed to the
outbound path is flushed to a socket or dropped. To count those frames, the senders are wrapped:
`CoreCtx::gossip_txs` is a `mycelium_core::outbound::GossipTxs` (it was `Arc<[mpsc::Sender<…>]>`), and
`writer::WriterEntry::tx` is a `WriterTx` (it was `mpsc::Sender<Bytes>`). Neither exposes its raw sender. A send goes
through `try_send_counted` or `send_counted`, so no send can skip the count. `writer::get_or_spawn_writer` and
`writer::request_state` take the node's `&Unsent` as a last argument and return a `WriterTx`. `run_peer_writer`
takes a `CountedRx<Bytes>`. `framing::dispatch_gossip_send`, `framing::dispatch_gossip_try_send` and
`framing::gossip_shard_fill` take `&GossipTxs`.

**Will the compiler tell me?** Yes, for every use: each one is a type error. Code that only uses `GossipAgent` and
its handles is unaffected; this concerns only an embedder building a `CoreCtx` or driving the writers directly.

**Migration.** Build the queues with `GossipTxs::new(senders, Unsent::default())`, and give `get_or_spawn_writer`
and `request_state` that node's `gossip_txs.unsent()`. A consumer of a gossip shard queue wraps its receiver in
`CountedRx::new(rx, unsent)` and keeps the `Held` that `recv` returns until it has handed the frame on. A frame read
from the raw receiver without that wrapper is never released, and every later shutdown waits its whole bound.

## 55. A fleet limit is bounded: at most 1 024 seats, a window of at most 24 hours (2.37.0, post-360 plan row K2's bounds)

**What changes.** A fleet-scoped limit (`HistoryScope::FleetConsensus` or `FleetTally`) whose count exceeds
`MAX_FLEET_SEATS` (1 024), or whose window is zero or exceeds `MAX_FLEET_WINDOW_MS` (24 hours, the history horizon), is
**not established**, wherever it comes from. The reference evaluator decides `Indeterminate` naming the bound, as it
already did for an exact limit over a wildcard actor. The enforcement point reads none of such a counter's seats or
tallies and claims no seat: `establish` gives `FleetState::NotEstablished`, whichever evaluator named the counter. And
`preflight_with_history` turns a permit relying on one into `PreflightRefusal::NotEstablished` — a sixth check of the
seam's. Under 2.36.0 a limit of `u32::MAX` read every seat, a slot digest and a KV read each, on every request the rule
covered, synchronously under the actor's history gate on an async worker: a worker pinned for minutes per request, a
denial of service by misconfiguration (the review of the private Cedar adapter). Nothing is clamped: a limit past the
bounds was never a limit the enforcement point could keep.

New public items: `MAX_FLEET_SEATS`, `MAX_FLEET_WINDOW_MS`, `FleetBoundsError` (`#[non_exhaustive]`),
`check_fleet_bounds`, `FleetCounter::try_new` and `FleetCounter::check_bounds`. `FleetCounter::new`, `Rule::at_most` and
`Rule::scoped` are unchanged and still accept any value; the refusal is where the counter is used.

**Will the compiler tell me?** No. A policy whose fleet limit is outside the bounds compiles and builds, and every
request its rule covers then decides `Indeterminate` (`authority_not_established` at the gateway) rather than being
counted. A per-point limit (`at_most` without `scoped`) is unaffected. **Yes** for an exhaustive `ContractCase` or `ae_contract::Report` literal: each gained a field.

**Migration.** Check every fleet-scoped limit: at most 1 024 in a window of at most 24 hours. A policy wanting more
wants a per-point limit, or a shorter window. An adapter can refuse such a policy when it loads it with
`check_fleet_bounds(count, window_ms)` or `FleetCounter::try_new`, rather than refusing every request it covers. An
adapter held to the contract suite meets three new cases — `fleet_limit_past_the_seat_cap`,
`fleet_window_past_the_horizon` (both not established) and `fleet_limit_at_the_bounds` (admitted). An adapter that
builds the first two is settled by the seam's own check, whatever it decides; one that **refuses the policy at load**
(`build` returns `Err`) passes them too, because those two cases accept a refusal at load
(`ContractCase::accepts_refusal_at_load`, recorded in `Report::refused_at_load`) — do not list them in
`declared_limits` — provided it passed `fleet_limit_at_the_bounds` by building (`ae_contract::REFUSAL_AT_LOAD_WITNESS`),
which shows it expresses the clause and refuses only on the bounds. An adapter that cannot express
`AllowAtMostAcross` at all declines all nine `fleet_*` cases, these two included, and declares them. Everywhere else a refusal at load is still a decline. `ContractCase` and `Report` each gained that
field, so an exhaustive literal of either — an adopter's own case — needs `accepts_refusal_at_load: false` (or
`refused_at_load: Vec::new()`). A stored tally entry's window is read clamped to the horizon, so an entry an earlier
release wrote with a longer window ages out at 24 hours. `ae.preflight` is catalogue revision 5.

## 56. A member's tally entries without its heartbeat are unknown coverage, not zero (2.37.0, post-360 plan row K2)

**What changes.** Under 2.36.0 a reader skipped a non-peer that it held tally entries for but no readable heartbeat:
the node was neither counted nor called unknown, so its permits counted as zero. That happens when a node's heartbeat
was collected before its entries arrived, or has not arrived yet — a reader restarted without persistence and catching
up by anti-entropy can read a crashed member's permits as nothing, and admit past the limit. Now such a node's entry
for the counter is dated by its own times: while any time is inside the counter's window plus the staleness and
clock-drift bounds, or the entry cannot be read, a **verified member**'s coverage is unknown and a `FleetTally` rule
decides `Indeterminate` (`authority_not_established` at the gateway), naming the node. A node nobody issued an identity
for is still ignored; without `[tls]`, where no member can be verified, any such node is unknown — as a non-peer
inside the horizon already was. The residue collector removes such entries on the same bound — the same when the
writer's window is the counter's, as `publish_permit` writes it — so the window is bounded: at most the counter's
window plus both bounds after the entry's last permit.

**Forged times** (#656's second review). A time past this node's clock plus `max_clock_drift_ms` dates nothing: an
entry holding one is read as one this node cannot read — unknown coverage for a peer or a counted non-peer, where it
used to count as a permit for ever. Its owner rewrites its own entries without such times on every withdrawal tick,
under the tally lock, and withdraws one with nothing honest left; so a forged time written into a live member's entry
holds that counter `Indeterminate` for one tick (a third of `control_max_staleness_ms`) plus its propagation. The
collector dates a non-peer's entry by its honest times only, so one holding nothing else is collected at once. A
heartbeat dated in the future is no heartbeat for a non-peer. **Not closed:** forged times that look honest — inside
the window — inflate a count; tally entries are unsigned. **Unreadable entries** go with their node's heartbeat (or,
with none, once nothing of the node is live), never at once: a count that cannot be read is not zero, and an older
collector must not erase a newer node's entries during a format change. **The heartbeat goes last:** a collector
takes a non-peer's entries each by its own times, and its heartbeat only once none of them is live and its `as_of` is
past `MAX_FLEET_WINDOW_MS` (24 hours) plus both bounds, so a departed node's heartbeat stays that long. **The cost,
bounded:** one key per departed node, and about one extra KV read per tally decision for each node id that departed in
the last 24 hours plus the bounds (the decision reads its heartbeat and skips it as past the counter's horizon).

**Will the compiler tell me?** No — no API changed. A tally rule can decide `Indeterminate` where it admitted before:
for at most that window, while a node's entries and heartbeat are out of step at the reader; and for about one
heartbeat tick, while a live member's entry holds a forged time, until its owner scrubs it.

**Migration.** None. A fleet that sees `Indeterminate` naming a node "not a peer: its entry is inside the window and it
has no heartbeat" is reading a member whose heartbeat has not reached it; it clears when the heartbeat arrives or the
entry ages past the bound.

## 57. `validate()` no longer creates the persistence base path (2.37.0, adopter-surface X1)

**What changed.** `GossipConfig::validate()`'s early check of `persistence.base_path` used to probe by
`create_dir_all`, so a validator wrote to disk: loading a configuration (`load_from_file`, the binary's `-c`) created
the data directory before the node id was known. It now reads only — the nearest existing ancestor of `base_path` must
be a writable directory, or a warning names the problem — and creates nothing. `start()` creates the node's own
directory under it and refuses by name if it cannot (R7, 2.24.0), exactly as before.

**Why.** `mycelium doctor` runs `validate()` and must touch no disk (`docs/features/doctor.md`); and a validator with a
side effect is a validator that lies about what it checked.

**Migration.** An embedder that relied on `validate()` or `load_from_file` having created `persistence.base_path` —
say, to write its own files beside the node's before `start()` — creates it itself, or writes after `start()`. A
configuration that validated before validates now; the warning's wording changed from *is not writable* to the problem
found (*no ancestor of the path exists*, *the nearest existing path is not a directory*, *the nearest existing
directory is read-only*).

## 58. The causal fields: a malformed `_meta.correlation` or `_meta.parent` is refused at every door; a tool's `params` may carry `_meta` (2.38.0, adopter-surface X4)

**What changed.** Every door the AE preflight runs — `/mcp`, `/a2a`, the LLM doors, the federation call, the provider
check — now reads two keys under the request's `params._meta`: `correlation` (one key segment, at most 128 bytes) and
`parent` (a 64-character lowercase hex content hash), and refuses a request whose value is present and malformed:
`PreflightRefusal::Malformed`, reason `request_malformed`, JSON-RPC `-32602`. `POST /gateway/kv` reads the same two
keys at the body's top level and answers `400`. Before, any value under `_meta` other than `operation_id` and
`mandate` was ignored — **with or without an evaluator attached**: the check runs before the evaluator question, so an
inert gateway refuses the malformed request too, and records nothing of it. `/mcp` now forwards those two keys to the
provider inside the `tools/call` request it sends (`params._meta`); only the provider's enforcement check reads them
there — a tool handler, in-process or bridged, still receives its `arguments` and never sees `_meta`. A caller envelope
carrying an unknown `correlation` or `parent` field is admitted as before and counted once per request, after its
signature held (`GET /stats` `causal_fields_misplaced`, a map by door). `AeEvidence` also gained `node`, the enforcing
node as it names itself, absent on older records. **The provider's
`parent` is the gateway's decision** (the second PR): `/mcp` forwards the content hash of its own decision record as
the `parent` of the `tools/call` it sends, whatever `parent` the caller gave — the caller's stays on the gateway's
record — so a provider's record under enforcement cites the gateway's decision; a provider that reads `parent` as the
caller's own value reads the gateway's instead. Where the gateway's journal established no record — an inert gateway,
no journal attached, a lenient journal that lost it — the caller's parent is forwarded unchanged, so the provider's
`parent` has two possible meanings and the gateway's record, where one exists, is what tells them apart. The SDKs' A2A `send`, prompt-skill `call`, federation `call` and `set` verbs gained `correlation`/`parent`
options and `KvReceipt` two fields (`mycelium-py` 0.2.13, `mycelium-ts` 0.2.7); `mycelium-reason` 0.11.0 puts a
traced run's `run_id` on the route's records as `correlation`. `PreflightRefusal` gained the variant (the enum is `#[non_exhaustive]`; a `_` arm must
already fail closed); `ActionEnvelope`, `AeEvidence`, `DecisionRecord` and `WriteReceipt` gained two optional fields
(all four are `#[non_exhaustive]`, built through their builders); the evidence record's serialised form is unchanged
when neither is set.

**Why.** A record that cites the record it depends on, by hash, is what lets a reader compute the closure of a
decision across nodes without a coordinator; a field placed outside the signed payload could be re-pointed under a
signature that still verifies (2.12.0's lesson), so it is refused by placement; and a malformed value corrected or
dropped would leave a record that reads as a root.

**Migration.** A caller that put other data under `_meta.correlation` or `_meta.parent` renames it. An evaluator
matching `PreflightRefusal` by name adds the arm or keeps its `_`. A reader of `/stats` that took
`causal_fields_misplaced` as a number (it was never released as one) reads the map. Nothing else changes for a caller
that sends neither key.

## 59. `PolicyViolation::ApprovalTimedOut` was never produced; the grant mode's refusals are `GrantNotPresented` and `GrantRefused`; `MandateRefusal` is `#[non_exhaustive]` (2.38.0, adopter-surface X3)

**What changed.** `AgentPolicy` gained `require_grant_for` and `grant_window`, and `AgentStateMachine` gained
`transition_invoking(tool, arguments)`: a tool listed there is invoked only under a **grant** — a scoped mandate a
supervisor issues for one call through `POST /gateway/grants/{agent}/{request}` (`grant:issue`; `GET
/gateway/grants/pending` under `grant:read`) — and silence refuses with `PolicyViolation::GrantNotPresented`, a
supervisor's refusal with `GrantRefused`. A plain `transition` to `Invoking` on a grant-required tool is refused the
same way. `PolicyViolation::ApprovalTimedOut` is `#[deprecated]`: `await_approval` has always returned `Ok` on
silence, so the variant was never produced and read as if a timeout refused. `MandateRefusal` gained
`BoundToOtherArguments` and `BoundToOtherPolicy` and is now `#[non_exhaustive]`; `MandateBinding` gained `digest` and
`bound_policy_revision`; `AeEvidence` gained `mandate_digest`; `Rule::requiring_grant()` and a trailing `*` in
`requiring_mandate` name a scope family. The KV namespace `grant/` is route-owned: the raw KV routes refuse it 403
`protected_key`. New scopes `grant:read`, `grant:issue`. `mycelium-guardrails` 1.1.0: `Policy::require_grant_for`,
and `require_approval_for`'s strength label now says *veto, not grant*.

**Why.** The veto mode's silence-approves is the opposite of what an approval gate is taken to mean; an enum variant
that reads as a timeout refusal and is never produced misleads a `match`. A grant that is a mandate inherits every
door the bypass matrix enumerated rather than needing the matrix re-run for a new signal.

**Migration.** A `match` on `PolicyViolation` adds arms for `GrantNotPresented` and `GrantRefused` (the enum is not
`#[non_exhaustive]`; the deprecated arm compiles with a warning). An exhaustive `match` on `MandateRefusal` outside
this crate adds a `_` arm that fails closed. Nothing changes for an agent whose policy lists no grant-required tool.

## 60. `Execution` gained `Unresolved`; `RefusalSnapshot` gained `external`; `mycelium-effects` gained features `sim` and the `external` module (2.38.0, adopter-surface X5)

**What changed.** `mycelium::Execution` (`#[non_exhaustive]` since 2.11.0) gained `Unresolved` — an effect whose
fate was asked for within its declared probe budget and not established: escalated. `mycelium_effects::RefusalSnapshot`
gained the field `external: ExternalRefusalSnapshot`, the side-effect contract's refusals by reason.
`mycelium-effects` gained the module `external` (`Reconciler`, `ExternalEffect`, `EffectClass`, `ProbeBudget`,
`ExternalDestination`, `ExternalRefusal`, `CompensationPermit`), the module `http` (`HttpDestination`), the
re-export `EgressPolicy`, a `sha2` dependency and the Cargo feature `sim` (forwards `mycelium/sim`; off in every
shipped build). `mycelium::sim_seam::input` is new in both builds.

**Who is affected.** A `match` on `Execution` already needs a `_` arm (2.11.0's note), and that arm **must fail
safe**: `Unresolved` is *we looked and could not tell*, never *nothing ran*. An exhaustive struct literal of
`RefusalSnapshot` breaks; `RefusalSnapshot::default()` and `.snapshot()` are unaffected. `refusals()` now includes the
contract's refusals.

**Migration.** Add the field, or build the snapshot from the counter. A consumer that summarises `Execution` treats
`Unresolved` as it treats `Unknown`, with the record's `reason` naming the budget.

## 61. A unit's `runtime` key is refused by readers before this release (2.38.0, adopter-surface X8)

The unit file gained an optional `runtime` key (`python` · `typescript` · `strands` · `langgraph`), read by
`wire-check` to say what that runtime carries ([the adapter contract](../reference/adapter-contract.md)) and
accepted, not read, by `POST /gateway/units/declare` and the stem. Every reader refuses an unknown key by name
(§49, 2.35.0), so a unit carrying `runtime` is refused by a gateway, a stem or a `wire-check` older than this
release: the gateway answers `400 invalid unit file`, the others refuse the directory. **Migration:** declare
`runtime` only once every gateway and stem that reads the directory runs this release; the Python scaffold ships
it because the scaffold pins the release it came from. Nothing else changed shape; a unit without the key is read
as before.

## 62. The side-effect contract moved into the substrate; its ledger is a journal; `Class` gained `Recovery`; `ExternalRefusal` gained `LeaveOfObservable` (2.38.0, adopter-surface X9)

**The move.** `Reconciler`, `ExternalEffect`, `EffectClass`, `ProbeBudget`, `ExternalDestination`, `HttpDestination`,
`CompensationPermit`, the refusal counters, and the vocabulary they rest on (`Effect`, `EffectRefusal`, `Composition`,
`CompositionLeg`, `check_composition`) are `mycelium::effects` now. `mycelium-effects` **re-exports every one of
them at its old path** (`mycelium_effects::external::*`, `mycelium_effects::http::*`, `mycelium_effects::{Effect, …}`),
so a companion user changes nothing; a user who wants the contract without the companion takes it from the
substrate. The companion's `envelope` feature still selects the evidence half (it is a `gateway` + `tls` build of the
substrate); its `metrics` feature now forwards to the substrate's. **Migration:** none for a companion user; a direct
user of `mycelium::effects` needs `gateway` + `tls` for `CompensationPermit` and `with_evidence`, as the companion
always did.

**The ledger.** The reconciler's ledger was a SQLite file (never released: it was on `main` only, between 2.37.0 and 2.38.0); it is the
**node-local journal** now — append-only, fsynced, folded at open, one owner at a time. A ledger file from an
unreleased X5 build is not read; there is no released format to migrate. **Behaviour change:** a second reconciler
opened on a ledger another still owns is refused by name (`ExternalRefusal::Ledger("… is owned by another …")`) —
hand a ledger over by dropping the first; `rusqlite` is no longer a dependency of the contract.

**`Class` gained `Recovery`** (`mycelium::faults::Class`, not `#[non_exhaustive]`): an exhaustive `match` over the
fault classes needs an arm; `Class::ALL` is six. `mycelium test --faults` with no class list now runs `recovery`
too — it reads `not_configured` without `[persistence]` and `not_in_build` without `gateway` + `tls`, and fails the
run only when asked for by name, like the others.

**`ExternalRefusal` gained `LeaveOfObservable { class }`** (`#[non_exhaustive]`, 2.10.0's note: a `_` arm fails
closed) and **`ExternalRefusalSnapshot` gained `leave_of_observable`** (an exhaustive struct literal breaks;
`..Default::default()` is unaffected); `Reconciler` gained `apply_caused`, `leave` and **claims under the ledger's
lock** for the retry, the probe, the compensation and the leave — of two moves racing on one record, the second is
refused before anything is sent (`SecondRetry`, `ProbeBudgetExhausted`, *already in flight*, `Unresolved`), where
before both could reach the destination (#677's review, finding 3). **Three more behaviour changes of the move**
(#677's review): `Reconciler::open` needs a tokio runtime (the ledger's writer is spawned there; the SQLite
constructor needed none); a ledger frame the fold cannot decode **refuses the open by name** rather than forgetting
the record (a forgotten record is an effect sent again); a `Failed` record re-prepared **keeps its probes, retries,
first unknown, evidence hash and compensation link** — *never reset*, as the field's doc says and the SQLite ledger
did. And two of X5's own, fixed on the way: a compensation whose send **fails** (a failed answer, a conflict, a
composition that did not hold) has its link taken off, so another compensation may be issued — before, the record was
refused *already in flight* for ever, while a bare `apply` of the failed compensation's id completed it without a
permit; and a **composed compensation is refused** (`compensate` takes no resource authority to check it against)
rather than sent unchecked — stated as not built.

## 63. A grant admits one call at each door; `MandateRefusal` gained `GrantUsed`; a grant is issued for a member's agent and revoked at its issuer; the authority's sweep judges a grant by its own standing (2.38.0, adopter-surface X3, second PR)

**Behaviour changes.** Every enforcement point (`gateway`, `provider`) now admits a grant **once**: a second presentation
of the same grant at the same door is denied `GrantUsed { door }` (`MandateRefusal`, `#[non_exhaustive]` — a `_` arm fails
closed) and the marker `grant/{agent}/{request}/used/{door}` is written beside the request; before, the doors accepted one
grant for identical calls until its window ended. A grant admitted at a door is spent there whether or not the work
completes. `POST /gateway/grants/{agent}/{request}` issues for an agent on **any fleet member** whose identity key the
gateway holds (it answered `409 not_this_node` for every other node); the agent's node decides whether that issuer may
with `EntitlementTable::entitle_grants_from_fleet` — a node that entitles only itself (`entitle_grants`) refuses a grant
from another node as *not established*, as before — and a principal no member's is `409 not_a_member`. New route:
`POST /gateway/grants/{agent}/{request}/revoke` (`grant:issue`; the evaluator decides `grant.revoke`), answered `409
not_the_issuer` anywhere but the issuing node; new counter `mycelium_grants_revoked_total`; new KV rows under the
route-owned `grant/` (`…/used/{door}`, `…/revoked`). **Fixed:** the authority's sweep judged work running under a grant
by the resource gate, whose scope rule a grant's own scope fails, so such work was cancelled at the first sweep tick
whether or not the grant was revoked (unobserved while no granted handler outlived a tick); it is judged by the grant's
window and its issuer's checkpoint now, and stops on revocation within the drain bound. **Also:** a grant's checkpoint is
exempt from the revocation view's start rule (`IssuedBeforeStart` is a freshness rule, and a grant reads no freshness) —
before this, every grant issued before a node's restart was refused after it; the agent's wait no longer refuses an
*unknown* standing but waits for the issuer's checkpoint to the expiry (a revoked standing is final); the SDKs gain
`grant_revoke` / `grantRevoke`. **Migration:** a client that re-presented one grant for a repeated call now asks for a
grant per call; a fleet whose supervisor sits at another gateway than the agent calls `entitle_grants_from_fleet()` on
the agent nodes' entitlement tables — **and accepts that every member's evaluator may then grant there**; a fleet that
wants one issuing node keeps `entitle_grants(that node)`.

## 64. `AgentPolicy` gained `grant_arguments_class` and `grant_renderable_classes`; a streamed A2A task can report `input-required`; `GET /gateway/grants/stream` (2.38.0, adopter-surface X3, third PR)

`AgentPolicy` gained two fields (an exhaustive struct literal breaks; `..Default::default()` is unaffected): the
arguments' data class and the classes a grant request may render. **Behaviour change for a policy that sets a class:**
arguments of a class not listed are withheld from the gossiped request record — shown as withheld with the class and
the size — while the grant binds their digest; with no class set, nothing changes. A streamed A2A send
(`tasks/sendSubscribe`, `message/stream`, `SendStreamingMessage`) whose serving node's agent is waiting for a grant now
emits a status update `input-required` (A2A 1.0: `TASK_STATE_INPUT_REQUIRED`) with the request's id in the message,
then `working` once it is answered — a client that treated every non-terminal status as `working` sees a new one.
The streamed A2A dispatch's bound is 120 s now (it was 30 s; the unary path's, and the longest a task can wait
for a grant through this door). New route `GET /gateway/grants/stream` under `grant:read`, with `grant_request`,
`grant_answered` and `grant_expired` events; the SDKs gain `grants_stream` / `grantsStream`.

## 65. A `[[rule]]` gained `requires_grant`; a `[[rule]]` or `[[mandate]]` with an unknown key is refused; `Class` gained `Grant` (2.38.0, adopter-surface X3, fourth PR)

The unit file's `[[rule]]` gained `requires_grant = true` — the rule applies only under a grant, `Rule::requiring_grant`'s
shorthand (one of this and `requires_mandate`, never both: refused at load by every reader) — and `mycelium test --faults` gained the `grant` class, which
drives the declaration's first such rule on its own fleet: default-deny, the declared issuer's grant, the restart over
the WAL. **Two readers change behaviour.** A `[[rule]]` or a `[[mandate]]` carrying a key it does not have is now
refused by name, as the unit file's sections already were (§49): a misspelt `requires_grant` or `requires_mandate`
loaded, until this release, as a rule that required nothing and admitted without a word. **Migration:** a rule or a
mandate with a stray key is refused by `wire-check`, `test`, the stem and `POST /gateway/units/declare` (`400 invalid
unit file`) — fix the key. A reader older than this release drops `requires_grant` silently and reads the rule as
plain, so declare it only once every reader of the directory runs this release; the scaffolds ship it because they
pin the release they came from. The other section tables (`[[capability]]`, `[[group]]`, `[[lane]]`, `[hosts]`,
`[[presence]]`, `[[serve]]`) still accept an unknown key silently — stated, not changed here. `Class::ALL` is seven
long and an exhaustive `match` on `Class` needs a `Grant` arm. `wire-check` gained one warning, `unissuable grant`: a
rule that requires a grant with no rule admitting anyone to `grant.issue` on its resource (`*` for the operation or
the resource counts; a `@<node>` suffix on either side is not matched — the reading `test --faults grant` shares).

## 66. A third guarantee profile, `secure-multi-domain`; ten `fed.*` guarantees in every report; `GuaranteeReport::unresolved` lists only what applies (2.39.0, adopter-surface Y7)

`profile = "secure-multi-domain"` is new: `start()` refuses a node unless the twenty guarantees `secure-single-domain`
rev 5 requires hold and the seven federation facts too — `fed.domain_enforced` (`domain_profile = "enforced"`), and
on a node with an edge or clients `fed.pinned`, `fed.body_bound`, `fed.partner_slots`, `fed.caller_named`,
`fed.catalogue_signed`, `fed.partner_egress` ([federation § Starting under `secure-multi-domain`](../operations/federation.md#starting-under-secure-multi-domain)).
`secure-single-domain` is **unchanged** (rev 5), and no node changes behaviour unless it selects the new profile.

**What a reader of the report sees change.** Every node's report gains ten entries in the `federation` subsystem —
`fed.domain_enforced` reads `not_configured` on a node whose domain profile is open, the other nine `not_applicable`
on a node with no federation edge or client, so a consumer that counts `not_configured` entries counts one more.
`GuaranteeReport::unresolved()` now leaves out an external prerequisite whose role predicate makes it moot (the
three federation ones on a node that faces no partner); before, it listed every external prerequisite. The log's
"external prerequisites are not verifiable here" count follows it. `PROFILE_NAMES` gained the name, so a reader
older than this release refuses `profile = "secure-multi-domain"` at `validate()` as an unknown profile.
`with_federation_clients`, `with_federation_edge` and `with_cohort_budget` after `start()` are counted in
`late_attachments` under their own names. Under `secure-multi-domain` — and only there — a federation edge or
clients attached after `start()` are **ignored** (a warning, not an error): that profile's check at start never saw
them; attach them before `start()`. Under `dev` and `secure-single-domain` a late attachment applies as before.
**Migration:** none is required. To adopt the profile, set what the runbook's table names on every node of every
federating domain, then select it; read each node's `GET /gateway/guarantees` after attaching its edge and clients,
because `mycelium doctor` builds a node from a file and cannot see them.

## 67. The `ops_console` example is a static page; its server and `/api` proxy are gone; `GossipConfig` gained `gateway_cors_origins` (2.39.0, adopter-surface Y3)

`cargo run --example ops_console` no longer exists: the Ops Console is `examples/ops_console/index.html`, a page you
serve with any static file host and open in a browser. Its `:8099` server and its `/api?host=…&path=…` proxy, which
fetched each node's routes for the browser, are removed — a process that fetches the fleet for a page is the
aggregating process decision D16 forbids, however stateless (D28). A bookmark of `http://127.0.0.1:8099/?target=…`
keeps working once the page is served on that port.

**What a node must now say.** A browser reads a node's answers from a page on another origin only when the node lists
that origin: `gateway_cors_origins = ["http://127.0.0.1:8099"]` (or `GOSSIP_GATEWAY_CORS_ORIGINS`). The setting is
**off by default** and changes nothing for a node that does not set it — no response carries
`Access-Control-Allow-Origin`. Set, it answers only `GET`/`HEAD` on the read-only routes
([observability § Cross-origin reads](../operations/observability.md#cross-origin-reads-gateway_cors_origins)), and the
bearer still decides every read. `validate()` refuses `*`, `null`, an entry with a path, query, fragment, credentials,
uppercase, a default port or any form a browser would not send, and a non-empty list on a node with no `http_port`,
naming `gateway_cors_origins`; `start()` refuses a non-loopback origin on a gateway with no credential model.

**What breaks.** `GossipConfig` gained a field, so an exhaustive struct literal breaks (`..Default::default()` is
unaffected); a reader older than this release refuses a configuration file that sets `gateway_cors_origins` as an
unknown key only if it denies unknown keys — `GossipConfig` does not, so an older node ignores it and answers no
cross-origin read. **Migration:** serve `examples/ops_console/` (for example
`python3 -m http.server 8099 --directory examples/ops_console`) and add its origin to each node you point it at.

## 68. A unit file's `[[schedule]]` is refused by readers before this release; `NodeCapabilityConfig` gained `schedules`; `sched/` is an owned namespace and the `schedule` capability namespace reserved (2.39.0, adopter-surface Y2)

The unit file gained `[[schedule]]` — declared recurring work, run by the embedding application with
`GossipAgent::run_schedule` ([unit file § schedule](../reference/unit-file.md#schedule)). Every reader refuses an
unknown section by name (§49, 2.35.0), so a unit carrying one is refused by a `wire-check`, a gateway or a stem older
than this release. In this release the gateway's `POST /gateway/units/declare` refuses it too (`422`,
`schedule section`) and a stem refuses to start with one, naming it: only an application supplies the work. The raw
KV routes refuse a key under `sched/` (`403 protected_key`): a forged `sched/{id}/last/{node}` would skip or repeat
slots. The capability namespace `schedule` is reserved for a schedule's candidacy and holding: `advertise_capability`
writes nothing there (a warning), the gateway's `capability/advertise` and `units/declare` answer `403
reserved_namespace` / `400`, and the loader refuses a `[[capability]]` or `[[group.provides]]` in it.
`NodeCapabilityConfig` gained the field `schedules`, so an exhaustive struct literal breaks (`..Default::default()`
is unaffected). **Migration:** declare `[[schedule]]` only in units read by this release's tools; an application
writing `sched/` keys through the KV routes, or advertising a capability in the `schedule` namespace, moves them
elsewhere.

## 69. `mycelium tls issue` refuses to replace a node key it issued before; `--force` replaces it; `tls ca` and `tls pin` are new (2.39.0, adopter-surface Y8)

`mycelium tls issue --ca-dir <ca> --node <ip:port> --out <dir>` used to write over `<dir>/<node>.key.pem` silently.
A key already issued is an identity in use — a node runs on it, a federation partner pins its gateway's key — so
issuing over it now exits 2, naming the file and what replacing it means: the node must restart on the new key, and
every partner that pins its gateway must be given the new pin. **Migration:** a script that re-issues on purpose (the
secure posture's recovery, [cert rotation § 2](../operations/cert-rotation.md)) adds `--force`. Also new:
`mycelium tls ca --out <dir>` mints a fleet CA on the issuer's host, refusing a directory that holds one (two
concurrent mints publish one CA, the other refused; it holds `ca.lock`, so it is refused while a node or another mint
holds it; it needs a filesystem with hard links; a `ca-key.pem` left without `ca-cert.pem` is named by `tls ca`,
`tls issue` and a node's start — an interrupted creation when its lock or temporaries are beside it, otherwise a lost
certificate to copy back from a node, never a key to delete), and `mycelium tls pin <cert.pem>` prints a gateway certificate's
pin, refusing a CA certificate and a file that carries a private key. `mycelium::doctor::Doctor` gained `attached`
(`run_attached` is new), so an exhaustive struct literal breaks; nothing outside the crate builds one.

## 70. A federation edge refuses a credential presented twice; `CallRefusal` gained `Replayed`, `AlreadyAnswered` and `ReplayUndecidable` (2.39.0, #695's review)

A gateway's `/a2a` now serves each federated credential **once**: the same signed bytes presented again are refused
`-32003` (`Replayed`), and while a partner's window is full a credential expiring no later than the oldest one it had
to forget is refused `-32004` (`ReplayUndecidable`). Before, the edge kept no record and served a captured credential
as often as it was sent inside its lifetime ([federation § A credential is one call](../operations/federation.md#a-credential-is-one-call-replays)).
`FederationClient` now mints each credential with a strictly later `issued_at_ms` than its last, and puts a random
JSON-RPC `id` in each call's body (it was `1`). **Any answer spends a credential:** one `/a2a` refused before its
dispatch, presented again, is refused `-32003` `AlreadyAnswered` (counted apart from replays,
`federation_credentials_already_answered`), never served — a retry needs a fresh credential.
`FederationEdge::authorize_and_claim` is new; `FederationEdge::claim` returns a `ReplayClaim` to `spend` at the
dispatch (dropped unspent, the credential stays spent, marked refused); `FederationEdge::authorize` is unchanged and
replay-blind, so a door an embedder builds on it alone must claim too.

**Who notices.** A client that re-sends a credential already served — a retry that reuses the header rather than
minting a new one — is refused the second time — `Replayed` if it was served, `AlreadyAnswered` if it was refused; this release's
`FederationClient` mints per attempt and is unaffected.
A partner running a client older than this release that makes the identical call (same principal, export and body)
twice inside one millisecond mints one credential twice, and the second is refused as a replay. An exhaustive `match`
on `CallRefusal` already needed a `_` arm (it is `#[non_exhaustive]`), which must fail closed. `GET /stats` gained
`federation_replays_refused` and `federation_replays_undecidable`. **Migration:** mint a credential per request;
upgrade partners' clients.

## 71. A directory the TLS module creates for keys and certificates is `0700`; every certificate it writes is `0644` (2.39.0, #695's confirmation round)

On unix, `auto_cert_dir` when a node creates it at start, `mycelium tls ca --out` and `mycelium tls issue --out` when
the directory does not exist yet, are now created `0700` whatever the umask (they were left to the umask, `0755` under
the usual `022` and `0777` under `000`), and the CA certificate and an issued node certificate are written `0644`,
narrowed when a wider file is rewritten; their missing parents are created `0700` too. Under a permissive umask another
local user could otherwise replace the trust anchor. A directory or `ca-cert.pem` that already exists is never
narrowed silently: a node warns at start, and `mycelium tls ca` / `issue` after their work, when either is writable by
group or others, naming the command — **remediation for a deployment created under a permissive umask:**
`chmod 700 <cert dir>` and `chmod 644 <cert dir>/ca-cert.pem`. Also: a node whose certificate directory holds
`ca-key.pem` without `ca-cert.pem` now refuses to start, naming the state, where it minted a new CA over that key.
**Who notices.** A deployment whose node directory
is created by the node and then read by a *different* user — already unable to read the `0600` keys since 2.38.2 —
now cannot list the directory either. **Migration:** create the directory yourself with the mode you need before the
first start, or read it as the node's user.

## 72. A `[[rule]]` may carry an `exposure` table, refused by readers before this release; `RuleDecl` gained `exposure`; `sys/ae-exposure/` is a self-owned namespace; a `protected_rpc_kinds` entry named as an exempt operation refuses the start (2.39.0, adopter-surface Y5)

A `[[rule]]` with an `exposure` table is an **exposure rule** — it holds work that extends an unresolved closure while
that closure is past a threshold ([feature page](../features/exposure-policy.md)). Every reader refuses an unknown key
in a `[[rule]]` by name (§65, 2.38.0), so a unit carrying one is refused by a `wire-check`, a gateway, a stem or
`mycelium test` older than this release. An exposure rule is not an allowance: `RuleDecl::to_rule` refuses it, the
evaluator loaders skip it and `wire-check` admits no edge by it; one with a `requires_*` field, no threshold, a zero
one or the `fleet_consensus` scope is refused at load. `RuleDecl` gained the field `exposure`, so an exhaustive struct
literal breaks. Every node built with `gateway` and `tls` now writes `sys/ae-exposure/{node}` on its tally
heartbeat's tick — one key per node, owned, its foreign writes counted by the `sys/` tripwire — and collects an
out-of-view node's unreadable, forged or expired one. **A `fleet_tally` rule fails closed until every node publishes
a summary**, so it holds or refuses as not established until the whole fleet runs this release — and a node publishes
only when it attaches an exposure policy with a rule or an exposure source (a node with neither is inert, and a
fleet-tally rule reads it as unknown coverage); an
`enforcement_point` rule needs only the node that decides. Under the `legacy` profile — the default — the policy is
neither read nor counted. A `protected_rpc_kinds` entry named `effect.probe`, `effect.compensate`, `effect.leave`,
`grant.issue` or `grant.revoke` refuses the start, by name: a provider checks a protected kind under its own name, and
those are the operations the policy never holds. **Migration:** declare `exposure` only in units read by this
release's tools; build `RuleDecl` literals with `exposure: None`; use `enforcement_point` until every node is
upgraded; rename a protected kind that collides.

## 73. A take, claim or emit door is decided when an exposure rule selects it; `TupleError`, `BlackboardError` and `ShardError` gained `Refused` (2.39.0, adopter-surface Y11)

A tuple take, a blackboard claim and the four emit doors — `/gateway/tuple/take`, `/gateway/tuple/take_by_key`,
`/gateway/bb/claim`, `/gateway/signal/emit`, `/gateway/mailbox/deliver`, `/gateway/overlay/emit_reliable`,
`/gateway/shard/emit`, and the new Rust verbs `take_in`, `take_by_key_in`, `claim_in`, `emit_in`, `deliver_event_in`,
`emit_reliable_in`, `emit_sharded_in` — run the action evaluator and the exposure gate when an exposure rule selects
their operation (`tuple.take`, `tuple.take_by_key`, `bb.claim`, `signal.emit`, `mailbox.deliver`,
`overlay.emit_reliable`, `shard.emit`), and only then ([feature page](../features/exposure-policy.md)). **An exposure
rule whose `operation` is `*` now selects these doors too**: on a node with an evaluator, a route that answered 200
answers 403 when the evaluator does not permit the operation or the exposure gate holds the work, and 400 for a
malformed `_meta`; under `observe` a selected door admits what the evaluator refuses, recorded. A raw
`/gateway/rpc/call`, `/gateway/scatter` or `/gateway/overlay/emit_reliable` of a companion's take or claim kind
(`tuple.{ns}.take`, `tuple.{ns}.take_by_key`, `blackboard.{ns}.claim`) runs that door's decision too — only at a
gateway node that hosts the companion; through one without it the call reaches the provider undecided. The existing
verbs are unchanged and never decided — `emit`, `emit_ordered`, `emit_async`, `request`, `advertise`,
`advertise_persistent`, `mycelium_core::ops`'s `emit_signal*`, `deliver_event`, `emit_reliable`, `emit_sharded`, `take`,
`take_by_key`, `claim`, a member's `rpc_call`, `rpc_call_with_mandate` and `rpc_call_as` — nor is a hosted WASM
component's `mesh.emit`. `TupleError`,
`BlackboardError` and `ShardError` gained a `Refused` variant, returned only by the new verbs; all three are
`#[non_exhaustive]`, so a `match` already has a `_` arm. **Migration:** an exposure rule written for Y5's call doors
names its operation (`tools/call`, `skill.invoke`, …) rather than `*`, or the evaluator permits the door operations it
should admit.

## 74. A client runs a `[[schedule]]` through the gateway (`GET /gateway/schedules/run`, scope `schedule:run`); `units/declare`'s schedule refusal names it; `sched.run` gained the reason `unacknowledged`; the Eve loader loads `schedules/` (2.39.0, adopter-surface Y12)

An SDK agent can now run declared recurring work: `GET /gateway/schedules/run?toml=<one [[schedule]]>` makes the node
a candidate in the schedule's holder band while the stream is open and hands the client each run, answered with
`POST /gateway/schedules/ack` ([feature page](../features/recurring-work.md)). A client's schedule runs under `{id}~{principal}` — its
principal's, so it reaches no application's schedule and no other principal's, and its runs' `operation_id` is
`sched/{id}~{principal}/{slot}`. A static-token principal needs `gateway_identity_issuer` set — the same on every
gateway — or the stream answers `422 issuer_not_fleet_wide`. Both routes need the new scope `schedule:run`; a scoped token without it is refused `403` naming it, and `mesh:serve` does not open them. A unit
still cannot carry a schedule: `POST /gateway/units/declare` answers `422 schedule section` as before, and its
`detail` now names the run stream instead of saying no route runs one. (`sched.run`, the rule behind declared
recurring work, is itself in no release yet: its reasons `unacknowledged` — a run handed to a client that never
answered, its slot not recorded — and `stood_down` — a stalled client's node withdrawing its candidacy — arrive with
it, so no released consumer matches them.) In `mycelium-ts`, `loadEveAgent` now loads an Eve
`schedules/` directory whose files' crons are a period and a phase, where it refused the directory by name before,
and refuses by name each schedule it cannot run (another cron, a nested schedule, a key it does not read).
**Migration:** give a token that runs schedules `schedule:run`, and set `gateway_identity_issuer` on every gateway it
uses; an Eve agent whose `schedules/` was refused now loads, and its schedules run only when
`runEveSchedules` is called.

## 75. A successful empty JWKS withdraws the IdP's keys, and keys the IdP has not confirmed for 24 h are refused (2.39.0, external review 2026-10-31 F4)

The OIDC verifier read a JWKS fetch that came back with **no keys** as a **failed** fetch and kept the keys it held,
so an issuer that withdrew every key — a compromised one among them — saw the node go on accepting tokens signed under
them; and a fetch that kept failing left the node verifying against keys of any age for as long as it ran. Now a
fetch that **succeeds** (a 2xx response carrying a JWKS) is authoritative: the cache becomes exactly what it returned,
an empty set included, so a token signed under a withdrawn `kid` is refused at the next refresh. A fetch that
**fails** (unreachable, timed out, non-2xx, a host the egress list denies, or a body that is not a JWKS) keeps the
previous keys as before, for at most **24 h** after the last successful fetch (`JWKS_MAX_STALE`, a constant);
past that every token is refused until a fetch succeeds. **What changes for a deployment:** a key the IdP has pulled
from its JWKS stops working within one refresh (the TTL, or the 30 s cooldown when an unknown `kid` forces one)
instead of never; an IdP outage longer than a day makes the gateway refuse OIDC bearers rather than keep trusting
stale keys. **Migration:** none for a healthy IdP. If your IdP publishes an empty JWKS while rotating (some do,
briefly), tokens are refused during that window; if an outage of more than a day must not lock operators out, keep a
static-token principal for them (`rbac.md`).

## 76. A companion's RPC kind at a gateway door — in a raw route's body, or the path of a signal, mailbox or serve stream — needs the companion's family scope (2.39.0, external review 2026-10-31 F1, plan row R1)

The gateway's raw routes — `POST /gateway/rpc/call`, `scatter`, `signal/emit`, `mailbox/deliver`, `shard/emit` and
`overlay/emit_reliable` — take an RPC kind from the request body. A companion serves its operations as kinds of the
form `blackboard.{ns}.{op}`, `tuple.{ns}.{op}` and `wiki.{group}.{op}`, and its own routes gate each behind a family
scope (`board:write`, `tuple:write`, `wiki:write`; the read verbs behind `:read`). The raw routes carried the same
kinds to the same handlers under the route's own scope alone, so a `mesh:write` holder with no `board:write` could
post to a board through `rpc/call` where `/gateway/bb/post` refused it. Now a companion's kind on a raw route is
refused `403 {"error": "insufficient scope", "required_scope": "board:write", "kind": "blackboard.board.post"}`
unless the token also holds the family scope the companion's route would require — the read verbs (`read`,
`depth` and `snapshot` for a board; `depth` and `wal_replay` for a tuple space; `access` for a wiki) need `:read`,
and every other verb under the prefix, an unknown one included, needs `:write` ([`rbac.md`](../operations/rbac.md)).
`snapshot` and `wal_replay` are the companions' replica-sync reads — a snapshot answers every fact on the board,
payloads included, and a WAL page the space's items — and were under `:write` until #714's second review round, so
a `board:write` token with no `board:read` read the whole board through `rpc/call` where `bb/read` refused it. The route's own scope is
still required; an open gateway, which gates no scope on the companion's routes either, is unchanged; a build
without `compliance` gates no scope anywhere. **Migration:** a scoped token that reached a companion through the
raw routes under `mesh:write` (or `consensus:write` on `overlay/emit_reliable`) alone now needs the companion's
scope too — add `board:write`, `tuple:write` or `wiki:write` (or the `:read` form) to it. A token used only on the
companion's own routes is unaffected. `mycelium-py` 0.2.14's `on_signal` (0.2.14 ships with v2.39.0; 0.2.13 shipped with v2.38.0) raises a refused stream as the HTTP error
(`httpx.HTTPStatusError`, or `ProtectedKindError` for a protected kind) where every earlier version surfaced any
`403` there as httpx-sse's content-type `SSEError`; `mycelium-ts` needs nothing.

The three doors that take the kind from the **path** are gated the same way, with the verb fixed by what the door
does (#714's review): `GET /gateway/signal/sse/{kind}` and `GET /signals/{kind}` register a tap on the handler table
a companion's `rpc_rx` registers on and receive every frame of the kind, payload included, so observing a
companion's kind needs the family's `:read` whatever the kind's own verb — a `mesh:read` token read each board
post's contents where `/gateway/bb/read` refused it; `GET /gateway/mailbox/{kind}` hands on what `mailbox/deliver`
put under the kind, and a delivery of a board kind is gated `board:write` there, so subscribing to it needs the
family's `:read` (the review's second round: the one path door the first round's list missed); and
`GET /gateway/rpc/serve/{kind}` answers the kind's requests
in the provider's place, so serving a companion's kind needs the family's `:write` as well as `mesh:serve` — a
`mesh:serve` token served `blackboard.other.post` on a node hosting no board. A declared skill naming a companion
kind reaches the serve door and is gated the same. The path is read percent-decoded, as the handler reads it. **Not
built:** a client with `cap:write` can still advertise any capability, a companion's `{ns}.primary` role included —
that is what `cap:write` means — so a client holding `cap:write`, `mesh:serve` and the family's `:write` can stand
in for a companion's primary; a client without the family scope cannot be sent its requests.

The gate reads the body before the handler's `Json` extractor, so a request naming a companion's kind is refused
`403` even where the handler would have answered `415` for its content type or `400` for a missing field; a body
the gate cannot read as JSON passes through to the handler's own refusal, and one over the 2 MiB limit answers
`413` as before. (This section is §76 because the plan's sibling PR #712, R4, took §75 first; R2+R3's, #713, is §77. The number
was taken at the merge, not resolved by overwriting.)

## 77. A grant record the disk will not take is a refusal by name: the door's use marker, the agent's `consumed`, a revocation (2.39.0, external review 2026-10-31, R3)

Four grant records now reach the disk **before** the step they admit, or that step is refused by name — the maintainer's
decision D2 on finding F2: a record that cannot be made durable is never a proceed.

- **The door's use marker.** A door that admits a grant writes `grant/{holder}/{term}/used/{door}` beside the request.
  Before this, that write was a fire-and-forget WAL append: a full queue or a poisoned writer dropped the record,
  counted it, and the call ran — so a node restarted after a crash had forgotten the use, and the same grant could be
  admitted at that door again within its window. Now the marker is on disk, fsynced, before the work runs, and a marker
  the disk will not take refuses the call: a denial on the record, *grant use not recorded durably at {door}: {reason}*,
  and the tool runs nothing. Nothing ran, so the use is **not spent**: the in-memory claim is released, and the same
  grant presented once the disk is repaired runs once — the answer a restart gives too, since no marker was written.
- **The agent's `consumed` marker** is on disk before the commit, or the invocation is refused
  `GrantNotPresented` naming it and the grant is left un-consumed; a retry after repair consumes once without asking
  again. A lost `consumed` did not fail safe: the un-consumed request was re-honoured and the tool admitted again, and
  a provider that attaches no enforcement has no door marker to remember a use by. Because the marker lands before the
  commit, a commit refused after it — the agent's state moved during the wait, or its budget ran out — leaves the
  request consumed and the grant spent with nothing run; the agent asks again, which is the safe side.
- **A revocation.** `POST /gateway/grants/{agent}/{request}/revoke` writes the grant's second checkpoint and its `revoked` record on the
  same path; a record the disk will not take answers **`503 not_durable`** (the node's state, not the request's — a
  retry is the answer) and the execution record says *failed*, where before the route answered `200` with
  `local_durability: failed` and a restart that kept only the WAL re-armed the grant for the rest of its window. The
  in-memory revocation stands in this process only — repairing the writer records nothing by itself, so **any** restart
  before the revoke is retried re-arms the grant; the supervisor owes a retry once `/health` no longer reports
  `wal_refusing_appends`. The issue-time checkpoint, `…/issued` and `…/refused` are on the same path: an answer the
  disk will not take is `503 not_durable`, no grant stands, and the request is still pending (the agent re-waits on it,
  `GET /gateway/grants/pending` lists it).
- **The two-door residual.** A call through `/mcp` is admitted at two doors, the gateway's and the provider's. If the
  gateway's `used/gateway` lands and the provider's `used/provider` write fails, the gateway's use is spent (a retry
  there answers `GrantUsed`, though nothing ran) while a direct call at the provider runs: a use is spent at the door
  that admitted it, and a refusal at a later door does not give it back. One execution at most, either way.
- **The window, by one fsync.** The door marker's `append_sync` now sits between the grant's assessment and the work,
  so the check-then-act window every door states (*narrowed and stated, not eliminated*) is longer by one `fdatasync`;
  a revocation landing inside it stops the work once it is registered, as before — and, at the provider, one landing
  before the work is registered refuses the call (§84).

The request itself stays a buffered append (losing it to a crash means the agent asks again). A lost `…/issued`
leaves the request standing un-answered — the agent re-waits on it, `GET /gateway/grants/pending` lists it, and the
supervisor's re-issue is not `already_answered` (that refusal needs the `issued` or `refused` record present), so the
grant is issued again against the same request. Under `sync_mode = "async"` or `"os"` each
of these four writes is a per-call `fdatasync` the node would not otherwise make. A node without `[persistence]` is
unchanged — its writes are in memory and it has never claimed a disk ([feature page](../features/grants.md)).
**Operator note:** a node whose WAL writer is poisoned (`/health`: `wal_refusing_appends`) now refuses every
grant-admitted call, every grant-required invocation and every revoke until a snapshot repairs it, where it used to
run them.

## 78. The gateway closes a connection that does not speak within `handshake_timeout_ms`, and serves at most `gateway_max_connections` at once; `GossipConfig` gained `gateway_max_connections` (2.40.0, external review 2026-10-31, R5)

The gateway's listener (`http_port`) bounded nothing before this release: a socket that connected and sent nothing held
a task and a file descriptor for as long as the client kept it open, on the plaintext listener and under
`gateway_tls` alike, and nothing capped how many it held — the gossip listener's `max_connections` and
`handshake_timeout_ms` never applied to it (finding F5). Now:

- **A silent connection is closed.** Under `gateway_tls` the TLS handshake, and then on both listeners the request's
  preface (on HTTP/1 its first byte; on HTTP/2 the 24-byte connection preface and the header of the frame after it),
  must each arrive within `handshake_timeout_ms` — the gossip listener's bound, now read by the gateway too (default
  `10_000`). A connection that misses either is closed with no answer and counted
  (`SystemStats::gateway_connections_timed_out`, `gateway_connections_timed_out_total`). After the preface, hyper's
  header-read bound (30 s) applies to every HTTP/1 request head — before, it was inert — and an HTTP/2 connection is
  sent a keep-alive PING after `handshake_timeout_ms` of quiet and closed if it does not answer within
  `handshake_timeout_ms`, each at least 1 s whatever the field says — so a dead HTTP/2 peer holds its permit at most
  max(2 × `handshake_timeout_ms`, 2 s) past its last activity. **Who notices:** a client that opens a connection and waits more than
  `handshake_timeout_ms` before its first request, or an HTTP/1 keep-alive connection idle for 30 s between requests, is
  closed by the server; a client library reconnects on its next request. **A pooling client whose idle timeout is
  longer than 30 s** (reqwest's default is 90 s; urllib3 and httpx keep connections until the server closes them) can
  send a request on a connection at the moment the server closes it: an idempotent request is retried by most
  clients, a **non-idempotent `POST` may surface as a connection error** the caller must decide whether to retry. Set the
  client's pool idle timeout below 30 s to avoid the race. A deployment that raises `handshake_timeout_ms` for slow
  gossip links raises the gateway's bound with it.
- **A cap.** `gateway_max_connections` (default `1024`, validated `1..=65535`, env `GOSSIP_GATEWAY_MAX_CONNECTIONS`)
  bounds the connections the gateway serves at once. At the bound it stops accepting: a further client is not refused
  but waits in the listen backlog (a fixed 1024, not `tcp_accept_backlog`, which is the gossip listener's) until a
  connection closes. A gateway behind a proxy that holds more than 1024 idle upstream connections should raise it, or
  cap the proxy's pool. **Long-lived streams count:** each SSE stream (`/gateway/signals/…`, `/gateway/rpc/serve/…`,
  the grant stream) holds a connection while it streams, so enough of them fill the cap, and then `/health` and
  `/ready` wait with every other request — an orchestrator's probe times out and may restart the node. Size the cap
  above the streams you expect, and watch `SystemStats::gateway_connection_cap_waits`
  (`gateway_connection_cap_waits_total`) and `gateway_connections_open`. A slot reserved for the probes is *not
  built*.
- **Shutdown releases them.** A connection still in its handshake or preface is closed when the node shuts down; one
  serving a request finishes it. **Under `gateway_tls` shutdown now waits for those**, up to the node's shutdown
  drain bound (5 s by default), as the plaintext listener always has; before, the TLS listener returned at once and
  left its connections running past the node's shutdown.
- **An accept error backs off.** Out of descriptors (`EMFILE`) or any other error that is not one client's, the
  gateway waits a second before accepting again, as it did before this release on the plaintext listener; a client's
  aborted connection is skipped at once.

**Struct literal:** `GossipConfig` gained `gateway_max_connections`, and `SystemStats` gained
`gateway_connections_timed_out`, `gateway_connection_cap_waits` and `gateway_connections_open` — an exhaustive
literal of either breaks; `..Default::default()` does not.

**Not bounded, stated:** a connection that keeps talking holds its permit, as on the gossip listener — an HTTP/2
client that answers PINGs and sends no request is one; a long-running stream holds its permit while it streams. There
is no per-source cap on the gateway (`max_connections_per_source` is the gossip listener's). A reverse proxy's own
limits remain the deployment's first bound against a flood.

## 79. A journal or a companion WAL whose directory cannot be synced refuses to open (2.40.0, external review 2026-10-31, R6)

**What changed.** Opening the node-local journal (and so the evidence journal, the rights ledger, the revocation and
epoch journals, the knowledge journals and the effects ledger), the tuple-space store's WAL or the blackboard's WAL now
syncs the holder of every directory on its path — on every open, not only the directories it creates — and the
directory holding the file, before the first record is acknowledged. An ancestor that existed before the open and that
the process may not read (`PermissionDenied`), or whose filesystem does not do directory fsync (`EINVAL`, `ENOTSUP`), is
passed over — logged once and counted: `mycelium::dir_syncs_passed_over()`, `SystemStats::dir_syncs_passed_over`, and
with `metrics` the counter `gossip_dir_syncs_passed_over_total`; a directory the open
created, the directory holding the file, and every other failure refuse the open, naming the directory. A sync
that fails is returned from the open; it used to be skipped because it was never attempted. `start()` syncs every
component of the path to `kv/` under `persistence.base_path` on every start, under the same rule. `SystemStats` gained
the field `dir_syncs_passed_over` (an exhaustive struct literal breaks).

**Who is affected.** A deployment whose journal or WAL directory — the directory holding the file, or one the open
creates — is on a filesystem that refuses `fsync` on a directory (some FUSE and network filesystems answer `EINVAL`).
That filesystem already refused the start-time snapshot, which has synced the KV directory since 2.4.4, so a node with
`[persistence]` there could not start; what is new is that an embedder opening a journal or a companion WAL there
without `[persistence]` is refused too. An *ancestor* on such a filesystem — a squashfs or iso9660 root under a
writable data mount, as embedded images have — does not refuse: it is passed over and counted. The pass-over can also
apply to a directory on the data filesystem itself, if that filesystem answers `EINVAL` or `ENOTSUP` only sometimes (a
FUSE daemon), but only to a directory that existed before the open — in practice one created before this release; a
directory an open creates is never passed over. A non-zero count on a node whose ancestors are all on one writable
filesystem is that case: read the `warn` line, which names the directory. `start()`'s refusal
now says a directory on the path could not be created or synced, naming it.

**What to do.** Put the journal and the companions' WALs on local storage the node owns, as the journal's own
assumptions already state. The new functions `mycelium::create_dir_all_synced` and `mycelium::sync_parent_dir` are what
these opens use, for an embedder's own log.

**What this does not establish.** The power-loss property — that the file is still named after the machine loses power
— is not observable without a filesystem fault adapter. The tests pin the order of the syncs and that each runs on a
real directory.

## 80. A bulk call works under `gateway_tls`; an operator certificate needs `bulk_ca_pem_path` that verifies it; `max_bulk_payload_bytes` bounds a payload (2.40.0, external review 2026-10-31, R7)

**What changed.** A `bulk_call` between nodes whose gateways serve TLS used to time out: the responder fetched the
staged payload over `http://` from a listener that speaks only TLS (F6). Now a caller whose gateway serves TLS marks its
ticket `https`, and the responder fetches over TLS, verifying the caller's gateway certificate by the caller's IP
address against the union of the fleet CA (with the mesh's removal and revocation checks) and
`gateway_tls.bulk_ca_pem_path`. A fetch whose certificate does not verify fetches nothing; it is never retried in
plaintext. A caller with a plaintext gateway sends the same ticket bytes it sent before. A new
`GossipConfig::max_bulk_payload_bytes` (default 64 MiB, env `GOSSIP_MAX_BULK_PAYLOAD_BYTES`) bounds the payload:
`bulk_call` refuses a larger one with the new `BulkError::PayloadTooLarge`, and a responder refuses a declared
`Content-Length` over it and abandons a body as soon as it passes it, where it used to collect the whole response. A
responder also refuses a status other than 2xx (a redirect, a `204`), whose body it used to hand the handler as the
payload. Every refusal is counted: `SystemStats::bulk_fetches_refused` (a new field) and
`mycelium_bulk_fetches_refused_total{reason}`. The node-cert gateway now presents the current identity at every
handshake, so after `rotate_identity` it serves the new certificate at once (it used to keep the start-time one until a
restart, which a bulk fetch refuses once the old key is revoked).

**Who is affected.**
- A node that serves a gateway (`http_port` set) on an **operator certificate** (`gateway_tls.cert_pem_path` set):
  `start()` now refuses with `InvalidField { field: "gateway_tls.bulk_ca_pem_path" }` unless `bulk_ca_pem_path`
  names a CA **and the node's own certificate verifies against it for the node's own IP address** — even on a node that
  makes no bulk call, because it cannot know whether a peer will fetch from it. Each operator certificate must carry
  its node's IP SAN. An operator certificate issued by the **fleet** CA is refused too unless it is a mesh certificate
  (an Ed25519 identity key): a responder judges it by the mesh's check, which would refuse every fetch from it. A node
  without `http_port` is unaffected (its gateway table is ignored).
- A fleet mixing node-cert and operator-cert gateways: every node that fetches from an operator-cert gateway sets
  `bulk_ca_pem_path` to that CA (on a node-cert node, beside the default fields). A node without `gateway_tls` has no
  place to name it and trusts only the fleet CA, so it cannot fetch from an operator-cert gateway — it refuses, logs and
  counts each such ticket, and the caller times out. Setting `gateway_tls` on it to name the CA also makes its own
  tickets `https`; a trust setting apart from the node's own gateway is not built.
- A bulk fetch is one TCP connection and one full TLS handshake (no pooling, no resumption), and both bulk fetch
  clients ignore `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` — a deployment that routed bulk through a proxy by the
  environment now goes direct.
- A rolling upgrade under `gateway_tls`: a responder older than this release drops an `https` ticket (its kind reads as
  invalid UTF-8), so a bulk call from an upgraded caller to an older responder times out — as every such call did
  before. Upgrade the responders, or the whole fleet; a plaintext fleet is unaffected.
- A bulk payload over 64 MiB: raise `max_bulk_payload_bytes` on **both** the caller and the responder. A responder
  holds up to this much per in-flight fetch, and keeps it while the handler runs; while `max_concurrent_bulk_handlers`
  (64) is non-zero that bounds the total, and at `0` (unlimited) nothing does.
- A `BulkError::Timeout` can now be permanent: a responder that refuses the fetch sends nothing back. Do not retry it
  blindly; read the responder's `bulk_fetches_refused` and its log (`docs/guide/error-handling.md`).
- Code that matches `BulkError` exhaustively: it is `#[non_exhaustive]`, so a `_` arm already covers
  `PayloadTooLarge`. Code that builds a `GatewayTlsConfig` with an exhaustive struct literal: add
  `bulk_ca_pem_path` or `..Default::default()`. `GossipConfig` gained `max_bulk_payload_bytes` and `SystemStats`
  gained `bulk_fetches_refused` (an exhaustive literal or destructure breaks; the `Default`-and-assign pattern is
  unaffected).

**Not built.** An integrity check on the payload beyond TLS (no per-payload signature); the fetch target is still the
address the caller's frames came from; a typed refusal reported back to the caller (a new reply on the wire). A
plaintext gateway still carries bulk payloads in the clear, and a TLS-terminating proxy in front of a plaintext gateway
is bypassed for bulk — the ticket says `http` and names the gateway's own port ([gateway-tls.md](../operations/gateway-tls.md)).
An application serving `/bulk/{id}` from its own server (`ServiceHandle::set_bulk_serving_port`) advertises the scheme
`gateway_tls` implies unless it calls the new `ServiceHandle::set_bulk_serving_https`.

## 81. A replica-sync answer is `Failed` while an inbound record is between its apply and its append; `WalHandle` gained `begin_inbound` and `inbound_in_flight` (2.40.0, external review 2026-10-31, R8)

A peer asked whether it holds a record (`sys.replica-sync.v1`, behind `set_with_replica_sync`, `POST /gateway/kv` with
`min_acks`, and both SDKs' replica-sync writes) used to answer `Persisted` from its store and a WAL sync. A record the
receive path had applied but not yet handed to the WAL was in the store and not in the queue the sync covers, so the
answer named a durability the peer did not have (finding F3). Now each inbound record is counted from before its apply
until it is on the WAL writer's channel, and the answer is `Failed` while the count is non-zero. The count is visible
as `/health`'s `persistence.inbound_in_flight` and the gauge `gossip_wal_inbound_in_flight`.

**Who is affected:** a caller that counts `persisted_by` under load. The count is one number for the node, not per
record, so a peer that is receiving a steady stream answers `Failed` for any record more often, and lands in
`missing` until a later round — `collect` re-asks until its deadline, so a short deadline against a busy peer returns
fewer acknowledgements than before. Nothing that was `Persisted` correctly becomes refused for good: the answer
returns to `Persisted` once the in-flight records are appended. A window abandoned without its append counts a dropped append,
which — as since the Phase-C audit — makes the node answer `Failed` for every record until restart (`/health`'s
`persistence.dropped_appends` shows it). That happens only when the connection task is aborted while its append
awaits — at shutdown, under `sync_mode = "flush"`; the read bounds wrap only the frame read, not its processing, and in
`async`/`os` the append does not wait. Under `flush` the count is released when the record is queued, not when its
`fdatasync` answers, so a slow disk alone does not hold it.

**API:** additive only. `mycelium_core::persistence::WalHandle` gained `begin_inbound()` (returning the `#[must_use]`
guard `InboundApply`), `append_inbound()` and `inbound_in_flight()`; `/health`'s `persistence` object gained
`inbound_in_flight`. Wire **v12** unchanged; invariant 1 (*apply, then persist*) unchanged.

## 82. Every crate requires Tokio 1.53.3 or later; `rustls-pemfile` is no longer a dependency (2.40.0, external review 2026-10-31, R9)

**Tokio.** Every crate in this repository — `mycelium`, `mycelium-core`, each companion and the examples — now declares
`tokio = "1.53.3"` where it declared `"1.35"`, and both templates where they declared `"1"`. Tokio 1.52.x carries RUSTSEC-2026-0386
(`AsyncFd::new` and its siblings violate I/O safety; unsound, not a vulnerability, and nothing in this tree calls
`AsyncFd`), and 1.52 has no patched release, so the floor is the first patched 1.53. Our own lockfiles were already
moved; the requirement makes sure an adopter's cannot resolve below it.

- **Who is affected:** an application whose `Cargo.lock` holds Tokio below 1.53.3, or which pins Tokio with `=1.52.x`
  or `<1.53`. Cargo refuses to resolve until it moves.
- **What to do:** `cargo update -p tokio` (to 1.53.3 or later), or relax the pin. Tokio 1.53 needs Rust 1.71; this
  workspace's `rust-version` stays 1.89, so no toolchain change follows.

**`rustls-pemfile`.** The `tls` feature no longer brings `rustls-pemfile` (RUSTSEC-2025-0134, unmaintained); the node
reads its gateway certificate and key, and its CA and node certificates, through `rustls-pki-types`' PEM parser, which
`rustls` already depends on. The accepted inputs are the same (pinned by `tls::pem_parse_tests`): certificates in order
with other sections skipped, PKCS#8, PKCS#1 and SEC1 keys by label, CRLF line endings, an encrypted key read as no key,
a malformed section an error.

## 83. `[authority]` attaches this node's authority from its declaration; `GossipConfig` gained `authority` (2.40.0, identity-and-lifecycle I0)

A node configured with an `[authority]` table takes its action evaluator, execution authority, provider enforcement
and evidence journal from the unit its `unit` names (`docs/features/declared-authority.md`). Without the table nothing
changes.

- **Who is affected:** a node that sets the table — its doors, which enforced nothing, now refuse what its unit's
  rules do not permit. The start is refused by name without `compliance`, without `[persistence]` (a grant's use
  markers and the epochs live there), with `skew_ms = 0`, with a journal path that cannot be opened, and with an
  evaluator, an execution authority or an evidence journal attached in code beside the table. **An older binary ignores the table
  silently** (configuration tables are tolerant), so upgrade every node before relying on it. A Rust caller that
  builds `GossipConfig` with an exhaustive struct literal: the new field breaks it; `..Default::default()` is
  unaffected.
- **The `governed-call` scaffold:** its `node.toml` now sets `gateway_identity_issuer = "governed-call"` and its
  unit names the treasurer `token:governed-call/treasurer` with a `grant.revoke` rule. A project scaffolded earlier
  keeps the bare `treasurer`, which no gateway principal matches: rename the actor to match your gateway's issuer.
- **What to do:** declare, in this node's unit, every rule its doors should admit — including the issuing rule
  (`grant.issue`, and `grant.revoke` if its issuer revokes) in every unit whose gateway may issue — with each actor as
  the door presents it (`token:<issuer>/<name>` for a named token, `node:<id>` for a node). Do not attach an evaluator
  in code as well: `start()` refuses one source of authority beside another.

## 84. A provider refuses a call whose mandate was revoked after its preflight and before its registration: *authority not established*, `mandate_lapsed_before_run` (2.40.0)

With provider enforcement on, a protected call presented under a mandate (a grant, or any scoped mandate the node's
execution authority verifies) is decided by the provider's preflight, then registered as running work so the
authority's sweep can stop it if the mandate lapses. The registration asks the authority again. Before this release, a
revocation (or an expiry, or a superseding epoch) that landed between the two made that second answer *not
established*, and the call **ran unregistered**: the node already knew the mandate was revoked, and no sweep could stop
the work. Now the call is refused.

- **What a caller sees:** an *authority not established* refusal, the family a revocation refused at the preflight is
  in, so both SDKs classify it the same way whichever side of the preflight the revocation landed (`authorityNotEstablished`
  / `authority_not_established`). On an MCP tool call, a JSON-RPC error with code **`-32031`** and
  `data: {"reason": "authority_not_established", "detail": "mandate_lapsed_before_run", "found": …}`; on a member's
  direct call of another kind or the SDK serve stream, a reply body `{"error": …, "reason": "authority_not_established",
  "data": {…the same…}}`. `detail` names this window; `found` names what the re-assessment found
  (`not established: … revoked`, `refused: …`). The handler does not run.
- **On the record:** the provider's evidence journal holds the preflight's permit (`execution: attempted`) and, for the
  same attempt, an execution record saying nothing ran (`execution: none`). The same record now follows a permit the
  cohort budget then refuses (`at_capacity`). The rule catalogue's outcome `mandate_lapsed_before_run`
  (`provider.enforcement`) is this wire `detail`, under the reason `authority_not_established`.
- **Not built:** two other refusals can follow a permit on the deferred-check path (a check settling a seat claim) and
  still leave the permit's `attempted` alone in the journal — `withdrawn` (the serve loop ended while the check was in
  flight, its task aborted) and the *authority not established* answer to a check whose task panicked. Neither has the
  permit in hand where the refusal is made.
- **Who is affected:** a caller whose mandate is revoked or expires within the moment between a provider's preflight
  and the handler. It used to get the tool's answer; it now gets this refusal. A grant presented this way is spent at
  the provider's door (its use marker was written by the preflight), as for any refusal after the claim. **Under a seat
  claim** (a fleet-consensus counter the evaluator names, post-360 row K2) the preflight's decision and the
  registration are a consensus round apart — the claim settles between them — so the window this refusal closes can be
  that long, and a revocation landing anywhere in it is refused here.
- **Not affected:** a call the policy admitted without an established mandate (none presented, or one the preflight
  found not established and the policy permitted anyway) — it still runs, unregistered, as before. The provider has no
  shadow rung, so there is no observe-only provider whose answer changes.
- **What to do:** treat it as *authority not established*: obtain a current mandate (ask for a new grant) and call
  again. Do not retry with the same mandate.
- **Counted:** `mycelium_provider_mandate_lapsed_total` (feature `metrics`).

## 85. A stem runs only the artifact kinds `[hosts].kinds` declares; `Provisioner` gained `restrict_kinds` (2.40.0, review of #736, defect A)

Before, every stem registered the WASM-component runtime whatever its unit said: `kinds` decided only whether the
blob runtime was added. A stem declaring `kinds = ["blob"]` therefore installed and ran a WASM component whenever a
`[[presence]]` floor or unmet demand named one and its catalogue line passed provenance. `mycelium wire-check` and the
unit-file reference already read such a stem as unable to host components; the stem now agrees with them.

- **Who is affected:** a stem whose `[hosts].kinds` omits `"wasm-component"` (including an empty or absent `kinds`)
  and which nevertheless hosted components. After the upgrade it refuses them. The refusal is reached only where the
  stem would have acted — below a `[[presence]]` floor, or with demand unmet — and is seen there in two ways: a
  `prov.eligible` record refused with reason `kind_not_declared`, when a decision trace is attached (`--trace-dir`);
  and `mycelium_artifact_ineligible_skips_total{reason="kind_not_declared"}` rising, in a build with `metrics`. A stem
  with an empty `kinds` hosts nothing and warns at start. A mesh-sourced stem no longer stages, or re-serves to peers,
  the bytes of a kind it does not declare. Bytes it staged before the upgrade stay under `<placement_root>/stage`,
  which nothing prunes, and are still served to peers: clear that directory once after upgrading a blob-only stem.
- **What to do:** add `"wasm-component"` to `[hosts].kinds` on every stem that is meant to host components; then
  `mycelium wire-check` also counts it as a host for them. Code that builds a `Provisioner` directly is unaffected
  unless it calls the new `Provisioner::restrict_kinds`.

The same release fixes a presence floor that any member could stall with a cheaper bad line (`prov.presence_floor`
rev 3). What it leaves open: a trusted publisher's line naming an artifact this host already hosts for something else
holds the floor at `already_hosted`, as does any member's such line on a host with `accept_unsigned = true`; and a
trusted line whose install always fails is chosen again every round (feeding an install's failure back into the choice
is not built).

## 86. A `mycelium-client://` certificate is refused at the mesh; `sys/client-cert/` and `POST /gateway/identity/client-cert` are new (2.40.0, identity-and-lifecycle I1)

A certificate naming `mycelium-client://{issuer}/{name}` is a **client certificate**. `mycelium tls issue --client
<name> --issuer <issuer>` issues one for a service's key. Both mesh verifiers now refuse one, from a dialling peer and
from a peer that serves it. Before, the fleet CA issued no such certificate, and a CA holder could mint one that a node
admitted as a mesh member. A mesh certificate whose names cannot be read is now refused too, where before it reached
the removal and revocation checks. Node certificates are unaffected, including a pre-row-Q certificate that names no
node.

- **Upgrade every node before issuing any client certificate.** A node running 2.39.0 or earlier reads a
  `mycelium-client://` certificate as a pre-row-Q certificate that names no node, and admits it at the mesh. Until
  the last node is upgraded, a service holding a client key can join the transport through that node.
- **Who is affected:** a token that holds `identity:write` for `POST /gateway/identity/revoke` gains the new door
  `POST /gateway/identity/client-cert` at the upgrade. The scope is an existing one. Reissue narrower tokens first if
  a revoking operator should not also publish client certificates.
- **Serials:** a client certificate's serial must carry its issuance in milliseconds, as `mycelium tls issue
  --client` writes it. One from other PKI tooling (a random 20-byte serial, as `openssl` writes) is refused
  `422 no_readable_issuance` at the door and ignored as a record. Certificates are ordered by the issuing host's
  clock, so a host whose clock runs behind can issue a certificate that orders below the one it replaces; the door
  then refuses it `409 superseded`.
- **The new namespace:** `sys/client-cert/{principal-hex}/{key-hex}` holds a client certificate's DER. An
  un-upgraded node stores and gossips the record and does nothing with it. A node judges a record by the certificate
  it holds, never by its write time. The raw KV routes refuse it `403 protected_key` and name the door.
- **Code:** `GossipAgent` gained `client_key_standing` and `publish_client_cert`, and the crate re-exports
  `issue_client_cert`, `client_identity`, `ClientIdentity`, `ClientKeyStanding` and `CLIENT_CERT_PREFIX` under `tls`.
  All are additions, so nothing breaks.
- **Under `[persistence]`:** a node writes `identity/client-certs.journal` beside `identity/revocations.journal`. A
  journal that cannot be opened refuses the start by name, as the revocation journal does.

Wire **v12** unchanged. What I1 leaves for later (`cs`/`cn`, the SDK clients, client-held grants) is listed on the
[feature page](../features/client-certificates.md).

## 87. A client's own signature on a call: `cs`/`cn` in `params._meta`; an unsigned call from a certificate holder is counted (2.40.0, identity-and-lifecycle I1, second half)

A service holding a client certificate can sign a `tools/call` or an A2A send with its key and send
`params._meta.cs` and `params._meta.cn`. The gateway carries both in the caller envelope, and the provider checks
them at its receive boundary.

- **Who is affected:** nothing changes for a call without `cs`/`cn`. Such a call from a principal whose client
  certificate is on record is counted and still served. It is counted at the `/mcp` `tools/call` door and at both
  `/a2a` doors (`gateway_actions_unsigned` on `/stats`, `mycelium_gateway_actions_unsigned_total{kind}` under
  `metrics`), and again at the provider that runs an `mcp.invoke` or `skill.invoke` (`provider_actions_unsigned`,
  `mycelium_provider_actions_unsigned_total`). The KV routes, the raw RPC routes and `ServiceHandle::gateway_rpc_call`
  count nothing. A later release will refuse an unsigned call; sign calls before then.
- **`_meta.cs` and `_meta.cn` are reserved.** An MCP or A2A client that already sends either key for its own purpose
  is now refused with `-32602` `request_malformed`, because both must parse as a client signature and travel
  together. The LLM doors bind no signature: `/gateway/llm/call` answers `400 client_signature_unbound` and
  `/gateway/llm/stream` an error event of that name, rather than dropping the signature.
- **The arguments' digest is RFC 8785 (JCS)**, in all three languages: ECMAScript's number and string forms, members
  sorted by UTF-16 code units. A signer that hashes another encoding is refused `client_signature_invalid` on any
  float written in exponent form. The mandate and grant `arguments_digest` is unchanged.
- **Large numbers:** a signed call whose arguments hold any number whose magnitude is 2^53 or more, float or not (e.g.
  `6.022e23`), is refused by every signer and by the provider (`client_signature_unsafe_integer`): send it as a string.
  The canonical form cannot bind such a number exactly, and the rule covers floats because JavaScript cannot tell
  `1e16` from `10000000000000000`.
- **Refusal codes:** `client_key_unknown` (no certificate for the key here yet; retry once its record arrives),
  `client_key_superseded` (final), `client_signature_invalid`, `client_signature_unbound`, `client_signature_malformed`,
  and `client_instances_exhausted` (more than 1,024 processes signing under one key inside the window; answered, not
  dropped). A replayed `cn` is dropped without an answer, and is counted (`client_signatures_replayed` on `/stats`).
- **A signed call needs `gateway_identity_issuer` set** on the gateway it goes through. Without it the gateway refuses
  the call with `-32602`, reason `gateway_identity_issuer_unset`.
- **Mixed versions:** an older provider keeps `cs`/`cn` among the envelope's unknown fields and checks nothing, so
  upgrade providers before relying on the check. An older gateway drops `_meta.cs`/`cn` and sends the call unsigned.
- **Code:** `CallerError` gained `ClientSignature` and `ClientReplayed`. It is `#[non_exhaustive]`, and a `_` arm must
  fail closed. `GatewayCaller` gained `client_signed`. `mycelium_core::tls` gained `RESERVED_CLIENT_NAMES`.
- **The publishing door checks its issuer:** `POST /gateway/identity/client-cert` now answers `409
  gateway_identity_issuer_unset` on a gateway without `gateway_identity_issuer`, and `422 issuer_mismatch` for a
  certificate issued for another issuer. Set the issuer on the gateway you publish through. The sealed audit record
  of a publication now names the publishing bearer (`by`) beside the certificate's principal.
- **Reserved name:** `mycelium tls issue --client legacy` is refused, and a certificate naming `legacy` is no client
  identity: `token:{issuer}/legacy` is the legacy `gateway_auth_token`'s principal.
- **Python:** signing needs `pip install 'mycelium-py[sign]'`, which adds `cryptography`.

Wire **v12** unchanged: `cs` and `cn` are payload.

## 88. Grants held by a client principal; `ExecutionAuthority::new`, `with_cohort_budget` and `with_membership_authorities` take a shared issuer set (2.40.0, identity-and-lifecycle I1, second half)

A service with a client certificate on record can now hold a grant. It asks with `POST /gateway/grants/request`
(`mcp:invoke`), the supervisor issues it at the existing door, and the service presents it in `_meta.mandate` with a
possession proof signed by its client key.

- **The `grant/` key names a client in hex.** A grant held by a node is keyed `grant/node:{id}/{request}` as before,
  and an existing record is read unchanged. One held by a client is keyed `grant/{hex(principal)}/{request}`, because
  the principal holds a `/`. A script that reads `grant/` keys and splits on `/` should decode a segment that does not
  start with `node:` as hex.
- **The decision and revocation doors take the principal too.** `POST /gateway/grants/{agent}/{request}` and its
  `/revoke` accept a node principal, a client principal (the pending row's `agent`, percent-encoded as the SDKs send
  it) or a client's hex segment. Nothing changes for a node-held grant.
- **A token holding `mcp:invoke` gains the request door.** Only a principal with a client certificate on record is
  served (`409 not_a_client_holder` otherwise). The door writes a pending request; nothing runs until a supervisor
  issues it. A request for the same tool and arguments while one stands answers that one (`"reused": true`); past 16
  standing requests per principal the door answers `429 too_many_pending` (a soft bound: concurrent requests can pass
  it). A request id written here names its gateway
  (`{hlc}-{node}`), so a script should not assume an id is 16 hex digits. Each request counts in
  `mycelium_grants_requested_total` and is sealed into the audit chain where the build keeps one.
- **An issued grant signs the resource it was evaluated against** as a fourth operation, `bound:resource:{resource}`
  (`Mandate::grant_resource`, `Mandate::grant_operations_on`), node-held grants included. A reader that lists a grant's
  operations sees one more entry, which no call can name. A revocation evaluates that resource — for a client,
  `tool:{tool}@{provider}`, the provider resolved when the request was written — so revoking needs no live provider. A
  grant issued before this, held by a node, keeps evaluating against the agent's node. The revoke door now also
  refuses (`409 unreadable_grant`) an `…/issued` record that no key this node has held signed (a grant issued before a
  `rotate_identity` is still revoked).
- **Code:** `ExecutionAuthority::new`, `GossipAgent::with_cohort_budget` and `GossipAgent::with_membership_authorities`
  take `impl Into<SharedExternalIssuers>`. A caller that passes a `TrustedExternalIssuers` keeps compiling.
  `TrustedExternalIssuers` gained `set_client_key(issuer, issued, key)` (it replaces a held key only with a newer
  issuance), `client_key` and `verify_client`. A clone of a `SharedExternalIssuers` is the same set. An attached
  execution authority's set is fed each client principal's current key. A client key verifies a grant holder's
  possession proof and nothing else: `verify_signed_by` reads configured keys only, so no mandate, checkpoint, cohort
  declaration, head, record or removal verifies under one.
- **The digest a grant binds** is still the mandate `arguments_digest`, which differs across languages for floats in
  exponent form. That defect is open as `identity-and-lifecycle.md` §9 O1.

Wire **v12** unchanged.

## 89. The SDKs sign as a service and present its own grants (2.40.0, identity-and-lifecycle I1, the SDK half)

Everything here is additive: no existing call changes what it sends unless it is given a signer.

- **New:** `McpClient` in both SDKs, a `tools/call` client for `/mcp` (`call_tool` / `callTool`, `list_tools` /
  `listTools`), with the grant verbs `request_grant` / `requestGrant`, `grant_status` / `grantStatus` and
  `wait_for_grant` / `waitForGrant`. Errors: `McpError` (a subclass of `A2aError`), `ActionRefusedError` for the
  evaluator's three refusals, `GrantNotIssuedError` for a request that ended without a grant.
- **A `signer` option** on `A2aClient` (Python `signer=`, TypeScript `{ signer }`) and `McpClient`: every call is then
  signed (`params._meta.cs`/`cn`). Its principal must be the one the bearer authenticates as, or the provider refuses
  the call.
- **The possession proof:** `ClientSigner.prove_possession` / `provePossession` and the functions under it
  (`mandate_canonical_bytes`, `mandate_digest`, `possession_message`; `mandateCanonicalBytes`, `mandateDigest`,
  `possessionMessage`). The SDK READMEs and `mandate_request_bytes`' docs no longer say the SDK does not sign.
- **TypeScript's two digests.** `argumentsDigest` is the RFC 8785 (JCS) digest a client signature binds; a possession
  proof binds the gateway's mandate digest, the new `mandateArgumentsDigest` (members by UTF-8 bytes, `undefined`
  members dropped as `JSON.stringify` drops them). Code that built a possession proof with `argumentsDigest` and
  `mandateRequestBytes` is refused for arguments with a key outside the Basic Multilingual Plane beside one in
  U+E000–U+FFFF; use `mandateArgumentsDigest` or `provePossession`. Python's `arguments_digest` was already the
  gateway's form.
- **TypeScript's `argumentsDigest` drops what the body drops.** An `undefined`, function or symbol member is left out
  (`null` inside an array) and `toJSON` is honoured, as `JSON.stringify` sends them. Before, `{gone: undefined}` was
  written into the signed digest but not the body, so the call was refused `client_signature_invalid`. A digest of
  arguments holding no such member is unchanged.
- **`wait_for_grant` / `waitForGrant`** bound each read by the time left, and end within their timeout.
- **Internal:** Python's `_task_params` and TypeScript's `@internal taskParams` take an optional signer last.
- **The digest a grant binds** is the gateway's mandate `arguments_digest`, which differs across languages for floats
  in exponent form (§88, `identity-and-lifecycle.md` §9 O1): send such a number as a string.

Wire **v12** unchanged; no Rust API change.


## 90. `mycelium-effects` depends on `rusqlite` 0.41 (2.40.0, RUSTSEC-2026-0459)

`rusqlite` 0.32.1 → **0.41.0**, which brings `libsqlite3-sys` 0.30.1 → **0.39.1** and the bundled SQLite 3.46.0 →
**3.54.1**, for RUSTSEC-2026-0459 (CVE-2026-119375). `rusqlite` is part of `mycelium-effects`' public API — a
`Handler` takes a `&rusqlite::Transaction<'_>` and `SqliteDestination::open` and `committed_count` return a
`rusqlite::Result` — so:

- **A crate that writes a `Handler`, or names a `rusqlite` type it gets from this crate, needs `rusqlite = "0.41"`
  itself.** A handler written against 0.32's `Transaction` does not type-check against 0.41's. The calls a handler
  makes (`execute`, `execute_batch`, `query_row`, `params!`) are unchanged across the two in this crate's own use.
- **Only one `libsqlite3-sys` may be in a build** (it declares `links = "sqlite3"`). A tree that also pulls an older
  `libsqlite3-sys` — through another `rusqlite`, `sqlx`'s SQLite driver or `diesel` — fails to resolve until that
  dependency moves to `libsqlite3-sys` 0.39 too.
- **`rusqlite` 0.41 declares Rust 1.89**, the workspace's `rust-version` already.
- **A database file is unchanged**: SQLite's file format is stable, and a destination written by 3.46.0 opens under
  3.54.1.

Wire **v12** unchanged; no other API change.

## 91. The gateway checks a client's signature before it spends a grant (2.40.0, identity-and-lifecycle I1, PR 4)

A call carrying `cs`/`cn` is now checked at the gateway's door as well as at the provider. At `/mcp`, `tasks/send` and
`tasks/sendSubscribe` the gateway verifies the signature over the call's binding and the key's standing **before** its
preflight.

- **A refused signature is answered by the gateway.** `/mcp` and `tasks/send` answer JSON-RPC `-32602` with
  `data.reason` set to `client_signature_invalid`, `client_key_unknown`, `client_key_superseded`,
  `client_signature_unsafe_integer` or `client_signature_malformed`. A stream answers a failed task naming the reason.
  Before, the provider refused, after the gateway had dispatched. A client that branched on the provider's answer
  reads the same reason in `data.reason`.
- **A grant presented on such a call is no longer spent.** The preflight that claims a grant's use does not run.
- **`cn` is still the provider's.** The gateway does not keep a replay window: a replayed `cn` passes the door and is
  dropped at the provider, as before. The door does check the issue time on the provider's window and answers `stale`.
- **At `/mcp`, a grant the provider refused before running is given back.** When the provider's tool serve loop
  answers `client_key_*`, `client_signature_*`, `stale` or `unknown_signer` (`-32022`), the gateway tombstones the grant's
  `…/used/{door}` marker and releases its claim, so the same grant admits a retry. Before, the retry was refused as
  already used. A bridged MCP server's reply in that shape is now rewritten into a tool error (`-32000`). The A2A doors
  and an embedder's `gateway_record_execution` give nothing back.
- **The client signature binds the whole resource.** The signed message carried the resource up to its first `@`; it
  now carries the bare resource whole, in Rust, Python and TypeScript (the cs vector is regenerated, with a case for a
  name holding `@`). A signer that passed a resource with the provider appended (`tool:x@node`) must pass the bare one,
  as `door_binding_mcp`, `mcp_binding` and `mcpBinding` return it. I1 was first released in 2.40.0, so no deployed signer was affected.
- **The provider's refusal carries its reason.** A tool's caller-context refusal (`-32022`) now has `data.reason`. At
  `/a2a` a provider's refusal is a JSON-RPC error (`-32003`, `data.reason`) on `tasks/send` and a failed task on a
  stream. Before, both answered a `completed` task with the refusal as its artifact. The evidence journal now records
  `failed` for it, where it recorded `completed`.
- **A gateway built with `tls` but started without `[tls]`** refuses a signed call as `client_signature_unverifiable`,
  where it answered `client_key_unknown`.
- **A tool or skill whose name holds `@`** now binds at the provider. The claim is read up to its last `@`; it was
  refused as `client_signature_unbound`.
- **A2A reads only the node's refusal bodies:** `rpc_rx`'s caller-context refusal and the provider-enforcement refusal.
  A skill's output that merely has `error` and `reason` stays its output.

Wire **v12** unchanged; no API change.

## 92. The published stem is built with `compliance`; `gateway_a2a` serves A2A from configuration; `GossipConfig` gained `gateway_a2a` (2.40.0)

Two changes to the published binaries, both additive.

- **The stem.** `mycelium-wasm-host` gains a `compliance` feature that turns on mycelium's (and so its `gateway` and
  `tls`), and the published `mycelium-stem` is built with `stem,gateway,llm,object_store,compliance`. A stem
  configuration with `gateway_named_tokens`, `gateway_scoped_tokens`, `[oidc]` or `[authority]` now starts, where a
  2.39.0 stem refused each by name; `[authority]` is attached by `start()` as on the `mycelium` binary. A stem whose only
  credential is `gateway_auth_token` behaves as before (that token holds every scope); the scope table is new, so a
  scoped or named token now gets only its scopes on the stem's routes (`artifact:publish` for
  `POST /gateway/artifacts/publish`). A stem built from source without `compliance` behaves as before.
  - **`tls` comes with it.** mycelium's `compliance` implies `tls`, so the published stem now takes `[tls]` and
    `[gateway_tls]` — it can join an mTLS fleet, where a 2.39.0 stem refused both tables at start.
  - **An open stem gateway narrows what it grants.** With no credential model, the gateway admits an anonymous request
    as before, but the authority it records and carries for it (`ResolvedPrincipal`, the caller context a provider
    receives) is the route's scope under `compliance`, where it was everything (`*`) without it
    (`http/authz.rs` `open_gateway_scopes`). A provider or evaluator that reads the caller's scopes now sees only the
    route's; configure a credential where something downstream needs more.
  - **`GOSSIP_GATEWAY_A2A=1` in a shared environment stops the stem.** The published stem has no `a2a`, so a
    `gateway_a2a` it reads from the environment (or its file) refuses its start by name. Set the variable on the
    `mycelium` node's environment only, not in one an image or a compose file shares with the stem.
- **The node.** `gateway_a2a = true` (or `GOSSIP_GATEWAY_A2A=1`) makes `start()` mount the AgentCard and `/a2a`, as
  `with_a2a()` does. Off by default, so no node serves anything new unless it sets it. Refused by `validate()` without
  an `http_port`, and by `start()` in a build without `a2a` (`InvalidField { field: "gateway_a2a" }`); the
  environment value is strict (`1/true/TRUE/yes`, `0/false/FALSE/no`). `/a2a` admits an anonymous caller — a token
  does not gate it — so set `[authority]` with rules that deny anonymous principals, or the node warns at start and
  `a2a.admission` reads `not_configured` (`secure-single-domain` then refuses it). **An older binary ignores the key
  silently**, as `GossipConfig`'s other keys are read: check `curl http://<gateway>/.well-known/agent-card.json` after
  rolling it out.
- **`with_a2a()` mounts once.** A second call, or a call beside `gateway_a2a`, now mounts nothing more; before, a
  second call panicked on overlapping routes.
- **Compiler:** `GossipConfig` gained a public field, so an exhaustive struct literal breaks; `..Default::default()`
  is unaffected.

Wire **v12** unchanged.

## 93. A mandate's arguments digest is RFC 8785, with a one-release window (2.40.0, identity-and-lifecycle I3, §9 O1)

§92 is #749's. The digest a grant binds (`bound:args:`) and a possession proof covers is now RFC 8785 (JCS) in Rust,
Python and TypeScript. It was each language's own JSON encoding, which disagreed on floats in exponent form, so an
honest Python or TypeScript holder could be refused.

- **What changed:**
  - `arguments_digest` (Rust, Python) and `mandateArgumentsDigest` (TypeScript) compute the RFC 8785 digest. It is the
    same digest a client signature binds.
  - The preflight's evidence records it (`arguments_digest`).
  - A grant requested after the upgrade binds it.
  - Rust now writes a number's digits as ECMAScript does, a tie going to the even digit: `1000000000000000.25` is
    `…000.2`, where Rust wrote `…000.3` and Python and TypeScript `…000.2`. This also changes the canonical form a
    client signature (`cs`) covers, for those numbers only: a Rust signer's `cs` over such a number verified at a
    Python or TypeScript peer only after this change, and an un-upgraded Rust verifier refuses an upgraded signer's.
- **The window, one release:** the older form (`serde_json`'s encoding, now `legacy_arguments_digest`) is still
  honoured, so a grant issued, a proof made or a request asked before the upgrade keeps working.
  - A grant may be bound to either form and its proof made over either, in any pairing. Both digests are computed
    from the call's own arguments, so either binds exactly them.
  - A standing request recorded with the older form is reused by the request door and by the agent's wait, so asking
    again does not write a second request.
  - A pre-I3 grant now also admits the integer/float twin of its arguments. RFC 8785 writes `5` and `5.0` the same
    (`5`), and so does `serde_json` for an integer. So the RFC 8785 digest of `{"n":5.0}` equals the older digest of
    `{"n":5}`, and a grant bound to the latter admits the former. Under RFC 8785 the twin is the same call anyway, so
    this matters only to a tool that tells `5` from `5.0`.
  - Each match is counted at the point that assessed it. `GET /stats` has three counters:
    - `legacy_digest_matches`, for this gateway's doors;
    - `provider_legacy_digest_matches`, for this node as a provider;
    - `agent_legacy_digest_matches`, for this node's agent verifying a grant its own wait read.

    With `metrics` there is also `mycelium_legacy_digest_matches_total{point="gateway"|"provider"|"agent"}`. A call
    assessed at more than one point counts once at each. The agent's point is there because a node-held grant goes to
    its provider directly, and a provider without `with_provider_enforcement` never assesses it. Without that point,
    such a grant could run while both other counters stayed at zero.
  - The counters are in memory and start from zero when a node restarts. The evidence record's `checked` names every
    match at a gateway door or a provider (`action_evaluator::LEGACY_DIGEST_CHECKED`), and it is the durable record.
    Read the journal to find a match from before a restart. The agent's wait writes no evidence record, so its matches
    are on its counter only.
  - Watch the counters fall to zero. The release after refuses the older form (the identity plan's §9 O5).
  - A grant is short-lived (at most an hour), so re-issue any older grant that still stands.
- **Upgrade order: verifiers before binders.**
  - Verifiers first: every gateway and every provider, meaning any node that assesses a presented mandate.
  - Binders after: the agents and SDK clients that ask for grants and make possession proofs.
  - Why this order: an un-upgraded verifier computes only the older digest. Where the two forms differ (non-ASCII
    keys outside the Basic Multilingual Plane, some floats), it refuses an upgraded binder's grant or proof as
    `BoundToOtherArguments` or as a possession mismatch. Where the forms agree, which is most arguments, nothing
    changes in either order.
  - An upgraded verifier accepts both forms, so with the verifiers upgraded first, no honest call is refused.
- **Large numbers are refused where a mandate binds them, deliberately** (the maintainer's decision, 2026-11-09). This
  departs from plain RFC 8785, which serialises any finite double (`1e21` as `1e+21`): the digest refuses such a number
  rather than bind a value that a reader may hold as a different integer. Any number of magnitude 2^53 or more, integer
  or not (`1e21` included), is refused:
  - by the request door, `400 unsafe_integer`;
  - by the agent's grant wait, `GrantRefused`;
  - at the gateway and the provider, `MandateRefusal::UnsafeInteger`, a new variant (the enum is `#[non_exhaustive]`).

  The SDKs' digest functions raise. Send such a number as a string. Each door names the refusal its own way:
  `unsafe_integer` is the request door's code (`POST /gateway/grants/request`) and appears nowhere else.
- **Evidence for a call without a mandate:** such a call is not refused for a large number. Its evidence record's
  `arguments_digest` is now the RFC 8785 digest, with two consequences:
  - For a number of magnitude 2^53 or more, it identifies the arguments only up to the nearest double.
  - Where the two forms differ, it is not the digest a pre-upgrade record holds for the same arguments, so an
    exporter joining records across the upgrade by digest does not match them.
  
  The older form is not recorded beside it.
- **Code:**
  - `ExecutionAuthority::assess_arguments`, `legacy_digest_matches`, `legacy_digest_matches_at_provider`,
    `legacy_digest_matches_at_agent`, `legacy_arguments_digest`, `DigestForm` and `MandateBinding::legacy_digest` are
    new. `MandateBinding` is `#[non_exhaustive]`.
  - `assess` takes one digest and is unchanged.
  - `MandateRefusal` gained `UnsafeInteger`.
  - The possession vector's `mandate_digest_cases` are now `legacy_digest_cases`, beside new `digest_cases`,
    `unsafe_cases` and a `float_grant`. Both vectors gained a tie case, and `tests/fixtures/cs/es_numbers.json` holds
    606 doubles as ECMAScript writes them.
  - `ryu` (already in the tree through `serde_json`) is now a direct dependency.

Wire **v12** unchanged.

## 94. `gateway_a2a_default_skill` names the skill a message without one reaches; `GossipConfig` gained `gateway_a2a_default_skill` (2.40.0)

`gateway_a2a_default_skill = "ns/name"` (or `GOSSIP_GATEWAY_A2A_DEFAULT_SKILL=ns/name`) is what
`with_a2a_default_skill(ns, name)` is in code: an `/a2a` message that names no `metadata.skillId` — what an unmodified
`a2a-sdk` client sends — reaches that skill, where a node served by `gateway_a2a` refused it `-32602`. A message that
names a skill still reaches that skill, and the AgentCard's description states the default. Unset by default, so no
node changes behaviour unless it sets it. The default is a name, not a grant: the call runs the same doors as a named
skill (the evaluator, mandates, evidence).

- **Refused by name at start** (`InvalidField { field: "gateway_a2a_default_skill" }`, from `preflight`, so `mycelium
  doctor` reports it too): where `/a2a` is not served — neither `gateway_a2a = true` nor `with_a2a()` — since it would
  be accepted and reach nothing; a value whose `ns` or `name` is not one key segment (empty, holding a `/`, or the
  reserved `locality/self` — `capability_name_problem`) or has leading or trailing whitespace (a file value is not
  trimmed: `"depot/dispatch "` would start and then match no skill) — the rule `with_a2a_default_skill` applies; a different default
  already set in code by `with_a2a_default_skill` (the same one is accepted — one default per node, and the code's
  would otherwise silently win); and any value in a build without `a2a`. The published `mycelium-stem` is built without
  `a2a` by decision, so it refuses the key: keep `GOSSIP_GATEWAY_A2A_DEFAULT_SKILL`, like `GOSSIP_GATEWAY_A2A`, out of
  an environment the stem shares.
- **The environment value** is trimmed, and a blank one is refused rather than read as unset.
- **`with_a2a_default_skill` refuses one more name:** a `namespace` or `name` with leading or trailing whitespace is
  now ignored with a warning, as its other bad names are (before, it was set and then matched no skill). A second call
  is still ignored with a warning.
- **An older binary ignores the key silently**: after rolling it out, check that
  `curl http://<gateway>/.well-known/agent-card.json` describes the default (`… goes to ns/name`).
- **Compiler:** `GossipConfig` gained a public field, so an exhaustive struct literal breaks; `..Default::default()`
  is unaffected.

## 95. Grant decisions may be signed and relayed under `grant:relay`; an unsigned decision is counted (2.40.0, identity-and-lifecycle I3, PR 2)

§94 is #751's. A supervisor holding a client certificate can sign its answer to a grant request, and the node records
the signer as the decision's actor. A client carrying a signed decision needs only the new `grant:relay` scope.

- **What changed:**
  - `POST /gateway/grants/{agent}/{request}` and `.../revoke` admit `grant:issue` on the bearer **or** `grant:relay`.
    A token holding neither is refused `403 insufficient scope`, whose body now also names `or_scope`.
  - A body may carry `signed`. When it does, the decider is the actor: the evaluator decides with it, and the evidence
    record's `subject` names it, and its new `signed_decision` field keeps the signed body, the terms and
    `relayed_by`. The grant's `purpose` names the decider and the relay. A refusal's or revocation's record gains
    `signed` (the body) and `relayed_by`.
  - One decision per request at a time on a node: a concurrent copy is answered `409 decision_in_flight`, then
    `already_answered` or `already_revoked`. Before this, concurrent copies of one decision were each answered.
    An issue that would write a checkpoint below one already written is refused `409 already_revoked` before it
    offers or writes any checkpoint, and its `Decided` record is followed by a `None` execution: nothing ran. Every
    other return after a permitted decision records an execution too.
  - A signed decision is fresh for `CALLER_CONTEXT_MAX_AGE_MS` (300 s) plus `max_clock_drift_ms` into the past, and
    `max_clock_drift_ms` into the future: 600 s and 300 s at the default drift.
  - New refusals: `403 not_the_decider`, `403 decision_retargeted`, `409 not_addressed_here`, `403 stale`,
    `403 decider_lacks_scope`, `403 decision_unsigned`, `400 decision_signature_malformed`, `403 decision_unverifiable`,
    `409 gateway_identity_issuer_unset` and `409 decision_in_flight`.
  - `AeEvidence` gained `signed_decision` (`#[non_exhaustive]`, so only a struct literal elsewhere would break).
- **Unsigned decisions, counted now and refused later:** an unsigned body under `grant:issue` is still decided as the
  bearer's own. It is counted once answered (issued, refused or revoked) on `GET /stats` as
  `grant_decisions_unsigned`, and as `mycelium_gateway_decisions_unsigned_total{decision}`.
  - Watch the counter, and give each supervisor a client certificate (`mycelium tls issue --client`) and a signing
    SDK (`signer=` / `{ signer }`) before the refusal release.
  - The refusal is tracked as the identity plan's §9 O6.
- **Who may decide:** a signed decision's decider must hold `grant:issue` at the node it is addressed to, as a named
  token (`gateway_named_tokens`) of that gateway's issuer. A decider known only through OIDC or a positional token
  cannot sign a decision here.
- **Rolling upgrades:** an un-upgraded gateway refuses `grant:relay` at the route (`insufficient scope`), so a relay
  admits nothing there. A signed body sent with a `grant:issue` bearer to an un-upgraded gateway has its `signed` field
  ignored, and is decided as the bearer's. Upgrade the gateways before supervisors start relaying.
- **Code:**
  - `SignedDecision`, `grant_decision_message`, `sign_grant_decision`, `GRANT_DECISION_TAG` and
    `ClientSigner::sign_grant_decision` are new.
  - Python: `grant_decision_message` and `ClientSigner.sign_grant_decision`, plus `signer=` / `node=` on
    `grant_decide` and `grant_revoke`.
  - TypeScript: `grantDecisionMessage`, `ClientSigner.signGrantDecision` and the `SignedDecision` type, plus
    `{ signer, node }` on `grantDecide` and `grantRevoke`.
  - `tests/fixtures/decision/vector.json` pins the message.

Wire **v12** unchanged.

## 96. A provider can verify an OIDC user's exchanged token; one that does refuses an `oidc:` action without it (2.40.0, identity-and-lifecycle I5, PR 1)

A provider verifies an OIDC user's token itself when its `[oidc]` table sets `exchange_audience`. The token is
exchanged by the gateway at the IdP (RFC 8693) and bound to the gateway's node key (DPoP). This section is the
provider's half; the gateway's exchange, in the same release, is §97.

- **What changed:**
  - `[oidc]` gained `exchange_audience`, `exchange_client_id` and `exchange_max_lifetime_secs`. **`OidcConfig` now
    implements `Default`**, so a literal that names every field breaks once; write
    `OidcConfig { issuer, audience, group_scopes, ..Default::default() }` and later fields will not break it. The
    defaults are a TOML table's: `group_claim = "groups"`, no group scopes, discovery for the JWKS, no exchange.
  - **The trigger is `exchange_audience`.** An `[oidc]` table without it is unchanged, and so is a node without one.
  - With it, `validate()` refuses a missing `exchange_client_id` (`oidc_exchange_client_unset`), a lifetime outside
    `1..=3600`, a node without `[tls]` (`oidc_exchange_needs_tls`: the token is bound to the node key that signs the
    envelope) and an `exchange_audience` equal to `audience` (`oidc_exchange_audience_is_gateway_audience`: a user's
    own gateway-audience token, forwarded unexchanged, would pass the audience check).
  - A node that is both gateway and provider has **one** OIDC verifier: the gateway validates bearers with the keys
    the provider fetched at start, and fetches nothing of its own.
  - A gateway bearer's `exp` is now read against the node's clock seam, with the same 60 s leeway, rather than inside
    `jsonwebtoken`. An exchanged token's `iat`, `nbf` and `exp` are read at the instant its replay window prunes at,
    under the leeway of `max_clock_drift_ms`.
  - The caller envelope gained two optional fields, `ot` (the exchanged token) and `so` (the gateway's signature over
    it). A provider that predates them keeps them in its unknown fields and reads nothing.
  - A verifying provider refuses an envelope's `ot` by name at each check, and refuses an `oidc:` principal's action
    without one (`ot_required`). The feature page lists every refusal.
  - **A build without `compliance` refuses any call carrying an `ot`** (`not_in_build`) rather than run it
    unverified. Before this release no envelope carried one.
- **A verifying provider needs exchanging gateways** (§97). A provider that sets `exchange_audience` refuses every
  `oidc:` action that arrives without an `ot`, so set it only once every gateway that calls it for OIDC users runs
  2.40.0 or later and has its exchange settings. (No gateway before 2.40.0 sends an `ot`.)
- **The actions** are the protected kinds (`mcp.invoke`, `skill.invoke`, `llm.invoke`, `protected_rpc_kinds`), the
  companions' write kinds, and a hosted capability's invoke (`cap.invoke/{namespace}/{name}`, which runs code). The
  artifact kinds and `reason.blob.fetch` are not.
- **An older binary ignores `exchange_audience` silently**, because `OidcConfig` accepts unknown keys. A provider
  configured to verify before it is upgraded enforces nothing.
- **The replay window is in memory** and is empty after a restart. It is one window per provider, keyed by the
  token's `(iss, jti)` whichever gateway carries it. It holds at most 262 144 token ids, 65 536 of them per gateway
  and 16 384 per principal, and refuses past any bound (`ot_provider_window_full`, `ot_window_full`,
  `ot_principal_window_full`). That is a ceiling of about 109 OIDC calls a second per gateway, 436 per provider and
  27 per principal (counted across every gateway it calls through), at the default lifetime and drift. A gateway that
  leaves frees its share as its entries expire. **Four gateways at their share fill the provider's window**, and every
  other gateway's OIDC users are then refused `ot_provider_window_full` until entries expire; the gateways' rate limits
  are the bound against that.
- **Code:**
  - `CallerError` gained `ExchangedToken(&'static str)` (it is `#[non_exhaustive]`).
  - `GatewayCaller` gained a `pub(crate)` field.

Wire **v12** unchanged.

## 97. A gateway exchanges an OIDC user's token for every action it dispatches (2.40.0, identity-and-lifecycle I5, PR 2)

A node whose `[oidc]` table sets `exchange_token_endpoint` exchanges the user's own bearer at the IdP (RFC 8693) under
its client credential, with a DPoP proof under its node key, and carries the exchanged token to the provider (§96). It
does so only for an **action** — the kinds a verifying provider requires an `ot` for — at `/mcp` and `/a2a`, and makes
no exchange for any other call, nor for a principal that is not `oidc:`.

- **What changed:**
  - `[oidc]` gained `exchange_token_endpoint` and `exchange_client_secret_env` (default
    `MYCELIUM_OIDC_EXCHANGE_SECRET`). `OidcConfig { …, ..Default::default() }` is unaffected.
  - **A node exchanges iff `exchange_token_endpoint` is set, and verifies iff `exchange_audience` is set** (the
    maintainer's decision, 2026-11-11). Whether it runs an HTTP port decides neither. A verifying node without the
    endpoint — a provider with only `/health` or `/metrics`, or a gateway that also provides — starts as under §96, with
    no secret; such a gateway's OIDC users' actions toward a verifying provider are refused there `ot_required`.
  - **With the endpoint set, the start is refused by name** when the audience is unset (`oidc_exchange_audience_unset`),
    the client unset (`oidc_exchange_client_unset`), the endpoint is neither https nor a loopback host
    (`oidc_exchange_endpoint_insecure`), the secret's variable is unset or empty (`oidc_exchange_secret_unset`), or the
    egress allow-list does not permit the endpoint. Under `gateway_caller_profile = legacy` the settings are inert and
    not demanded; the start warns.
  - **An exchange that yields no token refuses the call**, and nothing runs: `oidc_exchange_unreachable` (no answer, or
    a redirect, which is never followed), `oidc_exchange_refused`, `oidc_exchange_too_large` (past 64 KiB),
    `oidc_exchange_malformed`, `oidc_exchange_no_subject_token`, `oidc_exchange_no_identity` (JSON-RPC `-32024`; HTTP
    502 at the raw route). A grant's use the door claimed is given back, as it now is for the gateway's other refusals
    before sending (`caller_context_too_large`, `provider_without_caller_context`). An IdP outage therefore stops OIDC
    users' actions through that gateway; their reads continue.
  - Each action is one exchange: one IdP round trip per call, within a 10 s deadline.
  - §96's start-time warning is gone.
  - `mycelium doctor` reports `id.oidc_exchange` (this node exchanges) and `id.oidc_exchanged_token` (it verifies).
  - Discovery documents and JWKS bodies past 1 MiB are a failed fetch.
- **Traps:**
  - **"Action" is read per node.** A kind a provider lists in `protected_rpc_kinds` and its gateway does not is not
    exchanged for, and is refused `ot_required`. List a protected kind on every node that dispatches or serves it.
  - **A provider built without `compliance` refuses any `ot`** (`not_in_build`, §96). During a rolling upgrade, an
    exchanging gateway's actions to such a provider are refused, where before they ran; upgrade the providers, or set
    the endpoint only once they run a `compliance` build.
- **Code:** `GatewayDispatchError` gained `Exchange(&'static str)` (it is `#[non_exhaustive]`).
- **Clients do not change.** The SDKs send the same bearer.

Wire **v12** unchanged.

## 98. The reference evaluator decides the value rules it used to leave `Indeterminate`; a prohibition naming a value no longer refuses a differing one (2.40.0, identity-and-lifecycle I0, #733)

`ReferenceEvaluator` now names every `requires_values` key of its rules in `security_relevant_arguments`. An enforcement
point lifts exactly those arguments into the action envelope. Before, the trait's default selected none, so at every
live door — `/mcp`, `/a2a`, the provider's enforcement and `[authority]` (§83) — a rule that tests a value read
*argument value not available* and decided `Indeterminate`. That failed closed, and the rule never permitted.

- **What changed:**
  - **An allowance with `requires_values` now permits** when the caller's top-level `arguments` carry the values it
    names. Before, it never did at a live door.
  - **A prohibition naming a value decides on it.** If the value is present and equal, the call is denied: a
    definite `Deny` where a live door recorded `Indeterminate` before (refused either way, but the evidence record's
    verdict changes). If it is absent, the value is not established and the call decides `Indeterminate`, failing closed (as
    before). If it is **present and different** — `"Capital"` against a prohibited `"capital"`, `over_ceiling = false`
    against `over_ceiling = true` — the prohibition **does not apply** and the allowances decide. Before, every request
    the prohibition's actor, operation and resource selected was `Indeterminate`, whatever its value.
- **Who is affected:** a deployment whose reference-evaluator rules (built in code, or from a unit's `[[rule]]`s under
  `[authority]`) carry `requires_values`, and that relied — knowingly or not — on such a rule never permitting, or on a
  value-naming prohibition refusing every request it selected. An evaluator other than `ReferenceEvaluator` (the
  Cedar adapter, your own) is unaffected unless it relied on the default selection. A caller that builds an
  `ActionEnvelope` itself and calls `evaluate` directly saw the values already and is unaffected.
- **What to check before upgrading:**
  - List every rule with `requires_values`. For each **allowance**, confirm you want it to permit when its values
    match — it now will.
  - For each **prohibition** on a value, decide what a *different* value should do. If any value other than an
    allowed one must be refused, express that as an allowance on the allowed values (a request no allowance matches
    is not established — `Indeterminate`, refused at the door), not as a prohibition on one value.
  - **A rule is only as strong as the argument it names.** The values are the caller's own top-level `arguments`.
    A rule on a value the effect itself uses (a ledger's amount) holds; a prohibition on a flag the caller asserts
    (`over_ceiling`) holds nothing unless the caller cannot set it.
  - In the evidence journal, decisions recorded `Indeterminate` for *argument value not available* on these rules
    before the upgrade are the ones that now decide; compare a sample after upgrading.
- **Code:** no signature changed; `ReferenceEvaluator` implements `ActionEvaluator::security_relevant_arguments`
  (`src/agent/action_evaluator.rs`). Tests: `a_prohibition_whose_value_is_present_and_equal_denies`,
  `a_prohibition_whose_value_is_absent_is_indeterminate`,
  `a_prohibition_whose_value_is_present_and_different_does_not_apply`, and
  `the_gateway_door_decides_by_this_units_rules_and_records_each_decision`.

Wire **v12** unchanged.

## 99. The raw KV routes read application keys only; the listing leaves ten namespaces' names out (unreleased, the kv:read decision of 2026-11-12)

§19 closed the raw KV **write** routes to every namespace the substrate or a companion owns. The **read** routes
stayed open: a token holding only `kv:read` read a grant record without `grant:read`, the audit chain without
`audit:read`, a mailbox entry, a client certificate's record and every other owned key, and `GET
/gateway/overlay/consistent/get` did the same under `consensus:read` through its raw-key fallback. Every release
from 2.27.0 is affected. The read routes now follow the same table as the write routes (`OWNED_KV_PREFIXES`,
`src/agent/http/authz.rs`).

- **What changed:**
  - **`GET /gateway/kv?key=K`** answers **403** `protected_key` for a key in an owned namespace, whatever the token's
    scopes (`"*"` included) and whether or not the key exists. The `message` names the route that reads the namespace
    where it has one. An application key — `ckpt/`, `ckptw/`, `strands/`, `manifest/`, `schemas/`, `ui/`,
    `agent/{node}/provision/…`, and every key outside the table — is read as before.
  - **`GET /gateway/overlay/consistent/get?key=K`** answers the same 403 for an owned key.
  - **`GET /gateway/kv/keys`** leaves out the key **names** of ten namespaces: `grant/`, `mailbox/`, `audit/`,
    `sys/audit/`, `sys/audit-checkpoint/`, `sys/client-cert/`, `mandate/`, `rights/`, `sched/` and `call/` (§100).   An unprefixed listing, or a prefix that spans one of
    them (`sys/`), omits them; a `prefix` inside one of them (`grant/`, `sys/audit/n1`) is **403** `protected_key`.
    Every other owned namespace's names are still listed — `grp/`, `sys/identity`, `sys/caller-context/`, `cap/`,
    `skills/`, `consensus/` and the rest — so the Ops Console's panels and a readiness probe on a prefix keep
    working (§101 adds the names of a component's log streams, under `log/` and `clog/`, to what is left out).
    Names only: the values of those keys are refused as above. The ten are the namespaces whose key
    names are themselves the record: a grant's agent and request, a mailbox's recipient, the audit chain's length
    and its checkpoints' positions, a certificate's principal, a mandate's or a right's holder, a gateway
    client's principal in `sched/{id}~{principal}/…`, and a call's gateway and term in `call/{gateway}/{term}/…`. **A limit:** `/stats` is public and its `store_entries` is
    the count of every live key, hidden ones included, so a watcher can still see that *some* record was added.
  - **Three reads moved to routes of their own.** A schedule's last run: `GET /gateway/schedules/last?id=ID`
    (`schedule:run`), which answers the caller's own schedule's entries. The topology overrides in force: the
    `topology_overrides` array on `GET /gateway/govern` (`govern:read`). The `skillrunner` dashboard's invocation
    panel reads `GET /gateway/audit` (`audit:read`) in a `compliance` build, and without `compliance` the
    binary's own `GET /gateway/skillrunner/invocations`, which reads its unsigned `audit/` trail in process.
  - **What the two new reads are worth.** `sched/{id}/last/{node}` and `sys/topology-override/{group}` are unsigned
    records any admitted member can write or tombstone
    ([owner-signed records](../wiki/dev/owner-signed-records.md)), and both routes report them as the schedule loop
    and the consensus gate read them: what the node will act on, not proof that a slot ran or that an operator set
    an override through the audited route. `schedules/last` returns every parsable entry — it does not apply the
    loop's rules, which count only entries under the declaration's period and phase and leave out any more than
    64 slots ahead of the node's clock (`MAX_AHEAD_SLOTS`).
- **Will the compiler tell me?** No — it is an HTTP status. The SDKs raise it typed, as they do for writes:
  `ProtectedKeyError` from `get`, `consistent_get` / `consistentGet`, `scan_prefix` / `scanPrefix`, and from `keys`
  for a prefix inside a hidden namespace (`mycelium-py` 0.2.16, where it is still the `httpx.HTTPStatusError` these
  verbs raised; `mycelium-ts` 0.2.10, where these verbs threw a plain `Error` naming the status). `scan_prefix` over
  a listed owned namespace (`cap/`) raises on the first value.
- **Who is affected:** a client, script or dashboard that reads an owned namespace through `/gateway/kv` — a value,
  or the names of one of the ten. A client that reads its own keys is not.
- **Migration.** Read through the route that owns the namespace:

  | Namespace | Read it through | Scope |
  |---|---|---|
  | `grant/` | `GET /gateway/grants/pending`, `GET /gateway/grants/request/{request}`, `GET /gateway/grants/stream` | `grant:read` |
  | `sys/audit/` | `GET /gateway/audit` | `audit:read` |
  | `sys/govern/`, `sys/topology-override/` | `GET /gateway/govern` | `govern:read` |
  | `grp/` | `GET /gateway/mesh/group?group=G` | `mesh:read` |
  | `cap/` | `GET /gateway/capability/resolve`, `GET /gateway/fleet` | `cap:read`, `fleet:read` |
  | `prompts/` | `GET /gateway/prompts`, `GET /gateway/prompts/{ns}/{name}` | `llm:read` |
  | `log/` | `GET /gateway/overlay/log/scan` — an application stream; a component's stream (`cn/`, `wiki/`, `reason/`) by §101 | `consensus:read` |
  | `mailbox/` | `GET /gateway/mailbox/{kind}` (the recipient's stream) | `mesh:read` |
  | `consensus/` | `GET /consensus/{slot}` — an application's slot, a lock's or a leader's; a component's slot is refused there too (§102) | `consensus:read` |
  | `sched/` | `GET /gateway/schedules/last?id=ID` — a gateway client's own schedule only; an application's `run_schedule` records (`sched/{id}/…`, no `~`) have no HTTP read | `schedule:run` |

  A namespace with no gateway read route — `sys/client-cert/`, `mandate/`, `rights/`, `call/` (§100), `knowledge/`, `tuple/` and
  the other companions' — is read in process, through the handle of the component that owns it. `audit/` is the
  `skillrunner` binary's unsigned fallback trail in a build without `compliance`; that binary serves its count and
  ten newest records at `GET /gateway/skillrunner/invocations`, and nothing else serves it over HTTP.
- **Check after upgrading.** With a `kv:read` token, both of these answered 200 and now answer 403 `protected_key`:

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer KV_READ_TOKEN' \
       'http://NODE:PORT/gateway/kv?key=sys/audit/x'              # 200 before, 403 after
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer KV_READ_TOKEN' \
       'http://NODE:PORT/gateway/kv/keys?prefix=grant/'           # 200 before, 403 after
  ```

  Until every gateway is upgraded, treat `kv:read` as a scope that reads every replicated record, and review who
  holds it.
- **Not changed:** the log read routes (`/gateway/overlay/log/scan`, `/log/subscribe`, `/log/group/subscribe`) still
  read a stream under `cn/`, `wiki/` or `reason/` with `consensus:read`, though §19 refuses writes to them — a
  follow-up, **closed by §101**. Layer I is untouched: a peer still replicates every key (detection, not
  prevention); these are the gateway's doors.

## 100. The execution authority verifies a `call:` mandate; a `call:` mandate in the first slot is never established; the `call/` key family is owned (2.41.0, identity-and-lifecycle I4, PR 1a)

A **`call:` mandate** is a gateway's cancellation handle for one call it dispatches: scope `call:{term}`, issued by the
gateway to itself, carrying its first revocation checkpoint beside it. This release builds the **authority's half**:
`ExecutionAuthority::assess_call` verifies one and holds its scope until its window ends, and work registered under
it (`begin`) is cancelled by the sweep when its term is revoked. **Nothing produces or reads one at a live door yet**:
no gateway mints one, no provider reads one from a call, and no door cancels one. Those arrive in the following
releases of this row, so a node changes behaviour only in the ways listed here. A `call:` mandate is verified against
its gateway's member key, which a node holds only in a build with **`compliance`**; without it every `call:` mandate
reads `Unknown` (it fails closed).

- **Behaviour change — the first slot.** A mandate whose scope begins `call:` presented where a caller's mandate goes
  (`params._meta.mandate`, the envelope's `m`, `ExecutionAuthority::assess` / `assess_arguments`) reads `Unknown`,
  *"a call: mandate is not a caller's"*, whoever presents it and whatever the entitlement table says. And **no
  entitlement table entitles a `call:` scope**: `EntitlementTable::is_entitled` answers `false` for one, named or
  not, so every `GrantVerifier::check` caller — a capability's backing mandate (`mandate::protected`), an agent
  verifying a grant it waited for — reads `NotEntitled`. **Who is affected:** a deployment that named a scope
  beginning `call:` for its own mandates and entitled an authority for it. Such a mandate was verified as any other;
  it is now established nowhere. **Check:** `grep` your entitlements and unit files (`[[mandate]]`,
  `requires_mandate`) for a scope beginning `call:` and rename it.
- **Behaviour change — the `call/` key family is owned.** The raw KV routes — the writes (`POST` and `DELETE
  /gateway/kv`, the quorum write, the overlay's consistent set) and, with §99, the reads — answer a key under `call/`
  `403 protected_key`, and its names are hidden from the listing. The family holds a
  `call:` mandate's revocation checkpoints, which only the gateway that issued the mandate writes; a write there
  could hide a call's cancellation from the provider running it. **Who is affected:** an application that keeps its
  own keys under `call/` through the gateway's KV routes. **Check, before upgrading**, on the release you run
  now (2.40.0 or earlier): `GET /gateway/kv/keys?prefix=call/`, and move such keys to another prefix. From this
  release the routes neither write nor read a key under `call/`, and the listing leaves its names out and answers
  that prefix `403 protected_key` (§99), so the check cannot be made after the upgrade.
- **Behaviour change — `offer_checkpoint`.** A revocation checkpoint whose scope begins `call:` is kept only while the
  authority holds that scope's window (it admitted a call there, through `assess_call`). Otherwise it answers
  `CheckpointOffer::NotHeld` and keeps nothing. A checkpoint for any other scope is unaffected.
- **Behaviour change — the start rule.** A `call:` checkpoint is exempt from `IssuedBeforeStart`, as a grant's is:
  its standing reads no freshness and its mandate's window bounds its life. The exemption is `RevocationView::offer`'s,
  so it holds in every reader of a view — `mycelium-wiki`'s execution gate too. A **standing** scope must therefore
  not begin `call:` (or `grant:`): its checkpoints would skip the rule that keeps a replayed old checkpoint from
  refreshing a restarted reader.
- **Compiler:** `MandateRefusal` gained `Revoked { term }` and `CheckpointOffer` gained `NotHeld`. Both are
  `#[non_exhaustive]`, so a `match` outside the crate already has a `_` arm; **that arm must fail closed**. Additive:
  `PresentedCallMandate` (`#[non_exhaustive]`, built with `new`), `ExecutionAuthority::assess_call`,
  `named_stop_records`, `view_sizes` and four counters (below), `NamedStop` (`#[non_exhaustive]`),
  `RECLAIM_PER_HOLD`, `MAX_CALL_WINDOWS`, `MAX_CALL_WINDOWS_PER_GATEWAY`,
  `mandate::grant::{CALL_SCOPE_PREFIX, CALL_TERM_MAX_BYTES, GRANT_TTL_CAP_SECS, check_call_mandate}`,
  `Mandate::is_call`, `RevocationView::sizes`. `StopRecord` and `PresentedMandate` are unchanged.
- **`GRANT_TTL_CAP_SECS` (3600)** names the cap `POST /gateway/grants/{agent}/{request}` has always applied to
  `ttl_secs`; the door's behaviour is unchanged. `assess_call` refuses a `call:` mandate whose window is longer
  (`OutOfWindow`).
- **What a node holds for `call:` mandates is bounded.** A `call:` term is one key segment of at most 64 bytes
  (`CALL_TERM_MAX_BYTES`) with no `/`. A node holds windows for at most `MAX_CALL_WINDOWS` (262 144) calls, and
  `MAX_CALL_WINDOWS_PER_GATEWAY` (65 536) from one gateway; a `call:` mandate that would be one more is not
  established and is counted in `call_windows_refused`. Four gateways at their share fill the node's table, and a
  cancellable call from any other gateway is then refused there until windows expire. At the 3 600 s cap a share is
  held by about 18 calls a second. A gateway is its node's canonical name (`node:{NodeId}`): a `call:` mandate under
  another spelling of the same address is not established. The caps are constants in this release.
- **A call's checkpoint in the KV is verified at read, and four new counters say what was found** (`GET /stats`;
  `mycelium_call_…_total`; accessors of the same names on `ExecutionAuthority`). Any fleet member can write a key
  under `call/` by gossip, so a copy read from the KV is used only when its body names the gateway and scope its key
  names and its signature is that gateway's — whatever its sequence.
  - `call_checkpoints_refused`: a copy that failed that, or did not parse. It is treated as absent.
  - `call_checkpoints_stale`: a copy the gateway did sign, older than the newest this node holds — the first
    checkpoint written back over the revoking one.
  - `call_checkpoints_missing`: no value where this node had read a verified copy — a tombstone.
  - `call_windows_refused`: see the caps above.

  A forged copy can neither cancel a call nor un-cancel one, and a revocation a node has seen stays seen. **Watch
  the first three:** any of them rising is a member writing under another gateway's `call/` keys (or, for `refused`
  alone, a gateway whose key this node does not hold). **Not prevented, and not always detected.** A replay is counted `stale` only at a node that already holds the revocation. A provider the replay reaches before the revocation counts nothing and keeps running the call: one admitting later, one restarted, or one already running the call. A tombstone is counted `missing` wherever a verified copy had been read.
  The cancel is then denied at that provider until the mandate's window ends, at most 3 600 s (300 s at the
  default). The
  cancel-forward that does not go through the KV, and the gateway re-asserting its key, arrive with the row's later
  PRs (`docs/operations/what-is-proven.md`).
- **A background task:** a node with an execution authority attached subscribes to keys ending `/checkpoint` under
  `call/`. For each such write it looks up whether it holds that call's window, and reads the key only if it does —
  no read until a provider admits calls under `call:` mandates.
- **Code:** `src/agent/gateway_authority.rs` (`assess_call`, `feed_call_checkpoints`), `src/mandate/grant.rs`
  (`check_call_mandate`). Tests: `gateway_authority::call_tests` and `src/agent/call_mandate_tests.rs`.

## 101. The log read routes refuse a component-owned stream (unreleased, critique plan row CR1's follow-up)

§19 closed the log **write** routes — append and compact — to the streams a component owns: `cn/…` (the commitment
net's offers, closes, reports and assessments), `wiki/…` (a wiki's durable proposals) and `reason/…`
(`mycelium-reason`'s traces). The **read** routes stayed open: a token holding only `consensus:read` scanned or
subscribed to any of them, so it read a commitment's offers, a wiki's proposals, and a reasoning trace without
`llm:read`. `GET /gateway/overlay/log/group/subscribe` also proposed a consensus claim and recorded a position
under `clog/{stream}/…` for the stream it was given, under the same read scope. Every release from 2.27.0 is
affected (before it `consensus:write` reached them too). §99 closed the same asymmetry on the raw KV routes; the
read routes now follow the table the write routes use (`OWNED_LOG_STREAMS`, `src/agent/http/authz.rs`), and so do
the key names the raw listing shows. (§100 is identity-and-lifecycle I4's, PR #775.)

- **What changed:**
  - **`GET /gateway/overlay/log/scan?stream=S`**, **`GET /gateway/overlay/log/subscribe?stream=S`** and
    **`GET /gateway/overlay/log/group/subscribe?stream=S&group=G`** answer **403** `protected_stream` for a stream
    under `cn/`, `wiki/` or `reason/` (and for the bare names `cn`, `wiki`, `reason`), whatever the token's scopes
    (`"*"` included) and whether or not the stream holds anything. The body is JSON —
    `{"ok": false, "error": "protected_stream", "message": "…"}` — on the two subscribe routes as well: the event
    stream is never opened, so no event is sent, no watcher is registered, and the group route proposes no claim.
  - **The `message` says how the stream is read.** A trace: `GET /gateway/reason/trace/{run_id}` (`llm:read`),
    where `mycelium-reason`'s routes are merged. A commitment's records and a wiki's proposals have no HTTP read
    route: they are read in process, by the application that embeds the component.
  - **An application stream reads as before**, on all three routes. A stream is compared as the bytes it is
    stored under, after the query string is percent-decoded: `stream=cn%2Fr%2Foffers` is `cn/r/offers` and is
    refused; `CN/r/offers`, `/cn/r/offers` and an empty name are other streams, which hold none of a component's
    entries and read as (empty) application streams.
  - **A raw KV read of a key under `log/cn/`, `log/wiki/`, `log/reason/`** (403 `protected_key` since §99) no
    longer names the scan route in its `message`; it names the stream's own read, as above.
  - **`GET /gateway/kv/keys` leaves out the key names of a component's streams**: every name under `log/cn/`,
    `log/wiki/` and `log/reason/` — which requirement or run, when an entry was written and by which node — and
    under `clog/cn/`, `clog/wiki/` and `clog/reason/`, a consumer group's claim and position for such a stream.
    An unprefixed listing, or a prefix that spans them (`log/`, `log/c`), omits them; a `prefix` inside one
    (`log/cn/`, `clog/reason/x`) is **403** `protected_key`, as §99 answers inside its hidden namespaces. An
    application stream's names (`log/events/…`, `clog/events/…`) are listed as before. These six are not rows of
    §99's table: they are derived from `OWNED_LOG_STREAMS`, so a stream the log routes refuse is a stream whose
    names the listing hides.
- **Will the compiler tell me?** No — it is an HTTP status. The SDKs raise it typed, as they do for append and
  compact: `ProtectedStreamError` from `scan_log` / `scanLog`, `subscribe_log` / `subscribeLog` and
  `subscribe_log_group` / `subscribeLogGroup` (`mycelium-py` 0.2.16, where it is still an `httpx.HTTPStatusError`;
  `mycelium-ts` 0.2.10). The two subscriptions raise it before yielding anything. In `mycelium-py` any other
  refusal of a log subscription at open (a missing scope, say) is now the `httpx.HTTPStatusError` it is, where it
  was the SSE reader's content-type error.
- **Who is affected:** a client, script or dashboard that reads a `cn/`, `wiki/` or `reason/` stream through the
  log routes, or lists those streams' key names through `/gateway/kv/keys`. Nothing in this repository does: the companions read their streams in process, and `mycelium-py`'s
  `ReasonClient.trace` reads `GET /gateway/reason/trace/{run_id}`. A client that reads its own streams is not
  affected.
- **Migration.**

  | Stream | Read it through | Scope |
  |---|---|---|
  | `reason/{run_id}/…` | `GET /gateway/reason/trace/{run_id}` (`ReasonClient.trace` in `mycelium-py`) — the replayed run and its narrative | `llm:read` |
  | `cn/{requirement}/{offers,closes,reports,assessments}` | in process: `mycelium-commitment` (`offers`, `award_of`, `reports`, `assessments`) | — |
  | `wiki/{group}/proposals` | in process, by the application that embeds the wiki (`mycelium::mandate`) | — |

- **Check after upgrading.** With a `consensus:read` token, each of these answered 200 and now answers 403
  `protected_stream` (the stream need not exist):

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer CONSENSUS_READ_TOKEN' \
       'http://NODE:PORT/gateway/overlay/log/scan?stream=cn/x/offers'             # 200 before, 403 after
  curl -s -m 2 -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer CONSENSUS_READ_TOKEN' \
       'http://NODE:PORT/gateway/overlay/log/subscribe?stream=reason/x/n'         # 200 before, 403 after
  curl -s -m 2 -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer CONSENSUS_READ_TOKEN' \
       'http://NODE:PORT/gateway/overlay/log/group/subscribe?stream=wiki/x/proposals&group=g'   # 200 before, 403 after
  ```

  (`-m 2` ends the two subscriptions on an affected node, where they stay open; do not run the third against an
  affected production node you do not mean to write to — there it contends for a claim under `clog/wiki/x/…`.)
  Until every gateway is upgraded, treat `consensus:read` as a scope that reads every log stream, and review who
  holds it.
- **Not changed:**
  - **`GET /consensus/{slot}` still returns a component-owned slot's value under `consensus:read`**: a
    commitment's award (`cn/{requirement}/award`, which was never in a log) and a consumer group's claim
    (`clog/{stream}/{group}/claim`, the holder's node id). That route is not changed here; **§102 closes it**.
  - The names under `cn/`, `wiki/` and the other namespaces §99 left listed are still listed
    (`cn/{requirement}/award` among them); this section hides only the log and consumer-group keys of a
    component's streams.
  - Layer I is untouched: a peer still replicates every key (detection, not prevention), and an in-process
    caller reads any stream through `KvHandle`. These are the gateway's doors.

## 102. The consensus routes refuse component-owned slots, to read and to propose (unreleased, the slot decision of 2026-11-12)

Two gateway routes take a consensus **slot name** from the caller, and neither asked whose slot it was.
`GET /consensus/{slot}` answered any slot's committed value to a token holding only `consensus:read`: a
commitment's award (`cn/{requirement}/award` — the requirement, the awardee, the declarer), a consumer group's
claim, an electorate's record with its members (a read `GET /gateway/govern` puts behind `govern:read`), a
capability's authorisation policy and a fleet counter's seats. `POST /gateway/consensus/cross_group_propose`
proposed on any slot for a token holding `consensus:write`: on a fleet with no electorate declared it **decided**
an award's slot, a `capauthz/{ns}/{name}` policy slot, a seat, a consumer group's claim, a `lock/{name}` and a
`consistent/{key}` — each seen committed on the unguarded route, value and all (an `electorate/{group}/…` slot and
`leader/{group}` were already refused there by the electorate door, 409 `electorate_mismatch`). Every release that
has the two routes is affected. §99 and §101 closed the same asymmetry on the raw KV routes and the log routes;
the slot routes now classify by one table of their own (`SLOT_FAMILIES`, `src/agent/http/authz.rs`).

- **What changed:**
  - **`GET /consensus/{slot}`** answers **403** `{"ok": false, "error": "protected_slot", "message": "…"}` for a
    slot under `cn/`, `clog/`, `electorate/`, `ae-seat/` or `capauthz/` (and for each bare name — `cn`,
    `capauthz`, …), whatever the token's scopes (`"*"` included), before the store is read: the body is the same
    whether or not anything was ever decided on the slot. The `message` says how the decision is read — an
    electorate through `GET /gateway/govern` (`govern:read`); the others in process, by the component that owns
    them, and no route.
  - **`GET /consensus/consistent/{key}`** is read exactly when `GET /gateway/overlay/consistent/get?key={key}`
    reads `{key}`: for a key in an owned namespace (§99's table) it is 403 `protected_slot` with that key's own
    message. The gateway's `consistent/set` never wrote such a slot; an in-process `consistent_set` can.
  - **A lock's slot (`lock/{name}`) and a leader's (`leader/{group}`) are read as before** — the operator's read
    of a holder, which no other route gives without proposing — and so is every application slot. A slot is
    compared as bytes after the path is percent-decoded: `/consensus/cn%2Fr%2Faward` is `cn/r/award` and is
    refused; `CN/r/award`, `cnx/r/award` and a doubly-encoded `cn%252Fr%252Faward` are other slots, which hold
    none of a component's decisions.
  - **`POST /gateway/consensus/cross_group_propose`** answers the same **403** `protected_slot` for a `slot` in
    any of those five families **and in `lock/`, `leader/` and `consistent/`**, before anything is proposed: no
    ballot, no acceptor record and no decision is written for the slot. The `message` names the route or the
    component that proposes in the family — `POST /gateway/overlay/lock/acquire`, `POST /gateway/overlay/elect`,
    `POST /gateway/overlay/consistent/set`, `POST /gateway/govern/electorate` (`govern:write`), or the component in
    process. An application slot is proposed as before.
  - **A raw KV read of a `consensus/…` key** (403 `protected_key` since §99) says in its `message` which slots the
    slot route reads, and no longer sends every reader there.
- **Will the compiler tell me?** No — it is an HTTP status. The SDKs raise the proposal's refusal typed:
  `ProtectedSlotError` from `cross_group_propose` / `crossGroupPropose` (`mycelium-py` 0.2.16, where it is still a
  `PermissionError` and an `httpx.HTTPStatusError`; `mycelium-ts` 0.2.10). Neither SDK has a verb for
  `GET /consensus/{slot}`.
- **Who is affected:** a client, script or dashboard that reads a component's slot through `GET /consensus/…`, or
  that proposes through the cross-group route on a slot named under one of the eight families — for instance one
  that took a lock by proposing on `lock/{name}` instead of calling the lock route. Nothing in this repository
  does either: the runbooks read `lock/{name}`, which is unchanged; the SDKs and the examples propose on their own
  names (`donation/…`, `pipeline/config`). A client whose own slot names happen to start with one of the eight
  prefixes must rename them.
- **Migration.**

  | Slot family | Read it through | Propose in it through |
  |---|---|---|
  | `cn/{requirement}/award` | in process: `mycelium-commitment` (`award_of`, `award_of_verified`) | in process: `commit_award_linearizable` |
  | `clog/{stream}/{group}/claim` | — (held by the node serving the group subscription) | `GET /gateway/overlay/log/group/subscribe` (`consensus:read`) |
  | `electorate/{group}/…` | `GET /gateway/govern` (`govern:read`) | `POST /gateway/govern/electorate` (`govern:write`) |
  | `ae-seat/…` | — (the action evaluator's) | — (claimed when a call is admitted) |
  | `capauthz/{ns}/{name}` | in process, by the resolvers that enforce it | in process: `set_capability_authz_via_consensus` |
  | `lock/{name}` | `GET /consensus/lock/{name}` (`consensus:read`), unchanged | `POST /gateway/overlay/lock/acquire` (`consensus:write`) |
  | `leader/{group}` | `GET /consensus/leader/{group}` (`consensus:read`), unchanged | `POST /gateway/overlay/elect` (`consensus:write`) |
  | `consistent/{key}` | `GET /gateway/overlay/consistent/get?key={key}`, or `GET /consensus/consistent/{key}` for an application key | `POST /gateway/overlay/consistent/set` (`consensus:write`) |

- **Check after upgrading.** The read, with a `consensus:read` token, answered 200 and now answers 403
  `protected_slot` (the slot need not exist); a lock's slot still answers 200:

  ```bash
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer CONSENSUS_READ_TOKEN' \
       'http://NODE:PORT/consensus/cn/x/award'                     # 200 before, 403 after
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer CONSENSUS_READ_TOKEN' \
       'http://NODE:PORT/consensus/lock/x'                         # 200 before and after
  ```

  The proposal, with a `consensus:write` token. **Run it against an upgraded node only**: on an affected node it
  is a real proposal, and where the node can reach a quorum of group `G` it decides the slot — use a requirement
  name nothing will ever announce, or do not run it there at all.

  ```bash
  curl -s -w '\n%{http_code}\n' -X POST -H 'Authorization: Bearer CONSENSUS_WRITE_TOKEN' \
       -H 'content-type: application/json' \
       -d '{"slot":"cn/upgrade-check-never-announced/award","value_b64":"","groups":[{"group":"G","quorum":0.5,"veto":false}]}' \
       'http://NODE:PORT/gateway/consensus/cross_group_propose'    # 403 protected_slot after
  ```

  Until every gateway is upgraded, treat `consensus:read` as a scope that reads a commitment's award and an
  electorate's members, and `consensus:write` as one that can decide an award's slot, a capability's
  authorisation policy slot and a lock without the lock route — and review who holds them. If a `consensus:write`
  token may have been misused, an award is checked by its signature (`award_of_verified` with the declarer you
  pin), and a `capauthz/` slot's value is applied to `sys/capauthz/…` only by the node that called
  `set_capability_authz_via_consensus`, never by a learner.
- **Not changed:**
  - **The slot names are still listed.** `GET /gateway/kv/keys?prefix=consensus/` answers the key names under
    `consensus/{committed,ballot,lease,decided}/{slot}` and `consensus/life/{escaped slot}/…` to a `kv:read` token,
    so the listing still shows that a requirement has an award, an electorate's group and epoch, a
    `capauthz/{ns}/{name}` and a seat's two digests — the values are refused on every KV and slot route (and on
    the signal routes since §103, below), the names are not.
    Stated as a limit; a later row hides them (`docs/plans/external-critique-2026-11-12.md`, CR24).
  - **The signal routes — closed in §103.** When this section was written, `GET /signals/{kind}` and `GET
    /gateway/signal/sse/{kind}` (`mesh:read`) did not refuse the consensus kinds, so a tap received the proposals
    and commits its node was in scope for, slot and value included; and `POST /gateway/signal/emit`
    (`mesh:write`) sent a raw `consensus.commit` frame, which a learner without `[tls]` recorded as a decision for
    any slot. §103 refuses the consensus kinds on every door that takes a kind. **Until every gateway carries
    §103,** treat `mesh:read` on a node as reading the consensus traffic it sees, and `mesh:write` on a node
    without `[tls]` as able to plant a decision on any slot.
  - `lock/{name}` and `leader/{group}` stay readable under `consensus:read`: the holder's or the leader's node id.
  - The routes that build the slot themselves are untouched: lock acquire, elect, consistent set, the group
    subscription's claim, `POST /gateway/govern/electorate`.
  - Layer III is untouched: a member of the fleet proposes on any slot through the library, bounded by the
    electorate door and, for an award, by its signature. These are the gateway's doors.

## 103. The gateway's signal routes refuse the substrate's own kinds; the send routes refuse the RPC reply kinds; a hosted capability is called through `/mcp` only (unreleased, the signal-routes decision of 2026-11-13)

The gateway has ten doors that take a signal kind from the caller: the two signal streams (`GET /signals/{kind}`,
`GET /gateway/signal/sse/{kind}`), the serve stream (`GET /gateway/rpc/serve/{kind}`), the mailbox stream
(`GET /gateway/mailbox/{kind}`) and the six raw send routes (`rpc/call`, `scatter`, `signal/emit`,
`mailbox/deliver`, `shard/emit`, `overlay/emit_reliable`). They refused protected work (`mcp.invoke`,
`skill.invoke`, `llm.invoke`, `protected_rpc_kinds`) and, on the streams, the RPC reply kinds. They did not refuse
the kinds the substrate itself speaks, and those register on the same handler table a stream taps. Five things
followed, each reproduced by a test that failed before this change
(`src/agent/http/substrate_kind_tests.rs`):

1. **A `mesh:read` token read consensus traffic.** A tap on `consensus.propose` or `consensus.commit` received
   every proposal and commit its node was in scope for — the slot's name and the proposed value in `payload_b64` —
   whichever component owned the slot: a commitment's award (`cn/{requirement}/award`), an electorate's record, a
   lock's holder, an application's slot. `consensus.vote` and `consensus.nack` carry the acceptors' answers, an
   accepted value among them. A `mesh:serve` stream on a consensus kind received the same frames on a node whose
   consensus listener was not started, and on `consensus.vote` / `consensus.nack` between two proposals. §99, §101
   and §102 closed the KV, log and slot routes to a component's data; this was the same data by another door.
2. **A `mesh:write` token wrote a consensus decision on a mesh without `[tls]`.** `POST /gateway/signal/emit`
   (and `shard/emit`) send the caller's bytes as they are, with the gateway's node as the sender. A `CommitTerm`
   frame a client made up — a slot nobody proposed, a value nobody voted for — was written as that slot's decision
   by the gateway's node and by its peer; `GET /consensus/{slot}`, `consistent_get` and a lock's reader then read
   it. Without `[tls]` a consensus frame carries no signature (`sign_payload`, `decode_verify`), and a learner
   writes a commit's record without a quorum certificate. **With `[tls]`** a frame needs a member's signature, which
   a client cannot make; it could re-send one a tap had handed it, since the signature covers the message and not
   its sender, scope or time (see *Not changed*).
3. **A client of a provider's gateway answered that provider's RPC calls.** A reply is claimed by its correlation
   nonce and its sender (`sig.sender == target`). A tap on an observable RPC kind shows the nonce (the first eight
   bytes of `payload_b64`), and an emit's sender is the gateway's node — so a token with `mesh:read` and
   `mesh:write` on the provider's own gateway emitted `rpc.result` to the caller, and the caller's `rpc_call`
   returned the client's bytes. `[tls]` does not change this: the reply is the node's own emission. (This closes
   the forgery for a gateway client **against `rpc_call`, `scatter` and `bulk_call`**, whose replies are
   `rpc.result` and `bulk.result`. It does not close it against `MeshHandle::request`, whose reply kind the
   application chooses, nor for a *member*, which writes its own `sender` — both under *Not changed*, and
   `docs/wiki/dev/owner-signed-records.md`.)
4. **A `mesh:read` token read a bulk request's payload.** `bulk_call` stages its payload on the caller's gateway
   and sends an `invoke.bulk` ticket — `nonce ‖ port ‖ kind`. `GET /bulk/{nonce}` is public by design: the nonce
   is its credential. A tap on `invoke.bulk` on the responder's gateway showed the ticket and the caller's
   address, and the client fetched the whole payload from the caller while the call was in flight. An emitted
   `invoke.bulk` made a `bulk_serve` node fetch from the gateway's node and run its handler on what it got.
5. **A `mesh:write` token invoked a hosted capability around `/mcp`.** A WASM component's capability is served
   on `cap.invoke/{namespace}/{name}`; one in the `tool` namespace is reached over HTTP as a tool through `/mcp`,
   which asks `mcp:invoke` and runs the action evaluator's preflight. `POST /gateway/rpc/call` with that kind as its `method` reached the
   component's serve loop under `mesh:write` alone and returned its answer — as `mcp.invoke` did before §19's
   closure plan C1. A tap on the kind read every invocation's arguments and caller envelope.

Also: `POST /gateway/rpc/call` with `method: "sys.explain"` returned a peer's event ring under `mesh:write`,
where `GET /gateway/explain` asks `fleet:read`.

**Affected:** every release that has the routes. The read of consensus frames and of a bulk payload needs
`mesh:read` (or the legacy token, or no token model); the consensus write needs `mesh:write` and a mesh without
`[tls]`; the forged reply needs both scopes on the provider's gateway; the capability call needs `mesh:write`.

- **What changed.** All of it is `403 {"ok": false, "error": "protected_kind", "kind": …, "message": …}`, for
  every token (`"*"` included) and in every build:

  | Kind | Observe (`/signals/{kind}`, `/gateway/signal/sse/{kind}`) | Serve (`/gateway/rpc/serve/{kind}`) | Mailbox stream (`/gateway/mailbox/{kind}`) | Send (the six raw routes) |
  |---|---|---|---|---|
  | `consensus.propose`, `consensus.vote`, `consensus.commit`, `consensus.nack`, and **any kind starting `consensus.`** | refused (was 200) | refused, a listener registered or not (was 200, or `served_in_process`) | refused (was 200) | refused (was sent) |
  | `sys.explain`, `sys.replica-sync.v1`, and **any kind starting `sys.`** | refused (was 200) | refused (was 200, or `served_in_process`) | refused (was 200) | refused (was sent) |
  | `invoke.bulk` (the bulk transport's ticket) | refused (was 200) | refused (was 200, or `served_in_process`) | refused (was 200) | refused (was sent) |
  | `rpc.result`, `bulk.result` | refused since 2.32.0 | refused since 2.36.0 | refused (was 200) | **refused (was sent)** |
  | `cap.invoke/{namespace}/{name}` (a hosted capability's invocations) | **refused (was 200)** | unchanged (`served_in_process` where the component runs) | refused (was 200) | **refused (was sent): the door is `/mcp`** |
  | `mcp.invoke`, `skill.invoke`, `llm.invoke`, `protected_rpc_kinds` | refused, as before | as before | refused (was 200; `mailbox/deliver` has refused them since 2.15.0) | refused, as before |

  The list is `mycelium::SUBSTRATE_KINDS` (the six kinds above and `invoke.bulk`, which is refused by name), the
  prefixes `SUBSTRATE_KIND_PREFIXES`, the predicate `is_substrate_kind`; `HOSTED_CAPABILITY_KIND_PREFIX` is
  `cap.invoke/`. A test reads every kind constant from the source and fails until a new one is classified
  *refused* or *open by decision*; a new kind under `consensus.` or `sys.` is refused meanwhile, and **a kind
  outside every list is open**. A kind is compared as bytes, after the path is percent-decoded:
  `Consensus.propose`, `consensus`, `sys` and `system.x` are other kinds and pass.
- **A hosted WASM component's `mesh.emit`** refuses a substrate kind and an RPC reply kind even when its host
  listed it in `emit_kinds` (`confine_kind`, `ConfinementError::ProtectedKind`; rule `host.emit_admission`,
  revision 2).
- **Will the compiler tell me?** No — an HTTP status. Both SDKs raise `ProtectedKindError` for it from every verb
  that names a kind. Two verbs gained the mapping (`mycelium-py` 0.2.16, `mycelium-ts` 0.2.10, both unreleased):
  `mailbox` in both (it raised the SSE reader's error), and `onSignal` in TypeScript (it threw a generic
  `SSE request failed: 403`, for a protected kind too).
- **Who is affected:**
  - a client, script or dashboard that watches `consensus.*` on a signal stream. Nothing in this repository does
    (the ops console has no signal tab). There is **no replacement tap**: read a decided slot through
    `GET /consensus/{slot}` or `consistent_get` (application slots, §102), and a node's consensus health through
    `/stats` (`commit_conflicts`, the ballot and timeout counters) and `GET /gateway/explain` (`fleet:read`);
  - **an application whose own signal kind starts `sys.` or `consensus.`** — rename it. Those prefixes are the
    substrate's from this release;
  - a client that emitted `rpc.result` itself to answer a served request: answer through
    `POST /gateway/rpc/respond` (`rpc_respond` / `rpcRespond`), which is bound to the request it answers;
  - a client that tapped `cap.invoke/…` to watch a hosted capability's calls: read the host's invocation log
    (`InvocationLog`), or the decision records where an enforcement point is configured;
  - **a client that called a hosted capability through `rpc/call`, `scatter` or another raw route**: call it as
    a tool, `POST /mcp` `tools/call` (`mcp:invoke`). **Only a capability in the `tool` namespace is bridged as a
    tool** (`mycelium-wasm-host`'s runtime, `cap.invoke/tool/{name}`): a hosted capability in any other namespace
    (`route/optimize`, say) has **no gateway route** after this change — it is called by a member
    (`ServiceHandle::rpc_call`), in process. Nothing in this repository called one by a raw route. A member's own
    `rpc_call` to the kind is unchanged;
  - a client that tapped or emitted `invoke.bulk`: there is no gateway bulk verb and neither SDK has one; a bulk
    call is a member's (`ServiceHandle::bulk_call` / `bulk_serve`), unchanged;
  - **a client that reads a mailbox a *member* fills under a protected kind, a hosted capability's kind, or a
    kind starting `sys.` / `consensus.`**: the stream is refused; rename the mailbox kind. The gateway's
    `mailbox/deliver` never filled such a mailbox, but `ServiceHandle::deliver_event` checks no kind, so a Rust
    node could — and what it delivered stays under `mailbox/` until it expires;
  - a host that listed a `consensus.*` or `sys.*` kind in a component's `emit_kinds`: the emit is dropped, logged
    and counted (`mycelium_wasm_host_emits_refused_total{reason="protected_kind"}`).
- **Check after upgrading.** Each answered as its comment says — 200, or 504 for the last — and now answers 403
  `protected_kind`:

  ```bash
  curl -s -m 2 -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer MESH_READ_TOKEN' \
       'http://NODE:PORT/signals/consensus.propose'                              # 200 before, 403 after
  curl -s -m 2 -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer MESH_READ_TOKEN' \
       'http://NODE:PORT/gateway/signal/sse/consensus.commit'                    # 200 before, 403 after
  curl -s -m 2 -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer MESH_READ_TOKEN' \
       'http://NODE:PORT/signals/invoke.bulk'                                    # 200 before, 403 after
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer MESH_WRITE_TOKEN' \
       -H 'content-type: application/json' -d '{"kind":"rpc.result","scope":"node:127.0.0.1:1"}' \
       'http://NODE:PORT/gateway/signal/emit'                                    # 200 before, 403 after
  curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer MESH_WRITE_TOKEN' \
       -H 'content-type: application/json' \
       -d '{"target":"127.0.0.1:1","method":"cap.invoke/x/y","timeout_secs":1}' \
       'http://NODE:PORT/gateway/rpc/call'                                       # 504 before, 403 after
  ```

  (`-m 2` ends the three streams on an affected node, where they stay open. The emit sends an empty reply to an
  address nothing listens on, and the call names a capability and a target that do not exist; do not replace the
  emit's kind with `consensus.commit` against an affected node.)
  **Until every gateway is upgraded:** treat `mesh:read` as a scope that reads every consensus proposal and
  commit in the node's scope and every bulk request's payload sent to that node; `mesh:write` as a scope that
  invokes any hosted capability and — on a mesh without `[tls]` — can write a consensus decision; review who
  holds them, and prefer `[tls]`. If a `mesh:write` token may have been misused on a mesh
  without `[tls]`, a decision record nobody proposed is the trace: `commit_conflicts` on `/stats` counts a newer
  decision of another identity arriving over a live one, and a slot's records are under `consensus/life/`.
- **Not changed, each stated as it stands** (critique plan rows CR26–CR29; each kind below is a row marked *open
  by decision* in the classification table of `src/agent/http/substrate_kind_tests.rs`):
  - **Supervisor signals are unsigned.** `agent.approve` and `agent.veto` are ordinary signals: any `mesh:write`
    holder can emit `agent.veto` naming a tool and veto a pending approval, and any `mesh:read` holder sees the
    approvals asked. It fails closed — a veto refuses; nothing can be approved this way, since silence approves —
    and it is how a supervisor on an SDK takes part. Not changed; CR26 signs or scopes them.
  - **Other kinds the library speaks stay observable under `mesh:read` and sendable under `mesh:write`**:
    `boundary.opaque` / `boundary.transparent`, `agent.state`, `artifact.fetch` / `artifact.size` /
    `artifact.fetch_range` and `reason.blob.fetch` (a tap shows the ids asked for, never a reply; a call under
    `mesh:write` fetches the artifact or blob for an id the caller knows), and a component's own
    `comp/{namespace}/…` kinds.
  - **The serve stream on a hosted capability's kind** is refused only where a component serves it on that node.
  - **A member can still answer another member's call.** The reply claim is a nonce and a sender, and a member
    writes its own sender; item 3 above is closed for a gateway client only, and only for the reply kinds the
    library owns (`rpc.result`, `bulk.result`).
  - **A reply to `MeshHandle::request` can still be forged by a gateway client.** `request(kind, scope, payload,
    result_kind, timeout)` waits on a result kind the application names — `signal_kind`'s doc suggests
    `invoke.result` — and claims the first signal of that kind whose first eight bytes are its nonce: it checks
    **no sender** (`mycelium-core/src/mesh_handle.rs`). A client holding `mesh:read` and `mesh:write` taps the
    request kind, reads the nonce, and emits the result kind; the application's `request` returns the client's
    bytes — from any gateway in scope, not only the provider's. No library code calls `request`. Use `rpc_call`
    where the reply decides anything, or list both of your kinds in `protected_rpc_kinds`, which closes the tap
    and the emit to them (critique plan row CR27).
  - **A hosted capability outside the `tool` namespace has no gateway route** — *not built* (critique plan row
    CR29; `what-is-proven.md`). The maintainer confirmed on 2026-11-13 that the refusal stays for every namespace:
    a namespace carve-out would leave a `mesh:write` route to run installed code — a shadow install included —
    with no decision at a door.
  - **A tap on an ordinary RPC kind shows the whole request frame**, the caller envelope and a carried mandate
    with its possession proof included, as §19 said of a protected kind before the streams refused it. An RPC
    kind whose calls carry authority belongs in `protected_rpc_kinds`.
  - **A signed consensus frame is not bound to its sender or to a time.** With the tap and the emit refused there
    is no gateway route to a replay; a member can still re-send another member's signed frame.
  - A member is not a client: a node in the mesh sends and receives these kinds, as the protocol needs. These
    are the gateway's doors (and a hosted component's `mesh.emit`), not Layer II's.

## 104. A cancellable rule requires the call's `call:` mandate at the provider; `cm` in the caller envelope (2.41.0, identity-and-lifecycle I4, PR 1b)

A rule can be declared **cancellable** — `cancellable = true` in a unit file's `[[rule]]`, `Rule::cancellable()` in
code. The call it permits may run only where a cancel can stop it: an enforcing provider registers the call's work
under its gateway's `call:` mandate (§100), read from a new field of the caller envelope, `cm`, and **refuses the
call if that mandate is not established**. This release builds the provider's half. **No gateway mints a `call:`
mandate yet** (the row's next PR), so nothing changes for a node that declares no cancellable rule — and a node
that declares one refuses every call that rule permits.

- **Do not declare a cancellable rule yet.** Until the gateways of a fleet mint (`gateway_mint_call_mandates`, not
  in this release), an enforcing provider answers a call a cancellable rule permits `authority_not_established`
  (JSON-RPC -32031), `data.detail` `call_mandate_not_established`, with the stable name of why in `data.cause`
  (`no_call_mandate`) and the sentence in `data.found` (*"the envelope carries no call: mandate"*). Nothing runs, and the evidence journal records `execution: none` beside
  the permit. This is the rule doing its work: stripping the mandate cannot make a cancellable call run
  uncancellable.
- **The same refusal, whatever the rule,** for a call whose `call:` term is already revoked when the provider
  registers it: the gateway cancelled it before it ran (`data.cause` `revoked`). **The limit:** under a rule that is
  not cancellable this is best effort — a relay that strips `cm` makes the same call run.
- **`data.cause` is the machine-readable reason; `data.found` is prose and may change.** One detail name, ten
  causes: `revoked` (final: the gateway cancelled the call before it ran) · `no_envelope` (the call came in no
  caller envelope) · `no_call_mandate` (the envelope has no `cm`: no mint, or stripped) · `unsigned_envelope` (the
  envelope is not signed by its gateway — a node with no TLS identity — so its id binds nothing) · `no_authority`
  (the provider has no execution authority) · `malformed` · `no_envelope_id` · `other_envelope` (the term is not
  this envelope's) · `refused` (verified, and refused for its window or operation) · `not_verified` (issuer, key or
  checkpoint not verified here, or the window cap). Match on `revoked` to tell a cancellation from the rest.
- **Counted:** `mycelium_provider_call_mandate_refusals_total{cause}` (feature `metrics`), one label value per
  cause above, so a cancellation is not counted with a misconfiguration.
- **The refusal gives a grant back at the provider, and leaves it spent at the gateway that claimed it.** The
  provider refuses before any handler runs, so it releases its own use of a grant the same call presented: a caller
  that reaches the provider directly may present the grant again, once — at that provider node (§106). **A gateway does not give its claim back
  on this answer** — at `/mcp` or at the A2A doors — so a retry of the same grant through the same gateway door is
  refused there as already used, though the provider would admit it. Why: a provider at 2.40 or before that bridges
  an MCP server forwards the server's replies in this shape unrewritten, so a gateway that honoured the answer
  would let a server that ran its effect have the grant run again. From this release a bridging provider rewrites
  a server's reply in this shape into an ordinary tool error (-32000), so a later release can honour the answer at
  the gateway once every bridging provider is at a version that does. Until then, a `requires_grant` +
  `cancellable` rule behind a gateway costs a grant per refused call: one more reason not to declare a cancellable
  rule before the gateways mint.
- **A cancelled call cannot be made to run by presenting it again**, by three mechanisms in sequence: while the
  provider stays up, the envelope's id is admitted once and a replay is **dropped unanswered** (the re-presenter
  sees a timeout, not a refusal); after a provider restart inside the envelope's 300 s, the envelope is admitted
  again and its term reads revoked from the KV record; after that the envelope is stale. A gateway's retry is a new
  envelope and a new term, which is not revoked: a cancel is per dispatch.
- **Where it is enforced.** At a provider with `with_provider_enforcement` (or `[authority]`) in a build with
  `compliance`. A provider without enforcement never reads `cm` and runs the call unregistered; an older provider
  keeps `cm` among the envelope's unknown fields and reads nothing. Neither refuses. A gateway's own preflight
  carries the rule's `cancellable` and does not act on it: the mandate is assessed once, at the provider.
- **Not covered.** A member's own unframed call has no envelope, so it carries no `call:` mandate: under a
  cancellable rule it is refused (`no_envelope`), and under any other rule it runs and cannot be cancelled. A call
  served through the bridged MCP loop is registered but not stopped when cancelled. A call served to an SDK agent
  (`rpc/serve`) is **not stopped either**: its admission is only parked, and only under a cohort budget, and
  nothing waits on its lapse. Both arrive with the row's cancel doors.
- **Not built: the built-in `llm.invoke` provider is not checked at all.** The node's own LLM skill receiver
  verifies the caller context and goes to the backend; it never runs the provider check. So at an enforcing
  provider a rule on `llm.invoke` — cancellable or not — is not applied at that door, and a cancellable
  `llm.invoke` rule neither refuses nor registers the prompt there. This predates the release
  ([ledger](../operations/what-is-proven.md#llm-door-not-checked-at-provider)); the gateway's own preflight on
  `/gateway/llm/call` is unaffected.
- **A `call:` mandate fits one envelope.** Its term must be `c{n}` of the envelope that carries it (the envelope's
  id, inside the gateway's signature). One lifted into another envelope, even of the same gateway for an identical
  call, is not established. `cm` is outside the envelope's signature, as `m` is; wire **v12** is unchanged.
- **A unit file with `cancellable` is refused by an older reader** (a `[[rule]]` with an unknown key is refused by
  name, §58–§65; a reader at 2.37 or before reads it as a plain allowance). Declare it only once every node that
  loads the file is upgraded.
- **New refusal at `validate()`:** `requires_mandate` naming a `call:` scope (`"call:*"`, `"call:c1"`). No caller
  can present a `call:` mandate, so such a rule permitted nothing; it now says so by name and points to
  `cancellable`. A broader pattern that would cover one (`"call*"`, `"*"`) is not refused and is never met by a
  `call:` mandate either: the second slot is not what `requires_mandate` reads.
- **Order matters, as for every rule:** the first matching allowance decides, so a broader allowance listed before
  a cancellable rule makes it inert, without a word. List the cancellable rule first. An exposure rule with `cancellable` is refused too.
- **Compiler:** `Rule` and `RuleDecl` gained a public field, `cancellable` — an exhaustive struct literal breaks;
  `Rule::new(…)` and `..Default`-style construction are unaffected. `Decision` (`#[non_exhaustive]`) gained
  `cancellable` and `Decision::requiring_call_mandate`; `GatewayCaller` (`#[non_exhaustive]`) gained
  `call_mandate`. **An evaluator other than `ReferenceEvaluator`** returns `cancellable: false` from every
  constructor: under it no rule is cancellable and the requirement is not applied, until the adapter maps its
  policy's notion onto `requiring_call_mandate`.
- **No new configuration field.** `GossipConfig` is unchanged by this release of the row.
- **Code:** `src/agent/provider_enforcement.rs` (`admit_after`, `assess_call_slot`), `src/agent/gateway_caller.rs`
  (`cm`), `src/agent/action_evaluator.rs`, `src/capability_config.rs`. Tests: `src/agent/call_admission_tests.rs`.

## 106. A grant's use is kept in the door's own journal; the use marker is signed; a tombstone at its key reads *used*; a release is honoured only by the door that made it (2.41.0, critique plan row CR2, first PR)

A grant admits one call at each enforcement point. Until this release the only durable record of that use was the
marker `grant/{holder}/{term}/used/{door}` in the gossip KV — an unsigned timestamp, read for presence — so any
admitted member could tombstone it and **re-arm the grant**: at every other node at once, and at the door that
admitted the call after a restart or once its in-memory claim (60 s at most) was swept. **Every release that has
single-use grants is affected** (2.38.0 on).

Now:

- **The door keeps its own record.** A node whose door admits a call under a grant appends the use to a journal of
  its own — `{persistence.base_path}/{node}/grants/uses.journal`, never gossiped, fsynced **before the call is
  admitted** — and reads that journal before the KV. A use it holds reads *used* whatever the KV says, and a
  restart reads it back. A node without `[persistence]` keeps the record in memory, as it keeps everything.
- **At the marker key, anything other than *never written* reads *used*:** a marker that verifies, a value that
  does not, a tombstone, a release made at another door. The marker is now a JSON record signed by the door's node
  — `{v, kind: "use", grant, door, node, use, at_ms, signer, sig}` — its body naming the grant, the door and the
  use.
- **A door reads its own marker write's receipt, and admits only when the marker is the key's value.** If the
  write lost to a newer value at the key (a tombstone or an overwrite at a later stamp, landing after the door's
  read), the call is **refused** — a denial whose reason ends `(grant_use_superseded)`. If the node's store
  applied nothing (its live-entry cap, `max_store_entries`, is reached — which a member's flood can cause), the
  call is refused with a reason ending `(grant_use_not_stored)`: admitted, it would have run with no marker at
  this node's key. In both cases **the grant stays used at that door** though nothing ran — the record was synced
  to the WAL and gossiped before the apply was decided, so a restart or a peer may hold it.
- **A release is a signed record, and only the door that made it honours it.** A gateway that refuses a call
  before dispatch, or is answered a caller-context refusal by the provider (§91, §97), and a provider that refuses
  `call_mandate_not_established` before any handler (§104), give their use back: a signed `kind: "release"` record naming the use replaces the door's own marker (a
  tombstone before this release), and **only then** is the release appended to the door's journal. A give-back
  that does not complete — the key no longer holds this door's marker, or the release's write was not the key's
  value — leaves the journal showing the use, so the door reads used even once nothing is left at the key. **At that node's door the grant then admits one more call. At every other door of the same class the
  release reads *used*.**

**The behaviour change to plan for.** Before this release a released grant was admitted by *any* gateway (the
marker was tombstoned, and absence admitted). Now:

- after a refusal before dispatch at a gateway, **the retry must go to the same gateway node**; at any other
  gateway the grant is refused *already used at gateway*;
- after a provider's `call_mandate_not_established` give-back, **the retry must reach the same provider node**;
  at another provider of the same tool it is refused *already used at provider*. A direct caller chooses its
  provider. For a call a gateway routes, the caller does not choose the provider: `tools/call` goes to the
  first provider in scan order, so after provider P1 gives its use back a retry routed to P2 — after a membership
  change, say — is refused *already used at provider*, and the client cannot steer it. It needs a new grant.

A client behind a load balancer over several gateways pins its retry to the gateway that refused it (the request
id of a client-held grant ends in the gateway that wrote it), or asks for a new grant. Why it is so: a door that
did not record a use cannot tell a genuine release of the newest use from an earlier release written back over a
later use, from a member's own marker-and-release pair written over another door's use, or from a release that
overlapped another door's admission — each would re-arm a spent grant there. The signed release in the KV is
therefore never the reason a grant reads unused: the releasing door's journal is. The KV copy tells the other
doors, an auditor and a later collector who released which use.

**Mixed versions (one release).** No combination re-arms a grant at an upgraded door. An un-upgraded door is
unprotected until it is upgraded: a member's tombstone still re-arms the grant there. Case by case:

- an upgraded door reads an **old-form marker** (a timestamp) as *used*, and counts it (`legacy`);
- an un-upgraded door reads a new-form marker or release by presence: *used*;
- **an un-upgraded gateway releases by tombstone, which an upgraded door reads as *used*** — so during the mixed
  release a pre-dispatch refusal at an un-upgraded gateway **spends the grant at every upgraded door**, and at
  that gateway itself once it is upgraded (its journal holds nothing from before). The safe side, stated;
- a release made by an upgraded door reads *used* at an un-upgraded one.

**Upgrade the doors that verify before the ones you rely on to release** — as for I3's digest (§93), verifiers
before writers: upgrade every gateway and provider of a class together where you can, and expect a refused retry
to need a new grant while they differ. The release after this one stops recognising the old form as anything but
an unverifiable value, which reads *used* too.

**Counted** — `GET /stats` `grant_use`, one number per reading, in memory (zero again after a restart); with
`metrics`, `mycelium_grant_use_reads_total{reading}`:

| `reading` | what the door found | read as |
|---|---|---|
| `tombstone` | a tombstone at the marker key | used |
| `unverifiable` | a value that is neither form, or a marker whose signature, grant or door does not verify | used |
| `legacy` | a marker in the form before this release | used |
| `release_elsewhere` | a verified release made by another node's door | used |
| `release_forged` | a release that does not verify — another key, another grant or door | used |
| `release_replayed` | this node's own release that is not the release of the use its journal shows released | used |
| `superseded` | this door's own marker or release write lost to a newer value, or its release was not written because the key no longer held its own marker for that use | the call refused / the grant spent |
| `not_stored` | this door's own marker write was refused by the node's store (`max_store_entries` reached) | the call refused, the grant spent at this door |

`tombstone`, `unverifiable`, `release_forged` or `release_replayed` above zero on a fleet at one version means a
member wrote a marker key it does not own. `release_elsewhere` does not: an honest retry at another gateway than
the one that released produces it. `legacy` is expected while older doors run. `not_stored` means the store is
full.

**A release's signature decides nothing.** Under the rule above no reader takes a release for *unused* except
the door that made it, and that door decides from its journal. The signature is verified so that a forged release
is counted apart (`release_forged`); with the verification removed only the counters move. A release is written
only over the releasing door's own marker for that use: if the key holds anything else by then, nothing is
written over it, the use is not given back, and the journal still shows it.

**Not built, stated:**

- **Two doors admitting one grant at the same time** (critique plan D12). Two gateways that each read *never
  written* before either marker arrives both admit. A third gateway then sees whichever value won LWW — and if one
  of the two released its use, possibly that release, which it reads *used*. Stated, not prevented; the
  `FleetConsensus` scope is the offer for a grant that must not.
- **The tombstone horizon at a door that never recorded the use.** A second gateway has only the KV. A tombstone
  planted over a marker is collected after `default_ttl × propagation_window_secs × 10` seconds (3 000 s at the
  defaults), after which the key reads *never written* there; a grant valid longer than that (the cap is 3 600 s)
  can be presented once more at a gateway that did not make the first use. The door that made the use is not
  affected.
- **A member can spend a grant** by writing anything at its marker key before the first use: a refusal, which is
  availability and outside the owner-signed rule.
- **What a door's journal does not survive, stated.** A node restarted **under a new address** is a new door
  with an empty journal (the journal sits under the node's id): it reads uses the old address made from the KV
  only. An entry that does not decode is skipped, logged and counted (`GET /stats`
  `grant_use_journal_undecoded`), which forgets that use; a deleted
  journal file reads as empty. The journal is read whole at every start and never compacted. A build with
  `gateway` and `tls` and **without `compliance`** runs this path with no member-key view beyond the keys the
  node itself trusts (`trusted_keys`): a marker it cannot verify reads used and is counted `unverifiable`, and its
  own records verify under its own key — **untested**: the tests here are in a `compliance`-gated module.
- **The journal is not compacted**, and the signed markers and releases under `grant/` are not collected: both
  arrive with I4's collector. A use is about 200 bytes; a door forgets a use in memory 3 720 s after it (the
  grant's window cap plus two minutes).
- `consumed`, `revoked`, the request record, `issued` and `checkpoint` are unchanged (CR2's later PRs).

**Compiler and formats.** No public type, configuration field or wire frame changes (wire **v12**). The marker's
KV **value** changes; no existing on-disk file changes; one new file per persisting node. A node restored from a
backup taken before a use reads that use from the KV marker only.

**Check:** `curl -s http://NODE:PORT/stats | jq .grant_use` shows the eight
readings; after a call under a grant, `ls BASE_PATH/NODE/grants/uses.journal` exists on the node whose door
admitted it; and presenting the same grant again at a second gateway is refused *already used at gateway* whether
or not another member has first deleted the marker's key.

**Code:** `src/agent/grant_use.rs` (`reads_used`, `record_use`, `release_use`, `GrantUseLedger`),
`src/agent/http/authz.rs` (the claim in `Settling::finish`, `release_unspent_grant_claim`),
`src/agent/state_machine.rs` (`write_owned_kv_reporting`). Tests: `src/agent/grant_use_tests.rs`.
