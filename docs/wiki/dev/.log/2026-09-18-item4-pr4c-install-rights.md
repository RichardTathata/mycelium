## [2026-09-18] ingest | item 4 PR 4c — the provisioner against the rights ledger

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §4, §9 row 4c · code
`mycelium-wasm-host/src/provisioner.rs` (`InstallRights`, `with_install_rights`, `admit_install`,
`verify_published_head`), `src/control/ledger.rs` (`PublishedRightsHead`).

### What landed

The first governor to consume a right the ledger accounts for: the provisioner refuses an install when the node
holds too few units, records the refusal, and publishes its signed head. A companion crate on the public API,
which is the composability proof the ledger needed.

### Three things worth carrying forward

1. **The admission path is synchronous, so the record of a refusal is asynchronous.** `provision_round` is a
   plain `&mut self` pass; the ledger's `admit` fsyncs. Blocking the round on a journal write would have made
   every round's latency a disk latency. The refusal itself is decided from the ledger's *view* under
   `try_lock`, immediately; the `admission.rejected` record is appended by a spawned task. What that does not
   promise: a process that dies between the two loses the record, not a right — a lost tripwire, never a
   double-issue. The test polls the ledger for the record structurally rather than assuming it.
2. **A busy ledger is a refusal, not a wait.** `try_lock` failing means another task holds the ledger — a
   refusal being recorded, or the head being published. Refusing this round and re-judging next round is the
   conservative direction; the ledger's own `admit` re-judges against the live view when it records.
3. **Unsigned means unproven, and the reader must be the one who says so.** Without a signing key the head
   still publishes — an operator who never gave a key still gets the claim in the medium — but
   `verify_published_head` returns `false` for it rather than `true`-by-absence. `RightsHead::verify` is
   `tls`-gated in `mycelium`; the wasm-host verifies with its own `ed25519_dalek`, which it already uses for
   catalog provenance. Same key type, no new feature.

### Decided, and what is not modelled

The provisioner never allocates to itself: the ledger is the allocator's record, and an unallocated node is
refused every install — visibly, which is the point. Per-install rights and the right's own state transitions
(`installing → serving`) are not modelled; the units consumed are the provisioner's count of reserved-plus-live
artifacts, checked against the units held. Revocation or expiry of a right while an install is live is not
shown: the next round refuses, the live install is not torn down — a release is the holder's act.

### Pages touched

- [history.md](../history.md) — the item 4 PR 4c section.
- [concurrency/lock-order.md](../concurrency/lock-order.md) — row 37.
- The ADR's §9 marks 4c landed with the decisions; CHANGELOG.
