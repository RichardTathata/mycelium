## [2026-11-11] ingest | spike S-exchange: no per-call binding, but DPoP binds the exchanged token to the gateway (I5)

`docs/plans/spikes/2026-11-11-s-exchange.md`, in two parts:
- **Keycloak 26.2.5:** a supported configuration carries nothing from the exchange request into the exchanged token.
  Request parameters, the claims parameter with its mappers, session-note mappers and `actor_token` were each ignored.
  Only the experimental `dynamic-scopes` bound a per-call value.
- **Keycloak 26.8.2, the current release, default features:**
  - DPoP binds the exchanged token to the exchanging key (`cnf.jkt`), and EdDSA proofs are accepted, so the gateway's
    Ed25519 node key can be the bound one.
  - RFC 8705 mTLS binds it too (`cnf.x5t#S256`), Ed25519 client certificates included.
- **Size:** an envelope with a grant, `cs` and the DPoP-bound token measures 2 945 B against the 8 KiB cap.

I5's row (plan rev 0.11), after #757's review and the maintainer's decision, is rewritten to sender binding:
- the provider refuses a token not bound to the key that signed its envelope;
- replay rests on that and the token's `jti` admitted once;
- the gateway as attacker, the replay cache, the settings and the start-time trigger are specified.
