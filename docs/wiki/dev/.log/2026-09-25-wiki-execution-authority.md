## [2026-09-25] ingest | authority at execution at the wiki's git store (Boundary H A1 wiring)

Up: [dev](../dev.md) · page: [security](../security.md) · record `docs/design/authority-at-execution.md` §7 · code
`mycelium-wiki/src/execution.rs`, `mycelium-wiki/src/mandate_fence.rs` (`WriteAuthority`),
`mycelium-wiki/src/git_store.rs`, `mycelium-wiki/src/agent.rs` · lock-order row 44.

**Finding.** The mandate fence checks only that the appointment ref has not moved, which it does inside the git
transaction. An expired curator, a revoked one whose ref nobody had moved, or one cut off from its authority for
longer than the freshness bound all kept writing, and kept publishing commits made earlier.

**Change.**
- A data-plane seam, `WriteAuthority`, which `GitStore` asks on every commit attempt (just before `update-ref`) and
  every push attempt.
- `ExecutionGateAuthority` implements it over A1's `ExecutionGate` (feature `execution-authority`).
- The refusal is its own error, and the curator leaves the proposals queued instead of dropping them.

**Kept honest.** `FsStore` has no seam yet. The check-to-transaction distance is local subprocess time, and no bound on
it is claimed. The remote's half of the fence is unchanged.

**Found on the way.** A1's `RevocationView` kept only the newest checkpoint, so a later checkpoint that omitted a
revoked term reinstated it. Revocation was monotonic over time but not over checkpoints. Fixed in the view, with its
own unit gate. The fix covers the gateway too.
