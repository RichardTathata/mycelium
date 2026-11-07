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

Build with `compliance` and set the table; under a non-empty egress allow-list the IdP's issuer and key hosts must
be on it, or the start is refused. The per-vendor settings are in [sso § 3](../operations/sso.md#3-per-vendor-config-differences-are-just-group_claim--issuer).

- [`oidc`](../reference/configuration.md#oidc): `issuer`, `audience`, `group_claim`, `group_scopes` and an optional
  `jwks_uri` ([`OidcConfig`](../../mycelium-core/src/config.rs)).
- [`egress`](../reference/configuration.md#egress): must permit the issuer and the key host, when the allow-list is
  non-empty.
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
- **Not built:** symmetric (`HS*`) and `none` algorithms are rejected outright; a rotated key may be refused for up
  to the refresh cooldown, and the verifier keeps a key at most a stale ceiling after its last successful fetch
  ([sso § 4](../operations/sso.md#4-security-notes-what-the-validator-enforces)). A token is a route credential: what
  the caller may do is the [action evaluator](action-authorisation.md)'s decision.
- **Evidence:** the [what-is-proven](../operations/what-is-proven.md) ledger has no row specific to SSO.

## Go deeper

- **Guide:** [09, security](../guide/09-security.md).
- **Runbook:** [sso](../operations/sso.md) (configure, per-vendor settings, security notes, verify, failure modes).
- **Design:** no design record; the SOC 2 audit-gap work and the external review of 2026-10-31 (row R4, the empty key
  set) are the record — [`plans/external-review-2026-10-31.md`](../plans/external-review-2026-10-31.md).
- **Code:** [`src/agent/oidc.rs`](../../src/agent/oidc.rs) (the verifier, the key cache and its refresh).
