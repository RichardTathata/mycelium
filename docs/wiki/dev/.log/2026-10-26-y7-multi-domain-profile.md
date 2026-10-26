## [2026-10-26] ingest | Y7: `secure-multi-domain` and the `fed.*` family

Ingest of adopter-surface row Y7 (`docs/plans/adopter-surface.md` §6, evidence §8).

- **What landed:** ten `fed.*` guarantees in `core_guarantees()` (`src/agent/guarantee.rs`), resolved from the attached
  `FederationEdge` and `FederationClient`s through three role facts (`fed_edge_role`, `fed_client_role`, `fed_role`);
  the profile `SECURE_MULTI_DOMAIN` rev 1 = `SECURE_SINGLE_DOMAIN` rev 5 + seven node facts. Page updated:
  `dev/operations.md` (the profiles line).
- **Decided in the row:** a seventh node fact beyond the plan's six, `fed.domain_enforced` (`domain_profile =
  enforced`; agreed with the maintainer 2026-10-26, D25 rev 0.13), because it is the federation posture a configuration alone decides; a node facing no partner reads the
  side-specific facts `not_applicable` (G10), and `GuaranteeReport::unresolved` now lists only external
  prerequisites that apply.
- **Found on the way:** `with_federation_clients` did not count a post-`start()` attachment (G13); fixed.
- **Stated, not built:** the two-mesh Docker suite under the profile; `doctor` seeing code attachments.
