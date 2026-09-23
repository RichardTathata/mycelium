/**
 * The federation verbs (item 2 row 11). `fetch` is stubbed with the JSON shapes
 * `/gateway/federation/*` emits, so no node is needed.
 *
 * The happy path is the least interesting part. What is tested is that a refusal keeps its two
 * load-bearing fields on the way through the SDK: a `delivery: "unknown"` that surfaces as a
 * generic error is an invitation to retry an effect that may already have run.
 */
import { MyceliumAgent } from "../src/agent";
import { DeliveryUnknownError, FederationError } from "../src/federation";

const realFetch = globalThis.fetch;
afterAll(() => {
  globalThis.fetch = realFetch;
});

let seen: { url: string; method: string; body: unknown }[] = [];

function stub(body: Record<string, unknown>, status = 200, contentType = "application/json"): void {
  seen = [];
  globalThis.fetch = (async (url: string | URL, init?: RequestInit) => {
    seen.push({
      url: url.toString(),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(init.body as string) : null,
    });
    const payload = contentType === "application/json" ? JSON.stringify(body) : "<html>502</html>";
    return new Response(payload, { status, headers: { "content-type": contentType } });
  }) as unknown as typeof fetch;
}

const fed = () => new MyceliumAgent("127.0.0.1", 1, 1000).federation();

test("the read verbs hit the documented routes", async () => {
  stub({ partners: [{ domain: "b.example", link: "ready", last_catalogue: ["x"] }] });
  const rows = await fed().partners();
  expect(rows[0].link).toBe("ready");
  expect(seen[0].url).toBe("http://127.0.0.1:1/gateway/federation/partners");

  stub({ configured: true, domain: "a.example", exports: ["x"], policy_revision: 3 });
  expect((await fed().domain()).policy_revision).toBe(3);
  expect(seen[0].url).toBe("http://127.0.0.1:1/gateway/federation/domain");

  stub({ domain: "b.example", link: "ready", observed: true, exports: ["x"] });
  expect((await fed().catalog("b.example")).observed).toBe(true);
  expect(seen[0].url).toBe("http://127.0.0.1:1/gateway/federation/catalog/b.example");
});

test("a call sends its repeatability and returns the reply", async () => {
  stub({ reply: "ok", sent: true, delivery: "completed" });
  expect(await fed().call("b.example", "invoice.status", "INV-42")).toBe("ok");
  expect(seen[0].method).toBe("POST");
  // The default is the safe one: an unstated effect is never retried elsewhere.
  expect(seen[0].body).toEqual({
    domain: "b.example",
    export: "invoice.status",
    text: "INV-42",
    repeatable: false,
  });

  stub({ reply: "ok" });
  await fed().call("b.example", "invoice.status", "INV-42", { repeatable: true });
  expect((seen[0].body as { repeatable: boolean }).repeatable).toBe(true);
});

test("a local refusal says nothing was sent", async () => {
  stub({ error: "link", detail: "link: Down", sent: false, delivery: "none" }, 409);
  const err = await fed()
    .call("b.example", "invoice.status")
    .catch((e: unknown) => e);
  expect(err).toBeInstanceOf(FederationError);
  expect(err).not.toBeInstanceOf(DeliveryUnknownError);
  const e = err as FederationError;
  expect(e.kind).toBe("link");
  expect(e.nothingWasSent).toBe(true);
  expect(e.delivery).toBe("none");
});

test("delivery unknown is its own type", async () => {
  stub(
    {
      error: "delivery-unknown",
      detail: "outcome: DeliveryUnknown",
      sent: true,
      delivery: "unknown",
      attempted_via: ["gw-1", "gw-2"],
    },
    504,
  );
  const err = (await fed()
    .call("b.example", "invoice.status")
    .catch((e: unknown) => e)) as DeliveryUnknownError;
  expect(err).toBeInstanceOf(DeliveryUnknownError);
  expect(err.attemptedVia).toEqual(["gw-1", "gw-2"]);
  expect(err.nothingWasSent).toBe(false);
});

test("a partner refusal is not delivery unknown", async () => {
  // The partner answered and said no: it ran nothing, and that *is* knowable.
  stub({ error: "refused", detail: "refused (403)", sent: true, delivery: "refused", partner_status: 403 }, 502);
  const err = (await fed()
    .call("b.example", "invoice.status")
    .catch((e: unknown) => e)) as FederationError;
  expect(err).toBeInstanceOf(FederationError);
  expect(err).not.toBeInstanceOf(DeliveryUnknownError);
  expect(err.body["partner_status"]).toBe(403);
});

test("an unreadable refusal fails closed rather than throwing a parse error", async () => {
  // A proxy in the path can answer HTML.
  stub({}, 502, "text/html");
  const err = (await fed()
    .call("b.example", "invoice.status")
    .catch((e: unknown) => e)) as FederationError;
  expect(err).toBeInstanceOf(DeliveryUnknownError);
  expect(err.delivery).toBe("unknown");
});
