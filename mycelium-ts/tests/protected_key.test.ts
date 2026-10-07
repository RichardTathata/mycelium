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
