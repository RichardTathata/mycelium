/**
 * mycelium/artifacts — the artifact catalogue's gateway door (plan A3).
 *
 * `POST /gateway/artifacts/publish` takes one **already-signed** catalogue line — the hex a
 * `mycelium-artifact publish` step wrote to the library's manifest — verifies its provenance
 * against the node's trusted publishers, and writes it into the gossiped `installable/`
 * catalogue. The publisher's key never reaches a gateway and the bytes never ride the request.
 * Scope family: `artifact:publish`.
 *
 * Refusals are by name (`ArtifactError.kind`): `unsigned entry`, `untrusted publisher`,
 * `provenance does not verify`, `no trusted publishers configured` (403), `librarian-managed
 * signer` (409), `malformed entry` (400), `insufficient scope` (403, from the auth layer).
 */

/** The gateway's receipt for a published line. */
export interface PublishReceipt {
  key: string;
  artifact: string;
  signer: string;
  kind: string;
  provides: { ns: string; name: string };
}

/** A refusal, carrying the gateway's own name for it. */
export class ArtifactError extends Error {
  constructor(
    public readonly kind: string,
    public readonly detail: string,
    public readonly status: number,
    public readonly body: Record<string, unknown>,
  ) {
    super(`${kind}: ${detail}`);
    this.name = "ArtifactError";
  }
}

async function refuse(r: Response): Promise<never> {
  let body: Record<string, unknown> = {};
  try {
    const parsed: unknown = await r.json();
    if (parsed && typeof parsed === "object") body = parsed as Record<string, unknown>;
  } catch {
    body = {};
  }
  const kind = String(body.error ?? `http-${r.status}`);
  const detail = String(body.detail ?? body.required_scope ?? body.error ?? r.statusText);
  throw new ArtifactError(kind, detail, r.status, body);
}

/** The artifact catalogue verbs on one node's gateway. Get one from `MyceliumAgent.artifacts()`. */
export class Artifacts {
  constructor(
    private readonly base: string,
    private readonly auth: Record<string, string>,
    private readonly timeout: number,
  ) {}

  /** Publish one signed catalogue line (lowercase hex of the manifest line). Returns the receipt;
   * throws `ArtifactError` with the refusal's name otherwise. */
  async publish(entryHex: string): Promise<PublishReceipt> {
    const r = await fetch(`${this.base}/gateway/artifacts/publish`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...this.auth },
      body: JSON.stringify({ entry_hex: entryHex.trim() }),
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!r.ok) return refuse(r);
    return (await r.json()) as PublishReceipt;
  }
}
