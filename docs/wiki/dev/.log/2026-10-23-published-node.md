## [2026-10-23] ingest | the node is published (adopter-surface X0)

- `docs/plans/adopter-surface.md` (agreed 2026-10-23, #663) opens with X0: nothing published a binary or an image, so an
  SDK developer needed a Rust toolchain before their first line. `.github/workflows/release.yml` now runs on every `v*`
  tag: `mycelium`, `mycelium-stem` and `mycelium-artifact` for Linux x86_64, Linux aarch64 and macOS aarch64, each
  built natively on its own runner (no QEMU); the Linux tarballs smoked in a clean bookworm container with no toolchain
  (`mycelium --version` must equal the tag; `wire-check` over the unit fixtures); tarballs and `SHA256SUMS` attached to
  the tag's Release, created from the annotated tag's message if RELEASING § 8 has not run (the Releases page cannot
  fall behind the tags again, as after v2.4.4); a multi-arch image pushed by digest per arch and merged into one
  manifest at `ghcr.io/richardtathata/mycelium:<version>` and `:latest`. `workflow_dispatch` is the dry run.
- `docker/Dockerfile.node` is the image: the existing `docker/Dockerfile` builds the three-node demo example, not the
  node. A bookworm builder (glibc 2.36, `libssl3` — the node's `reqwest` is native-tls), an `export` stage the tarballs
  come from (tarball and image carry the same bytes), a non-root runtime stage. Feature sets: the node
  `cli,tls,compliance,a2a,llm,metrics` (a config naming a table the build lacks refuses to start, v2.18.1, so a
  published node must carry them all), the stem `stem,gateway,llm,object_store`.
- `--version` / `-V` on all three binaries, one line each (`src/main.rs` `version_line`, tested by
  `version_tests::version_line_names_the_crate_version`, which CI runs under `cargo test --features tls,a2a --bins`).
- Pages touched: `operations.md` (the published node), `docs/features/published-node.md` (new, registry entry
  `concept:published-node`, glossary row), `docs/guide/installation.md` § The published node, `deployment.md` § Get the
  node, `RELEASING.md` §§ 6–8, both SDK READMEs, `what-is-proven.md` (binary provenance in the third table), CHANGELOG.
- Not built, stated: no signing (Sigstore or GPG) — the sums are integrity, not provenance; no Windows build; no
  package-manager formula; reproducible builds not claimed.
