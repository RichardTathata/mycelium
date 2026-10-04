# The catalog units — one declaration directory, two deployments

The same four units run as stem nodes in two profiles of `docker/docker-compose.stem-examples.yml`,
and the difference between them is **where the bytes come from**, which is a flag, not a declaration:

| Profile | The librarian | The installer | Where the bytes are |
|---|---|---|---|
| `catalog` | `--library /lib --librarian /lib/manifest` | nothing — pulls over the mesh from whoever advertises the librarian (in ranges when past the frame cap) | a shared volume on the librarian's node |
| `catalog_store` (v2.22.0) | `--library s3://mycelium-test/library --librarian … --manifest-source s3://mycelium-test/library` — the manifest read from the store, the blobs mirrored to a stage | `--library s3://mycelium-test/library` — its source *is* the store | an object store (S3Mock in the suite; credentials from `AWS_*` in the environment) |

Nothing in `librarian.toml`, `installer.toml`, `caller.toml` or `late.toml` changes between them: a
unit file says what a node offers, needs and may host; the byte source is the operator's choice at
start. Run either with `make test-stem-examples STEM_DEMOS=catalog` or `…=catalog_store`. The
`late.toml` unit is the `catalog` profile's phase 6 — a late joiner that installs from the installer's
re-served stage after the librarian is killed.

What the store profile shows that a bucket would not: the adapter's whole path (publish through it,
read the manifest from it, stage by ranged pull, verify provenance and hash). What only a real bucket
shows is on `docs/operations/what-is-proven.md` (S4).
