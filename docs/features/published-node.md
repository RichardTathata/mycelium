# Published node

## What it is

Every `v*` tag publishes the [published node](../guide/00-concepts.md#published-node): the `mycelium`,
`mycelium-stem` and `mycelium-artifact` binaries for Linux x86_64, Linux aarch64 and macOS aarch64, a `SHA256SUMS`,
and a multi-arch image at `ghcr.io/richardtathata/mycelium`. A Python or TypeScript developer starts a node without a
Rust toolchain. The Linux tarballs are smoked in a clean container before anything is attached, and the GitHub Release
is created from the annotated tag's message if the runbook's step 8 has not run, so the Releases page cannot fall
behind the tags again. **Every tarball, `SHA256SUMS`, the image index and both per-architecture images carry build
provenance** — a GitHub artifact attestation (SLSA v1, signed keyless through Sigstore, logged in Rekor) naming the
repository, `release.yml`, the tag and the commit that built it — and the workflow verifies each attestation before
it attaches the file or moves `latest` ([plan row SO3](../plans/security-operations.md#so3)). The first tag cut after
SO3 is the first that carries them; v2.40.1 and earlier carry sums only.

## When to use it, and when not

- **Use it when** you run the node from an SDK, a container or a host without a Rust toolchain, with the published
  feature set (`cli,tls,compliance,a2a,llm,metrics` for the node; `stem,gateway,llm,object_store,compliance` for the stem).
- **Not when** you need another feature set, a platform not listed (Windows, Linux older than glibc 2.36), a
  reproducible build, or an SBOM: the build from a pinned tag is the path
  ([installation § Pin the source](../guide/installation.md#pin-the-source)). Provenance says which workflow, commit
  and tag built a file; it does not say a rebuild would produce the same bytes.

## Turn it on

This feature is turned on in code.

- [`version_line`](../../src/main.rs) — the one line `mycelium --version` prints, which the workflow's clean-container
  smoke compares to the tag.
- The workflow itself is [`.github/workflows/release.yml`](../../.github/workflows/release.yml) and the image is
  [`docker/Dockerfile.node`](../../docker/Dockerfile.node); a `workflow_dispatch` run is the dry run, and so is a
  pull request that touches the publishing path — both attest and verify the tarballs and `SHA256SUMS`.
- [`scripts/verify-release-attestation.sh`](../../scripts/verify-release-attestation.sh) — the workflow's verification,
  run after every attestation and before every upload; [`scripts/check-release-workflow.py`](../../scripts/check-release-workflow.py)
  (in `make check`) fails if an upload or the move of `latest` is not preceded by it, or an action is not pinned by SHA.
- **Verify a download** (the full set, offline and cosign forms: [`RELEASING.md` § 8](../../RELEASING.md#8-publish-the-github-release)):
  `gh attestation verify mycelium-NEW-linux-x86_64.tar.gz --repo RichardTathata/mycelium --signer-workflow
  RichardTathata/mycelium/.github/workflows/release.yml --source-ref refs/tags/vNEW`, and the same with
  `oci://ghcr.io/richardtathata/mycelium:NEW` for the image.
- `version_line_names_the_crate_version`, in [`src/main.rs`](../../src/main.rs): the line is `mycelium ` and the
  version `Cargo.toml` declares, read from the manifest by the test. A tag cut out of order is caught earlier, by the
  workflow's version job comparing the tag to `Cargo.toml`; the smoke's own value is that the binary loads and runs
  in a container with no toolchain.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The end-to-end run is the workflow itself on a tag, and its dry run on a pull request that touches the publishing
path (or by dispatch after merge); the
test that runs the code is in [`src/main.rs`](../../src/main.rs): `version_line_names_the_crate_version`. The
download block a reader copies is [installation § The published node](../guide/installation.md#the-published-node).

## What it guarantees, and what it does not

- No guarantee id: publishing registers nothing in the [guarantee catalogue](../reference/guarantee-catalogue.md),
  and a downloaded node reports its own guarantees as any node does.
- **Not built:** no SBOM; reproducible builds are not claimed; SLSA Build L2, not L3 (the build runs in the
  repository's own workflow, not an isolated reusable builder); no attestation on releases cut before SO3; no
  Windows build; no package-manager formula. The image attestations and the move of `latest` are first exercised by
  a tag, since a dry run pushes no image — the [what-is-proven](../operations/what-is-proven.md) ledger's third table
  names [binary provenance](../operations/what-is-proven.md#binary-provenance).
- The plan rows are [`adopter-surface.md`](../plans/adopter-surface.md) X0 and [`security-operations.md`](../plans/security-operations.md#so3) SO3.

## Go deeper

- Runbook: [deployment § Get the node](../operations/deployment.md#get-the-node) and
  [`RELEASING.md` § 7](../../RELEASING.md).
- No guide chapter and no design record; the plan row is the record.
- Code: [`src/main.rs`](../../src/main.rs), [`.github/workflows/release.yml`](../../.github/workflows/release.yml),
  [`docker/Dockerfile.node`](../../docker/Dockerfile.node).
