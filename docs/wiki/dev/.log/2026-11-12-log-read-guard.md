## [2026-11-12] ingest | the log read routes refuse component-owned streams (CR1's follow-up)

#774 closed the raw KV read doors and left the log read doors as a follow-up (critique plan row CR1): scan,
subscribe and group subscribe read any stream under `consensus:read`, though append and compact have refused
`cn/`, `wiki/` and `reason/` since #549. They now refuse those streams 403 `protected_stream`, by the one table and
the one predicate the write doors use, before anything is read, watched, proposed or sent.

- Pages touched: `dev/owner-signed-records.md` (the `log/wiki/…` row), `dev/security.md` (a paragraph after the kv:read decision's; its **Open** line no longer names the
  log doors).
- Outside the wiki: `docs/guide/deprecations.md` §101, `docs/operations/rbac.md`, `docs/operations/companions.md`,
  `docs/operations/deployment.md`, `docs/guide/10-language-bridges.md`, the feature page `http-gateway`, both SDK
  READMEs, `CHANGELOG.md`, the critique plan's CR1 row.
- Code: `src/agent/http/authz.rs` (`OwnedStream`, `owned_log_stream`, `refuse_owned_stream_read`; the raw KV read
  refusal for a key under an owned stream names the stream's read), `overlay.rs` (the three handlers); tests
  `src/agent/http/log_read_tests.rs`. SDKs: `ProtectedStreamError` from the three read verbs.
- Enumeration: no MCP, A2A or companion door reads a stream by a caller-chosen name, and nothing shipped read an
  owned stream through the log routes.
- After #777's review (the maintainer's decisions, the same day): the key names under `log/{owned}` and
  `clog/{owned}` are hidden from `GET /gateway/kv/keys`, derived from `OWNED_LOG_STREAMS`
  (`in_owned_stream_keyspace`); a `log/{name}/…` key in `src/lib.rs`'s ownership table must be a row or a stated
  application stream; `log/wiki/{group}/proposals` has an inventory row in `dev/owner-signed-records.md`
  (*Outside: no decision*, no reader yet).
- Stated, a follow-up PR: `GET /consensus/{slot}` returns a commitment's award and a group's claim under
  `consensus:read`.
