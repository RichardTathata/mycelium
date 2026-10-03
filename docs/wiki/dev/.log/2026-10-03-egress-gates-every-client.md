## [2026-10-03] ingest | the egress allow-list reaches every client the substrate chooses to make

**What:** `FederationClient::{with_egress, set_egress}` + `ClientError::Egress`; `with_federation_clients`
applies the node's policy; `OidcVerifier::new(cfg, egress)` gates discovery and JWKS; `start()` refuses an
`[oidc]` issuer/`jwks_uri` a non-empty allow-list denies. Crown-jewel coverage table updated.

**Durable knowledge:** a client constructed by the operator (the federation client, the artifact HTTP
source) cannot read the node's policy by itself, so "gated" has two halves — the type must *hold* a policy
and the agent must *hand it* one. `with_federation_clients` is the hand-off; a once-cell on the client is
what lets an `Arc`-shared client receive it without a lock. And an outbound dependency the node *must*
reach (the IdP) is not a reason to leave it ungated — it is a reason to refuse the contradiction at
start, by name, rather than find it behind a 401.
