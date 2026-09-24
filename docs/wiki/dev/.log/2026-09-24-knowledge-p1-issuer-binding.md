## [2026-09-24] ingest | knowledge issuer binding (Boundary H, M2 / P1)

Up: [dev](../dev.md) · record `docs/design/knowledge-issuer-binding.md` (new ADR; P1 adopted, P2 proposed) · code
`src/knowledge/issuer.rs`, `src/agent/knowledge_keys.rs` · plan `docs/plans/boundary-h.md` (PR #379) milestone M2.

**Finding.** `IssuerId::new` accepted any non-empty string, and `KnowledgeRecord::verify` took the key from its
caller, so one admitted member could be many issuers. Two further facts shaped the design:
- the existing `known_verifying_keys` *drops* revoked keys, which would make a revoked-key record unattributable
  rather than merely non-current;
- signed audit checkpoints already exist (`AuditCheckpoint`, `sys/audit-checkpoint/`), which the Boundary H plan's
  H4 should reuse rather than reinvent.

**Change.**
- `verify_issuer` has two admissible paths: member (`node:{id}`, the reader's retained keys) and configured
  external (`TrustedExternalIssuers`, `node:` refused).
- It has three outcomes: `Current`, `Revoked` (attributable history, no present standing) and `Unverifiable` with
  a reason.
- `knowledge_member_keys` keeps revoked keys in `retained` and reports them separately.
- `sign_knowledge_record` signs only as this node.
- Unit tests, plus a two-node `compliance` gate covering attribution, the refusal to sign as another node, the
  refusal of a foreign-key forgery, and revoke-then-`Revoked`.

**Kept honest.** The member path's strength rests on `require_identity_proofs` (default-off), and the store does not
yet verify on `put` (K1). Both are stated in the ADR and the changelog.
