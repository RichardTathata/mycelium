# The unit file — reference

↑ [Docs](../README.md) · workflow: [capability-lifecycle.md](../operations/capability-lifecycle.md) ·
summary: [guide 02](../guide/02-capabilities.md) · code: `src/capability_config.rs`

A **unit file** is one TOML file per deployable unit. It says what the unit offers, what it needs,
the groups it defines, the lanes it touches, the authority it expects, what it can host, and how a
hosted blob reaches its local runtime. Every section is optional. A file with only `[[capability]]`
blocks is a valid unit file. "Unit declaration" and "declaration" name the same file
([glossary](../guide/00-concepts.md#unit-file)).

The loader is `NodeCapabilityConfig::from_toml_str` (or `load_from_file`): it parses the file and
then runs `NodeCapabilityConfig::validate`, which refuses a bad value **by name**. The same loader
is used by `mycelium wire-check`, by the stem, and by `POST /gateway/units/declare`. There is no
second parser.

TOML needs one key per line. `ns = "a"; name = "b"` is not valid TOML.

## Who reads which section

| Section | `mycelium wire-check` | Stem (`mycelium-stem --units`) | SDK agent (`POST /gateway/units/declare`) |
|---|---|---|---|
| `principal` | carried per unit in the JSON, the join key | validated, not otherwise used | echoed back in the response |
| `runtime` | the adapter contract's row: `carries` / `not_carried` per unit, `unknown runtime` and `runtime with a stem section` by name | accepted, not read | accepted, not read |
| `[[capability]]` | an offer | advertised (always-alive: a stem does not run `probe_url`) | advertised |
| `[[requirement]]` | matched against offers | declared | declared |
| `[[group]]` | checked for members | defined | defined |
| `[[lane]]` | name join (`orphan lane`) | logged, nothing to do at runtime | accepted, reported in `not_enforced` |
| `[[mandate]]`, `[[rule]]` | the authority overlay | logged, nothing to do at runtime | accepted, reported in `not_enforced` |
| `[hosts]` | who could host an artifact | runs a provisioner | refused, **422** |
| `[[presence]]` | `presence unhostable`, `presence bands overlap` | a standing want (needs `[hosts]`) | refused, **422** |
| `[[activation]]` | not read | run after a blob is placed, confined when it names `[activation.confine]` | refused, **422** |
| `[[serve]]` | counted as an offer | registers a routable skill (needs feature `llm`) | refused, **422** |

An SDK agent hosts nothing, so the four hosting sections are a stem's. The route's guide is
[guide 10](../guide/10-language-bridges.md). The `mycelium` node binary does not read the unit file; an
application that embeds the library can run the `[[capability]]` probe loop with
`run_capability_probes`.

## `principal`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `principal` | string | absent | The issuer-qualified principal this unit presents at a gateway. It is the key that joins a declaration to what the fleet later reports. Not an address. Refused when present and empty. |

## `runtime`

| Field | Type | Default | Meaning |
|---|---|---|---|
| `runtime` | string | absent | The SDK an agent will load this unit with: `python` (`mycelium-py`), `typescript` (`mycelium-ts`), `strands` (`strands-session-mycelium`) or `langgraph` (`langgraph-checkpoint-mycelium`) — a row of [the adapter contract](adapter-contract.md). Absent, the unit is native: a Rust embedding. `wire-check` prints, per unit, which properties that runtime carries and which it does not, and an unknown value is the finding `unknown runtime`. A declaration of what will run the unit, not an instruction to any node: the gateway's `units/declare` accepts and ignores it. Refused when present and empty (adopter-surface X8). |

## `[[capability]]`

One block per capability the unit advertises.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns` | string | required | Capability namespace, e.g. `"llm"`. One key segment: refused when empty or holding a `/`. |
| `name` | string | required | Capability name, e.g. `"inference"`. One key segment, as `ns`; the pair `locality`/`self` is refused (the node's own locality entry). |
| `probe_url` | string | absent | HTTP GET probed for liveness; a 2xx keeps the capability advertised. Absent means always-alive. Honoured by `run_capability_probes`, not by a stem. |
| `probe_timeout_secs` | integer | `3` | Probe request timeout, in seconds. |
| `ttl_secs` | integer | `30` | Re-assertion TTL of the advertisement, in seconds: `1`–`3600` (`MAX_TIMER_PERIOD_SECS`), refused by `validate()` otherwise. |
| `attrs` | table of values | empty | Static attributes. A value is a string, integer, float, boolean, or `{ version = "x.y.z" }`. A bare string stays text. |
| `schema_id` | string | absent | The schema this capability advertises. A requirement with a `schema_id` matches only the same id. |

```toml
# examples/units/semantic_coordination/agent-b.toml
[[capability]]
ns       = "compute"
name     = "gpu"
ttl_secs = 30
schema_id = "acme-render/v1"
  [capability.attrs]
  vram_gb = 24
```

## `[[requirement]]`

What the unit needs: a `CapFilter`, the value `declare_requirement` takes.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns` | string | required | Namespace to match. Refused when empty or holding a `/`. |
| `name` | string | required | Name to match. Refused when empty or holding a `/`, and the pair `locality`/`self`. |
| `schema_id` | string | absent | Exact schema id to match. |
| `attrs` | table of constraints | empty | Per attribute: a bare value means equals; a table names one operator — `eq`, `ne`, `gt`, `gte`, `lt`, `lte`. Two operators in one table are refused. |
| `ranking` | table | absent | `attribute` (string) and `order` (`"ascending"` or `"descending"`). Any other order is refused. |

```toml
# tests/fixtures/units/coop/worker.toml
[[requirement]]
ns   = "depot"
name = "intake"
  [requirement.attrs]
  capacity = { gte = 50 }
  [requirement.ranking]
  attribute = "capacity"
  order     = "descending"
```

## `[[group]]`

A capability group the unit defines: the fields of `CapabilityGroupDef` plus a name. A group under a live
membership intent is **governed**, and `POST /gateway/units/declare` refuses to redefine it (**403**
`governed_group`, 2.29.0) — its filter decides whom the governor can elect.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | required | Group name. Refused when empty or holding a `/`. |
| `filter` | table | required | Who may join: the same fields as a `[[requirement]]` (`ns`, `name`, `schema_id`, `attrs`, `ranking`). |
| `topology_policy` | table | absent | How a group's quorum spreads across locality levels: `prefer_shared_depth` (integer, default `0`), `spread_depth` (integer, absent), `spread_min_distinct` (integer, default `1`), `enforcement` (`"Soft"` or `"Hard"`, default `"Soft"`). |
| `provides` | array of tables | empty | Capabilities the group asserts once it has members: `ns`, `name`, `schema_id`, `attrs` (values, as in `[[capability]]`; the same name rules). |
| `requires` | array of tables | empty | Filters the group needs, each with the `[[requirement]]` fields. |

```toml
# examples/units/elastic_intent/coop-operator.toml
[[group]]
name = "rush-pool"
  [group.filter]
  ns   = "rush"
  name = "worker"
```

## `[[lane]]`

A tuple-space stage, by name and side. Lanes match nothing on content.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | required | Lane name, compared literally. Refused when empty. |
| `role` | string | required | `"produces"` or `"consumes"`. |

```toml
# examples/units/llm_council/perish.toml
[[lane]]
name = "spec.perish"
role = "consumes"
```

## `[[mandate]]`

The authority vocabulary a unit expects to hold. Absence is denial and there is no wildcard.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `holder` | string | required | The principal that holds it. Refused when empty. |
| `scope` | string | required | The mandate scope. Refused when empty. |
| `operations` | array of strings | empty | The enumerated operations, in the gateway's `{operation}:{resource}` form. An empty list is refused: it grants nothing. |
| `established_by` | string | absent | Who established the mandate. A record, not checked. |
| `purpose` | string | absent | Why it exists. A record, not checked. |

```toml
# examples/units/a2a_skill_authority/client.toml
[[mandate]]
holder     = "client"
scope      = "depot-remit"
operations = ["skill.invoke:skill:depot/dispatch", "skill.invoke:skill:ledger/export"]
```

## `[[rule]]`

A reference-evaluator rule, as written. `"*"` means any.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `actor` | string | required | The principal the rule admits, or `"*"`. |
| `operation` | string | required | The operation, e.g. `"skill.invoke"`, or `"*"`. |
| `resource` | string | required | The resource, e.g. `"skill:depot/dispatch"`, or `"*"`. |
| `requires_scopes` | array of strings | empty | Scopes the actor must hold for the rule to apply. |
| `requires_facts` | array of strings | empty | Facts the rule needs that the reference evaluator cannot establish (e.g. `budget`); such a rule decides *indeterminate*. |
| `requires_mandate` | string | absent | The scope a live mandate must cover for the rule to apply. |
| `requires_values` | table | empty | Argument values the rule tests by name; the reference evaluator compares for equality. |

`actor`, `operation` and `resource` are refused when empty.

```toml
# examples/units/a2a_skill_authority/gateway.toml
[[rule]]
actor            = "client"
operation        = "skill.invoke"
resource         = "skill:depot/dispatch"
requires_mandate = "depot-remit"
```

## `[hosts]`

Present only for a unit that runs a provisioner. These are the provisioner's own settings.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `kinds` | array of strings | empty | Artifact kinds this host can run: `"wasm-component"`, `"blob"`. Any other is refused. |
| `install_budget_bytes` | integer | absent (no budget) | The most this host installs, in bytes. |
| `headroom` | float | absent (the provisioner's `0.8`) | Fraction of available resources an install may use. Refused outside `(0, 1]`. |
| `trusted_publishers` | array of strings | empty | Publisher keys, `ed25519:<64 hex>`, whose entries this host installs. **Empty is refused** (`hosts.trusted_publishers`): with no key, any entry any peer gossips installs here with no provenance. To run that way on purpose, set `accept_unsigned = true`. A stem refuses a key in any other form. |
| `accept_unsigned` | boolean | `false` | Say on purpose that this host installs entries with no provenance: it lets an empty `trusted_publishers` validate, and the stem warns once at start. Refused beside a non-empty `trusted_publishers` (one or the other names the policy). Never set it on a host that can reach a network any other party can publish into. |
| `placement_root` | string | absent (a stem uses `artifacts`, relative to its working directory) | Where blobs are placed. |
| `fuel_per_call` | integer | absent | Fuel budget, in wasm instructions, for each call into an entry signed by an agent (a trusted key not in `operator_publishers`). `0` is refused. |
| `operator_publishers` | array of strings | empty | The operator's own keys. Each must also be in `trusted_publishers`; refused otherwise, and refused when `trusted_publishers` is empty. |
| `operator_fuel_per_call` | integer | absent (unbounded) | Fuel budget for entries signed by an operator key. `0` is refused. |
| `call_deadline_ms` | integer | absent (the host's `5000`) | Wall-clock bound, in milliseconds, on each call into a hosted component (and on its instantiation), whoever signed it: a call past it is interrupted and recorded as *deadline exceeded*. Unlike fuel it needs no metering and applies to the operator's entries too. `0` is refused. |
| `trusted_reviewers` | array of strings | empty | Reviewer keys whose acceptance promotes a proposed entry from the shadow lane to a real load. Empty means a proposed entry never loads for real here. See [shadow → accept](../guide/00-concepts.md#shadow). |

```toml
# examples/units/model_deploy/model-host.toml
[hosts]
kinds                = ["blob"]
install_budget_bytes = 8589934592
headroom             = 0.8
trusted_publishers   = ["ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12"]
placement_root       = "/var/lib/mycelium/artifacts"
```

That key is the public half of the test seed `42…42` (`make stem-keys`). It is a fixture. Use your
own publisher's key.

**What a hosted component may emit.** A WASM component this host installs reaches the mesh only through
its `mesh.emit` import, and the host confines it (rule `host.emit_admission`): a component providing
`{ns}/{name}` may emit only kinds under `comp/{ns}/…`, and never a protected RPC kind — `mcp.invoke`,
`skill.invoke`, `llm.invoke`, or one the node lists in `protected_rpc_kinds` — because an emit from
inside the node's process would reach that kind's handlers unframed, past the door that checks
authority. A refused emit is dropped, logged and counted as
`mycelium_wasm_host_emits_refused_total{reason}` (`protected_kind`, `foreign_kind`, `malformed`). There is
no unit-file field to grant a component further kinds; that is `HostState::with_emit_kinds` in code.

## `[[presence]]`

Keep between `min_providers` and `max_providers` live providers of a filter across the fleet,
independent of demand. A stem refuses `[[presence]]` without `[hosts]`. A band is supervised by each stem
that declares it in its own unit file, not published to the fleet: a host that does not declare it does
not fill it. The band is a convergence target, not a cardinality guarantee; "provider floor" and
"desired floor" elsewhere mean `min_providers`, the presence floor
([glossary](../guide/00-concepts.md#presence)).

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns`, `name`, `schema_id`, `attrs`, `ranking` | as `[[requirement]]` | — | The filter, written inline. |
| `min_providers` | integer | required | The floor. `0` is refused. |
| `max_providers` | integer | absent (no ceiling) | The ceiling. Refused when below the floor. Above it, the hosts ranked beyond `max` by the band's rendezvous order withdraw — every host with the same view of the providers agrees which (rule `prov.shed` rev 2). Only providers that will act are ranked: a stem advertises `prov-shed/{ns}:{name}:{hash}` for each band it supervises with a ceiling (retracted only while it provides the band without an install it could withdraw), so a provider that does not (registered in code, a stem without this band) keeps its place — once seen unmarked for two advertise intervals; until then it is presumed to shed — and the rest rank against what is left. Avoid overlapping bands with tight ceilings — an unattributed band and an attribute-restricted one over the same capability can trade a provider back and forth. Runnable: `first_stem_fleet` (`mycelium-wasm-host` example, CI) declares a ceiling the run settles to. |

```toml
# examples/units/catalog/late.toml
[[presence]]
ns            = "route"
name          = "optimize"
min_providers = 2
```

## `[[activation]]`

How a placed blob reaches the node's local runtime. After a blob that provides `ns/name` is placed
and verified, the stem runs `command`, then gates the capability on `probe`. The probe is re-run in
the background (every 10 s by default); when it fails, the install is withdrawn and the next round
reinstalls. Activations need `"blob"` in `[hosts].kinds`.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns` | string | required | Namespace of the capability the blob provides. |
| `name` | string | required | Name of the capability the blob provides. |
| `command` | array of strings | required | The argv to run. No shell unless you name one. An empty list is refused. |
| `probe` | array of strings | empty | The argv whose exit 0 means healthy. Empty means *the placed file exists*. The probe has a 30 s timeout. A failing *initial* probe fails the install at stage `activation` — nothing is advertised, the next round retries (2026-10-03). |
| `timeout_secs` | integer | `300` | How long `command` may run. `0` is refused. |
| `resolve_artifact_refs` | boolean | `false` | Write `{rendered}`: a copy of the placed file with every `artifact:<64 hex>` reference replaced by that artifact's placed path. A reference not placed yet fails the activation, and the next round retries. |

Placeholders, in `command` and `probe`:

| Placeholder | Expands to |
|---|---|
| `{path}` | the placed file |
| `{dir}` | the directory it was placed in (the placement root) |
| `{artifact}` | its content address, as hex |
| `{ns}`, `{name}` | the capability it provides |
| `{rendered}` | the rendered copy; only with `resolve_artifact_refs = true` |

Any other placeholder is refused, and so is `{rendered}` without `resolve_artifact_refs = true`.

```toml
# examples/units/model_deploy/model-host.toml
[[activation]]
ns   = "llm"
name = "storyteller"
resolve_artifact_refs = true
command      = ["sh", "/repo/examples/units/model_deploy/ollama-activate.sh", "{rendered}", "coop-storyteller"]
probe        = ["sh", "/repo/examples/units/model_deploy/ollama-probe.sh", "coop-storyteller"]
timeout_secs = 300
```

**Security.** An activation command is the operator's, from a file someone reviewed. **By default it is
unconfined**: the stem runs it, and its probe, with its own privileges — the WASM sandbox covers components,
not these commands. Name an [`[activation.confine]`](#activationconfine) profile to run both under OS
confinement instead. Either way, review unit files as code and run the stem as a least-privileged user:
confinement narrows what a command can reach; it does not make an untrusted command safe to run.

### `[activation.confine]`

The OS confinement an activation's `command` and `probe` run under (post-360 row M). With the table, the
stem runs both under the profile **or refuses the activation by name and runs nothing** — never unconfined.
An empty table is the strictest profile: the placement directory read-only, no network.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `backend` | string | `"auto"` | `auto` (this platform's own), `seatbelt` (macOS) or `landlock` (Linux). A backend this platform does not have refuses the activation. |
| `read` | array of strings | empty | Paths the command may read, beyond the system set and the placement directory. |
| `write` | array of strings | empty | Paths the command may read and write. |
| `connect` | array of strings | empty | TCP destinations, `localhost:<port>` or `*:<port>`. Empty: no network at all (beyond the egress proxy, below). |
| `egress` | table | absent | [`[activation.confine.egress]`](#activationconfineegress): hosts reached **by name** through an egress proxy the stem runs for this activation, and the credential bindings it attaches. |
| `env` | array of strings | empty | Variables passed through from the stem's environment. **A confined command starts with a cleared environment**: `PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR`, these names (each as the stem has it), and with `egress` the proxy's address — nothing else. A name that is a binding's `value_env`, a variable whose value is a binding's value, `NO_PROXY` and the proxy variables refuse the start. |

A path is absolute, or starts with `{dir}` (the placement root — see below) or `{path}` (the placed file); no
other placeholder, and no `..` segment. Each must exist when the activation runs, or the activation is
refused naming it. A `connect` entry naming a hostname or an address is refused at load: neither backend
can limit a connection to one — name the host in [`egress`](#activationconfineegress), which the stem's
egress proxy limits by name. An unknown key in the table, in the `[[activation]]` itself, or at the top
level of the file is refused at load — so neither `[activation.confined]` nor `[activations.confine]` can
load as *no profile* (`deprecations.md` §49).

What each backend allows, besides the profile's own paths and ports:

| | Seatbelt (macOS) | Landlock + seccomp (Linux) |
|---|---|---|
| How | `/usr/bin/sandbox-exec` with a generated profile (`deny default`); paths passed as `-D` parameters | a Landlock ruleset and a seccomp filter on a dedicated thread, inherited by the command |
| Always readable — exactly these | `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`, `/bin`, `/sbin`, `/System`, `/private/var/select`, `/private/var/db/timezone`, `/private/etc/hosts`, `/private/etc/resolv.conf`, `/private/var/run/resolv.conf`, `/private/etc/passwd`, `/private/etc/group`, `/private/etc/localtime`, `/private/etc/ssl/cert.pem`, `/private/etc/ssl/certs`, `/private/etc/ssl/openssl.cnf`, `/dev/null`, `/dev/zero`, `/dev/random`, `/dev/urandom` | `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/lib32`, `/usr/lib64`, `/usr/libx32`, `/usr/libexec`, `/usr/share`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/libx32`, `/etc/ld.so.cache`, `/etc/ld.so.conf`, `/etc/ld.so.conf.d`, `/etc/hosts`, `/etc/resolv.conf`, `/etc/host.conf`, `/etc/gai.conf`, `/etc/nsswitch.conf`, `/etc/passwd`, `/etc/group`, `/etc/localtime`, `/etc/alternatives`, `/etc/ssl/certs`, `/etc/ssl/openssl.cnf`, `/etc/ca-certificates`, and the same devices (each skipped where absent) |
| Always writable | `/dev/null` | `/dev/null` |
| `localhost:<port>` | held | **refused** — Landlock limits a port, not an address; write `*:<port>` |
| `*:<port>` | that port at any address, and the resolver's socket, so a name resolves | that port at any address; no DNS (UDP is refused), so a name resolves only through `/etc/hosts` |
| Other sockets | denied (`deny default`), listening included | every socket but an IP stream over TCP refused, and every socket without `connect`; no `listen`, no TCP Fast Open (`MSG_FASTOPEN` on `sendto`/`sendmsg`/`sendmmsg`, the `TCP_FASTOPEN*` options), no `io_uring`; on x86_64 no x32 syscall |
| Needs | `sandbox-exec` | Landlock ABI 3 (Linux 6.2); ABI 4 (Linux 6.7) with `connect`; x86_64 or aarch64 |

Nothing else in `/etc` (or `/private/etc`), and nothing under `/usr/local`, is readable: the node's own
key, certificates and configuration stay out of reach unless a path names them.

`{dir}` is the **placement root** — the one directory every blob this stem places lands in, by content
address — not a directory of the artifact's own. `write = ["{dir}"]` therefore lets the command rewrite
every other placed artifact; write to a subdirectory you create for it (`{dir}/<name>`) or to a path
outside. A declared path that is a symlink, or reaches its target through a symlink below the placement
root or below a write path (which a confined command could have planted), is refused. Each path is
resolved the first time the activation runs and kept — on Linux held open, so the rules are made from those
descriptors, not re-resolved by name. Before each run, and each re-probe, the stem checks that every kept
path still names the same file (device and inode, not a symlink). A path removed and recreated — a cleaned
scratch directory, a re-placed blob, a directory rotated or a file replaced by an atomic rename — is resolved
again, with every check above, and the run or re-probe goes ahead under the fresh profile; only a resolution
that refuses (the path now a symlink, or gone) fails the re-probe, so the install is withdrawn, the refusal
recorded by name (`prov.probe`'s `confinement_failed`).

Each path is resolved from `/`, one component at a time. A symlink is followed only until the resolution enters the
placement root or a write path, which are recognised by identity, however the path spells them. From there on every
component is opened from its parent without following, so a symlink planted inside, at any component and at any
moment, is refused rather than followed. A `..` there is refused too: name the path directly.

An operator symlink outside every root may still point into one (a shortcut to `out/sub`). Everything past the point
where it enters the root is held to the same rule.

**Mounts.** On Linux a write path's filesystem may be mounted a second time, for example by a bind mount of a
directory inside it. The root's interior then shows under a path that never passes the root itself, so a symlink
there could be one the command wrote. A symlink on a filesystem that holds a write path **and** is mounted more
than once is therefore not followed: the declared path is refused by name. Name the path through the root, or
declare it without a symlink. The check reads `/proc/self/mountinfo`. If that cannot be read, or a device does not
appear there exactly once (btrfs subvolumes can do this), the filesystem is treated as mounted more than once.
In a container, `/etc/hosts`, `/etc/hostname` and `/etc/resolv.conf` are usually bind mounts from the host's disk.
A volume on that same disk therefore counts as mounted more than once. Keep rotation links outside the write volume,
or rotate with `rename(2)` over the old path. macOS has no bind mounts. Not detected, on either platform: a view of a write path through another filesystem (FUSE
`bindfs`, overlayfs, NFS or SMB loopback). Do not reach a declared path through such a view of a write path.

A path reached through an ancestor symlink that is flipped (`/models/current` → `v2`) resolves to the new target.
The profile re-resolves to name that target and no longer names the old one.
The profile names each path's on-disk spelling, so a different case or Unicode normalisation of a path that exists is
accepted.

A probe whose path changes while it runs, the activation's initial probe included, does not trust its verdict. It
resolves and runs once more, and a change under both runs is `confinement_changed`. The activation `command` may
replace its own declared paths; the probe after it re-resolves them.

A rotation that is not atomic (`mv out out.old && mkdir out`), caught between its two steps, finds the path missing.
It is refused, so the install is withdrawn and reinstalled. Rotate with `rename(2)` over the old path, or flip a
symlink above it.

Profiles of placed files that are
gone are forgotten at the next resolution, and a stem keeps at most 1024.

Neither limits reading a path's metadata (`stat`), CPU or memory. The command never inherits the stem's
environment (`env`, above). A credential binding's `value_file` under the placement root, a `read` or `write`
path, or the system set refuses the stem's start, for every confined activation: the command could read it; so
does a `value_file` with a second hard link, and an `env` variable holding a binding's value anywhere in its
text. These are checked once, at start: a file moved or linked into a readable path afterwards is not seen.
Two `[[activation]]`s for one `ns/name` are refused at load. On Linux below Landlock ABI 6, signals to
the stem user's other processes are not limited either. `/opt/homebrew` and `/Applications` are not in the
macOS set: an Ollama installed as an app needs `read = ["/Applications/Ollama.app"]` and its
`command` naming `/Applications/Ollama.app/Contents/Resources/ollama` (the `/usr/local/bin` link is not
readable).

The profile is resolved once, when the activation runs, and its re-probes reuse it (re-resolving a replaced
path, above); the unit file is read
at start, so its filesystem and port limits hold until the stem restarts. The network allow-list **by host**
changes live: it is the egress proxy's (below). Each refusal is recorded as `prov.activation`'s
`confinement_unavailable` (the platform cannot apply the profile), `confinement_failed` (it could, and this
run could not — a declared path missing) or `egress_proxy_unavailable` (its egress proxy could not start).

The stem says which applies. At start it logs each activation as confined (and by which backend),
unconfined, or refused, and `Stem::activation_confinement` returns the same. `mycelium-stem` registers the
guarantee `stem.activation_confined` before it starts, so the guarantee report (`GET /gateway/guarantees`)
reads `enforced` when every activation is confined here, `not_configured` naming *unconfined* or *refused*
otherwise, and `not_applicable` with no `[[activation]]`.

```toml
# A model runtime on macOS: read the placed weights, reach the local Ollama server, nothing else.
[[activation]]
ns   = "llm"
name = "storyteller"
resolve_artifact_refs = true
command = ["/Applications/Ollama.app/Contents/Resources/ollama", "create", "coop-storyteller", "-f", "{rendered}"]
probe   = ["/Applications/Ollama.app/Contents/Resources/ollama", "show", "coop-storyteller"]
  [activation.confine]
  read    = ["/Applications/Ollama.app"]
  connect = ["localhost:11434"]
```

### `[activation.confine.egress]`

Hosts an activation reaches **by name**, and the credentials it may use without holding them (post-360 rows
L and M). With the table, the stem starts an **egress proxy** for this activation when it starts — an HTTP/1.1
forward proxy on `127.0.0.1`, on a free port, kept for the stem's life — and the activation's profile lets its
command and probe connect to that one port. The proxy carries the command's traffic only to `hosts`, under the
node's own egress allow-list, and attaches `credentials` through the node's egress gate.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `hosts` | array of strings | empty | Upstreams, each `https://host[:port]` or `http://host[:port]` — one host name or address, no path, query, userinfo or wildcard; matched exactly (case-insensitive), never as a suffix. Empty: the proxy carries nothing (until the list is set live). An entry the node's `egress.allow_hosts` denies refuses the stem's start. |
| `credentials` | array of strings | empty | Names of the node's [`[[egress_credentials]]`](../operations/crown-jewel.md#credentials-attached-at-egress-post-360-plan-row-l-2350) bindings this activation's requests may name by placeholder. A name that is not a binding on the node refuses the stem's start; a placeholder naming any other binding is refused at the proxy. |

**What the command must do.** Like every confined command it starts with a **cleared environment**
(`[activation.confine] env`, above); with an egress table it also gets `HTTP_PROXY`, `HTTPS_PROXY`,
`ALL_PROXY`, their lower-case forms and `MYCELIUM_EGRESS_PROXY`, each
`http://mycelium:<token>@127.0.0.1:<port>` — the token, drawn per proxy, is required on every request
(`Proxy-Authorization`, which clients send from the URL's userinfo), so no other local process can use the
proxy. Clients that honour the proxy variables (curl, Python's `urllib` and `requests`, Go, most SDKs) need
nothing else. There are two ways through:

| The command asks for | The proxy | A credential |
|---|---|---|
| `http://api.example.com/v1/…` — sent through `HTTP_PROXY` in absolute form (`GET http://api.example.com/v1/… HTTP/1.1`) | matches the host (and the port, when written) to a `hosts` entry and sends the request on over **the entry's scheme** — TLS for an `https://` entry — through the node's egress gate | **attached** — a header carrying `mycelium-credential:<name>` gets the binding's value, only where the binding authorises (its host, port, path prefix, https or loopback); anywhere else the request is refused, 403, and nothing is sent |
| `https://api.example.com/…` — through `HTTPS_PROXY` as `CONNECT api.example.com:443` | opens a byte tunnel to a listed host and port — **except a granted binding's host**, refused 403: reach that one as `http://`, where the answer is checked | **not attached** — the proxy does not terminate TLS, so it cannot see or change a header inside |

So a command that needs a credential writes its URL with `http://` (the hop to the proxy is loopback; the hop
that leaves the host is the entry's `https`), and puts the placeholder where the secret would go:
`Authorization: Bearer mycelium-credential:openai`. The proxy mints no certificate authority and intercepts
no TLS: a key that could impersonate every host is not worth a header the plain-HTTP hop rewrites without it.
A redirect comes back to the command, which follows it through the proxy again — each hop meets the list and
the gate afresh. A refusal is a `403` (`407` without the token, `400` for a request that is not absolute form,
`503` past the buffering cap, `502` for an encoded answer below) with `x-mycelium-egress: refused` and a body
naming why — never a credential's value.

**An echo carries nothing back.** Every answer relayed to the command has the granted bindings' values masked —
in each header and in the body, a value split across chunks included — by the same number of `*`, so
`Content-Length` stays true; values are masked longest first, so one that prefixes another leaves nothing of
the longer. With a value to mask the proxy asks the upstream for an unencoded body (`Accept-Encoding:
identity`) and refuses (`502`) an answer that could carry one encoded — any coding but `identity` in any
`Content-Encoding` header, or a `Transfer-Encoding` naming anything but `chunked` — since a value inside a
compressed body could not be found. An answer with no body (to a `HEAD`, or a 1xx, 204 or 304) is relayed whatever
its `Content-Encoding` says, and repeated `identity` headers are no encoding.

**The list changes live.** `Stem::set_activation_egress_hosts(ns, name, hosts)` replaces an activation's
`hosts` while the stem runs, and `mycelium-stem` re-reads its unit file on `SIGHUP` and applies each
activation's `hosts` the same way. A running proxy whose activation the re-read file no longer names with an
egress table (the table removed, the activation removed or renamed) is narrowed to an **empty** list, logged by
name. Every entry is checked first, as at start; a bad list is refused and the old one stands. The next request
meets the new list; the command, its profile and the proxy's port are unchanged, and an open `CONNECT` tunnel is
not closed. `credentials`, `env` and everything else in the file are read once, at start.

`mycelium-stem` installs its `SIGHUP` handler before the node starts, whatever the unit file names, so a
`SIGHUP` never terminates it: one that arrives during startup is handled once the stem is up, and a stem with no
egress table re-reads the file and logs that there is nothing to apply. A table added to an activation that
started without one is refused by name — its proxy is started only at start, so a new table needs a restart. A
file that does not load leaves every list standing; a file that loads but names a list one proxy refuses (a host
the node's `egress.allow_hosts` denies) leaves that proxy's list standing, logged by activation. **Replace the file
atomically:** write the new file beside it, then `rename(2)` it over the old one (`mv` on one filesystem does
this), so a reload never reads a partial file. A partial file that still parses but has lost an activation's
`[activation.confine.egress]` table is read as that table removed: the proxy is narrowed to an empty list and
refuses everything (fail closed) until the next reload restores it.

**Only this user's processes.** At accept the proxy asks the kernel which user owns the connecting socket and
closes, before it holds any place, a connection another user's process opened — or one whose owner cannot be read
(fail closed; a client that has half-closed is still named, and a dual-stack client reaching `127.0.0.1` as
`::ffff:127.0.0.1`). On Linux it asks `sock_diag` for that one socket by its exact addresses, which the kernel
answers from its connection hash, so the cost does not grow with the host's TCP table; where a netlink socket
cannot be opened (a sandbox that forbids it) it reads `/proc/net/tcp` and `tcp6` whole instead, still failing
closed. On macOS no per-socket query is known: it reads the whole table (`net.inet.tcp.pcblist_n`), so another
user who opens enough idle sockets anywhere on the host can push that read past its bound — every connection is
then refused until the table shrinks (fail closed, never open).
One read of the kernel's table answers every connection accepted before it began, so a flood costs reads per
batch, not per connection, and the command's connection waits about one read; a read that takes longer than 2 s
(on macOS, `sysctl` killed after 1.5 s) refuses its batch and the next read begins. While 64 connections wait to
be named the proxy accepts nothing more: newer connections wait, in arrival order, in the kernel's listen
backlog, and **no waiting connection is closed to make room**. The stem's proxies share one descriptor budget — a
quarter of the process's soft `RLIMIT_NOFILE` (at least 32; the limit is read, not raised) — counting accepted
sockets and the upstream sockets they open; the proxy keeps no idle upstream connection, so every upstream socket
is counted. Each proxy draws on its **share**, the budget divided among the proxies (never under 8: an egress
activation that would push the share below 8 is refused at start, `egress_proxy_unavailable`), and a quarter of
each share is kept for upstream sockets. A proxy at its share, or at 256 accepted sockets, **stops accepting** —
newer connections wait in the backlog, not closed — and a request that finds no upstream descriptor within 10 s is
refused `503`. So a flood on one proxy cannot spend another proxy's descriptors or its own command's upstream, and
no flood can take the stem's descriptor table. Another user on the host can neither use the proxy nor crowd the
command out of it short of filling the listen backlog faster than reads drain it — which costs the command's
client a SYN retry, not a refusal. A read of the kernel's table that has not returned is never doubled: while it
is out, later connections are refused (`peer_uid_unknown`) rather than given a read each.

**Bounded.** Any process of the stem's own user can connect (only the token admits a request), so a connection
that has not presented the token is closed after 2 s, on its `407`, or — past 16 such connections — to make room for a
newer one, so idle token-less connections cannot keep the command out. Connections that have presented it count
against a cap of 64, a `CONNECT` tunnel's for the tunnel's life; an authenticated request past the cap is
refused `503`. The proxy allows 10 s for a request's headers, closes a tunnel that carries nothing for 5 minutes (2 s once
the command has closed its side — so a client that half-closes its write side and waits on an upstream silent
for longer than 2 s is cut off; TLS clients do not half-close),
and buffers at most 4 request bodies of up to 16 MiB at once (a request without a body takes no such place). A
process of the stem's own user that opens token-less connections faster than they are closed can still crowd
the command's connections out — that user is inside the stem's trust boundary already (it can read the stem's
environment and its value files).

**No silent fallback.** A proxy that cannot start (its address in use, no runtime) refuses its activation by
name each time it would run — `prov.activation`'s `egress_proxy_unavailable` — and `Stem::activation_confinement`
reports it refused. A build of the stem without feature `gateway` or `llm` has no egress gate, so a
`credentials` entry refuses the stem's start.

**What it holds, by backend.** Under Seatbelt the command can connect to `localhost:<proxy port>` and
nothing else beyond its own `connect` entries, so every other host is reachable only through the proxy, by
name. Under Landlock, which limits a port and not an address, the command can also open a TCP connection to
**the proxy's port on any address**: the hostname limit holds for everything the proxy carries, and a host
elsewhere listening on that port is reachable directly. `mycelium-stem` registers the guarantee
`stem.activation_egress` before it starts: `enforced` when every activation is confined by a backend that limits
by address and has no `connect` entry — no `*:<port>` (any address on that port) and
no `localhost:<port>` (a local service may forward it anywhere; list it in `hosts` as `http://localhost:<port>`
instead) — and `not_configured` naming which otherwise (an unconfined or refused activation, either kind of
`connect` entry, Landlock).

**Not built:** limits by path (a binding's `path_prefix` limits where its value goes, not where the command
goes); host names compared, not the addresses they resolve to (the node allow-list's own limit); a placeholder
in a URL or a body is not resolved; a credential inside a `CONNECT` tunnel; a value an upstream returns
transformed (re-encoded, split by its own encoding, hashed) is not recognised — only the value's bytes, and for
`basic` its base64 credential, are masked; `credentials` and `env` changing live; HTTP/2 between the command and
the proxy; a proxy for an unconfined activation.

```toml
# A model card fetch: reach one API by name, with the node's key attached at the proxy.
[hosts]
kinds              = ["blob"]
trusted_publishers = ["ed25519:2152f8d19b791d24453242e15f2eab6cb7cffa7b6a5ed30097960e069881db12"]
placement_root     = "/var/lib/mycelium/artifacts"

[[activation]]
ns   = "llm"
name = "tuned"
# http:// to the proxy: the proxy originates TLS to the https:// entry and attaches the key.
command = ["/usr/bin/curl", "-sf", "-o", "/var/lib/mycelium/out/card.json",
           "-H", "Authorization: Bearer mycelium-credential:example-api",
           "http://api.example.com/v1/models/{name}"]
  [activation.confine]
  write = ["/var/lib/mycelium/out"]   # must exist when the activation runs
  [activation.confine.egress]
  hosts       = ["https://api.example.com"]
  credentials = ["example-api"]       # the node's [[egress_credentials]] name = "example-api", host = "api.example.com"
```

This file loads as written (`unit_file_md_egress_example_loads_as_written` reads it from this page); the node
needs the binding, and the write directory must exist, before the activation runs.

## `[[serve]]`

Register the prompt skill `{ns}/{name}` — routable over `llm.invoke`, which an `InferenceRouter`
resolves — backed by an OpenAI-compatible endpoint, **while** this node hosts the install named by
`while_live`. When that install is withdrawn, the skill is retracted, so a router fails over to a
node that still has the model. A stem built without feature `llm` refuses to start when the file
has a `[[serve]]` section. Call it through `POST /gateway/llm/call` (`llm:invoke`; `PromptSkillClient.call`)
or the reason router (`/gateway/reason/route`, `/gateway/reason/v1/chat/completions`); it is not on the
A2A agent card, and `/a2a` `tasks/send` refuses it, naming that route.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns` | string | `"llm"` | Namespace of the skill. |
| `name` | string | required | Name of the skill. |
| `endpoint` | string | required | The OpenAI-compatible base URL, e.g. `http://localhost:11434/v1`. Refused when empty. |
| `model` | string | required | The model name at that endpoint. Refused when empty. |
| `while_live` | table | absent (always) | `ns` and `name` of the install this skill waits for; the skill is served while this node provides that capability. Refused when it names the skill itself: the two share a capability key. Name the install differently, e.g. `storyteller-deploy`. |
| `api_key` | string | `"none"` | The bearer sent to the endpoint, as a literal. One of `api_key` / `api_key_env` / `credential`. |
| `api_key_env` | string | absent | The **name** of an environment variable holding the bearer, read once at stem start. An unset variable refuses the start, naming the section and the field (not the variable — a key pasted there would be logged; since row L); an empty name is refused by the loader. |
| `credential` | string | absent | The **name of a credential binding** on the node (`[[egress_credentials]]` in its config — [crown-jewel.md § Credentials attached at egress](../operations/crown-jewel.md)). The stem holds only the binding's placeholder; the node's egress gate attaches the value on a request to the binding's host and nowhere else. A binding the node does not have, or one that does not authorise `endpoint`, refuses the start, naming it; a name outside `[A-Za-z0-9._-]` is refused by the loader. Post-360 plan row L, 2.35.0. |
| `max_tokens` | integer | `256` | The template's token limit. |
| `temperature` | float | `0.7` | The template's temperature. |

```toml
# examples/units/reheal_deploy/survivor.toml
[[serve]]
name       = "storyteller"
endpoint   = "http://172.40.0.56:11434/v1"
model      = "coop-storyteller"
max_tokens = 48
# api_key_env = "COOP_INFERENCE_KEY"   # a keyed endpoint names its variable; a local Ollama needs none
[serve.while_live]
ns   = "llm"
name = "storyteller-deploy"
```

**Keys.** A keyed endpoint names its variable — `api_key_env = "OPENAI_API_KEY"` — and the stem reads it
once at start, so the unit file carries no secret and stays safe to commit
([capability-lifecycle.md](../operations/capability-lifecycle.md) §1). A host where the variable is unset
refuses to start naming the section, rather than serving a skill with no key. `api_key` remains the literal form
for a file that is not committed (a local Ollama needs neither). The key is never logged, traced or reported.
With `credential = "openai"` the value never reaches the stem's side at all: the node reads it once, from a
variable or a file its `[[egress_credentials]]` binding names, and attaches it only to a request for the
bound host — a redirect off that host carries nothing.

## Checking a file

```sh
cargo run --features cli --bin mycelium -- wire-check <units-dir> --library <artifacts-dir>
```

A file that does not load makes the check exit 2 and names the field. The findings, flags and exit
codes are in [capability-lifecycle.md](../operations/capability-lifecycle.md) §2.

## Tool components

A component whose description provides `ns = "tool"` (kind `wasm-component`) is also an **MCP tool**
on the node that hosts it: a stem built with `gateway` registers `{name}` under `tools/{name}/{node}`
in KV and forwards `tools/call` to the component over `mcp.invoke`, and the tool disappears with the
install. A component that answers kind `describe` with `{"description": …, "inputSchema": …}`
publishes **its own** schema; one that does not gets the generic `{"type": "object"}`. There is no
tool section in the unit file — the artifact description is the declaration (zero-gaps Z4;
`examples/units/llm_agent/artifacts/tool-*.toml`, `mycelium-wasm-host/tests/fixtures/*-tool-component/`).
