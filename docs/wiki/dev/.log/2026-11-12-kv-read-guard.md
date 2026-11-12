## [2026-11-12] ingest | the raw KV read routes refuse owned namespaces (the kv:read decision)

The raw KV read doors had no guard where the write doors have had one since #549: `kv:read` read every owned
namespace's values, and `consensus:read` did through `consistent/get`'s raw-key fallback. Decided by the maintainer
2026-11-12: values refused for every owned key; key names hidden for seven namespaces by a `names_hidden` flag on the
rows of the one table (`OWNED_KV_PREFIXES`), listed for the rest; the log read doors left as a follow-up.

- Page touched: `dev/security.md` (the paragraph after the kv:write decision).
- Outside the wiki: `docs/guide/deprecations.md` §99, `docs/operations/rbac.md`, `docs/operations/diagnostics.md`,
  the feature pages `http-gateway`, `ops-console`, `recurring-work`, `grants`, both SDK READMEs, `CHANGELOG.md`.
- Code: `src/agent/http/authz.rs` (`OwnedPrefix`, `refuse_protected_key_read`, `refuse_hidden_prefix`,
  `raw_kv_name_hidden`), `kv.rs`, `overlay.rs`, `schedules.rs` (`GET /gateway/schedules/last`), `govern.rs`
  (`topology_overrides`); tests `src/agent/http/kv_read_tests.rs`.
