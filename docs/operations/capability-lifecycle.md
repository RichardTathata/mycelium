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

Every deployable unit gets a **unit file** (the [unit-file reference](../reference/unit-file.md) has
every field, type and default; guide 02 has the summary): its `principal`, the `[[capability]]` blocks it
offers, the `[[requirement]]` filters it needs, the `[[group]]`s it defines, the `[[lane]]`s it feeds or
drains, the `[[mandate]]`s and `[[rule]]`s it expects, and — for a host — the `[hosts]` table (kinds,
install budget, headroom, trusted publisher keys, placement root, fuel and reviewers), the
`[[presence]]` floors it keeps, the `[[activation]]` that hands a placed blob to its local runtime, and
the `[[serve]]` that makes a live install a routable model skill. The unit directory is the thing the rest of this page checks, deploys and compares.
Keep credentials out of committed units.
`[[serve]].api_key` is a literal secret: a directory containing one is not safe to commit — name the variable with `api_key_env` instead, read once at stem start (unset refuses the start by name).

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

The check applies the mesh's own match rule to the directory and prints what **could not bind**.
Each finding has a `kind` a script can key on (`src/wire_check.rs`):

| Kind | Severity |
|---|---|
| `unwired requirement` — no declared capability satisfies the filter | error |
| `schema-only mismatch` — it matches except for `schema_id` | error |
| `type-cross constraint` — a constraint's value type can never compare with the offered attribute | error |
| `empty group` — nothing satisfies the group's filter | error |
| `unknown schema` — a `schema_id` the schema directory does not define (only with `--schemas`) | error |
| `group requires unmet` — a group's `requires` binds to nothing | error |
| `orphan lane` — consumed and never produced, or the reverse | error |
| `unhostable entry` — an artifact would satisfy the filter but no `[hosts]` can take it | error |
| `presence unhostable` — a `[[presence]]` floor cannot be met | error |
| `presence bands overlap` — two `[[presence]]` bands, at least one capped, can match the same providers | warning |
| `unauthorisable edge` — the requirer declares authority, and no declared rule and mandate could admit the call | error |
| `would bind by provisioning` — no deployed provider, but a hostable artifact matches | warning; error with `--strict-deployed` |
| `would bind after acceptance` — as above, but the artifact is **proposed** and loads only into a shadow lane until accepted | warning; error with `--strict-deployed` |
| `ungoverned edge` — the requirer declares no mandate and no rule while other units do | warning |
| `unranked ranking` — a ranking on an attribute no matching capability carries | warning |
| `single provider` — exactly one possible provider | warning |

Flags (`mycelium wire-check <units-dir> --help`):

| Flag | Effect |
|---|---|
| `--library <dir>` | read every `*.toml` in `<dir>` as an artifact description |
| `--schemas <dir>` | the schema directory (guide 12); every declared `schema_id` must name a `.json` file in it |
| `--format text\|json\|dot` | text (default), the versioned JSON declaration document, or a Graphviz picture |
| `--strict-deployed` | the two *would bind* warnings become errors |
| `--no-authority` | skip the authority overlay (`unauthorisable edge`, `ungoverned edge`) |
| `--revision <rev>` | the revision stamped on the JSON; defaults to `git rev-parse HEAD` in the units directory |

Exit codes: **0** no errors, **1** at least one error, **2** a usage error or a file that does not
load. The JSON output's schema is [`docs/reference/declaration.schema.json`](../reference/declaration.schema.json)
(`$id` `mycelium.design/declaration/1`); a change to its shape is a new schema version. Put the check
in CI beside the schema gate (guide 12): a red check stops a merge.

What it cannot say, on its own `--help`: whether a provider is alive, which one is chosen, whether a
group has members *now*, whether provisioning would actually fill a gap on the night, whether a
mandate is current, or anything about load. It says *would bind under these declarations*.

## 3 · Deploy — the direct units and the hosts

Deploy the units as your platform deploys anything ([deployment.md](deployment.md): a gossip port,
a seed, the CA identity). A **host** unit is a direct deployment too — the image carries the mesh
binary, the wasm host and a provisioner, the runtimes for the kinds in its `[hosts]` table (the WASM
sandbox is built in; a `blob` runtime needs its native consumer, an Ollama or ONNX process, already on
the node), the publisher keys it trusts, a way to read artifact bytes, and its egress policy. A stem
reads bytes from a **library directory** on the node (`--library <dir>`, a local disk or a mounted
volume), **over the mesh** from a librarian (no `--library`), or from an **object store** by URL
(`--library s3://bucket/prefix`, feature `object_store`; credentials from the environment, the URL
gated by the node's egress policy). Every path stages what it pulls to disk under
`<placement_root>/stage` and verifies it; a blob past the 10 MiB frame cap crosses the mesh in 4 MiB
ranges (v2.22.0), so a model host no longer needs a mounted library, and a host re-serves what it
staged. A librarian over a store reads its manifest from there (`--manifest-source <url>`) and mirrors
what it names ([artifacts.md](artifacts.md) § Remote blob stores). A fleet
of identical hosts is the **stem fleet** (`docs/plans/design-time-tooling.md` §13): it holds nothing
application-specific at deploy time and loads what the declarations call for.

A **stem node** declares from the file at startup: `mycelium-stem --units <unit.toml>` (a binary of
`mycelium-wasm-host`, feature `stem`) advertises every `[[capability]]`, declares every
`[[requirement]]`, defines every `[[group]]`, and, with `[hosts]`, runs a provisioner from it with
every `[[presence]]` as a standing want — `--library <dir>` to read bytes from a directory,
`--library <url>` from an object store, none to pull them over the mesh from a librarian,
`--librarian <manifest> --publisher ed25519:<hex>` to take the librarian role too. The file and the runtime vocabulary are then one thing; every declaration
still travels as its own evaporating entry and no other node reads the file. A unit that declares in
code keeps working; the `mycelium` node binary itself does not read the file, because the provisioner
lives in the wasm-host crate and the dependency runs the other way. An **SDK agent's** unit file
reaches its node through `POST /gateway/units/declare` (scope `cap:write`; [guide 10](../guide/10-language-bridges.md)):
the node declares its capabilities, requirements and groups, refuses the hosting sections with 422 and a
governed group's `[[group]]` with 403 `governed_group` (2.29.0), and
reports lanes, mandates and rules as `not_enforced`.

### Running a stem

```sh
cargo build -p mycelium-wasm-host --features stem,gateway,llm --bin mycelium-stem
```

The binary needs feature `stem`. `gateway` adds the publish route `POST /gateway/artifacts/publish`;
without it there is no route. With it, the route is mounted only when `[hosts].trusted_publishers`
is non-empty, served only when the node's config sets `http_port`, and admits only a bearer token
holding `artifact:publish`. `llm` is needed for `[[serve]]`: a stem built without it refuses to start
when the unit file has a `[[serve]]` section.

| Flag | Default | Meaning |
|---|---|---|
| `--units <file>` | required | the unit file to declare from |
| `-c, --config <file>` | env overrides on `GossipConfig` defaults | the `GossipConfig` TOML |
| `-p, --port <port>` | from the config | bind port |
| `--host <ip>` | from the config | bind address |
| `-r, --peers <ip:port,…>` | from the config | bootstrap peers |
| `--library <dir \| url>` | none: pull over the mesh (5 s per fetch; a blob past the frame cap in 4 MiB ranges) | read artifact bytes from this directory, or from an object store by URL (`s3://`, `gs://`, `file://`; feature `object_store`, credentials from the environment) |
| `--stage-dir <dir>` | `<placement_root>/stage` | where a mesh or store pull stages what it fetches, verified, before the runtime reads it |
| `--librarian <manifest>` | off | also take the librarian role over `--library` and this manifest file; needs `--library` and `--publisher` |
| `--manifest-source <url>` | off | with `--librarian`: read the manifest from the store at this URL instead of the file; with a URL `--library`, the librarian also mirrors what the manifest names to its stage |
| `--publisher ed25519:<hex>` | none | the manifest's publisher key (with `--librarian`) |
| `--tick-ms <n>` | `500` (minimum `50`) | the provisioner's tick |
| `--self-elect <p>` | `0.5` (clamped to `0..1`) | self-election probability per round |
| `--trace-dir <dir>` | off | the **decision trace**: every provisioning decision (what it read, how it ended, the typed reason) recorded from the values the round produced, written as `<dir>/decisions.jsonl` with `decisions.stats.json` (what was dropped) and `coverage.json` (which rules this stem could record) on shutdown; read it with `mycelium explain <dir>/decisions.jsonl --catalogue docs/reference/rule-catalogue.json`. It changes no decision and never waits; the catalogue of what each record means is [`docs/reference/rule-catalogue.md`](../reference/rule-catalogue.md) |

The `model-host` service of `docker/docker-compose.stem-examples.yml`, as a command:

```sh
mycelium-stem --units /repo/examples/units/model_deploy/model-host.toml --library /lib \
              --host 172.40.0.42 -p 57000 -r 172.40.0.40:57000 --tick-ms 300
```

## 4 · Publish — the catalogue

For every artifact in `deploy/artifacts/`: build the bytes, then `mycelium-artifact publish
<description> --library <dir> --key-env <SEED>` stores them, signs the entry and appends the manifest
line; `mycelium-artifact verify <library> --trusted ed25519:<hex> --descriptions deploy/artifacts` in CI keeps the reviewable
description and the signed manifest from drifting ([artifacts.md](artifacts.md) §2). A librarian reconciles the
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

**Presence ceilings — which host withdraws.** Above a band's `max_providers`
([`[[presence]]`](../reference/unit-file.md)), the hosts ranked beyond `max` by the band's rendezvous order
withdraw (`mycelium::election::rank`, rule `prov.shed` rev 2, 2.26.0) — every host with the same view agrees
which, so exactly the surplus leaves; before 2.26.0 every host drew, and at `self_elect_p = 1.0` all withdrew at
once. Only providers that will act are ranked: a stem advertises a `prov-shed/{ns}:{name}:{hash}` capability
for each band it supervises with a ceiling (visible under `GET /gateway/kv/keys?prefix=cap/`); a provider that
does not — registered in code, a stem without the band — keeps its place once seen unmarked for two advertise
intervals. What to expect and do:

- **Bound.** A view with *k* wrong entries (a crashed provider still advertised, a mark not yet arrived) can
  leave the band at `max − k` for a round, and it never cascades. The floor refills only below `min`: between
  `min` and `max` the band stays where the round left it until demand or a new install moves it. A stem drops its
  `prov-shed` mark while it serves the band without an install it could withdraw (`marks_band`), so it then
  counts as fixed.
- **Rolling upgrade.** A stem older than 2.26.0 advertises no mark and still sheds by its own draw, so while old
  and new stems share a band it can dip below the ceiling; a new stem cannot stop an old one's draw. **Do one of:**
  upgrade a band's stems together; or raise the band's `max_providers` by the number of old stems for the roll and
  lower it after. **See it:** when a band that was above its ceiling falls below it and the providers that left
  had no `prov-shed` mark (and had been unmarked for two advertise intervals), a stem counts (2.28.0) `mycelium_artifact_presence_unranked_departures_total` and logs a
  warning naming the band and the providers (an old stem's draw — or a crash, which looks the same from here).
- **Overlapping bands** (say one unattributed, one attribute-restricted, over the same capability, at least one with
  a ceiling) can trade a provider back and forth — the capped one sheds against its ceiling while the other's floor
  or ceiling brings one back. `mycelium wire-check` names the pair (`presence bands overlap`, a warning) unless they
  are disjoint (an `eq` on one attribute to different values, or different `schema_id`s), are one band however
  spelled, or are a floor and a ceiling over one population with the floor at or below the ceiling; give them
  room, drop a ceiling, or make the filters disjoint.
- **Seeing who withdrew.** `mycelium_artifact_presence_sheds_total` counts this node's withdrawals; for the band
  and the rank, run the stem with `--trace-dir` and read the `prov.shed` decisions with `mycelium explain`
  ([diagnostics.md](diagnostics.md) § reading what a node decided): `above_ceiling` is a withdrawal,
  `ranked_within_ceiling` a host that stayed, with its rank in the inputs. **The stem writes the trace when it
  shuts down on SIGINT, and only then:** `docker stop` and a Kubernetes pod stop send SIGTERM, which
  `mycelium-stem` does not handle — it is killed without writing the trace or withdrawing gracefully. Stop it
  with `docker kill -s INT <container>` (or `kill -INT`) to collect a trace.
- **A runnable ceiling.** `cargo run -p mycelium-wasm-host --example first_stem_fleet` (run in CI) declares a
  band of exactly two over three stems that all self-elect, so the surplus host withdraws by rank.

**Placed blobs that need a runtime.** A model or data pack is *placed* by a hosting unit, but a
placed file serves nothing until the node-local runtime has it. A unit says how, per capability
(every field and placeholder: [`[[activation]]`](../reference/unit-file.md#activation)):

```toml
[[activation]]
ns   = "llm"
name = "storyteller"
command = ["ollama", "create", "storyteller", "-f", "{rendered}"]
probe   = ["ollama", "show", "storyteller"]
resolve_artifact_refs = true      # the profile's `FROM artifact:<hex>` becomes the placed weights' path
```

A model that should answer routed inference also needs a **routable skill**, which a placed and
activated file is not. `[[serve]]` registers one while the install is live and retracts it when the
install goes, so a router fails over instead of calling a node whose model was withdrawn (every
field: [`[[serve]]`](../reference/unit-file.md#serve)):

```toml
[[serve]]
name     = "storyteller"                       # the routable skill llm/storyteller
endpoint = "http://localhost:11434/v1"         # this host's OpenAI-compatible runtime
model    = "coop-storyteller"
[serve.while_live]
ns   = "llm"
name = "storyteller-deploy"                    # the install it waits for (a different name)
```

The stem runs `command` after placement and before the capability is advertised, re-runs `probe`
in the background, and withdraws the install when the probe fails — the next round reinstalls and
re-activates. A profile that references weights not yet placed fails its activation and is retried,
which is how the profile waits for its weights without a dependency resolver. The commands are the
operator's, run with the stem's privileges, from a reviewed file: this is not a sandbox.

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
