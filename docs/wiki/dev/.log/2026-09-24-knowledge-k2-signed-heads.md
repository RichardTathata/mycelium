## [2026-09-24] ingest | signed stream heads, verified ancestry, checkpoint contract (Boundary H, M2 / K2)

Up: [dev](../dev.md) · record `docs/design/knowledge-validity.md` §3 (K2 adopted) · code `src/knowledge/heads.rs`,
`src/knowledge/store.rs`, `src/knowledge/issuer.rs` · lock-order row 41 · plan `docs/plans/boundary-h.md` (PR #379).

**Finding.** Heads were unsigned, despite the knowledge-layer ADR's "signed heads", and `advance_head` accepted any
higher `seq`. A reader holding 10 would take 12 from a branch that diverged at 9.

**Change.**
- `Head.prev`, tagged canonical bytes and `digest`; `SignedHead` is the travelling form.
- `HeadCheckpoints::offer` authenticates through P1 (`verify_signed_by`, newly factored out), then advances only
  through a verified `prev` chain.
- Outcomes are `ContinuityUnavailable`, `ForkedStream` (forks kept, never resolved), `StaleHead`, `AlreadyHeld`
  and `SignedUnderRevokedKey`.
- `CheckpointStore` is a contract: load on open, **refuse unreadable state**, persist before reporting.
- `MemoryCheckpointStore` uses one leaf lock (row 41).
- Eleven tests, including the reviewer's diverged-at-9 case and restart.

**Kept honest.** `MemoryCheckpointStore` is not durable. Real durability needs the filesystem seam, and the seam
gate (`scripts/check-sim-seams.sh`) passes because this PR adds no filesystem access. It arrives with K3. Forks are
in memory only. `advance_head` stays, for local use, and is documented as such.
