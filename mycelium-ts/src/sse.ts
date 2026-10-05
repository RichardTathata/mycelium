import { createParser, type ParseEvent } from "eventsource-parser";

/** Options for {@link sseStream}. */
export interface SseOptions {
  /**
   * The most parsed events held for a consumer that has not taken them yet (default 1024). Past
   * it the stream throws `SseOverflowError` — an ordered stream is never silently dropped from.
   */
  maxPending?: number;
  /** Cancels the request and ends the stream when aborted. */
  signal?: AbortSignal;
}

/** A consumer fell further behind than `maxPending`; the stream was closed rather than truncated. */
export class SseOverflowError extends Error {
  constructor(public readonly maxPending: number) {
    super(`SSE consumer fell behind: more than ${maxPending} events pending (maxPending); the stream was closed rather than dropping an event`);
    this.name = "SseOverflowError";
  }
}

/**
 * Reads an SSE stream from `url` (GET request) and yields parsed objects. `parse` receives the
 * event's `data` and its `event` name (the gateway sends a signal's kind as the event name).
 *
 * **The stream has a lifetime** (realignment repairs S3; the review's F09). It reads on demand — no
 * background pump — so a slow consumer applies backpressure to the connection instead of growing a
 * buffer; and when the generator ends for any reason (`break`, `return()`, a thrown error, or
 * `options.signal`) the reader is cancelled and the request aborted, closing the connection. Before
 * this, a consumer that stopped left the read loop and the connection running, buffering every later
 * event without bound.
 */
export async function* sseStream<T>(
  target: string | { url: string; headers?: Record<string, string> },
  // `event` is the SSE event name (a string, or undefined). Typed loosely so an existing caller that
  // passes `JSON.parse` — whose second parameter is a reviver — keeps compiling; at run time
  // `JSON.parse` ignores a second argument that is not a function.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  parse: (data: string, event?: any) => T,
  options: SseOptions = {},
): AsyncGenerator<T> {
  const { url, headers } = typeof target === "string" ? { url: target, headers: undefined } : target;
  const maxPending = Math.max(1, options.maxPending ?? 1024);
  const controller = new AbortController();
  const onAbort = () => controller.abort();
  options.signal?.addEventListener("abort", onAbort, { once: true });

  const resp = await fetch(url, { headers, signal: controller.signal });
  if (!resp.ok || !resp.body) {
    options.signal?.removeEventListener("abort", onAbort);
    throw new Error(`SSE request failed: ${resp.status} ${resp.statusText}`);
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  const pending: T[] = [];
  const parser = createParser((event: ParseEvent) => {
    if (event.type !== "event") return;
    if (event.data === "[DONE]") return;
    try {
      pending.push(parse(event.data, event.event));
    } catch {
      // skip a malformed frame
    }
  });

  try {
    while (true) {
      while (pending.length > 0) {
        yield pending.shift()!;
      }
      const chunk = await reader.read();
      if (chunk.done) return;
      parser.feed(decoder.decode(chunk.value, { stream: true }));
      if (pending.length > maxPending) {
        throw new SseOverflowError(maxPending);
      }
    }
  } finally {
    options.signal?.removeEventListener("abort", onAbort);
    controller.abort();
    await reader.cancel().catch(() => {});
  }
}
