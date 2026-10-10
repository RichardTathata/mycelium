import { sseStream } from "./sse";
import { parseLossless, stringifyLossless, toBigInt } from "./json";
import { Federation } from "./federation";
import { Artifacts } from "./artifacts";
import { authHeaders, baseUrl, resolveToken, type AuthOptions } from "./auth";
import { pathSegment, wholeSeconds } from "./wire";
import {
  KvReceipt,
  CapabilityHandle,
  UnitHandle,
  DemandStatus,
  LockGuard,
  LogEntry,
  MailboxEvent,
  RpcRequest,
  Signal,
  CommitResult,
} from "./types";

function b64(buf: Buffer | Uint8Array): string {
  return Buffer.from(buf).toString("base64");
}

function fromb64(s: string): Buffer {
  return Buffer.from(s, "base64");
}

/**
 * Reads the gateway's `"persisted"` field (absent on a pre-v2.4.2 node → `null`) and, beside it,
 * `"local_durability"` / `"local_durability_error"` (absent on a pre-v2.8.0 node → `null`).
 */
function commitResult(data: {
  persisted?: boolean;
  local_durability?: string;
  local_durability_error?: string;
}): CommitResult {
  return {
    persisted: typeof data.persisted === "boolean" ? data.persisted : null,
    localDurability: typeof data.local_durability === "string" ? data.local_durability : null,
    localDurabilityError:
      typeof data.local_durability_error === "string" ? data.local_durability_error : null,
  };
}

/**
 * Connects to a running Rust Mycelium node over loopback HTTP.
 *
 * No native extension — the HTTP gateway sidecar adds ~1 ms per call,
 * invisible next to LLM inference latency.
 */
/**
 * The gateway refused a **protected** RPC kind on a raw mesh route (HTTP 403 `protected_kind`).
 * `mcp.invoke`, `skill.invoke` and `llm.invoke` (plus any kind the operator lists in
 * `protected_rpc_kinds`) are work with a door of their own, where authority is checked: use `/mcp`,
 * `A2aClient` or the LLM client instead.
 */
export class ProtectedKindError extends Error {
  constructor(public readonly kind: string, message: string) {
    super(message);
    this.name = "ProtectedKindError";
  }
}

/**
 * The gateway refused a **protected key** on a raw KV route (HTTP 403 `protected_key`). `set`, `delete`,
 * `setWithMinAcks` and `consistentSet` write application keys only (`ckpt/`, `manifest/`, `schemas/`,
 * your own namespaces); a key in a namespace the substrate or a companion owns (`sys/`, `grp/`, `cap/`,
 * `prompts/`, `log/`, `mailbox/`, …) is written through its own route, which `message` names.
 */
export class ProtectedKeyError extends Error {
  readonly status = 403;
  constructor(public readonly key: string, message: string) {
    super(message);
    this.name = "ProtectedKeyError";
  }
}

/**
 * The gateway refused a **protected log stream** (HTTP 403 `protected_stream`): `append` and
 * `compactLog` refuse a stream under `cn/`, `wiki/` or `reason/`, which the commitment net, a wiki and
 * `mycelium-reason` write through themselves. `message` is the gateway's.
 */
export class ProtectedStreamError extends Error {
  readonly status = 403;
  constructor(public readonly stream: string, message: string) {
    super(message);
    this.name = "ProtectedStreamError";
  }
}

/**
 * A consensus write that did not commit **your** value: the slot was decided for another (HTTP 409
 * `{"ok": false, "error": "superseded"}`, substrate 2.30.0+ — the normal answer to a concurrent loser).
 *
 * Thrown by `consistentSet`, `crossGroupPropose`, `distributedLock` and `electLeader` (0.2.4; the Python
 * SDK's `SupersededError` since 0.2.8). Before, these threw a plain `Error` naming the status, which a
 * caller could tell apart from a topology refusal only by reading the message. Read the decided value
 * with `consistentGet`. A timeout (504, a `TimeoutError`) and the other 409s — `topology_unsatisfied` —
 * stay what they were: a timeout does not mean the value lost.
 */
export class SupersededError extends Error {
  readonly status = 409;
  constructor(message = "the slot was decided for another value (409 superseded)") {
    super(message);
    this.name = "SupersededError";
  }
}

/** The key or stream a write names, for the refusal that names it back. */
type Subject = { key?: string; stream?: string };

export class MyceliumAgent {
  private readonly base: string;
  private readonly timeout: number;
  private readonly auth: Record<string, string>;
  private _nodeId: string | null = null;

  /**
   * @param host    Gateway host (default "127.0.0.1")
   * @param port    HTTP port the Mycelium node listens on (default 7946)
   * @param timeout Default request timeout in milliseconds (default 30_000)
   * @param opts    `{ token, scheme }` — gateway bearer (defaults to `MYCELIUM_GATEWAY_TOKEN`) and
   *                `"http"` (default) or `"https"` for a gateway serving TLS
   */
  constructor(
    host = "127.0.0.1",
    port = 7946,
    timeout = 30_000,
    opts: AuthOptions = {},
  ) {
    this.base = baseUrl(host, port, opts.scheme);
    this.timeout = timeout;
    this.auth = authHeaders(resolveToken(opts.token));
  }

  // ── Internals ─────────────────────────────────────────────────────────────

  private async _get(path: string, params?: Record<string, string>): Promise<unknown> {
    const url = new URL(path, this.base);
    if (params) {
      for (const [k, v] of Object.entries(params)) url.searchParams.set(k, v);
    }
    const resp = await fetch(url.toString(), {
      headers: this.auth,
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!resp.ok) throw new Error(`GET ${path} failed: ${resp.status}`);
    // Lossless: a 64-bit integer above 2^53 arrives as a decimal string, not a rounded double (S1).
    return parseLossless(await resp.text());
  }

  /**
   * `subject` names the key or stream a write carries, so a `protected_key` / `protected_stream`
   * refusal can say which one it was.
   */
  private async _post(path: string, body: unknown, subject: Subject = {}): Promise<unknown> {
    const resp = await fetch(`${this.base}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json", ...this.auth },
      // A `bigint` in the body is written as a bare JSON integer, exactly (S1).
      body: stringifyLossless(body),
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!resp.ok) await this._fail("POST", path, resp, subject);
    return parseLossless(await resp.text());
  }

  private async _delete(path: string, subject: Subject = {}): Promise<void> {
    const resp = await fetch(`${this.base}${path}`, {
      method: "DELETE",
      headers: this.auth,
      signal: AbortSignal.timeout(this.timeout),
    });
    if (!resp.ok) await this._fail("DELETE", path, resp, subject);
  }

  /** Throws for a non-2xx answer: the typed refusals by their `error` code, else the status and body. */
  private async _fail(method: string, path: string, resp: Response, subject: Subject): Promise<never> {
    const text = await resp.text().catch(() => "");
    if (resp.status === 403) {
      let body: { error?: string; kind?: string; message?: string } = {};
      try { body = JSON.parse(text) ?? {}; } catch { /* not JSON: fall through */ }
      if (body.error === "protected_kind") {
        throw new ProtectedKindError(body.kind ?? "", body.message ?? "protected kind");
      }
      if (body.error === "protected_key") {
        throw new ProtectedKeyError(subject.key ?? "", body.message ?? "protected key");
      }
      if (body.error === "protected_stream") {
        throw new ProtectedStreamError(subject.stream ?? "", body.message ?? "protected stream");
      }
    }
    if (resp.status === 409) {
      let body: { error?: string } = {};
      try { body = JSON.parse(text) ?? {}; } catch { /* not JSON: fall through */ }
      if (body.error === "superseded") throw new SupersededError();
    }
    // 504 is the gateway's answer to an expired deadline (`rpc/call`, `scatter`): a
    // `TimeoutError`, as the README promises and the Python SDK raises — not a generic failure.
    if (resp.status === 504) {
      throw Object.assign(new Error(`${method} ${path} timed out: ${text}`), { name: "TimeoutError" });
    }
    throw new Error(`${method} ${path} failed: ${resp.status} ${text}`);
  }

  private _sseUrl(path: string, params?: Record<string, string>): string {
    const url = new URL(path, this.base);
    if (params) {
      for (const [k, v] of Object.entries(params)) url.searchParams.set(k, v);
    }
    return url.toString();
  }

  // ── Federated domains ─────────────────────────────────────────────────────

  /**
   * The federation verbs on this node's gateway (item 2 row 11): discover a partner domain, read
   * the exports it granted us, and call one. Shares this agent's base URL, bearer and timeout.
   *
   * See `./federation` — the one thing to read first is what a refusal's `sent` and `delivery`
   * fields mean, because `delivery: "unknown"` is not a failure.
   */
  federation(): Federation {
    return new Federation(this.base, this.auth, this.timeout);
  }

  /**
   * The artifact catalogue's gateway door (plan A3): publish an **already-signed** catalogue line
   * into the gossiped `installable/` catalogue. The key stays with the publisher; the bytes stay
   * at the library. See `./artifacts`.
   */
  artifacts(): Artifacts {
    return new Artifacts(this.base, this.auth, this.timeout);
  }

  // ── Introspection ─────────────────────────────────────────────────────────

  /** Returns `{ status: "ok", node_id: "..." }`. */
  async health(): Promise<{ status: string; node_id: string }> {
    return this._get("/health") as Promise<{ status: string; node_id: string }>;
  }

  /** Returns a snapshot of store statistics. */
  async stats(): Promise<Record<string, unknown>> {
    return this._get("/stats") as Promise<Record<string, unknown>>;
  }

  /** This node's `"ip:port"` identifier (cached after first call). */
  get nodeId(): Promise<string> {
    if (this._nodeId) return Promise.resolve(this._nodeId);
    return this.health().then((h) => {
      this._nodeId = h.node_id;
      return h.node_id;
    });
  }

  // ── Capability advertisement ──────────────────────────────────────────────

  /**
   * Advertises a capability on the mesh. Re-asserted every `intervalSecs` so
   * late joiners discover it. Returns a `CapabilityHandle`; call `.drop()` or
   * use `await using` to retract.
   *
   * Pass `leaseSecs` to bind the advertisement to THIS process's liveness:
   * the node retracts it unless `handle.heartbeat()` is called within every
   * `leaseSecs` window (beat at `leaseSecs / 3` for margin). Without it the
   * node's refresh task keeps the advert alive until `.drop()` or node
   * shutdown — which outlives a crashed client.
   */
  async advertiseCapability(
    ns: string,
    name: string,
    options: {
      intervalSecs?: number;
      leaseSecs?: number;
      attributes?: Record<string, unknown>;
      authorizedCallers?: string[];
    } = {},
  ): Promise<CapabilityHandle> {
    const data = await this._post("/gateway/capability/advertise", {
      ns,
      name,
      interval_secs: options.intervalSecs ?? 30,
      ...(options.leaseSecs !== undefined ? { lease_secs: wholeSeconds(options.leaseSecs, 1, "leaseSecs") } : {}),
      attributes: options.attributes ?? {},
      authorized_callers: options.authorizedCallers ?? [],
    }) as { handle_id: string };
    const handleId = data.handle_id;
    return new CapabilityHandle(
      handleId,
      async () => {
        await this._delete(`/gateway/capability/${pathSegment(handleId)}`);
      },
      options.leaseSecs !== undefined
        ? async () => {
            await this._post(`/gateway/capability/${pathSegment(handleId)}/heartbeat`, {});
          }
        : undefined,
    );
  }

  /**
   * Declare a unit file's capabilities, requirements and groups on the node, under one handle
   * (design-time-tooling.md Q2). Pass the file's **text** — read it yourself (`fs.readFile`); the node
   * parses and validates it with its own loader, so this SDK carries no TOML parser. Hosting sections
   * (`[hosts]`, `[[presence]]`, `[[activation]]`, `[[serve]]`) are a stem's and are refused (422). The handle's
   * `drop()` retracts the whole unit; with `leaseSecs`, call `heartbeat()` within every window.
   */
  async declareUnits(
    tomlText: string,
    options: { intervalSecs?: number; leaseSecs?: number } = {},
  ): Promise<UnitHandle> {
    const data = (await this._post("/gateway/units/declare", {
      toml: tomlText,
      interval_secs: options.intervalSecs ?? 30,
      ...(options.leaseSecs !== undefined ? { lease_secs: wholeSeconds(options.leaseSecs, 1, "leaseSecs") } : {}),
    })) as {
      handle_id: string;
      principal: string | null;
      declared: { capabilities: number; requirements: number; groups: number };
      not_enforced: string[];
    };
    const handleId = data.handle_id;
    return new UnitHandle(
      handleId,
      async () => {
        await this._delete(`/gateway/capability/${pathSegment(handleId)}`);
      },
      options.leaseSecs !== undefined
        ? async () => {
            await this._post(`/gateway/capability/${pathSegment(handleId)}/heartbeat`, {});
          }
        : undefined,
      data.principal,
      data.declared,
      data.not_enforced,
    );
  }

  /**
   * Returns all live providers matching `(ns, name)`.
   * Pass `callerId` to respect `authorized_callers` restrictions.
   */
  async resolveCapability(
    ns: string,
    name: string,
    options: { callerId?: string } = {},
  ): Promise<Array<Record<string, unknown>>> {
    const params: Record<string, string> = { ns, name };
    if (options.callerId) params.caller_id = options.callerId;
    // The gateway answers `{"providers": [...]}`; this returned the envelope as if it were the array.
    const data = await this._get("/gateway/capability/resolve", params) as {
      providers?: Array<Record<string, unknown>>;
    };
    return data.providers ?? [];
  }

  /**
   * Returns demand pressure: `demandPressure > 1.0` signals a supply gap.
   */
  async demand(ns: string, name: string): Promise<DemandStatus> {
    const raw = await this._get("/gateway/demand", { ns, name }) as {
      ns: string; name: string; providers: number; requirers: number; demand_pressure: number;
    };
    return {
      ns: raw.ns,
      name: raw.name,
      providers: raw.providers,
      requirers: raw.requirers,
      demandPressure: raw.demand_pressure,
    };
  }

  // ── Signal mesh ───────────────────────────────────────────────────────────

  /**
   * Fires a signal into the mesh.
   * @param scope `"cluster"` (every node; default), `"group:NAME"`, or `"node:IP:PORT"`. `"system"` is a deprecated alias.
   * @returns the gateway's `ok`: `true` when the signal was handed to local delivery and queued for
   *   gossip fan-out; `false` when the gossip queue was full (local delivery still occurred). It does
   *   not say any subscriber ran.
   */
  async emit(
    kind: string,
    payload: Buffer | Uint8Array = Buffer.alloc(0),
    options: { scope?: string } = {},
  ): Promise<boolean> {
    const data = await this._post("/gateway/signal/emit", {
      kind,
      payload_b64: b64(payload),
      scope: options.scope ?? "cluster",
    }) as { ok: boolean };
    return data.ok === true;
  }

  /**
   * Async generator yielding admitted signals of `kind` as SSE events.
   */
  async *onSignal(kind: string): AsyncGenerator<Signal> {
    const url = this._sseUrl(`/gateway/signal/sse/${pathSegment(kind)}`);
    yield* sseStream<Signal>({ url, headers: this.auth }, (data, event) => {
      const raw = parseLossless(data) as {
        kind?: string; sender: string; payload_b64: string; nonce: number | string;
      };
      return {
        // The kind is the SSE event name; the data object carries it only on newer gateways.
        kind: raw.kind ?? event ?? kind,
        sender: raw.sender,
        payload: fromb64(raw.payload_b64),
        nonce: toBigInt(raw.nonce),
      };
    });
  }

  // ── KV store ──────────────────────────────────────────────────────────────

  /** Reads a key, returns `null` if absent or tombstoned. */
  async get(key: string): Promise<Buffer | null> {
    // `{"found": false}` carries no `value_b64` at all; checking it against `null` threw on absence.
    const data = await this._get("/gateway/kv", { key }) as { found?: boolean; value_b64?: string | null };
    return data.found && typeof data.value_b64 === "string" ? fromb64(data.value_b64) : null;
  }

  /**
   * Writes a key and queues it for gossip; resolves to the write's receipt — rung 1 always, rung 2
   * as `localDurability` (every field `null` on a pre-v2.16.0 gateway).
   */
  async set(key: string, value: Buffer | Uint8Array): Promise<KvReceipt> {
    const data = (await this._post("/gateway/kv", { key, value_b64: b64(value) }, { key })) as {
      operation_id?: string;
      local_durability?: string;
      local_durability_error?: string;
    } | null;
    return {
      operationId: typeof data?.operation_id === "string" ? data.operation_id : null,
      localDurability: typeof data?.local_durability === "string" ? data.local_durability : null,
      localDurabilityError:
        typeof data?.local_durability_error === "string" ? data.local_durability_error : null,
    };
  }

  /** Tombstones a key and queues for gossip. */
  async delete(key: string): Promise<void> {
    // The refusal's body is surfaced: it dropped it and threw only `failed: 403`.
    await this._delete(`/gateway/kv?key=${encodeURIComponent(key)}`, { key });
  }

  /** Lists live keys with an optional prefix filter. */
  async keys(prefix = ""): Promise<string[]> {
    const data = await this._get("/gateway/kv/keys", { prefix }) as { keys: string[] };
    return data.keys;
  }

  /** Returns all live key-value pairs under `prefix`. */
  async scanPrefix(prefix: string): Promise<Record<string, Buffer>> {
    const ks = await this.keys(prefix);
    const pairs = await Promise.all(
      ks.map(async (k) => [k, await this.get(k)] as const),
    );
    return Object.fromEntries(
      pairs.filter(([, v]) => v !== null).map(([k, v]) => [k, v as Buffer]),
    );
  }

  /**
   * Writes `value` and waits for peer acknowledgements.
   *
   * Since the substrate's item 1 PR 4b the gateway asks each peer whether it holds the operation.
   * The count is peers whose store holds this exact write and whose WAL fdatasync returned Ok.
   * Peers that do not answer are unknown, never "did not persist" — a timeout is not evidence the
   * write failed, and the write is applied and gossiped either way.
   * Returns the confirmed peer count on success; throws `TimeoutError` on timeout.
   */
  async setWithMinAcks(
    key: string,
    value: Buffer | Uint8Array,
    minAcks: number,
    options: { timeoutSecs?: number } = {},
  ): Promise<number> {
    const data = await this._post("/gateway/kv/quorum", {
      key,
      value_b64: b64(value),
      min_acks: minAcks,
      timeout_secs: options.timeoutSecs ?? 5,
    }, { key }) as { ok: boolean; acks_received: number; error?: string };
    if (!data.ok) {
      throw Object.assign(new Error(`set_with_min_acks timeout (${data.acks_received} acks)`), {
        name: "TimeoutError",
        acksReceived: data.acks_received,
      });
    }
    return data.acks_received;
  }

  // ── RPC ───────────────────────────────────────────────────────────────────

  /**
   * Blocking point-to-point RPC call. Throws `TimeoutError` if no reply arrives. `timeoutSecs` is
   * whole seconds, a fraction rounded up (the gateway clamps to 1–300).
   *
   * Protected kinds (`mcp.invoke`, `skill.invoke`, `llm.invoke`, and any the operator lists) are
   * refused with `ProtectedKindError`: call tools through `/mcp` and skills through `A2aClient`,
   * where authority is checked.
   */
  async rpcCall(
    target: string,
    method: string,
    payload: Buffer | Uint8Array = Buffer.alloc(0),
    options: { timeoutSecs?: number } = {},
  ): Promise<Buffer> {
    // The route reads `method`. This sent `kind` until 2026-09-25, so every call was a 400.
    const data = await this._post("/gateway/rpc/call", {
      target,
      method,
      payload_b64: b64(payload),
      // Whole seconds: the gateway reads `as_u64()`, and a fraction became its 30 s default silently.
      timeout_secs: wholeSeconds(options.timeoutSecs ?? 5),
    }) as { ok: boolean; result_b64?: string; error?: string };
    if (!data.ok) throw Object.assign(new Error("rpc_call timeout"), { name: "TimeoutError" });
    return fromb64(data.result_b64!);
  }

  /**
   * Async generator yielding incoming RPC requests of `kind`.
   * Call `rpcRespond` for each request to complete the round-trip.
   *
   * On a scoped gateway, serving needs the `mesh:serve` scope (for this stream and for
   * `rpcRespond`). A serving agent should not hold `mesh:write`, which also opens `rpcCall`.
   */
  async *rpcServe(kind: string): AsyncGenerator<RpcRequest> {
    const url = this._sseUrl(`/gateway/rpc/serve/${pathSegment(kind)}`);
    yield* sseStream<RpcRequest>({ url, headers: this.auth }, (data, event) => {
      const raw = JSON.parse(data) as {
        kind?: string; nonce_hex: string; sender: string; payload_b64: string;
        caller?: { principal: string; via: string; scopes: string[]; attested: boolean };
      };
      const req: RpcRequest = {
        // The serve stream's data has no `kind`: it is the SSE event name (sweep 2026-10-06).
        kind: raw.kind ?? event ?? kind,
        nonceHex: raw.nonce_hex,
        sender: raw.sender,
        payload: fromb64(raw.payload_b64),
      };
      if (raw.caller) req.caller = raw.caller;
      return req;
    });
  }

  /** Sends a reply to an in-flight RPC request. */
  async rpcRespond(request: RpcRequest, result: Buffer | Uint8Array = Buffer.alloc(0)): Promise<void> {
    await this._post("/gateway/rpc/respond", {
      nonce_hex: request.nonceHex,
      sender: request.sender,
      result_b64: b64(result),
    });
  }

  /**
   * Fan-out RPC to multiple targets; waits for at least `minOk` replies.
   * Throws `TimeoutError` if the threshold is not met.
   *
   * `minOk` defaults to **1** — the gateway's own default and the Python SDK's: the call returns at the
   * first reply and the other targets are cancelled. Pass `minOk: targets.length` to wait for every
   * target. `timeoutSecs` defaults to 10, the gateway's. Before 0.2.4 this SDK alone defaulted to every
   * target and 5 s.
   */
  async scatterGather(
    targets: string[],
    method: string,
    payload: Buffer | Uint8Array = Buffer.alloc(0),
    options: { minOk?: number; timeoutSecs?: number } = {},
  ): Promise<Array<{ sender: string; result: Buffer }>> {
    const data = await this._post("/gateway/scatter", {
      targets,
      // `gw_scatter` reads `method`; this sent `kind`, and every call was refused 400 (sweep 2026-10-06).
      method,
      payload_b64: b64(payload),
      min_ok: options.minOk ?? 1,
      // Whole seconds: the gateway reads `as_u64()`, and a fraction became its 10 s default silently.
      timeout_secs: wholeSeconds(options.timeoutSecs ?? 10),
    }) as { ok: boolean; replies?: Array<{ sender: string; result_b64: string }>; error?: string };
    if (!data.ok) throw Object.assign(new Error("scatter_gather timeout"), { name: "TimeoutError" });
    return (data.replies ?? []).map((r) => ({
      sender: r.sender,
      result: fromb64(r.result_b64),
    }));
  }

  // ── Mailbox ───────────────────────────────────────────────────────────────

  /**
   * Delivers a mailbox event to `target`'s mailbox.
   * At-least-once within TTL; gossiped to all peers.
   */
  async deliverEvent(target: string, kind: string, payload: Buffer | Uint8Array = Buffer.alloc(0)): Promise<void> {
    await this._post("/gateway/mailbox/deliver", {
      target,
      kind,
      payload_b64: b64(payload),
    });
  }

  /**
   * Streams events of `kind` addressed to this node.
   * Events are delivered in HLC-causal order and tombstoned on delivery.
   */
  async *mailbox(kind: string): AsyncGenerator<MailboxEvent> {
    const url = this._sseUrl(`/gateway/mailbox/${pathSegment(kind)}`);
    yield* sseStream<MailboxEvent>({ url, headers: this.auth }, (data) => {
      const raw = JSON.parse(data) as {
        kind: string; sender: string; payload_b64: string;
      };
      return {
        kind: raw.kind,
        sender: raw.sender,
        payload: fromb64(raw.payload_b64),
      };
    });
  }

  // ── Consistency & Ordering Overlay ────────────────────────────────────────

  /**
   * Ballot-serialized (consensus-durable) write: runs a consensus round before writing.
   * Concurrent writes to the same key are totally ordered by ballot number.
   * `consistentGet` is a local read and may lag by up to one anti-entropy round.
   * Resolves to a {@link CommitResult} — `.persisted` says whether the committed slot also
   * reached the gateway node's own disk (since 0.1.1; `null` from a pre-v2.4.2 node).
   * Rejects if the commit itself failed.
   */
  async consistentSet(key: string, value: Buffer | Uint8Array): Promise<CommitResult> {
    const data = await this._post("/gateway/overlay/consistent/set", {
      key,
      value_b64: b64(value),
    }, { key }) as { persisted?: boolean };
    return commitResult(data);
  }

  /** Read latest ballot-committed value visible to this node (local, eventually consistent). */
  async consistentGet(key: string): Promise<Buffer | null> {
    const data = await this._get("/gateway/overlay/consistent/get", { key }) as {
      value_b64?: string | null;
    };
    return typeof data.value_b64 === "string" ? fromb64(data.value_b64) : null;
  }

  /**
   * Acquires a named cluster lock via consensus.
   * Returns a `LockGuard`; use `await using` or call `.release()`.
   */
  async distributedLock(
    name: string,
    options: { ttlSecs?: number } = {},
  ): Promise<LockGuard> {
    const data = await this._post("/gateway/overlay/lock/acquire", {
      name,
      ttl_secs: wholeSeconds(options.ttlSecs ?? 30, 1, "ttlSecs"),
    }) as { guard_id: string; token: string };
    const guardId = data.guard_id;
    return new LockGuard(guardId, toBigInt(data.token), async () => {
      await this._delete(`/gateway/overlay/lock/${pathSegment(guardId)}`);
    });
  }

  /**
   * One-shot leader election for `group`.
   * Returns the elected node's `"ip:port"` string.
   */
  async electLeader(group: string): Promise<string> {
    const data = await this._post("/gateway/overlay/elect", { group }) as { leader: string };
    return data.leader;
  }

  /**
   * Proposes `value` for `slot` requiring independent quorum from each group.
   *
   * Commits only when **all** specified groups individually reach their
   * `quorum` fraction. A single ballot round — no partial commitment possible.
   *
   * @param slot   - Consensus slot name (namespaced by the caller).
   * @param value  - Payload bytes to commit.
   * @param groups - Per-group quorum requirements.
   * @returns a {@link CommitResult} (`.persisted` — local durability on the gateway node).
   *
   * @example
   * ```ts
   * await agent.crossGroupPropose("pipeline/config", Buffer.from("v2"), [
   *   { group: "llm-workers", quorum: 0.5, veto: false },
   *   { group: "compliance",  quorum: 0.5, veto: true  },
   * ]);
   * ```
   */
  async crossGroupPropose(
    slot: string,
    value: Buffer | Uint8Array,
    groups: Array<{ group: string; quorum?: number; veto?: boolean }>,
  ): Promise<CommitResult> {
    const data = await this._post("/gateway/consensus/cross_group_propose", {
      slot,
      value_b64: b64(value),
      groups: groups.map((g) => ({
        group:  g.group,
        quorum: g.quorum ?? 0.5,
        veto:   g.veto   ?? false,
      })),
    }) as { persisted?: boolean };
    return commitResult(data);
  }

  /**
   * Appends `value` to the named log stream.
   * Returns the HLC timestamp (use as cursor for `scanLog` or `subscribeLog`).
   */
  async append(stream: string, value: Buffer | Uint8Array = Buffer.alloc(0)): Promise<bigint> {
    const data = await this._post("/gateway/overlay/log/append", {
      stream,
      value_b64: b64(value),
    }, { stream }) as { hlc: number | string };
    return toBigInt(data.hlc);
  }

  /**
   * Range scan over a log stream. Returns `LogEntry[]` sorted by HLC.
   */
  async scanLog(
    stream: string,
    options: { fromHlc?: bigint; toHlc?: bigint } = {},
  ): Promise<LogEntry[]> {
    // The gateway reads `from` / `to` (inclusive / exclusive) and answers a bare array. This sent
    // `from_hlc` / `to_hlc`, which the gateway ignored, and mapped `data.entries` over the array.
    const params: Record<string, string> = { stream };
    if (options.fromHlc !== undefined) params.from = options.fromHlc.toString();
    if (options.toHlc !== undefined) params.to = options.toHlc.toString();
    const data = await this._get("/gateway/overlay/log/scan", params) as Array<{
      hlc: number | string; value_b64: string;
    }>;
    return data.map((e) => ({
      hlc: toBigInt(e.hlc),
      value: fromb64(e.value_b64),
    }));
  }

  /** Tombstones all entries with `hlc < beforeHlc`. Gossips tombstones to peers. */
  async compactLog(stream: string, beforeHlc: bigint): Promise<void> {
    // A JSON integer, as the gateway's `u64` requires; a string was refused with 422.
    await this._post("/gateway/overlay/log/compact", {
      stream,
      before_hlc: beforeHlc,
    }, { stream });
  }

  /**
   * Live SSE subscription. Yields entries with `hlc >= sinceHlc` (inclusive), then new ones as they
   * arrive. To resume after an entry you have handled, pass `entry.hlc + 1n`.
   */
  async *subscribeLog(stream: string, options: { sinceHlc?: bigint } = {}): AsyncGenerator<LogEntry> {
    const params: Record<string, string> = { stream };
    // `since`, as the gateway reads it; `since_hlc` was ignored and every resume replayed from 0.
    if (options.sinceHlc !== undefined) params.since = options.sinceHlc.toString();
    const url = this._sseUrl("/gateway/overlay/log/subscribe", params);
    yield* sseStream<LogEntry>({ url, headers: this.auth }, (data) => {
      const raw = parseLossless(data) as { hlc: number | string; value_b64: string };
      return { hlc: toBigInt(raw.hlc), value: fromb64(raw.value_b64) };
    });
  }

  /**
   * Consumer-group subscription: at most one consumer per group processes an
   * entry at a time. The offset is persisted in the gossip KV.
   */
  async *subscribeLogGroup(stream: string, group: string): AsyncGenerator<LogEntry> {
    const url = this._sseUrl("/gateway/overlay/log/group/subscribe", { stream, group });
    yield* sseStream<LogEntry>({ url, headers: this.auth }, (data) => {
      const raw = parseLossless(data) as { hlc: number | string; value_b64: string };
      return { hlc: toBigInt(raw.hlc), value: fromb64(raw.value_b64) };
    });
  }

  /**
   * Sends `payload` to `target` and waits for an explicit application-level ACK.
   * Returns `"acknowledged"` or `"timeout"`; a refusal (an unknown target, a protected kind, a
   * malformed request) is thrown, never reported as a timeout.
   *
   * `timeoutSecs` is whole seconds: a fraction is rounded **up**, and the gateway clamps to 1–300.
   */
  async emitReliable(
    target: string,
    kind: string,
    payload: Buffer | Uint8Array = Buffer.alloc(0),
    options: { timeoutSecs?: number } = {},
  ): Promise<"acknowledged" | "timeout"> {
    const data = await this._post("/gateway/overlay/emit_reliable", {
      target,
      kind,
      payload_b64: b64(payload),
      timeout_secs: wholeSeconds(options.timeoutSecs ?? 5),
    }) as { ack?: "acknowledged" | "timeout" };
    // The gateway answers `{"ack": …}`; this read `status`, which is never sent.
    if (data.ack !== "acknowledged" && data.ack !== "timeout") {
      throw new Error(`emit_reliable: unexpected reply ${JSON.stringify(data)}`);
    }
    return data.ack;
  }

  // ── Cluster sharding ────────────────────────────────────────────────────

  /**
   * Returns the consistent-hash owner node-id for `key` among providers of `ns/name`.
   * Throws when no providers match the filter.
   */
  async shardFor(ns: string, name: string, key: string): Promise<string> {
    const resp = await fetch(
      `${this.base}/gateway/shard/${pathSegment(ns)}/${pathSegment(name)}?key=${encodeURIComponent(key)}`,
      { headers: this.auth },
    );
    if (resp.status === 404) throw new Error(`no providers for ${ns}/${name}`);
    if (!resp.ok) throw new Error(`shardFor failed: ${resp.status}`);
    const data = (await resp.json()) as { owner: string };
    return data.owner;
  }

  /**
   * Emits `kind` signal to the consistent-hash owner for `key` among providers of `ns/name`.
   * Returns the owner node-id string. Throws when no providers match the filter.
   */
  async emitSharded(
    kind: string,
    ns: string,
    name: string,
    key: string,
    payload: Buffer | Uint8Array = Buffer.alloc(0),
  ): Promise<string> {
    const data = await this._post("/gateway/shard/emit", {
      kind,
      ns,
      name,
      shard_key:   key,
      payload_b64: b64(payload),
    }) as { ok: boolean; owner?: string; error?: string };
    if (!data.ok) throw new Error(data.error ?? "no providers");
    return data.owner!;
  }
}
