# Design-time tooling: declarations and the offline wire-check (plan)

**Status:** proposed, rev 0.3, 2026-09-27 (rev 0.2 added D9, W6 and §9, the declaration as a consumer record; rev 0.3 added §10, registering an artifact — D10, A1–A3 — and §11, the recorded questions). Nothing here is built. This plan argues the declaration
format once so the code that follows does not re-argue it. It is additive on v2.16.0: no wire change, no
new KV namespace, no runtime behaviour change.

> Posture, once: at design time you fix the **vocabulary** — what can match what, and who may do
> what. At runtime the mesh decides, continuously, who actually does. Today the vocabulary is
> scattered through `main.rs` files and the only way to learn that two units do not wire is to deploy
> them and read the opacity entries. This plan writes the vocabulary down and checks it before
> deployment, and it never claims more than *would bind under these declarations*.

---

## 1. What exists (checked in the code, 2026-09-27)

| # | Fact | Evidence |
|---|---|---|
| E1 | A node can declare what it **offers** in a file: `[[capability]]` blocks with `ns`, `name`, an optional `probe_url`, a TTL and static `attrs`. Nothing else is declarable in a file. | `src/capability_config.rs` (`NodeCapabilityConfig`, `CapabilityProbeEntry`, `TomlCapValue`) |
| E2 | What a node **requires** is code only: `declare_requirement(CapFilter, interval)` writes `req/{node}/{ns}/{name}` and, while unsatisfied, `sys/load/{node}/req/{ns}/{name}`. | `src/agent/capability_handle.rs:232`, `:238–241` |
| E3 | A **group** is code only: `define_capability_group(name, CapabilityGroupDef, interval)` writes `cap-group/{name}`; the def carries a `filter`, `provides` and `requires`. | `src/agent/capability_handle.rs:359`, `:369`; `src/capability.rs:217` |
| E4 | The match rule is one function, `CapFilter::matches`: same `(ns, name)`, every constraint satisfied by a *present* attribute, exact `schema_id` when set. Type-cross comparisons never match. A schema-only mismatch is told apart by `matches_ignoring_schema`. | `src/capability.rs:360`, `:378` |
| E5 | The runtime resolver, `wiring_snapshot`, is that rule applied over `cap/` and `gcap/` **plus freshness** (`is_fresh` against the entry's refresh interval) and an optional ranking. | `src/agent/wiring.rs:99` |
| E6 | Lanes are strings: a pipeline position is the lane an item sits in; `put(stage, …)`, `take(stage, …)`, `complete(…)`. Nothing declares which unit feeds or drains a lane. | `mycelium-tuple-space/src/lib.rs:1145`, `:1195`, `:1224`; guide 07 §Stages are lanes |
| E7 | Authority is declared in code: a `Mandate` enumerates `operations` (no wildcard, absence is denial) over a `scope`; a reference-evaluator `Rule` names `actor`/`operation`/`resource` with `*` for any, plus required scopes, facts, a mandate scope and argument values. | `src/mandate.rs:134`; `src/agent/action_evaluator.rs:1188` |
| E8 | Schemas already have a design-time gate: a git-first directory seeded at startup and a CI step that fails on conflict. It is the only place a declaration is checked before it reaches the mesh. | guide 12 §CI / CD gate |
| E9 | Every view of wiring is live: `resolve_wiring` / `watch_wiring` on a running node, the two `*_viz` examples, the ops console, replay of a recording. None answers a question about a deployment that has not started. | `src/agent/capability_handle.rs:384`, `:427`; `examples/coordination_viz.rs` |
| E10 | The node binary parses `-c/--config`, `-p`, `--host`, `-r`, `-i`; it has no subcommands. It requires feature `cli`; `toml` is already a dependency. | `src/main.rs:45`; `Cargo.toml:17`, `:175` |

**What this means.** The vocabulary exists in the types (E2–E4, E7) and the matching is a pure function
(E4). What is missing is a *file* that carries the whole vocabulary of a unit and a *check* that applies
E4 to a set of such files. E8 shows the shape that works here: git-first, fails the build, no daemon.

---

## 2. The line this plan draws

Design time fixes: capability names and attribute vocabularies; requirements (filters with schema
and constraints); group definitions; lane names and which units feed or drain them; the authority
vocabulary (mandate operations and scopes, rules). Runtime decides: which node serves, how many, the
ranking, group membership, where workers mass, provisioning, whether an intent is still asserted.

The checker therefore answers exactly one question: **given these declarations, is there any assignment
under which each requirement, group, lane and authority edge can bind?** It never answers whether it *is*
bound. That is a runtime fact, and the runtime already reports it (E2's opacity entries, E9's views).

---

## 3. The declaration format (decided here)

**D1 — extend the existing TOML file, do not add a second format.** `NodeCapabilityConfig` is already
loaded from TOML, is Rust-native, carries comments, and its `[[capability]]` array-of-tables shape
generalises directly. A second format (YAML, JSON) would put two sources of the same vocabulary on
disk. JSON is emitted as *output* (§4), never read as input.

**D2 — one file describes one unit; a deployment is a directory of units.** A *unit* is the thing an
operator deploys as one: a node binary with its co-located services, or an SDK agent. The file's name
is the unit's name. The checker takes a directory (or a list of files) and nothing else.

**D3 — a filter is written as attributes with operators; a bare value is equality.** This mirrors
`CapConstraint` exactly (E4) and adds nothing to it.

```toml
# units/planner.toml — what this unit offers, needs, defines and may do.
principal = "planner"                # D9 — the principal this unit presents; the join key

[[capability]]                       # unchanged from today (E1)
ns   = "plan"
name = "route"
ttl_secs = 30
  [capability.attrs]
  region = "north"
  version = "2.1.0"                  # parses to CapValue::Version

[[requirement]]                      # new — declare_requirement, in a file
ns   = "llm"
name = "inference"
schema_id = "llm.inference.v2"       # exact match, as CapFilter::schema_id
  [requirement.attrs]
  model   = "llama3.2"               # bare value = Eq
  context = { gte = 8192 }           # gt | gte | lt | lte | ne
  [requirement.ranking]
  attribute = "context"
  order = "descending"

[[group]]                            # new — define_capability_group, in a file
name = "routers"
  [group.filter]
  ns = "plan"; name = "route"
  [[group.provides]]
  ns = "plan"; name = "routing"
  [[group.requires]]
  ns = "data"; name = "realtime"

[[lane]]                             # new — a tuple-space stage this unit touches
name = "stage-b"
role = "consumes"                    # produces | consumes

[[mandate]]                          # new — the authority vocabulary this unit expects
holder = "planner"
scope  = "routers"
operations = ["plan.route", "plan.reroute"]

[[rule]]                             # new — reference-evaluator rules, same fields as Rule
actor = "planner"; operation = "plan.route"; resource = "*"
requires_mandate = "routers"
```

**D4 — a unit file loads with the loader the node uses.** `NodeCapabilityConfig` gains the new arrays with
`#[serde(default)]`, so every existing file still loads unchanged, and `run_capability_probes` keeps
reading only `capabilities`. The new sections become *usable at runtime* in a later step (a unit that
declares `[[requirement]]` could call `declare_requirement` from the file), but that is not this plan's
deliverable and is recorded as D8.

**D5 — lanes are declared by name and role only.** The tuple space matches nothing on content (E6), so
the check is a name join: every `consumes` has a `produces` somewhere and vice versa. Encoded
dimensions (`stage-b.tenant-42`) are the author's business; the checker compares names literally.

**D6 — authority is declared in the unit that expects it, and checked as reachability.** The unit
says what mandate scope and operations it will present and what rules it expects. The overlay (§4,
W4) asks, per wired edge, whether any declared rule admits the requiring unit's principal for that
operation with a declared mandate covering it. It does not evaluate policy; it finds edges that no
declaration could ever authorise.

**D7 — the file carries no secrets and no addresses.** Tokens, peers and ports stay in `GossipConfig`.
A unit file is safe to commit.

**D9 — a unit names its principal, and the checker's JSON names its revision** *(rev 0.2)*. The file
gains one top-level field, `principal = "planner"`, the issuer-qualified principal the unit will present
at a gateway (the same value `GatewayCaller` resolves at runtime). It is not an address: it is the one
key an observation record also carries. The JSON output (§4) carries `revision`, the git commit of the
units directory, mirroring the policy revision the gateway stamps on every decision
(`set_deployed_policy_revision`). Without these two, the output cannot be joined to anything the fleet
later reports (§9).

---

## 4. The checker: `mycelium wire-check`

A subcommand of the existing binary (feature `cli`, E10). Input: a directory of unit files. It runs
`CapFilter::matches` (E4), the same function the mesh runs, minus freshness and locality, which are
runtime facts.

**What it reports, in order of severity**

| Finding | Meaning | Exit |
|---|---|---|
| `unwired requirement` | no declared capability, in any unit, satisfies the filter | 1 |
| `schema-only mismatch` | the filter matches except for `schema_id` (E4's `matches_ignoring_schema`), named as such, with the versions on both sides | 1 |
| `empty group` | no declared capability satisfies the group's `filter`, so nothing could join | 1 |
| `group requires unmet` | a group's `requires` filter binds to nothing | 1 |
| `orphan lane` | a lane consumed and never produced, or produced and never consumed | 1 |
| `unauthorisable edge` (W4) | a wired edge for which no declared mandate enumerates the operation, or no rule admits it | 1 |
| `type-cross constraint` | a constraint whose value type can never compare with the offered attribute's type (E4 says this silently never matches) | 1 |
| `unranked ranking` | a ranking on an attribute no matching capability carries | warning |
| `single provider` | a requirement with exactly one possible provider across the deployment | warning |

**Output.** Text by default; `--format json` emits the resolved graph (units with their principals,
offers, requirements, edges, lanes, authority edges, findings) under a **versioned document schema**,
`mycelium.design/declaration/1`, with `revision` (D9) and the schema id in the envelope; `--format dot`
emits Graphviz. Exit 0 with no errors, 1 with any error, 2 for a file that does not load. The wording
in every line is *would bind* / *could not bind*. The JSON is a document a consumer reads (§9), so it
is pinned like the evidence schema: a change to its shape is a schema version, not an edit.

**CI recipe** (lands in guide 12's section, beside the schema gate):

```yaml
- name: Wire-check the deployment
  run: cargo run --features cli --bin mycelium -- wire-check ./units --format text
```

**What it cannot say, stated in its own `--help`:** whether a provider is alive (freshness), which one
is chosen (ranking over runtime attributes, locality), whether a group has members *now*, whether
provisioning would fill a gap, whether a mandate is current or revoked, or anything about load.

---

## 5. Phases and exit gates

Each phase ships with its regression test seen failing first, per the repository's bar.

| Phase | Deliverable | Exit gate |
|---|---|---|
| **W1** | The format: `NodeCapabilityConfig` gains `requirements`, `groups`, `lanes`, `mandates`, `rules` with `#[serde(default)]`; conversions to `CapFilter`, `CapabilityGroupDef`, `Mandate`-shaped and `Rule`-shaped values; loader rejects an unknown operator and a `Version` that does not parse | Every existing `[[capability]]` file loads byte-for-byte unchanged (pinned by a fixture); the §3 example loads and round-trips through `CapFilter::matches` against a hand-built `Capability` |
| **W2** | The checker over a directory, the six error findings and two warnings above, text and JSON output (the `mycelium.design/declaration/1` document with `principal` per unit and `revision` in the envelope, D9), the subcommand, exit codes; the coop demos' units written as the first fixture directory under `tests/fixtures/units/` | A fixture with one deliberately unwired requirement exits 1 naming it; the coop fixture exits 0; CI runs both; the JSON of the coop fixture is a golden file, so a shape change is a visible diff |
| **W3** | Schema awareness: the checker reads the same schema directory guide 12 seeds, so a `schema_id` that names no file is an error and a provider/consumer version split is reported as the rollout-window case | A fixture holding `v1` providers and a `v2` requirement reports `schema-only mismatch` with both ids |
| **W4** | The authority overlay (D6): per wired edge, reachability through declared mandates and rules; `--no-authority` to skip | A fixture whose only rule requires a mandate scope no unit declares reports `unauthorisable edge`; the `procurement_authority` example's vocabulary written as a fixture passes |
| **W5** | `--format dot`; a paragraph in guide 12 and one in `docs/operations/deployment.md`; a row in `examples/README.md` under the tutorial contract | The coop fixture renders; `/doc-coverage` gains a HOW·Ops cell for *deployment wiring* that opens the runbook |
| **W6** *(rev 0.2)* | The declaration as a consumer record (§9): the schema pinned in the private `COMPATIBILITY.md` beside the evidence schema; the private exporter ships a `deployment_declaration` batch next to `policy_deployment`, same signing, same batch identity, same byte-identical-under-one-id rule; the operation names in the JSON's authority edges pass through the exporter's reviewed catalogue so an unmapped one exports as `unmapped`, never as a guess | Public: the golden JSON from W2 validates against the pinned schema. Private: a retry of a declaration batch returns the remembered bytes; an observation joined to a declared edge by (principal, ns, name, schema_id) resolves to exactly one edge on the coop fixture; the consumer's rendering is on their side of the handover and is not a gate here |

W1 and W2 are the value; W3–W5 are each a day and can stop after any one of them without leaving a
half-feature, because each is a finding class added to a working checker. W6 is private work on the
exporter and depends only on W2's document; it can run in parallel with W3–W5.

---

## 6. Constraints this plan holds

- **A checker over files, not a designer and not a deployment engine.** It reads a directory and
  prints. It does not start nodes, does not write to any mesh, does not know addresses (D7). A cluster
  stays emergent from reachability; this tool describes a vocabulary. The library-not-platform line is
  the reason the output says *would bind*.
- **No new namespace, no runtime dependence.** The runtime does not read the checker's output, and the
  checker does not read the runtime's state. If a later step wants the two views on one picture (W5's
  motivation), it is the *viz* that consumes the checker's JSON, never the reverse.
- **One match rule.** The checker calls `CapFilter::matches`; it does not reimplement it. If the rule
  changes, the checker changes with it, and a test pins that the two agree on the §3 example.
- **Detection, not prevention.** A red wire-check stops a merge; it does not stop a deployment that
  bypasses CI, and nothing in the substrate refuses a unit whose file was never checked.

---

## 7. Decision register

| # | Decision | Alternatives declined, and why |
|---|---|---|
| D1 | Extend the existing TOML | YAML (a second format for one vocabulary); JSON as input (no comments, and the file is authored by hand) |
| D2 | One file per unit, a directory per deployment | One file per deployment (a unit's declaration then lives apart from the unit it describes, and two deployments of one unit duplicate it) |
| D3 | Operators as inline tables, bare value = Eq | A string mini-language (`"context >= 8192"`) — a second parser for six operators that `CapConstraint` already names |
| D4 | The node's own loader gains the sections | A separate `mycelium-design` crate — it would need the same types and would drift from them |
| D5 | Lanes by name and role | Declaring lane *content* — the space matches nothing on content (E6), so a declaration would describe something the runtime never checks |
| D6 | Authority as reachability over declared rules | Running the evaluator offline — an evaluator's answer depends on envelope facts (principal, scopes, arguments) that exist only per call |
| D7 | No secrets, no addresses | A single file for everything — it could not be committed, and the point is a checked-in vocabulary |
| D8 | Runtime use of the new sections is out of scope | Folding it in — it changes node startup behaviour and belongs in its own plan with its own gate |
| D10 *(rev 0.3)* | The signed line-hex manifest stays the library's truth; a reviewable TOML description is the input and a command derives the one from the other | Changing the manifest format (re-opens a shipped, signed record); a readable manifest with no derivation check (two sources of truth that drift) |
| D9 *(rev 0.2)* | The JSON is a versioned, revisioned document with a per-unit principal, so a consumer can join it to runtime records | Human-only output (then a consumer would parse an unstable shape); joining on node id (D7 forbids it, and a node id is not stable across redeploys, whereas a principal is what the evidence already names) |

---

## 8. Where it connects: the design-time half of composition

This plan is part of the composition theme, and it is the half that had no home. The theme as
recorded in `v3-contracts-axis.md` §13 is about **runtime records**: a commitment is a declared
requirement, an authorised acceptance, a mandate, a resource allocation, and the receipts and
assessments that follow, linked by a correlation record, with nobody assigning another participant's
obligations and no planner. `docs/design/composed-effect.md` then made one composed sentence
*reconstructable* from one execution record. Both halves live after deployment.

What neither states is the **vocabulary** those records are written in: which requirement can name
which capability, under which schema, which mandate scope and operations a unit will present, which
rules could admit it, and which lanes carry the work between them. Today that vocabulary is implicit in
code and is only discovered to be inconsistent when a record fails to appear. This plan writes it
down (§3) and checks it (§4). So:

- **§13's records are runtime; this file is design time.** The file says a requirement *could* be
  accepted and authorised; the acceptance, the mandate's currency and the receipt remain runtime
  records, exactly as §13.2 requires. D6 is the design-time face of §13.2's five records.
- **If the recorded question is ever answered with an *enforced* composition**, its declaration form is
  this file and this checker is where it is checked before deployment. That decision is not taken
  here; §13 says the question opens the next epoch, and this plan does not pre-empt it.
- **What this plan does not do for composition:** it does not make the arrangement of work a
  design-time artefact. The arrangement stays *observable through the records* (§13.2); this plan only
  makes sure the records have a consistent vocabulary to be written in.
- **Guide 12**'s schema gate (E8) is the precedent; W3 makes the two gates read one directory.
- **`examples/README.md`**'s tutorial contract: W2's coop fixture is the worked example, and the
  checker's DOT output is the picture the guide has never had of how the coop demos wire.

---

## 9. The declaration as a fleet target-state record *(rev 0.2)*

The evidence exporter already gives an external consumer (NovusLens, in the private companion) two
kinds of runtime record: `activity_observation` — what ran, under which decision, on which route, with
coverage never claimed complete — and `policy_deployment` — which policy revision was live when. Both
are facts about what happened. Nothing says what the fleet was *supposed* to look like, so the consumer
can show activity but not drift.

The checker's JSON is that missing record. Shipped as a third kind, `deployment_declaration`, the same
way `policy_deployment` is (W6), it lets a consumer render three comparisons it cannot render today:

- **Declared versus observed.** Every edge the checker said would bind, against whether any
  observation traversed it. An edge with no observations is idle or broken, and the runtime's own
  opacity entry (E2) says which.
- **Observed but undeclared.** A call between two principals that no declaration wired. That is the
  interesting finding, and under *detection, not prevention* it is a report, never a block.
- **Authority coverage.** W4's `unauthorisable edge` findings beside the evaluator's actual deny
  records, so a reader sees what the design excluded before the evaluator refused it.

**The join.** A unit file has no node ids or addresses (D7), so the join runs on what both sides carry:
the principal (D9) on the authority side, and `(ns, name, schema_id)` on the wiring side; an operation
name goes through the exporter's reviewed catalogue so it means one thing in both records. **The
revision** (D9) tells the consumer which declaration each observation is compared against, exactly as
the policy revision does for decisions.

**The sentence on the record.** The declaration says *would bind*. A view built on it says
**declared**, never *desired and enforced*: nothing makes the fleet conform to the file, and the
comparison is a report, not a control surface. A consumer that presents it as a target the substrate
enforces has misread it, and the record's own schema name is chosen to make that harder.

## 10. Adjacent, and in scope: registering an artifact *(rev 0.3)*

The provisioner (`mycelium-wasm-host/src/provisioner.rs`) can fill a requirement no deployed unit
offers by installing from the catalogue, and a catalogue entry's `provides` is a full `Capability`
chosen so `CapFilter::matches` works unchanged against it (`mycelium-wasm-host/src/catalog.rs:55`).
So the catalogue is a design-time input to the checker. Reading it exposed two gaps in how an artifact
gets there, both design-time and both in this plan's territory:

| # | Gap | Evidence |
|---|---|---|
| G1 | **Publishing is a Rust program.** The production path (`docs/operations/artifacts.md` §2) is `FsLibrarySource::store` → `InstallableEntry::new(…).with_kind(…).with_requirements(…).signed_by(&key)` → `Manifest::append_entry`, written as CI code. There is no `mycelium` subcommand and no gateway route (`grep '"/gateway/artifact' src/agent/http.rs` is empty), so a Python or TypeScript team cannot register an artifact without writing Rust | `mycelium-wasm-host/src/catalog.rs:141`, `:344`; `src/agent/http.rs` route table |
| G2 | **The manifest cannot be reviewed.** It is one hex line per bincode-encoded entry (`Manifest::parse`), the library's source of truth and a signed record — correct as a format for the librarian, unreadable in a pull request. A reviewer cannot see what capability, kind or footprint a line declares | `mycelium-wasm-host/src/catalog.rs:297` |

**D10 — the signed manifest stays the truth; a readable description is the input, and the tool derives
one from the other.** Changing the manifest's format would re-open a shipped, signed record. Instead an
artifact is *described* in a TOML file a reviewer reads, and a command turns the description plus the
bytes plus a key into the manifest line. The description is committed beside the unit files; the
manifest is regenerated and checked against it in CI, the way a lock file is.

```toml
# artifacts/route-optimizer.toml — what this artifact would provide once installed
kind = "wasm-component"              # wasm-component | blob
bytes = "build/route_optimizer.wasm" # relative; the content address is computed, never written
est_install_secs = 1
  [provides]                         # a Capability, the same shape as [[capability]] in a unit
  ns = "route"; name = "optimize"
  [provides.attrs]
  version = "1.4.0"
  [requires]                         # the signed footprint (artifacts.md §2)
  disk_bytes = 0
  mem_bytes  = 67108864
```

| Phase | Deliverable | Exit gate |
|---|---|---|
| **A1** | `mycelium artifact publish <description.toml> --library <dir> --key <file>`: stores the bytes, builds and signs the entry, appends the manifest line; `mycelium artifact list <dir>` renders a manifest as the same TOML shape with the content address and signer shown; `mycelium artifact verify <dir> --trusted <key>…` runs `verify_provenance` over every line | `list` of a manifest written by `publish` reproduces the description (golden fixture); a description whose `bytes` changed under an unchanged manifest fails `verify` in CI, seen failing first |
| **A2** | The checker reads a library directory (`--library <dir>`) and reports **`would bind by provisioning`** for a requirement no unit offers but a manifest entry's `provides` matches — a separate class, a warning not an error, naming the entry and its footprint; with `--strict-deployed` it is an error | The coop fixture with the `catalog` demo's manifest turns one `unwired requirement` into `would bind by provisioning`; the JSON document carries the entry's content address on that edge |
| **A3** | `POST /gateway/artifacts/publish` behind a new scope family `artifact:publish`: the body is an **already-signed** entry (signing stays with the publisher's key, which never reaches a gateway) that the route verifies against the node's trusted publisher keys and writes with `publish_installable`; the bytes are not uploaded through the gateway — they are at a blob store the librarian mirrors (`HttpLibrarySource`, artifacts.md §2). SDK verbs in `mycelium-py` / `mycelium-ts` that build and sign the entry client-side | An unsigned or untrusted entry is refused 403 by name; a signed one appears under `installable/` on a second node; the route is in the scope table and the bypass matrix's plant covers it |

A3 is the one that touches the gateway and so the authority surface; it follows the pattern every
route since v2.15.0 follows (a scope family, a refusal by name, a plant in the matrix) and ships only
with those. A1 and A2 are CLI and checker work and can go with W2.

## 11. Recorded questions for rev 0.4 *(rev 0.3; not decided here)*

- **Q1 — nothing binds a unit's code to its file.** D8 leaves runtime use of the new sections out of
  scope, so a unit can declare a requirement in its file and never call `declare_requirement`, or the
  reverse; the check passes on a vocabulary the unit does not speak. The candidate answer is small — a
  unit loads its file at startup and declares from it, so the file is the source rather than a
  description — and it is D8's own plan. It should follow W2 quickly.
- **Q2 — SDK units.** D2 admits an SDK agent as a unit, but only the Rust node loads
  `NodeCapabilityConfig`. A Python or TypeScript unit would have a file for the checker and nothing
  reading it at runtime: Q1 in a second form. The answer probably rides on A3's SDK work.
- **Q3 — catalogue units are answered by A2**, recorded here so the earlier list is complete.

## 12. Not claimed

A green wire-check does not mean the deployment will wire: a provider can be down, a probe can fail, a
mandate can be revoked, an intent can lapse, and the checker sees none of it. It means the vocabulary is
consistent, which is the only thing that can be known before the fleet exists. The runtime keeps the
last word, and its reports (`req/` opacity, `resolve_wiring`, the evidence journal) are the ones an
operator acts on.
