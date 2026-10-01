/**
 * Unit files from an SDK agent (design-time-tooling.md Q2). `fetch` is stubbed with the JSON
 * `/gateway/units/declare` emits: the file goes out as text, and the handle retracts the unit.
 */
import { MyceliumAgent } from "../src/agent";
import { UnitHandle } from "../src/types";

const realFetch = globalThis.fetch;
afterAll(() => {
  globalThis.fetch = realFetch;
});

let seen: { url: string; method: string; body: unknown }[] = [];
function stub(body: Record<string, unknown>, status = 200): void {
  globalThis.fetch = (async (url: string | URL, init?: RequestInit) => {
    seen.push({ url: url.toString(), method: init?.method ?? "GET", body: init?.body ? JSON.parse(init.body as string) : null });
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as unknown as typeof fetch;
}

const UNIT = 'principal = "sdk-planner"\n[[requirement]]\nns = "data"\nname = "realtime"\n';

test("declareUnits posts the file as text and the handle retracts the unit", async () => {
  seen = [];
  stub({ handle_id: "h1", principal: "sdk-planner", declared: { capabilities: 0, requirements: 1, groups: 0 }, not_enforced: ["[[lane]]"] });
  const h = await new MyceliumAgent("127.0.0.1", 1, 1000).declareUnits(UNIT, { leaseSecs: 10 });
  expect(h).toBeInstanceOf(UnitHandle);
  expect(h.principal).toBe("sdk-planner");
  expect(h.declared.requirements).toBe(1);
  expect(h.notEnforced).toEqual(["[[lane]]"]);
  expect(seen[0].url).toBe("http://127.0.0.1:1/gateway/units/declare");
  expect(seen[0].body).toEqual({ toml: UNIT, interval_secs: 30, lease_secs: 10 });
  stub({ ok: true });
  await h.drop();
  expect(seen[seen.length - 1].method).toBe("DELETE");
  expect(seen[seen.length - 1].url).toBe("http://127.0.0.1:1/gateway/capability/h1");
});
