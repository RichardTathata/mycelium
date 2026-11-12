/**
 * Since the gateway's `kv:write` decision (#549) the raw KV routes — `POST`/`DELETE /gateway/kv`,
 * `POST /gateway/kv/quorum`, `POST /gateway/overlay/consistent/set` — refuse a key in a namespace the
 * substrate or a companion owns with `403 protected_key`, and the log routes (`append`, `compact`) refuse
 * a stream under `cn/`, `wiki/` or `reason/` with `403 protected_stream`, each `message` naming the route
 * to use. The SDK throws `ProtectedKeyError` / `ProtectedStreamError` carrying that message from every
 * verb that reaches those routes. `delete()` used to drop the body and throw `failed: 403`.
 * No node needed — `fetch` is stubbed.
 */
import { MyceliumAgent, ProtectedKeyError, ProtectedKindError, ProtectedStreamError } from "../src/index";

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

let lastMethod = "";
function stub(body: Record<string, unknown>, status: number): void {
  globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
    lastMethod = init?.method ?? "GET";
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

const KEY_DOOR = "prompt templates are written through /gateway/prompts/{ns}/{name} (llm:write)";
const STREAM_DOOR = "this log stream belongs to a component (commitment, wiki, reason) and is written through it";
const agent = () => new MyceliumAgent("127.0.0.1", 1, 1000);
const v = Buffer.from("x");

const kvDoors: Array<[string, (a: MyceliumAgent) => Promise<unknown>]> = [
  ["set", (a) => a.set("prompts/ai/chat", v)],
  ["delete", (a) => a.delete("prompts/ai/chat")],
  ["setWithMinAcks", (a) => a.setWithMinAcks("prompts/ai/chat", v, 1)],
  ["consistentSet", (a) => a.consistentSet("prompts/ai/chat", v)],
];

test.each(kvDoors)("%s throws ProtectedKeyError carrying the gateway's route", async (_name, call) => {
  stub({ ok: false, error: "protected_key", message: KEY_DOOR }, 403);
  const err: any = await call(agent()).catch((e) => e);
  expect(err).toBeInstanceOf(ProtectedKeyError);
  expect(err).toBeInstanceOf(Error);
  expect(err.key).toBe("prompts/ai/chat");
  expect(err.status).toBe(403);
  expect(err.message).toBe(KEY_DOOR);
});

const logDoors: Array<[string, (a: MyceliumAgent) => Promise<unknown>]> = [
  ["append", (a) => a.append("reason/trace", v)],
  ["compactLog", (a) => a.compactLog("reason/trace", 10n)],
];

test.each(logDoors)("%s throws ProtectedStreamError carrying the gateway's message", async (_name, call) => {
  stub({ ok: false, error: "protected_stream", message: STREAM_DOOR }, 403);
  const err: any = await call(agent()).catch((e) => e);
  expect(err).toBeInstanceOf(ProtectedStreamError);
  expect(err.stream).toBe("reason/trace");
  expect(err.message).toBe(STREAM_DOOR);
});

test("the kinds are told apart", async () => {
  stub({ ok: false, error: "protected_key", message: KEY_DOOR }, 403);
  const err: any = await agent().set("grp/x/y", v).catch((e) => e);
  expect(err).not.toBeInstanceOf(ProtectedKindError);
  expect(err).not.toBeInstanceOf(ProtectedStreamError);
});

test("delete surfaces a refusal's body, not only its status", async () => {
  stub({ error: "insufficient scope", required_scope: "kv:write" }, 403);
  const err: any = await agent().delete("app/k").catch((e) => e);
  expect(lastMethod).toBe("DELETE");
  expect(String(err.message)).toContain("insufficient scope");
});

test.each([...kvDoors, ...logDoors])("an ordinary 403 on %s is not mistaken for it (the plant)", async (_name, call) => {
  stub({ error: "insufficient scope", required_scope: "kv:write" }, 403);
  const err: any = await call(agent()).catch((e) => e);
  expect(err).toBeInstanceOf(Error);
  expect(err).not.toBeInstanceOf(ProtectedKeyError);
  expect(err).not.toBeInstanceOf(ProtectedStreamError);
});

// The kv:read decision (2026-11-12, 0.2.10): the raw KV read routes refuse an owned key as the write routes do.
const READ_DOOR = "grant requests are read through GET /gateway/grants/pending and GET /gateway/grants/request/{request} (grant:read)";
const readDoors: Array<[string, (a: MyceliumAgent) => Promise<unknown>, string]> = [
  ["get", (a) => a.get("grant/a/r1"), "grant/a/r1"],
  ["consistentGet", (a) => a.consistentGet("grant/a/r1"), "grant/a/r1"],
  ["keys", (a) => a.keys("grant/"), "grant/"],
  ["scanPrefix", (a) => a.scanPrefix("grant/"), "grant/"],
];

test.each(readDoors)("%s throws ProtectedKeyError carrying the route that reads the namespace", async (_name, call, key) => {
  stub({ ok: false, error: "protected_key", message: READ_DOOR }, 403);
  const err: any = await call(agent()).catch((e) => e);
  expect(lastMethod).toBe("GET");
  expect(err).toBeInstanceOf(ProtectedKeyError);
  expect(err.key).toBe(key);
  expect(err.status).toBe(403);
  expect(err.message).toBe(READ_DOOR);
});

test.each(readDoors)("an ordinary 403 on %s stays the error it was (the plant)", async (_name, call) => {
  stub({ error: "insufficient scope", required_scope: "kv:read" }, 403);
  const err: any = await call(agent()).catch((e) => e);
  expect(err).toBeInstanceOf(Error);
  expect(err).not.toBeInstanceOf(ProtectedKeyError);
  expect(String(err.message)).toContain("failed: 403");
});

test("scanPrefix refuses when a listed key's value is refused", async () => {
  // `cap/` names are listed; their values are not read through the raw route.
  globalThis.fetch = (async (url: unknown) => {
    const u = String(url);
    return u.includes("/gateway/kv/keys")
      ? new Response(JSON.stringify({ keys: ["cap/n/demo/echo"] }), { status: 200 })
      : new Response(JSON.stringify({ ok: false, error: "protected_key", message: "capabilities are resolved through GET /gateway/capability/resolve (cap:read)" }), { status: 403 });
  }) as typeof fetch;
  const err: any = await agent().scanPrefix("cap/").catch((e) => e);
  expect(err).toBeInstanceOf(ProtectedKeyError);
  expect(err.key).toBe("cap/n/demo/echo");
});

test("scheduleLast reads the schedule's own route", async () => {
  let seen = "";
  globalThis.fetch = (async (url: unknown) => {
    seen = String(url);
    return new Response(JSON.stringify({ schedule: "close", last: [{ node: "10.0.0.5:9000", slot: 7, period_ms: 1000, phase_ms: 0 }] }), { status: 200 });
  }) as typeof fetch;
  expect(await agent().scheduleLast("close")).toEqual([{ node: "10.0.0.5:9000", slot: 7, periodMs: 1000, phaseMs: 0 }]);
  expect(seen).toContain("/gateway/schedules/last?id=close");
});

// CR1's follow-up (2026-11-12, 0.2.10): the log read routes refuse a component-owned stream as append and
// compact do, and the three read verbs throw the typed error — the subscriptions before any entry.
const LOG_READ_DOOR = "a reasoning trace is read through GET /gateway/reason/trace/{run_id} (llm:read), where mycelium-reason's routes are merged";
const drain = async (gen: AsyncGenerator<unknown>): Promise<unknown[]> => {
  const out: unknown[] = [];
  for await (const e of gen) out.push(e);
  return out;
};
const logReadDoors: Array<[string, (a: MyceliumAgent) => Promise<unknown>, string]> = [
  ["scanLog", (a) => a.scanLog("reason/run-1/n"), "/gateway/overlay/log/scan"],
  ["subscribeLog", (a) => drain(a.subscribeLog("reason/run-1/n")), "/gateway/overlay/log/subscribe"],
  ["subscribeLogGroup", (a) => drain(a.subscribeLogGroup("reason/run-1/n", "g")), "/gateway/overlay/log/group/subscribe"],
];

function stubSeeing(body: Record<string, unknown>, status: number, seen: string[]): void {
  globalThis.fetch = (async (url: unknown) => {
    seen.push(String(url));
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

test.each(logReadDoors)("%s throws ProtectedStreamError carrying the route that reads the stream", async (_name, call, path) => {
  const seen: string[] = [];
  stubSeeing({ ok: false, error: "protected_stream", message: LOG_READ_DOOR }, 403, seen);
  const err: any = await call(agent()).catch((e) => e);
  expect(new URL(seen[0]).pathname).toBe(path);
  expect(new URL(seen[0]).searchParams.get("stream")).toBe("reason/run-1/n");
  expect(err).toBeInstanceOf(ProtectedStreamError);
  expect(err.stream).toBe("reason/run-1/n");
  expect(err.status).toBe(403);
  expect(err.message).toBe(LOG_READ_DOOR);
});

test.each(logReadDoors)("an ordinary 403 on %s stays the error it was (the plant)", async (_name, call) => {
  stubSeeing({ error: "insufficient scope", required_scope: "consensus:read" }, 403, []);
  const err: any = await call(agent()).catch((e) => e);
  expect(err).toBeInstanceOf(Error);
  expect(err).not.toBeInstanceOf(ProtectedStreamError);
  expect(String(err.message)).toContain("403");
});

test("a protected_key answer on a log read is not turned into a stream error", async () => {
  stubSeeing({ ok: false, error: "protected_key", message: "x" }, 403, []);
  const err: any = await agent().scanLog("events").catch((e) => e);
  expect(err).not.toBeInstanceOf(ProtectedStreamError);
  expect(err).not.toBeInstanceOf(ProtectedKeyError);
});
