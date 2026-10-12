## [2026-10-12] ingest | post-360 row F: wire and crypto, one MINOR with a window

- **KV `SignedData` is domain-tagged** (`mycelium.kv/update/1`, `framing::kv_signing_message`); the bare 2.32 form
  is accepted for one release and counted (`kv_untagged_signatures`).
- **A TLS node can refuse an unsigned `Data` frame** (`kv_require_signed_frames`, default off this release:
  applied and counted). The enumeration that mattered: twenty-three agent write sites built bare `Data` frames
  and never signed under `[tls]` — a refusal shipped without rerouting them would have dropped a fleet's own
  membership, mailbox and gateway writes. They go through `framing::local_kv_wire_msg`;
  `scripts/check-signed-dispatch.sh` keeps a new one out. Anti-entropy is the stated residual.
- **The certificate key is read structurally** (`tls::ed25519_key_from_cert_der`, rustls-webpki): the byte-scan's
  "the input is CA-validated" premise covered the issuer, not the bytes, and every node held the CA key before
  `secure-single-domain` rev 2.
- **§23 (2.32.0's bare identity/consensus window) closed**: the bare forms refused, `SignatureForm` and both
  counters removed. "#587's window" did not exist — #587 accepted nothing for one release.
- Pages: `security.md` (the identity-proof paragraph, row F's other two, the SPKI lesson), `deprecations.md` §23/§35,
  threat model Boundary A, `what-is-proven.md`, `configuration.md`, the plan's §3.5 F block.
- Lesson: "the input is validated" has to say by whom — and whether the validator can be the adversary.
