## [2026-11-18] ingest | I7 PR 3a: `mycelium dev consent`, the developer's side for a software key

`dev` creates a software P-256 consent key (`SoftwareConsentKey`, `mycelium-core/src/consent_key.rs`), keeps it
`0600` in a `0700` directory under the home and never in a worktree, enrols under the certificate the CA issued over
its public half, and signs a grant decision from an answer typed at `/dev/tty` over a prompt rendered from the
gateway's record (`src/dev_consent.rs`). A bounded queue answers `busy`. So a consent decision is signed outside a
test fixture, and a node records it `tty`.

The durable points: **a signer checks its own certificate's grade before it signs** — `dev`'s software key signs only
under `tty` (`grade_not_this_keys`), so the device claims no more about its key than it does; `tty` is where the key
lives, not that a person typed (the tests sign through a pseudo-terminal); the signed decision does not name the tool
or the digest, so the prompt shows a record the addressed node may read differently (inventory row
`grant/{agent}/{request}`, I4a). Everything is behind the provisional grade OID: a shipped build refuses each verb
by name. Not built: the macOS shim (PR 3b), Windows, a `recording` signer. Behaviour and surface:
`docs/guide/deprecations.md` §121.

Pages: [security](../security.md) § client certificates; `docs/features/consent-certificates.md`;
`docs/design/replay-nondeterminism-inventory.md` (the key file and terminal, outside any recording).
