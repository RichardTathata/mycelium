# 2026-10-27 — the federation edge serves a credential once

**What:** `FederationEdge::claim` (`src/federation/edge.rs`), called on `/a2a` after `authorize` and before `admit`
(`src/agent/a2a.rs`), keeps each partner's served credentials — `(expires_at_ms, sha256(canonical_bytes))` in a
`BTreeSet`, with a floor — until expiry plus the skew tolerance, and refuses a second presentation as
`CallRefusal::Replayed` (-32003). Full (65 536 a partner by default, `with_replay_window_per_partner`), it forgets the
entry expiring first and refuses everything expiring no later than it as `ReplayUndecidable` (-32004): the shape of the
caller-envelope window (post-360 row E, `gateway_caller::ReplayWindows`), keyed on expiry. `FederationClient::present`
mints each credential strictly later than its last. Counters on `GET /stats`: `federation_replays_refused`,
`federation_replays_undecidable`. Lock-order row 75.

**Why:** #695's review, finding 1 — a body binding cannot tell a captured call's copy from the original, and the edge
kept no record, so a replayed `pantry/reserve` repeated its effect.

**Limits, stated** (`docs/operations/federation.md` § A credential is one call): per gateway, in memory (a restart
forgets), a wall clock stepped back past the skew tolerance re-opens what was forgotten, two clients of one domain with
one principal can mint one credential in the same millisecond. No guarantee id (nothing to configure).

**Upgrade note:** `deprecations.md` §67.
