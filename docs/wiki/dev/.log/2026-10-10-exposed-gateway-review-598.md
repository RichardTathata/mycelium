# 2026-10-10 — #598's adversarial review, answered

## [2026-10-10] ingest | blank secrets, the rev 3 upgrade note, the demo's published ports

- One rule for blank secrets: `validate()` refuses an empty or whitespace-only token in every form, by name;
  `resolve_token` never matches a blank presented bearer (HTTP/2 does not trim `Bearer `).
- `secure-single-domain` rev 3's note corrected: it refuses one node rev 2 admitted (a blank-token-only gateway).
- `docker/docker-compose.yml` publishes the waived gateways on the host's loopback only; the demos' own `0.0.0.0`
  listeners are named as outside the library. `gw.exposed_closed` names the routes that stay public.
  Pages: [security](../security.md), [operations](../operations.md).
