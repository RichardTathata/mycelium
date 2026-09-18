## [2026-09-18] ingest | item 4 PR 3 — the rights ledger

Up: [dev](../dev.md) · record `docs/design/adaptive-stability.md` §4 · code `src/control/ledger.rs`,
`src/agent/journal.rs` (ungated here), `Cargo.toml` (`sha2` unconditional).

### What landed

The ledger of allocated rights on the node-local journal: `Right` with five counted states, persist-then-apply,
release/expiry/revocation as the only exits, `admission.rejected` as a record, a fail-closed open, and the
bounded signed `RightsHead` whose bytes are `serde_fixint` — already the byte layer under the audit chain and
consensus signatures, so no hand-written codec.

### Three things worth carrying forward

1. **The gate that matters is two-sided, and the second side is the one that keeps the ledger useful.** *A
   right whose holder vanishes is not reissued* is enforced structurally — there is no method on the type that
   takes a peer set — and *a released, expired or revoked right is reissuable* is the twin. A ledger that
   passed only the first would be a ledger that never frees anything, which is the knowledge gate's
   refuse-everything failure in another form. Both are tests.
2. **Fail closed on an undecodable journal.** Starting with an empty view over a full file would readmit
   nothing and reissue everything — the exact double-issue the ledger exists to prevent, produced by the
   ledger itself. `open` returns `InvalidData` instead.
3. **The dead-code trap, third time this item, same lesson each time.** Ungating the journal left
   `Journal::path()` with only a gateway-gated caller. The fix was a real use — the ledger folds from the
   journal's own path, which is also the more honest source — not an `allow`. The pattern across PR 3a and PR
   3: *ungate a mechanism in the same change as its first ungated user, and make every method live in that
   user or drop it.*

### Not done here

Reserve → act → reconcile against a right's units, and publishing the head into `rights/head/{holder}` — PR 4.
The ledger is `&mut self`, single-owner; how it is shared is PR 4's question and it adds no lock row.

### Pages touched

- [history.md](../history.md) — a PR 3 paragraph under the item 4 section.
- The ADR's §9 marks PR 3 landed.
