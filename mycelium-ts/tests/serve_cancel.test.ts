/**
 * Cancellation on the SDK serve stream (identity-and-lifecycle I4, PR 3b; the maintainer's decision (C) of
 * 2026-11-16). No node needed: `fetch` is stubbed with the events a node sends, and records what was sent.
 *
 * - **The declaration is the subscriber's own.** `rpcServe(kind, { stopsOnCancel: true })` opens the stream with
 *   `stops_on_cancel=true`; every stream says it reads `cancel` events (`cancel_events=true`).
 * - **A cancel reaches the request it names.** The node's `cancel` event aborts that request's `signal` — and, on a
 *   declaring stream, while the caller still holds the request. A `cancel` event is never yielded as a request.
 * - **The confirmation** (the maintainer's decisions of 2026-11-17). `rpcConfirmStopped` posts `{nonce_hex, sender,
 *   stopped: true, stream}` — the stream the request came on — and a `409` is `false`. It throws
 *   `CancelNotReceivedError`, sending nothing, unless that stream received a `cancel` event for the request.
 */
import { CancelNotReceivedError, MyceliumAgent } from "../src/index";
import type { RpcRequest } from "../src/index";

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

const REQ = { nonce_hex: "00000000000000aa", sender: "127.0.0.1:7001", payload_b64: "d29yaw==", stream: "7" };
const OTHER = { nonce_hex: "00000000000000bb", sender: "127.0.0.1:7001", payload_b64: "d29yaw==", stream: "7" };
const CANCEL = { cancel: true, nonce_hex: REQ.nonce_hex, sender: REQ.sender, reason: "cancelled", stream: "7" };

let seen: Array<{ url: string; method: string; body: Record<string, unknown> }> = [];
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function stub(respondStatus = 200): void {
  seen = [];
  globalThis.fetch = (async (input: unknown, init?: RequestInit) => {
    const url = String(input instanceof URL ? input.toString() : input);
    let body: Record<string, unknown> = {};
    try { body = JSON.parse(String(init?.body ?? "{}")); } catch { /* not JSON */ }
    seen.push({ url, method: init?.method ?? "GET", body });
    if (url.includes("/rpc/serve/")) {
      const enc = new TextEncoder();
      const frame = (event: string, data: unknown) => enc.encode(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);
      const stream = new ReadableStream<Uint8Array>({
        async start(c) {
          c.enqueue(frame("depot.custom", REQ));
          await sleep(200);
          c.enqueue(frame("cancel", CANCEL));
          c.enqueue(frame("depot.custom", OTHER));
          await sleep(300);
          c.close();
        },
      });
      return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    const reply = respondStatus === 200 ? { ok: true, stopped: true } : { ok: false, error: "not_cancelled" };
    return new Response(JSON.stringify(reply), { status: respondStatus, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

const serveUrl = () => new URL(seen.find((s) => s.method === "GET")!.url);

test("a declaring stream says so, and an undeclared one does not", async () => {
  stub();
  const agent = new MyceliumAgent("127.0.0.1", 1);
  for await (const _ of agent.rpcServe("depot.custom", { stopsOnCancel: true })) break;
  expect([serveUrl().searchParams.get("cancel_events"), serveUrl().searchParams.get("stops_on_cancel")]).toEqual(["true", "true"]);
  stub();
  for await (const _ of agent.rpcServe("depot.custom")) break;
  expect([serveUrl().searchParams.get("cancel_events"), serveUrl().searchParams.get("stops_on_cancel")]).toEqual(["true", null]);
});

test("a cancel reaches the request a declaring subscriber still holds", async () => {
  stub();
  const stream = new MyceliumAgent("127.0.0.1", 1).rpcServe("depot.custom", { stopsOnCancel: true });
  const first = (await stream.next()).value!;
  // Not asking for the next request: the stream is read beside us, so the cancel still arrives.
  let timer: ReturnType<typeof setTimeout> | undefined;
  const aborted = await new Promise<boolean>((resolve) => {
    if (first.signal!.aborted) return resolve(true);
    first.signal!.addEventListener("abort", () => resolve(true), { once: true });
    timer = setTimeout(() => resolve(false), 5000);
  });
  clearTimeout(timer);
  const other = (await stream.next()).value!;
  await stream.return(undefined);
  expect(aborted).toBe(true);
  expect(other.signal!.aborted).toBe(false);
  expect([first.nonceHex, other.nonceHex]).toEqual([REQ.nonce_hex, OTHER.nonce_hex]);
});

test("an undeclared subscriber sees the cancel when it next asks", async () => {
  stub();
  const stream = new MyceliumAgent("127.0.0.1", 1).rpcServe("depot.custom");
  const first = (await stream.next()).value!;
  await sleep(500);
  const before = first.signal!.aborted;
  const other = (await stream.next()).value!;
  await stream.return(undefined);
  expect(other.nonceHex).toBe(OTHER.nonce_hex);
  expect([before, first.signal!.aborted]).toEqual([false, true]);
});

/** The first request once its cancel arrived, and the second (never cancelled). */
async function cancelledAndNot(agent: MyceliumAgent): Promise<[RpcRequest, RpcRequest]> {
  const stream = agent.rpcServe("depot.custom", { stopsOnCancel: true });
  const first = (await stream.next()).value!;
  await new Promise<void>((resolve) => {
    if (first.signal!.aborted) return resolve();
    first.signal!.addEventListener("abort", () => resolve(), { once: true });
  });
  const other = (await stream.next()).value!;
  await stream.return(undefined);
  return [first, other];
}

const posts = () => seen.filter((s) => s.method === "POST");

test("the confirmation names its stream, and a refusal is false", async () => {
  stub();
  const agent = new MyceliumAgent("127.0.0.1", 1);
  const [req] = await cancelledAndNot(agent);
  expect(await agent.rpcConfirmStopped(req)).toBe(true);
  const post = posts().pop()!;
  expect(new URL(post.url).pathname).toBe("/gateway/rpc/respond");
  expect(post.body).toEqual({ nonce_hex: REQ.nonce_hex, sender: REQ.sender, stopped: true, stream: "7" });
  stub(409);
  expect(await agent.rpcConfirmStopped(req)).toBe(false);
});

test("the SDK never confirms a stop it was not asked for", async () => {
  stub();
  const agent = new MyceliumAgent("127.0.0.1", 1);
  const [, other] = await cancelledAndNot(agent);
  // Never cancelled, and one built by hand with an aborted signal: refused, nothing sent.
  const abort = new AbortController();
  abort.abort();
  const byHand: RpcRequest = { kind: "depot.custom", nonceHex: REQ.nonce_hex, sender: REQ.sender, payload: Buffer.alloc(0), signal: abort.signal, stream: "7" };
  for (const req of [other, byHand]) {
    await expect(agent.rpcConfirmStopped(req)).rejects.toBeInstanceOf(CancelNotReceivedError);
  }
  expect(posts()).toEqual([]);
});

test("an answer names its stream", async () => {
  stub();
  const agent = new MyceliumAgent("127.0.0.1", 1);
  const [, other] = await cancelledAndNot(agent);
  await agent.rpcRespond(other, Buffer.from("done"));
  expect(posts().pop()!.body.stream).toBe("7");
});

test("a declaring stream reads ahead only as far as its bound", async () => {
  // #806's review, M2. A pull-driven body: each event is produced only when the SDK reads.
  let pulled = 0;
  const enc = new TextEncoder();
  globalThis.fetch = (async () => new Response(new ReadableStream<Uint8Array>({
    pull(c) {
      pulled += 1;
      if (pulled > 50) return c.close();
      c.enqueue(enc.encode(`event: depot.custom\ndata: ${JSON.stringify({ ...REQ, nonce_hex: pulled.toString(16).padStart(16, "0") })}\n\n`));
    },
  }, { highWaterMark: 0 }), { status: 200, headers: { "content-type": "text/event-stream" } })) as typeof fetch;
  const stream = new MyceliumAgent("127.0.0.1", 1).rpcServe("depot.custom", { stopsOnCancel: true, readAhead: 2 });
  await stream.next();
  await sleep(300); // a caller busy with its first request
  const read = pulled;
  await stream.return(undefined);
  // The one taken, two queued, one parsed and waiting for room, and the reader's own buffering — not all fifty.
  expect(read).toBeLessThan(10);
});
