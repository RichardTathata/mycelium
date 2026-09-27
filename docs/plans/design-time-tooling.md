# Design-time tooling: declarations and the offline wire-check (plan)

**Status:** proposed, rev 0.1, 2026-09-27. Nothing here is built. This plan argues the declaration
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

**Output.** Text by default; `--format json` emits the resolved graph (units, offers, requirements,
edges, findings); `--format dot` emits Graphviz. Exit 0 with no errors, 1 with any error, 2 for a
file that does not load. The wording in every line is *would bind* / *could not bind*.

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
| **W2** | The checker over a directory, the six error findings and two warnings above, text and JSON output, the subcommand, exit codes; the coop demos' units written as the first fixture directory under `tests/fixtures/units/` | A fixture with one deliberately unwired requirement exits 1 naming it; the coop fixture exits 0; CI runs both |
| **W3** | Schema awareness: the checker reads the same schema directory guide 12 seeds, so a `schema_id` that names no file is an error and a provider/consumer version split is reported as the rollout-window case | A fixture holding `v1` providers and a `v2` requirement reports `schema-only mismatch` with both ids |
| **W4** | The authority overlay (D6): per wired edge, reachability through declared mandates and rules; `--no-authority` to skip | A fixture whose only rule requires a mandate scope no unit declares reports `unauthorisable edge`; the `procurement_authority` example's vocabulary written as a fixture passes |
| **W5** | `--format dot`; a paragraph in guide 12 and one in `docs/operations/deployment.md`; a row in `examples/README.md` under the tutorial contract | The coop fixture renders; `/doc-coverage` gains a HOW·Ops cell for *deployment wiring* that opens the runbook |

W1 and W2 are the value; W3–W5 are each a day and can stop after any one of them without leaving a
half-feature, because each is a finding class added to a working checker.

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

## 9. Not claimed

A green wire-check does not mean the deployment will wire: a provider can be down, a probe can fail, a
mandate can be revoked, an intent can lapse, and the checker sees none of it. It means the vocabulary is
consistent, which is the only thing that can be known before the fleet exists. The runtime keeps the
last word, and its reports (`req/` opacity, `resolve_wiring`, the evidence journal) are the ones an
operator acts on.
