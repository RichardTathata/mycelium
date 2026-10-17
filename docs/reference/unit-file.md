# The unit file — reference

↑ [Docs](../README.md) · workflow: [capability-lifecycle.md](../operations/capability-lifecycle.md) ·
summary: [guide 02](../guide/02-capabilities.md) · code: `src/capability_config.rs`

A **unit file** is one TOML file per deployable unit. It says what the unit offers, what it needs,
the groups it defines, the lanes it touches, the authority it expects, what it can host, and how a
hosted blob reaches its local runtime. Every section is optional. A file with only `[[capability]]`
blocks is a valid unit file.

The loader is `NodeCapabilityConfig::from_toml_str` (or `load_from_file`): it parses the file and
then runs `NodeCapabilityConfig::validate`, which refuses a bad value **by name**. The same loader
is used by `mycelium wire-check`, by the stem, and by `POST /gateway/units/declare`. There is no
second parser.

TOML needs one key per line. `ns = "a"; name = "b"` is not valid TOML.

## Who reads which section

| Section | `mycelium wire-check` | Stem (`mycelium-stem --units`) | SDK agent (`POST /gateway/units/declare`) |
|---|---|---|---|
| `principal` | carried per unit in the JSON, the join key | validated, not otherwise used | echoed back in the response |
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
| `trusted_reviewers` | array of strings | empty | Reviewer keys whose acceptance promotes a proposed entry from the shadow lane to a real load. Empty means a proposed entry never loads for real here. |

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
independent of demand. A stem refuses `[[presence]]` without `[hosts]`.

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
| `connect` | array of strings | empty | TCP destinations, `localhost:<port>` or `*:<port>`. Empty: no network at all. |

A path is absolute, or starts with `{dir}` (the placement root — see below) or `{path}` (the placed file); no
other placeholder, and no `..` segment. Each must exist when the activation runs, or the activation is
refused naming it. A `connect` entry naming a hostname or an address is refused at load: neither backend
can limit a connection to one. An unknown key in the table — or in the `[[activation]]` itself, so a
misspelt `[activation.confined]` cannot load as *no profile* — is refused at load.

What each backend allows, besides the profile's own paths and ports:

| | Seatbelt (macOS) | Landlock + seccomp (Linux) |
|---|---|---|
| How | `/usr/bin/sandbox-exec` with a generated profile (`deny default`); paths passed as `-D` parameters | a Landlock ruleset and a seccomp filter on a dedicated thread, inherited by the command |
| Always readable — exactly these | `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/libexec`, `/usr/share`, `/bin`, `/sbin`, `/System`, `/private/var/select`, `/private/var/db/timezone`, `/private/etc/hosts`, `/private/etc/resolv.conf`, `/private/var/run/resolv.conf`, `/private/etc/passwd`, `/private/etc/group`, `/private/etc/localtime`, `/private/etc/ssl`, `/dev/null`, `/dev/zero`, `/dev/random`, `/dev/urandom` | `/usr/bin`, `/usr/sbin`, `/usr/lib`, `/usr/lib32`, `/usr/lib64`, `/usr/libx32`, `/usr/libexec`, `/usr/share`, `/bin`, `/sbin`, `/lib`, `/lib32`, `/lib64`, `/libx32`, `/etc/ld.so.cache`, `/etc/ld.so.conf`, `/etc/ld.so.conf.d`, `/etc/hosts`, `/etc/resolv.conf`, `/etc/host.conf`, `/etc/gai.conf`, `/etc/nsswitch.conf`, `/etc/passwd`, `/etc/group`, `/etc/localtime`, `/etc/alternatives`, `/etc/ssl/certs`, `/etc/ssl/openssl.cnf`, `/etc/ca-certificates`, and the same devices (each skipped where absent) |
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
resolved once, the first time the activation runs, and kept for the stem's life — on Linux held open, so
the rules are made from those descriptors and never re-resolved by name.

Neither limits reading a path's metadata (`stat`), CPU or memory, and the command inherits the stem's
environment (row L's credential bindings are the way to keep a secret out of it). On Linux below Landlock ABI 6, signals to
the stem user's other processes are not limited either. `/opt/homebrew` and `/Applications` are not in the
macOS set: an Ollama installed as an app needs `read = ["/Applications/Ollama.app"]` and its
`command` naming `/Applications/Ollama.app/Contents/Resources/ollama` (the `/usr/local/bin` link is not
readable).

The profile is resolved once, when the activation runs, and its re-probes reuse it; the unit file is read
at start, so every limit holds until the stem restarts. A network allow-list that changes live is **not
built**. Each refusal is recorded as `prov.activation`'s `confinement_unavailable` (the platform cannot apply
the profile) or `confinement_failed` (it could, and this run could not — a declared path missing).

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
| `api_key` | string | `"none"` | The bearer sent to the endpoint, as a literal. One of `api_key` / `api_key_env`, never both. |
| `api_key_env` | string | absent | The **name** of an environment variable holding the bearer, read once at stem start. An unset variable refuses the start, naming it; an empty name is refused by the loader. |
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
refuses to start naming it, rather than serving a skill with no key. `api_key` remains the literal form
for a file that is not committed (a local Ollama needs neither). The key is never logged, traced or reported.

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
