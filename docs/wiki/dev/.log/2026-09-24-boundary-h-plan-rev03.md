## [2026-09-24] ingest | Boundary H plan rev 0.3, after the second review round

Up: [dev](../dev.md) · page: [security](../security.md) · records `docs/plans/boundary-h.md` (rev 0.3) ·
`docs/threat-model.md` §5 Boundary H.

**Finding.** The reviewer accepted rev 0.2's architecture and asked for six bounded amendments:
- H4 overclaimed: a checkpoint cannot detect a rewrite of the unwitnessed suffix after it;
- cohort expiry could split a known dependence into two apparently independent groups;
- K2 compared sequence numbers where it should have verified ancestry;
- revocation freshness lacked an authoritative mechanism;
- "substantiated facts" conflated verification with trust;
- T_drain omitted the cancellation delay.

**Change.**
- H4 is restated as conflict with retained signed evidence, with tests for covered history and an unwitnessed
  suffix.
- H5 dependence is sticky: staleness marks, never voids. An optional `stale = Exclude`, and historical grouping as
  the union of grouping at issue and now.
- K2 advances only by a verified ancestry chain, with `ContinuityUnavailable` and durable checkpoints.
- A1 gets authority-signed revocation checkpoints, issued even when empty, aged by `issued_at_ms`, protected
  against replay by a retained `seq`, and with clock skew stated.
- H1 separates mechanically verified invalidation from a policy-decisive observer, and reports evidence-citing
  outside challenges as `evidenced_unadmitted`.
- T_drain now includes cancellation latency and confirmation.
- Three governing distinctions were added, the compatibility table was extended, and a round-2 disposition table
  was added.

The threat model's H4 and H5 text was narrowed to match.
