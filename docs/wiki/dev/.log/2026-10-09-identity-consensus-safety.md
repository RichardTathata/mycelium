# 2026-10-09 — three consensus and identity safety fixes (P1)

- **Domain-tagged signatures.** The identity proof (`mycelium.identity/proof/1`) and the consensus payload
  (`mycelium.consensus/msg/1`) are signed under tags; a victim's bare-signed `PrepareAck` carrying a proposer-chosen
  value could be published as the victim's identity record and merged. One-release allowance for the bare form,
  identity only with `require_identity_proofs` off (deprecations §23). Pages: `security.md` (new section),
  `docs/threat-model.md` Boundary A, `docs/design/identity-authentication.md` amendment.
- **The acceptor's record and the decided floor reach the WAL** (fsynced via `persist_sync`) before the ack, vote
  or proposal leaves; an unrecorded one is not answered (`Timeout { reason = unrecorded }` for a proposer's own).
  Crash modelled as R2's. Page: `architecture/runtime-invariants.md`.
- **A proposer must be in the roster it counts itself toward**: `ConsensusResult::NotAMember` at the engine's door,
  `CommitError`/`ConsistencyError::NotAMember`, 403 `not_a_member` at the gateway (deprecations §24). Page:
  `architecture/runtime-invariants.md`.
- Each seen failing first: `a_signed_consensus_answer_is_not_an_identity_proof`,
  `an_acceptors_record_survives_a_crash_without_a_snapshot`,
  `a_non_member_cannot_propose_to_a_group_on_either_surface`. Wire v12 unchanged.
