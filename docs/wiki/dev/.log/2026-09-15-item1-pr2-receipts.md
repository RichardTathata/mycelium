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

**External review before merge (2026-09-15), four findings, all taken — every one an instance of the
same failure: a receipt claiming more than the operation established.** (1) *`Buffered` could
acknowledge a record the WAL dropped:* in `Async`/`Os`, `append` is a `try_send` returning `Ok`
regardless, so the receipt claimed the page cache for bytes that might never have left the process.
The path now uses a new `append_acked`, which awaits the writer's acknowledgement of the *write*
(the protocol already carried the ack channel; nothing new was needed but the honesty), and a dead
writer reads `Failed` in every mode. (2) *`Superseded` conflated three outcomes:* an idempotent
retry and a capacity refusal both reported that a newer value had won, when in one case the operation
*is* the current value and in the other the key may be absent. `AlreadyCurrent` and `Refused` were
added, and only the losing CAS branch can tell the first two apart, so the distinction is captured
inside the compute closure. (3) *Attempt identities collided:* deriving `#n+1` from the prior receipt
made two retries of the same receipt identical — exactly the case that arises when a response is lost
or a replacement worker inherits a receipt. Identities are now fresh per dispatch, with `*_as` verbs
for callers who number their own. (4) *The content hash was not portable:* seeded `ahash` is not an
interchange format by its own documentation, so the same unchanged operation could hash differently on
another build and raise a false `Conflict`. It is now FNV-1a/64 over a specified canonical encoding,
pinned by golden vectors **verified against an independent implementation** rather than captured from
our own output.

**The lesson.** Three of the four were the reverse of the `Buffered` finding I had been pleased to
catch myself: I had noticed one place where the vocabulary claimed too much, and missed three others
of exactly the same shape one layer away. A receipt is only as honest as the weakest translation
between the operation and the word.

**The test worth keeping.** `a_late_retry_is_superseded_and_does_not_clobber_the_newer_value`: write,
let something newer take the key, then retry. The retry is `Superseded` and the newer value survives —
which is precisely why D11 forbids ticking a fresh HLC on a retry, executable rather than argued.
