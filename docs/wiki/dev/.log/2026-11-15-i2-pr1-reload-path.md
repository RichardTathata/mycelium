## [2026-11-15] ingest | CA succession, PR 1: one root in `ca_cert_pem`, a named fleet CA, the reload path (I2)

I2's row defines no PR split; this PR states one (plan §8, "I2 (PR 1 of 3)"): PR 1 the root set, the subject and the
reload path; PR 2 the succession protocol (record, phases, `succession/`, R1, R4, M4); PR 3 the re-issue (R2, R3).

- `NodeTls`'s chain checks are swappable; `reroot_material` (current key, a set of roots, no CA key); a roots
  generation bumped after the swap, read before the verifier; the two certificate verdict caches tagged with it.
- `fleet_server_verifier()` is one delegating verifier, so the bulk fetch's client follows a change of roots.
- `ca_cert_pem` with two certificates refused by name (`deprecations.md` §111); `tls ca` names fleet and generation;
  the issuer name is the CA certificate's subject (the chain is found by name, not by the key identifier).
- Finding for the maintainer: rcgen (0.13 and 0.14) writes OID arcs as `u64`, so the row's `2.25` UUID OIDs for
  `caGeneration`/`firstIssuedMs` (and I7's grade) cannot be written as planned.
- Pages touched: `dev/security.md` (identity paragraph), `dev/owner-signed-records.md` (the `sys/identity-cert/` and
  `sys/client-cert/` rows: re-judged on a change of roots). Outside the wiki: the plan's I2 note and §8 row,
  `deprecations.md` §111, `CHANGELOG.md`, `operations/cert-rotation.md`, `operations/what-is-proven.md` (two rows),
  `features/node-identity.md`.
