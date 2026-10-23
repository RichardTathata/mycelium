# Published node

## What it is

Every `v*` tag publishes the [published node](../guide/00-concepts.md#published-node): the `mycelium`,
`mycelium-stem` and `mycelium-artifact` binaries for Linux x86_64, Linux aarch64 and macOS aarch64, a `SHA256SUMS`,
and a multi-arch image at `ghcr.io/richardtathata/mycelium`. A Python or TypeScript developer starts a node without a
Rust toolchain. The Linux tarballs are smoked in a clean container before anything is attached, and the GitHub Release
is created from the annotated tag's message if the runbook's step 8 has not run, so the Releases page cannot fall
behind the tags again.

## When to use it, and when not

- **Use it when** you run the node from an SDK, a container or a host without a Rust toolchain, with the published
  feature set (`cli,tls,compliance,a2a,llm,metrics` for the node; `stem,gateway,llm,object_store` for the stem).
- **Not when** you need another feature set, a platform not listed (Windows, Linux older than glibc 2.36), or a
  signed artefact: the sums are integrity, not provenance, and the build from a pinned tag is the path
  ([installation § Pin the source](../guide/installation.md#pin-the-source)).

## Turn it on

This feature is turned on in code.

- [`version_line`](../../src/main.rs) — the one line `mycelium --version` prints, which the workflow's clean-container
  smoke compares to the tag.
- The workflow itself is [`.github/workflows/release.yml`](../../.github/workflows/release.yml) and the image is
  [`docker/Dockerfile.node`](../../docker/Dockerfile.node); a `workflow_dispatch` run is the dry run.
- `version_line_names_the_crate_version`, in [`src/main.rs`](../../src/main.rs): the line equals the crate's
  version, so a tag cut out of order is caught by the string.

## See it work

No runnable example exists: the [example index's capability matrix](../../examples/README.md#the-capability-matrix)
has no row for this feature. The end-to-end run is the workflow itself on a tag, and its dry run on any branch; the
test that runs the code is in [`src/main.rs`](../../src/main.rs): `version_line_names_the_crate_version`. The
download block a reader copies is [installation § The published node](../guide/installation.md#the-published-node).

## What it guarantees, and what it does not

- No guarantee id: publishing registers nothing in the [guarantee catalogue](../reference/guarantee-catalogue.md),
  and a downloaded node reports its own guarantees as any node does.
- **Not built:** no signing of the binaries or the image (Sigstore or GPG); no Windows build; no package-manager
  formula; reproducible builds are not claimed — the [what-is-proven](../operations/what-is-proven.md) ledger's
  third table names binary provenance.
- The plan row is [`adopter-surface.md`](../plans/adopter-surface.md) X0.

## Go deeper

- Runbook: [deployment § Get the node](../operations/deployment.md#get-the-node) and
  [`RELEASING.md` § 7](../../RELEASING.md).
- No guide chapter and no design record; the plan row is the record.
- Code: [`src/main.rs`](../../src/main.rs), [`.github/workflows/release.yml`](../../.github/workflows/release.yml),
  [`docker/Dockerfile.node`](../../docker/Dockerfile.node).
