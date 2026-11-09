/**
 * A grant held by a client, and the SDK's `tools/call` and A2A clients signing as it (identity-and-lifecycle I1).
 * The possession vector — `tests/fixtures/possession/vector.json` — is shared with the gateway's Rust tests (which
 * check it with the gateway's own functions and verifier) and the Python SDK. The clients run against a replaced
 * fetch that records what they sent; the signatures in it are verified here, as the provider would.
 */
import { createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { A2aClient, argumentsDigest, mandateRequestBytes, taskParams } from "../src/a2a";
import { ClientSigner, a2aBinding, clientSignatureMessage, mandateCanonicalBytes, mandateDigest, mcpBinding, possessionMessage, type SignedMandateGrant } from "../src/client_sig";
import { GrantNotIssuedError, McpClient } from "../src/mcp";

const V = JSON.parse(readFileSync(join(__dirname, "..", "..", "tests", "fixtures", "possession", "vector.json"), "utf8"));
const GRANT = V.grant as SignedMandateGrant;
const HOLDER = Buffer.from(V.holder_seed_hex, "hex");
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

function verifies(keyB64: string, message: Uint8Array, sigB64: string): boolean {
  const key = createPublicKey({ key: Buffer.concat([Buffer.from("302a300506032b6570032100", "hex"), Buffer.from(keyB64, "base64")]), format: "der", type: "spki" });
  return verify(null, message, key, Buffer.from(sigB64, "base64"));
}

type Call = { url: string; method: string; body: any };
const calls: Call[] = [];
let answer: unknown = {};
const realFetch = globalThis.fetch;
beforeAll(() => {
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    const url = typeof input === "string" ? input : input.toString();
    const body = init?.body ? JSON.parse(String(init.body)) : undefined;
    calls.push({ url, method: init?.method ?? "GET", body });
    let out: unknown = answer;
    if (url.endsWith("/mcp")) out = { jsonrpc: "2.0", id: body.id, result: { content: [{ type: "text", text: "sunny" }] } };
    else if (url.endsWith("/a2a")) out = { jsonrpc: "2.0", id: 1, result: { id: "t", status: { state: "completed" }, artifacts: [{ parts: [{ type: "text", text: "done" }] }] } };
    else if (url.endsWith("/gateway/grants/request")) out = { ok: true, request: "r-1", key: "grant/ab/r-1", resource: V.resource, reused: false };
    return new Response(JSON.stringify(out), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = realFetch; });
beforeEach(() => { calls.length = 0; delete process.env.MYCELIUM_GATEWAY_TOKEN; });

const csMessage = (s: ClientSigner, meta: any, [op, res, digest]: [string, string, Uint8Array]) =>
  clientSignatureMessage(s.principal, op, res, digest, BigInt(meta.cn.t), BigInt("0x" + meta.cn.i), BigInt(meta.cn.c));

test("the possession vector is rebuilt from its inputs", () => {
  expect(hex(mandateCanonicalBytes(GRANT.mandate))).toBe(V.mandate_canonical_hex);
  expect(hex(mandateDigest(GRANT.mandate))).toBe(V.mandate_digest_hex);
  expect(hex(argumentsDigest(V.arguments))).toBe(V.arguments_digest_hex);
  const request = mandateRequestBytes(V.operation, V.resource, argumentsDigest(V.arguments));
  expect(hex(request)).toBe(V.request_hex);
  expect(hex(possessionMessage(GRANT.mandate, request))).toBe(V.possession_message_hex);
  expect(verifies(V.issuer_key_b64, mandateCanonicalBytes(GRANT.mandate), Buffer.from(GRANT.signature).toString("base64"))).toBe(true);
});

test("the signer proves possession as the vector does, and only for its own grant", () => {
  const signer = new ClientSigner(HOLDER, V.principal);
  expect(signer.provePossession(GRANT, V.operation, V.resource, V.arguments)).toEqual({ grant: GRANT, possession: V.possession_b64 });
  expect(() => new ClientSigner(HOLDER, "token:fleet/someone-else").provePossession(GRANT, "tools/call", "tool:weather.forecast", {})).toThrow(/held by/);
});

test("a tools/call is signed and presents its grant with a proof for this call", async () => {
  const signer = new ClientSigner(HOLDER, V.principal);
  const mcp = new McpClient("http://127.0.0.1:8300", { token: "fc", signer });
  const result = (await mcp.callTool("weather.forecast", V.arguments, { grant: GRANT, correlation: "run-1" })) as any;
  expect(result.content[0].text).toBe("sunny");
  const { url, body } = calls[calls.length - 1];
  expect([url.split("8300")[1], body.method, body.params.name]).toEqual(["/mcp", "tools/call", "weather.forecast"]);
  const meta = body.params._meta;
  expect(verifies(meta.cs.k, csMessage(signer, meta, mcpBinding("weather.forecast", body.params.arguments)), meta.cs.sig)).toBe(true);
  expect(meta.mandate.grant).toEqual(GRANT);
  const request = mandateRequestBytes("tools/call", "tool:weather.forecast", argumentsDigest(body.params.arguments));
  expect(verifies(V.holder_key_b64, possessionMessage(GRANT.mandate, request), meta.mandate.possession)).toBe(true);
  expect(meta.correlation).toBe("run-1");
});

test("an unsigned client sends no _meta, and an unsafe integer or a grant without a signer is refused before sending", async () => {
  await new McpClient("http://127.0.0.1:8300").callTool("weather.forecast", { city: "Lyon" });
  expect(calls[calls.length - 1].body.params._meta).toBeUndefined();
  const sent = calls.length;
  await expect(new McpClient("http://127.0.0.1:8300", { signer: new ClientSigner(HOLDER, V.principal) }).callTool("weather.forecast", { id: 2 ** 60 })).rejects.toThrow(/2\^53/);
  await expect(new McpClient("http://127.0.0.1:8300").callTool("weather.forecast", {}, { grant: GRANT })).rejects.toThrow(/signer/);
  expect(calls.length).toBe(sent);
});

test("the grant verbs ask and read the caller's own answer", async () => {
  const mcp = new McpClient("http://127.0.0.1:8300", { token: "fc" });
  const asked = await mcp.requestGrant("weather.forecast", { city: "Lyon" }, { argumentsClass: "public", windowSecs: 30 });
  expect(asked.request).toBe("r-1");
  expect(calls[calls.length - 1].body).toEqual({ tool: "weather.forecast", arguments: { city: "Lyon" }, arguments_class: "public", window_secs: 30 });
  answer = { ok: true, state: "issued", grant: GRANT };
  expect((await mcp.waitForGrant("01a1-10.0.0.2:7946")).grant).toEqual(GRANT);
  expect(calls[calls.length - 1].url).toMatch(/\/gateway\/grants\/request\/01a1-10\.0\.0\.2%3A7946$/);
  answer = { ok: true, state: "refused", refusal: { reason: "not today" } };
  await expect(mcp.waitForGrant("r-1")).rejects.toBeInstanceOf(GrantNotIssuedError);
});

test("an A2A send and stream are signed over the skill and the text", async () => {
  const signer = new ClientSigner(HOLDER, V.principal);
  expect(await new A2aClient("http://127.0.0.1:8300", { signer }).send("depot/dispatch", "go")).toBe("done");
  const meta = calls[calls.length - 1].body.params._meta;
  expect(verifies(meta.cs.k, csMessage(signer, meta, a2aBinding("depot/dispatch", "go")), meta.cs.sig)).toBe(true);
  const streamed = (taskParams("t", "depot/dispatch", "go", undefined, signer)._meta as any);
  expect(streamed.cn.c).toBe(meta.cn.c + 1);
  expect(verifies(streamed.cs.k, csMessage(signer, streamed, a2aBinding("depot/dispatch", "go")), streamed.cs.sig)).toBe(true);
  expect(taskParams("t", "depot/dispatch", "go")._meta).toBeUndefined();
});
