# The signed catalogue

## What it is

The [catalogue](../guide/00-concepts.md#catalogue) is what a fleet can install: gossiped entries under
`installable/{ns}/{name}/{hex}`, each naming a capability, a runtime kind, a content address and a footprint, signed by
its publisher. There is no registry server — the catalogue is the gossip store, and every node holds it. Two checks
keep it honest. The bytes are verified against the entry's content address when they arrive, so a peer cannot
substitute others; and a host installs only an entry a key in its `trusted_publishers` signed, so an entry that
hashes correctly but comes from anyone else is refused. Publisher keys stay with the publisher; a node holds only the
public half.

## When to use it, and when not

- **Use it when** generic hosts should install what you publish and nothing anyone else gossips, whoever serves the
  bytes.
- **Not when** you want a host to run whatever arrives: an empty trust list is refused unless `accept_unsigned = true`
  says so on purpose, and the stem warns once at start.

## Turn it on

Sign entries where the key is, with the `mycelium-artifact` binary, and list the public key on each host.

- `mycelium-artifact publish | list | verify | accept`, in
  [`mycelium-wasm-host/src/bin/mycelium-artifact.rs`](../../mycelium-wasm-host/src/bin/mycelium-artifact.rs): sign a
  description into a library's manifest, list it, verify it, and co-sign a proposal as a reviewer.
- `[hosts].trusted_publishers` ([unit-file § hosts](../reference/unit-file.md#hosts)), `ed25519:<hex>` keys; in code,
  [`require_provenance`](../../mycelium-wasm-host/src/provisioner.rs) — without it a provisioner checks only that the
  bytes match their content address, not who signed them.
- In code, [`refresh_catalog`](../../mycelium-wasm-host/src/provisioner.rs) each round: a provisioner's catalogue is a
  snapshot taken at construction, so entries gossiped later are unseen until it is refreshed (a stem does this every
  tick).
- [`InstallableEntry::signed_by`](../../mycelium-wasm-host/src/catalog.rs) and
  [`verify_provenance`](../../mycelium-wasm-host/src/catalog.rs) for code that signs or checks one itself.
- `POST /gateway/artifacts/publish` announces an already-signed line to a running cluster, on a stem built with
  `gateway`, with an `http_port`, whose `[hosts]` names trusted publishers, under `artifact:publish`
  ([artifacts § Publishing through a gateway](../operations/artifacts.md#publishing-through-a-gateway-a3)).
  The scope is checked only in a build with `compliance`, which the published `mycelium-stem` carries
  (`stem,gateway,llm,object_store,compliance`, since the release after 2.39.0); a stem built without it admits any
  valid bearer (or anyone, on an open gateway with no token model) — either way the line's provenance is still checked
  against the trusted publishers before it is written.
- `require_provenance_refuses_unsigned_but_installs_trusted_signed`, in
  [`mycelium-wasm-host/src/provisioner.rs`](../../mycelium-wasm-host/src/provisioner.rs).

## See it work

The [example index](../../examples/README.md#the-capability-matrix) lists
[`catalog`](../../examples/README.md#the-capability-matrix): a component read from disk into a library with a signed
manifest, a librarian syncing it into the catalogue, and an installer that discovers the entry, verifies its
provenance and pulls it over the mesh. CI runs it.

## What it guarantees, and what it does not

- **Rule:** [`prov.provenance`](../reference/rule-catalogue.md#provprovenance) — `signed_by_trusted_publisher`, or
  refused `not_signed_by_trusted_publisher` (an invalid signature and an untrusted publisher are one answer), rev 2;
  `no_trust_list_configured` only where unsigned entries were accepted on purpose. These are the rule's described answers; a trace records a refused line as `provenance_rejected` under the calling rule (`prov.demand_response` or `prov.presence_floor`).
- **Not built:** signing proves who published an entry, not that the component is safe — the publisher keys are the
  posture to review; there is no revocation of a published entry beyond removing it from the manifest; a gateway
  publish is refused when the signer is a key a librarian on that node manages (409).
- **Evidence:** the ledger row
  [a signed catalogue line reaches a second node with provenance intact](../operations/what-is-proven.md#signed-catalogue-line-reaches-second-node).

## Go deeper

- **Guide:** [16 § Agent-authored functions](../guide/16-guardrails.md#agent-authored-functions-the-legitimate-uses-of-dynamic-load).
- **Runbook:** [artifacts § Trust and provenance](../operations/artifacts.md#trust--provenance) and
  [§ Declare the footprint, sign, and publish](../operations/artifacts.md#2--declare-the-footprint-sign-and-publish-to-the-library).
- **Design:** [`design/artifact-library.md`](../design/artifact-library.md).
- **Code:** [`mycelium-wasm-host/src/catalog.rs`](../../mycelium-wasm-host/src/catalog.rs).
