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
