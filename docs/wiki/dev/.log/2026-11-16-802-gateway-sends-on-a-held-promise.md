## [2026-11-16] ingest | #802: a gateway sends only to a provider whose caller-context marker it holds

**Root cause (a bug family, not one test).** A provider's `sys/caller-context/{node}` marker and its `tools/` / `cap/`
entries are separate KV keys; gossip (fan-out and anti-entropy) does not deliver separate keys in write order, so
#794's provider-side ordering (marker before advertisement) could not order them at the gateway. `/mcp`, `/a2a`, the
LLM doors and the reason router chose a provider from the advertisement and then `gateway_rpc_call_with_mandate`
refused it at once, `-32021 provider_without_caller_context`, because `provider_enforces_context` is a bare
`kv_get` of the marker.

**Position (agreed with the maintainer, Promise Theory):** the provider promises what it enforces (its marker); the
gateway judges only what it holds and sends only to a provider whose promise it has seen. A provider whose marker
has not arrived is not yet a provider.

- `gateway_caller::gateway_sends_to` (and the public `GossipAgent::gateway_sends_to`) is the predicate; every
  choosing door applies it: `mcp_bridge::tool_provider` (shared with the grant door, so a grant names the provider
  the call goes to), `a2a::resolve_skill` (send and stream), the LLM doors' `sendable_providers`, and
  `mycelium-reason`'s `call_inner` for `Dispatch::ForClient` (passed over, not an attempt). With none left: the
  door's own no-provider answer (`data.reason` `provider_not_promised` where it has a data field; the clients do not
  retry it — the gateway promises only its answer). The listings agree: `tools/list` and the A2A card leave out what no
  sendable provider offers (the Eve fleet's readiness probe was "node 1 lists get_weather").
- `-32021` remains for a **named** target (`/gateway/rpc/call`, `/gateway/scatter`, overlay `emit_reliable`,
  `rpc_call_as`) and the withdrawn-between-choice-and-send race. No marker value means "does not enforce" (`"1"` and
  `"2"` both promise), so -32021 means "this gateway holds no promise from that target". Message reworded;
  code/reason/status unchanged. `deprecations.md` §115.
- Not a door: `cap.invoke/…` is refused on raw routes (`refuse_protected_kind`); member-side `rpc_call` (companions,
  the wasm host, SkillRunner, the router's `AsNode`) carries no *client* envelope — on a secure gateway node it carries
  a signed `node:{self}` envelope (`rpc.rs`, `frame_self`) — and never reads the marker or answers -32021.
- #804's review: every node re-asserts its own marker each `CALLER_CONTEXT_MARKER_REFRESH_MS` (60 s, the id-marker
  ticker, now in every build), since a tombstone of it now hides the node from every choosing door; each door counts
  what it passes over (`/stats` `providers_passed_over_unpromised`, by door). Owner-signed inventory rows for
  `sys/caller-context/` and `tools/` updated.
- Test lore: a fixture that waits for a tool before calling must also wait for `gateway_sends_to` (call_mint_tests'
  `pair()` now does); `call_mint_tests`' -32021 / `running_count 0` sightings were this race.

Pages: `features/gateway-caller-identity.md`, `features/provider-enforcement.md`, `operations/rbac.md` §7,
`wiki/dev/security.md`, `wiki/dev/testing/testing.md`, the SDK READMEs.
