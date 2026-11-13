## [2026-11-12] ingest | the consensus routes refuse component-owned slots, to read and to propose

#777 left `GET /consensus/{slot}` stated open: it answered any slot under `consensus:read`. The survey that
followed found the write-side sibling — `POST /gateway/consensus/cross_group_propose` takes any slot name — and
the maintainer decided one table for both, `lock/` and `leader/` staying readable. Both routes now refuse by
`SLOT_FAMILIES`, 403 `protected_slot`, before the store is read or anything proposed.

- Pages touched: `dev/security.md` (a paragraph after the log read doors'; the earlier paragraph's open line now
  points at it), `dev/operations.md` (the `/consensus/{*slot}` row).
- Outside the wiki: `docs/guide/deprecations.md` §102 (and §99's table row, §101's *Not changed*),
  `docs/operations/rbac.md`, `observability.md`, `diagnostics.md`, `metrics.md`, `companions.md`, `deployment.md`,
  `docs/guide/04-consensus.md`, `10-language-bridges.md`, the feature pages `http-gateway` and `contract-net`,
  both SDK READMEs, `CHANGELOG.md`, the critique plan (CR1's row; a new row CR24 for the slot names).
- Code: `src/agent/http/authz.rs` (`SlotFamily`, `SLOT_FAMILIES`, `slot_family`, `refuse_owned_slot_read`,
  `refuse_owned_slot_proposal`; the raw KV read refusal for a `consensus/` key says which slots the route reads),
  `node.rs` and `overlay.rs` (the two handlers), `src/agent/electorate.rs` (`is_fleet_exclusive_slot`'s families as
  data, so the table is held against them). Tests `src/agent/http/slot_guard_tests.rs`,
  `mycelium-commitment/tests/award_slot_route.rs`. SDKs: `ProtectedSlotError` from `cross_group_propose` /
  `crossGroupPropose`.
- Shown first on the unguarded routes: the read answered an owned slot's value 200; the cross-group route decided
  `cn/…/award`, `capauthz/…`, `ae-seat/…`, `clog/…/claim`, `lock/x` and `consistent/{key}`; `electorate/g/…` and
  `leader/g` were already refused by the electorate door (409), and are guarded at the route anyway.
- Enumeration: no MCP tool, A2A skill, SDK verb or companion route reads or proposes on a slot by a caller-chosen
  name besides these two; nothing shipped read an owned slot through the route (the runbooks read `lock/{name}`,
  unchanged) or proposed on one through the cross-group route.
- Stated, not closed: the slot names in `GET /gateway/kv/keys?prefix=consensus/` (CR24); the signal SSE doors
  stream `consensus.propose` / `consensus.commit` frames to a `mesh:read` tap, slot and value included (probed).
