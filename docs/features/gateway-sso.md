# Single sign-on at the gateway (OIDC)

## What it is

With an `[oidc]` table, the [gateway](http-gateway.md) accepts a JWT from your identity provider as a bearer. It
validates the signature against the IdP's published keys and checks the issuer, audience and expiry; the token's
groups map to gateway [scopes](../guide/00-concepts.md#scope), and the caller becomes an `oidc:` principal that
providers see behind the call. OIDC composes with the static token tables: the middleware tries a JWT first, and
every credential ends at the same scope gate.

**At the provider, too** (identity-and-lifecycle I5, [plan](../plans/identity-and-lifecycle.md)). A gateway may
exchange the user's token at the IdP (RFC 8693) for one whose audience is `exchange_audience`, bound by DPoP to the
gateway's node key, and carry it to the provider in the caller envelope (`ot`). A provider whose `[oidc]` table sets
`exchange_audience` verifies that token itself, against its own issuer and keys, so an `oidc:` caller is
authenticated where the call runs, not only at the door. **This release builds the provider's half; the gateway's
exchange is the next PR** (I5's PR 2), so until then nothing carries an `ot` and a verifying provider refuses an
`oidc:` action (below).

## When to use it, and when not

- **Use it when** human operators or services already authenticate to an IdP (Okta, Entra, Keycloak and the like),
  and you want gateway access granted and revoked there rather than by distributing tokens.
- **Not when** the build lacks `compliance`: an `[oidc]` table then refuses the start by name, in every build.

## Turn it on

Build with `compliance` and set the table. Under a non-empty egress allow-list, start is refused when the list does not
permit the issuer, or a `jwks_uri` you configured; a key host found by discovery is checked only when keys are fetched,
so a node the list does not let reach it starts and, holding no keys, answers every JWT 401. The per-vendor settings are in [sso § 3](../operations/sso.md#3-per-vendor-config-differences-are-just-group_claim--issuer).

- [`oidc`](../reference/configuration.md#oidc): `issuer`, `audience`, `group_claim`, `group_scopes` and an optional
  `jwks_uri` ([`OidcConfig`](../../mycelium-core/src/config.rs)).
- [`egress`](../reference/configuration.md#egress): when non-empty, must permit the issuer and the key host; set
  `jwks_uri` so `start()` checks the key host too ([sso § 6](../operations/sso.md#6-failure-modes)).
- No configuration file in the repository that a test loads sets `[oidc]`; the test below builds the table itself.
- `test_gateway_oidc_jwt_maps_groups_to_scopes`, in [`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs):
  a JWT from a mock IdP reaches a `kv:read` route, is refused on a `kv:write` route, and no token is 401.

**The provider's half (I5, PR 1).** A node verifies exchanged tokens when its `[oidc]` table sets
**`exchange_audience`** — the trigger; nothing else changes an `[oidc]` table:
- `exchange_audience` — the audience the exchanged token must carry;
- `exchange_client_id` — the `azp` it must carry, the client the gateway exchanges as; required with
  `exchange_audience` (`validate()` refuses `oidc_exchange_client_unset`);
- `exchange_max_lifetime_secs` — the longest `exp − iat` accepted; default 300, in `1..=3600`.

The provider fetches the issuer's keys at `start()` and refreshes them as the gateway's verifier does — the same cache
([`OidcVerifier::keys_now`](../../src/agent/oidc.rs)): an unknown `kid` starts a refresh at most once per 30 s, keys
past the 1 h TTL refresh in the background on use, and keys more than 24 h past their last successful fetch are not
used. A token that arrives while no key is held is refused, retriable.

Every envelope carrying an `ot` is checked at the receive boundary
([`exchanged_token::check`](../../src/agent/exchanged_token.rs)), in this order, each refusal by name:

| Check | Refused as |
|---|---|
| The gateway's signature over the token, `so` | `ot_unsigned` (absent), `bad_signature`; `ot_unverifiable` on a mesh without `[tls]` |
| The keys | `ot_keys_unavailable` (none held, or the `kid` unknown: a refresh has started), retriable |
| The token's signature, against this provider's JWKS | `ot_signature_invalid`; `ot_malformed` |
| `iss`, `aud`, `azp` | `ot_wrong_issuer`, `ot_wrong_audience` (a gateway-audience token forwarded rather than exchanged), `ot_wrong_client` |
| Sender binding: `cnf.jkt` the thumbprint of the key that signed this envelope | `ot_unbound`, `ot_bound_to_another_key` |
| The subject: the envelope's principal is `oidc:{iss}/{sub}` | `ot_subject_mismatch` |
| `exp`, `nbf`, `iat` under `max_clock_drift_ms`; `exp − iat` | `ot_expired`, `ot_lifetime_too_long` |
| A `jti`, admitted once while unexpired | `ot_without_jti`, `ot_replayed` |
| The window's room: 65 536 ids per gateway, 16 384 per principal | `ot_window_full`, `ot_principal_window_full` |

An `oidc:` principal's **action** without an `ot` is refused `ot_required`: `mcp.invoke`, `skill.invoke`, `llm.invoke`,
each kind in `protected_rpc_kinds`, and every companion kind whose scope is `:write` (every `blackboard.*`, `tuple.*`
and `wiki.*` kind but their read operations). A build without `compliance` refuses any `ot` as `not_in_build`.

The tests, each refusal seen failing first, are in
[`src/agent/exchanged_token_tests.rs`](../../src/agent/exchanged_token_tests.rs):
`an_honest_exchanged_token_is_verified_and_admitted_once`, `each_check_refuses_an_exchanged_token_by_name`,
`the_gateways_signature_over_the_token_is_required`, `an_oidc_action_without_an_exchanged_token_is_refused`,
`a_token_arriving_before_any_key_is_held_is_refused_retriably`,
`a_provider_without_an_exchange_audience_reads_no_exchanged_token`,
`the_exchanged_token_window_is_bounded_per_principal_and_per_gateway`,
`the_kinds_an_oidc_action_needs_an_exchanged_token_for`, `the_exchange_settings_are_refused_by_name_when_incomplete`
and `an_envelope_too_large_with_its_exchanged_token_is_refused_by_the_producer`. A build without `compliance` is
tested by `a_build_without_compliance_refuses_an_exchanged_token`, in
[`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs).

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for SSO,
because it needs an IdP. The end-to-end run is the test in
[`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs): `test_gateway_oidc_jwt_maps_groups_to_scopes`.
The refusal of an issuer the egress policy denies is `an_oidc_issuer_the_egress_policy_denies_refuses_the_start`, in
[`src/lib_tests.rs`](../../src/lib_tests.rs).

## What it guarantees, and what it does not

- **Guarantee:** [`gw.oidc`](../reference/guarantee-catalogue.md#gw.oidc) — operators authenticate through the IdP,
  groups mapped to scopes.
- **Rule:** [`gateway.auth`](../reference/rule-catalogue.md#gatewayauth), where the JWT is resolved.
- **What it refuses:** symmetric (`HS*`) and `none` algorithms outright, an unknown `kid` after one refresh, and a key
  held past a 24-hour ceiling after its last successful fetch ([sso § 4](../operations/sso.md#4-security-notes-what-the-validator-enforces)).
- **At the provider (I5, PR 1):** an exchanged token bound to the envelope's signer, for its principal, by the gateway's
  client, admitted once per provider while unexpired. **Not built:** the gateway's exchange (I5's PR 2); a token bound
  to its call; the exchanging gateway reusing its own token, refused once per provider by `jti`; four principals at
  their bound filling a gateway's window (rate limits are the hard bound); a window that survives a restart (an id
  presented before one can be presented once more after it, only in an envelope signed by the bound gateway).
- **Not built:** a key host found by discovery is not checked at start; the first tokens under a rotated key may be
  refused for up to the refresh cooldown (30 s). A token is a route credential: what
  the caller may do is the [action evaluator](action-authorisation.md)'s decision.
- **Evidence:** the ledger row [a withdrawn IdP key stops verifying](../operations/what-is-proven.md#companion-s-rpc-kind-needs-family)
  (rows R1–R4 of the 2026-10-31 review).

## Go deeper

- **Guide:** [09, security](../guide/09-security.md).
- **Runbook:** [sso](../operations/sso.md) (configure, per-vendor settings, security notes, verify, failure modes).
- **Design:** no design record; the SOC 2 audit-gap work and the external review of 2026-10-31 (row R4, the empty key
  set) are the record — [`plans/external-review-2026-10-31.md`](../plans/external-review-2026-10-31.md).
- **Code:** [`src/agent/oidc.rs`](../../src/agent/oidc.rs) (the verifier, the key cache and its refresh).
