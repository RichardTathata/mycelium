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

**An external review, the same day, and what it changed.** Six findings, all accepted:
- out-of-order checkpoints could lose a revocation: fixed, since the revocation is recorded before the replay check;
- the gateway read `Hlc::current()`, which stands still on a quiet node and fails open: fixed in #405
  (`fix/preflight-reads-a-live-clock`, `Hlc::decision_now_ms()`), which another session had written the same day;
  this PR's copy of the fix was dropped;
- the *F* − 2*s* partition bound was stated as real time: corrected;
- the check-to-transaction window: I had written "not a gap", which was wrong, because normal latency is not a
  safety bound. It is now a limit pinned by a test, and closure plan C9;
- restart safety: closure plan C8;
- cancelling admitted work: closure plan C10.

The lesson worth keeping: **"it is only milliseconds" is a performance argument, not a safety one.** A bound that
depends on the process not pausing is not a bound.
