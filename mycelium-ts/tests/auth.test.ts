/**
 * Gateway bearer support — no node needed: `fetch` is replaced by a recorder and each
 * client's request must carry `Authorization: Bearer <token>` (explicit option or the
 * MYCELIUM_GATEWAY_TOKEN environment variable), and must NOT carry one when no token is set.
 */
import { MyceliumAgent } from "../src/agent";
import { Wiki } from "../src/wiki";
import { TupleSpace } from "../src/tuple";
import { Blackboard } from "../src/blackboard";
import { PromptSkillClient } from "../src/prompt_skill";
import { A2aClient } from "../src/a2a";
import { sseStream } from "../src/sse";
import { TOKEN_ENV, resolveToken, authHeaders } from "../src/auth";

type Call = { url: string; headers: Record<string, string> };
const calls: Call[] = [];

function headerMap(init?: RequestInit): Record<string, string> {
  const h = new Headers(init?.headers ?? {});
  const out: Record<string, string> = {};
  h.forEach((v, k) => { out[k.toLowerCase()] = v; });
  return out;
}

const realFetch = globalThis.fetch;
beforeEach(() => {
  calls.length = 0;
  delete process.env[TOKEN_ENV];
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    calls.push({ url, headers: headerMap(init) });
    const body = url.includes("/gateway/kv/keys") ? { keys: [] }
      : url.includes("/gateway/kv") ? { value_b64: null, ok: true }
      : url.includes("/gateway/wiki/read") ? { page: null }
      : url.includes("/gateway/tuple/depth") ? { stages: [] }
      : url.includes("/gateway/bb/depth") ? { depth: 0 }
      : url.includes("/gateway/prompts") ? { prompts: [] }
      : url.includes("agent.json") ? { name: "n", url: "u", skills: [] }
      : { found: false, ok: true };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = realFetch; });

const auth = (i = 0) => calls[i]?.headers["authorization"];

describe("auth helpers", () => {
  test("explicit token wins; env is the fallback; empty means none", () => {
    process.env[TOKEN_ENV] = "from-env";
    expect(resolveToken("explicit")).toBe("explicit");
    expect(resolveToken(undefined)).toBe("from-env");
    expect(resolveToken("")).toBeUndefined();
    delete process.env[TOKEN_ENV];
    expect(resolveToken(undefined)).toBeUndefined();
    expect(authHeaders("t")).toEqual({ Authorization: "Bearer t" });
    expect(authHeaders(undefined)).toEqual({});
  });
});

describe("every client sends the bearer", () => {
  test("MyceliumAgent GET / POST / DELETE", async () => {
    const a = new MyceliumAgent("127.0.0.1", 1, 1000, { token: "secret" });
    await a.get("k"); await a.set("k", Buffer.from("v")); await a.delete("k"); await a.keys("");
    expect(calls).toHaveLength(4);
    for (const c of calls) expect(c.headers["authorization"]).toBe("Bearer secret");
    // the JSON content-type survives the merge on POST
    expect(calls[1].headers["content-type"]).toContain("application/json");
  });

  test("MyceliumAgent reads MYCELIUM_GATEWAY_TOKEN when no option is given", async () => {
    process.env[TOKEN_ENV] = "env-token";
    const a = new MyceliumAgent("127.0.0.1", 1, 1000);
    await a.get("k");
    expect(auth()).toBe("Bearer env-token");
  });

  test("no token → no Authorization header (open gateway unchanged)", async () => {
    const a = new MyceliumAgent("127.0.0.1", 1, 1000);
    await a.get("k");
    expect(auth()).toBeUndefined();
  });

  test("Wiki / TupleSpace / Blackboard / PromptSkillClient / A2aClient", async () => {
    await new Wiki("127.0.0.1", 1, "g", { token: "w" }).read("p");
    await new TupleSpace("127.0.0.1", 1, "ns", { token: "t" }).depth();
    await new Blackboard("127.0.0.1", 1, "b", { token: "b" }).depth();
    await new PromptSkillClient("127.0.0.1", 1, 1000, { token: "p" }).list();
    await new A2aClient("http://127.0.0.1:1", { token: "a" }).fetchCard();
    expect(calls.map((c) => c.headers["authorization"])).toEqual(
      ["Bearer w", "Bearer t", "Bearer b", "Bearer p", "Bearer a"],
    );
  });

  test("sseStream carries the headers it is given", async () => {
    globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
      calls.push({ url: input.toString(), headers: headerMap(init) });
      return new Response("data: {\"x\":1}\n\n", { status: 200, headers: { "content-type": "text/event-stream" } });
    }) as typeof fetch;
    const got: unknown[] = [];
    for await (const ev of sseStream({ url: "http://127.0.0.1:1/signals/k", headers: { Authorization: "Bearer s" } }, JSON.parse)) got.push(ev);
    expect(got).toEqual([{ x: 1 }]);
    expect(auth()).toBe("Bearer s");
  });
});
