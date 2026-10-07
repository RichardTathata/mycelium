# mycelium-ts

TypeScript SDK for [Mycelium](https://github.com/RichardTathata/mycelium) — **A fleet that grows the capabilities it lacks, under rules it can show it kept.**

Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays.

Connects to a running Rust Mycelium node over loopback HTTP. No native extension —
gateway overhead depends on workload and deployment; see the [gateway benchmark](../benches/gateway_overhead.rs).
Choose and pin your node and SDK together using [installation and integration modes](../docs/guide/installation.md).

## Installation

```sh
# From a checkout of the chosen release:
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

// Subscribe first: the node registers the subscription when the stream opens, and a signal emitted
// before that is not delivered to it.
const sub = agent.onSignal("render-job");
const first = sub.next();                      // opens the stream
await new Promise((r) => setTimeout(r, 100));  // let the node register it
await agent.emit("render-job", Buffer.from("payload"), { scope: "cluster" });
const { value: sig } = await first;
console.log(sig!.kind, sig!.sender, sig!.payload);
await sub.return(undefined);

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

#### `declareUnits(tomlText, { intervalSecs?, leaseSecs? }) → Promise<UnitHandle>`

Hands a unit file's text to the node, which validates it and declares its capabilities,
requirements and groups under one handle. Hosting sections (`[hosts]`, `[[presence]]`,
`[[activation]]`, `[[serve]]`) are refused (422); `[[lane]]`, `[[mandate]]`, `[[rule]]` come back in
`notEnforced`.

```typescript
import { readFile } from "node:fs/promises";
const unit = await agent.declareUnits(await readFile("units/matcher.toml", "utf8")); // a food co-op's matcher
console.log(unit.principal, unit.declared, unit.notEnforced);
await unit.drop();  // retracts the whole unit
```

See [guide 10 — Declaring a unit file](../docs/guide/10-language-bridges.md#declaring-a-unit-file-from-python-or-typescript).

---

### Signal mesh

#### `emit(kind, payload?, options?) → Promise<boolean>`

Fires a signal into the mesh.

- `options.scope`: `"cluster"` (default), `"group:NAME"`, or `"node:IP:PORT"` (`"system"` is a deprecated alias)
- Returns the gateway's `ok`: `true` when the signal was handed to local delivery and queued for gossip
  fan-out, `false` when the gossip queue was full (local delivery still occurred). It does not say any
  subscriber ran. Before 0.2.0 this always returned `undefined` (it read a field the gateway never sends).

#### `onSignal(kind) → AsyncGenerator<Signal>`

Async generator yielding admitted signals of `kind`.

```typescript
for await (const sig of agent.onSignal("render-job")) {
  console.log(sig.kind, sig.sender, sig.payload, sig.nonce);
  break;
}
```

`Signal` fields: `kind: string` (from the event's data on a 2.24.0+ gateway, else the SSE event name), `sender: string`, `payload: Buffer`,
`nonce: bigint` (exact — see *64-bit values* below).

---

### RPC

#### `rpcCall(target, method, payload?, options?) → Promise<Buffer>`

Blocking point-to-point RPC call. Throws an error named `TimeoutError` if no reply arrives — the
gateway answers an expired deadline with 504 (0.2.1; before it, a plain `Error` naming the 504).

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

Fan-out RPC to multiple targets; waits for at least `minOk` replies, and throws `TimeoutError` when
fewer arrive. **Fixed in 0.2.1:** every call before it was refused with 400 `missing method` — the
SDK sent the method under `kind`, which the gateway does not read.

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
const rcpt = await agent.set("my/key", Buffer.from("value")); // write + gossip → KvReceipt (rung 1; rung 2 as .localDurability)
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
| 1 | did *this node* apply it? | `set` resolving to a `KvReceipt` (its `operationId`) |
| 2 | did *this exact write* cross that node's persistence barrier? | `KvReceipt.localDurability` for `set`; `CommitResult.localDurability` for the consensus verbs |
| 3 | do named, distinct **peers** hold it on disk? | `setWithMinAcks` |
| 4 | did a **destination** commit the business change? | not a KV verb — an effect adapter's receipt |

Applied is not on-disk. On-disk on one node is not replica sync. Three replicas are not a
destination commit. The full argument is [guide 18](../docs/guide/18-contracts-and-receipts.md);
what matters at the SDK boundary is that the fields below are already this vocabulary.

```ts
const rcpt = await agent.set("config/endpoint", Buffer.from("https://api.v2/")); // since 0.1.2: the receipt, not void
rcpt.operationId;        // rung 1: the write's stable identity on the gateway node
rcpt.localDurability;    // rung 2: "on_disk" | "buffered" | "not_configured" | "failed" (null on a pre-v2.16.0 gateway)

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

One-shot election for `group`. Returns the elected node's `"ip:port"` string. A group with no members
is refused (`electorate_unavailable`, thrown): absence is not authority.

#### `append(stream, value?) → Promise<bigint>`

Appends `value` to the named log stream. Returns the HLC timestamp, exactly.

#### `scanLog(stream, options?) → Promise<LogEntry[]>`

Range scan over a log stream, `[fromHlc, toHlc)`. Returns `LogEntry[]` sorted by HLC. Before 0.2.0 the
bounds were sent under names the gateway ignores, so every scan covered the whole stream, and the
reply could not be read.

`LogEntry` fields: `hlc: bigint`, `value: Buffer`.

#### `compactLog(stream, beforeHlc) → Promise<void>`

Tombstones all entries with `hlc < beforeHlc`. `beforeHlc` is sent as an exact JSON integer.

#### `subscribeLog(stream, options?) → AsyncGenerator<LogEntry>`

Live SSE subscription from `options.sinceHlc`, **inclusive** — the gateway yields entries with
`hlc >= sinceHlc`, so to resume after an entry you handled pass `entry.hlc + 1n`. Before 0.2.0 the cursor was sent under a name the
gateway ignores, so every resume replayed the stream from the beginning.

#### `subscribeLogGroup(stream, group) → AsyncGenerator<LogEntry>`

Consumer-group subscription: at most one consumer per group per entry.

#### `emitReliable(target, kind, payload?, options?) → Promise<"acknowledged" | "timeout">`

Sends `payload` and waits for an explicit application-level ACK. A refusal — an unknown target, a
protected kind — is thrown, never reported as `"timeout"`.

### 64-bit values, timeouts and streams (0.2.0)

- **64-bit values are exact.** The gateway sends HLCs, signal nonces and lock tokens as JSON numbers;
  an HLC is about 1.2 × 10¹⁷, above 2⁵³, so a plain `JSON.parse` rounded it and two HLCs one tick
  apart came back equal. The SDK now parses every response losslessly and returns these as `bigint`;
  it writes a `bigint` in a request body as an exact JSON integer. The wire is unchanged.
- **Timeouts are whole seconds** for `rpcCall`, `scatterGather` and `emitReliable`: a fraction is
  rounded up (minimum 1). The gateway reads these as integers — it refused a fraction with 422 on
  `emitReliable` and silently replaced it with a 10–30 s default on the other two.
- **A stream ends when you stop reading it.** `onSignal`, `subscribeLog`, `mailbox`, `rpcServe` and
  the rest read on demand and close the connection when the loop ends (`break`, `return()`, an error).
  Before 0.2.0 the connection and a background reader kept running after the loop ended, buffering
  every later event. Reading on demand means a slow consumer applies backpressure to the connection;
  **upstream, the node holds at most 256 undelivered signals per subscription, and past that it drops
  the signal** and logs `Signal handler channel full; signal dropped` — the client is not told, so use
  `subscribeLog` or a mailbox for anything that must not be lost. `SseOverflowError` (exported, with
  `sseStream`) bounds only the events parsed from a single network read (`maxPending`, default 1024,
  when you call `sseStream` yourself).

### Migrating from 0.1.x

| Verb | 0.1.x | 0.2.x |
|---|---|---|
| `get(key)` | threw for an absent key | `null` for an absent key |
| `resolveCapability` | the `{ providers }` envelope | the providers array |
| `emit` | `undefined` | the gateway's `ok` (`boolean`) |
| `emitReliable` | `undefined` | `"acknowledged"` / `"timeout"`; a refusal throws |
| `scanLog` / `subscribeLog` | bounds and resume cursor ignored | `fromHlc`/`toHlc`, `sinceHlc` honoured |
| HLCs, nonces, lock tokens | `number` (rounded above 2⁵³) | `bigint`, exact |
| timeouts | fractions refused or replaced by a default | whole seconds, rounded up |
| `scatterGather` | refused 400 by every gateway | works (0.2.1) |
| `RpcRequest.kind` from `rpcServe` | `undefined` | the kind (0.2.1) |
| an RPC or scatter timeout | a plain `Error` naming 504 | an error named `TimeoutError` (0.2.1) |

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

`npm test` runs the node-free suites, including `tests/contract.test.ts`, which pins every request and
response shape against the gateway's handlers with a mocked `fetch`. The live suite needs a node, and
runs in CI against one:

```sh
# Start a node: gossip on 9301, gateway on 9311
GOSSIP_HTTP_PORT=9311 cargo run --bin mycelium -- --port 9301

cd mycelium-ts
npm ci
MYCELIUM_LIVE_REQUIRED=1 MYCELIUM_TEST_HOST=127.0.0.1 MYCELIUM_TEST_PORT=9311 npx jest tests/live
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
| `declareUnits` | `POST /gateway/units/declare` | `cap:write` — a unit file's capabilities, requirements and groups under one handle (plan Q2) |
| `artifacts().publish` | `POST /gateway/artifacts/publish` | `artifact:publish` — one already-signed catalogue line (plan A3) |
