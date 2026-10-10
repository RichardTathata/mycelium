## [2026-10-10] ingest | the wasm host's doors (PR #586)

- Five fixes in `mycelium-wasm-host`: a guest's `mesh.emit` confined to `comp/{ns}/…` and never a protected
  RPC kind (P1); a `[hosts]` table without `trusted_publishers` refused unless `accept_unsigned = true`; a
  shadow-lane `tool/*` proposal no longer bridged as an MCP tool; the staging pull bounded by the entry's
  size hint and a stage ceiling, with tick back-off that does not count a missing holder; a memory limiter
  on every component store.
- The adversarial review's findings folded in: a CHANGELOG sentence corrected (a shadow blob's activation
  does not run), the librarian mirror bounded, the coop examples pass `protected_rpc_kinds`, a namespace
  with `/` emits nothing, the back-off's arithmetic.
- Pages touched: `dev/security.md` (new section, the two invariants); `docs/reference/unit-file.md`
  (`accept_unsigned`, what a component may emit).
