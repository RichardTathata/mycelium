/**
 * Adopter-surface X4 in the TypeScript SDK: `set` carries `correlation` and `parent` at the body's top level and the
 * receipt echoes what the gateway answered; nothing is sent, and `null` comes back, when neither is given.
 */
import { MyceliumAgent } from "../src/index";

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

let lastBody: Record<string, unknown> = {};
function stub(body: Record<string, unknown>): void {
  globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
    lastBody = init?.body ? JSON.parse(String(init.body)) : {};
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

const agent = () => new MyceliumAgent("127.0.0.1", 1, 1000);
const h = "cd".repeat(32);

test("set carries the causal fields and the receipt echoes them", async () => {
  stub({ ok: true, operation_id: "op-1", local_durability: "on_disk", correlation: "run-1", parent: h });
  const r = await agent().set("k", Buffer.from("v"), { correlation: "run-1", parent: h });
  expect(lastBody.correlation).toBe("run-1");
  expect(lastBody.parent).toBe(h);
  expect(r.correlation).toBe("run-1");
  expect(r.parent).toBe(h);
});

test("set sends neither field when none is given, and the receipt says so", async () => {
  stub({ ok: true, operation_id: "op-2", local_durability: "on_disk" });
  const r = await agent().set("k", Buffer.from("v"));
  expect("correlation" in lastBody).toBe(false);
  expect("parent" in lastBody).toBe(false);
  expect(r.correlation).toBeNull();
  expect(r.parent).toBeNull();
});

import { PromptSkillClient } from "../src/prompt_skill";
import { Federation } from "../src/federation";

test("the prompt-skill call carries the fields in the body's _meta, and nothing otherwise", async () => {
  stub({ output: "ok", provider: "127.0.0.1:1" });
  const client = new PromptSkillClient("127.0.0.1", 1);
  await client.call("demo", "greeting", "x", {}, 1000, { correlation: "run-1", parent: h });
  expect(lastBody._meta).toEqual({ correlation: "run-1", parent: h });
  await client.call("demo", "greeting", "x");
  expect("_meta" in lastBody).toBe(false);
});

test("the federation call carries the fields in the body's _meta", async () => {
  stub({ reply: "ok" });
  const fed = new Federation("http://127.0.0.1:1", {}, 1000);
  await fed.call("partner.example", "depot.read", "x", { correlation: "run-1", parent: h });
  expect(lastBody._meta).toEqual({ correlation: "run-1", parent: h });
});
