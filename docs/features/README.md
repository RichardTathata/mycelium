# Feature pages

One short page per feature: what it is, how to turn it on, and where to see it work. A feature page is an
entry point, not a manual. It gathers links to the reference, the guide, the example index, the catalogues
and the code, so a reader can find everything about one feature from one place. The
[capability map](../capabilities.md) is meant to be the index; today its rows link the pages that exist, and every
row lists its pages once the remaining pages are written (the plan's row F3).

The plan that introduced these pages is [`plans/feature-pages.md`](../plans/feature-pages.md).

## The six sections

Every page has these six sections, in this order, with these headings. Copy
[`_template.md`](_template.md) to start one.

1. **What it is.** Two or three plain sentences, and links to the concept's entries (one or more) in the
   [concepts glossary](../guide/00-concepts.md).
2. **When to use it, and when not.** One line each.
3. **Turn it on.** The settings that enable the feature, each linked to its entry in
   [`configuration.md`](../reference/configuration.md) or [`unit-file.md`](../reference/unit-file.md), and a link
   to a configuration or unit file that a test loads. For a feature enabled in code, the section opens with the
   fixed sentence "This feature is turned on in code", then lists one item per line: the item's name in
   backticks, linked to the file that defines it, whose last path segment is a `fn`, `struct`, `enum`, `trait` or
   `const` in the linked file. A configuration field the feature reads is its own line, `name` in backticks linked
   to its row, `configuration.md#<field>`. The section ends with the test that
   runs that code: its name in backticks, a `fn <name>` in the linked file (checked pre-push), listed by CI's test
   universe and matched by no entry of `scripts/test-coverage-exceptions.txt` (checked in CI). A test that reads
   the page is preferred; a test that runs a hand-kept copy is recorded as such.
   A guide snippet that no test runs is recorded as such, and the gap goes to the `/doc-coverage` pass.
4. **See it work.** A link to the feature's row in the [example index](../../examples/README.md), which holds
   the run command, the prerequisites and the CI status. If there is none, the section opens with the fixed
   sentence "No runnable example exists" and links the test that runs the feature end to end: its name in
   backticks, a `fn <name>` in the linked file, listed by CI's test universe and matched by no entry of
   `scripts/test-coverage-exceptions.txt`. The feature registry records `example: none`, and the gate fails if the
   example index later gains a row for that feature.
5. **What it guarantees, and what it does not.** The guarantee and rule ids, each linked to where it is
   catalogued, the plan rows whose *not built* items apply, and the
   [what-is-proven](../operations/what-is-proven.md) ledger. A guarantee id a companion registers (such as
   `stem.*`) is not in the guarantee catalogue: link it to where its states are described. Link one entry by its
   anchor: a guarantee as `guarantee-catalogue.md#<id>`, a rule by its heading, a ledger row by its `<a id>`, a
   configuration field as `configuration.md#<field>` (plan P4). The plans' rows have no anchors of their own: link
   the section and name the row beside the link.
6. **Go deeper.** The guide chapter, the operations runbook, the design record and the code. Where a slot has
   nothing, say so: "no design record; the plan row is the record", or no runbook, naming what covers the ground
   instead.

## Link, do not copy

A page links to its sources and copies nothing from them: no paragraph from a guide, no snippet of
configuration, no evidence status. A copy drifts from its source and nothing notices; a link either resolves
or fails a check. Where a page must name a limit, it names it in a few words and links to the place that
states it.

## The registry and the gate

[`registry.toml`](registry.toml) maps every id its sources name to a page. The ids come from seven sources:

- the [concepts glossary](../guide/00-concepts.md)'s quick-reference rows, each by its `<a id>` (a row without
  one fails the gate, so a new term cannot go unmapped);
- the [guarantee catalogue](../reference/guarantee-catalogue.md) and the [rule catalogue](../reference/rule-catalogue.md);
- the unit file's keys, read from `NodeCapabilityConfig` in `src/capability_config.rs` and checked against the
  section headings of [`unit-file.md`](../reference/unit-file.md) both ways;
- the Cargo features of every workspace crate, implicit ones included;
- the guarantees a companion registers in code (such as `stem.*`; an id that is not a string literal is refused);
- the workspace crates themselves.

Each id maps exactly once: to a feature with a page, to a feature whose page is not written yet
(`page = "todo"`, the plan's row F3), or to an exclusion that says why it is not a reader-facing feature. One page
belongs to one feature, and a registry key outside the known set is refused. A feature's page must link every id
the registry gives it.

`scripts/check-feature-pages.py` is the gate. It runs in `make check` (through `scripts/check-positioning.sh`) and
in CI, and fails on an id the registry does not map, a registry id no source has, a page the registry does not name,
and on any page whose sections, links, anchors, code items, configuration fields, named tests or example do not
check out. It does not judge prose: whether a page is still true of the code is the `/doc-coverage` pass. It
recognises the markers above this way:

- **A link** is inline, `[text](target)`. A reference-style link, a link with a title and an `<a href>` are
  refused, because the resolver does not read them.
- **A code item** is a link, anywhere on the page, whose text is one backticked name and whose target is a `.rs`
  file; its last path segment must be a `fn`, `struct`, `enum`, `trait`, `const`, `type` or `static` there, and a
  CamelCase qualifier (`Rule::` in `Rule::at_most`) a type there.
- **A configuration field** is a backticked name linked to `configuration.md`; it must have a `| name |` row in the
  field table, and the link must point at that row, `configuration.md#<name>`.
- **A catalogue entry** is a backticked id in a link to the guarantee or rule catalogue: one id per link, pointing at
  that entry (`guarantee-catalogue.md#<id>`, or the rule's heading). Every row of the configuration table and of the
  what-is-proven ledger must carry its own `<a id>`, so a row added without one fails.
- **A test** is a backticked snake_case name (a module path allowed), not itself a link, in *Turn it on*'s last
  item or anywhere in *See it work*. Its paragraph or list item must link its `.rs` file, it must be a `#[test]`
  `fn` there, and it must not be excepted by `scripts/test-coverage-exceptions.txt`. In CI the `test-coverage` job
  also checks that some job of the run executed it in that file's target: an integration file's own binary, or a
  `src/` file's crate. So a backticked snake_case name in *See it work* that is not a test belongs in a link.
- **No example:** with `example = "none"`, *See it work* opens with the fixed sentence and names at least one
  test, and the gate fails if the example index's capability matrix gains a row whose name contains the page's name
  in snake case or an `example_watch` entry, or that links the page. With an example, its matrix row must carry a
  CI status and *See it work* must link the index.

Adding or renaming a page means adding or changing its `[[feature]]` entry in the same change. The gate's mutation
suite is `scripts/test-check-feature-pages.py`.

## The pages

| Page | Feature | Capability-map row |
|---|---|---|
| [Activation egress proxy](activation-egress-proxy.md) | An activation command reaches hosts by name through a proxy its stem runs, with credentials attached at the proxy | Acquire and adapt capabilities |
| [Fleet-wide history limits](fleet-history-limits.md) | A history-limited rule counts an actor's calls across the fleet, exactly by consensus seats or approximately by a gossiped tally | Constrain actions |
| [Causal records](causal-records.md) | A request's `correlation` and `parent`, carried in its payload into every record its work produces; `mycelium explain --correlation` renders one run as one chain and `--closure` the dependency closure of one record, holes named and never filled | Constrain actions with authority and evidence |
| [Declare](declare.md) | `mycelium declare`: a project's units and skills sent to a gateway once, or kept current under `--watch` as the files change — `wire-check` first, the previous declaration kept when it refuses | Start from the published node |
| [Init](init.md) | `mycelium init`: a scaffold written from a template the binary embeds — a Rust fleet with one call governed at the provider, or a Python agent against the published node — pinned to the binary's release | Start from the published node |
| [Doctor](doctor.md) | `mycelium doctor`: what `start()` would print and whether it would refuse, from the configuration and the environment, without a node; the effective configuration with each field's source | Start from the published node |
| [Published node](published-node.md) | Every `v*` tag publishes the three binaries for three platforms with sums, and a multi-arch image; a developer starts a node without a toolchain | Start from the published node |
| [Inference routing as policy](inference-routing.md) | The action evaluator decides each routed model call on its destination, locality and data class | Constrain actions; Run tools and reasoning |
| [Authorising actions](action-authorisation.md) | An enforcement point asks a replaceable evaluator — permit, deny or indeterminate — before the work runs, and records the decision first | Constrain actions |
| [Authority from the declaration](declared-authority.md) | `[authority]`: the evaluator, execution authority, provider enforcement and evidence journal built from this node's unit at `start()`, for a node started from a file | Constrain actions |
| [Evidence journal](evidence-journal.md) | The node-local, fsynced, never-gossiped record of each decision; only a hash-bearing reference enters the audit chain | Constrain actions |
| [Provider enforcement](provider-enforcement.md) | A protected call is checked where it runs, whichever door it came through; fails closed without an evaluator | Constrain actions |
| [Mandates and authority drain](mandates.md) | One appointment checked at the resource; a newer epoch stops the old holder's writes and running work, across restarts | Constrain actions |
| [Audit trail and audit export](audit-trail.md) | Governance actions sealed into a per-node hash chain, mirrored to a SIEM or WORM sink, pruned behind a signed checkpoint | Constrain actions |
| [Guardrails](guardrails.md) | One declared policy compiled onto a node, each clause labelled with its strength; off-list callers refused at the provider and the denial sealed | Constrain actions |
| [The HTTP gateway and its credentials](http-gateway.md) | The node's HTTP edge, closed by a bearer, token tables or SSO; an exposed open gateway refuses to start | Constrain actions |
| [Single sign-on at the gateway](gateway-sso.md) | An IdP's JWT as a gateway bearer, its groups mapped to scopes | Constrain actions |
| [Gateway TLS](gateway-tls.md) | The gateway serves HTTPS with the node identity's certificate or an operator's | Constrain actions |
| [Gateway caller identity](gateway-caller-identity.md) | A provider sees the client behind a gateway call, qualified by its issuer and signed by the gateway | Constrain actions |
| [A2A interoperability](a2a.md) | An AgentCard and `/a2a` task dispatch in the 1.0 and 0.3 shapes, every send through the same doors | Run tools and reasoning |
| [Mesh mTLS and signed writes](mesh-mtls.md) | Gossip between nodes the fleet CA certified, every KV write frame signed by its writer | Discover participants and signal |
| [Node identity, proofs and revocation](node-identity.md) | One sealed identity record per node, rotation and revocation every verifier honours, an operator's removal of a member | Discover participants and signal |
| [Egress allow-list](egress-allow-list.md) | A hostname list on the substrate's own outbound calls, every redirect hop included | Constrain actions |
| [Credentials attached at egress](credentials-at-egress.md) | A secret attached at the node's gate only for the host its binding names; the caller holds a placeholder | Constrain actions |
| [Confined fleet](confined-fleet.md) | A deployment where agent pods reach nothing but their gateway, so every act passes a door that records it | Constrain actions |
| [Persistence](persistence.md) | The KV store and acceptor memory on disk, what an acknowledgement means per sync mode, and refusals over unreadable state | Inspect outcomes and reproduce |
| [Data-at-rest encryption](data-at-rest-encryption.md) | The WAL and snapshots encrypted through an operator's cipher; a wrong key refuses the start | Inspect outcomes and reproduce |
| [Consensus, electorates, locks and leadership](consensus-electorates.md) | Quorum agreement per slot, and exclusive outcomes decided only by an electorate pinned at genesis | Coordinate work |
| [Destination commit](destination-commit.md) | `mycelium-effects`: one effect, once, at a destination you own, keyed by `operation_id` | Inspect outcomes and reproduce |
| [Replay bundles](replay.md) | A recorded run replayed decision for decision, a divergence named where it first departs | Inspect outcomes and reproduce |
| [Durability receipts](durability-receipts.md) | What an acknowledgement proves, by rung and nothing above it; a timeout is unknown, never a negative | Inspect outcomes and reproduce |
| [Gossip KV](gossip-kv.md) | Every node's replica of one store, writes gossiped and merged by last-writer-wins on a hybrid logical clock | Discover participants and signal |
| [Signal mesh](signal-mesh.md) | Scoped, ephemeral events forwarded unconditionally; a node's boundary decides what it acts on | Discover participants and signal |
| [Capability discovery](capability-discovery.md) | Providers advertise what they do, readers resolve by filter, and a stale advertisement evaporates | Discover participants and signal |
| [The mailbox](mailbox.md) | Events delivered to one node in HLC order through the KV, deleted once handed over | Discover participants and signal |
| [Service calls: RPC, scatter-gather and bulk](service-calls.md) | One node calling another by kind and waiting for the answer: one target, several, or a payload too large for a frame, fetched over HTTP(S) | Discover participants and signal |
| [The Eve loader](eve-loader.md) | `mycelium-ts` runs an Eve agent directory unchanged on a fleet: what maps is loaded, every other slot refused by name | Run tools and reasoning |
| [The Ops Console](ops-console.md) | A static page reading each listed node directly, from the browser: no server, no proxy, no polling | Inspect outcomes and reproduce |
| [Groups and governed membership](governed-membership.md) | A group joined by filter, held in a band by an evaporating intent and self-electing governors | Coordinate work |
| [Opacity and stigmergic routing](opacity.md) | A node reports its own saturation as a pheromone, and others route around it | Coordinate by meaning |
| [Intents](intents.md) | Desired state published as soft state that each node reconciles and that evaporates | Coordinate by meaning |
| [Adaptive stability](adaptive-stability.md) | Governors deciding through one control contract, enforced as far as the node's control profile says | Constrain actions |
| [The guarantee report and profiles](guarantee-report.md) | What a node enforces and why not the rest; a profile that refuses the start when it is unmet | Constrain actions |
| [The rule catalogue and the decision trace](rule-catalogue-and-decision-trace.md) | Every decision point described by id, and a bounded trace of what the instrumented ones decided | Inspect outcomes and reproduce |
| [Metrics and tracing](metrics-and-tracing.md) | Prometheus metrics on the gateway, logs through `tracing`, OTLP spans under `otel` | Inspect outcomes and reproduce |
| [The stem node](stem.md) | A generic node that declares from its unit file and, with `[hosts]`, installs signed artifacts as demand or a band calls for them | Acquire and adapt capabilities |
| [Demand-driven provisioning](demand-provisioning.md) | Hosts self-elect to install what an unmet requirement needs, within budget and from trusted publishers; restart is provisioning | Acquire and adapt capabilities |
| [The signed catalogue](signed-catalogue.md) | Gossiped, signed catalogue entries; a host installs only what a key it trusts signed, the bytes checked against their content address | Acquire and adapt capabilities |
| [The artifact library and publishing](artifact-library.md) | Bytes and a signed manifest in a directory, over HTTP or in an object store, served by a librarian and pulled over the mesh | Acquire and adapt capabilities |
| [Presence bands](presence-bands.md) | A floor and a ceiling of live providers, independent of demand, supervised by each stem that declares them | Acquire and adapt capabilities |
| [Shadow acceptance](shadow-acceptance.md) | A proposed entry loads only into a shadow lane until a listed reviewer co-signs it | Acquire and adapt capabilities |
| [Declared activation](declared-activation.md) | A placed blob handed to a native runtime by the operator's command, its capability gated on a probe | Acquire and adapt capabilities |
| [Confined activations](confined-activations.md) | An activation's command and probe run under Seatbelt or Landlock and seccomp, or refused by name | Acquire and adapt capabilities |
| [Serving a model as a skill](model-serving.md) | `[[serve]]`: a routable prompt skill registered while its install is live, retracted when it goes | Acquire and adapt capabilities |
| [Hosted WASM components](hosted-components.md) | Components sandboxed in-process with a confined KV and emit, a memory cap, a deadline and fuel by publisher | Acquire and adapt capabilities |
| [Unit files and wire-check](unit-files-and-wire-check.md) | One declaration per unit, read by the check, the stem and the gateway; `wire-check` names what could not bind | Acquire and adapt capabilities |
| [Skills](skills.md) | `skillrunner`: an agent role defined by a `.skill.toml` manifest, a node others reach by capability as `skill.invoke` | Run tools and reasoning |
| [LLM prompt skills](llm-prompt-skills.md) | A model-backed capability hosted on any node, its prompt template a KV key read fresh on every call | Run tools and reasoning |
| [MCP tools](mcp-tools.md) | Tools registered with a schema and discovered fleet-wide, served to outside clients at `/mcp`, external servers bridged in | Run tools and reasoning |
| [The reason router](reason-router.md) | `mycelium-reason`: model calls routed to a healthy server with failover, an OpenAI-compatible façade, traces and the checkpointer's blob store | Run tools and reasoning |
| [The tuple space](tuple-space.md) | `mycelium-tuple-space`: stage lanes workers pull from when ready, an atomic advance, at-least-once with failover | Coordinate work |
| [The blackboard](blackboard.md) | `mycelium-blackboard`: facts posted with attributes and claimed by predicate, exactly one claimer per fact | Coordinate work |
| [The contract net](contract-net.md) | `mycelium-commitment`: announce, offer, close, award once with a receipt, report and assess — signed records, no dispatcher | Coordinate work |
| [Federated domains](federation.md) | Two domains calling each other's exports without merging, through a signed per-partner catalogue and a body-bound credential on `/a2a` | Connect trust domains |
| [AgentFacts](agentfacts.md) | `mycelium-agentfacts`: a node's self-signed public facts at `/.well-known/`, verified by the fetcher against the key they embed | Connect trust domains |
| [The knowledge layer](knowledge-layer.md) | Signed, immutable claims, observations, assessments and decisions, resolved by a reader's policy; competing statements kept | Maintain knowledge and disagreement |
| [The wiki substrate](wiki-substrate.md) | `mycelium-wiki`: a group's curated canon in a store it does not own, written through one elected curator | Maintain knowledge and disagreement |
| [The adapter contract](adapter-contract.md) | One table of what each runtime adapter carries — persistence, cancellation, tracing, identity, grants, enforcement, receipts — each cell naming its test | Start from the published node |
| [Grants](grants.md) | Default-deny approval for one call, bound to the tool, the arguments' digest and the agent, checked at every door | Constrain actions |
| [The causal exposure policy](exposure-policy.md) | New work that would extend an unresolved closure held while the closure is past a threshold | Constrain actions |
| [The side-effect contract](side-effects.md) | An external destination's class declared before execution; a lost answer probed before at most one retry | Inspect outcomes and reproduce |
| [Selective recovery](selective-recovery.md) | One unknown effect's dependency closure read across the fleet and each member recovered by its class | Inspect outcomes and reproduce |
| [Fault classes against a declaration](test-faults.md) | `mycelium test --faults`: seven fault classes run against an in-process fleet built from a project's unit files | Start from the published node; Inspect outcomes and reproduce |
| [Span export](span-export.md) | A record's identity and outcome sent to an OpenTelemetry collector as a span, never its inputs — not an evidence path | Inspect outcomes and reproduce |
| [Recurring work](recurring-work.md) | A `[[schedule]]` run once per period by one self-elected holder, its runs keyed so two holders' runs of a slot are one | Coordinate work |
| [Context selection](context-selection.md) | Knowledge records selected for a run, each with its issuer, standing and the reader's verdict, never blended | Maintain knowledge and disagreement |
| [The multi-domain profile](multi-domain-profile.md) | `secure-multi-domain`: `start()` refuses unless the single-domain guarantees and the federation family hold | Connect trust domains |
