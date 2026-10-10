## [2026-10-10] ingest | discovery is not an electorate — the consensus electorate decision recorded

- A recurring third-party review comment ("consensus is safe only with a fixed electorate … treat dynamic discovery
  and dynamic consensus membership as separate capabilities") answered in one place: the decision record
  `docs/design/consensus-electorate.md` (post-360 plan `docs/plans/post-360-hardening.md` §2, D1; rows P2, C1, C2, Φ).
  Position: separation is the design; the supported profile is a fixed electorate per decision; consensus is a
  Layer III protocol, never a service.
- Enforced today, cited by `file:line` in the record: `resolve_electorate` + the `MembershipIntent.min` floor,
  `ConsensusResult::NotAMember`, the prepare phase, the gateway's `governed_group` refusal, emergent membership
  deferring to governance. Residual stated: embedded `join_group` / `grp/` writes (Φ3), the opt-in governor moving a
  governed group, an intent evaporating, the §7 profile naming voters by node id (`declare_trust`), and no check that a
  safety-sensitive proposal uses a governed group (P2). C1 and C2 not built.
- Pages touched: `dev/architecture/runtime-invariants.md` (Layer III posture, new paragraph); `domain/theory/coordinator-trap.md` (its own `.log`). Outside the wiki:
  `philosophy.md` (row Φ), `threat-model.md` §7, guide 04 (new section *Discovery is not an electorate*), the FAQ,
  `what-is-proven.md`, the engineering deck (three qualifiers), CHANGELOG `[Unreleased]`.
