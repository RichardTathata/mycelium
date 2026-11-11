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
authenticated where the call runs, not only at the door. The gateway exchanges only for an **action** — the kinds a
verifying provider requires an `ot` for (below); every other call by an OIDC user is unchanged.

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

`validate()` also refuses `exchange_audience` on a node without `[tls]` (`oidc_exchange_needs_tls`: the token binds to
the key that signs the envelope) and equal to `audience` (`oidc_exchange_audience_is_gateway_audience`). `OidcConfig`
implements `Default`; write a literal with `..Default::default()`.

**The gateway's half (I5, PR 2).** A node with an HTTP gateway and `exchange_audience` exchanges an OIDC user's own
bearer at the IdP (RFC 8693) for every action it dispatches for that user, under its own client credential, with a DPoP
proof (RFC 9449, `alg: EdDSA`) signed by its node key, and carries the token as `ot`. Two more settings:
- `exchange_token_endpoint` — the IdP's token endpoint;
- `exchange_client_secret_env` — the environment variable holding the gateway's client secret (default
  `MYCELIUM_OIDC_EXCHANGE_SECRET`), read once at start.

The start is refused by name when the endpoint is unset (`oidc_exchange_endpoint_unset`), the client unset
(`oidc_exchange_client_unset`), the secret's variable unset or empty (`oidc_exchange_secret_unset`), or the egress
allow-list does not permit the endpoint. A provider-only node (no `http_port`) needs none of them. At call time an
exchange that does not yield a token refuses the call, and nothing runs: `oidc_exchange_unreachable` (no answer within
10 s), `oidc_exchange_refused` (the IdP's error), `oidc_exchange_malformed` (no token in its answer) — JSON-RPC
`-32024`, HTTP 502 at the raw route. A non-OIDC principal's call, and an OIDC user's call of a kind that is not an
action, make no exchange. `mycelium doctor` reports [`id.oidc_exchange`](../reference/guarantee-catalogue.md#id.oidc_exchange).

The provider fetches the issuer's keys at `start()` and refreshes them through the node's one verifier, which its
gateway uses for bearers too — one cache, one fetcher
([`OidcVerifier::keys_now`](../../src/agent/oidc.rs)): an unknown `kid` starts a refresh at most once per 30 s, keys
past the 1 h TTL refresh in the background on use, and keys more than 24 h past their last successful fetch are not
used. A token that arrives while no key is held is refused, retriable.

Every envelope carrying an `ot` is checked at the receive boundary
([`exchanged_token::check`](../../src/agent/exchanged_token.rs)), in this order; the first failing check names the
refusal:

| Check | Refused as |
|---|---|
| The gateway's signature over the token, `so` | `ot_unsigned` (absent), `ot_signature_malformed`, `bad_signature`; `ot_unverifiable` on a mesh without `[tls]` |
| The header: an asymmetric `alg`, an access token's `typ` (`JWT` or `at+jwt`) if any, a `kid` | `ot_malformed` (`none`, `HS*`, no `kid`), `ot_wrong_type` |
| The keys | `ot_keys_unavailable` (none held, or the `kid` unknown: a refresh has started), retriable |
| The token's signature, against this provider's JWKS | `ot_signature_invalid` |
| `iss`, then `aud` (a string or an array), then `azp` | `ot_wrong_issuer`, `ot_wrong_audience` (a gateway-audience token forwarded rather than exchanged), `ot_wrong_client` |
| Sender binding: `cnf.jkt` the thumbprint of the key that signed this envelope | `ot_unbound`, `ot_bound_to_another_key` |
| The subject: the envelope's principal is `oidc:{iss}/{sub}` | `ot_subject_mismatch` |
| `iat`, `exp`, `nbf` under `max_clock_drift_ms`, at the instant the window admits at; `exp − iat` | `ot_without_iat`, `ot_without_exp`, `ot_not_yet_valid`, `ot_expired`, `ot_lifetime_too_long` |
| A `jti`, admitted once per provider, whichever gateway carries it, while unexpired | `ot_without_jti`, `ot_replayed` |
| The window's room: 262 144 ids, 65 536 per gateway, 16 384 per principal | `ot_provider_window_full`, `ot_window_full`, `ot_principal_window_full` |

An `oidc:` principal's **action** without an `ot` is refused `ot_required`: `mcp.invoke`, `skill.invoke`, `llm.invoke`,
each kind in `protected_rpc_kinds`, every companion kind whose scope is `:write` (every `blackboard.*`, `tuple.*`
and `wiki.*` kind but their read operations), and a hosted capability's invoke, `cap.invoke/{namespace}/{name}`. The
artifact kinds and `reason.blob.fetch` are not actions. A build without `compliance` refuses any `ot` as `not_in_build`.

The tests, each refusal seen failing first, are in
[`src/agent/exchanged_token_tests.rs`](../../src/agent/exchanged_token_tests.rs):
`an_honest_exchanged_token_is_verified_and_admitted_once`, `each_check_refuses_an_exchanged_token_by_name`,
`the_gateways_signature_over_the_token_is_required`, `an_oidc_action_without_an_exchanged_token_is_refused`,
`a_token_arriving_before_any_key_is_held_is_refused_retriably`,
`a_provider_without_an_exchange_audience_reads_no_exchanged_token`,
`the_exchanged_token_window_is_bounded_per_principal_and_per_gateway`,
`the_kinds_an_oidc_action_needs_an_exchanged_token_for`, `the_exchange_settings_are_refused_by_name_when_incomplete`,
`an_envelope_too_large_with_its_exchanged_token_is_refused_by_the_producer`,
`a_token_is_read_against_the_instant_its_window_prunes_at`,
`a_node_that_is_gateway_and_provider_fetches_its_keys_once`, `the_first_failing_check_names_the_refusal`,
`the_header_and_audience_shapes_are_refused_or_accepted_by_name`, `a_token_in_an_unsigned_envelope_is_unverifiable`,
`a_verify_off_a_runtime_refuses_rather_than_panics`, `the_thumbprint_is_rfc_8037s`,
`the_window_is_one_across_gateways_and_frees_a_departed_ones`,
`two_concurrent_admissions_of_one_token_admit_it_once`, `an_oidc_literal_with_default_is_the_minimal_table` and
`preflight_builds_the_verifier_and_fetches_nothing`. A build without `compliance` is
tested by `a_build_without_compliance_refuses_an_exchanged_token`, in
[`src/agent/gateway_caller.rs`](../../src/agent/gateway_caller.rs).

The gateway's half is tested in [`src/agent/oidc_exchange_tests.rs`](../../src/agent/oidc_exchange_tests.rs), against a
mock IdP: `an_oidc_users_call_is_exchanged_at_the_gateway_and_verified_at_the_provider`,
`only_an_oidc_users_action_is_exchanged_for`, `the_exchange_settings_are_refused_by_name_at_start`,
`an_exchange_that_yields_no_token_is_refused_by_name` and `the_dpop_proof_is_an_eddsa_jws_under_the_node_key`.

## See it work

No runnable example exists: the [example index](../../examples/README.md#the-capability-matrix) has no row for SSO,
because it needs an IdP. The end-to-end run is the test in
[`src/agent/http/http_tests.rs`](../../src/agent/http/http_tests.rs): `test_gateway_oidc_jwt_maps_groups_to_scopes`.
The refusal of an issuer the egress policy denies is `an_oidc_issuer_the_egress_policy_denies_refuses_the_start`, in
[`src/lib_tests.rs`](../../src/lib_tests.rs).

The exchange against a real IdP is `oidc_exchange_live_an_oidc_users_call_is_verified_at_the_provider`, in
[`src/agent/oidc_exchange_live_tests.rs`](../../src/agent/oidc_exchange_live_tests.rs): Keycloak 26.8.2, pinned by
digest, in CI's `oidc-exchange-live` job; it makes its own realm, and an OIDC user's two `/mcp` calls are each exchanged
by the gateway and verified at the provider. Without `MYCELIUM_KEYCLOAK_URL` it skips itself, and under
`MYCELIUM_OIDC_EXCHANGE_LIVE_REQUIRED` (which that job sets) a missing Keycloak fails it.

## What it guarantees, and what it does not

- **Guarantee:** [`gw.oidc`](../reference/guarantee-catalogue.md#gw.oidc) — operators authenticate through the IdP,
  groups mapped to scopes.
- **Rule:** [`gateway.auth`](../reference/rule-catalogue.md#gatewayauth), where the JWT is resolved.
- **What it refuses:** symmetric (`HS*`) and `none` algorithms outright, an unknown `kid` after one refresh, and a key
  held past a 24-hour ceiling after its last successful fetch ([sso § 4](../operations/sso.md#4-security-notes-what-the-validator-enforces)).
- **Guarantee:** [`id.oidc_exchange`](../reference/guarantee-catalogue.md#id.oidc_exchange) — an OIDC user's action
  is exchanged at the gateway and verified at the provider.
- **At the provider (I5):** an exchanged token bound to the envelope's signer, for its principal, by the gateway's
  client, admitted once per provider, whichever gateway carries it, while unexpired. **Not built:** a token bound
  to its call; the gateway as attacker (it holds the client secret and sees users' tokens, so it can act as any user
  who calls through it); an IdP other than Keycloak (the live run is Keycloak 26.8.2); an IdP without exchange; the exchanging gateway reusing its own token, refused once per provider by `jti`; four principals at
  their bound filling a gateway's share, or four gateways at theirs filling the provider's window (262 144 ids, about
  436 calls a second at the defaults), after which every other gateway's OIDC users are refused
  `ot_provider_window_full` until entries expire (rate limits are the hard bound); the per-principal bound (16 384)
  counts across gateways; a window that survives a restart (an id
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
