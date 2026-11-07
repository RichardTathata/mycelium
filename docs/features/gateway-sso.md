# Single sign-on at the gateway (OIDC)

## What it is

With an `[oidc]` table, the [gateway](http-gateway.md) accepts a JWT from your identity provider as a bearer. It
validates the signature against the IdP's published keys and checks the issuer, audience and expiry; the token's
groups map to gateway [scopes](../guide/00-concepts.md#scope), and the caller becomes an `oidc:` principal that
providers see behind the call. OIDC composes with the static token tables: the middleware tries a JWT first, and
every credential ends at the same scope gate.

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
