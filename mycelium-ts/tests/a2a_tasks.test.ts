/**
 * `A2aClient.cancel` and `getTask` (adopter-surface X7's *cancellation* cell): `tasks/cancel` and `tasks/get` for
 * one task id, under the bearer, the node's JSON-RPC error thrown. No node: fetch is replaced.
 */
import { A2aClient } from "../src/a2a";

type Seen = { body: { method: string; params: { id: string } }; auth: string | undefined };
const seen: Seen[] = [];
let terminal = false;

const realFetch = globalThis.fetch;
beforeAll(() => {
  globalThis.fetch = (async (_input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const body = JSON.parse(String(init?.body));
    const headers = new Headers(init?.headers ?? {});
    seen.push({ body, auth: headers.get("authorization") ?? undefined });
    const answer = body.method === "tasks/cancel" && terminal
      ? { jsonrpc: "2.0", id: body.id, error: { code: -32002, message: "task completed; cannot cancel" } }
      : { jsonrpc: "2.0", id: body.id, result: { id: body.params.id, status: { state: body.method === "tasks/cancel" ? "canceled" : "working" }, artifacts: [] } };
    return new Response(JSON.stringify(answer), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = realFetch; });
beforeEach(() => { seen.length = 0; terminal = false; delete process.env.MYCELIUM_GATEWAY_TOKEN; });

test("cancel sends tasks/cancel for the task and returns its state", async () => {
  const client = new A2aClient("http://127.0.0.1:8300", { token: "owner" });
  const task = await client.cancel("t-1");
  expect(task.status.state).toBe("canceled");
  expect(seen[0].body.method).toBe("tasks/cancel");
  expect(seen[0].body.params).toEqual({ id: "t-1" });
  expect(seen[0].auth).toBe("Bearer owner");
});

test("getTask reads the task, and a terminal cancel throws", async () => {
  const client = new A2aClient("http://127.0.0.1:8300");
  expect((await client.getTask("t-2")).status.state).toBe("working");
  expect(seen[seen.length - 1].body.method).toBe("tasks/get");
  terminal = true;
  await expect(client.cancel("t-2")).rejects.toThrow(/cannot cancel/);
});
