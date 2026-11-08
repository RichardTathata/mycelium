/**
 * The client-signature vector (identity-and-lifecycle I1, D1′), shared with the gateway's Rust tests and the Python
 * SDK: the message rebuilt from its inputs, and the signature the seed's key makes over it.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ClientSigner, clientSignatureMessage, mcpBinding } from "../src/client_sig";
import { argumentsDigest } from "../src/a2a";

const V = JSON.parse(readFileSync(join(__dirname, "..", "..", "tests", "fixtures", "cs", "vector.json"), "utf8"));
const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

test("the message is the vector's", () => {
  const [op, res, digest] = mcpBinding("weather.forecast", V.arguments);
  expect([op, res, hex(digest)]).toEqual([V.operation, "tool:weather.forecast", V.arguments_digest_hex]);
  const msg = clientSignatureMessage(V.principal, op, V.resource, digest, BigInt(V.issued_at_ms), BigInt("0x" + V.instance_hex), BigInt(V.counter));
  expect(hex(msg)).toBe(V.message_hex);
});

test("the signer produces the vector's cs and cn", () => {
  const signer = new ClientSigner(Buffer.from(V.seed_hex, "hex"), V.principal, { instance: BigInt("0x" + V.instance_hex) });
  const [, , digest] = mcpBinding("weather.forecast", V.arguments);
  expect(signer.sign("tools/call", V.resource, digest, { issuedAtMs: V.issued_at_ms, counter: V.counter })).toEqual(V.meta);
});

test("each call takes the next counter and processes draw different instances", () => {
  const a = new ClientSigner(new Uint8Array(32).fill(1), "token:fleet/forecaster");
  const b = new ClientSigner(new Uint8Array(32).fill(1), "token:fleet/forecaster");
  const d = new Uint8Array(32);
  expect([0, 1, 2].map(() => a.sign("tools/call", "tool:x", d).cn.c)).toEqual([0, 1, 2]);
  expect(a.instance).not.toBe(b.instance);
});

test("the canonical arguments are the same in every language", () => {
  // #742's review, finding 1: RFC 8785, as the gateway and the Python SDK compute it.
  for (const c of V.canonical_cases) {
    expect(hex(argumentsDigest(c.arguments))).toBe(c.digest_hex);
  }
});
