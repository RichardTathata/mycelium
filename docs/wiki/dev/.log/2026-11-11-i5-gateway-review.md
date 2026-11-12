## [2026-11-11] ingest | I5 PR 2, #760's review: two resolution sites, one trigger each

`dev/security.md`: a bearer resolves to an `oidc:` principal at **two** places (`gateway_auth` and `a2a_optional_auth`);
a field that rides with the principal must be set at both — the first cut set the user's token at one, and `/a2a`
refused every OIDC skill call. The maintainer's decision: a node exchanges iff `exchange_token_endpoint` is set and
verifies iff `exchange_audience` is (the HTTP port decides neither); `doctor` reports the halves apart
(`id.oidc_exchange`, `id.oidc_exchanged_token`). The gateway's own refusals before sending give a claimed grant use
back. The exchange client follows no redirect and reads a bounded answer.
