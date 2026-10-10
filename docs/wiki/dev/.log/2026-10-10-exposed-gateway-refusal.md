# 2026-10-10 — an open gateway is a loopback gateway (plan P1)

## [2026-10-10] ingest | exposed-gateway refusal

- `start()` refuses a gateway on a non-loopback `http_addr` with no credential model, as field `http_addr`, unless
  `gateway_allow_unauthenticated` (`GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED`) waives it — then it warns once.
  Loopback = `127.0.0.0/8`, `::1`, IPv4-mapped loopback; `0.0.0.0`/`::` are exposed; a blank token is no credential.
- Guarantee `gw.exposed_closed`; `secure-single-domain` rev 3 requires it. One predicate,
  `guarantee::gateway_credential_model`, for `gw.not_open`, `gw.exposed_closed` and the refusal.
- Demo nodes `three_node_demo` and `federation_node` opt in with a comment. Page: [security](../security.md)
  § *An open gateway is a loopback gateway*.
