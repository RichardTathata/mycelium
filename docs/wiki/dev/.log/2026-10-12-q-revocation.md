## [2026-10-12] ingest | revocation reaches every verifier, per node, revoked only by a holder (row Q)

- The post-360 plan's row Q. Before it, revocation (`sys/revocation/`) was consulted by consensus,
  `known_verifying_keys` and the knowledge member keys only; the KV `SignedData` arm read `peer_keys` raw, and
  identity chaining and the unsigned identity exemption trusted any held key, so a key revoked after a compromise
  still signed its node's KV writes and chained new keys in (#611's review, reproduced on 7c967ba3). Now one answer —
  `CoreCtx::trusted_keys` / `retain_trusted`, over `CoreCtx::revoked_keys` (per node, monotonic, papaya) — is read by
  every verifier, and the revocation module is compiled under `tls` (validated in every TLS build, issued under
  `compliance`). The identity watcher re-validates revocations on any `sys/identity*` or `sys/revocation/` change and
  after the startup prewarm.
- Who may revoke, the other half: the old rule took the revoker's "current key" from the raw `sys/identity/{N}` value
  any member can write, and kept one fleet-wide set — so a member could strip another's key fleet-wide two ways. A
  revocation now counts only when signed by a key the reader holds for N, naming a key it holds for N, and removes it
  from N's set only. A node cannot revoke the key it signs with. Contested identities fail closed (both revocations
  count); the mTLS handshake does not consult revocation (stated, not built).
- Pages touched: `dev/security.md` § WS5 (the revocation paragraph). Outside the wiki: `src/agent/revocation.rs`
  module doc, `docs/threat-model.md`, `docs/operations/what-is-proven.md`, `docs/guide/deprecations.md` §39,
  `CHANGELOG.md`, `docs/plans/post-360-hardening.md` (row Q, §3.5, §3.6).
