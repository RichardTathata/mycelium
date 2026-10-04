## [2026-10-04] ingest | realignment repairs R3 — a redirect is a destination

**What:** `mycelium-core/src/config.rs` (`EgressPolicy::redirect_verdict`, `MAX_REDIRECT_HOPS`), a new
`src/agent/egress_client.rs` (re-exported as `mycelium::egress_client`), every outbound client in the
`mycelium` crate moved onto it (MCP bridge, `OpenAiBackend`, OIDC, capability probes, skillrunner,
federation client, bulk peer fetch), the companions' own clients against the same verdict
(`HttpLibrarySource`, `OllamaProbe`, the stem's `[[serve]]` backend, `GitMirror`'s git flags), two
test helpers in `src/test_util.rs` (`spawn_redirector`, `spawn_counting_listener`), the plan's D6
amended and an R3b row, the changelog, and this log. The threat model, guarantee catalogue and
crown-jewel table are R5's.

**Durable knowledge:**

- **Every outbound client followed ten redirects, unchecked.** The egress gate checked the first URL
  and nothing else, in every client the workspace builds; `reqwest::Client::new()` and
  `Client::builder()` both default to `Policy::limited(10)`. Construct an outbound client only
  through `mycelium::egress_client` (or, in a companion without `gateway`, against
  `EgressPolicy::redirect_verdict`); a bare `reqwest::Client::new()` is the regression, and so is
  `Client::default()` or an `unwrap_or_default()` on a builder — all three follow ten.
- **"Refused by default" was the wrong rule as adopted.** An empty allow-list permits every host,
  so refusing every redirect would have broken deployments that never configured egress. The rule
  is per-hop re-checking when the policy is known, and none when it is not or when the client
  carries a credential header reqwest does not strip cross-host (it strips `Authorization`,
  `Cookie`, `Proxy-Authorization` and `WWW-Authenticate`; it does not strip
  `x-mycelium-federation-call` or any static header a source attaches).
- **The witness is the destination's own accept counter**, planted first through an allowed
  redirect so a zero means the gate refused rather than the listener being unreachable. The
  redirector is reached as `localhost` and names `127.0.0.1`: two hosts an allow-list can tell
  apart that both reach the same loopback listener.
- **Not done here:** the object-store fetcher's endpoint (R3b — `object_store` builds its own
  client) and the host parser's disagreement with reqwest (R4).
