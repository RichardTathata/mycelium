# mycelium-py

Python SDK for the [Mycelium](https://github.com/RichardEko/mycelium) gossip mesh.

Connects to a running Rust Mycelium node over loopback HTTP. No native extension —
the HTTP gateway sidecar adds ~1 ms per call, invisible next to LLM inference latency.

## Installation

```sh
pip install mycelium-py            # PyPI (when published)

# Or from source:
cd mycelium-py
pip install -e ".[dev]"
```

**Requires Python ≥ 3.10** and a running Mycelium node with `http_port` set.

## Quick start

```python
import asyncio
from mycelium import MyceliumAgent

async def main():
    agent = MyceliumAgent("127.0.0.1", 8300)

    # Advertise a capability; drop the handle to retract it
    with agent.advertise_capability("compute", "gpu", attributes={"model": "A100"}):
        providers = agent.resolve_capability("compute", "gpu")
        print(providers)  # [{"node_id": "...", "ns": "compute", "name": "gpu", "attributes": {...}}]

        # Emit a signal
        agent.emit("render-job", b"payload", scope="system")

        # Subscribe to signals
        async for sig in agent.on_signal("render-job"):
            print(sig.sender, sig.payload)
            break

asyncio.run(main())
```

## API reference

### `MyceliumAgent(host, port, timeout, *, token=None)`

| Parameter | Default | Description |
|-----------|---------|-------------|
| `host` | `"127.0.0.1"` | Gateway host |
| `port` | `7946` | HTTP port the Mycelium node listens on |
| `timeout` | `30.0` | Default request timeout (seconds) |

---

### Authentication (gateway bearer)

A node with `gateway_auth_token` (or scoped tokens / OIDC) set answers every `/gateway/*` route
with `401` unless the request carries `Authorization: Bearer <token>`. Every handle in this
package takes `token=`; when omitted, the `MYCELIUM_GATEWAY_TOKEN` environment variable is used.
No token → no header (a loopback node with no token model is unchanged).

```python
agent = MyceliumAgent("10.0.0.5", 8300, token="…")          # explicit
agent = MyceliumAgent("10.0.0.5", 8300)                     # MYCELIUM_GATEWAY_TOKEN, if set
Wiki("10.0.0.5", 8300, "council", token="…")                # companions too — same option
```

The header rides the pooled clients *and* the dedicated SSE/stream clients (`on_signal`,
`rpc_serve`, `mailbox`, `subscribe_log*`, A2A streaming). Under scoped tokens the token must
carry the route's scope (`kv:read`, `mesh:write`, `wiki:*`, … — the node's
`docs/operations/rbac.md`). Since 0.2.4.

**Who the provider sees (core v3 item 7).** A call this client makes through the gateway
(`rpc_call`, `scatter`, `llm_*`, `/mcp` `tools/call`, A2A `send`) reaches the provider with a
node-attested caller context: the provider's `authorized_callers` judges *this client's principal*
(`token:<issuer>/legacy` for `gateway_auth_token`, `token:<issuer>/<name>` for a named token,
`token:<issuer>/#i` for the i-th positional scoped token, `oidc:<idp issuer>/<sub>` for a JWT,
`anonymous` with no token; `<issuer>` is the gateway's `gateway_identity_issuer` or its node id) — never
the gateway node. Raw emissions (`emit`, shard emit, mailbox deliver) carry no principal and cannot reach
an RPC provider as the node. On the serving side, `rpc_serve` requests
carry it as `RpcRequest.caller` (`{"principal", "via", "scopes", "attested"}`; `None` for a direct
in-mesh call). Two new gateway refusals, both meaning "the node will not impersonate": HTTP `412` /
JSON-RPC `-32021` `provider_without_caller_context` (the target node predates item 7 — upgrade it,
or run the gateway with `gateway_caller_profile = legacy` during the rollout) and `-32020`
`caller_context_missing`. The `A2aClient` sends its `token` on `/a2a` too; without one the skill
sees `anonymous`. Operator page: `docs/operations/rbac.md` §7.

### Capability advertisement

#### `advertise_capability(ns, name, *, interval_secs, attributes, authorized_callers) → CapabilityHandle`

Advertises a capability on the mesh. Re-asserted every `interval_secs` so late joiners
discover it. Returns a `CapabilityHandle`; call `.drop()` or use as a context manager to retract.

```python
handle = agent.advertise_capability(
    "compute", "gpu",
    interval_secs=30,
    attributes={"model": "A100", "vram_gb": 80},
    authorized_callers=["orchestrator"],  # empty = unrestricted
)
handle.drop()  # tombstones the KV entry

# or
with agent.advertise_capability("compute", "gpu") as h:
    ...  # live inside the block
```

#### `resolve_capability(ns, name, *, caller_id) → list[dict]`

Returns all live providers matching `(ns, name)`. Pass `caller_id` to respect
`authorized_callers` restrictions.

```python
providers = agent.resolve_capability("compute", "gpu", caller_id="orchestrator")
# [{"node_id": "127.0.0.1:57001", "ns": "compute", "name": "gpu", "attributes": {...}}]
```

#### `demand(ns, name) → DemandStatus`

Returns demand pressure: `DemandStatus(ns, name, providers, requirers, demand_pressure)`.
`demand_pressure > 1.0` signals a supply gap.

---

### Signal mesh

#### `emit(kind, payload, *, scope) → bool`

Fires a signal into the mesh.

- `scope`: `"system"` (default), `"group:NAME"`, or `"node:IP:PORT"`
- Returns `True` if queued for gossip; `False` if the gossip shard was full (local delivery still occurred).

#### `on_signal(kind) → AsyncIterator[Signal]`

Async generator yielding admitted signals of `kind` as SSE events.

```python
async for sig in agent.on_signal("render-job"):
    print(sig.kind, sig.sender, sig.payload, sig.nonce)
    break
```

`Signal` fields: `kind: str`, `sender: str`, `payload: bytes`, `nonce: int`.

---

### Presenting a mandate (Boundary H, A1)

A gateway with an execution authority establishes a mandate you present, so policy rules that require one
can be satisfied. You hold a grant signed by the appointing authority, and sign a possession proof with your
own key over the exact request bytes. The SDK computes those bytes, and does not sign:

```python
from mycelium import A2aClient, arguments_digest, mandate_request_bytes

request = mandate_request_bytes("skill.invoke", "skill:depot/dispatch", arguments_digest({"text": message}))
# The holder signs mycelium::mandate::grant::possession_message(grant, request) with its own key,
# using the Rust crate or an equivalent implementation; this SDK computes `request` and does not sign.
client.send("depot/dispatch", message, mandate={"grant": grant, "possession": base64_signature})
```

### RPC

#### `rpc_call(target, method, payload, *, timeout_secs) → bytes`

Blocking point-to-point RPC call. Raises `TimeoutError` if no reply arrives.

```python
result = agent.rpc_call("127.0.0.1:57001", "echo", b"hello", timeout_secs=5)
```

#### `rpc_serve(kind) → AsyncIterator[RpcRequest]`

Async generator yielding incoming RPC requests of `kind`. For each request, call
`rpc_respond` to complete the round-trip.

```python
async for req in agent.rpc_serve("echo"):
    agent.rpc_respond(req, req.payload + b"-reply")
```

`RpcRequest` fields: `kind: str`, `nonce_hex: str`, `sender: str`, `payload: bytes`.

#### `rpc_respond(request, result)`

Sends a reply to an in-flight RPC request.

#### `scatter_gather(targets, method, payload, *, min_ok, timeout_secs) → list[dict]`

Fan-out RPC to multiple targets; waits for at least `min_ok` replies. Raises `TimeoutError`
if the threshold is not met.

```python
replies = agent.scatter_gather(
    ["127.0.0.1:57001", "127.0.0.1:57002"],
    "vote",
    b"proposal",
    min_ok=2,
    timeout_secs=5,
)
# [{"sender": "127.0.0.1:57001", "result": b"yes"}, ...]
```

---

### KV store

```python
agent.set("my/key", b"value")              # write + gossip
val   = agent.get("my/key")               # → bytes | None
agent.delete("my/key")                    # tombstone + gossip
keys  = agent.keys(prefix="my/")          # → list[str]
data  = agent.scan_prefix("my/")          # → dict[str, bytes]
```

All writes are gossiped to peers with last-write-wins (HLC) semantics.

#### `set_with_min_acks(key, value, min_acks, *, timeout_secs=5.0) → int`

Write `value` and wait for peer acknowledgements. Since the substrate's item 1 PR 4b the gateway **asks** each peer whether it holds the operation, so this now succeeds: `acks_received` counts peers whose store holds this exact write and whose WAL `fdatasync` returned `Ok`. Peers that do not answer are reported as **unknown**, never as "did not persist" — a timeout is not evidence the write failed.
With `min_acks=0` it is an ordinary local write and returns `0`. Returns the peer count on success; raises
`TimeoutError` on timeout.
The write is **not** rolled back on timeout.

```python
# Wait for 2 peers to confirm before continuing.
n = agent.set_with_min_acks("config/endpoint", b"https://api.v2/", min_acks=2)
print(f"{n} peers confirmed")

# Immediate local-only write (no peer confirmation required).
agent.set_with_min_acks("key", b"value", min_acks=0)
```

---

### Mailbox (Actor/Event delivery)

#### `deliver_event(target, kind, payload)`

Delivers a mailbox event to `target`'s mailbox at key
`mailbox/{target}/{kind}/{hlc_ts}`. Gossiped to all peers; at-least-once within the TTL.

```python
agent.deliver_event("127.0.0.1:57001", "task.result", b"done")
```

#### `mailbox(kind) → AsyncIterator[MailboxEvent]`

Streams events of `kind` addressed to this node. Events are delivered in HLC-causal
order and tombstoned on delivery (won't reappear after a restart).

```python
async for event in agent.mailbox("task.result"):
    print(event.sender, event.kind, event.payload)
```

`MailboxEvent` fields: `kind: str`, `sender: str`, `payload: bytes`.

---

### Introspection

```python
agent.health()  # → {"status": "ok", "node_id": "..."}
agent.stats()   # → {"node_id": "...", "store_entries": N, "dropped_frames": N}
```

---

### Consistency & Ordering Overlay

Opt-in strong guarantees layered on top of the epidemic substrate. Requires the Mycelium
node to be started with `MYCELIUM_ROLE=overlay` (or any role that calls
`start_consensus_listener`).

#### Receipts — what an acknowledgement proves (and what it does not)

Every write verb here answers a question, and **the four questions are different facts, not degrees
of confidence on one scale**. Nothing infers a higher rung from a lower one:

| Rung | Question | What reports it from Python |
|---|---|---|
| 1 | did *this node* apply it? | `set` returning without raising |
| 2 | did *this exact write* cross that node's persistence barrier? | `CommitResult.local_durability` |
| 3 | do named, distinct **peers** hold it on disk? | `set_with_min_acks` |
| 4 | did a **destination** commit the business change? | not a KV verb — an effect adapter's receipt |

Applied is not on-disk. On-disk on one node is not replica sync. Three replicas are not a
destination commit. The full argument is [guide 18](../docs/guide/18-contracts-and-receipts.md);
what matters at the SDK boundary is that the fields below are already this vocabulary.

```python
res = agent.consistent_set("config/endpoint", b"https://api.v2/")

res.persisted            # rung 2, the v2.4.2 bool — folds "on disk" and "nothing was promised"
res.local_durability     # rung 2, unfolded: "on_disk" | "buffered" | "not_configured" | "failed"
res.on_disk              # True ONLY for "on_disk" — the one state that establishes durability
res.local_durability_error   # why, when it is "failed"
```

Read each state precisely, because each is a different operational fact:

- **`on_disk`** — the forced `fdatasync` returned. Durability established.
- **`buffered`** — the log took it and the bytes are in the OS page cache: it **survives a process
  crash and is lost to a power failure**. Not a softer way of saying `on_disk`.
- **`not_configured`** — that node has no persistence. Nothing was promised, so nothing is claimed
  — and this is the state `persisted: True` quietly hides.
- **`failed`** — durability was **not established**. That is not the same as *the record is
  absent*: the log writes before it syncs, so the bytes may or may not be there and a replay may
  restore them. Nothing is promised in either direction.

**A timeout is not a negative.** `set_with_min_acks` raising `TimeoutError` means fewer peers
*answered* in time — unreachable, mid-restart, or already holding a newer value all look the same
from here. The write was applied locally and gossiped either way, and **is not rolled back**; do not
retry it on the strength of a timeout. The same rule crosses a domain boundary as
`DeliveryUnknown` in `mycelium.federation`.

#### `consistent_set(key, value)` / `consistent_get(key) → bytes | None`

Ballot-serialized (consensus-durable) write: runs a consensus round before writing. Concurrent
writes to the same key are totally ordered by ballot number. `consistent_get` is a local read
and may lag by up to one anti-entropy round.

```python
res = agent.consistent_set("config/endpoint", b"https://api.v2/")
val = agent.consistent_get("config/endpoint")  # → b"https://api.v2/"
res.persisted   # True: on the gateway node's disk · False: committed but that node's WAL
                # append failed (anti-entropy repairs it after a restart) · None: pre-v2.4.2 node
```

`consistent_set` and `cross_group_propose` return a `CommitResult` (since 0.2.4; both returned
`None` before, so existing callers are unaffected). The commit is cluster-wide either way —
`persisted` is the *gateway node's* local durability, the same flag the Rust API reports.

#### `distributed_lock(name, *, ttl_secs=30) → LockGuard`

Acquires a named cluster lock via consensus. Returns a `LockGuard`; use as a context
manager or call `.release()`. The `.token` field is a monotonic fencing token.

```python
with agent.distributed_lock("job-42", ttl_secs=30) as lock:
    print("fencing token:", lock.token)
    # exclusive work here
# released automatically on __exit__

# async context manager also available:
async with agent.distributed_lock("job-42") as lock:
    ...
```

#### `elect_leader(group) → str`

One-shot election for `group`. Returns the elected node's `"ip:port"` string.
All nodes calling concurrently converge on the same winner.

```python
leader = agent.elect_leader("shard-0")
if leader == agent.node_id:   # node_id property returns this node's id string
    start_serving()
```

#### `append(stream, value) → int`

Appends `value` to the named log stream. Returns the HLC timestamp (use as a cursor
for `scan_log` or `subscribe_log`).

```python
hlc = agent.append("events", b"order-placed")
```

#### `scan_log(stream, *, from_hlc=0, to_hlc=2**64-1) → list[LogEntry]`

Range scan over a log stream. Returns `LogEntry(hlc, value)` objects sorted by HLC.

```python
entries = agent.scan_log("events")                   # full log
recent  = agent.scan_log("events", from_hlc=cursor)  # since cursor
```

#### `compact_log(stream, before_hlc)`

Tombstones all entries with `hlc < before_hlc`. Gossips the tombstones to peers.

```python
agent.compact_log("events", checkpoint_hlc)
```

#### `subscribe_log(stream, *, since_hlc=0) → AsyncIterator[LogEntry]`

Live SSE subscription. Yields new entries as they arrive, starting from `since_hlc`.

```python
async for entry in agent.subscribe_log("events"):
    print(entry.hlc, entry.value)
```

#### `subscribe_log_group(stream, group) → AsyncIterator[LogEntry]`

Consumer-group subscription: at most one consumer per group processes an entry at a time.
The offset is persisted in the gossip KV so any node can take over if the holder fails.

```python
async for entry in agent.subscribe_log_group("events", "workers"):
    process(entry.value)
```

#### `emit_reliable(target, kind, payload=b"", *, timeout_secs=5) → str`

Sends `payload` to `target` and waits for an explicit application-level ACK
(the receiver calls `rpc_respond`). Returns `"acknowledged"` or `"timeout"`.

```python
ack = agent.emit_reliable("127.0.0.1:57001", "task.assign", b"payload")
if ack == "timeout":
    retry_or_fail()
```

#### Dataclasses

```python
from mycelium import LogEntry, LockGuard

# LogEntry
entry.hlc    # int — HLC timestamp, use as cursor
entry.value  # bytes

# LockGuard
guard.guard_id  # str — opaque ID used to release via HTTP
guard.token     # int — monotonic fencing token (consensus ballot)
guard.release()           # sync release
await guard.arelease()    # async release
```

### Federated domains

A **domain** is one independently admitted mesh. Federation is one domain calling a service
another has explicitly *exported* to it — the two meshes never merge, and neither learns the
other's members.

These verbs drive **your own node**, which holds the domain's signing key and the partner's trust
bundle. The SDK never speaks the cross-domain protocol itself; the node must be started with
`with_federation_clients([...])` for a partner, or the verbs answer *no client is configured*.

```python
fed = agent.federation()

fed.domain()                       # {"configured": True, "domain": "…", "exports": [...], …}
fed.partners()                     # [{"domain": …, "link": "ready", "last_catalogue": [...]}]
fed.catalog("partner.example")     # the LAST OBSERVED catalogue — no network
fed.connect("partner.example")     # go and ask; returns the exports granted to us
reply = fed.call("partner.example", "invoice.status", "INV-42")
```

**Who the partner sees.** The credential names *the principal your bearer resolved to at your own
gateway* — never the node, never a service account. With no token model configured that principal
is `anonymous`, which is honest and usually not what you want in a partner's records.

**Reading a refusal** — two fields, not the message:

```python
from mycelium import DeliveryUnknown, FederationError

try:
    fed.call("partner.example", "invoice.submit", body)
except DeliveryUnknown as e:
    # The call MAY HAVE RUN. Not a failure — nobody can say. Retrying it retries the effect.
    print("attempted via", e.attempted_via)
except FederationError as e:
    if e.nothing_was_sent:                 # refused at our own gateway; nothing crossed
        retry_later()
```

`e.delivery` is `none` · `refused` · `completed` · `unknown`, and `e.sent` says whether any byte
reached the partner. `repeatable=True` on `call` states that *your* effect tolerates being run
twice — it is the only thing that lets a silent gateway be retried elsewhere, and it defaults to
`False`.

---

## Running the tests

Tests require a live Mycelium node. Start one with the demo binary or a custom config:

```sh
# Start a node on port 8300
cargo run --example three_node_demo  # or any node with http_port=8300

# Run the gateway tests
cd mycelium-py
pip install -e ".[dev]"
MYCELIUM_TEST_HOST=127.0.0.1 MYCELIUM_TEST_PORT=8300 pytest tests/ -v
```

## Gateway endpoint reference

All methods talk to the embedded HTTP gateway on the Rust node:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `advertise_capability` | `POST /gateway/capability/advertise` | |
| `resolve_capability` | `GET /gateway/capability/resolve` | |
| `emit` | `POST /gateway/signal/emit` | |
| `on_signal` | `GET /gateway/signal/sse/{kind}` | SSE stream |
| `demand` | `GET /gateway/demand` | |
| `rpc_call` | `POST /gateway/rpc/call` | |
| `rpc_serve` | `GET /gateway/rpc/serve/{kind}` | SSE stream |
| `rpc_respond` | `POST /gateway/rpc/respond` | |
| `scatter_gather` | `POST /gateway/scatter` | |
| `get` | `GET /gateway/kv?key=K` | |
| `set` | `POST /gateway/kv` | |
| `delete` | `DELETE /gateway/kv?key=K` | |
| `keys` | `GET /gateway/kv/keys?prefix=P` | |
| `set_with_min_acks` | `POST /gateway/kv/quorum` | |
| `mailbox` | `GET /gateway/mailbox/{kind}` | SSE stream |
| `deliver_event` | `POST /gateway/mailbox/deliver` | |
| `health` | `GET /health` | |
| `stats` | `GET /stats` | |
| `consistent_set` | `POST /gateway/overlay/consistent/set` | |
| `consistent_get` | `GET /gateway/overlay/consistent/get` | |
| `distributed_lock` | `POST /gateway/overlay/lock/acquire` | |
| *(lock release)* | `DELETE /gateway/overlay/lock/{id}` | |
| `elect_leader` | `POST /gateway/overlay/elect` | |
| `append` | `POST /gateway/overlay/log/append` | |
| `scan_log` | `GET /gateway/overlay/log/scan` | |
| `compact_log` | `POST /gateway/overlay/log/compact` | |
| `subscribe_log` | `GET /gateway/overlay/log/subscribe` | SSE stream |
| `subscribe_log_group` | `GET /gateway/overlay/log/group/subscribe` | SSE stream |
| `emit_reliable` | `POST /gateway/overlay/emit_reliable` | |
| `federation().domain` | `GET /gateway/federation/domain` | `federation:read` |
| `federation().partners` | `GET /gateway/federation/partners` | `federation:read` |
| `federation().catalog` | `GET /gateway/federation/catalog/{domain}` | last observation, no network |
| `federation().connect` | `POST /gateway/federation/connect` | `federation:invoke` |
| `federation().call` | `POST /gateway/federation/call` | `federation:invoke` |
