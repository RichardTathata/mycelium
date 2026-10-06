/**
 * Integration tests for the Mycelium TypeScript SDK.
 *
 * Requires a live Mycelium node. Set MYCELIUM_TEST_HOST and MYCELIUM_TEST_PORT
 * to point at it. All tests are skipped when those variables are absent.
 *
 * Start a node with:
 *   cargo run --example three_node_demo
 *   # or: MYCELIUM_ROLE=node cargo run --example three_node_demo
 *
 * Run:
 *   MYCELIUM_TEST_HOST=127.0.0.1 MYCELIUM_TEST_PORT=8300 npm test
 */

import { MyceliumAgent } from "../src/agent";

const TEST_HOST = process.env.MYCELIUM_TEST_HOST;
const TEST_PORT = parseInt(process.env.MYCELIUM_TEST_PORT ?? "0", 10);

const describe_ = TEST_HOST ? describe : describe.skip;
const it_ = TEST_HOST ? it : it.skip;

function agent(): MyceliumAgent {
  return new MyceliumAgent(TEST_HOST!, TEST_PORT, 10_000);
}

describe_("MyceliumAgent — live node tests", () => {
  let a: MyceliumAgent;

  beforeEach(() => {
    a = agent();
  });

  // ── Introspection ──────────────────────────────────────────────────────────

  it_("health returns ok", async () => {
    const h = await a.health();
    expect(h.status).toBe("ok");
    expect(typeof h.node_id).toBe("string");
  });

  it_("stats returns an object", async () => {
    const s = await a.stats();
    expect(typeof s).toBe("object");
  });

  it_("nodeId returns a non-empty string", async () => {
    const id = await a.nodeId;
    expect(id.length).toBeGreaterThan(0);
    expect(id).toContain(":");
  });

  // ── KV store ───────────────────────────────────────────────────────────────

  it_("set and get round-trips bytes", async () => {
    const key = `test/ts/${Date.now()}`;
    const val = Buffer.from("hello from typescript");
    await a.set(key, val);
    const got = await a.get(key);
    expect(got).not.toBeNull();
    expect(got!.toString()).toBe("hello from typescript");
  });

  it_("get returns null for missing key", async () => {
    const got = await a.get(`test/ts/missing/${Date.now()}`);
    expect(got).toBeNull();
  });

  it_("delete tombstones a key", async () => {
    const key = `test/ts/del/${Date.now()}`;
    await a.set(key, Buffer.from("x"));
    await a.delete(key);
    const got = await a.get(key);
    expect(got).toBeNull();
  });

  it_("keys returns prefix-filtered list", async () => {
    const prefix = `test/ts/keys/${Date.now()}`;
    await a.set(`${prefix}/a`, Buffer.from("1"));
    await a.set(`${prefix}/b`, Buffer.from("2"));
    // Give gossip a moment to stabilise.
    await new Promise((r) => setTimeout(r, 100));
    const ks = await a.keys(`${prefix}/`);
    expect(ks.length).toBeGreaterThanOrEqual(2);
  });

  it_("set_with_min_acks with min_acks=0 returns 0 immediately", async () => {
    const key = `test/ts/quorum/${Date.now()}`;
    const n = await a.setWithMinAcks(key, Buffer.from("v"), 0);
    expect(n).toBe(0);
  });

  // ── Capability advertisement ───────────────────────────────────────────────

  it_("advertise_capability returns a handle with a non-empty handleId", async () => {
    const handle = await a.advertiseCapability("ts-test", "ping");
    expect(handle.handleId.length).toBeGreaterThan(0);
    await handle.drop();
  });

  it_("resolve_capability finds advertised capability", async () => {
    const handle = await a.advertiseCapability("ts-test", "resolve-test", {
      attributes: { lang: "typescript" },
    });
    await new Promise((r) => setTimeout(r, 100));
    const providers = await a.resolveCapability("ts-test", "resolve-test");
    expect(providers.length).toBeGreaterThan(0);
    await handle.drop();
  });

  it_("demand returns a DemandStatus", async () => {
    const d = await a.demand("ts-test", "demand-check");
    expect(typeof d.demandPressure).toBe("number");
    expect(d.ns).toBe("ts-test");
    expect(d.name).toBe("demand-check");
  });

  // ── Signal mesh ────────────────────────────────────────────────────────────

  it_("emit returns a boolean", async () => {
    // Since the SDK reads the gateway's `ok` (it read `queued`, which is never sent).
    const queued = await a.emit("ts-test-signal", Buffer.from("hello"), {
      scope: "system",
    });
    expect(typeof queued).toBe("boolean");
  });

  // ── RPC ────────────────────────────────────────────────────────────────────

  // A target the gateway cannot see a caller-context marker for is refused before dispatch (HTTP 412
  // `provider_without_caller_context`, the secure default since the gateway caller-identity work) —
  // it never reaches the point of timing out. This test used to expect a `TimeoutError`.
  it_("rpcCall to an unknown target is refused before dispatch", async () => {
    await expect(
      a.rpcCall("127.0.0.1:1", "echo", Buffer.from("hi"), { timeoutSecs: 0.2 }),
    ).rejects.toThrow(/provider_without_caller_context/);
  });

  // ── Mailbox ────────────────────────────────────────────────────────────────

  // ── Round-trips the contract mocks cannot prove (sweep 2026-10-06) ──────────────────────
  // A mocked `fetch` answers whatever it is sent, so a wrong field name passes it. These run the verb
  // against the node, which refuses a field it does not read.

  it_("rpcServe yields the request's kind, and scatterGather to self collects the reply", async () => {
    const self = await a.nodeId;
    const kind = `ts-test.scatter-echo.${Date.now()}`;
    const server = agent();
    const serve = server.rpcServe(kind);
    const seenKind: string[] = [];
    const served = (async () => {
      for await (const req of serve) {
        seenKind.push(req.kind);
        await server.rpcRespond(req, Buffer.concat([Buffer.from("echo:"), req.payload]));
        break;
      }
    })();
    await new Promise((r) => setTimeout(r, 300)); // the serve stream registers on connect
    let replies: Array<{ sender: string; result: Buffer }>;
    try {
      replies = await a.scatterGather([self], kind, Buffer.from("ping"), { minOk: 1, timeoutSecs: 5 });
    } catch (e) {
      // Release the serve loop so a failure fails the test rather than hanging the run.
      await a.rpcCall(self, kind, Buffer.alloc(0), { timeoutSecs: 2 }).catch(() => undefined);
      await served;
      throw e;
    }
    await served;
    expect(seenKind).toEqual([kind]);
    expect(replies).toHaveLength(1);
    expect(replies[0].sender).toBe(self);
    expect(replies[0].result.toString()).toBe("echo:ping");
  });

  it_("onSignal receives a signal emitted after it subscribed, with its kind", async () => {
    const kind = `ts-test.signal.${Date.now()}`;
    const sub = a.onSignal(kind);
    const first = sub.next();                         // opens the stream
    await new Promise((r) => setTimeout(r, 300));     // and lets the node register it
    expect(await agent().emit(kind, Buffer.from("hello"))).toBe(true);
    const { value } = await first;
    await sub.return(undefined);
    expect(value?.kind).toBe(kind);
    expect(value?.payload.toString()).toBe("hello");
    expect(typeof value?.nonce).toBe("bigint");
  });

  it_("subscribeLog starts at the HLC it is given (inclusive)", async () => {
    const stream = `ts-test-sub-${Date.now()}`;
    const first = await a.append(stream, Buffer.from("one"));
    const second = await a.append(stream, Buffer.from("two"));
    // The gateway keeps `hlc >= since`. Before 0.2.0 the SDK sent `since_hlc`, which the gateway
    // ignored, so this began at zero and yielded "one".
    const sub = a.subscribeLog(stream, { sinceHlc: second });
    const { value } = await sub.next();
    await sub.return(undefined);
    expect(value?.hlc).toBe(second);
    expect(value?.value.toString()).toBe("two");
    expect(first < second).toBe(true);
  });

  it_("an RPC to a kind nobody serves is a TimeoutError", async () => {
    const self = await a.nodeId;
    await expect(
      a.rpcCall(self, `ts-test.unserved.${Date.now()}`, Buffer.alloc(0), { timeoutSecs: 1 }),
    ).rejects.toMatchObject({ name: "TimeoutError" });
  });

  it_("deliverEvent does not throw", async () => {
    const id = await a.nodeId;
    await expect(
      a.deliverEvent(id, "ts-test.task", Buffer.from("payload")),
    ).resolves.not.toThrow();
  });

  // ── Overlay ────────────────────────────────────────────────────────────────

  it_("consistent_set and consistent_get round-trip", async () => {
    const key = `test/ts/overlay/${Date.now()}`;
    await a.consistentSet(key, Buffer.from("consistent-val"));
    const got = await a.consistentGet(key);
    expect(got?.toString()).toBe("consistent-val");
  });

  // An election over a group nobody joined has no electorate and is refused by name (409
  // `electorate_unavailable`, since v2.14.0): absence is not authority. This test used to expect
  // a leader for a fresh, empty group.
  it_("electLeader over an empty group is refused by name", async () => {
    const group = `ts-test-elect-${Date.now()}`;
    await expect(a.electLeader(group)).rejects.toThrow(/electorate_unavailable/);
  });

  it_("append and scanLog round-trip", async () => {
    const stream = `ts-test-log-${Date.now()}`;
    const hlc1 = await a.append(stream, Buffer.from("entry-1"));
    const hlc2 = await a.append(stream, Buffer.from("entry-2"));
    expect(hlc2).toBeGreaterThan(hlc1);

    const entries = await a.scanLog(stream);
    expect(entries.length).toBeGreaterThanOrEqual(2);
    const values = entries.map((e) => e.value.toString());
    expect(values).toContain("entry-1");
    expect(values).toContain("entry-2");
  });

  it_("compactLog does not throw", async () => {
    const stream = `ts-test-compact-${Date.now()}`;
    const hlc = await a.append(stream, Buffer.from("x"));
    await expect(a.compactLog(stream, hlc + 1n)).resolves.not.toThrow();
  });

  // A real timeout needs a target the gateway will dispatch to: this node itself, on a kind nobody
  // serves. `0.3` s is sent as 1 s (whole seconds, rounded up), which the gateway accepts; it used to
  // refuse the fraction with 422, and the SDK read `status` where the gateway sends `ack`.
  it_("emitReliable to a kind nobody serves returns timeout", async () => {
    const self = await a.nodeId;
    const result = await a.emitReliable(self, `ts-test.reliable.${Date.now()}`, Buffer.alloc(0), {
      timeoutSecs: 0.3,
    });
    expect(result).toBe("timeout");
  });

  // An unknown target is a refusal, thrown — never reported as a timeout.
  it_("emitReliable to an unknown target is refused, not reported as a timeout", async () => {
    await expect(
      a.emitReliable("127.0.0.1:1", "ts-test.reliable", Buffer.alloc(0), { timeoutSecs: 1 }),
    ).rejects.toThrow(/provider_without_caller_context/);
  });
});
