/**
 * The SDK against the gateway's actual wire shapes, without a node (realignment repairs S1–S3; the
 * review's F06–F09). Each response here is what the Rust handler in `src/agent/http.rs` sends, and
 * each request assertion is what that handler's `Deserialize` struct accepts. A mocked `fetch`
 * captures the request and replays the response, so these run in CI's node-free `jest` step.
 *
 * Written first and seen failing against the SDK before the fixes (the commit names which).
 */

import { MyceliumAgent } from "../src/agent";
import { sseStream } from "../src/sse";

type Seen = { url: string; method: string; body: string | null };

/** Replace `fetch` with one that records the request and answers `text` (status 200 unless given). */
function mockFetch(text: string, status = 200): Seen[] {
  const seen: Seen[] = [];
  globalThis.fetch = (async (input: unknown, init?: { method?: string; body?: string }) => {
    seen.push({ url: String(input), method: init?.method ?? "GET", body: init?.body ?? null });
    return new Response(text, { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
  return seen;
}

/** An SSE response whose body yields `frames`, then stays open until cancelled. */
function mockSse(frames: string[], onCancel: () => void = () => {}): void {
  globalThis.fetch = (async () => {
    const enc = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        for (const f of frames) c.enqueue(enc.encode(f));
      },
      cancel() {
        onCancel();
      },
    });
    return new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } });
  }) as typeof fetch;
}

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

const agent = () => new MyceliumAgent("127.0.0.1", 1, 1_000, { token: "" });

// Two HLCs one logical tick apart, in the same millisecond: (ms << 16) | 1 and | 2. Above 2^53, so a
// plain `JSON.parse` maps both to the same double.
const HLC_1 = 117381021747249153n;
const HLC_2 = 117381021747249154n;

describe("64-bit values survive the wire (F06)", () => {
  it("append returns the exact HLC the gateway sent", async () => {
    mockFetch(`{"hlc":${HLC_2}}`);
    expect(await agent().append("s", Buffer.from("x"))).toBe(HLC_2);
  });

  it("two HLCs one tick apart stay distinct through scanLog", async () => {
    mockFetch(`[{"hlc":${HLC_1},"value_b64":"YQ=="},{"hlc":${HLC_2},"value_b64":"Yg=="}]`);
    const entries = await agent().scanLog("s");
    expect(entries.map((e) => e.hlc)).toEqual([HLC_1, HLC_2]);
  });

  it("a signal's nonce is exact", async () => {
    const nonce = 0xdeadbeef01020304n;
    mockSse([`event: k\ndata: {"sender":"127.0.0.1:1","payload_b64":"","nonce":${nonce}}\n\n`]);
    const gen = agent().onSignal("k");
    const first = await gen.next();
    await gen.return(undefined);
    expect(first.value?.nonce).toBe(nonce);
  });

  it("compactLog sends before_hlc as a JSON integer, exactly", async () => {
    const seen = mockFetch(`{"ok":true}`);
    await agent().compactLog("s", HLC_2 + 1n);
    expect(seen[0].body).toContain(`"before_hlc":${HLC_2 + 1n}`);
    expect(seen[0].body).not.toContain(`"before_hlc":"`);
  });
});

describe("response shapes match the gateway (F07)", () => {
  it("get returns null for an absent key ({found:false}, no value_b64)", async () => {
    mockFetch(`{"found":false}`);
    expect(await agent().get("nope")).toBeNull();
  });

  it("get returns the bytes for a present key", async () => {
    mockFetch(`{"found":true,"value_b64":"aGk="}`);
    expect((await agent().get("k"))?.toString()).toBe("hi");
  });

  it("resolveCapability returns the providers array, not the envelope", async () => {
    mockFetch(`{"providers":[{"node_id":"127.0.0.1:1","ns":"n","name":"x","attributes":{}}]}`);
    const providers = await agent().resolveCapability("n", "x");
    expect(Array.isArray(providers)).toBe(true);
    expect(providers.length).toBe(1);
  });

  it("emit returns the gateway's ok", async () => {
    mockFetch(`{"ok":true}`);
    expect(await agent().emit("k")).toBe(true);
    mockFetch(`{"ok":false}`);
    expect(await agent().emit("k")).toBe(false);
  });

  it("emitReliable maps ack, and sends whole seconds the server accepts", async () => {
    const seen = mockFetch(`{"ack":"timeout"}`);
    expect(await agent().emitReliable("127.0.0.1:1", "k", Buffer.alloc(0), { timeoutSecs: 0.3 })).toBe("timeout");
    expect(JSON.parse(seen[0].body!).timeout_secs).toBe(1);
    mockFetch(`{"ack":"acknowledged"}`);
    expect(await agent().emitReliable("127.0.0.1:1", "k")).toBe("acknowledged");
  });

  it("a signal's kind comes from the SSE event name", async () => {
    mockSse([`event: the-kind\ndata: {"sender":"127.0.0.1:1","payload_b64":"","nonce":7}\n\n`]);
    const gen = agent().onSignal("the-kind");
    const first = await gen.next();
    await gen.return(undefined);
    expect(first.value?.kind).toBe("the-kind");
  });
});

describe("timeouts the gateway reads as whole seconds", () => {
  it("rpcCall and scatterGather round a fraction up instead of losing it to the server's default", async () => {
    let seen = mockFetch(`{"ok":true,"result_b64":""}`);
    await agent().rpcCall("127.0.0.1:1", "echo", Buffer.alloc(0), { timeoutSecs: 0.2 });
    expect(JSON.parse(seen[0].body!).timeout_secs).toBe(1);
    seen = mockFetch(`{"ok":true,"replies":[]}`);
    await agent().scatterGather(["127.0.0.1:1"], "echo", Buffer.alloc(0), { timeoutSecs: 2.5 });
    expect(JSON.parse(seen[0].body!).timeout_secs).toBe(3);
  });
});

describe("ordered-log requests use the gateway's names (F08)", () => {
  it("scanLog sends from and to", async () => {
    const seen = mockFetch(`[]`);
    await agent().scanLog("s", { fromHlc: HLC_1, toHlc: HLC_2 });
    const u = new URL(seen[0].url);
    expect(u.searchParams.get("from")).toBe(String(HLC_1));
    expect(u.searchParams.get("to")).toBe(String(HLC_2));
  });

  it("subscribeLog sends since", async () => {
    const seen: string[] = [];
    const enc = new TextEncoder();
    globalThis.fetch = (async (input: unknown) => {
      seen.push(String(input));
      const body = new ReadableStream<Uint8Array>({
        start(c) {
          c.enqueue(enc.encode(`data: {"stream":"s","hlc":${HLC_2},"value_b64":""}\n\n`));
        },
      });
      return new Response(body, { status: 200 });
    }) as typeof fetch;
    const gen = agent().subscribeLog("s", { sinceHlc: HLC_2 });
    const first = await gen.next();
    await gen.return(undefined);
    expect(new URL(seen[0]).searchParams.get("since")).toBe(String(HLC_2));
    expect(first.value?.hlc).toBe(HLC_2);
  });
});

describe("an SSE stream has a lifetime (F09)", () => {
  it("ending the generator cancels the underlying stream", async () => {
    let cancelled = false;
    mockSse([`data: {"n":1}\n\n`], () => {
      cancelled = true;
    });
    const gen = sseStream<{ n: number }>("http://127.0.0.1:1/x", (d) => JSON.parse(d));
    const first = await gen.next();
    expect(first.value).toEqual({ n: 1 });
    await gen.return(undefined);
    expect(cancelled).toBe(true);
  });

  it("a consumer that falls behind is told by name rather than buffered without bound", async () => {
    const frames = Array.from({ length: 50 }, (_, i) => `data: {"n":${i}}\n\n`);
    mockSse([frames.join("")]);
    // Untyped so this file compiles against an `sseStream` without the option (the fail-first run).
    const open = sseStream as unknown as (u: string, p: (d: string) => unknown, o: { maxPending: number }) => AsyncGenerator<unknown>;
    const gen = open("http://127.0.0.1:1/x", (d) => JSON.parse(d), { maxPending: 8 });
    await expect(gen.next()).rejects.toThrow(/fell behind|maxPending/);
  });
});

describe("the lossless codec itself", () => {
  // Imported here so the fail-first run above did not depend on the module existing.
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const { parseLossless, stringifyLossless } = require("../src/json");

  it("leaves safe integers, floats and strings alone, and quotes only unsafe integers", () => {
    const text = `{"a":1,"b":-2.5e3,"c":"9007199254740993","d":9007199254740993,"e":[18446744073709551615,0]}`;
    expect(parseLossless(text)).toEqual({ a: 1, b: -2500, c: "9007199254740993", d: "9007199254740993", e: ["18446744073709551615", 0] });
  });

  it("does not touch digits inside strings, including after escapes", () => {
    const text = `{"s":"x\\\\\\"123456789012345678901","n":123456789012345678901}`;
    // The JSON string holds an escaped backslash then an escaped quote: it decodes to x, \, ".
    expect(parseLossless(text)).toEqual({ s: 'x\\"123456789012345678901', n: "123456789012345678901" });
  });

  it("writes a bigint as a bare integer", () => {
    expect(stringifyLossless({ before_hlc: 117381021747249155n, s: "t" })).toBe(`{"before_hlc":117381021747249155,"s":"t"}`);
  });
});
