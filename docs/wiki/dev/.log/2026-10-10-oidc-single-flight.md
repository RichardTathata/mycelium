## [2026-10-10] ingest | OIDC key refresh: single-flight, bounded, timed out

- **The defect:** `OidcVerifier::cached_keys` awaited `fetch_keys()` *before* taking the cache's write guard, so
  refresh was never single-flight (16 concurrent verifies with an unknown `kid` on a cold cache → 32 JWKS
  requests), while lock-order row 17 claimed the opposite since WS4. Because the gateway tries OIDC first for every
  bearer (`http.rs` `resolve_bearer`), an unauthenticated client choosing random `kid`s forced one IdP fetch per
  request, through an `egress_client` with no timeout.
- **The fix** (`src/agent/oidc.rs` `keys_for`): one lock, its write guard held across the fetch with the state
  re-read under it; a forced refresh (unknown `kid`) and a retry after a failure at most once per
  `JWKS_REFRESH_COOLDOWN` (30 s); `JWKS_FETCH_TIMEOUT` (10 s) on the verifier's client; previous good keys kept on
  failure; TTL path unchanged. Clock reads through `sim_seam::mono_instant` / `mono_elapsed`; `oidc.rs` left
  `scripts/sim-seams-baseline.txt`.
- **Row 17 is true again** — [lock-order](../concurrency/lock-order.md); the Async-contexts paragraph names rows
  17 and 37 as the two `tokio::sync` exceptions, each with its reason.
- **Other `egress_client` users, timeouts:** the federation client, the bulk peer fetch, capability probes
  (per request) and the skillrunner's LLM client (120 s) already had one; the `LlmBackend` client and the MCP
  bridge's client have none, deliberately left — a generation or a tool call may legitimately run long, and
  bounding them is a per-call decision, not this fix's.
- Pinned by four tests in `oidc.rs`, each seen failing first; ops: `docs/operations/sso.md`.
- Pages touched: `dev/concurrency/lock-order.md` (row 17 + Async contexts), `dev/security.md` (WS4 line);
  outside the wiki `docs/operations/sso.md`, `docs/design/replay-nondeterminism-inventory.md` (OIDC row).
