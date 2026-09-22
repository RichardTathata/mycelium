import { authHeaders, resolveToken, type AuthOptions } from "./auth";
/**
 * A2A (Agent-to-Agent) protocol client for Mycelium.
 *
 * Lets TypeScript callers discover and invoke skills on A2A-protocol nodes
 * (requires the `a2a` cargo feature on the server side).
 *
 * @example
 * ```typescript
 * const client = new A2aClient("http://localhost:8300");
 * const card   = await client.fetchCard();
 * const reply  = await client.send("compute/gpu", "hello");
 * for await (const event of client.stream("compute/gpu", "hello")) {
 *   console.log(event.status.state);
 * }
 * ```
 */

// ── Refusals ───────────────────────────────────────────────────────────────

/** A JSON-RPC error from the A2A endpoint, with the code kept as a number. */
export class A2aError extends Error {
  constructor(
    readonly code: number,
    readonly detail: string,
    readonly data: Record<string, unknown> = {},
  ) {
    super(`A2A error ${code}: ${detail}`);
    this.name = "A2aError";
  }
}

/**
 * The gateway's action evaluator refused the call.
 *
 * **Three refusals, and they must not be collapsed.** This is the distinction the whole
 * authorisation slice exists to keep, and it has to survive the trip to a caller:
 *
 * - `action_denied` (-32030) — an authority decided **no**; a prohibition matched. Do not retry;
 *   the answer will not change until the policy does.
 * - `authority_not_established` (-32031) — **nobody decided**. No clause covered the action, a
 *   fact could not be established, the policy revision was not the expected one, or the operation
 *   is outside the reviewed catalogue. This is *not* evidence of drift: treating it as a violation
 *   reports a breach that never happened.
 * - `evidence_not_recorded` (-32032) — the policy **permitted** it, but the decision could not be
 *   recorded, so it was refused anyway. An action allowed to proceed with no record of why is an
 *   unlogged gate, not governance.
 *
 * `policyRevision` is what tells a *stale policy* apart from a real denial: if the refusal names a
 * revision you did not deploy, redeploy and retry rather than stop.
 */
export class ActionRefusedError extends A2aError {
  static readonly DENIED = "action_denied";
  static readonly NOT_ESTABLISHED = "authority_not_established";
  static readonly NOT_RECORDED = "evidence_not_recorded";

  /** The three JSON-RPC codes the evaluator seam uses. */
  static readonly CODES: Record<number, string> = {
    [-32030]: ActionRefusedError.DENIED,
    [-32031]: ActionRefusedError.NOT_ESTABLISHED,
    [-32032]: ActionRefusedError.NOT_RECORDED,
  };

  constructor(code: number, detail: string, data: Record<string, unknown> = {}) {
    super(code, detail, data);
    this.name = "ActionRefusedError";
  }

  /** The machine-readable reason, from `data.reason` or the code. */
  get reason(): string {
    return (this.data.reason as string) ?? ActionRefusedError.CODES[this.code] ?? "unknown";
  }

  /** An authority decided no. */
  get denied(): boolean {
    return this.reason === ActionRefusedError.DENIED;
  }

  /** Nobody decided — **never** the same as a denial. */
  get authorityNotEstablished(): boolean {
    return this.reason === ActionRefusedError.NOT_ESTABLISHED;
  }

  /** Permitted, but unrecordable, so refused. */
  get evidenceNotRecorded(): boolean {
    return this.reason === ActionRefusedError.NOT_RECORDED;
  }

  /** Which policy artifact decided, when the gateway said. */
  get policyRevision(): string | undefined {
    return this.data.policy_revision as string | undefined;
  }

  /** What the evaluator reports it checked. */
  get checked(): string[] {
    return (this.data.checked as string[]) ?? [];
  }
}

/** Throw the most specific error type this JSON-RPC error warrants. */
export function raiseForError(error: {
  code: number;
  message: string;
  data?: Record<string, unknown>;
}): never {
  if (error.code in ActionRefusedError.CODES) {
    throw new ActionRefusedError(error.code, error.message, error.data ?? {});
  }
  throw new A2aError(error.code, error.message, error.data ?? {});
}

// ── Wire types ─────────────────────────────────────────────────────────────

export interface AgentCard {
  name:         string;
  url:          string;
  version:      string;
  capabilities: A2aCapabilities;
  skills:       AgentSkill[];
}

export interface A2aCapabilities {
  streaming: boolean;
}

export interface AgentSkill {
  id:          string;
  name:        string;
  description: string;
}

export interface Task {
  id:        string;
  status:    TaskStatus;
  artifacts?: Artifact[];
}

export interface TaskStatus {
  state: "submitted" | "working" | "completed" | "failed" | "canceled";
}

export interface Artifact {
  parts: Part[];
}

export type Part = { type: "text"; text: string };

export interface TaskStatusUpdate {
  id:     string;
  status: TaskStatus;
  /** Present when state is "completed". */
  artifacts?: Artifact[];
  error?: string;
}

// ── Client ─────────────────────────────────────────────────────────────────

/**
 * HTTP client for A2A-protocol Mycelium nodes.
 *
 * All methods use the standard `fetch` API — no runtime-specific dependencies.
 */
export class A2aClient {
  private readonly baseUrl: string;
  private readonly auth: Record<string, string>;
  private readonly timeoutMs: number;

  constructor(agentCardUrl: string, { timeoutMs = 30_000, token }: { timeoutMs?: number } & AuthOptions = {}) {
    this.baseUrl   = agentCardUrl.replace(/\/$/, "");
    this.timeoutMs = timeoutMs;
    this.auth      = authHeaders(resolveToken(token));
  }

  // ── Discovery ───────────────────────────────────────────────────────────

  /** Fetch the AgentCard from `/.well-known/agent.json`. */
  async fetchCard(): Promise<AgentCard> {
    const resp = await fetch(`${this.baseUrl}/.well-known/agent.json`, {
      headers: this.auth,
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    if (!resp.ok) throw new Error(`AgentCard fetch failed: ${resp.status}`);
    return resp.json() as Promise<AgentCard>;
  }

  // ── Synchronous dispatch ────────────────────────────────────────────────

  /**
   * Send a `tasks/send` request and return the reply text.
   *
   * @param skillId  Skill ID in `"ns/name"` format, e.g. `"compute/gpu"`.
   * @param message  Plain-text input to the skill.
   * @returns First text part of the completed task's first artifact.
   * @throws {Error} On JSON-RPC error or HTTP failure.
   */
  async send(skillId: string, message: string): Promise<string> {
    const taskId  = crypto.randomUUID();
    const payload = {
      jsonrpc: "2.0",
      id:      1,
      method:  "tasks/send",
      params:  {
        id:       taskId,
        skillId,
        message:  { role: "user", parts: [{ type: "text", text: message }] },
      },
    };

    const resp = await fetch(`${this.baseUrl}/a2a`, {
      method:  "POST",
      headers: { "Content-Type": "application/json", ...this.auth },
      body:    JSON.stringify(payload),
      signal:  AbortSignal.timeout(this.timeoutMs + 5_000),
    });
    if (!resp.ok) throw new Error(`/a2a responded ${resp.status}`);

    const body = (await resp.json()) as {
      result?: Task;
      error?: { code: number; message: string; data?: Record<string, unknown> };
    };
    if (body.error) {
      raiseForError(body.error);
    }
    return extractText(body.result ?? { id: taskId, status: { state: "failed" } });
  }

  // ── SSE streaming ────────────────────────────────────────────────────────

  /**
   * Send a `tasks/sendSubscribe` request and yield status events until the
   * task reaches a terminal state (`completed`, `failed`, or `canceled`).
   *
   * @param skillId  Skill ID in `"ns/name"` format.
   * @param message  Plain-text input.
   */
  async *stream(skillId: string, message: string): AsyncGenerator<TaskStatusUpdate> {
    const taskId  = crypto.randomUUID();
    const payload = {
      jsonrpc: "2.0",
      id:      1,
      method:  "tasks/sendSubscribe",
      params:  {
        id:      taskId,
        skillId,
        message: { role: "user", parts: [{ type: "text", text: message }] },
      },
    };

    const resp = await fetch(`${this.baseUrl}/a2a`, {
      method:  "POST",
      headers: { "Content-Type": "application/json", ...this.auth },
      body:    JSON.stringify(payload),
      signal:  AbortSignal.timeout(this.timeoutMs + 5_000),
    });
    if (!resp.ok) throw new Error(`/a2a SSE responded ${resp.status}`);
    if (!resp.body) throw new Error("/a2a SSE response has no body");

    const reader  = resp.body.getReader();
    const decoder = new TextDecoder();
    let   buffer  = "";

    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          if (line.startsWith("data:")) {
            const data = line.slice(5).trim();
            if (!data) continue;
            try {
              const event = JSON.parse(data) as TaskStatusUpdate;
              yield event;
              const { state } = event.status;
              if (state === "completed" || state === "failed" || state === "canceled") return;
            } catch {
              // ignore malformed SSE data
            }
          }
        }
      }
    } finally {
      reader.cancel().catch(() => undefined);
    }
  }
}

// ── Helpers ────────────────────────────────────────────────────────────────

function extractText(task: Task): string {
  const artifacts = task.artifacts ?? [];
  if (!artifacts.length) return "";
  const parts = artifacts[0].parts ?? [];
  if (!parts.length) return "";
  const part = parts[0];
  return part.type === "text" ? part.text : "";
}
