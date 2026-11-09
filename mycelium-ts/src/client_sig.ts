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
 * The bytes a client signs: the tag; the principal, the operation and the resource before `@`, each a `u32`
 * little-endian length and its UTF-8 bytes; the 32-byte arguments digest; then `issuedAtMs`, `instance` and `counter`
 * as `u64` little-endian.
 */
export function clientSignatureMessage(
  principal: string, operation: string, resource: string, digest: Uint8Array,
  issuedAtMs: bigint, instance: bigint, counter: bigint,
): Uint8Array {
  if (digest.length !== 32) throw new Error("the arguments digest is 32 bytes");
  const enc = new TextEncoder();
  const chunks: Uint8Array[] = [enc.encode(TAG)];
  for (const part of [principal, operation, resource.split("@")[0]]) {
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
   * by the gateway's mandate digest ({@link mandateArgumentsDigest} — not the signature's JCS digest), so it is good
   * for this call alone. Throws for a grant another
   * principal holds. The gateway's mandate digest is not RFC 8785: for a float in exponent form it differs, and the
   * call is refused (identity-and-lifecycle §9 O1) — send such a number as a string.
   */
  provePossession(grant: SignedMandateGrant, operation: string, resource: string, args: unknown): PresentedMandate {
    if (grant.mandate.holder !== this.principal) {
      throw new Error(`the grant is held by ${JSON.stringify(grant.mandate.holder)}, not ${JSON.stringify(this.principal)}`);
    }
    const request = mandateRequestBytes(operation, resource, mandateArgumentsDigest(args));
    const proof = sign(null, possessionMessage(grant.mandate, request), this.key);
    return { grant, possession: proof.toString("base64") };
  }

  /** Sign one call; returns `{cs, cn}` to merge into `params._meta`. Each call takes the next counter unless given. */
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
