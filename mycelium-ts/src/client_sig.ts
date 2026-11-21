/**
 * A client's own signature on a gateway call — `cs` and `cn` (identity-and-lifecycle I1, decision D1′).
 *
 * A service holding a client certificate (`mycelium tls issue --client <name> --issuer <issuer>`) signs each
 * `tools/call` or A2A send with the certificate's Ed25519 key and sends the result in `params._meta`; the provider
 * that runs the call checks the signature, that the key is the principal's current one, that it covers this call's
 * operation, resource and arguments, and that its `cn` was not seen before. The signed bytes are
 * {@link clientSignatureMessage}, pinned by `tests/fixtures/cs/vector.json`, which the gateway's Rust tests and the
 * Python SDK check too.
 *
 * The same key proves **possession of a grant held by the client** (`POST /gateway/grants/request`):
 * {@link ClientSigner.provePossession} signs {@link possessionMessage} over the grant's {@link mandateCanonicalBytes}
 * and the call's {@link mandateRequestBytes}, pinned by `tests/fixtures/possession/vector.json`, which the gateway's
 * Rust tests and the Python SDK check too.
 */
import { createHash, createPrivateKey, createPublicKey, randomBytes, sign, type KeyObject } from "node:crypto";
import { argumentsDigest, mandateArgumentsDigest, mandateRequestBytes, type PresentedMandate } from "./a2a";

const TAG = "mycelium.gateway/client-signature/1";

/** `cs`: the signing key and the signature, base64. */
export interface ClientSignature { k: string; sig: string }
/** `cn`: the client's issue time (ms), its process's instance id (16 lowercase hex), and its counter. */
export interface ClientNonce { t: number; i: string; c: number }

/**
 * The bytes a client signs: the tag; the principal, the operation and the resource, each a `u32` little-endian length
 * and its UTF-8 bytes; the 32-byte arguments digest; then `issuedAtMs`, `instance` and `counter` as `u64` little-endian.
 * `resource` is the bare resource (`tool:{name}`, `skill:{id}`), whole: a name holding `@` is bound in full, and no
 * provider is named (one signature serves every provider of the tool).
 */
export function clientSignatureMessage(
  principal: string, operation: string, resource: string, digest: Uint8Array,
  issuedAtMs: bigint, instance: bigint, counter: bigint,
): Uint8Array {
  if (digest.length !== 32) throw new Error("the arguments digest is 32 bytes");
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [enc.encode(TAG)];
  for (const part of [principal, operation, resource]) {
    const b = enc.encode(part);
    const len = new Uint8Array(4);
    new DataView(len.buffer).setUint32(0, b.length, true);
    chunks.push(len, b);
  }
  chunks.push(digest);
  for (const v of [issuedAtMs, instance, counter]) {
    const b = new Uint8Array(8);
    new DataView(b.buffer).setBigUint64(0, v, true);
    chunks.push(b);
  }
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let off = 0;
  for (const c of chunks) { out.set(c, off); off += c.length; }
  return out;
}

const GRANT_DECISION_TAG = "mycelium.gateway/grant-decision/1";

/** A decision's signature, sent as the decision body's `signed` field (identity-and-lifecycle I3). */
export interface SignedDecision {
  decider: string; node: string; agent: string; request: string; t: number; k: string; sig: string;
  /**
   * The grade the signer names, inside the signature (identity-and-lifecycle I7). Absent from a service's body, or
   * `"service"` there; a consent principal's body names its consent certificate's grade.
   */
  grade?: string;
  /** What the decider saw of the request (identity-and-lifecycle I7-S), inside the signature after the grade. */
  shown?: Shown;
}

/**
 * What a decider saw of a grant request (identity-and-lifecycle I7-S): the record's tool, resource, arguments digest
 * (its own string) and expiry. The node the decision is addressed to compares each with its own record and refuses a
 * difference by name (`decision_names_another_request`).
 */
export interface Shown {
  tool: string;
  resource: string;
  arguments_digest: string;
  request_expires_at_ms: number;
}

/** The block's marker in the signed message. */
export const SHOWN_MARKER = "shown/1";

/**
 * What a decision on `record` names of it — a request as `grantsPending()` lists it or `grantsStream()` reports it:
 * its `tool`, `resource`, `arguments_digest` and `expires_at_ms` (as `request_expires_at_ms`). Throws when one is
 * absent or not of its type.
 */
export function shownFromRecord(record: Record<string, unknown>): Shown {
  const { tool, resource, arguments_digest } = record;
  const expires = typeof record.expires_at_ms === "bigint" ? Number(record.expires_at_ms) : record.expires_at_ms;
  if (typeof tool !== "string" || typeof resource !== "string" || typeof arguments_digest !== "string" || typeof expires !== "number" || !Number.isSafeInteger(expires) || expires < 0) {
    throw new Error("a request record names tool, resource and arguments_digest (strings) and expires_at_ms (an integer)");
  }
  return { tool, resource, arguments_digest, request_expires_at_ms: expires };
}

/**
 * The bytes a decider signs to answer a grant request (identity-and-lifecycle I3): the tag; the decider, the addressed
 * node, the request's holder (a principal), the request id, the decision (`issue`, `refuse` or `revoke`), an issue's
 * `ttlSecs` in decimal (empty when absent) and the reason (empty when absent), each a `u32` little-endian length and
 * its UTF-8 bytes; then `issuedAtMs` as a `u64` little-endian. `grade` (identity-and-lifecycle I7): when given, a
 * `u32` little-endian length and the grade's UTF-8 bytes follow; when left out nothing follows, so a decision naming
 * no grade has the message it had before I7. Pinned, with a grade and without, by
 * `tests/fixtures/decision/vector.json`. `shown` (identity-and-lifecycle I7-S): when given, after the grade come
 * `"shown/1"`, the tool, the resource and the digest, each a `u32` little-endian length and its UTF-8 bytes, then
 * `request_expires_at_ms` as a `u64` little-endian. A block requires a grade. Pinned by the vector's `shown` section.
 */
export function grantDecisionMessage(
  decider: string, node: string, agent: string, request: string, decision: string,
  ttlSecs: number | undefined, reason: string | undefined, issuedAtMs: bigint, grade?: string, shown?: Shown,
): Uint8Array {
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [enc.encode(GRANT_DECISION_TAG)];
  for (const part of [decider, node, agent, request, decision, ttlSecs === undefined ? "" : String(Math.trunc(ttlSecs)), reason ?? ""]) {
    const b = enc.encode(part);
    const len = new Uint8Array(4);
    new DataView(len.buffer).setUint32(0, b.length, true);
    chunks.push(len, b);
  }
  const t = new Uint8Array(8);
  new DataView(t.buffer).setBigUint64(0, issuedAtMs, true);
  chunks.push(t);
  // I7: the grade, when named, follows the clock; a decision naming none has the message it had before I7.
  if (grade !== undefined) {
    const g = enc.encode(grade);
    const len = new Uint8Array(4);
    new DataView(len.buffer).setUint32(0, g.length, true);
    chunks.push(len, g);
  }
  // I7-S: what the decider saw, after the grade.
  if (shown !== undefined) {
    for (const part of [SHOWN_MARKER, shown.tool, shown.resource, shown.arguments_digest]) {
      const b = enc.encode(part);
      const len = new Uint8Array(4);
      new DataView(len.buffer).setUint32(0, b.length, true);
      chunks.push(len, b);
    }
    const e = new Uint8Array(8);
    new DataView(e.buffer).setBigUint64(0, BigInt(shown.request_expires_at_ms), true);
    chunks.push(e);
  }
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let off = 0;
  for (const c of chunks) { out.set(c, off); off += c.length; }
  return out;
}

/**
 * Does `value` hold, at any depth, an integral number outside ±(2^53 − 1)? RFC 8785 writes it as the nearest double,
 * so a signature could not tell it from its neighbour: such a call is refused, at the signer and the provider. Send
 * large ids as strings.
 */
export function holdsUnsafeInteger(value: unknown): boolean {
  if (typeof value === "bigint") return value > BigInt(Number.MAX_SAFE_INTEGER) || value < -BigInt(Number.MAX_SAFE_INTEGER);
  if (typeof value === "number") return Number.isInteger(value) && !Number.isSafeInteger(value);
  if (Array.isArray(value)) return value.some(holdsUnsafeInteger);
  if (value !== null && typeof value === "object") return Object.values(value as Record<string, unknown>).some(holdsUnsafeInteger);
  return false;
}

/**
 * What a `tools/call` of `name` signs: `["tools/call", "tool:{name}", digest(arguments)]`. Throws for arguments holding
 * an integer outside ±(2^53 − 1) ({@link holdsUnsafeInteger}).
 */
export function mcpBinding(name: string, args: unknown): [string, string, Uint8Array] {
  if (holdsUnsafeInteger(args)) {
    throw new Error("an integer outside ±(2^53 - 1) cannot be bound exactly by a client signature: send it as a string");
  }
  return ["tools/call", `tool:${name}`, argumentsDigest(args)];
}

/** What an A2A send of `text` to `skillId` signs: `["skill.invoke", "skill:{id}", digest({text})]`. */
export function a2aBinding(skillId: string, text: string): [string, string, Uint8Array] {
  return ["skill.invoke", `skill:${skillId}`, argumentsDigest({ text })];
}

/** A mandate as the gateway serves it inside a grant (`Mandate`). */
export interface Mandate {
  holder: string;
  established_by: string;
  purpose: string;
  scope: string;
  operations: string[];
  epoch: number;
  term: string;
  valid_from_ms: number;
  valid_until_ms: number;
}

/** A grant as `GET /gateway/grants/request/{request}` serves it: the mandate and its issuer's signature bytes. */
export interface SignedMandateGrant {
  mandate: Mandate;
  signature: number[];
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let off = 0;
  for (const c of chunks) { out.set(c, off); off += c.length; }
  return out;
}

function u32le(n: number): Uint8Array {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
}

function u64le(n: number | bigint): Uint8Array {
  const b = new Uint8Array(8);
  new DataView(b.buffer).setBigUint64(0, BigInt(n), true);
  return b;
}

function lp(bytes: Uint8Array): Uint8Array[] {
  return [u32le(bytes.length), bytes];
}

/**
 * The bytes a mandate's establishing authority signs — the gateway's `Mandate::canonical_bytes`: the tag
 * `mycelium.mandate/grant/1`, `holder`, `established_by`, `purpose` and `scope` each as a `u32` little-endian length
 * and its UTF-8 bytes; the number of `operations` as a `u32` and each operation so; then `epoch` as a `u64`, `term`
 * length-prefixed, `valid_from_ms` and `valid_until_ms` as `u64`, all little-endian.
 */
export function mandateCanonicalBytes(mandate: Mandate): Uint8Array {
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [...lp(enc.encode("mycelium.mandate/grant/1"))];
  for (const f of [mandate.holder, mandate.established_by, mandate.purpose, mandate.scope]) chunks.push(...lp(enc.encode(f)));
  chunks.push(u32le(mandate.operations.length));
  for (const op of mandate.operations) chunks.push(...lp(enc.encode(op)));
  chunks.push(u64le(mandate.epoch), ...lp(enc.encode(mandate.term)), u64le(mandate.valid_from_ms), u64le(mandate.valid_until_ms));
  return concat(chunks);
}

/** SHA-256 of {@link mandateCanonicalBytes}. */
export function mandateDigest(mandate: Mandate): Uint8Array {
  return new Uint8Array(createHash("sha256").update(mandateCanonicalBytes(mandate)).digest());
}

/**
 * What a grant's holder signs to prove possession for one call: the tag `mycelium.mandate/possession/1`, the grant's
 * {@link mandateDigest}, then `request` ({@link mandateRequestBytes}) as a `u32` little-endian length and its bytes.
 */
export function possessionMessage(mandate: Mandate, request: Uint8Array): Uint8Array {
  return concat([new TextEncoder().encode("mycelium.mandate/possession/1"), mandateDigest(mandate), ...lp(request)]);
}

/**
 * A service's signer: its client certificate's key and principal (`token:{issuer}/{name}`), an instance id drawn once
 * from `crypto.randomBytes`, and a counter. `key` is the PEM `mycelium tls issue --client` wrote, or the 32-byte seed.
 */
export class ClientSigner {
  readonly instance: bigint;
  private readonly key: KeyObject;
  private readonly raw: Buffer;
  private counter = 0n;

  constructor(key: string | Uint8Array, readonly principal: string, opts?: { instance?: bigint }) {
    this.key = typeof key === "string"
      ? createPrivateKey(key)
      : createPrivateKey({ key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), Buffer.from(key)]), format: "der", type: "pkcs8" });
    if (this.key.asymmetricKeyType !== "ed25519") throw new Error("a client certificate's key is Ed25519");
    this.raw = createPublicKey(this.key).export({ format: "der", type: "spki" }).subarray(-32);
    this.instance = opts?.instance ?? randomBytes(8).readBigUInt64BE();
  }

  /** Sign a `tools/call` of `name` ({@link mcpBinding}, which refuses an unsafe integer). */
  signToolsCall(name: string, args: unknown): { cs: ClientSignature; cn: ClientNonce } {
    const [op, res, digest] = mcpBinding(name, args);
    return this.sign(op, res, digest);
  }

  /**
   * Present `grant` — held by this signer's principal — for one call: `{grant, possession}`, to send as
   * `params._meta.mandate`. The proof binds the grant, the operation, the resource before `@` and the call's arguments
   * by the gateway's mandate digest ({@link mandateArgumentsDigest}, RFC 8785 since identity-and-lifecycle I3), so it is
   * good for this call alone. Throws for a grant another principal holds, and for arguments holding an integer outside
   * ±(2^53 − 1), which that digest cannot bind exactly — send such a number as a string.
   */
  provePossession(grant: SignedMandateGrant, operation: string, resource: string, args: unknown): PresentedMandate {
    if (grant.mandate.holder !== this.principal) {
      throw new Error(`the grant is held by ${JSON.stringify(grant.mandate.holder)}, not ${JSON.stringify(this.principal)}`);
    }
    const request = mandateRequestBytes(operation, resource, mandateArgumentsDigest(args));
    const proof = sign(null, possessionMessage(grant.mandate, request), this.key);
    return { grant, possession: proof.toString("base64") };
  }

  /**
   * Sign an answer to the grant request `request` held by `agent` (a principal, as the pending list's `agent` names
   * it), addressed to the node `node` (its id, as `GET /health` reports it): `decision` is `"issue"` (with `ttlSecs`),
   * `"refuse"` or `"revoke"` (with `reason`). Returns the decision body's `signed` field; the node it is addressed to
   * records this signer as the decision's actor, whoever carries it (identity-and-lifecycle I3).
   *
   * `grade` (identity-and-lifecycle I7): a service's decision is recorded `service` whether or not it names one, and
   * `"service"` is the only grade this signer may name — its key is a client certificate's Ed25519 key, never a
   * consent key. Left out (the default), the body is what it was before I7, which a node before I7 accepts; a body
   * naming a grade is refused by such a node.
   *
   * `shown` (identity-and-lifecycle I7-S): what this signer saw of the request ({@link shownFromRecord}), signed after
   * the grade — which is then `"service"`, named or not. A gateway after I7-S refuses an issue or refusal naming no
   * block by default (`decision_names_nothing`); a node before I7-S refuses one naming it, so upgrade the gateways
   * first. A revocation names no block.
   */
  signGrantDecision(
    node: string, agent: string, request: string, decision: "issue" | "refuse" | "revoke",
    opts?: { ttlSecs?: number; reason?: string; issuedAtMs?: number; grade?: "service"; shown?: Shown },
  ): SignedDecision {
    if (!["issue", "refuse", "revoke"].includes(decision)) throw new Error('decision must be "issue", "refuse" or "revoke"');
    if (opts?.grade !== undefined && opts.grade !== "service") throw new Error('a service signer names the grade "service" or none: a consent grade is a consent key\'s');
    if (opts?.shown !== undefined && decision === "revoke") throw new Error("a revocation names the grant at its path, not what was shown");
    const shown = opts?.shown === undefined ? undefined : shownFromRecord({ ...opts.shown, expires_at_ms: opts.shown.request_expires_at_ms });
    const grade = shown !== undefined ? "service" : opts?.grade;
    const t = opts?.issuedAtMs ?? Date.now();
    const msg = grantDecisionMessage(this.principal, node, agent, request, decision, opts?.ttlSecs, opts?.reason, BigInt(t), grade, shown);
    const signed: SignedDecision = { decider: this.principal, node, agent, request, t, k: this.raw.toString("base64"), sig: sign(null, msg, this.key).toString("base64") };
    if (grade !== undefined) signed.grade = grade;
    if (shown !== undefined) signed.shown = shown;
    return signed;
  }

  /**
   * Sign one call; returns `{cs, cn}` to merge into `params._meta`. Each call takes the next counter unless given.
   * `resource` is the **bare** resource — `tool:x`, never `tool:x@node` — as {@link mcpBinding} and {@link a2aBinding}
   * return it: the signature binds it whole.
   */
  sign(operation: string, resource: string, digest: Uint8Array, opts?: { issuedAtMs?: number; counter?: number }): { cs: ClientSignature; cn: ClientNonce } {
    const t = opts?.issuedAtMs ?? Date.now();
    let c: bigint;
    if (opts?.counter !== undefined) c = BigInt(opts.counter);
    else { c = this.counter; this.counter += 1n; }
    const msg = clientSignatureMessage(this.principal, operation, resource, digest, BigInt(t), this.instance, c);
    const sig = sign(null, msg, this.key);
    return {
      cs: { k: this.raw.toString("base64"), sig: sig.toString("base64") },
      cn: { t, i: this.instance.toString(16).padStart(16, "0"), c: Number(c) },
    };
  }
}
