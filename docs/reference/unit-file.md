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
| `[[presence]]` | `presence unhostable` | a standing want (needs `[hosts]`) | refused, **422** |
| `[[activation]]` | not read | run after a blob is placed | refused, **422** |
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
| `ns` | string | required | Capability namespace, e.g. `"llm"`. |
| `name` | string | required | Capability name, e.g. `"inference"`. |
| `probe_url` | string | absent | HTTP GET probed for liveness; a 2xx keeps the capability advertised. Absent means always-alive. Honoured by `run_capability_probes`, not by a stem. |
| `probe_timeout_secs` | integer | `3` | Probe request timeout, in seconds. |
| `ttl_secs` | integer | `30` | Re-assertion TTL of the advertisement, in seconds. |
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
| `ns` | string | required | Namespace to match. |
| `name` | string | required | Name to match. |
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

A capability group the unit defines: the fields of `CapabilityGroupDef` plus a name.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | required | Group name. Refused when empty. |
| `filter` | table | required | Who may join: the same fields as a `[[requirement]]` (`ns`, `name`, `schema_id`, `attrs`, `ranking`). |
| `topology_policy` | table | absent | How a group's quorum spreads across locality levels: `prefer_shared_depth` (integer, default `0`), `spread_depth` (integer, absent), `spread_min_distinct` (integer, default `1`), `enforcement` (`"Soft"` or `"Hard"`, default `"Soft"`). |
| `provides` | array of tables | empty | Capabilities the group asserts once it has members: `ns`, `name`, `schema_id`, `attrs` (values, as in `[[capability]]`). |
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
| `trusted_publishers` | array of strings | empty | Publisher keys, `ed25519:<64 hex>`, whose entries this host installs. Empty means provenance is not required. A stem refuses a key in any other form. |
| `placement_root` | string | absent (a stem uses `artifacts`, relative to its working directory) | Where blobs are placed. |
| `fuel_per_call` | integer | absent | Fuel budget, in wasm instructions, for each call into an entry signed by an agent (a trusted key not in `operator_publishers`). `0` is refused. |
| `operator_publishers` | array of strings | empty | The operator's own keys. Each must also be in `trusted_publishers`; refused otherwise, and refused when `trusted_publishers` is empty. |
| `operator_fuel_per_call` | integer | absent (unbounded) | Fuel budget for entries signed by an operator key. `0` is refused. |
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

## `[[presence]]`

Keep between `min_providers` and `max_providers` live providers of a filter across the fleet,
independent of demand. A stem refuses `[[presence]]` without `[hosts]`.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `ns`, `name`, `schema_id`, `attrs`, `ranking` | as `[[requirement]]` | — | The filter, written inline. |
| `min_providers` | integer | required | The floor. `0` is refused. |
| `max_providers` | integer | absent (no ceiling) | The ceiling. Refused when below the floor. |

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

**Security.** An activation command is the operator's. The stem runs it with its own privileges,
from a file someone reviewed. It is **not a sandbox**: there is no allowlist and no confinement.
The WASM sandbox covers components, not these commands. Review unit files as code and run the
stem as a least-privileged user.

## `[[serve]]`

Register the prompt skill `{ns}/{name}` — routable over `llm.invoke`, which an `InferenceRouter`
resolves — backed by an OpenAI-compatible endpoint, **while** this node hosts the install named by
`while_live`. When that install is withdrawn, the skill is retracted, so a router fails over to a
node that still has the model. A stem built without feature `llm` refuses to start when the file
has a `[[serve]]` section.

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
