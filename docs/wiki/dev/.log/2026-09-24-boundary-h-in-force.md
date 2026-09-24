## [2026-09-24] ingest | Boundary H: eight mitigations in force; plan adopted (rev 0.5)

Up: [dev](../dev.md) · page: [security](../security.md) · records `docs/threat-model.md` §5 Boundary H ·
`docs/plans/boundary-h.md` (rev 0.5, §16 delivery record).

**Change.**
- Threat model Boundary H: three gains are marked mitigated (many issuers from one member, manufactured support,
  jammed verdicts).
- A new *in force* block lists H2, P1, K1, K1b, K2, K3a, H5 and H1, each with its PR, gate and stated limit.
- *Proposed* now holds only P2, K3b, K3c, H3, H4, H6, H7, A1 and the consensus boundary.
- The "Authority lapses" mitigation no longer implies that A1 is built.
- Plan: adopted, rev 0.5, with a §16 delivery record of commits, gates and departures. The departures are K1
  additive, the K1b retraction defect, the K3 split, H1's concrete invalidation and moved caps, H4's reuse, and H5's
  graph fix.
