/**
 * A `tools/call` client for a Mycelium gateway's `/mcp` door, and a client's own grants (identity-and-lifecycle I1).
 *
 * A service with a client certificate signs each call with its key ({@link ClientSigner}, sent as
 * `params._meta.cs`/`cn`); the provider that runs the call checks it. A tool whose policy requires a grant is called
 * under one the service holds: it asks with {@link McpClient.requestGrant}, a supervisor issues it at the gateway's
 * decision door, it reads the answer with {@link McpClient.grantStatus}, and presents it with
 * {@link McpClient.callTool}`(…, { grant })`, which proves possession with the same key.
 *
 * @example
 * ```typescript
 * const signer = new ClientSigner(readFileSync("forecaster.key.pem", "utf8"), "token:fleet/forecaster");
 * const mcp = new McpClient("https://gw:8300", { token, signer });
 * const asked = await mcp.requestGrant("weather.forecast", { city: "Lyon" });
 * const answer = await mcp.waitForGrant(asked.request);
 * const result = await mcp.callTool("weather.forecast", { city: "Lyon" }, { grant: answer.grant });
 * ```
 */
import { A2aError, ActionRefusedError, type PresentedMandate } from "./a2a";
import { authHeaders, resolveToken, type AuthOptions } from "./auth";
import type { ClientSigner, SignedMandateGrant } from "./client_sig";
import { pathSegment } from "./wire";

/**
 * A JSON-RPC error from `/mcp`: the numeric `code`, its message and `data`. The action evaluator's three refusals throw
 * {@link ActionRefusedError} instead, so they stay told apart.
 */
export class McpError extends A2aError {
  constructor(code: number, detail: string, data: Record<string, unknown> = {}) {
    super(code, detail, data);
    this.name = "McpError";
    this.message = `MCP error ${code}: ${detail}`;
  }
}

/**
 * A grant request ended without a grant: `state` is `refused`, `revoked` or `expired` (or `pending` when
 * {@link McpClient.waitForGrant} timed out); `answer` is the door's whole answer.
 */
export class GrantNotIssuedError extends Error {
  constructor(readonly state: string, readonly answer: GrantStatus) {
    super(`the grant request ended ${state}`);
    this.name = "GrantNotIssuedError";
  }
}

/** The answer of `POST /gateway/grants/request`. */
export interface GrantRequestAnswer {
  ok: boolean;
  request: string;
  key: string;
  resource: string;
  arguments_digest: string;
  expires_at_ms: number;
  /** A request standing for the same tool and arguments was answered again. */
  reused: boolean;
}

/** The answer of `GET /gateway/grants/request/{request}`. */
export interface GrantStatus {
  ok: boolean;
  state: "pending" | "issued" | "refused" | "revoked" | "expired";
  /** With `state: "issued"`: the signed grant, to pass to {@link McpClient.callTool}. */
  grant?: SignedMandateGrant;
  refusal?: Record<string, unknown>;
  revocation?: Record<string, unknown>;
  request?: Record<string, unknown>;
}

/** Options for {@link McpClient.callTool}. */
export interface CallToolOptions {
  /** A grant this signer's principal holds: presented as `params._meta.mandate` with a possession proof for this call. */
  grant?: SignedMandateGrant;
  /** An already-proven `{grant, possession}`, instead of `grant`. */
  mandate?: PresentedMandate;
  /** The causal fields (adopter-surface X4). */
  correlation?: string;
  parent?: string;
}

/** HTTP client for a gateway's `/mcp` door (`mcp:invoke`). */
export class McpClient {
  private readonly baseUrl: string;
  private readonly auth: Record<string, string>;
  private readonly timeoutMs: number;
  private nextId = 0;
  /**
   * The service's {@link ClientSigner}: every `tools/call` is then signed (`cs`/`cn`), and a grant passed to
   * {@link callTool} is presented with a possession proof by the same key. Its principal must be the one the bearer
   * authenticates as, or the provider refuses the call.
   */
  readonly signer?: ClientSigner;

  constructor(baseUrl: string, { timeoutMs = 30_000, token, signer }: { timeoutMs?: number; signer?: ClientSigner } & Pick<AuthOptions, "token"> = {}) {
    this.baseUrl = baseUrl.replace(/\/$/, "");
    this.timeoutMs = timeoutMs;
    this.auth = authHeaders(resolveToken(token));
    this.signer = signer;
  }

  private async rpc(method: string, params: Record<string, unknown>): Promise<unknown> {
    this.nextId += 1;
    const resp = await fetch(`${this.baseUrl}/mcp`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...this.auth },
      body: JSON.stringify({ jsonrpc: "2.0", id: this.nextId, method, params }),
      signal: AbortSignal.timeout(this.timeoutMs + 5_000),
    });
    if (!resp.ok) throw new Error(`/mcp responded ${resp.status}`);
    const body = (await resp.json()) as { result?: unknown; error?: { code: number; message: string; data?: Record<string, unknown> } };
    if (body.error) {
      if (body.error.code in ActionRefusedError.CODES) throw new ActionRefusedError(body.error.code, body.error.message, body.error.data ?? {});
      throw new McpError(body.error.code, body.error.message, body.error.data ?? {});
    }
    return body.result;
  }

  private async gateway<T>(method: "GET" | "POST", path: string, body?: unknown, timeoutMs = this.timeoutMs): Promise<T> {
    const resp = await fetch(`${this.baseUrl}${path}`, {
      method,
      headers: body === undefined ? this.auth : { "Content-Type": "application/json", ...this.auth },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeoutMs),
    });
    const data = (await resp.json().catch(() => ({}))) as T & { error?: string; detail?: string };
    if (!resp.ok) throw new Error(`${path} responded ${resp.status}: ${data.error ?? ""} ${data.detail ?? ""}`.trim());
    return data;
  }

  /** `tools/list`: the tools this gateway can call. */
  async listTools(): Promise<unknown[]> {
    const result = (await this.rpc("tools/list", {})) as { tools?: unknown[] } | undefined;
    return result?.tools ?? [];
  }

  /**
   * `tools/call` of `name` with `args`; returns the JSON-RPC `result`. With a `signer` the call is signed
   * (`params._meta.cs`/`cn`); arguments holding an integer outside ±(2^53 − 1) throw before anything is sent. Throws
   * {@link ActionRefusedError} for the evaluator's refusals and {@link McpError} for any other JSON-RPC error.
   */
  async callTool(name: string, args: Record<string, unknown> = {}, opts: CallToolOptions = {}): Promise<unknown> {
    const meta: Record<string, unknown> = {};
    let mandate = opts.mandate;
    if (opts.grant) {
      if (!this.signer) throw new Error("presenting a grant needs the holder's signer: new McpClient(url, { signer })");
      if (mandate) throw new Error("pass a grant or a mandate, not both");
      mandate = this.signer.provePossession(opts.grant, "tools/call", `tool:${name}`, args);
    }
    if (mandate) meta.mandate = mandate;
    if (this.signer) Object.assign(meta, this.signer.signToolsCall(name, args));
    if (opts.correlation !== undefined) meta.correlation = opts.correlation;
    if (opts.parent !== undefined) meta.parent = opts.parent;
    const params: Record<string, unknown> = { name, arguments: args };
    if (Object.keys(meta).length > 0) params._meta = meta;
    return this.rpc("tools/call", params);
  }

  /**
   * `POST /gateway/grants/request` (`mcp:invoke`): ask for a grant to call `tool` with `args`. The caller must hold a
   * client certificate on record at the gateway (`409 not_a_client_holder`). A request standing for the same tool and
   * arguments is answered again (`reused: true`); past 16 standing requests the door answers `429 too_many_pending`.
   * `argumentsClass` names the arguments' data class (a classified set is withheld from the record the supervisor
   * reads); `windowSecs` is how long the request stands (default 60, at most 600).
   */
  async requestGrant(tool: string, args: Record<string, unknown> = {}, opts: { argumentsClass?: string; windowSecs?: number } = {}): Promise<GrantRequestAnswer> {
    return this.gateway<GrantRequestAnswer>("POST", "/gateway/grants/request", {
      tool,
      arguments: args,
      ...(opts.argumentsClass !== undefined ? { arguments_class: opts.argumentsClass } : {}),
      ...(opts.windowSecs !== undefined ? { window_secs: opts.windowSecs } : {}),
    });
  }

  /**
   * `GET /gateway/grants/request/{request}` (`mcp:invoke`): the caller's own request and its answer. Another
   * principal's request is `404 unknown_request`.
   */
  async grantStatus(request: string, { timeoutMs }: { timeoutMs?: number } = {}): Promise<GrantStatus> {
    return this.gateway<GrantStatus>("GET", `/gateway/grants/request/${pathSegment(request)}`, undefined, timeoutMs ?? this.timeoutMs);
  }

  /**
   * Poll {@link grantStatus} until the request is answered; resolve with the `issued` answer, or throw
   * {@link GrantNotIssuedError} naming the state it ended in (`pending` when `timeoutMs` passed first). Each read and
   * each pause is bounded by the time left, so the wait ends within `timeoutMs`; a read the deadline cuts short is
   * `pending` with the last answer seen.
   */
  async waitForGrant(request: string, { timeoutMs = 60_000, pollMs = 500 }: { timeoutMs?: number; pollMs?: number } = {}): Promise<GrantStatus> {
    const deadline = Date.now() + timeoutMs;
    let answer = { ok: false, state: "pending" } as GrantStatus;
    for (;;) {
      const left = deadline - Date.now();
      if (left <= 0) throw new GrantNotIssuedError("pending", answer);
      const bound = Math.min(left, this.timeoutMs);
      try {
        answer = await this.grantStatus(request, { timeoutMs: bound });
      } catch (e) {
        // Cut short by the deadline, not the client's own timeout.
        if (bound < this.timeoutMs && (e as Error).name === "TimeoutError") throw new GrantNotIssuedError("pending", answer);
        throw e;
      }
      if (answer.state === "issued") return answer;
      if (answer.state !== "pending") throw new GrantNotIssuedError(answer.state, answer);
      await new Promise((r) => setTimeout(r, Math.max(0, Math.min(pollMs, deadline - Date.now()))));
    }
  }
}
