## [2026-11-11] ingest | spike S-exchange: Keycloak's token exchange binds no per-call value (I5)

`docs/plans/spikes/2026-11-11-s-exchange.md`, against Keycloak 26.2.5 pinned by digest:
- A supported configuration carries nothing from the exchange request into the exchanged token. Request parameters,
  the claims parameter with its mappers, session-note mappers and `actor_token` were each ignored.
- Only the experimental `dynamic-scopes` feature binds a per-call value.
- An envelope with a grant, `cs` and the exchanged token measures 2 854 B against the 8 KiB cap.

So I5's "a replay refused" rests on the token's `jti`, admitted once. The maintainer's four I5 decisions are recorded
on the row (plan rev 0.11).
