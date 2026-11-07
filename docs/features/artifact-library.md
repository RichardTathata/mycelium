# The artifact library and publishing

## What it is

An [artifact](../guide/00-concepts.md#artifact) is deployable bytes — a WASM component, model weights, a profile —
addressed by the hash of its content. The [library](../guide/00-concepts.md#library) is where the bytes and their signed
manifest live: a directory, an HTTP server or an object store. A librarian is the node role that serves a library's
bytes over the mesh and keeps the gossiped catalogue reconciled to the manifest, adding what it lists and tombstoning
what it no longer does, for the entries its own publisher key signed. An installer may pull from the librarian over the
mesh, in ranges for a blob past the frame cap, or read the library directly; whatever the source, the bytes are
checked against their content address before use.

## When to use it, and when not

- **Use it when** a fleet installs from a durable, reviewable set of artifacts, and the publisher's key should never be
  on a node.
- **Not when** you need a registry that decides who may install what: the library serves bytes and a manifest, and the
  trust decision is each host's ([the signed catalogue](signed-catalogue.md)).

## Turn it on

This feature is turned on in code or by the stem's flags. `object_store` (on `mycelium-wasm-host`) adds `s3://` and
`gs://` libraries; `gateway` adds the publish route.

- `mycelium-stem --library <dir | url>`: read bytes from a directory or an object store; `--librarian <manifest>` with
  `--publisher ed25519:<hex>` takes the librarian role, and `--manifest-source <url>` reads the manifest from the store
  ([capability lifecycle § Running a stem](../operations/capability-lifecycle.md#running-a-stem)).
- In code, [`FsLibrarySource`](../../mycelium-wasm-host/src/artifact.rs),
  [`ObjectStoreFetcher`](../../mycelium-wasm-host/src/object_store_source.rs) (`object_store`), and
  [`spawn_librarian`](../../mycelium-wasm-host/src/librarian.rs) with a
  [`LibrarianConfig`](../../mycelium-wasm-host/src/librarian.rs).
- [`MeshArtifactSource`](../../mycelium-wasm-host/src/mesh_source.rs): pull from whoever serves the content address.
- [`artifact_router`](../../mycelium-wasm-host/src/gateway.rs) (`gateway`): `POST /gateway/artifacts/publish`, under
  `artifact:publish`; `mycelium-stem` mounts it when its `[hosts].trusted_publishers` is non-empty, served on its
  `http_port`.
  The scope is checked only in a build with `compliance`; the published `mycelium-stem` is built without it
  (`stem,gateway,llm,object_store`), so there any valid bearer reaches the route (or anyone, on an open gateway with no token model) — the line's provenance is still
  checked against the trusted publishers before it is written.
- `reconciles_manifest_to_catalogue_scoped_to_its_own_signer`, in
  [`mycelium-wasm-host/src/librarian.rs`](../../mycelium-wasm-host/src/librarian.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`model_deploy`](../../examples/README.md#the-capability-matrix): real model weights and their profile travel the library
as two signed, content-addressed artifacts; it needs Ollama, so it is a manual run, not CI's. The run CI does is the
[`catalog`](../../examples/README.md#the-capability-matrix) example, and the publish route end to end is
`a_signed_entry_reaches_a_second_node_and_the_refusals_are_by_name`, in
[`mycelium-wasm-host/tests/gateway.rs`](../../mycelium-wasm-host/tests/gateway.rs).

## What it guarantees, and what it does not

- No guarantee or rule id of its own: provenance is [`prov.provenance`](../reference/rule-catalogue.md#provprovenance)'s
  ([the signed catalogue](signed-catalogue.md)), and the content check is in every source.
- **Not built:** a real S3 or GCS bucket in CI — `s3://` is proved against S3Mock and `gs://` only against a real bucket
  when one is configured ([artifacts § Object stores](../operations/artifacts.md#object-stores-what-ci-proves-and-what-it-does-not));
  redirects inside `object_store`'s own client are outside the egress check; a librarian reconciles only its own
  signer's entries — scoped to the key, not the node, so a librarian on another node holding the same key with a
  different manifest tombstones what this one lists.
- **Evidence:** the ledger rows
  [a signed catalogue line reaches a second node](../operations/what-is-proven.md#signed-catalogue-line-reaches-second-node)
  and [models pulled from S3 or GCS](../operations/what-is-proven.md#models-pulled-from-s3-gcs) (not yet shown).

## Go deeper

- **Guide:** [02, capabilities](../guide/02-capabilities.md).
- **Runbook:** [artifacts](../operations/artifacts.md) (operating the catalogue, authoring and publishing).
- **Design:** [`design/artifact-library.md`](../design/artifact-library.md).
- **Code:** [`mycelium-wasm-host/src/artifact.rs`](../../mycelium-wasm-host/src/artifact.rs),
  [`librarian.rs`](../../mycelium-wasm-host/src/librarian.rs), [`mesh_source.rs`](../../mycelium-wasm-host/src/mesh_source.rs),
  [`object_store_source.rs`](../../mycelium-wasm-host/src/object_store_source.rs).
