/**
 * The client-signature vector (identity-and-lifecycle I1, D1′), shared with the gateway's Rust tests and the Python
 * SDK: the message rebuilt from its inputs, and the signature the seed's key makes over it.
 */
import { createHash } from "node:crypto";
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

test("an integer beyond 2^53 is refused by the signer", () => {
  // #742's confirm round, finding 1: RFC 8785 cannot bind such an integer exactly; send large ids as strings.
  for (const text of V.unsafe_integer_cases) expect(() => mcpBinding("weather.forecast", JSON.parse(text))).toThrow(/2\^53/);
  for (const text of V.safe_integer_cases) expect(() => mcpBinding("weather.forecast", JSON.parse(text))).not.toThrow();
});

test("a name holding an @ is bound whole", () => {
  // #745's confirm round: the resource is signed whole, so a tool named `weather@v2` is not `weather`.
  const c = V.at_case;
  const [op, res, digest] = mcpBinding(c.name, c.arguments);
  expect([res, Buffer.from(digest).toString("hex")]).toEqual([c.resource, c.arguments_digest_hex]);
  const msg = clientSignatureMessage(V.principal, op, res, digest, BigInt(V.issued_at_ms), BigInt("0x" + V.instance_hex), BigInt(V.counter));
  expect(Buffer.from(msg).toString("hex")).toBe(c.message_hex);
  const signer = new ClientSigner(Buffer.from(V.seed_hex, "hex"), V.principal, { instance: BigInt("0x" + V.instance_hex) });
  expect(signer.sign(op, res, digest, { issuedAtMs: V.issued_at_ms, counter: V.counter }).cs.sig).toBe(c.signature_b64);
});

test("RFC 8785 writes every sampled double as ECMAScript does, through the digest", () => {
  // #750's review, item 1: the shared fixture; here through the digest a signature and a mandate bind.
  const fx = JSON.parse(readFileSync(join(__dirname, "..", "..", "tests", "fixtures", "cs", "es_numbers.json"), "utf8"));
  const buf = Buffer.alloc(8);
  for (const [bits, want] of fx.cases as [string, string][]) {
    buf.write(bits, "hex");
    const x = buf.readDoubleBE(0);
    const expected = createHash("sha256").update(`{"x":${want}}`, "utf8").digest("hex");
    expect([bits, Buffer.from(argumentsDigest({ x })).toString("hex")]).toEqual([bits, expected]);
  }
});
