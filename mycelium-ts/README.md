# mycelium-ts

TypeScript SDK for the [Mycelium](https://github.com/RichardEko/mycelium) gossip mesh.

Connects to a running Rust Mycelium node over loopback HTTP. No native extension —
the HTTP gateway sidecar adds ~1 ms per call, invisible next to LLM inference latency.

## Installation

```sh
npm install mycelium-ts          # npm registry (when published)

# Or from source:
cd mycelium-ts
npm install
npm run build
```

**Requires Node.js ≥ 18** and a running Mycelium node with `http_port` set.

## Quick start

```typescript
import { MyceliumAgent } from "mycelium-ts";

const agent = new MyceliumAgent("127.0.0.1", 8300);

// Advertise a capability; call .drop() to retract it
const handle = await agent.advertiseCapability("compute", "gpu", {
  attributes: { model: "A100" },
});

const providers = await agent.resolveCapability("compute", "gpu");
console.log(providers); // [{ node_id: "...", ns: "compute", name: "gpu", ... }]

// Emit a signal
await agent.emit("render-job", Buffer.from("payload"), { scope: "system" });

// Subscribe to signals
for await (const sig of agent.onSignal("render-job")) {
  console.log(sig.sender, sig.payload);
  break;
}

await handle.drop();
```

## API reference

### `new MyceliumAgent(host, port, timeout, { token })`

| Parameter | Default | Description |
|-----------|---------|-------------|
| `host` | `"127.0.0.1"` | Gateway host |
| `port` | `7946` | HTTP port the Mycelium node listens on |
| `timeout` | `30_000` | Default request timeout (milliseconds) |

---

### Authentication (gateway bearer)

A node with `gateway_auth_token` (or scoped tokens / OIDC) set answers every `/gateway/*` route
with `401` unless the request carries `Authorization: Bearer <token>`. Every client class takes a
trailing `{ token }` option; when omitted, `MYCELIUM_GATEWAY_TOKEN` is read from `process.env`
(Node only — in a browser pass it explicitly). No token → no header.

```ts
const agent = new MyceliumAgent("10.0.0.5", 8300, 30_000, { token: "…" });
const wiki  = new Wiki("10.0.0.5", 8300, "council", { token: "…" });
const a2a   = new A2aClient("http://10.0.0.5:8300", { token: "…" });
```

The header rides every request including the SSE streams (`onSignal`, `rpcServe`, `mailbox`,
`subscribeLog*`). Under scoped tokens the token must carry the route's scope (`kv:read`,
`mesh:write`, `wiki:*`, … — the node's `docs/operations/rbac.md`). Since 0.1.1; the
`auth.test.ts` suite runs without a node and is CI-gated.

**Who the provider sees (core v3 item 7).** A call this client makes through the gateway
(`rpcCall`, `scatter`, `llm*`, `/mcp` `tools/call`, A2A `send`) reaches the provider with a
node-attested caller context: the provider's `authorized_callers` judges *this client's principal*
(`token:<issuer>/legacy` for `gateway_auth_token`, `token:<issuer>/<name>` for a named token,
`token:<issuer>/#i` for the i-th positional scoped token, `oidc:<idp issuer>/<sub>` for a JWT,
`anonymous` with no token; `<issuer>` is the gateway's `gateway_identity_issuer` or its node id) — never
the gateway node. Raw emissions (`emit`, shard emit, mailbox deliver) carry no principal and cannot reach
an RPC provider as the node. On the serving side, `rpcServe` requests
carry it as `RpcRequest.caller` (`{ principal, via, scopes, attested }`; absent for a direct
in-mesh call). Two new gateway refusals, both meaning "the node will not impersonate": HTTP `412` /
JSON-RPC `-32021` `provider_without_caller_context` (the target node predates item 7 — upgrade it,
or run the gateway with `gateway_caller_profile = legacy` during the rollout) and `-32020`
`caller_context_missing`. The `A2aClient` sends its `token` on `/a2a` too; without one the skill
sees `anonymous`. Operator page: `docs/operations/rbac.md` §7.

### Capability advertisement

#### `advertiseCapability(ns, name, options?) → Promise<CapabilityHandle>`

Advertises a capability on the mesh. Re-asserted every `intervalSecs` so late joiners
discover it. Returns a `CapabilityHandle`; call `.drop()` or use `await using` to retract.

```typescript
const handle = await agent.advertiseCapability("compute", "gpu", {
  intervalSecs: 30,
  attributes: { model: "A100", vramGb: 80 },
  authorizedCallers: ["orchestrator"],  // empty = unrestricted
});

await handle.drop();  // tombstones the KV entry

// or with Symbol.asyncDispose:
await using h = await agent.advertiseCapability("compute", "gpu");
// retracted automatically when the block exits
```

#### `resolveCapability(ns, name, options?) → Promise<object[]>`

Returns all live providers matching `(ns, name)`. Pass `callerId` to respect
`authorizedCallers` restrictions.

```typescript
const providers = await agent.resolveCapability("compute", "gpu", {
  callerId: "orchestrator",
});
// [{ node_id: "127.0.0.1:57001", ns: "compute", name: "gpu", attributes: {...} }]
```

#### `demand(ns, name) → Promise<DemandStatus>`

Returns demand pressure. `demandPressure > 1.0` signals a supply gap.

---

### Signal mesh

#### `emit(kind, payload?, options?) → Promise<boolean>`

Fires a signal into the mesh.

- `options.scope`: `"system"` (default), `"group:NAME"`, or `"node:IP:PORT"`
- Returns `true` if queued for gossip; `false` if the gossip shard was full.

#### `onSignal(kind) → AsyncGenerator<Signal>`

Async generator yielding admitted signals of `kind`.

```typescript
for await (const sig of agent.onSignal("render-job")) {
  console.log(sig.kind, sig.sender, sig.payload, sig.nonce);
  break;
}
```

`Signal` fields: `kind: string`, `sender: string`, `payload: Buffer`, `nonce: bigint`.

---

### RPC

#### `rpcCall(target, method, payload?, options?) → Promise<Buffer>`

Blocking point-to-point RPC call. Throws `TimeoutError` if no reply arrives.

```typescript
const result = await agent.rpcCall("127.0.0.1:57001", "echo", Buffer.from("hello"), {
  timeoutSecs: 5,
});
```

#### `rpcServe(kind) → AsyncGenerator<RpcRequest>`

Async generator yielding incoming RPC requests of `kind`.

```typescript
for await (const req of agent.rpcServe("echo")) {
  await agent.rpcRespond(req, req.payload);
}
```

`RpcRequest` fields: `kind: string`, `nonceHex: string`, `sender: string`, `payload: Buffer`.

#### `rpcRespond(request, result?) → Promise<void>`

Sends a reply to an in-flight RPC request.

#### `scatterGather(targets, method, payload?, options?) → Promise<Array<{sender, result}>>`

Fan-out RPC to multiple targets; waits for at least `minOk` replies.

```typescript
const replies = await agent.scatterGather(
  ["127.0.0.1:57001", "127.0.0.1:57002"],
  "vote",
  Buffer.from("proposal"),
  { minOk: 2, timeoutSecs: 5 },
);
// [{ sender: "127.0.0.1:57001", result: Buffer }, ...]
```

---

### KV store

```typescript
await agent.set("my/key", Buffer.from("value"));   // write + gossip
const val = await agent.get("my/key");             // → Buffer | null
await agent.delete("my/key");                      // tombstone + gossip
const keys = await agent.keys("my/");              // → string[]
const data = await agent.scanPrefix("my/");        // → Record<string, Buffer>
```

All writes are gossiped to peers with last-write-wins (HLC) semantics.

#### `setWithMinAcks(key, value, minAcks, options?) → Promise<number>`

Write `value` and wait for peer acknowledgements. Since the substrate's item 1 PR 4b the gateway **asks** each peer whether it holds the operation, so this now succeeds: `acks_received` counts peers whose store holds this exact write and whose WAL `fdatasync` returned `Ok`. Peers that do not answer are reported as **unknown**, never as "did not persist" — a timeout is not evidence the write failed.
With `minAcks: 0` it is an ordinary local write and returns `0`. Returns the peer count; throws
`TimeoutError` on timeout.

```typescript
const n = await agent.setWithMinAcks("config/endpoint", Buffer.from("https://api.v2/"), 2);
console.log(`${n} peers confirmed`);
```

---

### Mailbox (Actor/Event delivery)

#### `deliverEvent(target, kind, payload?) → Promise<void>`

Delivers a mailbox event to `target`'s mailbox. At-least-once within TTL.

#### `mailbox(kind) → AsyncGenerator<MailboxEvent>`

Streams events of `kind` addressed to this node.

```typescript
for await (const event of agent.mailbox("task.result")) {
  console.log(event.sender, event.payload);
}
```

`MailboxEvent` fields: `kind: string`, `sender: string`, `payload: Buffer`.

---

### Introspection

```typescript
await agent.health();  // → { status: "ok", node_id: "..." }
await agent.stats();   // → { node_id: "...", store_entries: N, ... }
const id = await agent.nodeId;  // cached property
```

---

### Consistency & Ordering Overlay

#### Receipts — what an acknowledgement proves (and what it does not)

Every write verb answers a question, and **the four questions are different facts, not degrees of
confidence on one scale**. Nothing infers a higher rung from a lower one:

| Rung | Question | What reports it from TypeScript |
|---|---|---|
| 1 | did *this node* apply it? | `set` resolving |
| 2 | did *this exact write* cross that node's persistence barrier? | `CommitResult.localDurability` |
| 3 | do named, distinct **peers** hold it on disk? | `setWithMinAcks` |
| 4 | did a **destination** commit the business change? | not a KV verb — an effect adapter's receipt |

Applied is not on-disk. On-disk on one node is not replica sync. Three replicas are not a
destination commit. The full argument is [guide 18](../docs/guide/18-contracts-and-receipts.md);
what matters at the SDK boundary is that the fields below are already this vocabulary.

```ts
const res = await agent.consistentSet("config/endpoint", Buffer.from("https://api.v2/"));

res.persisted;            // rung 2, the v2.4.2 bool — folds "on disk" and "nothing was promised"
res.localDurability;      // rung 2, unfolded: "on_disk" | "buffered" | "not_configured" | "failed"
res.localDurabilityError; // why, when it is "failed"
```

Read each state precisely, because each is a different operational fact:

- **`on_disk`** — the forced `fdatasync` returned. Durability established.
- **`buffered`** — the log took it and the bytes are in the OS page cache: it **survives a process
  crash and is lost to a power failure**. Not a softer way of saying `on_disk`.
- **`not_configured`** — that node has no persistence. Nothing was promised, so nothing is claimed
  — and this is the state `persisted: true` quietly hides.
- **`failed`** — durability was **not established**, which is not the same as *the record is
  absent*: the log writes before it syncs, so a replay may restore it. Nothing is promised either
  way.

**A timeout is not a negative.** `setWithMinAcks` timing out means fewer peers *answered* in time —
unreachable, mid-restart, or already holding a newer value all look the same from here. The write
was applied locally and gossiped either way and **is not rolled back**; do not retry it on the
strength of a timeout. The same rule crosses a domain boundary as `DeliveryUnknownError` in
`federation()`.

#### `consistentSet(key, value)` / `consistentGet(key) → Promise<Buffer | null>`

Linearizable KV: runs a consensus round before writing.

```typescript
const res = await agent.consistentSet("config/endpoint", Buffer.from("https://api.v2/"));
const val = await agent.consistentGet("config/endpoint");
res.persisted;  // true: on the gateway node's disk · false: committed but that node's WAL
                // append failed (anti-entropy repairs it after a restart) · null: pre-v2.4.2 node
```

`consistentSet` and `crossGroupPropose` resolve to a `CommitResult` (since 0.1.1; both resolved
`void` before, so existing callers are unaffected). The commit is cluster-wide either way —
`persisted` is the *gateway node's* local durability, the same flag the Rust API reports.

#### `distributedLock(name, options?) → Promise<LockGuard>`

Acquires a named cluster lock via consensus.

```typescript
const lock = await agent.distributedLock("job-42", { ttlSecs: 30 });
console.log("fencing token:", lock.token);
await lock.release();

// or with Symbol.asyncDispose:
await using lock = await agent.distributedLock("job-42");
// released automatically
```

`LockGuard` fields: `guardId: string`, `token: bigint`, `release()`, `[Symbol.asyncDispose]()`.

#### `electLeader(group) → Promise<string>`

One-shot election for `group`. Returns the elected node's `"ip:port"` string.

#### `append(stream, value?) → Promise<bigint>`

Appends `value` to the named log stream. Returns the HLC timestamp.

#### `scanLog(stream, options?) → Promise<LogEntry[]>`

Range scan over a log stream. Returns `LogEntry[]` sorted by HLC.

`LogEntry` fields: `hlc: bigint`, `value: Buffer`.

#### `compactLog(stream, beforeHlc) → Promise<void>`

Tombstones all entries with `hlc < beforeHlc`.

#### `subscribeLog(stream, options?) → AsyncGenerator<LogEntry>`

Live SSE subscription.

#### `subscribeLogGroup(stream, group) → AsyncGenerator<LogEntry>`

Consumer-group subscription: at most one consumer per group per entry.

#### `emitReliable(target, kind, payload?, options?) → Promise<"acknowledged" | "timeout">`

Sends `payload` and waits for an explicit application-level ACK.

### Federated domains

A **domain** is one independently admitted mesh. Federation is one domain calling a service
another has explicitly *exported* to it — the two meshes never merge, and neither learns the
other's members.

These verbs drive **your own node**, which holds the domain's signing key and the partner's trust
bundle; the SDK never speaks the cross-domain protocol itself. The node must be started with
`with_federation_clients([...])` for a partner, or the verbs answer *no client is configured*.

```ts
const fed = agent.federation();

await fed.domain();                     // { configured: true, domain: "…", exports: [...] }
await fed.partners();                   // [{ domain, link: "ready", last_catalogue }]
await fed.catalog("partner.example");   // the LAST OBSERVED catalogue — no network
await fed.connect("partner.example");   // go and ask; returns the exports granted to us
const reply = await fed.call("partner.example", "invoice.status", "INV-42");
```

**Who the partner sees.** The credential names *the principal your bearer resolved to at your own
gateway* — never the node, never a service account. With no token model configured that principal
is `anonymous`, which is honest and usually not what you want in a partner's records.

**Reading a refusal** — two fields, not the message:

```ts
import { DeliveryUnknownError, FederationError } from "mycelium-ts";

try {
  await fed.call("partner.example", "invoice.submit", body);
} catch (e) {
  if (e instanceof DeliveryUnknownError) {
    // The call MAY HAVE RUN. Not a failure — nobody can say. Retrying it retries the effect.
    console.warn("attempted via", e.attemptedVia);
  } else if (e instanceof FederationError && e.nothingWasSent) {
    await retryLater();                  // refused at our own gateway; nothing crossed
  }
}
```

`e.delivery` is `none` · `refused` · `completed` · `unknown`, and `e.sent` says whether any byte
reached the partner. `{ repeatable: true }` on `call` states that *your* effect tolerates being run
twice — it is the only thing that lets a silent gateway be retried elsewhere, and it defaults to
`false`.

---

## Running the tests

Tests require a live Mycelium node:

```sh
# Start a node on port 8300
cargo run --example three_node_demo

# Install dependencies and run tests
cd mycelium-ts
npm install
MYCELIUM_TEST_HOST=127.0.0.1 MYCELIUM_TEST_PORT=8300 npm test
```

## Gateway endpoint reference

| Method | Endpoint | Description |
|--------|----------|-------------|
| `advertiseCapability` | `POST /gateway/capability/advertise` | |
| `resolveCapability` | `GET /gateway/capability/resolve` | |
| `emit` | `POST /gateway/signal/emit` | |
| `onSignal` | `GET /gateway/signal/sse/{kind}` | SSE stream |
| `demand` | `GET /gateway/demand` | |
| `rpcCall` | `POST /gateway/rpc/call` | |
| `rpcServe` | `GET /gateway/rpc/serve/{kind}` | SSE stream |
| `rpcRespond` | `POST /gateway/rpc/respond` | |
| `scatterGather` | `POST /gateway/scatter` | |
| `get` | `GET /gateway/kv?key=K` | |
| `set` | `POST /gateway/kv` | |
| `delete` | `DELETE /gateway/kv?key=K` | |
| `keys` | `GET /gateway/kv/keys?prefix=P` | |
| `setWithMinAcks` | `POST /gateway/kv/quorum` | |
| `mailbox` | `GET /gateway/mailbox/{kind}` | SSE stream |
| `deliverEvent` | `POST /gateway/mailbox/deliver` | |
| `health` | `GET /health` | |
| `stats` | `GET /stats` | |
| `consistentSet` | `POST /gateway/overlay/consistent/set` | |
| `consistentGet` | `GET /gateway/overlay/consistent/get` | |
| `distributedLock` | `POST /gateway/overlay/lock/acquire` | |
| *(lock release)* | `DELETE /gateway/overlay/lock/{id}` | |
| `electLeader` | `POST /gateway/overlay/elect` | |
| `append` | `POST /gateway/overlay/log/append` | |
| `scanLog` | `GET /gateway/overlay/log/scan` | |
| `compactLog` | `POST /gateway/overlay/log/compact` | |
| `subscribeLog` | `GET /gateway/overlay/log/subscribe` | SSE stream |
| `subscribeLogGroup` | `GET /gateway/overlay/log/group/subscribe` | SSE stream |
| `emitReliable` | `POST /gateway/overlay/emit_reliable` | |
| `federation().domain` | `GET /gateway/federation/domain` | `federation:read` |
| `federation().partners` | `GET /gateway/federation/partners` | `federation:read` |
| `federation().catalog` | `GET /gateway/federation/catalog/{domain}` | last observation, no network |
| `federation().connect` | `POST /gateway/federation/connect` | `federation:invoke` |
| `federation().call` | `POST /gateway/federation/call` | `federation:invoke` |
