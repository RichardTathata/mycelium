/**
 * The supervisor's grant verbs (adopter-surface X3's door; X7's *grants* cell): `grantsPending` reads
 * `GET /gateway/grants/pending` and `grantDecide` posts `{decision, ttl_secs?, reason?}` to
 * `POST /gateway/grants/{agent}/{request}`, each under the bearer. No node: fetch is replaced.
 */
import { MyceliumAgent } from "../src/agent";

type Call = { url: string; method: string; body: unknown; auth: string | undefined };
const calls: Call[] = [];
let status = 200;
let sse = "";
const pending = [{ key: "grant/node:127.0.0.1:7946/r1", agent: "node:127.0.0.1:7946", request: "r1", tool: "purchase.place",
  resource: "tool:purchase.place@127.0.0.1:7946", arguments_digest: "ab".repeat(32), policy_revision: "rev-1", requested_at_ms: 1, expires_at_ms: 2 }];
const SHOWN = { tool: "purchase.place", resource: "tool:purchase.place@127.0.0.1:7946", arguments_digest: "ab".repeat(32), request_expires_at_ms: 2 };
const row = (request: string, agent = "node:127.0.0.1:7946") => ({ ...pending[0], request, agent, key: `grant/${agent}/${request}` });

const realFetch = globalThis.fetch;
beforeAll(() => {
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const headers = new Headers(init?.headers ?? {});
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined, auth: headers.get("authorization") ?? undefined });
    if (status !== 200) {
      return new Response(JSON.stringify({ ok: false, error: "not_a_member", detail: "no member's agent" }), { status, headers: { "content-type": "application/json" } });
    }
    if (url.endsWith("/gateway/grants/stream")) {
      return new Response(sse, { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    if (url.endsWith("/health")) {
      return new Response(JSON.stringify({ status: "ok", node_id: "127.0.0.1:7946" }), { status: 200, headers: { "content-type": "application/json" } });
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

test("grantDecide posts the decision naming the record, and returns the record's receipt", async () => {
  // Identity-and-lifecycle I7-S (D4): a bearer's decision on a record names its tool, resource, digest and expiry.
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  const answer = await agent.grantDecide(row("r1"), "issue", { ttlSecs: 120 });
  expect(answer.local_durability).toBe("on_disk");
  await agent.grantDecide(row("r2"), "refuse", { reason: "not today" });
  expect(calls.map((c) => [c.method, c.url.split("/gateway/")[1], c.body])).toEqual([
    ["POST", "grants/node%3A127.0.0.1%3A7946/r1", { decision: "issue", ttl_secs: 120, shown: SHOWN }],
    ["POST", "grants/node%3A127.0.0.1%3A7946/r2", { decision: "refuse", reason: "not today", shown: SHOWN }],
  ]);
  expect(calls.every((c) => c.auth === "Bearer super")).toBe(true);
});

test("the ids-only form names nothing, and a record lacking a field it names is refused before it is sent", async () => {
  // I7-S: the deprecated ids-only form sends no block (a gateway after I7-S refuses it by default).
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  await agent.grantDecide("node:127.0.0.1:7946", "r1", "issue", { ttlSecs: 120 });
  expect(calls.map((c) => c.body)).toEqual([{ decision: "issue", ttl_secs: 120 }]);
  for (const field of ["tool", "resource", "arguments_digest", "expires_at_ms"]) {
    const r: Record<string, unknown> = row("r1");
    delete r[field];
    await expect(agent.grantDecide(r as never, "issue")).rejects.toThrow(/names tool/);
  }
  expect(calls).toHaveLength(1);
});

test("grantsStream yields the requests and answers it is sent, naming each", async () => {
  sse = 'event: grant_request\ndata: {"request":"r1","agent":"node:127.0.0.1:7946","tool":"deploy","key":"grant/node:127.0.0.1:7946/r1"}\n\n' +
        'event: grant_answered\ndata: {"request":"r1","outcome":"issued","issuer":"token:gw/alice"}\n\n';
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  const got: unknown[] = [];
  for await (const ev of agent.grantsStream()) got.push(ev);
  expect(got).toEqual([
    { event: "grant_request", request: "r1", agent: "node:127.0.0.1:7946", tool: "deploy", key: "grant/node:127.0.0.1:7946/r1" },
    { event: "grant_answered", request: "r1", outcome: "issued", issuer: "token:gw/alice" },
  ]);
  expect(calls[0].url).toMatch(/\/gateway\/grants\/stream$/);
  expect(calls[0].auth).toBe("Bearer super");
});

test("grantRevoke posts to the revoke route under the bearer", async () => {
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "super" });
  await agent.grantRevoke("node:127.0.0.1:7946", "r1", { reason: "freeze" });
  expect(calls.map((c) => [c.method, c.url.split("/gateway/")[1], c.body])).toEqual([
    ["POST", "grants/node%3A127.0.0.1%3A7946/r1/revoke", { reason: "freeze" }],
  ]);
  expect(calls[0].auth).toBe("Bearer super");
});

test("a bad decision is refused before it is sent, and the gateway's refusal surfaces", async () => {
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5);
  await expect(agent.grantDecide(row("r"), "maybe" as unknown as "issue")).rejects.toThrow(/issue.*refuse/);
  expect(calls).toHaveLength(0);
  status = 409;
  await expect(agent.grantDecide(row("r1", "node:other"), "issue")).rejects.toThrow(/409|not_a_member/);
});

test("a signed decision and revocation carry the signer's signature, addressed to the node", async () => {
  // Identity-and-lifecycle I3: with a signer, the body carries `signed` over exactly this decision, addressed to the
  // node `GET /health` names (read once), so a bearer holding only `grant:relay` can carry it.
  const { ClientSigner, grantDecisionMessage } = await import("../src/client_sig");
  const { createPublicKey, verify } = await import("node:crypto");
  const signer = new ClientSigner(new Uint8Array(32).fill(7), "token:fleet/treasurer");
  const agent = new MyceliumAgent("127.0.0.1", 8300, 5, { token: "relay" });
  await agent.grantDecide(row("r1", "token:fleet/forecaster"), "issue", { ttlSecs: 120, signer });
  await agent.grantRevoke("token:fleet/forecaster", "r1", { reason: "season over", signer });
  expect(calls.filter((c) => c.method === "GET").map((c) => c.url.split("8300")[1])).toEqual(["/health"]);
  type Body = { shown?: unknown; signed: { decider: string; node: string; agent: string; request: string; t: number; k: string; sig: string; grade?: string; shown?: typeof SHOWN } };
  const posts = calls.filter((c) => c.method === "POST").map((c) => c.body as Body);
  // I7-S: the issue names the record's block inside the signature, under `service`, and nothing beside it; the
  // revocation names none.
  expect([posts[0].signed.shown, posts[0].signed.grade, posts[0].shown]).toEqual([SHOWN, "service", undefined]);
  expect([posts[1].signed.shown, posts[1].signed.grade]).toEqual([undefined, undefined]);
  const expected: [string, number | undefined, string | undefined, string | undefined, typeof SHOWN | undefined][] = [["issue", 120, undefined, "service", SHOWN], ["revoke", undefined, "season over", undefined, undefined]];
  posts.forEach(({ signed: s }, i) => {
    const [decision, ttl, reason, grade, shown] = expected[i];
    expect([s.decider, s.node, s.agent, s.request]).toEqual(["token:fleet/treasurer", "127.0.0.1:7946", "token:fleet/forecaster", "r1"]);
    const msg = grantDecisionMessage(s.decider, s.node, s.agent, s.request, decision, ttl, reason, BigInt(s.t), grade, shown);
    const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(s.k, "base64")]), format: "der", type: "spki" });
    expect(verify(null, msg, key, Buffer.from(s.sig, "base64"))).toBe(true);
  });
});
