## [2026-09-24] ingest | Boundary H implementation plan (proposed)

Up: [dev](../dev.md) · page: [security](../security.md) · records `docs/plans/boundary-h.md` (new, indexed in
`docs/plans/README.md`) · `docs/threat-model.md` §5 Boundary H.

**Finding.** Checking the code for the plan added four things to the threat model draft:
- a seventh gain: issuers are not bound to admitted identities (`IssuerId::new` takes any string), so one member
  can be many issuers;
- an in-force mitigation that had been missed: the WS-C audit sink;
- prerequisites P1 (issuer binding), P2 (a signed mandate grant) and K1–K3 (the knowledge layer's own deferred
  PRs);
- H7, the confined-fleet profile.

Among the prerequisites, `Mandate` is unsigned, and the knowledge layer's durability and transport PRs appear in no
plan.

**Change.** The plan sequences the work in four phases: prerequisites · evidence counting (H2, H5, H1) · authority
and history (H3, H4, H6) · proof (H7 and a decisive demonstration shaped like the Hugging Face incident). It lists
nine replayed negative cases and what it does not claim, and states that H5, the cohort declared at admission, is
the keystone.
