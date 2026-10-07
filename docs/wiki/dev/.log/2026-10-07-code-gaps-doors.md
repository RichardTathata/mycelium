## [2026-10-07] ingest | The recorded code gaps, and every door to them (#544)

- The timing door refuses what no node would apply, an intent that governs nothing, and loose bodies; its
  siblings (tuning, membership) parse as strictly; timing's publishes are audited.
- Rule 1 in practice: each review found another door to the same invariant — the raw KV routes, then
  `kv/quorum`, then `overlay/consistent/set`, then every other substrate key. The four doors now refuse `sys/`
  and `consensus/` (allow-listing `sys/topology-override/`). `kv:write` as a data-plane superuser over other
  route-owned namespaces is recorded in `rbac.md`, not decided.
- Also: cooling-off decides a visible term first; one `with_egress` shape; `/signals` data shape;
  `mycelium-py` 0.2.6 `node_id`.
- Pages: `dev/security.md`.
