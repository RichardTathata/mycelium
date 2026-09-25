/**
 * Closure plan C1: the gateway refuses protected RPC kinds on its raw mesh routes with
 * `403 protected_kind`; the SDK throws `ProtectedKindError` naming the kind. Also pins the
 * `rpcCall` body: the route reads `method`, and this SDK sent `kind` until 2026-09-25.
 * No node needed — `fetch` is stubbed.
 */
import { MyceliumAgent, ProtectedKindError } from "../src/index";

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

let lastBody: Record<string, unknown> = {};
function stub(body: Record<string, unknown>, status: number): void {
  globalThis.fetch = (async (_url: unknown, init?: RequestInit) => {
    lastBody = JSON.parse(String(init?.body ?? "{}"));
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

test("rpcCall sends `method`, the field the route reads", async () => {
  stub({ ok: true, result_b64: Buffer.from("r").toString("base64") }, 200);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  await a.rpcCall("127.0.0.1:1", "echo", Buffer.from("hi"));
  expect(lastBody.method).toBe("echo");
  expect(lastBody.kind).toBeUndefined();
});

test("a protected kind throws ProtectedKindError naming it", async () => {
  stub({ ok: false, error: "protected_kind", kind: "mcp.invoke",
         message: "`mcp.invoke` is protected work and is not accepted on raw mesh routes; use /mcp (tools/call)" }, 403);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  const err = await a.rpcCall("127.0.0.1:1", "mcp.invoke").catch((e) => e);
  expect(err).toBeInstanceOf(ProtectedKindError);
  expect(err.kind).toBe("mcp.invoke");
  expect(String(err.message)).toContain("/mcp");
  await expect(a.deliverEvent("127.0.0.1:1", "mcp.invoke")).rejects.toBeInstanceOf(ProtectedKindError);
});

test("an ordinary 403 is not mistaken for it (the plant)", async () => {
  stub({ error: "insufficient scope", required_scope: "mesh:write" }, 403);
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  const err = await a.rpcCall("127.0.0.1:1", "echo").catch((e) => e);
  expect(err).not.toBeInstanceOf(ProtectedKindError);
});
