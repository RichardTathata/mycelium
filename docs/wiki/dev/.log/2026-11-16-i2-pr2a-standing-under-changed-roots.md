## [2026-11-16] ingest | CA succession, PR 2a: standing under changed roots (I2)

PR 1's row named PR 2 as one PR; it is split (plan §8, "I2 (PR 2a of 4)"): PR 2a, what a change of roots does to
what a node trusts, driven by `GossipAgent::change_roots`; PR 2b, the protocol that decides when (`ca-commit.pem`,
`tls commit`, the successor-signed record and its door, `succession/`, the hold, the profiles); PR 3 the re-issue.

- Kept anchors: an anchored key keeps its certificates (one per issuer, journaled `Anchor`), removed only when they
  fail the current roots; a node left with none keeps an empty entry and trusts nothing (fails closed, never legacy).
  Each fold runs under one roots generation; a merge that meets a change merges nothing and the fold re-runs.
- `legacy_closed` on `KeyStanding`, from the roots' generations alone (`caGeneration`, provisional OID
  `1.3.6.1.4.1.32473.2.1` under the documentation PEN; a shipped build refuses such a root by name; a release blocked).
- Client marks keep their certificate (`RaisedWithCert`); `current_mark` is the standing readers' read.
- Sessions re-judged at their next frame after a change of roots; handshake anchors tagged and cleared.
- One owner (`IdentityLedger::identity_owner`, a `tokio` mutex, lock-order row 82) over rotation and the change of roots.
- Pages touched: `dev/concurrency/lock-order.md` (row 82 and the `tokio::sync` exception), `dev/owner-signed-records.md`
  (the `sys/identity-cert/` and `sys/client-cert/` rows). Outside the wiki: the plan's I2 row and §8 row,
  `deprecations.md` §115, `CHANGELOG.md`, `features/node-identity.md`, `features/client-certificates.md`, the registry.
