/**
 * Federated domains — the consumer side, over one node's gateway (item 2 row 11).
 *
 * A **domain** is one independently admitted mesh. Federation is not two meshes merging: it is
 * one domain calling a service another has explicitly *exported* to it, over an authenticated
 * edge, with neither mesh learning the other's members.
 *
 * These verbs drive your **own** node's gateway; they never speak the cross-domain protocol
 * directly. The credential minted for a federated call is signed with your domain's key and
 * names you — neither the key nor the trust bundle belongs in an SDK process.
 *
 * ```ts
 * const fed = agent.federation();
 * await fed.connect("partner.example");                       // what have they exported to us?
 * const reply = await fed.call("partner.example", "invoice.status", "INV-42");
 * ```
 *
 * **Who the partner sees.** The principal your bearer resolved to at your own gateway — not the
 * node, not a service account. With no token model configured it is `anonymous`, which is honest
 * and usually not what you want in a partner's evidence.
 *
 * **Reading a refusal.** Every failure throws a {@link FederationError}; the two fields that
 * matter are not the message. `sent` says whether any byte reached the partner — `false` means
 * the refusal happened at your own gateway and a retry is safe. `delivery` is `none` · `refused`
 * · `completed` · `unknown`, and `unknown` throws {@link DeliveryUnknownError}, which is **not**
 * a failure: a gateway went silent and nobody can say whether the call ran.
 */

/** One partner, as the gateway reports it. */
export interface PartnerLink {
  domain: string;
  /** `down` · `refreshing` · `ready`. */
  link: string;
  /**
   * The exports the last successful {@link Federation.connect} was granted — remembered, not
   * fresh. Whether an export may be called *now* is decided at call time.
   */
  last_catalogue: string[] | null;
}

/** This node's own federated identity. */
export interface DomainInfo {
  configured: boolean;
  domain?: string;
  exports?: string[];
  policy_revision?: number;
  signs_catalogue?: boolean;
}

/** The last observed catalogue for one partner (no network). */
export interface CatalogView {
  domain: string;
  link: string;
  observed: boolean;
  exports: string[] | null;
}

/** A federated call did not complete. See the module docs for `sent` / `delivery`. */
export class FederationError extends Error {
  readonly kind: string;
  readonly detail: string;
  readonly sent: boolean;
  readonly delivery: string;
  readonly status: number;
  readonly body: Record<string, unknown>;

  constructor(
    kind: string,
    detail: string,
    sent: boolean,
    delivery: string,
    status: number,
    body: Record<string, unknown>,
  ) {
    super(`${kind}: ${detail}`);
    this.name = "FederationError";
    this.kind = kind;
    this.detail = detail;
    this.sent = sent;
    this.delivery = delivery;
    this.status = status;
    this.body = body;
  }

  /**
   * True when the refusal happened here and no byte crossed, so retrying cannot double-run an
   * effect at the partner. Not a promise that a retry will succeed.
   */
  get nothingWasSent(): boolean {
    return !this.sent;
  }
}

/**
 * The call **may have run**. Not a negative — retrying it is retrying a possible effect, which is
 * the caller's decision, and `repeatable: true` is how that decision is stated up front.
 */
export class DeliveryUnknownError extends FederationError {
  constructor(
    kind: string,
    detail: string,
    sent: boolean,
    delivery: string,
    status: number,
    body: Record<string, unknown>,
  ) {
    super(kind, detail, sent, delivery, status, body);
    this.name = "DeliveryUnknownError";
  }

  /** The gateways tried, in order, so an operator can go and look. */
  get attemptedVia(): string[] {
    const v = this.body["attempted_via"];
    return Array.isArray(v) ? (v as string[]) : [];
  }
}

async function raiseFor(resp: Response): Promise<never> {
  let body: Record<string, unknown> = {};
  try {
    const parsed: unknown = await resp.json();
    if (parsed && typeof parsed === "object") body = parsed as Record<string, unknown>;
  } catch {
    // A proxy in the path can answer HTML. Fall through with an empty body and fail closed
    // below — never turn an unreadable refusal into a success, and never crash reporting it.
  }
  const kind = typeof body["error"] === "string" ? (body["error"] as string) : `http-${resp.status}`;
  const detail = typeof body["detail"] === "string" ? (body["detail"] as string) : kind;
  const sent = typeof body["sent"] === "boolean" ? (body["sent"] as boolean) : true;
  const delivery = typeof body["delivery"] === "string" ? (body["delivery"] as string) : "unknown";
  const Cls = delivery === "unknown" ? DeliveryUnknownError : FederationError;
  throw new Cls(kind, detail, sent, delivery, resp.status, body);
}

/** Federation verbs on one node's gateway. Get one from `agent.federation()`. */
export class Federation {
  constructor(
    private readonly base: string,
    private readonly auth: Record<string, string>,
    private readonly timeout: number,
  ) {}

  private async get(path: string): Promise<Record<string, unknown>> {
    const resp = await fetch(`${this.base}${path}`, {
      headers: this.auth,
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!resp.ok) await raiseFor(resp);
    return (await resp.json()) as Record<string, unknown>;
  }

  private async post(path: string, body: unknown): Promise<Record<string, unknown>> {
    const resp = await fetch(`${this.base}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", ...this.auth },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!resp.ok) await raiseFor(resp);
    return (await resp.json()) as Record<string, unknown>;
  }

  // ── Read (federation:read) ────────────────────────────────────────────────

  /** This node's own federated identity, or `{ configured: false }` when it serves no domain. */
  async domain(): Promise<DomainInfo> {
    return (await this.get("/gateway/federation/domain")) as unknown as DomainInfo;
  }

  /** One row per configured partner. */
  async partners(): Promise<PartnerLink[]> {
    const data = await this.get("/gateway/federation/partners");
    return (data["partners"] ?? []) as PartnerLink[];
  }

  /**
   * The **last observed** catalogue for one partner — no network, so looking at a partner during
   * an outage does not change the link's state. {@link connect} is the one that asks.
   */
  async catalog(domain: string): Promise<CatalogView> {
    return (await this.get(
      `/gateway/federation/catalog/${encodeURIComponent(domain)}`,
    )) as unknown as CatalogView;
  }

  // ── Invoke (federation:invoke) ────────────────────────────────────────────

  /**
   * Fetch a partner's catalogue and bring the link up. Returns the exports **granted to us**,
   * which is the grant — not the partner's full export list.
   */
  async connect(domain: string): Promise<string[]> {
    const data = await this.post("/gateway/federation/connect", { domain });
    return (data["exports"] ?? []) as string[];
  }

  /**
   * Invoke one of a partner's exports and return its first text artifact.
   *
   * `repeatable` is a statement about **your** effect and is the only thing that decides whether
   * a silent gateway may be retried elsewhere. It defaults to `false`, because the safe default
   * for an unstated effect is the one that never runs twice.
   */
  async call(
    domain: string,
    exportName: string,
    text = "",
    opts: { repeatable?: boolean } = {},
  ): Promise<string> {
    const data = await this.post("/gateway/federation/call", {
      domain,
      export: exportName,
      text,
      repeatable: opts.repeatable ?? false,
    });
    return (data["reply"] ?? "") as string;
  }
}
