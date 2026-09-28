# The capability lifecycle — declare, check, deploy, publish, watch, read

↑ [Operations](README.md) · pairs with [deployment.md](deployment.md) and [artifacts.md](artifacts.md)

A capability reaches a cluster in one of **two ways**, and until this page they were told on two
different pages that never met (`docs/plans/design-time-tooling.md` §12, D16). This is the one
workflow, in the order an operator lives it, for both.

| | **Directly deployed** | **Dynamically installed** |
|---|---|---|
| What is deployed | a unit whose capabilities are in its binary or its `[[capability]]` file, started by your platform | a **host** unit that runs a provisioner (`mycelium-wasm-host`), with runtimes for some kinds and a budget |
| Who decides it exists | you, at deploy time | the host node, at runtime, when it sees unmet demand (a `req/` entry with no provider) or a presence floor below its count, self-elects, and the signed footprint fits its headroom |
| Where the code comes from | the unit's image | the catalogue entry's content address, pulled from the library and verified on arrival |
| When it goes away | when the unit stops | when demand lapses or a governor sheds it: a tombstone, and the placed bytes at the host's placement root |
| Its page | [deployment.md](deployment.md), guide 13 | [artifacts.md](artifacts.md) §3, [dynamic-scaling.md](dynamic-scaling.md) § Elastic capacity, the `provisioning` and `catalog` demos |
| What the offline check knows | the unit's `[[capability]]` blocks | an artifact description matches **and** some unit's `[hosts]` could take it — otherwise `unhostable entry` |

Both are **runtime facts** once the fleet exists: a directly deployed capability evaporates when its
advertisement is not renewed, an installed one is re-provisioned by a standby when its host dies, and
nothing assigns either to a node. What you fix at design time is the **vocabulary**: what could match
what, and who could host what.

## 1 · Declare — one file per unit

Every deployable unit gets a **unit file** (`src/capability_config.rs` is the reference; guide 02 has
the summary): its `principal`, the `[[capability]]` blocks it offers, the `[[requirement]]` filters it
needs, the `[[group]]`s it defines, the `[[lane]]`s it feeds or drains, the `[[mandate]]`s and
`[[rule]]`s it expects, and — for a host — the `[hosts]` table (kinds, install budget, headroom, trusted
publisher keys, placement root) and the `[[presence]]` floors it keeps. No secrets, no addresses: the
directory is safe to commit, and it is the thing the rest of this page checks, deploys and compares.

```
deploy/
  units/        one *.toml per unit         (checked in; the checker's input)
  artifacts/    one *.toml per artifact     (what each would provide, its kind, its footprint)
```

The co-op deployment is the worked example: `tests/fixtures/units/coop/` (six units, one artifact,
one presence floor), with its README.

## 2 · Check — before anything runs

```sh
cargo run --features cli --bin mycelium -- wire-check deploy/units --library deploy/artifacts
```

The check applies the mesh's own match rule to the directory and prints what **could not bind**:
`unwired requirement`, `schema-only mismatch`, `type-cross constraint`, `empty group`,
`group requires unmet`, `orphan lane`, `unhostable entry`, `presence unhostable` (each an error,
exit 1), and `would bind by provisioning`, `unranked ranking`, `single provider` (warnings; the first
becomes an error with `--strict-deployed`). `--format json` emits the versioned declaration
document, `--format dot` a Graphviz picture. Put it in CI beside the schema gate (guide 12): a red
check stops a merge.

What it cannot say, on its own `--help`: whether a provider is alive, which one is chosen, whether a
group has members *now*, whether provisioning would actually fill a gap on the night, whether a
mandate is current, or anything about load. It says *would bind under these declarations*.

## 3 · Deploy — the direct units and the hosts

Deploy the units as your platform deploys anything ([deployment.md](deployment.md): a gossip port,
a seed, the CA identity). A **host** unit is a direct deployment too — the image carries the mesh
binary, the wasm host and a provisioner, the runtimes for the kinds in its `[hosts]` table (the WASM
sandbox is built in; a `blob` runtime needs its native consumer, an Ollama or ONNX process, already on
the node), the publisher keys it trusts, reach to the library or store, and its egress policy. A fleet
of identical hosts is the **stem fleet** (`docs/plans/design-time-tooling.md` §13): it holds nothing
application-specific at deploy time and loads what the declarations call for.

Today the node does **not** read the unit file at startup: a unit declares its requirements, groups
and presence in code, and the file is the same vocabulary written down for the check. Making the
file the source a node declares from is the plan's R1, next.

## 4 · Publish — the catalogue

For every artifact in `deploy/artifacts/`: build the bytes, store them in the library, describe and
sign the entry, append the manifest line ([artifacts.md](artifacts.md) §2). A librarian reconciles the
manifest to the gossiped catalogue, so every node sees the entry; the bytes travel only to the nodes
that install. A large artifact reaches a host in `Range` pieces staged to disk, never through memory
([artifacts.md](artifacts.md) § Remote blob stores).

## 5 · Watch — demand pulls the rest in

Start the fleet. A unit's `req/` entry with no provider is **demand**; a host's `[[presence]]` floor
is a standing want. Every host reconciles both locally: it sees the gap, self-elects with a probability
that damps the herd, checks its runtime, budget and headroom, pulls, verifies, installs, advertises.
Kill the host and a standby does the same. Watch it: `mycelium_artifact_*` in
[metrics.md](metrics.md), `ineligible_skips` for a host that could not take an entry, and
[dynamic-scaling.md](dynamic-scaling.md) for the governors above it.

## 6 · Read — what actually bound

The check's JSON is a **declaration**: what would bind. The fleet's own view — `resolve_wiring`,
`GET /gateway/fleet`, the opacity entries under `sys/load/*/req/` — is what **did**. Compared, they
give three views: an edge declared and never traversed (idle or broken; the opacity entry says
which), a call between two principals no declaration wired (a report, never a block), and an
authority edge the design already excluded. A consumer that holds the evidence records can render the
comparison (`design-time-tooling.md` §9); the record says *declared*, never *desired and enforced*.

## What this page does not claim

A green check does not mean the deployment will wire; the runtime keeps the last word and its reports
are the ones you act on. The `[hosts]` table describes eligibility, not placement: which host installs
a given artifact is decided at runtime and the check never says which. And only two kinds load
dynamically — WASM components against the host's four-interface world, and blobs for a runtime
already present; new native code is a new image and a rolling upgrade.
