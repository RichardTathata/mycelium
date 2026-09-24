## [2026-09-24] ingest | knowledge store verifies on storage (Boundary H, M2 / K1)

Up: [dev](../dev.md) · record `docs/design/knowledge-validity.md` (new ADR; K1 adopted, K1b/K2/K3 proposed) · code
`src/knowledge/store.rs` · plan `docs/plans/boundary-h.md` (PR #379).

**Finding.**
- `KnowledgeStore::put` verified nothing, and records carried no signature.
- `KnowledgeRecord` derives `Deserialize`, so a wire record bypasses `new`: its id need not match its content, and
  a foreign retraction can be smuggled in.
- Heads carry a `seq` and refuse stale ones, but are unsigned despite the knowledge-layer ADR calling them signed.

**Change.**
- `SignedRecord` is the travelling form.
- `put_signed` checks integrity (id equals the content digest, plus `new`'s rules), then attributes the record
  through P1.
- Each record carries an `Attribution` snapshot, and its signature is retained for K1b.
- `PutRefusal` covers `IdMismatch`, `Malformed` and `Unverifiable`, counted by label.
- A revoked-key record is stored as history and flagged.
- Seven tests (`store::tests::k1`).

**Kept honest.** K1 is additive, not the breaking `put → Result` the plan listed: `put` has 72 call sites that
build trusted local records, so it stays and marks records `Unchecked`. The deviation is recorded in the ADR. No
unchecked record is yet excluded from counting; that is K1b.
