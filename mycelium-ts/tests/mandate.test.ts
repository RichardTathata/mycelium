/**
 * Presenting a mandate through the SDK (Boundary H, A1 at the gateway).
 *
 * The gateway establishes a presented mandate only if the holder's possession proof covers exactly
 * the bytes the gateway computes. These golden vectors are shared with the gateway's own tests
 * (`src/agent/gateway_authority.rs`) and the Python SDK. No live node needed.
 */
import { argumentsDigest, mandateRequestBytes, taskParams } from "../src/a2a";

const hex = (b: Uint8Array) => Buffer.from(b).toString("hex");

test("the arguments digest matches the gateway's canonical form", () => {
  expect(hex(argumentsDigest({ text: "dispatch" }))).toBe(
    "719121f66b67e12629032511ad5cff8f9b541591eed0fffbe645b9f5e14a7a23",
  );
});

test("the mandate request bytes match the golden vector", () => {
  const got = mandateRequestBytes("skill.invoke", "skill:depot/dispatch@10.0.0.1:57000", new Uint8Array(32).fill(0xab));
  expect(hex(got)).toBe(
    "6d7963656c69756d2e676174657761792f6d616e646174652d726571756573742f31" +
      "0c000000" + "736b696c6c2e696e766f6b65" +
      "14000000" + "736b696c6c3a6465706f742f6469737061746368" +
      "ab".repeat(32),
  );
});

test("a presented mandate travels in _meta, and is absent otherwise", () => {
  const mandate = { grant: { mandate: {}, signature: [] }, possession: "c2ln" };
  expect(taskParams("t", "depot/dispatch", "go", { mandate })._meta).toEqual({ mandate });
  expect(taskParams("t", "depot/dispatch", "go")._meta).toBeUndefined();
});
