## [2026-10-16] ingest | the mTLS handshake refuses a revoked key; the mesh resumes no TLS session

**What changed:** the mesh's two certificate verifiers (`RemovalAwareClientVerifier`, `RemovalAwareServerVerifier`,
`mycelium-core/src/tls.rs`) hold `CoreCtx::key_standing` and refuse a certificate whose key is **anchored** for its
owner and revoked by it — post-360 plan row Q's recorded follow-up. The owner is the node the certificate names
(`mycelium://{node}`), or for a URI-less certificate the node holding the key anchored. Anchored only: N may revoke a
key without standing for N, so reading those would let one member refuse another's handshakes, and a *provisional*
revocation (the key's certificate record not yet here) would cut a freshly rotated node off — the review's findings
1 and 3 moved the first cut (named node, any revoked key) to this rule.

**The transferable part:** the first test passed its plant handshake and then *also* passed the post-revocation one —
the second handshake resumed the first's TLS 1.3 session, and a resumed session runs no certificate verifier. The
mesh's rustls configs kept the defaults (client session cache, server tickets), so the removal check (closure plan C5,
2.15.0) was skippable the same way. Both sides now disable resumption. Any test of a handshake refusal needs a
handshake *before* the change in the same config, or it cannot see this.

**Enumeration (rule 1):** `git grep -n "ClientCertVerifier\|ServerCertVerifier\|ServerConfig::builder\|ClientConfig::builder"`
over `mycelium-core/src src`: the mesh server and client configs (`build_rustls_configs`, used by `load_or_generate`
and `generate_rotation_after`); the gateway server config (no client certificates — nothing to re-check); the
federation pinned client (`src/federation/pinning.rs`, SPKI pinning of a partner — resumption can only reach the
server holding the session). `verify_member_cert` (certificate records) deliberately does not consult revocation: the
fold needs revoked keys' certificates.

**Tests:** `a_revoked_keys_certificate_is_refused_at_the_handshake_both_ways`,
`a_removed_member_cannot_reconnect_on_a_resumed_session`, `a_revoked_key_is_refused_behind_a_certificate_that_names_no_node`
(`mycelium-core/src/tls.rs`),
`a_revoked_keys_certificate_is_refused_at_a_peers_handshake` (`src/lib_tests.rs`) — each seen failing first.

**Pages touched:** [security](../security.md); `docs/design/revocation-authority.md`, `docs/design/member-removal.md`,
`docs/operations/what-is-proven.md`, `docs/threat-model.md`, `docs/guide/deprecations.md` §39 and §44, the post-360
plan (row Q's block and §3.6).
