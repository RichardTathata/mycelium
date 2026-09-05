/**
 * `consistentSet` / `crossGroupPropose` surface the gateway's `persisted` field (v2.4.2).
 * No node needed — `fetch` is stubbed with the JSON shapes the gateway emits.
 */
import { MyceliumAgent } from "../src/agent";

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

function stub(body: Record<string, unknown>, status = 200): void {
  globalThis.fetch = (async () =>
    new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } })
  ) as typeof fetch;
}

const groups = [{ group: "a", quorum: 0.5 }];

test.each([
  [{ ok: true, persisted: true }, true],
  [{ ok: true, persisted: false }, false],
  [{ ok: true }, null],                       // pre-v2.4.2 gateway: field absent
])("persisted is surfaced: %j → %p", async (body, expected) => {
  stub(body);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  expect(await a.consistentSet("k", Buffer.from("v"))).toEqual({ persisted: expected });
  expect(await a.crossGroupPropose("slot", Buffer.from("v"), groups)).toEqual({ persisted: expected });
});

test("a failed commit still rejects", async () => {
  stub({ ok: false, error: "superseded" }, 409);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  await expect(a.consistentSet("k", Buffer.from("v"))).rejects.toThrow(/409/);
});
