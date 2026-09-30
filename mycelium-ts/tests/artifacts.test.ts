/**
 * The artifact publish verb (plan A3). `fetch` is stubbed with the JSON
 * `/gateway/artifacts/publish` emits, so no node is needed. What is tested: the signed line goes
 * out untouched under the bearer, and a refusal keeps the gateway's own name for itself.
 */
import { MyceliumAgent } from "../src/agent";
import { ArtifactError } from "../src/artifacts";

const realFetch = globalThis.fetch;
afterAll(() => {
  globalThis.fetch = realFetch;
});

let seen: { url: string; method: string; body: unknown; auth: string | undefined }[] = [];

function stub(body: Record<string, unknown>, status = 200): void {
  seen = [];
  globalThis.fetch = (async (url: string | URL, init?: RequestInit) => {
    const headers = (init?.headers ?? {}) as Record<string, string>;
    seen.push({
      url: url.toString(),
      method: init?.method ?? "GET",
      body: init?.body ? JSON.parse(init.body as string) : null,
      auth: headers.Authorization,
    });
    return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }) as unknown as typeof fetch;
}

const LINE = "01000102".repeat(20);
const arts = () => new MyceliumAgent("127.0.0.1", 1, 1000, { token: "artpub" }).artifacts();

test("publish posts the line as is under the bearer and returns the receipt", async () => {
  stub({ key: "installable/route/optimize/ab", artifact: "ab", signer: "ed25519:cd", kind: "wasm-component", provides: { ns: "route", name: "optimize" } });
  const receipt = await arts().publish(`  ${LINE}\n`);
  expect(receipt.key).toBe("installable/route/optimize/ab");
  expect(seen[0].url).toBe("http://127.0.0.1:1/gateway/artifacts/publish");
  expect(seen[0].method).toBe("POST");
  expect(seen[0].body).toEqual({ entry_hex: LINE });
  expect(seen[0].auth).toBe("Bearer artpub");
});

test("a refusal keeps the gateway's name for it", async () => {
  stub({ error: "untrusted publisher", detail: "ed25519:cd is not in this node's trusted publishers" }, 403);
  await expect(arts().publish(LINE)).rejects.toMatchObject({ kind: "untrusted publisher", status: 403 });
  stub({ error: "insufficient scope", required_scope: "artifact:publish" }, 403);
  const err = await arts().publish(LINE).catch((e: unknown) => e);
  expect(err).toBeInstanceOf(ArtifactError);
  expect((err as ArtifactError).detail).toBe("artifact:publish");
  stub({ error: "librarian-managed signer", detail: "publish to the library instead" }, 409);
  await expect(arts().publish(LINE)).rejects.toMatchObject({ kind: "librarian-managed signer", status: 409 });
});
