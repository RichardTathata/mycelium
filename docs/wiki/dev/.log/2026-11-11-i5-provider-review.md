## [2026-11-11] ingest | I5 PR 1, #759's review: one instant, one verifier, one window

`dev/security.md`: the exchanged token's clocks are read at the receive boundary's one instant (`verify_received` reads
`decision_now_ms` once), which the replay window prunes at too — a check and the memory of its result must share a
clock. The node has one OIDC verifier, shared by its gateway and its provider. The window is one per provider, keyed by
`(iss, jti)` across gateways, with an expiry queue. `dev/concurrency/lock-order.md`: row 17 gains `keys_now` and the
off-runtime rule; row 78 rewritten to the one window. The maintainer's decision: a hosted capability's invoke
(`cap.invoke/…`) needs an `ot`. Plan rev 0.12, `deprecations.md` §96 restated.
