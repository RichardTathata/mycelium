## [2026-11-22] ingest | Release assets attested and verified before they are attached (security-operations SO3)

The release workflow now attests the three tarballs, `SHA256SUMS`, the image index and both per-arch images
(`actions/attest` v4.2.2: SLSA v1 provenance, Sigstore keyless, Rekor) and verifies each attestation
(`scripts/verify-release-attestation.sh`) before it uploads anything or moves `latest`, which is now its own step,
by digest, after the image verifies. Permissions are per job (nothing at the top level; only `bundle` and `publish`
hold `id-token`/`attestations`), and every action in `release.yml` is pinned by commit SHA.
`scripts/check-release-workflow.py` (make check, CI lint job) holds that shape; its self-test plants twelve faults.

Durable knowledge from the build:
- **`actions/attest`, not `attest-build-provenance`:** since v4 the latter is a wrapper and its README points new
  workflows at the former; with no predicate input it generates SLSA build provenance. Storage records (and the
  `artifact-metadata` permission) apply only to organisation-owned repositories, so the image steps set
  `create-storage-record: false`.
- **A same-repository pull request can attest:** it gets an OIDC token, so the PR dry run attests and verifies the
  real files; a fork's (or Dependabot's) cannot, and the job says so. The dry run cannot reach the image half without
  pushing an image, so the image attestations, `--bundle-from-oci`, the Release attachment and the move of `latest`
  first run at a tag (`what-is-proven.md`, `binary-provenance`).
- **A dry run's attestation is harmless because the operator's check pins `--source-ref refs/tags/vNEW`:** checked by
  hand on #827's bundle — it verifies as `release.yml@refs/pull/827/merge` and is refused when pinned to a tag.
- **The verify script includes a check that must fail** (a ref that did not build the file), so a pin that stops
  biting fails the run.
- **cosign v3** reads GitHub's bundle with `verify-blob-attestation --bundle … --type https://slsa.dev/provenance/v1`
  and either the file or `--digest <bare hex> --digestAlg sha256`; it needs the public-good bundle (one with a Rekor
  entry), which a public repository's attestations are.
- **Rekor is a release dependency now:** the first dry run failed at the attest step with `error fetching tlog entry
  - (404) Not Found` and passed on the re-run (RELEASING § 8, *If verification fails*).
- The repository's other workflows are **not** SHA-pinned (only `release.yml` is, and only it holds `id-token`).

Pages: `.github/workflows/release.yml`, `RELEASING.md` § 7–8, `SECURITY.md`, `docs/features/published-node.md`,
`docs/guide/installation.md`, `docs/operations/deployment.md`, `docs/operations/what-is-proven.md`
(`binary-provenance`), `docs/plans/security-operations.md` (new), `docs/plans/README.md`,
`identity-and-lifecycle.md` § 4, `rust-ecosystem-watch.md` W1, `docs/wiki/dev/operations.md`.
