# Design-time tooling: declarations and the offline wire-check (plan)

**Status:** proposed, rev 0.8, 2026-09-27 (rev 0.2 added D9, W6 and §9, the declaration as a consumer record; rev 0.3 added §10, registering an artifact — D10, A1–A3; rev 0.4 added §11, object stores — S3 and GCS as requirements, D11–D14, S1–S4; rev 0.5 added §12, the two ways a capability arrives — D15–D16, L1–L2; rev 0.6 added §13, the stem fleet — D17–D18, R1–R2, answering Q1 — and §15, the build order; rev 0.7 added §16, recutting the examples in three tiers — X1–X2, Q4 recorded; rev 0.8 added §17, agent-authored functions — U1–U4, D19–D20, F1–F3. **Complete as an argument at rev 0.8.**) Nothing here is built. This plan argues the declaration
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

**D3 — a filter is written as attributes with operators; a bare value is equality.** *(W1 refinement, 2026-09-28: a **Version** is written `{ version = "2.1.0" }` wherever a value goes and is parsed at load; a bare string stays `Text`, so every existing `[[capability]]` file keeps its meaning — the §3 example's `version = "2.1.0"` comment was wrong on this point.)* This mirrors
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
| **W1** ✅ *shipped 2026-09-28 (with L1's and R1's format halves)* | The format: `NodeCapabilityConfig` gains `principal`, `requirements`, `groups`, `lanes`, `mandates`, `rules`, `hosts`, `presence` with `#[serde(default)]`; conversions to `CapFilter`, `CapabilityGroupDef`, `Mandate`-shaped and `Rule`-shaped values; loader rejects an unknown operator and a `Version` that does not parse | Every existing `[[capability]]` file loads byte-for-byte unchanged (pinned by a fixture); the §3 example loads and round-trips through `CapFilter::matches` against a hand-built `Capability` |
| **W2** ✅ *shipped 2026-09-28 (`src/wire_check.rs`, `mycelium wire-check`)* | The checker over a directory, the six error findings and two warnings above, text and JSON output (the `mycelium.design/declaration/1` document with `principal` per unit and `revision` in the envelope, D9), the subcommand, exit codes; the coop demos' units written as the first fixture directory under `tests/fixtures/units/` | A fixture with one deliberately unwired requirement exits 1 naming it; the coop fixture exits 0; CI runs both; the JSON of the coop fixture is a golden file, so a shape change is a visible diff |
| **W3** ✅ *shipped 2026-09-30 (`--schemas <dir>`; `[[capability]]` gained `schema_id` for it)* | Schema awareness: the checker reads the same schema directory guide 12 seeds, so a `schema_id` that names no file is an error and a provider/consumer version split is reported as the rollout-window case | A fixture holding `v1` providers and a `v2` requirement reports `schema-only mismatch` with both ids |
| **W4** ✅ *shipped 2026-09-30 (`unauthorisable edge` · `ungoverned edge`; `--no-authority`)* | The authority overlay (D6): per wired edge, reachability through declared mandates and rules; `--no-authority` to skip | A fixture whose only rule requires a mandate scope no unit declares reports `unauthorisable edge`; the `procurement_authority` example's vocabulary written as a fixture passes |
| **W5** ✅ *shipped 2026-09-30 (DOT with W2; the two paragraphs now)* | `--format dot`; a paragraph in guide 12 and one in `docs/operations/deployment.md`; a row in `examples/README.md` under the tutorial contract | The coop fixture renders; `/doc-coverage` gains a HOW·Ops cell for *deployment wiring* that opens the runbook |
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
| D8 | Runtime use of the new sections is out of scope *(superseded by D18 at rev 0.6: taken, with its own phase R1)* | Folding it in unplanned — it changes node startup behaviour and needed its own gate |
| D19 *(rev 0.8)* | Fuel on by default for an entry published by an agent principal, from `[hosts].fuel_per_call`; the operator's own entries as configured | Fuel for everything (a cost the operator's reviewed code need not pay); fuel for nothing (an agent's loop runs until the node is shed) |
| D20 *(rev 0.8)* | An agent-published entry is *proposed*, loads only into a shadow lane, and becomes loadable on a reviewer's co-signature — the award's shape | Trusting the agent's key outright (one compromised or confused agent fills the fleet); a human approval outside the manifest (a policy in someone's head, invisible to the checker and the records) |
| D17 *(rev 0.6)* | Presence policies are declarable (`[[presence]]`) and checked as a requirement with a count against distinct hosting units | Leaving presence code-only (the one desired state an all-stem fleet runs on would be the one thing the checker could not see) |
| D18 *(rev 0.6)* | A unit declares from its file at startup (`--units`), so file and runtime vocabulary are one; code declaration still works, doing both warns | Keeping the file descriptive (Q1: the check passes on a vocabulary the unit does not speak); a fleet-level file any node reads (a control plane by another door — every declaration stays a unit's own evaporating entry) |
| D15 *(rev 0.5)* | A unit declares what it hosts (`[hosts]`: kinds, budget, headroom, trusted publishers, placement root), so *would bind by provisioning* requires a host that could | Treating any matching entry as bindable (the checker would pass a fleet the provisioner never installs into) |
| D16 *(rev 0.5)* | One page owns the capability lifecycle for both arrival paths; `deployment.md`, `artifacts.md` and guide 02 point at it | Growing each existing page (three partial tellings of one workflow, the drift this plan exists to remove) |
| D11 *(rev 0.4)* | One adapter over the `object_store` crate, behind `store-aws` / `store-gcp` features | The AWS and Google SDKs (two large trees, two shapes, two paths to keep honest); hand-rolled SigV4 over reqwest (a signing implementation this project would then own) |
| D12 *(rev 0.4)* | A ranged fetcher trait with chunks staged to disk | Extending the in-memory prefetch cache (a model in RAM before its hash is checked, which is E12's defect generalised) |
| D13 *(rev 0.4)* | Credentials are the node's cloud identity, resolved by the adapter, egress-gated | Keys in the unit file, description or manifest (D7); credentials on librarians only (holds only for the small-artifact mesh path, artifact-library §5) |
| D14 *(rev 0.4)* | The manifest is also an object in the store | Keeping it file-only (the runbook's cron-sync step, a second thing to keep consistent) |
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
| **A1** ✅ *shipped 2026-09-28 as `mycelium-artifact publish|list|verify` — a binary of the wasm-host crate (feature `stem`), not a subcommand of the node binary, for the same reason as D18: the manifest types live there* | `mycelium artifact publish <description.toml> --library <dir> --key <file>`: stores the bytes, builds and signs the entry, appends the manifest line; `mycelium artifact list <dir>` renders a manifest as the same TOML shape with the content address and signer shown; `mycelium artifact verify <dir> --trusted <key>…` runs `verify_provenance` over every line | `list` of a manifest written by `publish` reproduces the description (golden fixture); a description whose `bytes` changed under an unchanged manifest fails `verify` in CI, seen failing first |
| **A2** ✅ *shipped 2026-09-28 — over artifact **descriptions** (D10's TOML), not the binary manifest, since `mycelium` cannot depend on the wasm-host crate that parses it* | The checker reads a library directory (`--library <dir>`) and reports **`would bind by provisioning`** for a requirement no unit offers but a manifest entry's `provides` matches — a separate class, a warning not an error, naming the entry and its footprint; with `--strict-deployed` it is an error | The coop fixture with the `catalog` demo's manifest turns one `unwired requirement` into `would bind by provisioning`; the JSON document carries the entry's content address on that edge |
| **A3** | `POST /gateway/artifacts/publish` behind a new scope family `artifact:publish`: the body is an **already-signed** entry (signing stays with the publisher's key, which never reaches a gateway) that the route verifies against the node's trusted publisher keys and writes with `publish_installable`; the bytes are not uploaded through the gateway — they are at a blob store the librarian mirrors (`HttpLibrarySource`, artifacts.md §2). SDK verbs in `mycelium-py` / `mycelium-ts` that build and sign the entry client-side | An unsigned or untrusted entry is refused 403 by name; a signed one appears under `installable/` on a second node; the route is in the scope table and the bypass matrix's plant covers it |

A3 is the one that touches the gateway and so the authority surface; it follows the pattern every
route since v2.15.0 follows (a scope family, a refusal by name, a plant in the matrix) and ships only
with those. A1 and A2 are CLI and checker work and can go with W2.

## 11. Object stores: S3 and GCS are requirements *(rev 0.4)*

The artifact-library design (§5 there) says a large artifact is pulled **directly from the durable
store by every installing node**, chunked, hashed on the way, gated by that node's egress policy. What
ships realises that only when the store is a filesystem the node can mount:

| # | Fact | Evidence |
|---|---|---|
| E11 | `FsLibrarySource` is the only durable source that is ranged and servable; it is what the librarian fronts and the coop demos use | `mycelium-wasm-host/src/artifact.rs:240`, `:323`; `examples/coop/src/bin/catalog.rs:91` |
| E12 | `HttpLibrarySource` materialises the **whole body in memory before hashing**, capped at 512 MiB by `Content-Length`, and a chunked response with no length is not bounded at all. The code's own comment (audit 2026-07-15, pass 5) names the fix — streaming through `RangedArtifactSource` — as tracked, not added | `mycelium-wasm-host/src/http_source.rs:146–160` |
| E13 | `BlobFetcher` is the vendor extension point (one method, whole-object) and **nothing implements it** in the repository; there is no ranged variant to implement | `mycelium-wasm-host/src/http_source.rs:36` |
| E14 | `PrefetchingSource` caches in memory (`Mutex<HashMap<ArtifactId, Bytes>>`), so even a streaming fetcher behind it would land a model in RAM | `mycelium-wasm-host/src/http_source.rs:43` |
| E15 | The manifest is a local file (`LibrarianConfig::manifest_path`); for a remote store the runbook tells the operator to sync it down by cron or a mounted volume | `docs/operations/artifacts.md` §2 "Remote blob stores" |

So a buyer whose models live in S3 or GCS cannot install one today. This is code, not docs, and it is
in this plan because the artifact path is the design-time input A2 reads and A1 writes.

**D11 — one adapter over the `object_store` crate, not two vendor SDKs.** `object_store` (Apache
Arrow's, 0.14) gives S3, GCS, Azure, plain HTTP and local behind one interface with **ranged reads**
(`get_range`, `get_ranges`), streaming bodies, and credentials from the environment, instance
metadata and workload identity — the three the two clouds actually use in production. The AWS and
Google SDKs are each larger than this whole crate's dependency tree, differ in shape, and would give
two code paths to keep honest. The adapter goes behind features `store-aws` and `store-gcp` (and
`store-azure` for free) in `mycelium-wasm-host`, off by default, so a build that does not want the tree
does not carry it; `cargo audit` and the feature-matrix clippy cover each.

**D12 — a ranged fetcher trait, and staging to disk, never to memory.** `BlobFetcher` gains a ranged
sibling (`size`, `fetch_range`) that the object-store adapter and a fixed `HttpLibrarySource` both
implement, and the bridge to `RangedArtifactSource` stages chunks to a node-local directory, so the
blob runtime's 4 MiB chunked placement reads from disk. `PrefetchingSource`'s in-memory cache stays
for small artifacts and is refused above a configured size rather than silently used.

**D13 — credentials are the node's cloud identity, never a file this plan defines.** An IAM role or a
GCP service account attached to the instance, resolved by the adapter; static keys stay possible
through the environment for a developer machine and are never in a unit file, an artifact description
or a manifest (D7 again). Every request still passes the node's `EgressPolicy` (`permits_url`) first,
so an operator can pin a bucket host.

**D14 — the manifest lives in the store too.** *(built with S2: `ObjectStoreFetcher::read_manifest`/`write_manifest` at `<prefix>/manifest`; the librarian takes an optional `ManifestSource`; `mycelium-artifact publish --library s3://…` writes both.)* The library's manifest is written as an object at a
fixed key beside the blobs, so a librarian fronting a remote store reads it from the store and the
"sync it down" step in the runbook goes away; `mycelium artifact publish --library s3://bucket/prefix`
(A1) writes blob and manifest line through the same adapter. The manifest stays the signed source of
truth (D10); only its location gains a second option.

| Phase | Deliverable | Exit gate |
|---|---|---|
| **S1** ✅ *shipped 2026-09-28* | The ranged fetcher trait (`RangedBlobFetcher`), disk staging (`DiskStagedSource`), and `HttpLibrarySource` re-done over HTTP `Range` requests with the body streamed and hashed incrementally; the 512 MiB cap and the unbounded-chunked hole both closed; `PrefetchingSource` refuses above its size bound by name | A 2 GiB blob served by a local HTTP fixture installs with peak RSS bounded (measured in the test, asserted under a ceiling); a chunked response with no `Content-Length` is bounded; both seen failing on the current code first |
| **S2** ✅ *shipped 2026-09-28 — one feature `object_store` (aws + gcp together, the crate's own features) rather than `store-aws`/`store-gcp`; the CI fixture is Adobe's S3Mock (MinIO's images are no longer publicly pullable); without `MYCELIUM_S3_TEST_URL` the same test runs over `file://` and says so* | The `object_store` adapter behind `store-aws`; A1's `publish` and the librarian read and write `s3://` URLs; D14's manifest-in-store | CI: a MinIO container as the S3-compatible fixture — publish, librarian reconcile, a node installs a blob by ranged pull, provenance verified; the egress gate refusing a bucket host outside `allow_hosts` |
| **S3** *(the GCS builder is compiled in with S2 — `gs://` URLs parse — but no GCS fixture runs in CI yet)* | `store-gcp`, same adapter, same tests; the S3-interoperability path (HMAC keys against the GCS XML API) documented as a fallback, not the route | CI: `fake-gcs-server` as the fixture, the S2 sequence green; a build with `store-gcp` and without `store-aws` compiles and passes the feature-matrix clippy |
| **S4** | Real-cloud evidence: a nightly against one real S3 bucket and one real GCS bucket, workload-identity credentials, the blob a real quantised model, results recorded like the scale nightly. Emulators prove the code path; only this proves the cloud | Two green runs each, recorded with dates; a runbook section per cloud in `docs/operations/artifacts.md` with the IAM/service-account policy the node needs, and the honest line that a credential on every pulling node is the price of no relay |

S1 is independent of the store and closes a shipped hole; it goes first. S2 and S3 share the adapter and
differ only by fixture and feature. S4 is delivery evidence in the sense AE4 uses the term: it needs
an account, not a commit, and the plan does not claim the clouds until it runs.

**Not claimed by this section.** Peer serving of a pulled model over the bulk transport stays an
optimisation (artifact-library §5) and is unchanged. Multi-region replication, lifecycle rules and
bucket versioning are the store's business and the operator's; the node reads by content address and
nothing here manages a bucket.

## 12. Two ways a capability arrives, one workflow *(rev 0.5)*

A capability reaches a cluster in one of two ways, and the documentation describes each on its own
page without ever putting them side by side:

| | **Directly deployed** | **Dynamically installed** |
|---|---|---|
| What is deployed | a unit whose capabilities are in its binary or its `[[capability]]` file, started by the operator's platform | a *host* unit that runs a provisioner (`mycelium-wasm-host`), with runtimes for some kinds and a budget |
| Who decides it exists | the operator, at deploy time | the host node, at runtime, when it sees unmet demand (`req/` with no provider), self-elects, and the signed footprint fits its headroom |
| Where the code comes from | the unit's image | the catalogue entry's content address, pulled from the library (§11) and verified |
| When it goes away | when the unit stops | when demand lapses or a governor sheds it; a tombstone, and the placed bytes at the host's placement root |
| Where it is documented today | `docs/operations/deployment.md`, guide 13 | `docs/operations/artifacts.md` §3, `dynamic-scaling.md` §Elastic capacity, the `provisioning` and `catalog` demos |
| What the checker knows (rev 0.4) | the unit's `[[capability]]` blocks | "an entry matches" (A2) — **but not whether any unit could host it** |

The last row is the design defect: A2 as written would say *would bind by provisioning* for a 5 GB
model in a fleet where no unit runs a blob runtime or has 5 GB of headroom, and the provisioner
would never self-elect (`Provisioner::register_runtime`, `set_install_budget`,
`set_resource_policy`, `mycelium-wasm-host/src/provisioner.rs:282–296`). The row above it is the
documentation defect: an operator reading `deployment.md` does not learn that some of the fleet's
capabilities will not be in any image, and one reading `artifacts.md` does not learn how a host unit
is deployed in the first place.

**D15 — a unit declares what it hosts.** The unit file gains a `[hosts]` table: the artifact kinds
the unit has runtimes for, its install budget, its headroom fraction, the publisher keys it trusts,
and the placement root. These are the provisioner's own settings (E-rows above) written down, and D8
applies: nothing here makes the node read them at runtime yet.

```toml
[hosts]                              # this unit runs a provisioner
kinds            = ["wasm-component", "blob"]
install_budget_bytes = 8589934592    # 8 GiB
headroom         = 0.8
trusted_publishers = ["ed25519:3f…"] # keys, never secrets
placement_root   = "/var/lib/mycelium/artifacts"
```

With it, A2 becomes precise: a requirement *would bind by provisioning* only when an entry's
`provides` matches **and** some unit's `[hosts]` names the entry's kind with a budget at or above the
entry's signed footprint. Otherwise the finding is **`unhostable entry`** (an error), naming the kind or
the bytes short, which is the design-time form of the provisioner's `ineligible_skips` tripwire.

**D16 — one page owns the lifecycle, and the two existing pages point at it.** A new
`docs/operations/capability-lifecycle.md` states the workflow once, for both columns, in the order an
operator lives it: declare (the unit files, §3) → check (`wire-check`, §4) → deploy the direct units
and the host units (`deployment.md`) → publish the catalogue (§10, §11) → watch demand pull the rest
in (`dynamic-scaling.md`) → read the three views (§9). `deployment.md` gains one paragraph saying that
some capabilities will not be in any image and linking here; `artifacts.md` gains one saying how a host
unit is deployed and linking here. Guide 02 (capabilities) gets the same two-column table above, since
that is where a developer first meets the word. The wiki's architecture folder cites the page rather
than restating it.

| Phase | Deliverable | Exit gate |
|---|---|---|
| **L1** ✅ *shipped 2026-09-28 (format with W1, `unhostable entry` with W2)* | `[hosts]` in the format (W1) and the `unhostable entry` finding in the checker (A2) | A fixture with a matching entry and no hosting unit reports `unhostable entry`; the same fixture plus a host unit with budget below the footprint reports it naming the bytes short; with budget above, `would bind by provisioning`; all three seen failing first |
| **L2** ✅ *shipped 2026-09-28* | `capability-lifecycle.md`, the two pointing paragraphs, guide 02's table, the wiki citation; the coop `provisioning` demo's units written as the fixture so the page's example is the checker's fixture | `/doc-coverage` gains a row *capability lifecycle* with HOW·Ops and HOW·Dev both Clear by opening the page; `/wiki-lint`'s dead-link and coverage checks pass; a reader of `deployment.md` reaches the page in one link |

L1 is part of W1 and A2 rather than after them, because it changes the format. L2 is the
documentation deliverable of the whole plan and lands with W2, so the page is written against a
checker that exists.

**Not claimed.** The `[hosts]` table describes eligibility, not placement: which host installs a given
artifact stays the provisioner's probabilistic self-election at runtime, and the checker says a
host *exists*, never *which*.

## 13. The stem fleet: identical nodes that load what the declarations call for *(rev 0.6)*

A cluster can be a set of identical **stem nodes** that hold no application capability at deploy time
and load what is required dynamically. The autonomic loop was built for this shape and the coop
`provisioning` demo runs it: two identical providers, a declared need, one self-elects and installs,
kill it and the standby re-provisions (`examples/coop/src/bin/provisioning.rs`). What the shape needs,
and where it stops, is stated here so a buyer is not left to infer it.

**What a stem node is.** Not empty: the mesh binary with the wasm host and a provisioner, runtimes for
the kinds it hosts (the WASM sandbox is registered by default; a blob runtime needs its native consumer
— an Ollama or ONNX process — already on the node), the CA identity, the publisher keys it trusts, a
reachable library or store, and its egress policy. That is D15's `[hosts]` table plus `GossipConfig`.
A stem fleet is a fleet of one image.

**What drives the loading** is two kinds of desired state, both reconciled locally by every node
through the one resolve-and-pull path (`mycelium-wasm-host/src/provisioner.rs:107–126`):

- **demand** — a `req/` entry with no live provider, declared by any unit that needs the capability;
- **presence** — a `SupervisionPolicy` (`filter`, `min_providers`, `max_providers`) that keeps at
  least N and at most M providers across the fleet **independent of demand**. This is what makes the
  shape self-healing, and it is the answer to *who requires anything in an all-stem fleet*: the
  operator declares presence, and the fleet fills it.

Nothing assigns a node an artifact. Each node sees the gap, self-elects with a probability that damps
the herd, checks its own runtime, budget and headroom, and installs; which node ends up hosting is not
predictable and the checker never says.

**D17 — presence is declarable.** The unit file gains `[[presence]]`, the `SupervisionPolicy` fields
written down, and the checker treats a presence policy as a requirement with a count: it must be
hostable by at least `min_providers` distinct units' `[hosts]` (D15), or the finding is
**`presence unhostable`** naming the shortfall.

```toml
[[presence]]                         # keep 2–4 route optimizers alive fleet-wide
ns = "route"; name = "optimize"
min_providers = 2
max_providers = 4
```

**D18 — Q1 answered: the file is the source, and a unit declares from it at startup.** This is D8's
own plan, now taken: a node given `--units <file>` loads it with the same loader (W1) and, after
`start()`, declares every `[[requirement]]`, defines every `[[group]]`, attaches a provisioner
configured from `[hosts]`, and publishes every `[[presence]]` as a supervision policy. Nothing in the
file is read by any other node; every declaration still travels as the evaporating KV entry it always
was, so the runtime law is unchanged — intents lapse, nodes reconcile locally, and the records say what
was installed. What changes is that the design-time file and the runtime vocabulary can no longer
disagree, because there is one of them. A unit that declares in code keeps working; a unit that does
both gets a warning naming the duplicate.

*D18 amended at R1 (2026-09-28):* the runtime half lives in **`mycelium-stem`**, a binary of the
`mycelium-wasm-host` crate (`Stem::start`), not as `--units` on the `mycelium` node binary — the
provisioner, the runtimes and the librarian live in that crate and the dependency runs the other way,
so the node binary could never have attached a provisioner. A `Stem` advertises every `[[capability]]`
(always-alive; `probe_url` is logged, not honoured — the node binary's probe loop is the place for it),
declares every `[[requirement]]`, defines every `[[group]]`, and, with `[hosts]`, runs a provisioner
configured from it with every `[[presence]]` as a supervision policy, refreshing the catalogue from KV
each tick. One image, every role: `--librarian <manifest> --publisher ed25519:<hex>` takes the librarian
role over `--library`. Lanes, mandates and rules are vocabulary for the check and the evaluator and do
nothing at runtime, said once at start.

**Where the shape stops**, on the tin:

- **Only two kinds load dynamically** — WASM components against the host interface, and blobs for a
  runtime already present. New native code, a companion crate, a gateway route, a TLS change are a new
  image and a rolling deploy (`docs/operations/deployment.md` §Rolling upgrades). The stem image is
  itself a direct deployment.
- **The catalogue is the supply chain.** Every stem node trusts the publisher keys in its config, a
  signed entry is what stops a relabelled artifact, and install rights through the rights ledger bound
  what any node takes on. This is the posture to review before adopting the shape (threat model §7).
- **Large models are filesystem-backed until S1–S3 land** (§11).

| Phase | Deliverable | Exit gate |
|---|---|---|
| **R1** ✅ *shipped 2026-09-28 — the runtime half as `mycelium-stem` in the wasm-host crate, not `--units` on the node binary (D18 amended below)* | `[[presence]]` in the format and `presence unhostable` in the checker; `--units <file>` on the node binary and the startup declaration path (D18), with the duplicate warning | A fixture with `min_providers = 2` and one hosting unit reports `presence unhostable (1 of 2)`; a node started with the §3 file shows its `req/`, `cap-group/` and presence entries on a second node; the same requirement declared in both file and code warns once; all seen failing first |
| **R2** ✅ *shipped 2026-09-28 — as an in-process fleet test in the wasm-host CI job (`the_stem_fleet_fills_a_presence_floor_and_reheals`), not a Docker suite; the Docker cut is open* | The stem fleet as the second reference topology: N identical nodes from one image, one units directory, a presence policy, a librarian; in CI beside the coop suites | The fleet converges to `min_providers` live providers within a bound; killing one restores it; the checker's JSON for the directory and the fleet's `cap/` view agree on which capabilities exist (the first declared-versus-observed comparison, run locally without a consumer); guide 13 gains the topology with its limits |

R1 depends on W1 and L1 (the format) and A2 (the hosting check); R2 depends on R1 and W2. R2 is also
the first place §9's comparison runs end to end, so it doubles as W6's local fixture.

## 14. Recorded questions *(rev 0.3; status at rev 0.6)*

- **Q1 — nothing binds a unit's code to its file.** **Answered by D18 / R1.**
- **Q2 — SDK units.** D2 admits an SDK agent as a unit, but only the Rust node loads the file. A
  Python or TypeScript unit has a file for the checker and nothing reading it at runtime. **Open.** The
  candidate answer is that the SDKs gain a `declare_from(path)` verb over the existing gateway routes
  (requirements and groups already have them; presence and hosts do not apply to an SDK agent), so it
  rides on A3's SDK work and is a day once A3 exists.
- **Q3 — catalogue units.** **Answered by A2 / L1.**

## 15. Build order

The plan is complete as an argument at rev 0.6; nothing is built. The order, chosen so each step
ships a whole thing and the shipped defect goes first:

1. **S1** — streaming HTTP pulls; closes E12, independent of everything else.
2. **W1 + L1 + the format half of R1** — the unit file, one PR, because every later step reads it.
3. **W2 + A2 + L2** — the checker, the provisioning class, the lifecycle page; the coop fixtures.
4. **R1's runtime half, then R2** — the node declares from its file; the stem fleet in CI.
5. **S2 + A1** — the S3 adapter and `mycelium artifact publish|list|verify`, which share it.
6. **S3** — GCS on the same adapter.
7. **W3, W4, W5** — schema awareness, the authority overlay, DOT; each a day, any order.
8. **A3** — the gateway publish route, with its scope family and matrix plant.
9. **F1, F2, F3** — fuel by default, the proposed-then-accepted entry with its shadow lane, the guide section.
10. **X1, then X2** — the examples' units directories after step 3, the stem cuts after step 4.
11. **W6** — the consumer record, private repo, in parallel from step 3 on.
12. **S4** — real buckets, when an account exists; delivery evidence, not a commit.

What the plan leaves outside itself: Q2, Q4, a designer UI (§6, deliberately none), the enforced
composition (§8, the axis plan's §13 decides), and NovusLens's own rendering (their side of the
handover).

## 16. Recutting the examples to a stem cluster plus declarations *(rev 0.7)*

The examples (25 top-level, 19 coop bins, 21 across the companions) are Rust binaries that call the
public API directly; that is what they are for, and guide chapters cite them by name. "Recut every
example as a stem cluster plus requirements" would hide the API most of them exist to show, and for
most it is not possible today, because of one fact:

| # | Fact | Evidence |
|---|---|---|
| E16 | A WASM component can **handle a request**, read and write its **confined KV subtree**, **emit a signal**, and log. It cannot take from a tuple-space lane, call another capability, declare a requirement, hold a mandate, or vote. The host world is four interfaces | `mycelium-wasm-host/wit/host.wit` (`world capability-component`) |

So what can load dynamically is an *invocable function with local state*, and a demo whose logic is a
worker draining a lane, a proposer, a curator or a federated caller cannot become an artifact without
widening that world. The recut therefore comes in three tiers, decided here:

**Tier 1 — every example that declares anything gets a units directory.** This is the universal
recut and it costs nothing in code: each example that advertises a capability, declares a requirement,
defines a group or names a lane gets `examples/<name>/units/` written in the §3 format, the checker
runs on it in CI, and with D18 the example can start from its file instead of its `main`. The
example keeps showing the API; the directory shows the same vocabulary as a declaration, and the two
are gated to agree (R1's duplicate warning is the gate). This is what "stem cluster + requirements"
means for the suite as a whole, and it is what makes the checker's fixtures the examples rather than
a parallel set that drifts.

**Tier 2 — the artifact-shaped coop demos run from one stem image.** Five demos already load their
dynamic part from the catalogue (`provisioning`, `catalog`, `model_deploy`, `reheal_deploy`,
`mcp_toolgrowth`, plus `llm_agent`'s model path). Their static roles (seeder, buffer, worker) stay
code; their dynamic role becomes the R2 topology: N stem nodes from one image, the demo's units
directory, its presence policy, its artifact in the library. One `Makefile` target runs each demo both
ways and asserts the same outcome. These are the examples the buyer deck should show, because they
are the shape §13 describes.

**Tier 3 — mechanism demonstrations stay as they are.** Election, authority drain, the receipt ladder,
identity as one record, federation trust, the distributed lock, the replay bundles, the viz pages,
Conway: none has a capability to install, and a stem cut would add a catalogue to a demo about
something else. They get Tier 1's directory only where they declare something, and otherwise nothing.

**Q4 — widening the component world** (tuple-space take/complete, calling a capability, declaring a
requirement from inside a component) is the question a full recut would need answered, and it is a
security question before it is a feature: every import is a hole in the confinement the host exists to
enforce. **Recorded, not decided.** It belongs with the guardrails plan, not here.

| Phase | Deliverable | Exit gate |
|---|---|---|
| **X1** | Tier 1: a units directory for every declaring example, `wire-check` over all of them in CI, a *declared* facet in `examples/README.md`'s matrix | Every directory exits 0; deleting one capability block from any directory turns its CI row red (seen failing first); the README's tutorial contract names the directory as part of the example |
| **X2** | Tier 2: the five artifact-shaped demos and `llm_agent` run from the stem image with `--units`; the both-ways target | Each demo's stem run reaches the same asserted outcome as its code run, in CI beside the coop smokes; the buyer deck's provisioning slide cites the stem run |

X1 follows W2 and R1 and is mostly authoring; X2 follows R2. Neither changes any example's code path,
so guide citations stay valid.

## 17. Agent-authored functions: the legitimate uses of dynamic load *(rev 0.8)*

The sandbox was built for code the fleet does not trust: content-addressed, provenance-signed,
confined to its own KV subtree, optionally fuel-metered per call, no filesystem, no network
(`mycelium-wasm-host/src/host.rs:90`, `:147`; `confine.rs`). That is the right envelope for a
function an agent wrote an hour ago, and the four-interface world (E16) is what keeps it one. The uses
below are recorded so the shape is argued for once and the gates travel with it.

| Use | What loads | Why an artifact and not a prompt or native code |
|---|---|---|
| **U1 — functions an agent authors** for itself or its peers: a scorer, a vendor-format parser, a route heuristic | a component compiled outside the fleet, published under the agent's key, loaded where demand is unmet | distribution is the catalogue (signed, pulled, verified), not a prompt; the fleet loads, it never builds (`docs/plans/mycelium-reason.md`, the tool-growth frame) |
| **U2 — evaluators that must be reproducible**: a knowledge-layer assessment, a contract-net evaluation, a policy predicate | a deterministic component with no clock and no network | the same answer on replay, and the invocation is in the evidence journal; *why was this awarded* becomes answerable from records, not from a transcript |
| **U3 — transforms at boundaries**: validators and normalisers between schema versions, redaction before export, per-receiver guardrail checks beyond a regex | a component invoked at the boundary | the deliberate contrast with schema migrations, which stay *declarative data, never code* because they **gossip** (guide 12 §tier 3); a component does not gossip — it is pulled by content address under a signature, which is why code is acceptable on this path and not on that one |
| **U4 — pure work in a pipeline**: the stage function the coop demos stand in for | the stage function | the worker that drains the lane stays native — exactly the boundary E16 draws |

**What has to be true for this to be safe.** Most of it exists; two decisions are new.

- A publisher key is an identity, so an agent that publishes is a principal holding a mandate for
  `artifact.publish`, and A3's route refuses anything else by name.
- Install rights bound how much of the fleet any one key can fill
  (`Provisioner::with_install_rights`, `provisioner.rs:204`), and a presence policy (D17) bounds how
  many copies run.
- The host trusts only the publisher keys in `[hosts].trusted_publishers` (D15;
  `provisioner.rs:145`); an agent's key is trusted by being listed, and delisted by an edit that goes
  through review.
- **D19 — fuel on by default for agent-published entries.** `WasmHost::with_fuel_per_call` is opt-in
  today (`host.rs:188`). An entry whose publisher is an agent principal, rather than the operator's
  CI key, is invoked with a fuel budget from `[hosts].fuel_per_call`, and a component that runs past
  it traps and is recorded. The operator's own entries may run unmetered; an agent's may not.
- **D20 — a second signature before promotion.** An agent-published entry enters the catalogue as
  *proposed* and is loaded only into a **shadow lane** (the control-profile ladder,
  `docs/operations/control-profiles.md`), beside the incumbent, where its outputs are recorded and
  compared but take no demand. It becomes loadable for real when a reviewer's key co-signs the entry
  — the shape `mycelium-commitment` already uses for an award, where a name without a signature is a
  claim (`mycelium-commitment/src/lib.rs:162`). *An agent proposed and a reviewer accepted* is then a
  fact in the manifest, not a policy in someone's head.
- The one thing never to do is widen the component world to make any of this easier. Q4 stands: an
  evaluator that can call out is no longer an evaluator that replays.

| Phase | Deliverable | Exit gate |
|---|---|---|
| **F1** ✅ *shipped 2026-09-30 (`FuelPolicy` decided per entry from its verified signer; `[hosts].operator_publishers` ⊆ `trusted_publishers`; the record is the wasm-host crate's own `InvocationRecord` — the evidence journal is gateway-side and not reached from a serve loop)* | D19: `fuel_per_call` in `[hosts]`; the provisioner invokes an agent-published entry metered and an operator-published one as configured; the trap recorded as an execution with the fuel exhausted named | An entry signed by an agent principal that loops is stopped at the budget and its execution record says so; the same component under the operator's key runs to completion; seen failing first |
| **F2** | D20: `proposed` in the manifest line (inside the signature), a second-signer field, the shadow-lane load, `mycelium artifact accept <entry> --key <reviewer>`, and A2 reporting a proposed entry as *would bind after acceptance* rather than *would bind by provisioning* | A proposed entry never takes demand (the incumbent's call count is unchanged across a run); after acceptance it does; a forged acceptance fails provenance; the coop `provisioning` demo gains the shadow-then-accept sequence as its third wave |
| **F3** | The guide: a section in `docs/guide/16-guardrails.md` stating U1–U4, the five gates, and the honest limit; the catalogue runbook's trust section pointing at it | `/doc-coverage` gains a row *agent-authored functions* with HOW·Dev and HOW·Ops Clear by opening the page |

F1 needs W1 (the `[hosts]` table); F2 needs A1 and A2 (the manifest tooling and the provisioning
class); F3 lands with F2. In the build order they sit after step 5.

**Not claimed by this section.** Determinism is a property of the component, not a guarantee of the
host: a component that reads its own KV subtree can see a different value on replay if that subtree
changed, and only a recording (`mycelium-sim`) makes the replay exact. Fuel bounds instructions, not
wall time or memory; memory stays the instance limit the host already sets. And a reviewer's
signature says a person accepted the entry, not that the function is correct.

## 18. Not claimed

A green wire-check does not mean the deployment will wire: a provider can be down, a probe can fail, a
mandate can be revoked, an intent can lapse, and the checker sees none of it. It means the vocabulary is
consistent, which is the only thing that can be known before the fleet exists. The runtime keeps the
last word, and its reports (`req/` opacity, `resolve_wiring`, the evidence journal) are the ones an
operator acts on.
