## [2026-09-15] ingest | item 1 PR 2 — identities, typed receipts, the failure vocabulary

Up: [dev](../dev.md) · record `docs/design/contracts-receipts.md` §2–§4, §8 · plan §3 (PR 2), D11, D24 ·
code `mycelium-core/src/receipt.rs`, `ops.rs`, `kv_handle.rs`, `src/agent/consensus_handle.rs`.

**What shipped.** The contract's vocabulary as types, and the first two rungs actually returned.
`OperationId` (caller-minted, stable across retries; a *correlation* identity, never authority) and
`AttemptId`; `LocalApplication`, `LocalDurability`, `ReplicaSync` and `DestinationCommit`;
`ReceiptError` and `CommitError` with **no variant meaning "nothing happened"**. `set_with_receipt`
returns what it established and nothing above it; `retry_with_receipt` re-submits with the original
stamp and refuses same-identity-different-content; `{cluster,group}_propose_receipt` separate
cluster agreement from local durability and read a timeout as `DeliveryUnknown`.

**The gap the implementation found in the record.** `LocalDurability` had three states because the ADR
assumed every write is either forced to disk or failed. That holds only under `SyncMode::Flush` or an
explicit `append_sync`; under `Async`/`Os` a successful append sits in the OS page cache — surviving a
process crash, lost to a power failure. `OnDisk` there would have been an overclaim and `Failed` a
denial of a write that is very likely recoverable, so a fourth state, **`Buffered`**, says what is
true. Item 6's storage model had already drawn exactly this line between process memory, page cache
and durable storage (its own review correction, a day earlier); the receipt vocabulary had not caught
up. Two records, one boundary — worth remembering when a contract and a harness describe the same
machine.

**Decisions taken here.** (1) *New verbs, not new fields* (D24, as corrected after review): the old
`persisted: bool` and `ConsensusResult::Committed`'s shape are untouched, because growing a
non-`#[non_exhaustive]` variant breaks every exhaustive destructure. (2) *Conflict detection is local*:
the receipt carries a stable content hash, so a retry compares against its own prior receipt and needs
no registry — and the hash is explicitly a divergence detector, **not** a security binding, unlike the
AE envelope's `sha256`. (3) *Every receipt type is `#[non_exhaustive]` with public constructors* — the
AE seam's review showed what the attribute costs without them. (4) *Attempt numbers derive from the
prior attempt id*, so the handle holds no per-operation state.

**The test worth keeping.** `a_late_retry_is_superseded_and_does_not_clobber_the_newer_value`: write,
let something newer take the key, then retry. The retry is `Superseded` and the newer value survives —
which is precisely why D11 forbids ticking a fresh HLC on a retry, executable rather than argued.
