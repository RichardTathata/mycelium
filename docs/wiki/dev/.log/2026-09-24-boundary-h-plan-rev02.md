## [2026-09-24] ingest | Boundary H plan rev 0.2, after an external design review

Up: [dev](../dev.md) · page: [security](../security.md) · records `docs/plans/boundary-h.md` (rev 0.2) ·
`docs/threat-model.md` §5 Boundary H.

**Finding.** An external design review found four blocking over-claims in rev 0.1:
- an in-pod sidecar cannot separate agent egress from gateway egress, because a pod shares one network namespace;
- a connection the network blocks produces no gateway evidence;
- a witness-signed hash proves only the witness's assertion;
- "non-renewal halts action" conflated stopping admissions with stopping admitted work.

It also found three gaps: H5 had no cohort lifecycle and confused control with origin; K1–K3 lacked
present-validity and rollback rules; and H depended on consensus paths still under repair.

Code checks confirmed the premises:
- `ActionEnvelope::mandate` is an `Option`, and the binding is an assessment made at admission;
- `ReaderPolicy::group_of` resolves by first match;
- #374 is merged and the S12 election failure is still open;
- AE2's strength depends on each resource's atomicity tier.

**Change.** The plan is restructured into three separately claimed deliverables: trustworthy evidence counting,
portable authority with defensible audit proofs, and an enforced confinement profile. It adds nine governing
distinctions, dependency milestones in place of release numbers, and a consensus acceptance gate. Other changes:
- new items A1 (authority at execution) and K1b (present eligibility);
- H4 rebuilt on source-signed checkpoints;
- H5 resolved as order-independent connected components with a merge-never-split fallback;
- H7 on separate pods with its own ADR;
- every demonstration step and negative case labelled replay or deployment evidence;
- an explicit compatibility table;
- a review-disposition table.

The threat model's H text was narrowed to match, including the halt.
