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
  const absent = { persisted: expected, localDurability: null, localDurabilityError: null };
  expect(await a.consistentSet("k", Buffer.from("v"))).toEqual(absent);
  expect(await a.crossGroupPropose("slot", Buffer.from("v"), groups)).toEqual(absent);
});

test.each([
  // v2.8.0 gateway: the receipt vocabulary beside `persisted`.
  [{ ok: true, persisted: true, local_durability: "on_disk" }, "on_disk", null],
  // `persisted: true` also covers "nothing was promised" — the collapse the new field undoes.
  [{ ok: true, persisted: true, local_durability: "not_configured" }, "not_configured", null],
  [{ ok: true, persisted: false, local_durability: "failed", local_durability_error: "no ack" }, "failed", "no ack"],
  [{ ok: true, persisted: true }, null, null],  // pre-v2.8.0 gateway: fields absent
])("local durability is surfaced: %j → %s", async (body, durability, error) => {
  stub(body);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  const results = [
    await a.consistentSet("k", Buffer.from("v")),
    await a.crossGroupPropose("slot", Buffer.from("v"), groups),
  ];
  for (const r of results) {
    expect(r.localDurability).toBe(durability);
    expect(r.localDurabilityError).toBe(error);
    expect(r.persisted).toBe(body.persisted);   // the old field is untouched by the new one
  }
});

test("a failed commit still rejects", async () => {
  stub({ ok: false, error: "superseded" }, 409);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  await expect(a.consistentSet("k", Buffer.from("v"))).rejects.toThrow(/409/);
});
