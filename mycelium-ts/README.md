# mycelium-ts

TypeScript SDK for [Mycelium](https://github.com/RichardTathata/mycelium) — **A fleet that grows the capabilities it lacks, under rules it can show it kept.**

Mycelium is an embedded library for agent fleets with no coordinator. Generic nodes install signed capabilities when demand goes unmet and re-heal what they declared when a provider dies. Where you configure an enforcement point, it checks authority before work runs and records what it decided; a recorded run replays.

Connects to a running Rust Mycelium node over loopback HTTP. No native extension —
gateway overhead depends on workload and deployment; see the [gateway benchmark](../benches/gateway_overhead.rs).
Choose and pin your node and SDK together using [installation and integration modes](../docs/guide/installation.md).

## Installation

No toolchain is needed for the node: it is a [published release](../docs/guide/installation.md#the-published-node).
The scaffolds `mycelium init` writes are Rust and Python today ([the first hour](../docs/guide/tutorials/07-the-first-hour.md)
walks the Python one); a TypeScript agent starts from the Python scaffold's `node.toml` and this SDK:

```sh
mycelium init my-fleet --template governed-call-py      # node.toml with a named-token table, and a unit to declare
mycelium -c my-fleet/node.toml &                         # the published node
```

The SDK, from a checkout of the chosen release (also the fallback for developing it):

```sh
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

### `new MyceliumAgent(host, port, timeout, { token, scheme })`

| Parameter | Default | Description |
|-----------|---------|-------------|
| `host` | `"127.0.0.1"` | Gateway host |
| `port` | `7946` | HTTP port the Mycelium node listens on |
| `timeout` | `30_000` | Default request timeout (milliseconds) |
| `token` | — | Gateway bearer; `MYCELIUM_GATEWAY_TOKEN` when unset ([below](#authentication-gateway-bearer)) |
| `scheme` | `"http"` | `"https"` for a gateway serving TLS ([below](#reaching-a-gateway-over-tls)) |

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
`subscribeLog*`). Under scoped tokens the token must carry the route's scope **by name** (`kv:read`,
`mesh:write`, `wiki:read` / `wiki:write`, … — a family wildcard such as `wiki:*` is refused by the
node's `validate()`; the table is the node's `docs/operations/rbac.md`). Since 0.1.1; the
`auth.test.ts` suite runs without a node and is CI-gated.

### Reaching a gateway over TLS

A gateway serving HTTPS — the node's own `gateway_tls` (`docs/operations/gateway-tls.md`) or a
TLS-terminating proxy in front of it — is reached with `{ scheme: "https" }`. Every client class
takes it in its trailing options (`MyceliumAgent`, `Wiki`, `TupleSpace`, `Blackboard`,
`PromptSkillClient`; `A2aClient` takes a full URL, so the scheme is in it). The default is
`"http"`, so an existing loopback deployment is unchanged; it is case-insensitive at run time, and an
IPv6 host is bracketed for you (`"::1"` → `https://[::1]:9443`).

Certificate verification is Node's own and stays **on** — this SDK has no option to turn it off. A
private fleet CA (in the node-cert mode of `gateway_tls`, the cluster CA's `ca-cert.pem`, whose
certificate carries an IP SAN, so connect by the IP it names) is trusted by starting the process
with it in `NODE_EXTRA_CA_CERTS`; the built-in `fetch` takes no per-call CA option without a
dependency on `undici`, which this SDK deliberately does not add.

```ts
const agent = new MyceliumAgent("10.0.0.5", 9443, 30_000, { scheme: "https", token: "…" });
const wiki  = new Wiki("10.0.0.5", 9443, "council", { scheme: "https" });
const a2a   = new A2aClient("https://10.0.0.5:9443");
```
```sh
NODE_EXTRA_CA_CERTS=mycelium-tls/ca-cert.pem node app.js     # a pinned fleet CA
```

Since 0.2.3 — before it every client hard-coded `http://`, so a TLS gateway was unreachable from
this package and the bearer travelled in cleartext off loopback. `baseUrl(host, port, scheme)` is
the one place the URL is built; `scheme.test.ts` runs without a node.

**Who the provider sees (core v3 item 7).** A call this client makes through the gateway
(`rpcCall`, `scatterGather`, `PromptSkillClient.call`, `/mcp` `tools/call`, A2A `send`) reaches the provider with a
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

#### `scheduleRuns(decl, { signal?, onStatus? }) → AsyncGenerator<ScheduleRun>` / `scheduleAck(run, error?)` / `runSchedule(decl, work, { signal?, onStatus?, onAckError? })`

Declare a `[[schedule]]` through the gateway and receive its runs (adopter-surface Y12, scope `schedule:run`). The node
this agent is connected to stands in the schedule's holder band while the stream is open, so it must match `holders`:

```typescript
await agent.advertiseCapability("ledger", "closer", { intervalSecs: 5 }); // put the node in the band
await agent.runSchedule(
  { id: "close-day", periodSecs: 86_400, onMissed: "run_once", holders: { ns: "ledger", name: "closer" } },
  async (run) => closeTheDay(run.operationId), // the key every effect of the run carries
);
```

Answer every run within one period (`runSchedule` does: resolved is done, thrown is the work's failure). A run you
never answer — the stream closed, the process went away — is traced `unacknowledged` and its slot is not recorded, so
the next holder decides it. A second answer, or one from another principal, throws `UnservedRunError`; a declaration
the loader refuses, or an id the node already runs, throws `ScheduleRefusedError`. A static-token gateway needs `gateway_identity_issuer`
set fleet-wide, or the stream is refused (`ScheduleRefusedError`, `issuer_not_fleet_wide`). In `runSchedule` an answer that
does not land (refused, or lost to the network) is a missed run, given to `onAckError`, and the schedule goes on. A
client that stays connected but stalls makes its node stand down for ten periods (at most ten minutes) so another node
holds — only while another candidate is in view. `onStatus` is
told why a stream hands out nothing (`not_eligible`: the node does not match `holders`; `warming_up`; `not_holder`;
`stood_down`). A partition runs a slot on each side under one key: carry `operationId` to a destination that
deduplicates. Values are written as TOML basic strings (`tomlBasicString`).

#### `scatterGather(targets, method, payload?, options?) → Promise<Array<{sender, result}>>`

Fan-out RPC to multiple targets; waits for at least `minOk` replies, and throws `TimeoutError` when
fewer arrive. `minOk` defaults to **1** and `timeoutSecs` to 10 — the gateway's own defaults and the Python
SDK's: the call returns at the first reply and the other targets are cancelled. Pass
`minOk: targets.length` to hear from every target. **Changed in 0.2.4:** this SDK alone defaulted to every
target and 5 s. **Fixed in 0.2.1:** every call before it was refused with 400 `missing method` — the
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

These routes write **application keys**: your own namespaces, plus `ckpt/`, `ckptw/`, `manifest/`, `schemas/` and an application's `agent/{node}/provision/…` report. Every other namespace in [`src/lib.rs` § KV namespace ownership](https://github.com/RichardTathata/mycelium/blob/main/src/lib.rs) is owned — including names an application might take for its own, such as `agent/`, `tools/`, `facts/`, `knowledge/`, `svc/`, `comp/`, `tuple/` and `rights/`.
A key in a namespace the substrate or a companion owns — `sys/`, `grp/`, `cap/`, `prompts/`, `log/`,
`mailbox/`, … — is refused: `set`, `delete`, `setWithMinAcks` and `consistentSet` throw
`ProtectedKeyError`, whose message names the route that writes it ([Errors](#errors)).

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

Of two concurrent writes of different values to one key, at most one succeeds; the other throws
`SupersededError` (409 `superseded`, 0.2.4 — a plain `Error` before) or times out (504, a `TimeoutError`),
and a timeout does not mean its value lost — read the key (substrate ≥ 2.30.0). `crossGroupPropose`,
`distributedLock` and `electLeader` throw `SupersededError` the same way; a 409 `topology_unsatisfied` stays
a plain `Error`.

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
Since substrate 2.32.0 the leadership is **leased** (30 s): the leader keeps it by calling again (every
~10 s); a leader that stops — or dies — is no longer reported once the lease lapses. The gateway route
also takes `ttl_secs` or `"permanent": true`, and `DELETE /gateway/overlay/elect/{group}` steps down;
this SDK does not wrap either yet.

#### `append(stream, value?) → Promise<bigint>`

Appends `value` to the named log stream. Returns the HLC timestamp, exactly.

#### `scanLog(stream, options?) → Promise<LogEntry[]>`

Range scan over a log stream, `[fromHlc, toHlc)`. Returns `LogEntry[]` sorted by HLC. Before 0.2.0 the
bounds were sent under names the gateway ignores, so every scan covered the whole stream, and the
reply could not be read.

`LogEntry` fields: `hlc: bigint`, `value: Buffer`.

#### `compactLog(stream, beforeHlc) → Promise<void>`

Tombstones all entries with `hlc < beforeHlc`. `beforeHlc` is sent as an exact JSON integer.

A stream under `cn/`, `wiki/` or `reason/` belongs to a component (the commitment net, a wiki,
`mycelium-reason`): `append` and `compactLog` throw `ProtectedStreamError` for it ([Errors](#errors)).

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

### The supervisor's grants, and a task's lifecycle (0.2.7)

`grantsPending()` reads `GET /gateway/grants/pending` (`grant:read`) and `grantDecide(agent, request, "issue" |
"refuse", { ttlSecs, reason })` posts the answer to `POST /gateway/grants/{agent}/{request}` (`grant:issue`);
issuing is itself an action the node's evaluator decides, so a refusal is the route's 403, and 409 `not_a_member`
names an agent on no member node this gateway knows. A grant is presented by its holder, the agent's node — not by this client. `grantRevoke(agent, request, {
reason })` revokes at the issuing node, and `grantsStream()` is the requests as they come (an async generator of
`grant_request`, `grant_answered` and `grant_expired` events).
`A2aClient.getTask(id)` and `cancel(id)` are `tasks/get` and `tasks/cancel`: the task as the node holds it, readable
and cancellable by the identity that created it; a cancel ends the task's record and stops no work — a task runs
inside its request — and a terminal task throws (adopter-surface X7, the adapter contract).

### The causal fields (0.2.7)

Every verb that starts governed work takes `correlation` (the run's id, one key segment) and `parent` (the content hash
of the record this call depends on): `A2aClient.send`/`stream` (`SendOptions`), `PromptSkillClient.call`'s last
argument, `Federation.call`'s options and `set(key, value, { correlation, parent })`, whose `KvReceipt` echoes them.
They ride in the request body's `_meta` — the application payload, never a header — and onto every evidence record
the gateway writes for the call; `mycelium explain --correlation <id> <evidence.log>` reads the run back as one
chain. A malformed value is refused (`request_malformed`; `400` on `set`). Substrate X4,
`docs/features/causal-records.md`.

### An Eve agent directory (unreleased)

`loadEveAgent(dir)` reads an [Eve](https://www.npmjs.com/package/eve) `agent/` directory installed beside the one
`eve` version the loader pins, **0.80.0**, and refuses a directory beside any other version (adopter-surface Y6). It is a
demonstration, not a compatibility promise. Every slot of the directory has a status in `EVE_SLOTS`, and a slot the
loader does not load is refused by name: `EveRefusedError.refused` lists every one. `sandbox/` and `hooks/` are among
them, and so is a tool with `approval`, which points to the `[[rule]]` with `requires_grant` to declare instead.

- **Tools:** `serveEveTools(agent)` serves the tools as an MCP server on loopback. A node bridges it
  (`connect_mcp_server`), so the fleet's `/mcp` door decides each call before the tool's `execute` runs. Arguments are
  checked with the tool's own Standard Schema.
- **Instructions and skills:** `declareEveAgent(agent, { mycelium, prompts })` declares the unit (`eveUnitToml`,
  `runtime = "typescript"`) and writes the instructions and each Markdown skill as a prompt skill
  `<agent>/<name>`.
- **Schedules (Y12):** each `schedules/<name>.md` or `.ts` whose cron is a period and a phase in UTC (`cronAsPeriod`)
  is a schedule `<agent>.<name>`; `runEveSchedules(agent, mycelium, { onMarkdown, signal })` runs them through the
  gateway (`scheduleRuns`, below), the node in the band the unit's `eve/<agent>` capability names. A `run` handler is
  called with Eve's `waitUntil` and a `to` that throws (channels are not built); a Markdown schedule's prompt goes to
  `onMarkdown`, without which its schedules are refused by name. Another cron, a nested schedule or a key the loader
  does not read is refused by name.

Importing a `.ts` tool uses Node's own type stripping, and Eve 0.80.0 needs Node 24. The run, and the table of every
slot, are in [`examples/eve_weather/README.md`](../examples/eve_weather/README.md).

### A serve stream refused or closed (0.2.6)

`rpcServe(kind)` throws `ProtectedKindError` (naming the kind) when the node refuses the stream: `403 protected_kind`
for `mcp.invoke`, `skill.invoke` and `llm.invoke`, and `403 served_in_process` for any kind the node already serves
with a receiver of its own (substrate post-360 row R). Before 0.2.6 both surfaced as `SSE request failed: 403`. A
stream the node later closes because a receiver of its own took the kind throws `ServeEvictedError` (a
`ProtectedKindError`) — the `for await` never just ends.

### Timer periods (0.2.5)

- **`intervalSecs` and `leaseSecs` are whole seconds in `1..=3600`** (`advertiseCapability`, `declareUnits`),
  as the node now requires (post-360 row E, 2.33.0 — `MAX_TIMER_PERIOD_SECS`, exported; the gateway answers 400
  `timer_out_of_range` naming the field otherwise). A fraction rounds **up**; `0` or a value past 3600 throws,
  naming the option, before any request. Before 0.2.5 `intervalSecs` was sent as given, and the gateway read a
  fraction as its 30 s default and any `u64` as a period.

### Timeouts, path segments and streams (0.2.4)

- **Every timeout is whole seconds now**, as `rpcCall`, `scatterGather` and `emitReliable` have been since
  0.2.0: `TupleSpace.take` / `takeByKey` and `Wiki.ingest` round a fraction **up** too (`take` keeps `0`, the
  poll that answers at once), and so do `leaseSecs` (`advertiseCapability`, `declareUnits` — a fraction left the
  advert unleased) and `ttlSecs` (`distributedLock` — refused 422). Before 0.2.4 a fraction on the takes, `ingest`
  and `ttlSecs` was refused 422. A negative, `NaN`, infinite or past-`u64` value throws, naming the option,
  before any request. (`setWithMinAcks`'s `/gateway/kv/quorum` reads a float and is sent as given.)
- **Caller-supplied path segments are escaped.** A prompt's `ns`/`name` and a capability handle or lock guard id
  are percent-encoded as **one** path segment, as signal kinds, shard names and federation domains already were.
  A value that cannot be one segment throws ("cannot travel as a URL path segment") before any request, on every
  such route: `.` or `..`, which the URL parser resolves as a dot segment even when encoded (WHATWG reads `%2e%2e`
  as `..`); the empty string; a string with a lone surrogate. Each would reach a different route.
- **A dropped stream is not resumed — not built: the gateway has no resume point.** No SSE route sends an
  event `id:` or reads `Last-Event-ID`, so there is nothing for a client to resume *from*, and this SDK does not
  reconnect: when the connection drops, `onSignal`, `rpcServe`, `mailbox`, `subscribeLog` and
  `subscribeLogGroup` end (or throw the network error). What to do today is re-open the stream in a loop,
  knowing what each one loses in the gap:
  - `onSignal` — signals emitted while no stream is open are **not** delivered later; signals are
    best-effort. Use a mailbox or a log for anything that must arrive.
  - `rpcServe` — a request that arrives in the gap reaches no server; its caller times out (and must treat
    the timeout as *unknown*, not *not done*).
  - `mailbox` — the gateway deletes each event as its stream hands it to the HTTP connection (since post-360
    row E; before, as it queued it, up to 256 ahead of your loop), so a re-opened stream delivers everything
    still queued when it dropped, and two streams of one kind never get the same event — though a connected but
    stalled stream holds the events it has claimed from its siblings until it reads them or its connection drops. What the connection had
    already accepted — written to the socket, not yet read by you — is gone. An event that must not be lost needs an acknowledgement of your own — the sender
    keeps it until the receiver confirms — or a log or tuple space instead of a mailbox.
  - `subscribeLog` — pass `sinceHlc: entry.hlc + 1n` for the last handled entry (the cursor is inclusive).
  - `subscribeLogGroup` — re-subscribing contends for the group's claim again and resumes from the group's
    recorded position, which moves when the gateway's stream hands an entry to the HTTP connection (since
    post-360 row E; before, as it queued it), not when you finish it: entries still queued when the stream
    dropped are sent again; one the connection had already accepted is not.

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
| `scatterGather` with no `minOk` | waited for every target, 5 s | waits for one reply, 10 s (0.2.4) — pass `minOk: targets.length` for all |
| a lost consensus write (409 `superseded`) | a plain `Error` naming 409 | `SupersededError` (0.2.4) |

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

### Errors

Four gateway answers are thrown as typed errors. The three refusals are each an `Error` whose `message` is the
gateway's, naming the route to use instead; `SupersededError` is a lost consensus write. Any other non-2xx
answer to a write is an `Error` naming the method, path, status and response body.

| Error | Gateway answer | Thrown by | Fields |
|---|---|---|---|
| `ProtectedKindError` | `403 protected_kind` — `mcp.invoke`, `skill.invoke`, `llm.invoke` (and operator-listed kinds) on a raw mesh route | `rpcCall`, `scatterGather`, `emit`, `deliverEvent`, `emitSharded`, `emitReliable` | `kind` |
| `ProtectedKeyError` (0.2.2) | `403 protected_key` — a key in a namespace the substrate or a companion owns | `set`, `delete`, `setWithMinAcks`, `consistentSet` | `key`, `status` |
| `ProtectedStreamError` (0.2.2) | `403 protected_stream` — a log stream under `cn/`, `wiki/`, `reason/` | `append`, `compactLog` | `stream`, `status` |
| `SupersededError` (0.2.4) | `409 superseded` — the slot was decided for another value (a concurrent writer, holder or leader won) | `consistentSet`, `crossGroupPropose`, `distributedLock`, `electLeader` | `status` |

```typescript
import { ProtectedKeyError } from "mycelium-ts";

try {
  await agent.set("prompts/ai/chat", template);
} catch (e) {
  if (e instanceof ProtectedKeyError) console.log(e.key, "→", e.message);
  else throw e;
}
```

Before 0.2.2 `delete` threw `DELETE /gateway/kv failed: 403` and dropped the response body; it now
carries the body like every other verb.

---

## Running the tests

`npm test` runs the node-free suites, including `tests/contract.test.ts`, which pins every request and
response shape against the gateway's handlers with a mocked `fetch`. The live suite needs a node, and
runs in CI against one:

```sh
# Start a node: gossip on 9301, gateway on 9311 — the published binary (docs/guide/installation.md § The published
# node, no toolchain); from source, `GOSSIP_HTTP_PORT=9311 cargo run --bin mycelium -- --port 9301` instead
GOSSIP_HTTP_PORT=9311 mycelium --port 9301

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

## What this does not establish

The SDK is a client of one gateway; it carries what the doors it calls carry, and nothing it does not send
([the adapter contract](../docs/reference/adapter-contract.md)):

- **No cancellation of running work.** `A2aClient.cancel` reaches `tasks/cancel`, which ends a task's record and
  stops nothing (a task runs inside its request); `tools/call` has no cancel; ending an `sseStream` generator closes
  the client's side only, and a dropped stream is not resumed.
- **Presenting a grant.** A grant's holder is the agent's node, so this SDK's client — a bearer, not the node — cannot
  present one: the supervisor's verbs are carried, the presenting side is the node's own (adopter-surface X3).
- **Identity is the bearer's.** The principal the gateway resolves is the token's; this SDK signs nothing.
- **A receipt names the gateway node's rung and nothing above it** — never replication, never a peer's state.
- **64-bit values are exact and the shapes match the gateway's** (0.2.0); what the gateway does not return — a
  provider's own record, a peer's durability — this SDK cannot show.
- **No session.** The SDK keeps nothing between calls.
- **The Eve loader runs no Eve runtime** (adopter-surface Y6). A tool's `execute` gets an empty context, the
  `agent.ts` model is read and not routed, and a slot the loader does not load is refused by name, never translated.
  The server `serveEveTools` runs decides nothing; the bridging node's `/mcp` door does. One Eve version is read,
  0.80.0. The rest is in [`examples/eve_weather/README.md`](../examples/eve_weather/README.md) § What this does not
  establish.
