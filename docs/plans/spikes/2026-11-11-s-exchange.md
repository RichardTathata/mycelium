# Spike S-exchange — 2026-11-11

Settles row I5 of [`identity-and-lifecycle.md`](../identity-and-lifecycle.md) (D6), on the maintainer's instructions of
2026-11-11. It ran in two parts: per-call binding first, then sender binding after #757's review (H1–H3).

**Evidence:** the tag **`spike/s-exchange-2026-11-11-r2`** (`92361dd2`), on the branch `spike-s-exchange`, off `main`
at `e7889001`. Its code is throwaway and never merges. The first round's tag, `spike/s-exchange-2026-11-11`
(`78934fc2`), is kept and not moved. The evidence lives in `spikes/s-exchange/`:
- **Part 1:** the probes (stdlib Python), their **raw** output (`run-*.jsonl`, with the exchanged token on each `TOKEN`
  line), and both containers' whole logs (`kc-a-26.2.5.log`, `kc-b-26.2.5.log`).
- **Part 2:** `part2/harness.py`, which holds the image digest itself. Beside it are its raw output (`results.jsonl`,
  tokens included), each phase's whole container log, the server's `/admin/serverinfo`
  (`serverinfo-default.json`, carrying the feature table cited below) and the throwaway certificates. Their keys are
  not committed.
- `spike_s_exchange_envelope_size` in `src/agent/client_grant_tests.rs` measures the envelope's size, and
  `envelope-size.txt` is its output.

The realm (`fleet`), built by both parts over the admin API:
- a public client `user-app`, whose tokens carry `mycelium-gateway` in `aud` (the gateway-audience token a user
  presents);
- a confidential client `mycelium-gateway`, with the client credential the gateway exchanges under and
  `standard.token.exchange.enabled`, whose tokens carry `mycelium:fleet` in `aud`;
- the target audience `mycelium:fleet`;
- a user `alice`.

## The questions

The maintainer, verbatim: *can a per-call nonce (the envelope's `cn`, or the gateway's own per-call value) be bound into
the exchanged token? Through which mechanism: a requested claim, `actor_token`, a client-scope mapper, or not at all?
Measure the exchanged token's size against the 8 KiB envelope cap, beside a grant and a `cs`. Conclude with exactly
what "a replay refused" in I5's exit can rest on.*

After #757's review: *run DPoP (RFC 9449) and RFC 8705 mTLS certificate-bound tokens on the current Keycloak release,
pinned by digest in the harness … say for each whether it is supported or preview/experimental … for DPoP, show
`cnf.jkt` on the exchanged token, and that the algorithm the gateway would use is accepted … then state the
provider-side check I5 would make.*

## Answers

**A per-call value: not bound in a supported configuration** (part 1, Keycloak 26.2.5). Only the experimental
`dynamic-scopes` feature bound one.

**The sender: bound, in Keycloak's current release with default features** (part 2, Keycloak **26.8.2**).
- **DPoP:** the exchanged token carries `cnf.jkt` equal to the proof key's RFC 7638 thumbprint. **EdDSA (Ed25519) is
  accepted** alongside ES256, so the gateway can prove possession with the Ed25519 node identity key it already signs
  envelopes with.
- **mTLS:** the exchanged token carries `cnf.x5t#S256` equal to the client certificate's hash. An **Ed25519** client
  certificate works, and a request without one is refused.
- In 26.8.2's own feature table, `DPOP` and `TOKEN_EXCHANGE_STANDARD_V2` are both `type: DEFAULT, enabled: true`. The
  mTLS binding is a client attribute, not a feature.

**So I5 binds the token to the gateway that exchanged it, with DPoP under the gateway's node key.** The provider
refuses a token without `cnf.jkt`, and one whose `cnf.jkt` is not the thumbprint of the key that signed the envelope.
That closes the member replay #757 found (H1, H2): a member's envelope is signed by the member's own key. See *The
provider's check*, below.

## Part 1 — binding a per-call value (Keycloak 26.2.5)

- **The image:** `quay.io/keycloak/keycloak@sha256:4883630ef9db14031cde3e60700c9a9a8eaf1b5c24db1589d6a2d43de38ba2a9`,
  in `start-dev`. #757's review called it an old release, which is why part 2 runs the current one.
- **Instance A** ran default features.
- **Instance B** ran `--features=dynamic-scopes`. Its log carries the line
  `WARN [org.keycloak.common.Profile] (main) Experimental features enabled: dynamic-scopes:v1` (`kc-b-26.2.5.log`).
- The value used was `0194f2a07c1e0000-3f9a5c1d2e4b6a70-17`. "Absent" means it appears nowhere in the decoded token.

| Mechanism | Inst. | Request | Result |
|---|---|---|---|
| Baseline exchange | A | `audience=mycelium:fleet`, the user's token, the gateway's client credential | 200: `aud=mycelium:fleet`, `azp=mycelium-gateway`, a fresh `jti` per exchange, a 300 s lifetime, **1218 B** |
| A request parameter | A | `cn=<value>`, `nonce=<value>` | 200, absent |
| A requested claim | A | `claims={"access_token":{"cn":{"value":…}}}` | 200, absent |
| The claims-parameter mappers attached | A | `oidc-claims-param-token-mapper`, `oidc-claims-param-value-idtoken-mapper`; access and ID tokens | 200, absent in both |
| Session-note mappers | A | on `client_request_param_cn` and `cn`, with `cn=<value>` sent | 200, absent |
| `actor_token` | A | the gateway's own `client_credentials` token | 200: accepted and ignored, no `act` claim |
| A dynamic scope | A | `scope=cn:<value>` | 400 `invalid_scope` (the feature is off) |
| A dynamic scope | B | the scope optional on the gateway's client only | 400 `invalid_scope`, though a `client_credentials` grant with the same scope carries it |
| A dynamic scope, optional on the user's client too | B | subject token without it; `cn:call-0003`, then `cn:call-0004` | 200: each value carried in `scope`, **1235 B**; two values in one request are both carried |
| Latency, loopback | A / B | 200 sequential exchanges after 20 warm-ups | p50 3.85 / 3.89 ms, p99 12.6 / 15.4 ms |

**A mapper survey, not a run.** The server's mapper types were listed from `/admin/serverinfo`: 23 on 26.2.5, 25 on
26.8.2 (`*_mapper_type_survey`). Only the three named above were attached and run. A type was judged unable to read
the request from its name and documentation, not by a run.

## Part 2 — sender-constrained tokens (Keycloak 26.8.2)

- **The image:** `quay.io/keycloak/keycloak@sha256:723b1d6e254c0fc96d5cffec860c7e0744d6a3277822b865bb538cf891575e41`.
  This is `quay.io/keycloak/keycloak:latest` on 2026-11-11, built 2026-11-09. `serverinfo` reports `systemInfo.version`
  **26.8.2**.
- **How it ran:** over HTTPS, with client certificates requested (`--https-client-auth=request`), trusting a P-256 CA
  and an Ed25519 CA. Every probe ran twice: with default features, then with `--features=dpop`. Both phases gave the
  same results.

**Support, from the server's own feature table** (`serverinfo-default.json`, `features`):

| Feature | Type | Enabled by default |
|---|---|---|
| `DPOP` ("OAuth 2.0 Demonstrating Proof-of-Possession at the Application Layer") | `DEFAULT` | yes |
| `TOKEN_EXCHANGE_STANDARD_V2` ("Standard Token Exchange version 2") | `DEFAULT` | yes |
| `TOKEN_EXCHANGE` (the legacy service) | `PREVIEW`, deprecated | no |
| `TOKEN_EXCHANGE_DELEGATION` | `PREVIEW` | no |
| `PARAMETERIZED_SCOPES` | `PREVIEW` | no |
| `RESOURCE_INDICATORS` | `EXPERIMENTAL` | no |
| `SCRIPTS` | `PREVIEW` | no |
| `CLIENT_POLICIES` | `DEFAULT` | yes |

Keycloak logs only non-default features. The 26.8.2 log names the deprecated ones (`Deprecated features
identity-brokering-api:v1, twitter-broker:v1 enabled by default`), and no preview or experimental feature appears in
either phase's log (`part2/default.log`, `part2/dpop_feature.log`). **The feature table is the evidence of support, not
a log line.** The release notes were not consulted.

| Probe | Request | Result |
|---|---|---|
| Baseline exchange | as part 1 | 200, `token_type: Bearer`, no `cnf`, 1203 B |
| **DPoP, ES256** | a DPoP proof (P-256 key, `htm=POST`, `htu`= the token endpoint) in the `DPoP` header | 200, `token_type: DPoP`, `cnf: {"jkt": <the key's RFC 7638 thumbprint>, "kc-jkt-type": "DPoP"}`, **`jkt` matches**, 1309 B |
| **DPoP, EdDSA** | the same, with an Ed25519 key (`alg: EdDSA`, an OKP JWK) | 200, `token_type: DPoP`, **`jkt` matches the Ed25519 thumbprint**, 1309 B. EdDSA is accepted (its `cryptoInfo` lists `EdDSA` among the asymmetric signature algorithms) |
| **mTLS, P-256 client certificate** | the gateway's client with `tls.client.certificate.bound.access.tokens=true`, the exchange over TLS with the certificate | 200, `cnf: {"x5t#S256": <the certificate's SHA-256>}`, **matches**, 1290 B |
| **mTLS, Ed25519 client certificate** | the same, with an Ed25519 certificate under an Ed25519 CA (the fleet CA's algorithm) | 200, **`x5t#S256` matches**, 1290 B |
| mTLS, no client certificate | the same client setting, no certificate presented | 400 `invalid_request`: *"Client Certification missing for MTLS HoK Token Binding"* |
| `resource` parameter | `resource=urn:mycelium:call:<value>` | 200, the value absent (`RESOURCE_INDICATORS` is off) |
| `requested_token_type=refresh_token` | | 400 `invalid_request`: *"requested_token_type unsupported"* |
| Lightweight access token | the gateway's client with `client.use.lightweight.access.token.enabled` | 400 `invalid_request`: *"Requested audience not available: mycelium:fleet"*. The lightweight token drops the audience mapper this realm uses, so it was not shown with another configuration |

## The envelope's size against the 8 KiB cap

Measured with the one producer, `frame_with_client_signature_at`, in `spike_s_exchange_envelope_size`:
- an `oidc:` principal of Keycloak's shape, and the scope `mcp:invoke`;
- the mandate a client presents for a grant issued on the node;
- the client's `cs`/`cn`;
- the token, counted as the bytes `,"ot":"<token>"` adds.

| Envelope | Bytes | Under the 8 192 B cap |
|---|---:|---:|
| bare | 485 | 7 707 |
| + a presented grant | 1 419 | 6 773 |
| + a grant and `cs` | 1 628 | 6 564 |
| + a grant, `cs` and the 1 218 B bearer token (26.2.5) | 2 854 | 5 338 |
| + a grant, `cs` and the **1 309 B DPoP-bound token** (26.8.2) | **2 945** | **5 247** |
| + a grant, `cs` and a 4 096 B token (computed) | 5 732 | 2 460 |

With a grant and a `cs` beside it, the cap holds a token of up to **6 556 B (6.4 KiB)**: the 6 564 B left, less the 8 B
of the `ot` field's framing. The measured tokens belong to a user with no groups or client roles. Past the cap, the
producer refuses `caller_context_too_large`.

## The provider's check

**With DPoP, the gateway's Ed25519 node key is the one bound.**
- The gateway signs the exchange's DPoP proof with the key it signs envelopes with. The spike shows Keycloak accepts an
  EdDSA proof, so **no P-256 key is needed, and nothing new is published.**
- The provider already resolves that key: `gateway_caller::verify` checks that the envelope's signer holds `via`'s key.
- The provider computes the RFC 7638 thumbprint of the Ed25519 key that verified **this envelope's** signature (an OKP
  JWK, `{"crv":"Ed25519","kty":"OKP","x":…}`), and requires `cnf.jkt` to equal it.
- A token without `cnf.jkt`, or with another key's thumbprint, is refused by name.
- The envelope's gateway signature is the proof of possession at the provider. It covers the principal, the target
  `o`, the id `n`, the time and the payload's digest, and it is made per call by the bound key. No DPoP proof travels
  to the provider.

**What it closes, and what it does not:**
- **A member is refused.** A member that lifts an `ot` from a frame it relays (H1) signs its own envelope with its own
  key, so the thumbprint differs. Any envelope by any node other than the exchanging gateway is refused.
- **The exchanging gateway itself is not bound by this.** It can present one token in several envelopes of its own
  until `exp`, and the `jti` check below is what refuses that.

**The `jti` check also stays**, so that one token is admitted once per provider.

**mTLS is the alternative, not chosen:**
- The gateway would present its fleet-CA node certificate to the IdP. The spike shows an Ed25519 client certificate is
  accepted.
- The provider would compare `cnf.x5t#S256` with the SHA-256 of `via`'s certificate, which it reads from the
  certificate records (`sys/identity-cert/`).
- It needs the IdP's TLS endpoint to trust the fleet CA and to request client certificates. That is an operator
  burden DPoP does not have.
- Its check is one hash of a certificate the provider must look up, where DPoP's is one thumbprint of the key the
  provider already used.

## What this could not do

- **One IdP.** Keycloak alone. Whether Entra, Okta or Auth0 issue DPoP-bound exchanged tokens, or accept EdDSA proofs,
  is not shown.
- **Not run:**
  - client-policy executors;
  - script mappers (`SCRIPTS` is preview, and needs deployed JavaScript);
  - a custom ProtocolMapper SPI;
  - `TOKEN_EXCHANGE_DELEGATION` (preview);
  - `PARAMETERIZED_SCOPES` (preview; part 1's dynamic scopes were not re-run on 26.8.2);
  - the legacy `TOKEN_EXCHANGE` (preview, deprecated);
  - a lightweight access token with an audience the token keeps;
  - a token lifetime other than the realm's 300 s;
  - a refresh or revocation of the subject token between exchanges;
  - the gateway's node key rotated between the exchange and the call (the bound key is then the old one, and the
    provider refuses).
- **A minimal user.** Larger tokens are computed, not issued.
- **Loopback.** The latency is loopback, sequential and single-client.
