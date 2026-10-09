/**
 * The scheme is the caller's to say — no node needed: `fetch` is replaced by a recorder and each
 * client constructed with `{ scheme: "https" }` must send its requests to an `https://` URL.
 *
 * Every client built its base URL as `http://${host}:${port}`: a gateway serving HTTPS (`gateway_tls`)
 * was unreachable from this SDK, and the bearer travelled in cleartext off loopback. Seen failing on
 * 0.2.2 (`scheme` was not an option).
 */
import { MyceliumAgent } from "../src/agent";
import { Wiki } from "../src/wiki";
import { TupleSpace } from "../src/tuple";
import { Blackboard } from "../src/blackboard";
import { PromptSkillClient } from "../src/prompt_skill";
import { baseUrl } from "../src/auth";

const urls: string[] = [];

const realFetch = globalThis.fetch;
beforeEach(() => {
  urls.length = 0;
  globalThis.fetch = (async (input: Parameters<typeof fetch>[0]) => {
    const url = typeof input === "string" ? input : input.toString();
    urls.push(url);
    const body = url.includes("/gateway/kv/keys") ? { keys: [] }
      : url.includes("/gateway/wiki/read") ? { page: null }
      : url.includes("/gateway/tuple/depth") ? { stages: [] }
      : url.includes("/gateway/bb/depth") ? { depth: 0 }
      : url.includes("/gateway/prompts") ? { prompts: [] }
      : { found: false, ok: true };
    return new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
  }) as typeof fetch;
});
afterAll(() => { globalThis.fetch = realFetch; });

describe("baseUrl helper", () => {
  test("http by default, https on request, nothing else", () => {
    expect(baseUrl("10.0.0.5", 8300)).toBe("http://10.0.0.5:8300");
    expect(baseUrl("10.0.0.5", 8300, "https")).toBe("https://10.0.0.5:8300");
    expect(() => baseUrl("h", 1, "ftp" as unknown as "http")).toThrow(/scheme/);
  });
});

describe("every client sends to the scheme it was given", () => {
  test("MyceliumAgent GET / POST / DELETE and the derived handles", async () => {
    const a = new MyceliumAgent("10.0.0.5", 8300, 1000, { scheme: "https" });
    await a.get("k"); await a.set("k", Buffer.from("v")); await a.delete("k"); await a.keys("");
    expect(urls).toHaveLength(4);
    for (const u of urls) expect(u.startsWith("https://10.0.0.5:8300/")).toBe(true);
  });

  test("Wiki / TupleSpace / Blackboard / PromptSkillClient", async () => {
    await new Wiki("10.0.0.5", 8300, "g", { scheme: "https" }).read("p");
    await new TupleSpace("10.0.0.5", 8300, "ns", { scheme: "https" }).depth();
    await new Blackboard("10.0.0.5", 8300, "b", { scheme: "https" }).depth();
    await new PromptSkillClient("10.0.0.5", 8300, 1000, { scheme: "https" }).list();
    expect(urls).toHaveLength(4);
    for (const u of urls) expect(u.startsWith("https://10.0.0.5:8300/")).toBe(true);
  });

  test("no scheme → http, unchanged", async () => {
    await new MyceliumAgent("127.0.0.1", 7946, 1000).get("k");
    await new Wiki("127.0.0.1", 7946).read("p");
    expect(urls[0].startsWith("http://127.0.0.1:7946/")).toBe(true);
    expect(urls[1].startsWith("http://127.0.0.1:7946/")).toBe(true);
  });
});

describe("the adversarial review's findings on #583", () => {
  test("scheme is case-insensitive for a JS caller, and a non-string is refused by name", () => {
    expect(baseUrl("h", 1, "HTTPS")).toBe("https://h:1");
    expect(baseUrl("h", 1, "Http")).toBe("http://h:1");
    expect(baseUrl("h", 1, undefined)).toBe("http://h:1");
    for (const bad of [null, 5, {}, ["https"]]) {
      expect(() => baseUrl("h", 1, bad as unknown as string)).toThrow(/scheme/);
    }
    expect(() => new MyceliumAgent("h", 1, 1000, { scheme: null as unknown as "http" })).toThrow(/scheme/);
  });

  test("an IPv6 host is bracketed once", async () => {
    expect(baseUrl("::1", 8300, "https")).toBe("https://[::1]:8300");
    expect(baseUrl("[::1]", 8300)).toBe("http://[::1]:8300");
    await new MyceliumAgent("::1", 8300, 1000, { scheme: "https" }).get("k");
    await new Wiki("::1", 8300, "g").read("p");
    expect(urls[0].startsWith("https://[::1]:8300/")).toBe(true);
    expect(urls[1].startsWith("http://[::1]:8300/")).toBe(true);
  });
});
