# Mycelium — SSO / OIDC Operations Runbook

Operator guide to WS4 generic-OIDC single sign-on for the gateway (`compliance`
feature). This is **human-operator authentication** for the HTTP gateway — *not*
agent/node identity (that is the `tls` Ed25519 layer, see
[`../guide/09-security.md`](../guide/09-security.md)).

One code path serves every OIDC-conformant IdP — **Entra ID, Okta, Auth0,
Keycloak, Google** — by standard discovery (`.well-known/openid-configuration`)
and JWKS. Vendor differences are **configuration, not code**.

---

## 1. How it fits the gateway auth model

A gateway request may authenticate three ways; they compose:

1. **OIDC JWT** (this doc) — `Authorization: Bearer <jwt>`. Validated against the
   IdP's JWKS; the token's groups map to gateway scopes.
2. **Scoped token** — a static `gateway_scoped_tokens` bearer ([`rbac.md`](rbac.md) §2).
3. **Legacy token** — `gateway_auth_token` (≡ scope `"*"`).

The middleware tries OIDC first (if a JWT), then the static table. All three end
at the same **scope gate**: the route's required scope must be granted, or `"*"`.
Public routes (`/health`, `/ready`, `/stats`, `/metrics`, descriptor) are never gated.

---

## 2. Configure

```rust
cfg.oidc = Some(mycelium::OidcConfig {
    issuer:   "https://login.example.com/".into(), // must equal the JWT `iss`
    audience: "mycelium-cluster".into(),           // must equal the JWT `aud`
    group_claim: "groups".into(),                  // the claim carrying group names
    group_scopes: HashMap::from([
        ("platform-admins".into(), vec!["*".into()]),
        ("sre".into(),             vec!["kv:read".into(), "kv:write".into(), "consensus:read".into()]),
        ("auditors".into(),        vec!["audit:read".into()]),
    ]),
    jwks_uri: None, // None = discover via {issuer}/.well-known/openid-configuration
});
```

- **`issuer` / `audience`** are validated on every token (mismatch → 401).
- **`group_claim`** names the JWT claim holding the user's groups/roles — this is
  the main per-vendor knob (see §3).
- **`group_scopes`** maps each IdP group to gateway scopes; a user's scopes are
  the union over their groups. The scope vocabulary is the same as
  [`rbac.md`](rbac.md) §2. Each scope is either a named scope (`llm:read`, `kv:write`, …) or the
  single `"*"`. A family wildcard such as `llm:*` is refused by `validate()` at start
  (`oidc.group_scopes`, `mycelium-core/src/config.rs`): it would match nothing at the gateway, so
  list the family's scopes by name.
- **`jwks_uri`** — leave `None` for standard discovery; set explicitly only if the
  IdP's JWKS is hosted off the discovery path. Keys are cached (TTL ~1h) and
  re-fetched on an unknown `kid`, so IdP key rotation is picked up automatically.

---

## 3. Per-vendor config (differences are just `group_claim` + issuer)

| IdP | `issuer` | `group_claim` | Notes |
|---|---|---|---|
| **Entra ID** | `https://login.microsoftonline.com/{tenant}/v2.0` | `roles` (app roles) or `groups` | `groups` emits object IDs unless you configure group-name emission; app `roles` are often cleaner. |
| **Okta** | `https://{org}.okta.com/oauth2/{authz-server}` | `groups` | Add a `groups` claim to the authorization server's token policy. |
| **Auth0** | `https://{tenant}.auth0.com/` | `https://example.com/groups` (namespaced) | Auth0 namespaces custom claims; use the full namespaced claim name. |
| **Keycloak** | `https://{host}/realms/{realm}` | `groups` | Add a "Group Membership" mapper to the client scope. |
| **Google** | `https://accounts.google.com` | (no groups) | Groups require Cloud Identity / Directory; map a custom claim instead. |

In all cases `audience` is your registered client/application id, and the IdP must
be reachable from the node for discovery + JWKS fetch. If `egress.allow_hosts` is
non-empty it must permit the `issuer` and `jwks_uri` hosts. The start-time check covers the
`issuer` and a `jwks_uri` **you configured** — either off the list and **the node refuses to start**
naming it (since 2.20.0, `an_oidc_issuer_the_egress_policy_denies_refuses_the_start`). A `jwks_uri`
taken from discovery is checked only when keys are fetched: off the list, the node starts, logs
`oidc: the egress policy does not permit the JWKS host; no keys, every token refused`, and answers every
JWT 401. Google is the common case — issuer `accounts.google.com`, keys at `www.googleapis.com` — so allow
both, or set `jwks_uri` explicitly. Since 2.23.0 a redirect from either URL is checked against the list
too. Network-layer egress control must allow the same hosts.

---

## 4. Security notes (what the validator enforces)

- **Asymmetric algorithms only.** RS256/384/512, ES256/384, PS256/384/512. `HS*`
  and `none` are rejected outright — this closes the classic JWT alg-confusion
  attack (re-signing with the public key as an HMAC secret). The verifier never
  trusts the token header to choose the verification family.
- **Signature, `iss`, `aud`, and `exp`** are all checked (≈60s clock-skew leeway
  on expiry). Unknown `kid` → rejected (after one JWKS refresh attempt).
- **Failure is opaque.** Any validation failure is a flat `401` to the caller;
  the specific reason goes to logs only — never leak validation detail to an
  unauthenticated client.

---

## 5. Verify

```bash
# A valid IdP JWT for a user in a kv:read group reaches a kv:read route:
curl -H "Authorization: Bearer $JWT" http://NODE:PORT/gateway/kv/keys      # 200
# …but not a kv:write route:
curl -X POST -H "Authorization: Bearer $JWT" http://NODE:PORT/gateway/kv \
     -d '{"key":"k","value":"v"}'                                          # 403 {"required_scope":"kv:write"}
# No / invalid token:
curl http://NODE:PORT/gateway/kv/keys                                      # 401
```

CI uses an in-process mock IdP (discovery + JWKS) — no live vendor dependency;
see `src/agent/http.rs::test_gateway_oidc_jwt_maps_groups_to_scopes`.

---

## 6. Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| every JWT → 401 | `issuer`/`audience` mismatch, or JWKS unreachable | match `iss`/`aud` exactly; confirm the node can reach the IdP (egress) |
| every JWT → 401; log `oidc: the egress policy does not permit the JWKS host` | the discovered `jwks_uri` is on a host the allow-list does not permit | allow that host, or set `jwks_uri` so `start()` checks it |
| every JWT → 401 after upgrading to 2.23.0; log `egress: the redirect target … is not on the allow-list` | the issuer or JWKS URL redirects to an unlisted host | allow the redirect target, or configure the final URL |
| valid user → 403 | their groups map to no/insufficient scopes | extend `group_scopes`, or check `group_claim` is the right claim |
| works then breaks after IdP key rotation | stale JWKS cache | automatic — the verifier refetches on unknown `kid`; if persistent, check JWKS reachability |
| `groups` claim empty (Entra) | tenant emits group object-IDs or omits groups | switch to app `roles`, or configure group-name emission |
