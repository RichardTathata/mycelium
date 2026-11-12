## [2026-11-12] ingest | the log read routes refuse component-owned streams (CR1's follow-up)

#774 closed the raw KV read doors and left the log read doors as a follow-up (critique plan row CR1): scan,
subscribe and group subscribe read any stream under `consensus:read`, though append and compact have refused
`cn/`, `wiki/` and `reason/` since #549. They now refuse those streams 403 `protected_stream`, by the one table and
the one predicate the write doors use, before anything is read, watched, proposed or sent.

- Page touched: `dev/security.md` (a paragraph after the kv:read decision's; its **Open** line no longer names the
  log doors).
- Outside the wiki: `docs/guide/deprecations.md` §101, `docs/operations/rbac.md`, `docs/operations/companions.md`,
  `docs/operations/deployment.md`, `docs/guide/10-language-bridges.md`, the feature page `http-gateway`, both SDK
  READMEs, `CHANGELOG.md`, the critique plan's CR1 row.
- Code: `src/agent/http/authz.rs` (`OwnedStream`, `owned_log_stream`, `refuse_owned_stream_read`; the raw KV read
  refusal for a key under an owned stream names the stream's read), `overlay.rs` (the three handlers); tests
  `src/agent/http/log_read_tests.rs`. SDKs: `ProtectedStreamError` from the three read verbs.
- Enumeration: no MCP, A2A or companion door reads a stream by a caller-chosen name, and nothing shipped read an
  owned stream through the log routes.
- Stated, not changed: `GET /gateway/kv/keys?prefix=log/cn/` lists the entries' key names (the names decision of
  #774 left `log/` listed).
