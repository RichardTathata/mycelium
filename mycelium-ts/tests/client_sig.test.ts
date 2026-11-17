/**
 * The client-signature vector (identity-and-lifecycle I1, D1′), shared with the gateway's Rust tests and the Python
 * SDK: the message rebuilt from its inputs, and the signature the seed's key makes over it.
 */
import { createHash, createPublicKey, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { ClientSigner, clientSignatureMessage, grantDecisionMessage, mcpBinding } from "../src/client_sig";
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

test("the decision vector is the same in every language", () => {
  // Identity-and-lifecycle I3: the grant-decision signature, pinned for Rust, jest and pytest.
  const v = JSON.parse(readFileSync(join(__dirname, "..", "..", "tests", "fixtures", "decision", "vector.json"), "utf8"));
  const signer = new ClientSigner(Buffer.from(v.seed_hex, "hex"), v.decider);
  for (const c of v.cases) {
    const msg = grantDecisionMessage(v.decider, v.node, v.agent, c.request, c.decision, c.ttl_secs ?? undefined, c.reason ?? undefined, BigInt(v.t));
    expect(Buffer.from(msg).toString("hex")).toBe(c.message_hex);
    const signed = signer.signGrantDecision(v.node, v.agent, c.request, c.decision, { ttlSecs: c.ttl_secs ?? undefined, reason: c.reason ?? undefined, issuedAtMs: v.t });
    expect(signed).toEqual(c.signed);
  }
});

test("the graded decision vector is the same in every language", () => {
  // Identity-and-lifecycle I7: the grade inside the signed message. A service naming `service` (Ed25519, reproduced
  // exactly), and a consent principal's P-256 decisions (ECDSA is randomised, so each pinned signature is verified).
  const v = JSON.parse(readFileSync(join(__dirname, "..", "..", "tests", "fixtures", "decision", "vector.json"), "utf8"));
  const signer = new ClientSigner(Buffer.from(v.seed_hex, "hex"), v.decider);
  const enc = new TextEncoder();
  for (const c of v.graded.service) {
    const msg = grantDecisionMessage(v.decider, v.node, v.agent, c.request, c.decision, c.ttl_secs ?? undefined, c.reason ?? undefined, BigInt(v.t), c.grade);
    expect(Buffer.from(msg).toString("hex")).toBe(c.message_hex);
    const ungraded = grantDecisionMessage(v.decider, v.node, v.agent, c.request, c.decision, c.ttl_secs ?? undefined, c.reason ?? undefined, BigInt(v.t));
    const len = Buffer.alloc(4);
    len.writeUInt32LE(enc.encode(c.grade).length);
    expect(Buffer.from(msg)).toEqual(Buffer.concat([Buffer.from(ungraded), len, Buffer.from(enc.encode(c.grade))]));
    const signed = signer.signGrantDecision(v.node, v.agent, c.request, c.decision, { ttlSecs: c.ttl_secs ?? undefined, reason: c.reason ?? undefined, issuedAtMs: v.t, grade: c.grade });
    expect(signed).toEqual(c.signed);
  }
  const consent = v.graded.consent;
  const key = createPublicKey(consent.spki_pem);
  expect(consent.cases.map((c: { grade: string }) => c.grade).sort()).toEqual(["os_presence(macos, unbound)", "recording", "tty"]);
  for (const c of consent.cases) {
    expect(c.signed.grade).toBe(c.grade);
    expect(Buffer.from(c.signed.k, "base64").toString("hex")).toBe(consent.point_hex);
    const msg = grantDecisionMessage(consent.decider, v.node, v.agent, c.request, c.decision, c.ttl_secs ?? undefined, c.reason ?? undefined, BigInt(v.t), c.grade);
    expect(Buffer.from(msg).toString("hex")).toBe(c.message_hex);
    const sig = Buffer.from(c.signed.sig, "base64");
    expect(verify("sha256", msg, key, sig)).toBe(true);
    // A relay altering the grade: the signature is over the grade it was signed with.
    for (const other of ["tty", "os_presence(macos, unbound)", "recording", "service", undefined]) {
      if (other === c.grade) continue;
      const altered = grantDecisionMessage(consent.decider, v.node, v.agent, c.request, c.decision, c.ttl_secs ?? undefined, c.reason ?? undefined, BigInt(v.t), other);
      expect(verify("sha256", altered, key, sig)).toBe(false);
    }
  }
});

test("a service signer names the grade service or none", () => {
  // I7: a ClientSigner's key is a client certificate's Ed25519 key; a consent grade is a consent key's.
  const signer = new ClientSigner(Buffer.alloc(32, 7), "token:fleet/treasurer");
  expect("grade" in signer.signGrantDecision("n", "a", "r", "issue", { ttlSecs: 60, issuedAtMs: 1 })).toBe(false);
  expect(signer.signGrantDecision("n", "a", "r", "issue", { ttlSecs: 60, issuedAtMs: 1, grade: "service" }).grade).toBe("service");
  for (const grade of ["tty", "os_presence(macos, unbound)", "recording", ""]) {
    expect(() => signer.signGrantDecision("n", "a", "r", "issue", { ttlSecs: 60, issuedAtMs: 1, grade: grade as "service" })).toThrow();
  }
});
