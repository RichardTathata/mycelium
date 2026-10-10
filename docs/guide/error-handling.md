# Error Handling in Mycelium

Mycelium exposes distinct error types per domain. Each handle returns exactly
the type that matches its failure modes; callers never need to downcast a
catch-all error to discover what went wrong.

---

## Error type taxonomy

| Type | Returned by | Recoverable? |
|------|-------------|:------------:|
| [`GossipError`](#gossiperror) | `GossipAgent::new` / `start` / config load | Depends on variant |
| [`ConsistencyError`](#consistencyerror) | `ConsensusHandle::consistent_set`, `distributed_lock`, `elect_leader` | Yes — retry |
| [`RpcError`](#rpcerror) | `ServiceHandle::rpc_call` | Yes — retry / call another peer |
| [`QuorumError`](#quorumerror) | `KvHandle::set_with_min_acks` | Yes — retransmit when peers rejoin |
| [`ReceiptError`](#receipterror) | `KvHandle::set_with_receipt`, `set_requiring_sync`, `retry_with_receipt`, `GossipAgent::set_with_replica_sync` | Depends on variant — and one variant is *neither* |
| [`ScatterError`](#scattererror) | `ServiceHandle::scatter_gather` | Yes — retry or reduce `min_ok` |
| [`SchemaError`](#schemaerror) | `SchemaHandle::publish_schema`, `seed_schemas_from_dir` | Depends on variant |
| [`BulkError`](#bulkerror) | `ServiceHandle::bulk_call` | Yes — retry |
| [`ShardError`](#sharderror) | `ServiceHandle::emit_sharded` | Yes — wait for providers |

Feature-gated extras: `PromptSkillError` (`llm` feature), `LlmError` (`llm`),
`McpError` (internal to the MCP bridge, not public).

---

## `GossipError`

Re-exported from `mycelium-core` (`mycelium::GossipError`). All ten variants:

```rust
pub enum GossipError {
    InvalidField { field: &'static str, reason: String },   // a setting out of range, or refused at start() (below)
    FieldConflict { field_a: &'static str, field_b: &'static str, reason: String }, // e.g. http_port == bind_port
    NodeIdMismatch { node_id: String, bind_addr: String },  // node_id doesn't encode the bind address
    FrameTooLarge { size: usize, limit: usize },            // a frame exceeds MAX_FRAME_BYTES
    UnsupportedWireVersion { received: u8, current: u8, prev: u8, hint: &'static str }, // peer wire skew
    AlreadyRunning,                                          // start() called twice
    Shutdown,                                                // start() after shutdown (create a new agent)
    Io(std::io::Error),                                      // listener bind, TLS cert setup
    Toml(toml::de::Error),                                  // config file parse failure
    Parse(std::num::ParseIntError),                         // env-var parse failure
}
```

**When you see it:** mostly startup (`new` + `apply_env_overrides` + `start`) and
config loading (`GossipConfig::load_from_file`). Two are not startup-only:
`FrameTooLarge` guards the wire path and `UnsupportedWireVersion` is raised when a peer
speaks an out-of-range wire version.

**Start refusals.** `start()` refuses rather than run degraded, each as `InvalidField` naming the
setting — the full list, by `field`:

| `field` | Refused when | Since |
|---|---|---|
| `gateway_named_tokens` / `gateway_scoped_tokens` / `oidc` | set in a build without `compliance` (it would ignore them and run an open gateway) | 2.18.1 |
| `tls` / `gateway_tls` | set in a build without `tls` (it would run plaintext) | 2.18.1 |
| `http_port` / `gateway_tls` | set in a build without `gateway` (it would run no gateway) | 2.25.0 |
| `oidc` | the issuer, or a configured `jwks_uri`, is not on a non-empty `egress.allow_hosts` | 2.20.0 |
| `audit_sink` | an audit sink is attached (`compliance`) without `[tls]` — nothing would be sealed | 2.20.0 |
| `gateway` | the gateway cannot bind its port or load its certificate | 2.20.0 |
| `http_addr` | the gateway binds a non-loopback address with no credential model (no token, no token table, no `[oidc]`) and `gateway_allow_unauthenticated` is unset | unreleased |
| `persistence` | unreadable state (2.20.0); a second owner of the directory, or a failed startup snapshot (2.23.0); a directory that cannot be created (2.24.0) — `docs/operations/deployment.md` § *Persistence start refusals* | — |
| `profile` | a guarantee the configured profile requires is unmet; the message names each | 2.19.0 |
| `http_addr` / `bind_address` | not a valid IP address | — |

**Recoverability:**
- `InvalidField` / `FieldConflict` / `NodeIdMismatch` / `Toml` / `Parse` — fix the
  configuration; always fatal at startup.
- `Io` — typically fatal at startup (port in use, cert not found). Runtime TCP errors
  (peer unreachable, write timeout) are **not** raised here — they are absorbed and
  surfaced via `system_stats().dropped_frames` and `peer_drop_counts()`.
- `FrameTooLarge { size, limit }` — the frame exceeds `framing::MAX_FRAME_BYTES`. KV writes
  are size-gated at `framing::MAX_KV_WRITE_BYTES` (= `MAX_FRAME_BYTES − 64 KiB`), so **chunk
  large state yourself**. An oversized *inbound* frame is dropped (counted in `dropped_frames`)
  without tearing down the connection, and anti-entropy *skips* an oversized entry rather than
  stalling — so this rarely reaches a caller, but it is the error to expect if you hand a single
  value larger than the budget to a write.
- `UnsupportedWireVersion` — the peer is on a different wire version (see the wire-version policy
  atop `framing.rs`); upgrade the lagging node.
- `AlreadyRunning` — call `start()` at most once per agent instance.
- `Shutdown` — create a new `GossipAgent` instead of restarting a shut-down one.

---

## `ConsistencyError`

```rust
#[non_exhaustive]
pub enum ConsistencyError {
    Timeout { ballots_tried: u32 }, // no quorum reached within deadline
    Superseded,                     // the slot was decided for another caller's value
    TopologyUnsatisfied,            // quorum met but Hard topology gate failed
    ElectorateUnavailable { observed_members: usize, declared_min: usize }, // no electorate: nothing decided
    NotAMember { group: Arc<str> }, // this node is not in the group's roster: nothing proposed (2.32.0)
}
```

**When you see it:** `consistent_set`, `consistent_get`, `append`,
`distributed_lock`, `elect_leader`.

**Recoverability:**
- `Timeout` — retry; the cluster may be partitioned or underloaded. Check
  `ballots_tried` to distinguish a slow cluster from a hard split.
- `Superseded` — the slot was decided for another value: a concurrent writer committed first, or (2.30.0)
  this call's prepare phase found a value a quorum had already accepted and committed that instead. Re-read
  the current value and decide whether to retry with a new key or accept the other writer's value.
- `ElectorateUnavailable` — the group roster this node sees is empty (an unknown or unjoined group) or
  smaller than a fresh `MembershipIntent { min }` declares, so no electorate could be established and nothing
  was decided. Join the group — `POST /gateway/mesh/group`, or `/gateway/govern/group` (`govern:write`) for a group under a membership intent — or wait for the roster to converge (`GET /gateway/mesh/group?group=G`); do not
  read it as a refusal of the value.
- `NotAMember` — this node is not in the group's roster, so it did not propose: its own vote would be one the
  electorate does not contain. Join the group (the same routes as above); nothing was decided.
- `ElectorateNotGoverned` (P2) — the node requires an **electorate group** for an exclusive outcome
  (`consensus_require_electorate`) and this safety-sensitive proposal's scope was the whole cluster (`group: None`)
  or a group with no electorate declaration. Nothing was proposed. Declare the group (`declare_electorate`,
  `POST /gateway/govern/electorate`) and, for the cluster-scoped lock and consistent verbs, mark one group
  `exclusive_default`. Over HTTP **403** `electorate_not_governed`.
- `ElectorateStale` (P2) — a member of the proposal's electorate holds a later epoch: the electorate stepped and this
  proposer had not learned it. Nothing was decided by this proposal; retry, which reads the new epoch. **409**
  `electorate_stale`.
- `ElectorateMismatch` (P2) — the electorate and this node's view disagree: the roster differs from the epoch's
  members (a node joined or left without a declared step — declare, or undo it), a step that is not one member, a
  cross-group proposal over an electorate group, or a `consensus_electorate` that disagrees with the fleet's
  exclusive default. `detail` says which; nothing was proposed. **409** `electorate_mismatch`.
- `TopologyUnsatisfied` — quorum has the right headcount but the Hard topology
  policy (e.g. "must span two racks") was not satisfied. Retry is unlikely to
  help unless nodes rejoin from the missing segments. If availability matters more than the spread
  for a while, an operator can relax the group's gate with `POST /gateway/govern/topology-override
  {"group": "G", "override": true}` (`govern:write`), which does not lapse — release it with
  `"override": false` ([04 § Hard topology](04-consensus.md)). Over HTTP this refusal is **409**
  `{"error": "topology_unsatisfied"}`.

---

## `RpcError`

```rust
pub enum RpcError {
    Timeout,  // no reply before the deadline
}
```

**When you see it:** `ServiceHandle::rpc_call`.

**Recoverability:** yes. The target node may be slow or temporarily
unreachable. Retry against the same node, or resolve a different provider via
`capabilities().resolve(...)` and retry there.

---

## `QuorumError`

```rust
pub enum QuorumError {
    Timeout { acks_received: usize }, // fewer peers ACKed than requested
}
```

**When you see it:** `KvHandle::set_with_min_acks`.

**Recoverability:** yes. The write succeeded locally and propagates normally. Note this error is the
*only* outcome of the deprecated `set_with_min_acks`, which watched the gossip stream for evidence the
substrate does not carry (`docs/design/contracts-receipts.md` §1a); `acks_received` will be `0`. Its
replacement `GossipAgent::set_with_replica_sync` asks each peer instead and returns a receipt rather
than an error, with peers that did not answer reported as **unknown**. Either way, do not treat a
timeout as evidence the value was not written, and do not retry the write on the strength of it.

---

## `ReceiptError`

```rust
#[non_exhaustive]
pub enum ReceiptError {
    Conflict { operation_id, expected, found },   // same identity, different content
    DeliveryUnknown { established, awaiting },    // the fate is unknown, not negative
    Rejected(String),                             // refused before anything was applied
    DurabilityNotEstablished { persistence_configured, reason },
}
```

**When you see it:** the receipt-returning writes. Full treatment in
[18 · Contracts & receipts](18-contracts-and-receipts.md).

**Note what is not here.** There is no variant meaning *nothing happened*, deliberately: no verb on
this path can establish that. This is the one error type in the library where the absence of a
variant is part of the contract.

**Recoverability, variant by variant:**

- **`Conflict`** — not recoverable by retrying. The same operation identity was reused with different
  content, and nothing was written. Either you have an identity-minting bug, or this is genuinely a
  new operation and needs a new identity.
- **`DeliveryUnknown`** — **neither recoverable nor a failure.** `established` carries the receipt as
  far as it got, and `awaiting` names what the caller was still waiting for. Retry only if the
  operation is idempotent under its own identity, which is what the identity is for. Never report it
  to a user as a failure.
- **`Rejected`** — recoverable once the request is fixed. This *is* a clean negative, because it is a
  statement about the request rather than about the world.
- **`DurabilityNotEstablished`** — the required-sync write applied nothing and gossiped nothing, so no
  reader, subscriber or peer saw the value from that call. `persistence_configured` distinguishes
  *this node was never able to* from *the attempt failed*. It does **not** promise the value can never
  appear here: the log writes before it syncs, so a later replay may restore bytes from a failed sync.

---

## `ScatterError`

```rust
pub enum ScatterError {
    InsufficientReplies { got: usize, needed: usize },
}
```

**When you see it:** `ServiceHandle::scatter_gather`.

**Recoverability:** yes. Fewer than `min_ok` targets replied before timeout.
Check `got` / `needed` and either retry, reduce `min_ok`, or wait for more
capable peers to join.

---

## `SchemaError`

```rust
pub enum SchemaError {
    InvalidJson(String),                           // bytes are not valid JSON
    NotAnObject { kind: &'static str },            // JSON root is not an object
    InvalidSchemaId { id: String, reason: &'static str }, // malformed schema ID
    Io { path: PathBuf, source: std::io::Error },  // file read error
}
```

**When you see it:** `SchemaHandle::publish_schema`, `force_publish_schema`,
`seed_schemas_from_dir`.

**Recoverability:**
- `InvalidJson` / `NotAnObject` / `InvalidSchemaId` — fix the schema bytes or
  ID; these are always caller errors.
- `Io` — file system error during directory seeding; the `path` field
  identifies the offending file.

Note: `publish_schema` returns `SchemaPublishResult` (not an error type) to
distinguish `Published`, `Unchanged`, and `Conflict` outcomes. `SchemaError`
is only returned when the input itself is malformed.

---

## `BulkError`

```rust
pub enum BulkError {
    Timeout,      // target did not fetch staged payload before deadline
    NoHttpPort,   // caller has no http_port configured in GossipConfig
}
```

**When you see it:** `ServiceHandle::bulk_call`.

**Recoverability:**
- `Timeout` — retry; the target node may have been slow to connect.
- `NoHttpPort` — configuration error; set `GossipConfig::http_port` before
  starting the agent. This will always fail until the config is fixed.

---

## `ShardError`

```rust
pub enum ShardError {
    NoProviders,  // no peers match the capability filter at call time
}
```

**When you see it:** `ServiceHandle::emit_sharded`.

**Recoverability:** yes. Wait for providers to advertise the required
capability and retry, or widen the `CapFilter`.

---

## Propagation strategy

All six handles propagate errors via `?` throughout their implementations.
`GossipError` is the only error type that surfaces from startup and lifecycle
paths; domain errors (`ConsistencyError`, `RpcError`, etc.) only surface from
the specific calls that can fail at runtime.

There is no global error wrapper — callers match exactly the variants they
care about:

```rust
match agent.consensus().consistent_set("seq/head", &b"v2"[..]).await {
    Ok(())                                   => { /* committed */ }
    Err(ConsistencyError::Superseded)        => { /* read current, re-evaluate */ }
    Err(ConsistencyError::Timeout { .. })    => { /* retry */ }
    Err(ConsistencyError::TopologyUnsatisfied) => { /* alert ops */ }
    Err(ConsistencyError::ElectorateUnavailable { .. }) => { /* join the group / wait for the roster; nothing decided */ }
    Err(ConsistencyError::NotAMember { .. })     => { /* join the group; nothing proposed */ }
    Err(_)                                   => { /* #[non_exhaustive]: an unknown error is not a commit */ }
}
```

`unwrap()` inside the library is limited to slice-indexing operations where
the invariant is enforced by the type system, plus a small number of `Mutex`
lock calls where poisoning is recovered rather than propagated.

---

## Relationship diagram

```
GossipAgent lifecycle ──── GossipError
KvHandle ────────────────── (infallible set/get; QuorumError for set_with_min_acks)
MeshHandle ─────────────── (infallible emit / signal_rx)
ConsensusHandle ─────────── ConsistencyError
ServiceHandle ──────────── RpcError, ScatterError, BulkError, ShardError
SchemaHandle ───────────── SchemaError (malformed input)
                           SchemaPublishResult (conflict detection — not an error)
CapabilitiesHandle ──────── (infallible resolve; no runtime errors)
```
