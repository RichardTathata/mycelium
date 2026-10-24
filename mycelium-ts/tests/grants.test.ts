/**
 * The supervisor's grant verbs (adopter-surface X3's door; X7's *grants* cell): `grantsPending` reads
 * `GET /gateway/grants/pending` and `grantDecide` posts `{decision, ttl_secs?, reason?}` to
 * `POST /gateway/grants/{agent}/{request}`, each under the bearer. No node: fetch is replaced.
 */
import { MyceliumAgent } from "../src/agent";

type Call = { url: string; method: string; body: unknown; auth: string | undefined };
const calls: Call[] = [];
let status = 200;
const pending = [{ key: "grant/node:127.0.0.1:7946/r1", agent: "node:127.0.0.1:7946", request: "r1", tool: "purchase.place",
  arguments_digest: "ab".repeat(32), policy_revision: "rev-1", requested_at_ms: 1, expires_at_ms: 2 }];

const realFetch = globalThis.fetch;
beforeAll(() => {
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers = new Headers(init?.headers ?? {});
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined, auth: headers.get("authorization") ?? undefined });
    if (status !== 200) {
      return new Response(JSON.stringify({ ok: false, error: "not_this_node", detail: "another node's agent" }), { status, headers: { "content-type": "application/json" } });
    }
    const body = url.endsWith("/gateway/grants/pending")
      ? { ok: true, pending, now_ms: 1 }
      : { ok: true, key: "grant/a/r1", operation_id: "op-1", local_durability: "on_disk" };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = realFetch; });
beforeEach(() => { calls.length = 0; status = 200; delete process.env.MYCELIUM_GATEWAY_TOKEN; });

test("grantsPending reads the route under the bearer", async () => {
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  const rows = await agent.grantsPending();
  expect(rows).toEqual(pending);
  expect(calls).toHaveLength(1);
  expect(calls[0].url).toMatch(/\/gateway\/grants\/pending$/);
  expect(calls[0].auth).toBe("Bearer super");
});

test("grantDecide posts the decision and returns the record's receipt", async () => {
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  const answer = await agent.grantDecide("node:127.0.0.1:7946", "r1", "issue", { ttlSecs: 120 });
  expect(answer.local_durability).toBe("on_disk");
  await agent.grantDecide("node:127.0.0.1:7946", "r2", "refuse", { reason: "not today" });
  expect(calls.map((c) => [c.method, c.url.split("/gateway/")[1], c.body])).toEqual([
    ["POST", "grants/node%3A127.0.0.1%3A7946/r1", { decision: "issue", ttl_secs: 120 }],
    ["POST", "grants/node%3A127.0.0.1%3A7946/r2", { decision: "refuse", reason: "not today" }],
  ]);
  expect(calls.every((c) => c.auth === "Bearer super")).toBe(true);
});

test("a bad decision is refused before it is sent, and the gateway's refusal surfaces", async () => {
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5);
  await expect(agent.grantDecide("a", "r", "maybe" as unknown as "issue")).rejects.toThrow(/issue.*refuse/);
  expect(calls).toHaveLength(0);
  status = 409;
  await expect(agent.grantDecide("node:other", "r1", "issue")).rejects.toThrow(/409|not_this_node/);
});
