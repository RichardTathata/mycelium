## [2026-10-02] ingest | I1's audit: three more settings a build could not enforce, refused at start

**What:** the first increment of `docs/plans/guarantees-and-rule-catalogue.md` ran as a read-only sweep (plan
§8). Fixed in one PR: `[oidc]` dropped at parse time in a non-`compliance` build (gateway open); `[tls]` and
`[gateway_tls]` accepted and ignored in a non-`tls` build (plaintext); `/a2a`'s warning predicate wrong
(required *no bearer*, which never gated the route) and attach-order dependent, now a start-time fact; the
confinement report saying `Set` for two settings inert without `tls`. Tests seen failing first for the two
refusals. Open from the same audit, each named in the plan's table: the audit sink that seals nothing without
`config.tls`, the egress report's overclaim, the at-rest cipher attached late, replay-failure fail-open.

**Durable knowledge:** two shapes make a setting a lie — a `#[cfg]`'d field under a serde that ignores unknown
keys (it vanishes), and an all-builds field whose consumers are `#[cfg]`'d (it is kept and ignored). The fix for
the first is a parse-everywhere placeholder (`OidcNotInBuild`) so there is something to refuse.
