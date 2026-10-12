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

Honest answer: **usually not**. Only two entries carry `#[deprecated]`, so for the rest this page is
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
| 23 | a bare (untagged) identity proof or consensus signature | 2.32.0 (accepted until the next MINOR) | the tagged form every 2.32 node writes — upgrade the fleet | No — counted (`identity_untagged_proofs`, `consensus_untagged_signatures`); an upgraded proposer times out until a quorum is upgraded |
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
| 34 | "declare a new group and move the work yourself" as the way to change an electorate's members; a default that cannot move | unreleased (after 2.32.0) | `declare_successor_electorate` → `retire_electorate` → `complete_migration` (`POST /gateway/govern/electorate` with `predecessors` / `retire_to` / `complete`; `drop_predecessor` for one that never retires) | Partly — `ElectorateDecl` and `Retirement` gained fields, `ElectorateError` gained `Retired` (`#[non_exhaustive]`; a `_` arm must fail closed); otherwise refusals by name |

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

**What changes.** Registering to serve an RPC kind — `POST /gateway/rpc/serve/{kind}` and
`/gateway/rpc/respond` — requires the new scope **`mesh:serve`**. From 2.15.0 a token holding
`mesh:read` or `mesh:write` was still admitted there, **with a warning in the gateway log**. The
window was promised for one release and stayed open through 2.18.1; **2.18.2 closes it** — such a
token now gets `403 {"required_scope": "mesh:serve"}`.

**Why.** Serving and calling were one scope, so a token that could serve could also reach the
raw call routes; separating them is what lets a skill server hold *only* `mesh:serve` (closure plan
C1, `src/agent/http.rs`).

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

## 23. Identity proofs and consensus signatures are domain-tagged: the bare form is accepted for one release (2.32.0)

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

## 34. An electorate's members change by a migration that terminates (unreleased, after 2.32.0)

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

## 37. Nineteen public items nobody references are deprecated, not removed (unreleased, after 2.32.0)

**What changes** (post-360 plan row J, PR #608). A sweep of every `pub fn` / `const` / `struct` / `enum` / `type`
/ `trait` in the library crates against every file in the repository — `src/`, every `mycelium-*` crate, `tests/`,
`examples/`, both SDKs' sources and docs, and `docs/` — found nineteen items whose only occurrence in the tree is
their own definition (the grep for each is in the PR). Each now carries `#[deprecated(since = "2.33.0", note =
"…")]` with the same note: *no caller, test, SDK or document in the repository references it … kept on the 2.x line
— open an issue if you use it and it will be undeprecated.* Nothing is removed; the 2.x line removes nothing.

`mycelium_core::MeshHandle::group_quorum_prehashed` · `mycelium_core::receipt::…::with_replica_sync` ·
`mycelium_core::kv_ns::{MANDATE, CONSUMER_LOG, LOCK}` · `mycelium_effects::Counting::with_counts` ·
`mycelium_sim::bundle::…::path_for` · `mycelium_wasm_host::MeshArtifactSource::cached_len` ·
`Provisioner::disable_resource_policy` · `Provisioner::decision_trace` · `mycelium_wiki::…::with_semantic_lint` ·
`mycelium_wiki::lint::…::with_max_tokens` and `reconcile::…::with_max_tokens` · `AeEvidence::audit_outcome`
(`compliance`) · `RequestPrincipal::is_client` · `AgentStateMachine::set_task_id` ·
`mycelium::federation::{verify_descriptor_rotating, verify_policy}` (`tls`) ·
`FederationClient::with_credential_lifetime` · `mycelium::knowledge::store::…::issuers_about`.

A twentieth, `Preflight::is_inert` in `src/agent/http.rs`, is left as it is until that file's split (row P5) and
row E, which touch the file.

**Will the compiler tell me?** Yes — every use site of a deprecated item warns, and a crate built with
`-D warnings` fails on it; that is the whole mechanism, and it is why the items are deprecated rather than removed.

**Migration.** None is needed for an item you do not call. For one you do: the three `kv_ns` constants are the
namespaces' *vocabulary*, not the namespaces — `lock/`, `clog/` and `mandate/` are live and owned, their rows in
`src/lib.rs` § KV namespace ownership unchanged, and the owners write the literals (a `grep -rn '"lock/'` finds
them); write the literal, as they do. For a builder knob (`with_replica_sync`, `with_counts`, `with_semantic_lint`,
`with_max_tokens`, `with_credential_lifetime`) or an accessor (`cached_len`, `decision_trace`, `is_client`,
`issuers_about`, `audit_outcome`), keep calling it — the warning is the request to say so on an issue, after which
the attribute comes off in the next MINOR. `verify_descriptor_rotating` / `verify_policy` have `verify_descriptor`
and `TrustBundle::key_for` beside them; `set_task_id` has the `"current"` default it documents; `path_for` is
`root.join(name)`; `disable_resource_policy` is `Provisioner::with_resource_policy` with a permissive policy, or
the default.

**Not built:** an automated sweep in CI. The grep above is a one-off (`scripts/` holds no copy); rustc's `dead_code`
cannot see an exported item, so the next sweep is a session's work, not a gate.
