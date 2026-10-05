## [2026-10-05] ingest | mycelium-ts 0.2.0 — the TypeScript SDK matches the gateway

**What:** `mycelium-ts/src/json.ts` (new: lossless parse and stringify), `src/sse.ts` (on-demand
reads, a lifetime, `maxPending`), `src/agent.ts` (the shapes, the log names, whole-second timeouts),
`tests/contract.test.ts` (new, node-free), `tests/gateway.test.ts` (three stale expectations
corrected), the `sdk-ts` CI job (the live suite against a `mycelium` node), the SDK README, guide 10,
the changelog, CLAUDE.md's CI line, the realignment plan's S1–S4 rows, and this log.

**Durable knowledge:**

- **An SDK's live suite that skips itself without a node is a suite that never runs.** `gateway.test.ts`
  had been green in CI since 2026-09-05 by skipping; run by hand it failed 9 of 19 against the gateway
  it was written for. The fix is not only to run it — `tests/contract.test.ts` pins every shape with a
  mocked `fetch` answering the Rust handler's exact JSON, so the contract is checked on every PR even
  where no node can be started.
- **`as` casts made every shape mismatch type-check.** `tsc` cannot see a response that does not match
  the type asserted on it; only a test that answers the real JSON can.
- **A JSON number above 2⁵³ is a precision bug in any JavaScript client, not only this one.** The fix
  that keeps the wire is a lossless parse (quote unsafe integer literals before `JSON.parse`); the
  Python SDK never had the problem because `json.loads` keeps big ints exact.
- **Two ways a gateway treats a fractional timeout:** a typed `Option<u64>` field refuses it with 422;
  a field read with `as_u64().unwrap_or(default)` *silently replaces it with the default*. The second
  is worse and harder to notice; both are now avoided by sending whole seconds.
- **Stale live expectations were the gateway being right.** An RPC or reliable emit to an address with
  no caller-context marker is refused before dispatch (412), and an election over an empty group is
  refused by name (409). A test that expects a timeout from an unknown target needs a target the
  gateway will dispatch to — the node itself, on a kind nobody serves.
