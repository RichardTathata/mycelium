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

## [2026-10-12] ingest | row Q rev 3: standing from certificate records (after #616's review and two design reviews)

- #616's first fix ("any held key may revoke any other, for ever") was reviewed: it turned identity poisoning, first
  sighting after a restart and any leaked retired key into permanent silencing, and a restart bypassed the chaining fix.
  The decision record `docs/design/revocation-authority.md` went through rev 1 → rev 2 (the reviewer's minimal sound set)
  → rev 3 (three named simplifications, then succession links cut because every successor in the tree is certified).
- Built: certificate records (`sys/identity-cert/`), certificates naming their node by URI with issuance in the serial
  (`mycelium-core/src/tls.rs`), `KeyStanding` replacing `RevokedKeys`, the fold in `src/agent/revocation.rs` (anchored ⇒
  trusted = anchored, successor-only by issuance, `(P, S)` self-contained v2 records behind the unchanged v1 bytes, a
  bounded journal failing closed, owner republish, the tripwire on the new prefixes), consensus's raw fallback removed,
  agentfacts on `trusted_identity_keys`, guarantee `id.revocation_authority`.
- Pages touched: `dev/security.md` § WS5. Outside the wiki: the design record, threat model, runbook
  (`docs/operations/cert-rotation.md`), `what-is-proven.md`, `deprecations.md` §39, CHANGELOG, the plan.

## [2026-10-13] ingest | row Q: the final review's round (named fixes only)

- The final review of 38da0472 found the node's own audit genesis re-sealed after re-anchoring (live trust used for
  history) and a re-issue un-revoking a key (max issuance, re-judged from scratch). Fixed with a verify-only history set
  (`CoreCtx::history_keys`), first-issuance ranking, counted-stays-counted, a start refusal for a self-revoked key, the
  fold's caps before any check (≤ 2 certificates per record, a hashed CA-check cache), a rotation issuance floor, and
  three low items; each test seen failing on 38da0472. Pages: the design record §9, §39, CHANGELOG, the plan.
