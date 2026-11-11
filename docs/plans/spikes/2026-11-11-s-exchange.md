# Spike S-exchange — 2026-11-11

Settles row I5 of [`identity-and-lifecycle.md`](../identity-and-lifecycle.md) (D6), on the maintainer's instruction of
2026-11-11.
- **Evidence:** the tag **`spike/s-exchange-2026-11-11`** (`78934fc2`), on the branch `spike-s-exchange`, off `main` at
  `e7889001`. Its code is throwaway and never merges.
  - `spikes/s-exchange/` holds the probes (stdlib Python), their raw outputs (`run-*.jsonl`) and a README with the
    commands.
  - `spike_s_exchange_envelope_size` in `src/agent/client_grant_tests.rs` is the size measurement, and
    `envelope-size.txt` is its output.
- **The IdP:** Keycloak **26.2.5**, `quay.io/keycloak/keycloak@sha256:4883630ef9db14031cde3e60700c9a9a8eaf1b5c24db1589d6a2d43de38ba2a9`,
  in Docker (`start-dev`). There are two instances:
  - **A**, with default features. Standard token exchange (RFC 8693, Keycloak's "V2") is a supported feature, on by
    default.
  - **B**, with `--features=dynamic-scopes`, which Keycloak logs as **experimental** (`Experimental features enabled:
    dynamic-scopes:v1`).
- **The realm** (`fleet`), configured by `probe.py` over the admin API:
  - a public client `user-app`, whose tokens carry `mycelium-gateway` in `aud` (the gateway-audience token a user
    presents);
  - a confidential client `mycelium-gateway`, with the client credential the gateway exchanges under and
    `standard.token.exchange.enabled`, whose tokens carry `mycelium:fleet` in `aud`;
  - a client `mycelium:fleet`, the exchange's target audience;
  - a user `alice`.

## The question

From the maintainer, verbatim: *can a per-call nonce (the envelope's `cn`, or the gateway's own per-call value) be
bound into the exchanged token? Through which mechanism: a requested claim, `actor_token`, a client-scope mapper, or
not at all? Measure the exchanged token's size against the 8 KiB envelope cap, beside a grant and a `cs`. Conclude
with exactly what "a replay refused" in I5's exit can rest on.*

**Answer: not in a supported configuration.**
- With Keycloak's default features, nothing the gateway sends in the exchange request reaches the exchanged token.
  Every mechanism below was tried, and the value appears nowhere in the token.
- With the **experimental** `dynamic-scopes` feature, the gateway can bind a per-call value as a dynamic scope
  (`scope=cn:<value>`), which the token carries in its `scope` claim. That needs the dynamic scope configured as optional
  on **the subject token's client** as well as the gateway's.
- So I5's "a replay refused" rests on **the token's `jti`, admitted once by the provider within its `exp`**, beside the
  envelope's own replay window. It does not rest on the token naming the call (below, *What "a replay refused" can
  rest on*).

## What was run

The `cn` value used was `0194f2a07c1e0000-3f9a5c1d2e4b6a70-17` (the shape of an envelope id). "Absent" means the value
appears nowhere in the decoded token (`nonce_anywhere: false`).

| Mechanism | Instance | Request | Result |
|---|---|---|---|
| Baseline exchange | A | `audience=mycelium:fleet`, the user's token as `subject_token`, the gateway's client credential | 200. `aud = mycelium:fleet`, `azp = mycelium-gateway`, `sub` the user, a `jti` per exchange (two exchanges differ), lifetime 300 s (the realm's default access-token lifespan), **1218 B** |
| A requested parameter | A | `cn=<value>`, `nonce=<value>` | 200, absent: unknown parameters are ignored |
| A requested claim | A | `claims={"access_token":{"cn":{"value":<value>}}}` | 200, absent |
| The same, with the claims-parameter mappers attached | A | the gateway's client given `oidc-claims-param-token-mapper` and `oidc-claims-param-value-idtoken-mapper`; access and ID tokens requested | 200, absent in both (the ID token is 1031 B). A standard claim (`nickname`) asked for by value is absent too |
| A session-note mapper | A | `oidc-usersessionmodel-note-mapper` on the notes `client_request_param_cn` and `cn`, with `cn=<value>` sent | 200, absent: the exchange sets no note from its request |
| `actor_token` | A | the gateway's own `client_credentials` token as `actor_token` | 200. **Accepted and ignored**: no `act` claim, the token unchanged |
| Any other mapper | A | the server's 23 OIDC mapper types (`run-a.jsonl`, `mapper_types`) | none reads the token request, apart from the three tried above |
| A dynamic scope | A | `scope=cn:<value>` | **400** `invalid_scope` (the feature is off) |
| A dynamic scope | B | `scope=cn:<value>`, the scope optional on the gateway's client only | **400** `invalid_scope` |
| A dynamic scope, its configuration checked | B | a `client_credentials` grant with `scope=cn:<value>` | 200, `scope` carries `cn:<value>`: the scope is configured right, and exchange refuses it |
| A dynamic scope, optional on the user's client too | B | a subject token issued **without** the scope, exchanged with `scope=cn:call-0003`, then `cn:call-0004` | 200 each. `scope` carries exactly the value asked for, a different value on each exchange, **1235 B**. The binding is per call, and the gateway chooses the value |
| Two values in one request | B | `scope=cn:call-0005 cn:call-0006` | 200, both carried (1253 B). Keycloak does not hold the gateway to one value, so a verifier would have to require exactly one |
| Exchange latency, loopback | A / B | 200 sequential exchanges after 20 warm-ups, each with a fresh value on B | p50 **3.85 / 3.89 ms**, p90 5.35 / 5.76 ms, p99 12.6 / 15.4 ms, max 19.4 / 19.2 ms |

### The envelope's size against the 8 KiB cap

Measured with the one producer, `frame_with_client_signature_at`, in `spike_s_exchange_envelope_size`:
- an `oidc:` principal of Keycloak's shape and the scope `mcp:invoke`;
- the mandate a client presents for a grant issued on that node (the signed grant and its possession proof);
- the client's `cs`/`cn`;
- the exchanged token, counted as the bytes `,"ot":"<token>"` adds, since it is not yet a field.

| Envelope | Bytes | Under the 8 192 B cap |
|---|---:|---:|
| bare (principal, scopes, the gateway's signature, id, target) | 485 | 7 707 |
| + a presented grant | 1 419 | 6 773 |
| + a grant and `cs` | 1 628 | 6 564 |
| + a grant, `cs` and the 1 218 B token measured | **2 854** | **5 338** |
| + a grant, `cs` and the 1 235 B token with one dynamic-scope value | 2 871 | 5 321 |
| + a grant, `cs` and a 2 048 B token (computed) | 3 684 | 4 508 |
| + a grant, `cs` and a 4 096 B token (computed) | 5 732 | 2 460 |

The measured token belongs to a user with no groups and no client roles. Groups and roles grow a token. The cap holds
up to a token of about **6.5 KiB** beside a grant and `cs`. Past that, the producer's existing refusal answers,
`caller_context_too_large`, as the maintainer decided the cap stays.

## What "a replay refused" can rest on

**What the token gives a verifier.** In the supported configuration, an exchanged token names:
- the user (`sub`);
- the audience (`aud = mycelium:fleet`);
- the client that exchanged it (`azp = mycelium-gateway`);
- an id unique per exchange (`jti`);
- a lifetime (`exp`, 300 s here).

It names nothing of the call.

**So "a replay refused" rests on three checks at the provider:**
1. **The `jti`, admitted once** while the token is unexpired: a second presentation of one exchanged token is refused,
   in any envelope. This needs one exchange per call, which costs about 4 ms at p50 on loopback.
2. **The envelope's own replay window**, which exists today (post-360 row E): one envelope, by its gateway-signed id
   `n`, is admitted once.
3. **`azp` equal to the gateway's configured client**, so a token another client of the IdP exchanged is not one this
   fleet's gateway asked for.

**What these do not refuse.** A member that copies a token out of a frame in transit, and presents it in an envelope of
its own **before** the honest one arrives, is admitted once: the token does not name the call it was exchanged for.
- Binding the token to the call would close that. It is available only through Keycloak's **experimental** dynamic
  scopes, configured on the user's client as well, with the provider requiring exactly one `cn:` value equal to the
  envelope's. The spike shows that mechanism works. It does not show that it is portable to other IdPs.
- Members are admitted by the fleet CA and relay each other's frames (individual-scope forwarding is a flood
  fallback), so this is the same trust boundary as a member reading any other caller envelope it relays.

**What the row's exit can say:**
- *"a replay refused"*: **the same exchanged token presented twice is refused**, by its `jti`, at the provider.
- *"a member's forged `ot` refused by the JWKS"*: unchanged. A token the member signs itself does not verify against
  the issuer's keys.

## What this could not do

- **One IdP.** RFC 8693 leaves the binding to the IdP, and the spike shows Keycloak only. Whether another IdP (Entra,
  Okta, Auth0) binds a per-call value, and how, is not shown.
- **Experimental is not supported.** The dynamic-scope binding rests on a feature Keycloak 26.2.5 marks experimental,
  so its behaviour may change between Keycloak releases. It was run against this digest only.
- **A minimal user.** The token sizes are for a user with no groups or client roles. Larger tokens are computed, not
  issued.
- **Loopback.** The latency is loopback, sequential and single-client. An IdP across a network adds its round trip to
  every call.
- **Not run:** the token's lifetime configured shorter than 300 s; a refresh or a revocation of the subject token
  between exchanges; Keycloak's legacy (V1) exchange, which is a preview feature.

## For I5's row (to build in the next PR)

The maintainer's decisions of 2026-11-11, and what the spike adds to them:
- **The exchange.** The gateway exchanges the user's token under its own client credential:
  - new `[oidc]` settings for the token endpoint and the client id, with the secret from an environment variable;
  - refused by name at start when unset;
  - clients do not change.
- **The audience.** `oidc.exchange_audience` is read by the gateway (to ask for it) and by the provider (to check it).
  The provider takes the issuer, the JWKS and the audience it checks `ot` against from its own `[oidc]` table.
- **The cap stays at 8 KiB.** The measured envelope is 2 854 B. An oversized one is refused `caller_context_too_large`.
- **Replay.** The provider admits a token's `jti` once while it is unexpired, and checks `azp` against the gateway's
  client id from its `[oidc]` table. So each call is exchanged afresh.
- **Not built, stated:**
  - a token bound to its call, which needs an experimental IdP feature;
  - a member front-running the honest call with a copied token, admitted once.
