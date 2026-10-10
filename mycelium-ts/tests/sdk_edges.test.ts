/**
 * The SDK's edges with the gateway (post-360 hardening, row G). No node needed — `fetch` is stubbed and
 * records what would have been sent.
 *
 * - **Whole-second timeouts** on the tuple `take` routes and `wiki.ingest`: the routes read `u64`, so a
 *   fraction was refused 422. A fraction rounds up; `take`'s `0` stays the poll.
 * - **Path segments escaped**: a caller-supplied value becomes exactly one segment; `.`/`..`, which the
 *   URL parser resolves even when encoded, is refused before any request.
 * - **`SupersededError`** for a consensus write that lost (409 `superseded`), as the Python SDK raises.
 * - **One `scatterGather` default**: `minOk` 1, the gateway's own and the Python SDK's.
 */
import * as sdk from "../src/index";
import { MyceliumAgent, PromptSkillClient, TupleSpace, Wiki } from "../src/index";

// A stand-in when the SDK predates the error, so the fail-first run reaches the behaviour (a plain
// `Error`) instead of stopping at the import.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
const SupersededError: any = (sdk as any).SupersededError ?? class SupersededErrorMissing extends Error {};

const realFetch = globalThis.fetch;
afterAll(() => { globalThis.fetch = realFetch; });

let seen: Array<{ url: string; method: string; body: Record<string, unknown> }> = [];
function stub(reply: Record<string, unknown> = {}, status = 200): void {
  seen = [];
  globalThis.fetch = (async (input: unknown, init?: RequestInit) => {
    const url = String(input instanceof URL ? input.toString() : input);
    let body: Record<string, unknown> = {};
    try { body = JSON.parse(String(init?.body ?? "{}")); } catch { /* not JSON */ }
    seen.push({ url, method: init?.method ?? "GET", body });
    if (/\/(signal\/sse|rpc\/serve|mailbox)\//.test(url)) {
      return new Response("", { status: 200, headers: { "content-type": "text/event-stream" } });
    }
    return new Response(JSON.stringify(reply), { status, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
}

/** What `p` threw (or `undefined`), typed loosely for the assertions. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
async function caught(p: () => Promise<unknown>): Promise<any> {
  try { await p(); return undefined; } catch (e) { return e; }
}

const pathOf = (url: string) => {
  // The path as sent, before any decoding: everything after the authority, up to the query.
  const rest = url.replace(/^[a-z]+:\/\/[^/]+/, "");
  return rest.split("?")[0];
};

// ── Whole-second timeouts ─────────────────────────────────────────────────────

const OK_TAKE = { id: 1, payload_b64: "" };

test.each([[0.3, 1], [2.2, 3], [5, 5], [0, 0]])("tuple take(%p) parks for %p whole seconds", async (given, sent) => {
  stub(OK_TAKE);
  const ts = new TupleSpace("127.0.0.1", 1);
  await ts.take("stage-a", given);
  await ts.takeByKey("stage-a", "k", given);
  expect(seen.map((s) => s.body.timeout_secs)).toEqual([sent, sent]);
});

test.each([[1.5, 2], [0.2, 1], [60, 60]])("wiki ingest(%p) sends %p whole seconds", async (given, sent) => {
  stub({ summary: { applied: 0, refused: 0, findings: [] } });
  await new Wiki("127.0.0.1", 1).ingest("batch/1", given);
  expect(seen[0].body.timeout_secs).toBe(sent);
});

// ── Path segments ─────────────────────────────────────────────────────────────

const HOSTILE = ["a/b", "x?y=1#z", "sp ace%", "%2e%2e"];
const enc = encodeURIComponent;

async function drain<T>(gen: AsyncGenerator<T>): Promise<void> {
  for await (const _ of gen) { /* nothing */ }
}

type Call = [(a: MyceliumAgent, p: PromptSkillClient, s: string) => Promise<unknown>, (s: string) => string];
const SEGMENT_CALLS: Record<string, Call> = {
  onSignal:      [(a, _p, s) => drain(a.onSignal(s)),               (s) => `/gateway/signal/sse/${enc(s)}`],
  rpcServe:      [(a, _p, s) => drain(a.rpcServe(s)),               (s) => `/gateway/rpc/serve/${enc(s)}`],
  mailbox:       [(a, _p, s) => drain(a.mailbox(s)),                (s) => `/gateway/mailbox/${enc(s)}`],
  shardFor:      [(a, _p, s) => a.shardFor(s, s, "k"),              (s) => `/gateway/shard/${enc(s)}/${enc(s)}`],
  catalog:       [(a, _p, s) => a.federation().catalog(s),          (s) => `/gateway/federation/catalog/${enc(s)}`],
  promptGet:     [(_a, p, s) => p.get(s, s),                        (s) => `/gateway/prompts/${enc(s)}/${enc(s)}`],
  promptUpdate:  [(_a, p, s) => p.updatePrompt(s, s, { system: "s", userTemplate: "u" } as never),
                  (s) => `/gateway/prompts/${enc(s)}/${enc(s)}`],
  promptDelete:  [(_a, p, s) => p.deletePrompt(s, s),               (s) => `/gateway/prompts/${enc(s)}/${enc(s)}`],
};

describe.each(Object.keys(SEGMENT_CALLS))("%s", (verb) => {
  const [call, shape] = SEGMENT_CALLS[verb];

  test.each(HOSTILE)(`${verb} keeps %p one path segment`, async (seg) => {
    stub({ owner: "127.0.0.1:1", system: "s", user_template: "u" });
    await call(new MyceliumAgent("127.0.0.1", 1, 1000), new PromptSkillClient("127.0.0.1", 1), seg);
    expect(seen.length).toBeGreaterThan(0);
    expect(pathOf(seen[seen.length - 1].url)).toBe(shape(seg));
  });

  test.each([[".", "."], ["..", ".."], ["empty", ""], ["lone-surrogate", "\ud800"]])(
    `${verb} refuses the segment %s before any request`, async (_label, seg) => {
    stub({});
    const err = await caught(() =>
      call(new MyceliumAgent("127.0.0.1", 1, 1000), new PromptSkillClient("127.0.0.1", 1), seg));
    expect(String(err?.message)).toMatch(/cannot travel as a URL path segment/);
    expect(seen).toEqual([]);
  });
});

test("a server-issued handle or guard id is escaped too", async () => {
  stub({ ok: true, handle_id: "h/1?x", guard_id: "g/1#y", token: "5" });
  const a = new MyceliumAgent("127.0.0.1", 1, 1000);
  const h = await a.advertiseCapability("ns", "name", { leaseSecs: 30 });
  await h.heartbeat();
  await h.drop();
  const g = await a.distributedLock("jobs");
  await g.release();
  expect(seen.slice(1).map((s) => pathOf(s.url))).toEqual([
    "/gateway/capability/h%2F1%3Fx/heartbeat",
    "/gateway/capability/h%2F1%3Fx",
    "/gateway/overlay/lock/acquire",
    "/gateway/overlay/lock/g%2F1%23y",
  ]);
});

// ── SupersededError ───────────────────────────────────────────────────────────

const CONSENSUS: Record<string, (a: MyceliumAgent) => Promise<unknown>> = {
  consistentSet:     (a) => a.consistentSet("config/endpoint", Buffer.from("x")),
  crossGroupPropose: (a) => a.crossGroupPropose("slot", Buffer.from("x"), [{ group: "g" }]),
  distributedLock:   (a) => a.distributedLock("jobs/nightly"),
  electLeader:       (a) => a.electLeader("workers"),
};

test.each(Object.keys(CONSENSUS))("%s that lost throws SupersededError", async (verb) => {
  stub({ ok: false, error: "superseded" }, 409);
  const err = await caught(() => CONSENSUS[verb](new MyceliumAgent("127.0.0.1", 1, 1000)));
  expect(err).toBeInstanceOf(SupersededError);
  expect(err.status).toBe(409);
  expect(err.name).toBe("SupersededError");
});

test.each(Object.keys(CONSENSUS))("%s: other refusals are not SupersededError (the plant)", async (verb) => {
  stub({ ok: false, error: "topology_unsatisfied" }, 409);
  let err = await caught(() => CONSENSUS[verb](new MyceliumAgent("127.0.0.1", 1, 1000)));
  expect(err).toBeInstanceOf(Error);
  expect(err).not.toBeInstanceOf(SupersededError);
  stub({ ok: false, error: "timeout after 3 ballot(s)" }, 504);
  err = await caught(() => CONSENSUS[verb](new MyceliumAgent("127.0.0.1", 1, 1000)));
  expect(err).not.toBeInstanceOf(SupersededError);
  expect(err.name).toBe("TimeoutError");
});

// ── scatterGather's default ───────────────────────────────────────────────────

test("scatterGather waits for one reply by default, as the gateway and the Python SDK do", async () => {
  stub({ ok: true, replies: [] });
  await new MyceliumAgent("127.0.0.1", 1, 1000).scatterGather(["127.0.0.1:1", "127.0.0.1:2"], "echo");
  expect(seen[0].body.min_ok).toBe(1);
  expect(seen[0].body.timeout_secs).toBe(10);
});

// ── Adversarial review of #595 ────────────────────────────────────────────────

const BAD_SECONDS = [-1, -0.5, Infinity, -Infinity, NaN, 2 ** 64];

const ALL_TIMEOUTS: Record<string, (t: number) => Promise<unknown>> = {
  rpcCall:       (t) => new MyceliumAgent("127.0.0.1", 1, 1000).rpcCall("127.0.0.1:1", "echo", Buffer.alloc(0), { timeoutSecs: t }),
  scatterGather: (t) => new MyceliumAgent("127.0.0.1", 1, 1000).scatterGather(["127.0.0.1:1"], "echo", Buffer.alloc(0), { timeoutSecs: t }),
  emitReliable:  (t) => new MyceliumAgent("127.0.0.1", 1, 1000).emitReliable("127.0.0.1:1", "echo", Buffer.alloc(0), { timeoutSecs: t }),
  ingest:        (t) => new Wiki("127.0.0.1", 1).ingest("b", t),
  take:          (t) => new TupleSpace("127.0.0.1", 1).take("s", t),
  takeByKey:     (t) => new TupleSpace("127.0.0.1", 1).takeByKey("s", "k", t),
};

describe.each(Object.keys(ALL_TIMEOUTS))("%s", (verb) => {
  test.each(BAD_SECONDS)(`${verb} refuses timeoutSecs %p by name, before any request`, async (bad) => {
    stub({ ok: true, ack: "acknowledged", replies: [], id: 1, payload_b64: "", summary: {} });
    const err = await caught(() => ALL_TIMEOUTS[verb](bad));
    expect(String(err?.message)).toMatch(/timeoutSecs/);
    expect(seen).toEqual([]);
  });
});

type Lease = [(a: MyceliumAgent, t: number) => Promise<unknown>, string, string];
const LEASES: Record<string, Lease> = {
  "advertiseCapability.leaseSecs": [(a, t) => a.advertiseCapability("ns", "n", { leaseSecs: t }), "lease_secs", "leaseSecs"],
  "declareUnits.leaseSecs":        [(a, t) => a.declareUnits("", { leaseSecs: t }), "lease_secs", "leaseSecs"],
  "distributedLock.ttlSecs":       [(a, t) => a.distributedLock("jobs", { ttlSecs: t }), "ttl_secs", "ttlSecs"],
};

describe.each(Object.keys(LEASES))("%s", (verb) => {
  const [call, field, option] = LEASES[verb];
  const reply = { ok: true, handle_id: "h", guard_id: "g", token: "1", principal: null,
                  declared: { capabilities: 0, requirements: 0, groups: 0 }, not_enforced: [] };

  test.each([[1.5, 2], [0.2, 1], [30, 30]])(`${verb} sends %p as %p whole seconds`, async (given, sent) => {
    // The gateway reads `lease_secs` with `as_u64()` — a fraction left the advert unleased — and
    // `ttl_secs` as `Option<u64>` — a fraction was refused 422.
    stub(reply);
    await call(new MyceliumAgent("127.0.0.1", 1, 1000), given);
    expect(seen[0].body[field]).toBe(sent);
  });

  test.each(BAD_SECONDS)(`${verb} refuses %p by name, before any request`, async (bad) => {
    stub(reply);
    const err = await caught(() => call(new MyceliumAgent("127.0.0.1", 1, 1000), bad));
    expect(String(err?.message)).toContain(option);
    expect(seen).toEqual([]);
  });
});
