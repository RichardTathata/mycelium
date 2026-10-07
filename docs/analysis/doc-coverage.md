# Documentation coverage audit

> **Living matrix**, maintained by the `/doc-coverage` skill — refresh it as a *diff*, not a
> re-derivation. Seed run: **2026-07-10** (below). On each run, prepend a dated changelog entry, update
> the matrix cells that moved, and append to the calibration section any prior `Clear` cell later
> found thin.

A systematic audit of whether every core Mycelium concept has a clear landing across a
**WHAT · WHY · HOW × Dev · Ops** matrix — the doc analogue of `ratings.md`'s code audit. Run with
four parallel auditors (substrate · coordination primitives · fleet/groups/topology · security/
extensions/companions), each opening the actual docs and returning a per-cell verdict with the
file:section or the named gap. This file is the persisted result and the **re-run target**: a
future pass should diff against it, not start from scratch.

**Method.** Adversarial (a name-drop is not "Clear"). Cells: **Clear** (correct mental model +
actionable next step, in a doc addressed to that persona or cross-linked) · **Thin** (present but
partial / wrong-audience / HOW missing) · **Missing** · **N/A** (legitimately not that persona's
concern). WHY is usually shared Dev+Ops.

## Changelog

- **2026-10-07 (run 21)** — diff-gated over **#541–#551** (v2.26.0 and the unreleased 2.27.0 window: the
  verification policy and the `test-coverage` job, S5's blob reasons, A3's history source, the governance and KV
  doors, the ranked shed, the raw KV routes writing application keys only, the A2A card excluding plumbing and
  prompt skills, the coverage universe). #552 (`group_sizes`) was not merged and is not audited. Three parallel
  auditors (the KV and governance doors · A2A, prompt skills and the shed · blob reasons, the history source and
  the verification policy), each opening the pages and checking every must-work instruction against
  `src/agent/http.rs`, `src/agent/a2a.rs`, the mandate modules and both SDKs' sources. **The sweep the brief asked
  for came back clean:** no doc writes an owned namespace through `/gateway/kv` or an SDK `set()`, and none calls a
  prompt skill through `/a2a` (`llm/orchestrator` in guide 08 is a SkillRunner skill, which answers
  `skill.invoke`). **Floor before fixes: 4 ✗ cells (two Ops pairs) and 12 Tier-1 items, nine of them instructions
  that fail as written** — most predate the window and were exposed by reading the snippets against the code the
  window touched. **After: 0 ✗, 0 failing instructions in scope; three `~` — two Dev walkthroughs (an appointment-stream source with no example, a ceiling with no example) and the contributor how-to that lives in the wiki and a stale `CONTRIBUTING.md`, outside this run's scope.**
  Moves:
  - **Prompt skills · HOW·Dev ✓ → Thin → ✓** (calibration): guide 05's Rust registration and call were on
    `GossipAgent` (they are on `agent.llm()`, with an `Arc` backend), its Python client used a URL constructor, a
    `register` that does not exist, sync calls and attribute access on a dict; its generic skill-calling snippets
    called `agent.resolve` / `agent.rpc_call` (not on `GossipAgent`) and, in Python, sent `skill.invoke` through
    `rpc_call`, refused **403** `protected_kind` since 2.15.0 — now `A2aClient.send`. The deck copied the prompt
    client. Guide 05 now says a prompt skill is called at `/gateway/llm/call`, never `/a2a`.
  - **A2A (new row)**: guide 08's two Python snippets posted a non-JSON-RPC body (`-32601`) and read
    `input_schema` (the card says `inputSchema`) — now the `A2aClient` form with the wire shape; every "the card
    lists every capability" sentence in scope corrected (`00-concepts`, cookbook, `skillrunner.html`, the deck);
    `unit-file.md` § `[[serve]]` says how to call a served model; `production-readiness.md`'s negative probe no
    longer passes by naming a prompt skill; `confined-fleet.md` routes prompt skills to `llm:invoke`.
  - **Presence ceiling and ranked shed (new row)**: WHAT/HOW·Ops were ✗ — `capability-lifecycle.md` §5 gains
    *Presence ceilings*: which host withdraws, `prov-shed`, the `max − k` bound, the rolling-upgrade dip,
    overlapping bands, and how to see a withdrawal (`mycelium explain` over `--trace-dir`; there is no metric).
  - **Strict eligibility (folded into the scoped-mandates row)**: WHAT/HOW·Ops were ✗ — `audit.md` §8 gains
    what the operator owns (publishing, key revocation, forks forgotten on restart) and what
    `eligibility unknown` asks them to check. Guide 21 said "a head signed under a since-revoked key stops the
    walk", which the source's round 2 reversed (only the checkpoint head needs a current key); and "unreleased"
    for code in 2.26.0, with `design/scoped-mandates.md` the same.
  - **Governance doors · HOW·Ops ✓ → Thin → ✓** (calibration): `dynamic-scaling.md`'s three governance `curl`s
    sent no `Content-Type` and answer **415** before any parsing (the body run 20 corrected was right; the header
    was never there); the same in `cert-rotation.md`'s revoke, `federation.md`'s two federation calls, and the
    negative examples in `rbac.md` and `sso.md`. `tuning.md` said every governance POST is audited — `profile`
    never is, and none are without `compliance` **and** `[tls]` (the seal's error is discarded); it gains a
    `profile` row, and "set `target` on any intent" no longer invites a field `profile` silently ignores.
  - **KV doors (Security row)**: `rbac.md` § failure modes said to grant the scope or `"*"` for any 403, which
    never fixes `protected_key`; a row now says so. Guide 10's wire table gains the 403 and what each SDK raises
    (no typed error; TypeScript `delete()` drops the body); `deployment.md`'s client-facing upgrade list names
    §18–§19; `deprecations.md` §19's migration lists the capability and group routes exactly; the cookbook's
    "write your own `sys/load/`" is Rust-only now. `companions.md` told an operator to compact `cn/` offers over
    `POST /gateway/overlay/log/compact`, refused **403** `protected_stream` since #549 — it says there is no HTTP
    door.
  - **Hard topology (Layer III)**: guide 04's own #549 example sent a `group` the consistent-set route ignores;
    `error-handling.md` gains the override as the remedy, the **409**, and `ElectorateUnavailable` (missing from
    its `ConsistencyError`); `diagnostics.md` gains *Consensus refused — `topology_unsatisfied`* (the override has
    no lease); guide 02's "see chapter 13" pointed at a chapter with nothing on topology policies.
  - **Reasoning companion**: `companions.md`'s version line (0.6.2 → 0.7.0), a missing damaged-at-rest log line,
    and its probe — "for an id the error names" handed the operator the 12-character prefix the route refuses
    **400** `bad_id`; guide 15 listed four reasons and told the reader to retry unconditionally.
  - **What is proven**: two `ci.yml` line references had drifted; they name the step now.
  - **Outside scope, reported** (SDK READMEs, `CONTRIBUTING.md`, `examples/**` other than `examples/README.md`,
    `docs/wiki/**`): listed under § Run 21 below.

- **2026-10-06 (run 20)** — diff-gated over **#517–#538** (v2.22.0 → v2.25.0: the realignment repairs —
  durability R1/R2/R7, egress R3/R3b/R4, the gateway-free refusals R8, the timing setters R9, the SDKs S1–S5,
  architecture A1–A3). Three parallel auditors (durability · refusals · timing · configuration — egress ·
  authority · profiles — the SDKs and the checkpointer), each opening the pages and checking every must-work
  instruction against code; the instructions this run rewrote were run against a node (`[egress]` TOML →
  `egress.allow_list: enforced`; the corrected `govern/tuning` body answers `ok`, the documented one 400).
  **Floor before fixes: 0 ✗ cells, but four HOW instructions that fail when followed literally** — both SDK
  quick starts and guide 10 emitted a signal before subscribing and waited forever; `dynamic-scaling.md`'s live
  tuning `curl` answered 400; guide 10's A2A and prompt-skill snippets called the clients with the wrong
  arguments in both languages; `artifacts.md` told an operator to allow the store URL where 2.23.0 gates the
  endpoint, so a bucket on the list admitted nothing — **one SDK verb that never worked** (TypeScript
  `scatterGather`, 400 on every call since 2026-05-25 — fixed in `mycelium-ts` 0.2.1, #539, a code defect, not
  a doc gap), **one wrong security claim** (`sso.md`: a discovered JWKS host off the list refuses the start —
  it starts and 401s every token), and **a dozen stale sentences** from the window. **After: 0 ✗, 0 failing
  instructions.** Moves:
  - **New row: configuration ownership** (A2's `reference/configuration.md`), all five cells ✓ once its two
    rows R8 contradicted were corrected and it was linked from the operations funnel, `tuning.md` and
    `building-on`.
  - **Persistence and start refusals reach the operator**: `deployment.md` § *Persistence start refusals*
    (the four `persistence` messages, cause and action; the journals' one-owner and poison rules; per-pod
    PVCs), a readiness item, `error-handling.md` § *Start refusals* (every field `start()` refuses, by
    release), `deployment.md` § Rolling upgrades' table of start refusals by release; `config.rs`'s
    `persistence` rustdoc and `validate()` warning no longer promise an in-memory fallback.
  - **Runtime timing**: `tuning.md` says five hot params with the timing setters' bounds and `Result`, and
    documents `POST /gateway/govern/timing`; guide 22 shows the setters; `deprecations.md` §15–17 carry
    2.23.0–2.25.0's API notes; `installation.md` the toolchain floor (1.89; wasm host 1.94).
  - **Egress**: `artifacts.md`'s endpoint table; `crown-jewel.md`'s TOML form, redirects, region default and
    virtual-hosted S3, three failure rows; `sso.md`'s discovered-JWKS case; guides 05, 13, 17 and the
    threat model; `EgressPolicy`'s rustdoc; a glossary row.
  - **Authority and profiles**: guide 21's eligibility table states the head condition every rule needs and
    who the source is; `design/scoped-mandates.md` records D4; the profile table gains the **domain
    profile** (four things are called *profile*, not three) and one name for the guarantee profile;
    guide 20, `production-readiness.md` (a control-profile item) and `philosophy.md` point at it.
  - **SDKs**: the Python README's default scope, overlay requirement, `node_id`, and test recipe; guide 10's
    signal-stream data shapes; the checkpointer's bounded retry and an operator view of a persistent
    `IncompleteCheckpoint`; the dropped-signal log line in `observability.md`.
  - **Code gaps recorded, not papered** (§ Bugs): the timing door publishes out-of-range values every node
    ignores; the checkpointer cannot tell a lost or corrupt blob from a slow one; A3's strict eligibility has
    no shipped source; cooling-off answers `Unknown` before `Ineligible`; one `with_egress` takes a reference;
    five node-free Python test files run in no CI job. **Closing (2026-10-07):** the timing door, cooling-off
    and `with_egress` in the recorded-code-gaps PR (with Python `node_id`, the `/signals` data shape and the
    `LockGuard` docstring); the blob distinction in #542 (S5); A3's source in #543; the Python files in #541,
    which also made CI's test coverage something a job observes (verification policy rule 3).

- **2026-10-03 (zero gaps, not a run)** — the four `~` cells that recorded a *code gap* close with
  `docs/plans/zero-gaps.md` (#507–#512): the stem reads an object store and pulls past the frame cap
  (Z1/Z3), `[[serve]].api_key_env` (Z2), the whole-agent replay as a checked-in assertion (Z5), the
  effects refusal counter and the SDKs' receipt (Z6/Z7, in #507). The matrix now carries no `~` that
  says *code gap*; the `~` cells left are documentation shape (no single Dev chapter lists every start
  refusal; the declaration schema has no consumer walkthrough).

- **2026-10-03 (run 19)** — diff-gated over **31 commits since run 18** (v2.18.2 → v2.21.0: the guarantees
  MINOR, the decision-trace MINOR, profile rev 2, the fail-closed start refusals, the materials pass). Three
  parallel auditors (guarantees · profiles · refusals — the rule catalogue · the decision trace · what the
  trace does not show — the existing rows the window touched), each opening the pages and running or diffing
  every must-work instruction against code (`GOSSIP_PROFILE=secure-single-domain` on a default build: refuses
  naming 14 unmet; `mycelium tls issue --help`; `mycelium rules` / `explain`; `on_unreadable = "quarantine"`
  and `[egress]` through the real loader; the catalogue gate; the seven "changes no decision" tests; tutorial
  05's example; both materials gates). **Floor before fixes: 0 ✗ cells but one HOW instruction that fails if
  followed literally, one documented setting that silently no-ops when placed where the page implies, two
  non-compiling config literals, two `start()` refusals with no operator landing, twelve stale sentences, two
  claims resting on tests CI did not run. After: 0 ✗, 0 failing instructions, every refusal landed, the two
  tests in CI; three `~` that are recorded gaps.** Moves:
  - **Seven new rows** (below): the guarantee report; profiles (rev 2); a node certificate issued off-node;
    the fail-closed start refusals; the rule catalogue; the decision trace; what the trace does not show.
  - **Profiles · WHAT·Ops was stale on the go-live checklist**: `production-readiness.md` § 2 listed rev 1's
    fifteen under a "rev 2" heading and said `id.ca_key_off_node` "cannot be required yet" — followed
    literally, an operator ticking the item on a node that minted its own certificate gets a refusal the page
    says cannot happen. Rewritten (seventeen ids, the two settings, the issuing step); the same sentence on
    `cert-rotation.md`, `what-is-proven.md` and the plan's own header ("Nothing here is built").
  - **Fail-closed refusals · WHAT/HOW·Ops**: the gateway bind/cert-load refusal and the audit-sink-without-
    `[tls]` refusal had no landing outside CHANGELOG — `gateway-tls.md` (which still said "inert, stays
    plaintext") and `audit.md` (which still said "logged, not written") carry them now; `sso.md` says the
    issuer host must be in `egress.allow_hosts` or the node does not start; `federation.md` and guide 17 name
    `ClientError::Egress`; `crown-jewel.md`'s two wrong troubleshooting rows corrected; `EgressPolicy`'s
    rustdoc no longer says "MCP bridge only".
  - **KV persistence · HOW·Ops Thin → ✓**: `on_unreadable` was shown as a bare key and no page showed the
    `[persistence]` table — a top-level paste is silently ignored (no `deny_unknown_fields`); the table is on
    `deployment.md` now. **HOW·Dev ✓ → ✗ → ✓**: guide 01's and guide 13's exhaustive `PersistenceConfig`
    literals lacked `on_unreadable` since v2.20.0 (E0063; the struct has no `Default`) — the in-tree examples
    were fixed with the release, the guides were not. Fourth hit of the config-literal class run 16 swept.
  - **Security · HOW·Ops**: the `mesh:serve` window's closure was dated to 2.19.0 on `rbac.md` and
    `deprecations.md` (it is 2.18.2, `718dbc1c`) and guide 09 still described it as open; `rbac.md`'s scope
    table gained the two families it omitted (`fleet:read` incl. `/gateway/guarantees`, `govern:*`).
  - **Decision trace · HOW·Dev Thin → ✓**: no page showed how to attach a sink; guide 19 gained the snippet
    (`DecisionSink::new(SinkConfig {..})`, `with_decision_trace`, `StemOptions.trace`, the bundle attachment
    in the recording snippet) and its `mycelium-stem --units ./units` — a directory where the flag takes one
    file — corrected. **WHAT/HOW·Ops Thin → ✓**: `diagnostics.md` § reading what a node decided (the
    catalogue, `mycelium rules`, `mycelium explain`, the three bundle files, absent ⇒ unavailable);
    `operations/README.md` routes an operator to the report, the profile and the two catalogues.
  - **What the trace does not show**: `what-is-proven.md` now says precisely what `tests/decision_trace_replay.rs`
    carries (the recording half) and that the 20th-choice divergence was observed by hand; the inventory
    gained the row; guide 19 cross-links the finding. **Two claims rested on hand-run tests**: that file and
    `the_trace_makes_no_extra_draw_under_the_replay_seams` were in no CI job — both added (`ci.yml`, the
    `cli,sim` step). Still `~`: the divergence is not a checked-in assertion.
  - **Upgrade notes**: `deprecations.md` §6&7 named 2.5.0–2.8.0 only; now 2.19.0–2.21.0 too, plus two new
    sections (the three-argument activation closure; `TracePolicy::Partial` / `ClientError::Egress` gaining
    variants). Tutorial 05 retitled *Replay a bundle* — it never mentioned a decision.
  - **Glossary**: `00-concepts.md` gained *guarantee report · profile*, *rule catalogue*, *decision trace*.
  - **Calibration entry**: Confined fleet · HOW·Ops (✓ᴿ¹⁷) asserted "the substrate's own outbound paths fail
    closed" while the federation client and OIDC fetch were ungated until v2.20.0.

- **2026-10-02 (run 18)** — diff-gated over **41 commits since run 17** (v2.15.1 → v2.18.0: v2.16.0's
  settings, the design-time tooling MINOR, the stem-fleet MINOR). Three parallel auditors (the stem fleet
  and artifact delivery · the declaration format and unit files · v2.16.0's settings and run 17's `~`
  cells), each opening the pages and running or diffing every must-work instruction against code. **Floor
  before fixes: one security defect behind a documented setting, four instructions that fail if followed
  literally, three Missing cells, about twenty Thin. After: 0 ✗, 0 failing instructions, five `~` that are
  code gaps (named in the cells and on `operations/what-is-proven.md`).** Moves:
  - **Security · HOW·Ops was ✗ in effect** (carried ✓ᴿ¹⁷): `GOSSIP_GATEWAY_NAMED_TOKENS` — and both token
    tables — are honoured only under `compliance`, but parsed in every build; the default `mycelium`
    binary, given a named-token table and no positional token, **ran an open gateway with no warning**.
    A code defect, not a doc gap: fixed in its own PR (`start()` refuses, regression test
    `a_token_table_this_build_cannot_enforce_refuses_to_start` seen failing), with `rbac.md` §1,
    `tuning.md` and the Kubernetes README saying a token table needs a `compliance` build. Calibration
    entry (the *setting that silently no-ops* class, **second hit**, now with security impact).
  - **Capability lifecycle · HOW·Ops and WHAT·Dev ✓ → Thin → ✓**: the canonical unit-file example (the
    `capability_config.rs` module doc) and `capability-lifecycle.md` §5 joined keys with `;` — **not
    TOML**; pasted into a unit file they exit 2. Fixed, and the module-doc example is now a test
    (`the_module_doc_unit_file_example_loads`, seen failing at the semicolon). §2 listed 11 of 15
    findings and three of six flags; §4's `mycelium-artifact verify` exited with a usage error. New
    `docs/reference/unit-file.md` — every section and field with defaults, who reads what. Calibration
    entry (the 2026-09-28 row was Clear by opening the pages, not by pasting their snippets).
  - **Effects · HOW·Dev ✓ → Thin → ✓**: guide 18 called `OperationId::generate()` with no argument
    (it takes `&NodeId`) — did not compile. Fixed.
  - **Run 17's `~` cells**: Adaptive stability · HOW·Ops ✓ (v2.16.0's `control_max_staleness_ms` /
    `control_min_peers_heard`, env-applied); Replay · HOW·Ops ✓ (the `sim` binary's
    `GOSSIP_RECORD_BUNDLE_DIR` capture path); Companions · HOW·Dev ✓ (guide 21's wiki execution-authority
    section); Commitments · HOW·Ops ✓ — **it was never a code gap**: `KvHandle::compact_log` and
    `POST /gateway/overlay/log/compact` existed; run 17 recorded a doc gap as a code gap (calibration).
    Effects · HOW·Ops stays `~` (no refusal counter — code gap; the composed path now has an operator
    landing).
  - **New rows:** the stem node and fleet; declared activation and serving (D21, D22); artifact delivery
    (tool, publish door, ranged fetch, object stores); the declaration schema; SDK unit files (Q2).
  - **Staleness cleared:** the plan's *"Nothing here is built"* header, its X2 tail, `plans/README.md`'s
    *"Nothing built yet"*, guide 13's *"Docker cut not yet built"*, the module doc's *"not declared at
    startup yet"*, the wasm-host README's *"M12, in progress"*, the compose header's wave-3 sentence, the
    Makefile's three-demo usage line (and `examples-both-ways DEMO=llm_agent`, which had no code half).

- **2026-09-30 — plan F3 (design-time-tooling.md §17):** one new row, *Agent-authored functions*, all five cells Clear by opening the pages written for it (guide 16 § Agent-authored functions; `operations/artifacts.md` § Trust & provenance). Not a full re-audit; every other row carried.

- **2026-09-28 (design-time tooling, not a run)** — one new row, *capability lifecycle*: HOW·Ops is
  `capability-lifecycle.md` (declare · check · deploy · publish · watch · read, each step opened and
  its command run); HOW·Dev is the unit file's module doc and guide 02's two-column table; WHY is the
  plan's §12. Opened and read, not inferred from titles.
- **2026-09-28 (positioning pass, not a run)** — one new row, *what is proven*: a dated
  `docs/operations/what-is-proven.md` (WHY: why one page; HOW·Ops and HOW·Dev: the three tables with a
  gate, a bound or a missing proof per line, each opened and read), linked from the README, both decks,
  `production-readiness.md` and the shared-responsibility matrix. The rest of the matrix is carried.
- **2026-09-26 (run 17)** — diff-gated over **283 commits since run 16** (v2.5.0 → v2.15.1: the whole
  v3 contracts axis, Boundary H C1–C12, the election rewrite, three external reviews). Four parallel
  auditors (contracts axis · authority and evidence · stability/knowledge/federation · the carried rows),
  each opening the docs and diffing must-work instructions against code; the run-16 structural check
  (the literal-vs-struct sweep) reproduced across every guide chapter and the three operations pages.
  **Twelve new rows** (below). **Floor before fixes: 2 ✗ cells, 5 non-compiling or silently-failing
  HOW instructions, 11 Thin; after: 0 ✗, 0 non-compiling, 4 Thin that are code gaps rather than doc
  gaps.** Moves:
  - **Security · HOW·Dev was ✗ in effect** (carried ✓ᴿ¹³): `09-security.md`'s `TlsConfig { key_path,
    cert_path }` literal — neither field exists (`cert_pem`/`key_pem`); did not compile. Fixed; the
    chapter's own later snippet had it right. Calibration entry (the config-literal class, **third
    hit**; the run-16 sweep covered `PersistenceConfig` and missed this one because it swept the
    operations pages and guides 01/13 by name rather than every chapter — now every chapter).
  - **Layer III · HOW·Dev ✓ → Thin → ✓**: `04-consensus.md`'s two reference blocks called ten methods
    on `agent.` that live on `agent.consensus()` / `agent.service()`, called
    `start_consensus_listener()` without its `ConsensusConfig`, matched a `#[non_exhaustive]`
    `ConsensusResult` without a `_` arm (the release note says the arm must fail closed; the chapter
    showed the opposite), named `max_peers` as a `ConsensusConfig` field (it is `GossipConfig`'s), and
    cited a `/gateway/kv/scan` route that does not exist. All fixed. Calibration entry.
  - **Capabilities · HOW·Dev**: `02-capabilities.md`'s `CapabilityGroupDef { filter, provides,
    requires }` omits `topology_policy` with no `..` — did not compile. Fixed (found by the sweep, not
    a cluster). Calibration entry.
  - **Persistence · HOW·Ops ✓ → Thin → ✓**: `production-readiness.md` §3 still prescribed *"restore =
    put the dirs back + restart"*, the exact wording `deployment.md` retracted the same morning;
    rewritten to the quiesce/snapshot rule and the back-up-together set. Calibration entry (same-day
    drift inside one funnel: the runbook moved, the checklist that links it did not).
  - **Security · HOW·Ops (A2A)**: the readiness checklist's negative probe, written 2026-09-26, named
    `message/send` — a method the handler does not know, so it passed vacuously; now `tasks/send`.
    And `cargo run --example identity_one_record` bare, where the demonstration needs
    `--features tls,compliance`. Calibration entries (both in text hours old).
  - **New rows, audited across all five cells** — Contracts & receipts ✓✓✓✓✓ (HOW·Dev gained
    `prepare_write`/`commit_prepared`) · Effects companion (WHAT/HOW·Dev were **Missing** outside
    rustdoc; guide 18 § *Rung 4 in practice*, a glossary row, a cookbook entry) · Replay & simulation
    (HOW·Dev **Thin**: the node-recording recipe lived only in a companion's test — guide 19
    § *Recording a node*; HOW·Ops **Thin**: diagnostics.md implied an operator capture path that does
    not exist — reworded; code gap recorded) · Commitments ✓ (README and guide 24 drifts fixed) ·
    Gateway/SDK receipt parity (WHAT/HOW·Dev **Thin** for raw HTTP: no doc described the routes, the
    2.14.0 400, or that the plain write returns no receipt — guide 10 § *The HTTP surface*; deprecations
    §12; code gap recorded) · Gateway caller identity (HOW·Ops **Thin**: named tokens recommended three
    times and shown nowhere — rbac.md §1; guide 09's principal forms were un-qualified) · Action
    evaluator + evidence ✓ (guide 20's two stale 2.14.0 paragraphs rewritten; AE3 corrections still
    only in the ADRs — Tier 3) · Scoped mandates + authority at execution (WHAT/HOW·Dev **Thin**: guide
    21 stopped at the wiki fence — new § *Presenting and enforcing a mandate*; HOW·Ops: readiness item
    added; `mesh:serve` window → deprecations §11) · Member removal (WHAT/HOW·Ops were **stale**:
    cert-rotation.md said force-removal was *not yet provided* — new § *Removing a member*; readiness
    item) · Confined fleet (HOW·Ops: the runbook was unreachable from the operations funnel — README
    row added) · Adaptive stability / control (HOW·Ops **Thin**: step 2 told operators to loosen a bound
    that has no setter — reworded, code gap recorded; step 4 gained the `RightsLedger` setup) ·
    Knowledge layer (WHAT/HOW·Ops were **Missing**, WHAT/HOW·Dev Thin: nothing after item 3 had a Dev
    or Ops landing — guide 23 gained three sections and the run command's required feature;
    companions.md gained the operator's section) · Federated domains (HOW·Ops **hole**: the enforced
    domain profile was never told to the operator; `GOSSIP_DOMAIN_PROFILE` now in federation.md step 0,
    guide 17, tuning.md; the SDK version gate named an unreleased 0.2.5) · Public discovery /
    AgentFacts (HOW·Ops Thin: rotation behaviour — cert-rotation.md paragraph) · Election / leadership ✓
    (the 30 s floor TTL now stated in diagnostics.md).
  - Also: threat-model §7 was inserted before §6 (2026-09-26, mine) — moved; `08-a2a-interop.md` had no
    word on optional auth or the evaluator — a callout; the legible-emergence taxonomy now links its
    content-plane addendum; `mycelium-sim`'s crate doc and `mycelium-commitment`'s README each carried
    a "not yet" that had been true for weeks; `production-readiness.md` §7's companion list omitted
    four crates; `rbac.md` says scopes match exactly or `*` (a token scoped `llm:*` admits nothing).
  **Code gaps surfaced (not papered):** (1) `ConfidenceBound` has no setter — every governor hardcodes
  the default, so the control runbook's "loosen the bound" could never be followed; (2) `POST
  /gateway/kv` discards the write's receipt and answers `{"ok":true}`, so an HTTP/SDK client cannot
  get rung 2 for an ordinary set; (3) no operator bundle-capture path exists (`sim_seam::install` is a
  per-thread API call); (4) `gateway_named_tokens` — the credential model the docs say to prefer — has
  no env var, unlike `gateway_auth_token`; (5) `scope_admits` is exact-or-`*`; (6) three examples
  (`identity_one_record`, `auditor_questions`, `coordination_viz`) declare no `required-features`
  though their headers need them, so the bare command runs a hollow demo; (7) two TTLs read one
  `sys/govern/membership/{group}` key (governor 5 min, electorate floor 30 s) — probably intended,
  stated nowhere but diagnostics.md now. **Floor after fixes: 0 ✗ cells; Tier-1 open: 0 doc, 4 code.**
  **Same day, the code gaps closed** (`fix/doc-coverage-code-gaps`): (1) `control_max_staleness_ms` /
  `control_min_peers_heard` on `GossipConfig` + env, every governor through
  `ConfidenceBound::from_config`; (2) `POST /gateway/kv` returns `operation_id` / `local_durability`;
  (3) the node binary records a run under `sim` with `GOSSIP_RECORD_BUNDLE_DIR`; (4)
  `GOSSIP_GATEWAY_NAMED_TOKENS`; (5) `validate()` refuses a family wildcard by name rather than
  widening `scope_admits` (a silent grant at upgrade was the wrong fix); (6) two of the three examples
  already had entries — `coordination_viz` and `control_envelope_viz` gained theirs; (7) the two TTLs
  are pinned by a test and cross-referenced in their doc comments. The four `~` cells above return to
  ✓ once that branch merges.
- **2026-09-05 (run 16)** — diff-gated over **20 commits since run 15**: the reason 0.6.0 PAIR imports
  (OpenAI façade, router reservations, `llm_meta` + Ollama collector, `openai_serve`), two gateway-auth
  fixes (companion routes 09-04; node-level `/mcp` `/signals` `/consensus/{slot}` 09-05), the three P1
  persistence fixes + `Committed { persisted }` (v2.4.2), the wiki erase verb, mycelium-py 0.2.3
  pooling, checkpointer 0.1.1. Four clusters re-audited (the parallel auditors were rate-limited before
  reporting; the audit was run inline against the same rubric — every verdict below was made by
  opening the doc and diffing against code). **One new row:** *KV persistence (WAL + snapshot)*, split
  out of Layer I — its durability contract is now a named invariant with five distinct landings, and
  it is where this run's misses cluster. **Moves:**
  - **Persistence · HOW·Dev was ✗ in effect** (carried ✓ since run 1): both Dev-guide
    `PersistenceConfig { … }` literals **did not compile** — `01-gossip-kv.md` used `data_dir`,
    `13-cluster-topology.md` used `path` (the field is `base_path`) and both omitted the two required
    snapshot fields. Fixed (+ a paragraph on what an ack means per `SyncMode`). Calibration entry —
    the config-literal class, second hit; **structural fix:** a mechanical literal-vs-struct sweep over
    every `*Config`/`*Token`/`*Policy` literal in `docs/guide` + the three operations pages (0 unknown
    fields after the fix) — reproduce it each run, do not spot-check.
  - **Persistence · WHAT/HOW·Ops Thin → ✓:** `deployment.md` gained *§ Persistence modes* (the
    ack-meaning table per `sync_mode`, consensus always-fsynced, `persisted`, snapshot knobs) and its
    restore step no longer says "replays the WAL *up to* the latest snapshot" (backwards: snapshot,
    then WAL tail, LWW). **WHAT·Dev Thin → ✓:** `00-concepts.md` gained the *replication vs.
    persistence* pair (survives a node vs. survives the cluster).
  - **Layer III · HOW·Dev:** `04-consensus.md`'s example `Committed { slot, value, ballot }` stopped
    compiling the moment v2.4.2 added `persisted` (same-day drift, not a prior miss). Fixed and the flag
    landed for Devs (what `false` means; match with `..` if unneeded).
  - **Reasoning · HOW·Ops Thin → ✓:** `operations/companions.md` had **no reason section at all** —
    the façade's exposure, scopes, the collector, and "what to run" lived only in the crate's
    examples README and the Dev chapter. Block added. WHY/WHAT/HOW·Dev ✓ (chapter 15 covers façade
    routes — verified against `http.rs` — reservations with `reservation_weight` default 0.1 verified,
    `llm_meta`, the PAIR positioning; run commands' feature flags exist).
  - **Security:** all five cells ✓ *after* the two fixes; but two prior-run `Clear` verdicts were
    false in code — calibration entries below (the public-surface statement; the "always fsynced"
    guarantee). Must-work re-checked: scope names in `rbac.md`/`09-security.md` = `required_scope`;
    the curl checklist statuses match the middleware; `GOSSIP_GATEWAY_AUTH_TOKEN` is applied by
    `apply_env_overrides`; the OpenAI-client "API key = bearer" path works because the façade sits under
    the same layer.
  - **Companions:** carried ✓; the erase verb has Ops (`companions.md`, `data-erasure.md`) and now a
    Dev pointer in the cookbook wiki recipe (Tier 3). Pooling is internal (`_pool.py`, no knob) — no
    doc needed beyond the changelog.
  **Code gaps surfaced (not papered):** (1) **neither SDK can present a gateway bearer** — the Python
  `Agent(host, port, timeout)` and the TS client send no `Authorization` header — so every
  token-protected deployment the docs recommend for non-loopback exposure is unusable from the SDKs
  (see *Bugs the audit surfaced*); (2) the SDKs read only `ok` from consensus responses, dropping
  `persisted`. Also fixed in passing: the cookbook's crate-choice link pointed at `docs/guide/README.md`
  for a heading that lives in the repo-root `README.md` (dead since 2026-07-13; the lint's link sweep
  never covered the cookbook — ledger entry in `wiki/dev/.log/lint-calibration.md`). Floor after fixes:
  **0 ✗ cells, 0 Tier-1 open**; before fixes this run found **1 ✗ in effect** (non-compiling Dev
  literals) and 3 Thin.
- **2026-08-16 (run 15)** — diff-gated. Since run 14 the delta is **one concept row: Companions
  (the wiki third)** — the council-substrate arc (GitMirror change sink · `GitStore` + six
  hardening items incl. two recorded measurements · the bulk-ingest claim-check with a gateway
  edge + py/ts SDK verbs · the pluggable `PageFormat` codec), plus two design records and a
  hardening plan; the only other commit in the window was a wiki-companion design note (8508bb4).
  **Re-audit verdicts:** WHY richer (three cross-linked records) ✓; WHAT·Dev ✓ (crate rustdoc +
  concepts unchanged); **HOW·Ops was Thin for the new surface** — the git-as-truth deployment
  shape (topology, refresh-refusal semantics, the ≤1-round durability window, the batch-atomic
  gate, ingest sizing) had design records but *zero operator-runbook landing* — fixed in-run: a
  "Git-as-truth deployments" block in `operations/companions.md` mirroring the GitMirror block;
  **HOW·Dev** had only the "deliberately not a GitStore" clause — fixed: the cookbook recipe now
  names the envelope option + the ingest surface with the architecture link. Must-work spot-checks
  on run-14→now snippets pass (GitMirrorConfig fields, `push_divergences`, `rebuild`). Row stays ✓
  post-fix; **no new concept row** (GitStore/ingest are wiki-companion internals — the run-11
  identity-auth precedent). No calibration entry: the Thin was new-surface drift since run 14,
  caught by the first run after it. Floor unchanged: **0 ✗ cells, 0 Tier-1.**
- **2026-07-26 (run 14)** — diff-gated. **Zero source change** since run 13 (`src/` · `mycelium-*/src/`
  untouched); the only `docs/` delta is `4ecc1aa` (install-story fix) + `7687242` (wiki lint). **No
  matrix cell moves — all rows carried.** One **must-work-if-followed** observation recorded — same
  class as run 13's `key_path` / regenerate-key hits, but on the *most foundational* Dev instruction:
  `building-on-mycelium.md` §1 had told integrators `mycelium = "2"` / `mycelium-core = "2"`, crates.io
  **version** deps that resolve against an **unrelated, dormant 2019 project** of the same name
  (`gitlab.com/matthew.bradford/myceliumdds`, 0.1.1) — so a literal `cargo add` / build pulls the wrong
  crate or fails. Fixed (already merged, `4ecc1aa`) to **git-tag deps** (`git = "…RichardEko/mycelium",
  tag = "v2.3.0"`; the two companions on their own tags), which resolve to this repo and carry the
  workspace-internal `mycelium` automatically — **must-work re-verified** (the three tags exist; the
  workspace resolves at `mycelium` v2.3.0). The companion re-versioning (guardrails 1.0.0 / reason
  0.5.0) and the git-tag-not-crates.io distribution constraint also gained a wiki home
  (`companions.md`, `history.md`, via the 2026-07-26 lint). **Scope note (not a false-Clear — the
  install line was never a scored concept cell):** the *dependency/install* snippet is the most
  upstream HOW·Dev step yet sat outside the matrix's concept inventory while silently broken; the
  presence-is-not-sufficiency spot-check should treat any documented dependency snippet as
  must-work-if-followed going forward. Considered an *Installation / dependency* row and left it out
  (onboarding HOW, not a substrate concept) — flagging the spot-check scope instead. Floor unchanged:
  **0 ✗ cells, 0 Tier-1.**

- **2026-07-24 (run 13)** — diff-gated over the **SOC 2 audit-gap arc** (2026-07-22, on `main`: WS-A…F
  — gateway TLS, audit export/checkpoint, `sys/identity` authentication 1a/1b/2/3, revocation glue,
  crypto-shred erasure). Two moves:
  - **New concept row — Data erasure (crypto-shred).** `SubjectKeyRegistry` (WS-F): WHY
    `design/data-lifecycle-and-erasure.md`, WHAT/HOW·Ops `operations/data-erasure.md`. WHAT/HOW·Dev
    were initially **Thin** (the runnable API lived only in the ops runbook + rustdoc, no Dev-addressed
    landing) → closed by adding the control to the `09-security.md` Dev "Compliance controls" table.
    Row lands ✓✓✓✓✓.
  - **Security (TLS/RBAC/SSO/audit) — enriched + two `must-work-if-followed` bugs fixed.** The arc
    added six Dev-facing controls (gateway TLS, audit sink, audit checkpoint/prune, compromise
    remediation, `require_identity_proofs`, erasure) that `09-security.md` (the Dev chapter) named
    **none** of — plus a stale "gateway can't be TLS'd natively" note. Fixed by a new Compliance-controls
    table + runbook links. **And the presence-is-not-sufficiency spot-check caught two pre-existing
    bugs in that chapter's Dev Notes** (see Calibration): a `TlsConfig { key_path: … }` example that
    does **not compile** (the field is `key_pem`), and a false "default regenerates the key every
    restart" claim (the default persists to `auto_cert_dir` and reloads). Both fixed; Security · HOW·Dev
    re-verified genuinely Clear. Calibration entry appended (the 7th). All other rows carried.

- **2026-07-20 (run 12)** — diff-gated. Delta since run 11: **v2.2.0** (tag only — hardening fixes,
  no landing moves), the **scrape-fleet launcher examples** (deployment utilities; cluster-name row
  re-verified: `13-cluster-topology.md`'s `apply_env_overrides()` warning is accurate and the new
  launchers comply — carried ✓), and the **capability lease** (`lease_secs` +
  `/gateway/capability/{id}/heartbeat`, this session) — a new API surface on the Capabilities row
  that also *exposed a pre-existing doc falsehood*: for **gateway-bridged** advertisers the refresh
  loop runs in the node, so `02-capabilities.md`'s "stops refreshing → evaporates" claim and
  `10-language-bridges.md`'s "`handle` keeps the advertisement alive" comment silently inverted the
  crash semantics (a crashed bridge client left a permanently-live advert — the scraper-fleet w15
  incident). Three cells briefly Thin, **all fixed in-run**: `10-language-bridges.md` (lease +
  heartbeat in both SDK blocks with the liveness warning — HOW·Dev), `02-capabilities.md` (refresher-
  liveness nuance callout — WHAT·Dev), `operations/diagnostics.md` (new "Stale bridged advert"
  pathology entry, the inverse of the coverage gap — HOW·Ops). Verdicts re-land ✓. Calibration entry
  appended (the 6th — 2nd found by a live incident rather than an audit). All other rows carried. — diff-gated, after the five-pass code audit (Runs 50–58, ~40 fixes) + the
  identity-auth work. **No cell verdict moves** (all stay ✓), but **two "must-be-accurate-if-followed-
  literally" staleness fixes** in the *scored* Ops runbooks, both created by this session's own code
  changes:
  - **Security · HOW·Ops** — `operations/cert-rotation.md` step 2 claimed `sys/identity` is "signed by
    the **old** key"; the code writes it **unsigned** (the identity-poisoning gap). Corrected to state
    unsigned + linked the new **`design/identity-authentication.md`** ADR — which also **enriches
    Security · WHY** (the identity trust-model + the phased fix). Calibration entry added (the **4th**
    claim-present-but-false hit — the **1st a *security guarantee***, not a setting/command).
  - **Operational readiness (HOW·Ops)** — `operations/observability.md`'s `/ready` row said "capabilities
    advertised + no dead shards"; this session changed `/ready` to reflect **startup completion** (a node
    advertising no soft state is now ready). Corrected. (Same drift also fixed in `wiki/dev/operations.md`
    during this session's wiki-lint.)
  The ~40 other code fixes are **bug-fixes that do not move concept landings** — a diff-gated carry for
  every other row. Zero new concepts (identity-auth is a *design* for the existing Security concept, not
  a new sub-handle/companion/standard).
- **2026-07-15 (run 10)** — diff-gated: **no cell moves; carried from run 9.** The delta is the two new
  **artifact-library browser showcases** (`provisioning_viz` :8097 — autonomic self-heal; `catalog_viz`
  :8098 — origin-death survival), both UI-contract compliant (verified live). Concept impact lands on the
  already-✓ **Artifacts / library · HOW·Dev**: a **visual landing** now exists alongside the CLI demos +
  the operations walkthrough — a Tier-3 discoverability add (same shape as runs 6–8), not a verdict
  change. Their run commands carry `--features wasm,metrics` (no repeat of the run-9 gap). Zero
  `src/`·`mycelium-*/src/` change.
- **2026-07-15 (run 9)** — diff-gated. Delta since run 8: the examples **capability-matrix**
  restructure (discoverability; rows already ✓), the **artifact deploy/install surfacing** (a guide
  ladder "Artifacts & deploy" row → `operations/artifacts.md § Solution/Dev`), the **`## Loads`
  banner** across the 5 runtime-loading demos, and the **philosophy `.html`→`.md` port** (WHY home
  renamed; content verbatim — WHY cells carry). Concept impact lands on **Artifacts / library ·
  HOW·Dev** (was `✓ ᵀ²`, mis-homed):
  - The surfacing + Loads banner make the Dev walkthrough **cross-linked from the Dev guide** and the
    5 demos self-declare **content · type · loaded-from** — the mis-homing is resolved.
  - **But the "must work if followed literally" check surfaced a real bug:** the `catalog` and
    `provisioning` run commands **omitted `--features wasm`** in **5 places** (the new guide-ladder row,
    `operations/artifacts.md`, `coop/README.md` ×2, and `presentation.html`), so following them
    literally fails with `error: target … requires the features: wasm`. **Fixed all 5** — the cell is
    now *genuinely* ✓ (cross-linked walkthrough + working commands + the Loads banner). Calibration
    entry below. *(`presentation.html` — a persuasion surface — was edited for the run-command fix only;
    flagged for the next publication-lint.)*
- **2026-07-15 (run 8)** — diff-gated: **no cell moves; carried from run 7.** The only concept-touching
  delta is the new `wiki_council_viz` browser showcase, which enriches the **Companions** row's
  wiki-companion HOW·Dev landing (a watchable specialist-fleet demo alongside the `wiki_chat` CLI). That
  row was already `✓✓✓✓✓`, so this is a Tier-3 discoverability add, not a verdict change — the same
  shape as runs 6–7 (a new/better landing for an already-Clear concept). Zero `src/`·`mycelium-*/src/`
  change; no calibration hit.
- **2026-07-15 (run 7)** — diff-gated: **no cell moves; carried from run 6.** The delta is the
  examples completeness sweep (guardrails · `mycelium-reason` · `wiki_chat` indexed; README restructured
  into one flow) + the Ops Console **Audit** tab. Concept impact lands on two already-`✓✓✓✓✓` rows:
  - **Reasoning / LLM / MCP / guardrails · HOW·Dev** — genuinely Clear before: `guide/16-guardrails.md`
    links the runnable `guardrail_fleet` / `guardrail_wedge`. The user's "no examples for guardrails"
    gap was examples-*index* discoverability (`examples/README.md` didn't list them), fixed under
    wiki-lint — **not** a concept-doc gap. No move.
  - **Security (TLS/RBAC/SSO/audit) · HOW·Dev** — Clear at the **API** level (`09-security.md`
    § "in practice" already has write/verify + a `GET /gateway/audit` curl), but it cross-linked **no
    runnable way to *see* the trail** — the "no examples for viewing audit" the user actually hit.
    **Fixed** (Tier-3 discoverability): a "See it live" pointer to the `community` mgmt UI + the new
    Ops Console Audit tab. Cell stays ✓. Calibration entry below.
  Zero `src/`·`mycelium-*/src/` change.
- **2026-07-14 (run 6)** — diff-gated: **no cell moves; carried from run 5.** Since run 5 the only
  concept-touching docs delta is on **Reasoning / LLM / LangGraph**: the FAQ now cites the langgraph
  `deploy/reheal` rung for its "survives the loss of the orchestrator node" claim (`eb89232`), and
  `15-reasoning-and-langgraph.md`'s flagship section links back to that FAQ positioning (`28ad9b1`) —
  so FAQ-claim ↔ ch15-how-to ↔ example are now three-way connected. That row was already `✓✓✓✓✓`, so
  this is a **Tier-3 discoverability improvement** (WHY↔HOW connectivity, shown-not-told), not a
  verdict change. Everything else in the delta is examples hygiene (orphan `mesh_demo` deleted,
  `diagnostics` registered, operator demos re-themed + ops-linked) and non-concept prose (the broker
  bullet de-jargon) — no concept's landing gained or lost. Zero `src/`·`mycelium-*/src/` change. No
  calibration hit (no prior-Clear cell found Thin).
- **2026-07-14 (run 5)** — diff-gated. Delta since run 4 is the `cluster_name` work (`f5c7f6c`,
  `15a33eb`): every example now sets a cluster name, and `guide/13-cluster-topology.md` gained the
  `apply_env_overrides()` caveat. **Calibration hit** (below): **Membership + cluster_name · HOW·Dev**
  was ✓ in run 1 (the seed called this corner *"the strongest, Clear on every cell"*), yet the
  documented `GOSSIP_CLUSTER_NAME=…` way to set it **silently no-ops** unless the app calls
  `apply_env_overrides()` — a real user hit exactly this. The instruction was *present* but did not
  *work when followed literally*. **Fixed** (`13-cluster-topology.md` ⚠️ caveat + build→apply→new
  sequence); the cell is now genuinely ✓. Separately, the **Ops Console** (`examples/ops_console.rs`)
  enriches **Observability · HOW·Ops** (a live dashboard over `/stats`·`/gateway/fleet`·
  `/gateway/diagnose`·`/metrics`) but that cell was already ✓ — no verdict move. No other concept
  touched; the rest carry from runs 3–4.
- **2026-07-14 (run 4)** — diff-gated: **no material diff to the matrix; carried unchanged from run 3.**
  Zero product-core (`src/` · `mycelium-*/src/`) change since run 3. The whole delta is
  examples/tooling/docs: four browser **visual showcases** (`microgrid_viz` · `stigmergy_viz` ·
  `redistribution_viz` · `llm_council_viz`, the `/state`+canvas pattern) plus their discoverability
  across all three surfaces (wiki `dev/examples.md`, `examples/README.md`, the presentation deck), the
  conway bind/URL fixes, and the local scale-nightly runner. This **enriches** the
  Companions (tuple-space/blackboard) and coordination **HOW·Dev** cells — a reader now has runnable
  visual demos — but those cells were already ✓, so **no verdict moves**. No new concept (a visual
  showcase is an example *category*, not a substrate concept warranting a row). Not re-audited; the
  next scored re-audit waits for a concept-cell-moving change (product code, a new sub-handle/companion,
  or a found gap).
- **2026-07-13 (run 3)** — diff-gated re-audit. The only material diff since run 2 is this session's
  two commits: `8456dc4` (wiki-store **section-granular CAS** — the dual-curator lost-update fix) and
  `d316cdf` (the **`coordination-approaches.md`** design note + cross-links). **No concept cell
  regressed.** The wiki-store CAS is an internal correctness fix, documented in `companions/wiki.md`
  and `wiki-concurrent-edit.md §3.5` (agent/WHY-facing — no new persona gap). The design note **closes
  a latent WHY gap** and produced a **calibration hit** (below): runs 1–2 scored **Distributed
  locks · WHY** and **Companions · WHY** as ✓, but the *cross-cutting* decision — *when to reach for
  the distributed lock vs the capability ring, and why all three companions reject it* — had no
  user-facing home. Each primitive's own rationale was covered; the **comparison spanning
  Locks+Companions+Consensus fell between the matrix rows** (a structural blind spot the row-by-row
  scoring cannot see). *Fixed:* `docs/design/coordination-approaches.md` (CP-vs-AP decision matrix +
  the rule + a fourth-companion checklist), cross-linked so **both** personas reach it — Dev via
  `04-consensus.md` / `faq.md`, Ops via `companions.md`, plus `exactly-once-effect.md`,
  `wiki-concurrent-edit.md`, and `docs/README.md`. All rows carry; WHY for Locks/Companions/Consensus
  is now genuinely — not nominally — Clear.
- **2026-07-11 (run 2)** — diff-gated re-audit. Nothing in the existing matrix's *concept* cells
  changed since the seed (the post-seed commits were the wire-compat gate, wiki ingests, the two new
  skills, and persuasion-surface fixes) — those rows **carry**. One **new concept row** was surfaced
  by the wire-compat gate: **Rolling upgrade**. WHY/WHAT/HOW·Dev were covered (`building-on-mycelium`,
  `faq`, `09-security`, `error-handling`); **HOW·Ops was Thin** — only a one-line "supported"
  assurance in `production-readiness`, no procedure. *Fixed:* added `operations/deployment.md §
  Rolling upgrades` (node-by-node procedure + the two-step-gap tripwire) with cross-links from
  `09-security` and `production-readiness`. Also fixed a **staleness the seed missed** —
  `09-security.md` cited wire **v10/v9** as current → **v12/v11** (logged under Calibration).
- **2026-07-10 (run 1, seed)** — the full four-auditor audit + Tier 1–3 remediation (below).

## Headline

The architecture holds up: `docs/README.md` assigns every area a document *type* and each doc a
declared *audience*, so the WHAT/WHY/HOW × Dev/Ops matrix is how the tree is actually cut, not a
retrofit. At audit time the large majority of cells were already Clear, **no cell was a black
hole**, and the recently-reworked cluster/group/`cluster_name` corner was the strongest (Clear on
every cell). The gaps clustered in one place: **operational failure-mode runbooks for the
consensus/lock family, and Dev guide-chapters for two shipped features.** All of them are now
closed (Tiers 1–3); the residue is genuinely nothing at ✗ or `~`.

## Final matrix (post-remediation)

Legend: ✓ Clear · — N/A. Every cell that was ✗/`~` at audit time is annotated with the pass that
closed it.

| Concept | WHY | WHAT·Dev | HOW·Dev | WHAT·Ops | HOW·Ops |
|---|:--:|:--:|:--:|:--:|:--:|
| Layer I — Gossip KV | ✓ | ✓ | ✓ ᵀ² | ✓ | ✓ |
| KV persistence (WAL + snapshot) — split from Layer I, run 16 | ✓ ᴿ¹⁶ | ✓ ᴿ¹⁶ ᴿ²⁰ (`config.rs` rustdoc and `validate()` warning no longer say a node falls back to memory; the `Io` gloss drops WAL replay) | ✓ ᴿ¹⁶ ᴿ¹⁹ (guide 01 + 13 literals carry `on_unreadable` — the struct has no `Default`) ᴿ²⁰ (guide 01: the receipt verbs named; the direct-embedder obligation (`OwnershipLock` → `hold_ownership` → `trigger_snapshot`)) | ✓ ᴿ¹⁶ ᴿ²⁰ (`deployment.md`: a torn tail is truncated before the first append, not appended after) | ✓ ᴿ¹⁶ ᴿ¹⁷ ᴿ¹⁹ (`deployment.md`: the `[persistence]` table — a bare key no-ops) ᴿ²⁰ (`deployment.md` § *Persistence start refusals* — four messages, cause, action; journals' one-owner and poison rules; per-pod PVC; readiness § 3 item) |
| Layer II — Signal mesh | ✓ | ✓ | ✓ ᴿ²⁰ (both SDK quick starts and guide 10 subscribed after emitting and waited forever; guide 10's table gains both signal streams' data shapes) | ✓ | ✓ ᴿ²⁰ (`observability.md`: `Signal handler channel full; signal dropped` — what it means and what to do) |
| Layer III — Consensus | ✓ | ✓ ᴿ²¹ (`error-handling.md`: `ElectorateUnavailable`, the 409 and the override as `TopologyUnsatisfied`'s remedy) | ✓ ᵀ² ᴿ¹⁶ ᴿ¹⁷ ᴿ²¹ (guide 04: the consistent-set body sent a `group` the route ignores; the Rust form of the override, no lease) | ✓ ᵀ¹ | ✓ ᵀ¹ ᴿ²¹ (`diagnostics.md` § Consensus refused — `topology_unsatisfied`) |
| Capabilities / groups | ✓ | ✓ | ✓ ᴿ¹⁷ | ✓ | ✓ |
| Distributed locks | ✓ | ✓ | ✓ | ✓ ᵀ¹ | ✓ ᵀ¹ |
| Services / RPC | — | ✓ | ✓ ᵀ² | ✓ | ✓ |
| Schema lifecycle | ✓ | ✓ | ✓ | ✓ | ✓ ᵀ¹ |
| Scopes (Cluster/Group/Individual) | ✓ ᵀ³ | ✓ | ✓ | ✓ ᵀ³ | — |
| Membership + cluster_name | ✓ | ✓ | ✓ | ✓ | ✓ |
| Groups (three kinds) | ✓ | ✓ ᵀ³ | ✓ | ✓ | ✓ |
| Legible Emergence | ✓ ᵀ³ | ✓ | ✓ ᵀ³ | ✓ | ✓ |
| Security (TLS/RBAC/SSO/audit) | ✓ | ✓ ᴿ²¹ (guide 10's wire table: the KV doors' 403 `protected_key`) | ✓ ᴿ¹³ ᴿ¹⁷ ᴿ¹⁹ (guide 09: the serve window is closed since 2.18.2; `cert_pem` means an issued certificate) ᴿ²¹ (guide 10: what each SDK raises on `protected_key`/`protected_stream`; `deprecations.md` §19's routes exact) | ✓ ᴿ¹⁹ (`rbac.md`: `fleet:read` + `govern:*` rows; the window dated right) ᴿ²¹ (`rbac.md`: the consensus row's refusals current) | ✓ ᴿ¹⁷ ᴿ¹⁸ ᴿ¹⁹ (`gateway-tls.md`: refuses, not inert, and the start-time bind; `audit.md`: a sink without `[tls]` refuses the start) ᴿ²⁰ (`sso.md`: the start check covers a *configured* `jwks_uri` only — a discovered one off the list starts and 401s every JWT; two failure rows) ᴿ²¹ (`rbac.md` § failure modes: no scope fixes `protected_key`; `deployment.md`'s client-facing upgrade list; five JSON `curl`s gained `Content-Type`) |
| Data erasure (crypto-shred) | ✓ | ✓ ᴿ¹³ | ✓ ᴿ¹³ | ✓ | ✓ |
| Artifacts / library | ✓ | ✓ | ✓ ᵀ² ᴿ⁹ | ✓ | ✓ ᴿ²⁰ (`artifacts.md`: an object store is gated on the endpoint it dials, with the per-scheme table; it said the store URL) |
| Agent-authored functions (D19 fuel by publisher · D20 proposed → shadow → accept) — added 2026-09-30 (plan F3) | ✓ `design-time-tooling.md` §17 | ✓ guide 16 § Agent-authored functions (U1–U4, the five gates) | ✓ guide 16 § the five gates + the co-op `provisioning` demo's wave 3 (opened: the description's `proposed = true`, `mycelium-artifact accept`, `Provisioner::invocations()` are the shipped names) | ✓ `operations/artifacts.md` § Trust & provenance (the `[hosts]` keys by name) | ✓ same section: `accept`, `verify --reviewer`, the counter (opened; each command exists in the `mycelium-artifact` bin) |
| Federation / AgentFacts (public discovery) | ✓ | ✓ | ✓ ᵀ¹ | ✓ | ✓ ᴿ¹⁷ |
| Reasoning / LLM / MCP / guardrails | ✓ | ✓ | ✓ ᵀ² ᴿ²⁰ (checkpointer README: a bounded `IncompleteCheckpoint` retry; guide 15 rung 3) ᴿ²¹ (calibration: guide 05's prompt-skill Rust and Python and its skill-calling snippets never worked; guide 15's five reasons, the `e.retriable` guard, the upgrade order) | ✓ ᴿ²¹ (`companions.md`: 0.7.0) | ✓ ᴿ¹⁶ ᴿ²⁰ (`companions.md` § mycelium-reason: a persistent `IncompleteCheckpoint`, the resolve and blob probes) ᴿ²¹ (the probe takes a full id from `e.missing` — the message's 12-character prefix is 400 `bad_id`) |
| Companions | ✓ | ✓ | ✓ ᴿ¹⁸ (guide 21 § authority at execution in the wiki store) | ✓ | ✓ ᵀ² |
| Rolling upgrade (wire compat) | ✓ | ✓ | ✓ | ✓ | ✓ ᴿ² ᴿ¹⁹ (`deprecations.md`: 2.19.0–2.21.0's literal breaks and two `#[non_exhaustive]` enums gaining variants) ᴿ²⁰ (`deployment.md` § Rolling upgrades: the start refusals by release; `deprecations.md` §15–17; `installation.md` the toolchain floor) |
| Contracts & receipts (item 1) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Effects companion (`mycelium-effects`) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁸ (guide 18 compiles: `generate(&NodeId)`) | ✓ ᴿ¹⁷ | ✓ ᶻ⁶ (`Counting<D>` + `RefusalCounts`, by kind and leg; `mycelium_effects_refusals_total` under `metrics` — zero-gaps Z6, 2026-10-03) |
| Replay & simulation (item 6) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ ᴿ¹⁹ (guide 19: the recording snippet attaches the trace; the stem command takes a unit *file*) | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁸ ᴿ¹⁹ (`diagnostics.md`: the bundle layout carries the three trace files) |
| Commitments (`mycelium-commitment`) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁸ ᴿ²¹ (`companions.md`: `compact_log` in process — the HTTP door refuses `cn/` **403** `protected_stream` since #549) |
| Gateway / SDK receipt parity — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᶻ⁷ (both SDKs' `set()` return `KvReceipt` — zero-gaps Z7, 2026-10-03) ᴿ²⁰ (mycelium-ts 0.2.1 (#539): `scatterGather` refused 400 on every call since 2026-05-25; `rpcServe` kind; 504 → `TimeoutError`; guide 10's A2A/prompt-skill snippets in both languages) | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Gateway caller identity (item 7) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Action evaluator + evidence (AE) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Scoped mandates + authority at execution (item 5, Boundary H) — new, run 17 | ✓ ᴿ¹⁷ ᴿ²⁰ (`design/scoped-mandates.md` § strict eligibility (D4)) ᴿ²¹ (its rule sufficiency matches the code; 2.26.0, not unreleased) | ✓ ᴿ¹⁷ ᴿ²¹ (guide 21: the revoked-key rule as the source applies it; forks; `from_reader`/`from_parts`) | ~ ᴿ²¹ (`examples/strict_eligibility` runs; no example or snippet builds an `AppointmentStream` for the shipped source — nothing in the repository calls `history_from_appointment_stream` outside its tests) | ✓ ᴿ¹⁷ ᴿ²¹ (`audit.md` §8 § strict eligibility — was ✗) | ✓ ᴿ¹⁷ ᴿ²¹ (`audit.md` §8: what `eligibility unknown` asks the operator to check — was ✗) |
| Member removal + identity authentication (C5) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Confined fleet (H7) — new, run 17 | ✓ ᴿ¹⁷ | — | — | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁹ (calibration: ✓ᴿ¹⁷ asserted every outbound path failed closed while the federation client and OIDC fetch were ungated until v2.20.0; the row says so now) ᴿ²⁰ (`crown-jewel.md` § 2: the TOML form, redirects, region and virtual-hosted S3, three failure rows) |
| Adaptive stability / control (item 4) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ ᴿ²⁰ (guide 22 § *Setting a node's timing yourself*) | ✓ ᴿ¹⁷ ᴿ²¹ (`tuning.md`: the audit conditions — `compliance` **and** `[tls]`, never `profile`; a `profile` row) | ✓ ᴿ¹⁸ (`control-profiles.md` step 2 + `tuning.md`: the two bound fields and env vars, applied at start) ᴿ²⁰ (`tuning.md`: five hot params, the timing setters' bounds and `Result`, `POST /gateway/govern/timing`; `dynamic-scaling.md`'s curl answered 400 as written) ᴿ²¹ (calibration: its three governance `curl`s answered 415 — no `Content-Type`; `target` on `profile` is ignored, now said) |
| Knowledge layer (item 3 + Boundary H) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| Federated domains — the transport (item 2) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ ᴿ¹⁹ (guide 17: `ClientError::Egress`) | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ ᴿ¹⁹ (`federation.md`: the client under `egress.allow_hosts`; `sso.md`: a denied issuer refuses `start()`) |
| Election / leadership (`mycelium::election`) — new, run 17 | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ | ✓ ᴿ¹⁷ |
| What is proven (`operations/what-is-proven.md`) — new, 2026-09-28 | ✓ | ✓ | ~ ᴿ²¹ (the contributor's how-to for the verification policy lives in the wiki; `CONTRIBUTING.md` § Testing is stale — outside this run's scope, reported) | ✓ | ✓ ᴿ²¹ (two `ci.yml` line references named by step) |
| Capability lifecycle — the two arrival paths, the unit file, `wire-check` (`operations/capability-lifecycle.md`, guide 02, `src/capability_config.rs`) — new, 2026-09-28 | ✓ | ✓ ᴿ¹⁸ (`reference/unit-file.md`; the module-doc example now a test) | ✓ | ✓ ᴿ¹⁸ (all 15 findings, six flags, exit codes) | ✓ ᴿ¹⁸ (snippets are TOML; `verify` runs) |
| Stem node and fleet (`mycelium-stem`, R1/R2) — new, run 18 | ✓ plan §13 | ✓ ᴿ¹⁸ (wasm-host README; `reference/unit-file.md` § who reads what) | ✓ ᴿ¹⁸ (guide 13 §5; `examples/units/`; `make examples-both-ways`) | ✓ ᴿ¹⁸ (`capability-lifecycle.md` § Running a stem: every flag and default) | ✓ ᶻ¹ ᶻ³ (a stem reads an object store by URL and pulls past the frame cap in ranges, both staged to disk — zero-gaps Z1/Z3, 2026-10-03) |
| Declared activation and serving (`[[activation]]` D21, `[[serve]]` D22) — new, run 18 | ✓ plan D21/D22 | ✓ ᴿ¹⁸ (`reference/unit-file.md`, placeholders, defaults) | ✓ (`examples/units/model_deploy`, `reheal_deploy`, `llm_agent`) | ✓ ᴿ¹⁸ (not a sandbox — `shared-responsibility-matrix.md` CC8) | ✓ ᶻ² (`api_key_env`, read once at start, unset refuses the start by name — zero-gaps Z2, 2026-10-03) |
| Artifact delivery — the tool, the publish door, ranged fetch, object stores (A1–A3, S1–S2) — new, run 18 | ✓ plan §10–§11 | ✓ (`artifacts.md` §2, § Remote blob stores) | ✓ | ✓ ᴿ¹⁸ (line-hex manifest; the route needs `http_port` + `artifact:publish`; the stem image has no `object_store`) | ✓ ᴿ¹⁸ (store build line with `stem,object_store`; `MYCELIUM_EGRESS_ALLOW_HOSTS`) |
| The declaration schema (`reference/declaration.schema.json`, W6) — new, run 18 | ✓ plan §9 | ✓ ᴿ¹⁸ (`docs/README.md` reference row) | ~ ᴿ¹⁸ (linked with its `$id`; no consumer walkthrough — validating against it, the join key) | ✓ ᴿ¹⁸ (`capability-lifecycle.md` §2) | ~ ᴿ¹⁸ (same) |
| SDK unit files (`POST /gateway/units/declare`, `declare_from` / `declareUnits`, Q2) — new, run 18 | ✓ plan §14 Q2 | ✓ ᴿ¹⁸ (both SDK READMEs; docstrings name `[[serve]]`) | ✓ ᴿ¹⁸ (guide 10 § Declaring a unit file — body, response, 400/422, `not_enforced`, lease) | ✓ (`rbac.md` `cap:write`) | ✓ ᴿ¹⁸ (`capability-lifecycle.md` §3) |
| Guarantee report (`guarantee_report()`, `GET /gateway/guarantees`, five resolutions, `register_guarantee`) — new, run 19 | ✓ plan §1, §3 G4/G10/G11/G13 | ✓ ᴿ¹⁹ (`00-concepts.md` glossary; `reference/guarantee-catalogue.md`; the rustdoc) | ✓ guide 20 § the guarantee report (verified: `GuaranteeDescriptor::new`'s ten arguments) | ✓ `production-readiness.md` § 2 (the five states) · `diagnostics.md` | ✓ `GET /gateway/guarantees` under `fleet:read` (`rbac.md` ᴿ¹⁹); `operations/README.md` routes to it ᴿ¹⁹ |
| Profiles (`dev` · `secure-single-domain` rev 2, `profile` / `GOSSIP_PROFILE`) — new, run 19 | ✓ plan G3/G5/G12, §8 | ✓ ᴿ¹⁹ (glossary; `config.rs` names the public path) | ✓ ᴿ¹⁹ (guide 20: `cfg.profile`, `SECURE_SINGLE_DOMAIN_PROFILE`, `check_profile`; the refusal was run: 14 unmet on a default build) | ✓ ᴿ¹⁹ (`production-readiness.md` § 2: seventeen ids, the two settings — was rev 1's fifteen under a rev-2 heading) | ✓ `tuning.md` + `production-readiness.md`: `profile = …` loaded, `GOSSIP_PROFILE` applied (`config.rs:1818`) |
| A node certificate issued off-node (`mycelium tls issue`, `issue_node_cert`, `id.ca_key_off_node`) — new, run 19 | ✓ `design/member-removal.md` (a node holding the CA key can re-mint a removed member) | ✓ `config.rs` `cert_pem` doc; guide 09's snippet comment ᴿ¹⁹ | ✓ ᴿ¹⁹ (guide 20: `issue_node_cert` + the `TlsConfig` the node starts from — was Missing) | ✓ `cert-rotation.md` § off-node (rev 2 requires it ᴿ¹⁹) | ✓ `cert-rotation.md`: the command matches `main.rs` flag for flag, the output names match `tls.rs`; the `--features tls` build note ᴿ¹⁹ |
| Fail-closed start refusals (token tables / `[oidc]` / `[tls]` / `[gateway_tls]` without their feature; unreadable state; the gateway bind; a denied OIDC issuer; an audit sink without `[tls]`; a late cipher) — new, run 19 | ✓ plan §8, CHANGELOG 2.18.1 / 2.20.0 / 2.21.0 | ✓ ᴿ¹⁹ (`EgressPolicy` rustdoc names every gated path; `on_unreadable` doc) ᴿ²⁰ (`error-handling.md` § Start refusals: every `InvalidField` `start()` returns, by field) | ~ (guide 09 routes to `rbac.md` for the build refusals; `crown-jewel.md` for the cipher — no single Dev chapter lists all eight, and the operator pages are the landing) | ✓ ᴿ¹⁹ (`production-readiness.md`, `deployment.md`, `gateway-tls.md`, `audit.md`, `sso.md`, `crown-jewel.md` — each refusal named, with its why) | ✓ ᴿ¹⁹ (each page says what the error names and what to change; `on_unreadable` and `[egress]` were loaded through the real loader) |
| Rule catalogue (`mycelium_core::rule`, `RULES`, `reference/rule-catalogue.{md,json}`, `mycelium rules`) — new, run 19 | ✓ plan §3 G1/G2/G8, §6 (a catalogue entry is a claim about code, checked by a test) | ✓ `rule.rs` module doc · `docs/README.md` reference row · glossary ᴿ¹⁹ | ✓ ᴿ¹⁹ (regeneration works literally and the gate was run; guide 19 § adding a decision point — the `kv.expiry` template, what `check` refuses) | ✓ ᴿ¹⁹ (`diagnostics.md` § reading what a node decided — was Thin: only `capability-lifecycle.md`'s aside) | ✓ ᴿ¹⁹ (`mycelium rules [--format md\|json]` on `diagnostics.md`; run: 11 rules, the cross-crate note) |
| Decision trace (`mycelium_core::decision`, `with_decision_trace`, `--trace-dir`, `decisions.jsonl` + `coverage.json`, `mycelium explain`) — new, run 19 | ✓ plan G7/G8, §6 (diagnostics, not the audit trail) | ✓ `decision.rs` module doc; guide 19 § reading what a node decided; glossary ᴿ¹⁹ | ✓ ᴿ¹⁹ (the sink snippet — `DecisionSink::new(SinkConfig {..})`, `with_decision_trace`, `StemOptions.trace`, the bundle attachment — was Missing; every identifier verified) | ✓ `capability-lifecycle.md` § `--trace-dir` (stem) · `diagnostics.md` ᴿ¹⁹ (node binary) | ✓ `capability-lifecycle.md` (works literally) · `diagnostics.md` ᴿ¹⁹ (`mycelium explain … --catalogue …`, matches `main.rs`); the seven "changes no decision / bounded / never blocks" tests run green |
| What the trace does not show (the whole-agent replay diverging at the kernel's 20th choice; draw-count equivalence) — new, run 19 | ✓ plan I5/I6 rows | ✓ `tests/decision_trace_replay.rs` module doc · guide 19 ᴿ¹⁹ (cross-link) · the inventory row ᴿ¹⁹ | ✓ ᶻ⁵ (the replay half is a checked-in assertion — `a_recorded_node_replays_decision_for_decision`, in CI; the whole-agent replay is proven, zero-gaps Z5) | ✓ `what-is-proven.md` § not yet shown (what would show it) | — (an operator cannot act beyond not expecting a decision-for-decision replay, which the page says) |
| Configuration ownership (`docs/reference/configuration.md`, A2) — new, run 20 | ✓ ᴿ²⁰ `configuration.md` § intro (what a type definition cannot answer); D3 in the plan | ✓ ᴿ²⁰ (rows 71/73 corrected for R8; the defect count; linked from `building-on`, the architecture folder note) | ✓ ᴿ²⁰ (`building-on` names it beside the feature list) | ✓ ᴿ²⁰ (`operations/README.md` funnel row; `tuning.md`'s head — whose "an env var exists for every field" was false, eleven have none) | ✓ ᴿ²⁰ (the refusal and restart columns; spot-checked twelve rows against `config.rs`) |
| A2A interop — the agent card and `tasks/send` (#550) — new, run 21 | ✓ guide 08 § Concept; `00-concepts` | ✓ ᴿ²¹ (guide 08 § How it works; `00-concepts`, cookbook, `skillrunner.html`: the card lists what `/a2a` can call — not plumbing, not prompt skills) | ✓ ᴿ²¹ (guide 08's snippets were not JSON-RPC (`-32601`) and read `input_schema`; now `A2aClient` + the wire shape; guide 10's card is a dict) | ✓ `rbac.md` (public surface), `production-readiness.md` | ✓ ᴿ²¹ (the negative probe names a skill on the card; `confined-fleet.md`: prompt skills under `llm:invoke`) |
| Presence ceiling and the ranked shed (`max_providers`, `prov-shed`, `election::rank`, `prov.shed` rev 2, #547) — new, run 21 | ✓ ᴿ²¹ (`capability-lifecycle.md` § Presence ceilings: why the draw was replaced) | ✓ `reference/unit-file.md` § `[[presence]]`; `rule-catalogue.md` rev 2 | ~ ᴿ²¹ (the field row explains it; no example shows a ceiling, and `election::rank` is in no guide) | ✓ ᴿ²¹ (`capability-lifecycle.md` § Presence ceilings — was ✗) | ✓ ᴿ²¹ (the bound, the rolling-upgrade dip, overlapping bands, and reading a withdrawal from the trace — was ✗) |

ᴿ²¹ closed in run 21 (2026-10-07) · ᴿ²⁰ closed in run 20 (2026-10-06) · ᴿ¹⁹ closed in run 19 (2026-10-03) · ᵀ¹ closed in Tier 1 · ᵀ² Tier 2 · ᵀ³ Tier 3 · ᴿ² closed in run 2 (2026-07-11) · ᴿ⁹ run-command fix + re-verified, run 9 (2026-07-15) · ᴿ¹³ SOC 2 arc: Dev security chapter gained the compliance-controls table + two must-work-if-followed bug fixes; new erasure row got its Dev landing (run 13, 2026-07-24) · ᴿ¹⁷ run 17 (2026-09-26): twelve new rows for the v3 axis and Boundary H; the compile-breaking literals in guides 02/04/09; the readiness checklist re-aligned with the retracted backup wording; `~` cells are recorded code gaps, not doc gaps · ᴿ¹⁸ run 18 (2026-10-02): five new rows for the design-time tooling and stem fleet; four literal failures fixed (non-TOML unit-file snippets, `verify`, guide 18's `generate()`); run 17's `~` cells re-audited — three closed by v2.16.0, one never a code gap; a token-table setting found to leave the gateway open, fixed in code · ᴿ¹⁶ run 16 (2026-09-05): persistence row split out and every cell given a landing (two non-compiling Dev literals fixed, `deployment.md § Persistence modes`, the concepts pair); consensus example fixed for `persisted`; reason companion gained its operations block.

## What was found, and how it was closed

### Tier 1 — real holes (✗ / weak-pair cells)
- **Locks · HOW·Ops** was ✗ — a shipped feature with no operational recovery story. Fix: a
  `diagnostics.md` "Stuck / contended lock" runbook (`GET /consensus/lock/{name}` inspection,
  lease-expiry self-heal) + the new metric family.
- **Federation · HOW·Dev** was ✗ — the only external-interop standard with no guide chapter. Fix:
  new `guide/17-federation.md` (serve/verify the edge doc + the multi-author domain board).
- **Consensus · HOW·Ops** — diagnostics covered only the *conflict* case, not the common *no-quorum
  stall*, and consensus had no Prometheus surface. Fix: a "Consensus stalled — quorum unavailable"
  runbook + the `mycelium_consensus_*` metric family.
- **Schema · HOW·Ops** — `schema_mismatch` was a `/stats` scalar with no runbook. Fix: a "Schema
  mismatch" runbook + a mirror gauge.

New in Tier 1: the `mycelium_consensus_*` metric family — `mycelium_consensus_timeouts_total{reason}`
(event-emitted; `no_voters`/`quorum_short`/`all_opaque`/`empty_groups`) plus
`mycelium_consensus_commit_conflicts` / `mycelium_schema_mismatch` gauges mirroring the `/stats`
scalars. Deliberately **no per-lock gauge** (cardinality) — locks are consensus slots, inspected via
`GET /consensus/lock/{name}`.

### Tier 2 — Dev-guide HOW trapped elsewhere
- Consensus **leased commits** + the **converged-holder discipline** → added to `04-consensus.md`
  (were `src/lib.rs`/wiki only).
- **MCP external bridge** (`connect_mcp_server`) → new section in `06-tool-discovery.md` so the
  chapter matches its title; bridged tools land in the same `tools/` namespace.
- **Artifacts** Dev routing → `cookbook.md` recipe now points at the Solution/Dev + DevOps anchors.
- **Companions ops runbook** → new `operations/companions.md` (durability/WAL, capability-ring
  failover, the wiki's node-independent store, teardown; **none emit Prometheus metrics**).
- **RPC** discoverability → cookbook recipe links the service-layer reference.

### Tier 3 — WHY / discoverability polish
- **Legible Emergence** got a WHY landing in `philosophy.md` (under Emergent Levels / Anderson).
- **`explain` gateway-only** made intentional in `diagnostics.md` + `src/lib.rs` (it is a cross-node
  `sys.explain` RPC fan-out, not a local read — no in-process accessor by design).
- **System→Cluster** ops note in `observability.md` (the gateway still accepts `"system"`).
- Scope-unification WHY sentence (`13-cluster-topology.md`); three-kinds-vs-API cross-link
  (`00-concepts.md`).

## Bugs the audit surfaced (correctness, not coverage)

- **2026-09-05 (run 16) — the language SDKs cannot authenticate to the gateway.** `mycelium-py`'s
  `Agent(host, port, timeout)` and `mycelium-ts`'s client send no `Authorization` header and expose no
  token option (grep of both `src/` trees for `Bearer`/`Authorization`: zero hits), while every
  operations page tells operators to set `gateway_auth_token` for any non-loopback exposure and the
  companion/node-level routes are now all behind it. A token-protected node is therefore unreachable
  from both SDKs; the OpenAI façade path works only because OpenAI clients send their API key as a
  bearer. **Code gap, not a doc gap** — the docs never claimed SDK token support. Fix shape: a
  `token=`/`authToken` constructor option that sets `Authorization: Bearer …` on every request (the
  pooled client already centralises headers), mirrored in the SDK READMEs and `rbac.md`. Related: both
  SDKs read only `ok` from propose / `consistent_set` responses and drop the new `persisted` field.
  **Fixed 2026-09-05, same day** (mycelium-py 0.2.4 / mycelium-ts 0.1.1: `token=` / `{ token }` on
  every handle, `MYCELIUM_GATEWAY_TOKEN` fallback, bearer on pooled + SSE clients; node-free gates in
  both SDKs, `jest` added to CI). The `persisted` drop was closed the same day too (`CommitResult` on both SDKs).

The digging turned up defects that reading-for-coverage exposed — the recurring lesson that
verifying-against-code finds real problems:

1. **Fencing-token doc drift** — `lock_service.rs:46` + `04-consensus.md:366` called `LockGuard::token`
   a "consensus ballot", contradicting the guide, the module's own test, and the #164 fix (the token
   is the commit HLC; the ballot regresses under gossip lag). Fixed.
2. **`GossipError` enum was fabricated** — `error-handling.md` documented `Network(String)` /
   `Config(String)` (which do not exist) and omitted five real variants incl. `FrameTooLarge`.
   Confirmed `mycelium::GossipError` is the re-exported mycelium-core enum; rewrote to the real 10.
3. **Broken/inaccurate anchors** — `#gossipError`→`#gossiperror`; the diagnostics verb table's
   "all also available programmatically" was false for `explain`.

### Run 20 (2026-10-06) — code gaps the audit surfaced

*Status, 2026-10-07:* 1, 4 and 5 fixed in #544 (with the raw-KV door to `sys/govern/` the fix's review found); 2 in #542; 3 in #543; 6 in #541. Each item below is as it was found.

1. **`POST /gateway/govern/timing` publishes any value.** The setters refuse past `validate()`'s bounds since
   2.25.0 and every node's reconciler ignores an intent outside 1–3600 / 1–300, but the route accepts and
   publishes it, answering `{"published": true}` (`src/agent/http.rs` `gw_govern_timing`). The R9 rule at the
   HTTP door. Documented as it behaves (`tuning.md`); fix: 400 naming the field.
2. **`IncompleteCheckpoint` cannot say why.** The blob tier folds *no provider*, *a provider error* and *every
   provider failed verification* into one 404, so a lost or corrupt blob raises the same retriable error as a
   slow one forever (`mycelium-reason/src/blob.rs`). Plan S5 promised corrupt content stays distinguishable.
3. **A3's strict eligibility has no shipped source.** The plan's row said the handover journal would record
   the scope, start and endpoint a source needs; nothing builds a `TermHistory` except the example, and
   nothing calls `eligible_strict` or `ready` in shipped code. The guide now says an embedding application is
   the source; the plan row overstated what merged.
4. **Cooling-off checks the head before `Ineligible`**, unlike the other two rules — a candidate visibly inside
   the window behind a stale head reads `Unknown`. Fails closed; documented.
5. **`OpenAiBackend::with_egress` takes `&EgressPolicy`**; the other three take it by value.
6. **Five node-free Python test files run in no CI job** (`test_kv_receipt.py`, `test_gateway_token.py`,
   `test_artifacts.py`, `test_units.py`, `test_action_refusal.py`); and two SDK defects fixed in
   `mycelium-ts` 0.2.1 (#539) — `scatterGather`'s field name and `rpcServe`'s kind.

### Run 21 (2026-10-07) — code gaps and findings outside this run's scope

The run-20 list above is closed in code — each item verified at its site (`TimingIntent::check` and
`gw_govern_timing`; `BlobMiss` and the 404/502/503 mapping; `mandate::history_source`; `eligibility.rs`'s
cooling-off order; the four `with_egress` signatures; the Python tests run by directory).

**Code gaps (documented as they behave, not papered):**
1. **`POST /gateway/govern/profile` neither parses strictly nor audits** — it never calls
   `refuse_unknown_fields` or `audit_govern`, so a `target` is silently ignored and the step leaves no record
   (`src/agent/http.rs` `gw_govern_profile`). Its siblings do both. `tuning.md` says so.
2. **"Audited" needs `[tls]`.** `audit_govern` discards `seal_and_write`'s error, which a node without the TLS
   identity always returns — a `compliance` build without `[tls]` records no governance action and says nothing.
3. **No typed SDK error for `protected_key` / `protected_stream`.** Both SDKs type `protected_kind` only; Python
   raises `httpx.HTTPStatusError`, TypeScript a plain `Error`, and TypeScript's `delete()` drops the body, losing
   the route-naming `message` (`mycelium-ts/src/agent.ts` `delete`).
4. **The shipped strict-eligibility source has no caller and no example** — `history_from_appointment_stream`
   is reached only by its own tests; both examples build `TermHistory` by hand.
5. **No metric for the ranked shed** — a withdrawal is visible only in the decision trace.
6. **`rule-catalogue.md`'s `prov.shed` input sources render broken code spans** (sources containing backticks,
   generated from `mycelium-wasm-host/src/rules.rs`); and the `installable/` refusal names
   `POST /gateway/artifacts/publish`, present only where `mycelium-wasm-host`'s routes are merged.

**Outside this run's file scope** (reported to their owners, not edited):
- `mycelium-py/README.md:79`, `mycelium-ts/README.md:81` — "`llm_*` / `llm*`" calls that carry caller context do
  not exist; the call is `PromptSkillClient.call`. `mycelium-py/README.md` § Health (~l.287): add
  `agent.node_id` (0.2.6).
- `langgraph-checkpoint-mycelium/README.md:78` still calls `IncompleteCheckpoint` "retriable" unconditionally;
  the upgrade order (checkpointer 0.3.0 before `mycelium-reason` 0.7.0) is on no Dev surface but guide 15 now.
- `examples/a2a_langchain/README.md:80` — "lists every capability currently advertised"; should say what `/a2a`
  can call (plumbing and prompt skills excluded, prompt skills at `POST /gateway/llm/call`).
- `examples/fluid_pipeline/flow_networks.html:789,798–808` — the same broken prompt-skill Rust and Python as
  guide 05 had (`agent.llm()`, `PromptSkillClient("localhost", 8300)`, async `call(…, timeout_ms=…)` → dict).
- `CONTRIBUTING.md` § Testing (l.69–85) — a memorised test count, no `make check`, no inventory check, no
  exceptions file or `*_LIVE_REQUIRED`; l.103 and l.126 cite `CLAUDE.md` sections that no longer exist
  (point at `docs/wiki/dev/testing/testing.md` and `docs/wiki/dev/concurrency/lock-free-and-atomics.md`).
- `docs/wiki/dev/security.md:190` — "(unreleased on `main`, #543)" → "(2.26.0, #543)".
- At the 2.27.0 release: `what-is-proven.md`'s universe sentence (jest by test name, every crate's featureless
  build — #551) and CHANGELOG's "audited in a `compliance` build" (add "with `[tls]`").

## Artifacts created

- `docs/guide/17-federation.md` (new chapter)
- `docs/operations/companions.md` (new runbook)
- `mycelium_consensus_*` + `mycelium_schema_mismatch` metric family (`metrics.md` §Consensus/locks)
- Three new `diagnostics.md` pathologies + a `mycelium-consensus` Prometheus rule group
- **Run 2:** `operations/deployment.md § Rolling upgrades` (new operator procedure)
- **Run 3:** `docs/design/coordination-approaches.md` (new WHY decision guide — when to use the
  distributed lock vs the capability ring, and why the three companions reject it; cross-linked from
  `04-consensus.md`, `companions.md`, `exactly-once-effect.md`, `wiki-concurrent-edit.md`, the guide
  FAQ, and `docs/README.md`)

## Calibration

Prior `Clear` cells later found Thin/Missing — the ledger that scores this audit's own verdicts (the
doc analogue of `ratings.md`'s calibration ledger). A cell with repeated hits deserves structural
skepticism, not a re-asserted ✓.

- **2026-09-26 (run 17) — Security · HOW·Dev** read `Clear` in runs 13–16 while `09-security.md`'s
  first `TlsConfig` literal used `key_path`/`cert_path`, fields that do not exist (`cert_pem`/`key_pem`)
  — did not compile. **Third hit of the config-literal class** (2026-07-11 wire constant; 2026-09-05
  `PersistenceConfig`). The run-16 sweep was reproduced by name over guides 01/13 and the operations
  pages; it never swept chapter 09. Structural fix: the sweep is over **every** `docs/guide/*.md`, and
  the auditor prompt carries it verbatim.
- **2026-09-26 (run 17) — Layer III · HOW·Dev** read `Clear` in runs 1–16 while `04-consensus.md`'s
  reference blocks called ten methods on `agent.` that live on `agent.consensus()`, called
  `start_consensus_listener()` without its argument, and matched a `#[non_exhaustive]` enum without
  a `_` arm. The blocks were "moved from README" and never re-verified as code after the sub-handle
  split. Found by opening the block, not by the literal sweep (which checks struct fields, not
  receivers) — the sweep now also checks that every `agent.method(` a chapter shows exists on
  `GossipAgent` or names its handle.
- **2026-09-26 (run 17) — Capabilities · HOW·Dev** read `Clear` in runs 1–16 while
  `02-capabilities.md`'s `CapabilityGroupDef` literal omitted `topology_policy` with no `..`. Same
  class as the first entry; found by the widened sweep.
- **2026-09-26 (run 17) — Persistence · HOW·Ops** read `Clear` from run 16 while
  `production-readiness.md` §3's backup bullet prescribed the live-copy procedure `deployment.md`
  retracted the same morning. Same-day drift inside one funnel: the runbook moved and the checklist
  that links it did not. Sharpening: a change to a runbook procedure is a change to every checklist
  line that restates it — grep the readiness page for the procedure's key phrase before closing.
- **2026-09-26 (run 17) — Security · HOW·Ops (A2A) and identity** — text written **the same day**
  by the materials review was already wrong when audited: the negative probe named `message/send`
  (the handler knows `tasks/send`), so followed literally it passed vacuously on an open node; and
  the identity example command omitted `--features tls,compliance`. The lesson is the audit's own:
  a must-work instruction is verified against code *when written*, not at the next run. Both fixed.
- **2026-09-05 (run 16) — Security · WHAT·Ops + HOW·Ops** read `Clear` in runs 1–15 while `rbac.md`
  (and the wiki security page) stated the public surface as exactly `/health|/ready|/stats|/metrics`
  + descriptor — but `POST /mcp` (tool invocation **with the node's identity**), `GET /signals/{kind}`
  and `GET /consensus/{slot}` answered without a bearer, and until 2026-09-04 so did every companion
  `/gateway/…` route. Found by an **external code review** (finding 4) and the 09-04 façade work, not by
  this audit. **The 5th "asserted guarantee false in code" hit, and the first where the claim is a
  *boundary enumeration* ("the public set is exactly X")** rather than a property of one mechanism.
  **Sharpening:** a `Clear` on any doc that *enumerates a security boundary* (public routes, allowlist,
  gated set, reserved prefixes) must be diffed **mechanically** against the code table that defines it
  (here the router + `required_scope`) — the wiki-lint's per-occurrence diff recipe applies; reading
  the sentence is not verification. Fixed 2026-09-05 (#172): routes gated, every statement of the set
  aligned (rbac.md, 09-security, config.rs doc, observability, wiki).
- **2026-09-05 (run 16) — Layer I persistence · HOW·Ops** read `Clear` in runs 1–15 while
  `production-readiness.md` §3 asserted "consensus committed slots are always fsynced regardless" of
  `sync_mode` — **false in code**: `append_sync` only synced in `Flush` mode, and a stopped WAL writer
  acknowledged every append as `Ok`. Found by the same external review (finding 3). **6th false-guarantee
  hit** ("fsynced"). Fixed in code (v2.4.2 forces the sync in every mode; acks are `BrokenPipe` on a
  dead writer) so the sentence is now true; the page also points at `persisted`. Reinforces the
  2026-07-15 sharpening — a durability word ("fsynced", "durable", "survives") is an asserted guarantee
  and must trace to the syscall.
- **2026-09-05 (run 16) — Layer I persistence · HOW·Dev** read `Clear` in runs 1–15 while **both**
  Dev-guide `PersistenceConfig { … }` literals failed to compile (`01-gossip-kv.md` `data_dir`,
  `13-cluster-topology.md` `path`; the field is `base_path`, and two required fields were absent).
  Found by this run's spot-check. **Second hit of the config-literal class** (`TlsConfig { key_path }`,
  2026-07-24) — and the 07-24 sharpening ("diff every `Config { … }` literal against the struct") was
  applied to the security chapter only. **Structural fix, not another point patch:** run 16 wrote and ran
  a mechanical sweep — every `*Config`/`*Token`/`*Policy { field: … }` literal in `docs/guide/*.md` +
  `operations/{deployment,rbac,companions}.md`, fields diffed against `pub struct` fields across all
  crates — 0 unknown fields after the fix. Re-run it every pass (it is in the run-16 transcript; ~30
  lines of Python) instead of spot-checking.
- **2026-07-24 — Security · HOW·Dev** was `Clear` in runs 1–12 while `09-security.md`'s Dev Notes held
  two `must-work-if-followed` defects: a `TlsConfig { key_path: … }` example that **does not compile**
  (the field is `key_pem`) and a false "`TlsConfig::default()` regenerates the key every restart" claim
  (it persists to `auto_cert_dir` and reloads — so a copy-paster's identity is actually stable, and the
  doc's stated remedy used a non-existent field). Found by **this run's presence-is-not-sufficiency
  spot-check** opening the Dev chapter's code examples and checking field names against `TlsConfig`.
  Root cause: prior runs verified the chapter's *prose* and endpoint curls but never compiled its Rust
  `TlsConfig` literal against the struct. Lesson (folded into the check): the must-work-if-followed gate
  must diff every `Config { … }` literal in a Dev doc against the actual struct fields, not just spot-check
  env-vars/version-constants — a wrong field name is the same class of silent failure as a stale constant.
- **2026-07-20 — Capabilities · WHAT·Dev + HOW·Dev (bridge)** were `Clear` in runs 1–11 while the
  evaporation story was **false for gateway-bridged advertisers**: `02-capabilities.md` claimed "a
  node that stops refreshing simply evaporates" and `10-language-bridges.md` said the handle "keeps
  the advertisement alive" — but the bridge's refresh loop runs in the *node*, so a crashed
  Python/TS client left a permanently-live advert (no evaporation, ever). Found by a **live
  incident** (scraper-fleet worker w15's stale advert, 2026-07-20), which then drove both the code
  fix (the capability lease) and the doc fixes. Root cause: the audits verified the evaporation
  claim against the in-process path only — the claim was *true for the persona the chapter had in
  mind* and silently wrong for the bridge persona one chapter over. Lesson: a liveness/consistency
  claim must be checked against **every advertise path** (in-process, gateway, SDK), not the default
  one.
- **2026-07-11 — Security · WHAT/HOW·Dev** was `Clear` in run 1 (seed) while `09-security.md` cited
  the wire version as **v10 "(current)"** and framed the rolling-upgrade window as **v10 ↔ v9** —
  both stale (current is v12/v11). Found by run 2's rolling-upgrade diff-audit. Root cause: the seed
  auditor confirmed the *concept* was explained but did not spot-check the *version constant* in the
  prose. Lesson for future runs: a `Clear` verdict on a doc that pins a constant/version must verify
  the value against code, not just its presence.
- **2026-07-13 — Distributed locks · WHY + Companions · WHY (cross-cutting)** were `Clear` in runs 1–2,
  but the decision *"which coordination primitive do I reach for, and why not the lock?"* had **no
  user-facing landing** — each primitive's own rationale existed (`04-consensus.md` for the lock,
  `exactly-once-effect.md` for the companions), yet the *comparison* lived nowhere: `companions.md`
  only **asserted** "no distributed lock" without the why. Found by a user question ("is the
  lock-vs-ring design decision documented anywhere?") + a doc audit that confirmed the absence. Root
  cause is **structural, not an oversight**: the matrix scores each concept's cells independently, so a
  cross-cutting decision guide that spans several concepts (here the CP-vs-AP coordination axis over
  Locks/Companions/Consensus) falls *between* rows and reads as covered when every individual cell is
  ✓. **Sharpening (a method change, not a point patch):** when two or more concepts share a decision
  axis, audit whether the *comparison itself* has a home — add a "cross-cutting decisions" pass that
  asks "if a reader must choose between these N concepts, where do they learn how?", distinct from
  scoring each concept's own WHY. Fixed: `coordination-approaches.md`.
- **2026-07-14 — Membership + cluster_name · HOW·Dev** was `Clear` in run 1 (the seed called this
  corner *"the strongest, Clear on every cell"*) while the documented way to set it via
  `GOSSIP_CLUSTER_NAME` **silently did nothing** — env vars only apply if the binary calls
  `cfg.apply_env_overrides()`, which `13-cluster-topology.md` never mentioned. Found by a user question
  ("cluster name is unset — how do I set it?"). Root cause: the auditor confirmed the instruction was
  *present*, not that it *works when followed literally*. **This is the 2nd hit of the same class**
  (Security wire-version, 2026-07-11 was the 1st): a cell marked Clear on the *presence* of an
  instruction/value whose content was actually stale or silently-failing. **Sharpening:** a `Clear`
  verdict on any doc that gives a **setting / config / run instruction** must verify the steps,
  followed literally, actually *succeed* (or trace to code that makes them succeed) — presence is not
  sufficiency; a silently-no-op instruction is **Thin**, not Clear. Folded into the skill's adversarial
  rule. Fixed: `guide/13-cluster-topology.md`.
- **2026-07-15 — Security (audit) · HOW·Dev** read `Clear` across runs 1–6: `09-security.md`
  § "The audit trail in practice" covers the trail's write / verify / query API thoroughly (incl. a
  `GET /gateway/audit` curl). But it cross-linked **no runnable demo or browser view** of the trail, so
  a dev's "how do I *watch* this fill?" had no landing. Found by a user question ("we have no examples
  for viewing audit or guardrails?"). **A lighter hit than a full Clear-found-Thin:** the API-level HOW
  *was* Clear (curl present), so this is a **runnable-landing** gap, not a mental-model gap. Root cause
  shares this session's theme — the example that would *be* that landing (`community`, the audit
  producer) wasn't cross-linked from the concept doc, and the companion examples (guardrails, reason)
  weren't indexed at all (the examples audit only swept `examples/` + coop). Fixed: a "See it live"
  pointer in `09-security.md` → the `community` mgmt UI + the new Ops Console **Audit** tab.
  **Sharpening:** a HOW·Dev `Clear` on a mechanism a reader would want to *watch* (audit, emergence,
  convergence) should confirm a **runnable / visual landing** is cross-linked, not just the API + a curl.
- **2026-07-15 — Artifacts / library · HOW·Dev** read `✓ (ᵀ²)` across runs 1–8 while the `catalog` and
  `provisioning` demo **run commands silently failed** — documented without the `--features wasm` those
  bins require (`coop/README.md` ×2, `operations/artifacts.md`, and — introduced in run 9's own diff —
  the new guide-ladder row + `presentation.html`), so a dev following any of them hit `error: target …
  requires the features: wasm`. **The 3rd hit of the "instruction present but no-ops/fails" class**
  (wire-version 2026-07-11; `GOSSIP_CLUSTER_NAME` 2026-07-14). Found by run 9's must-work-if-followed
  spot-check — and notably by *running the binary*, which is how the missing feature first surfaced.
  Root cause: prior `Clear` verdicts confirmed the *walkthrough prose + API* but never *ran the demo
  command*; the three bins that **did** carry `--features wasm`
  (`mcp_toolgrowth`/`model_deploy`/`reheal_deploy`) masked the two that didn't. **Reinforces the
  2026-07-14 sharpening** rather than adding a new one — the existing "verify the steps succeed" rule
  already covers this; the gap was applying it to the *run command*, not just env-vars/constants. Fixed
  all five.
- **2026-07-15 (run 11) — Security · HOW·Ops** read `✓` across runs 1–10 while `operations/cert-rotation.md`
  step 2 asserted `sys/identity/{self}` is **"signed by the old key"** — **false in code**: the entry is
  written UNSIGNED (`encode_identity_history` → raw `32×N` bytes; the publish is a bare `kv().set`, no
  Ed25519 signature). The signature was design intent that was never implemented, and the false claim
  masked the identity-poisoning gap (a compromised admitted node can LWW-inject a verifying key; code
  audit pass 3, 2026-07-15). Found by this session's code audit + this run's diff-check of the identity
  docs. **The 4th "claim present but false-in-code" hit — and the first where the false claim is a
  *security guarantee* ("signed"), not a setting/command** (wire-version 2026-07-11; `GOSSIP_CLUSTER_NAME`
  2026-07-14; artifact run-commands 2026-07-15). **Sharpening (extends the 2026-07-14 rule):** the
  value-vs-code check must also cover **asserted guarantees** — "signed" / "authenticated" / "verified" /
  "validated" — a `Clear` verdict on a doc claiming a crypto or safety property must confirm the code
  *performs* it, not merely that the property is described. Fixed: `cert-rotation.md` step 2 (states
  unsigned + links `design/identity-authentication.md`); also this session, the `rotate_identity` code
  comments + `wiki/dev/security.md`, and the stale `/ready` row in `observability.md`.
- 2026-10-02 (run 18): **Security·HOW·Ops was Clear in runs 13–17** but `GOSSIP_GATEWAY_NAMED_TOKENS`
  (and both token tables) did nothing in a non-`compliance` build, which then ran an open gateway (found by
  this run's literal-follow check of a v2.16.0 setting). Second hit of the *setting that silently no-ops*
  class after `GOSSIP_CLUSTER_NAME`. **Sharpening:** for a security setting, *works if followed literally*
  includes the build — check the setting's consumer is compiled into the build the doc tells the operator
  to run, and send the unauthenticated request.
- 2026-10-02 (run 18): **Capability lifecycle·HOW·Ops was Clear on 2026-09-28** (a row added by opening the
  pages) but its `[[activation]]`/`[[serve]]` snippets and the module-doc reference block were not TOML, and
  §4's `verify` exited 2 (found by pasting the snippets into the loader). **Sharpening:** a config snippet's
  Clear needs it *loaded*, not read — and the canonical one is now a test.
- 2026-10-02 (run 18): **Commitments·HOW·Ops was recorded `~` as a code gap in run 17** when the verb existed
  (`compact_log`, and a gateway route). A miscalibration the other way: a doc gap filed as a code gap is a
  gap nobody fixes. **Sharpening:** a "code gap" verdict cites the grep that found nothing.
- 2026-10-03 (run 19): **Confined fleet·HOW·Ops was Clear in run 17** (`confined-fleet.md`: "the substrate's own
  outbound paths fail closed" under `egress.allow_hosts`) while the federation client dialled any partner and
  the gateway fetched OIDC discovery and JWKS from any host — gated only in v2.20.0 (CHANGELOG § [2.20.0];
  `lifecycle.rs` and `federation/client.rs`). Found by diffing the row against the 2.20.0 entry. Same class as
  the 2026-07-15 "signed" hit: *asserted guarantee not confirmed in code* — the run-17 auditor opened the page
  and the confinement report, not the two clients' dial paths. **Sharpening:** a confinement claim's Clear
  enumerates the outbound call sites (`grep -rn "reqwest::\|Client::new\|connect(" src/ mycelium-*/src/`)
  and names the gate on each.
- 2026-10-03 (run 19), not a miss but recorded: **KV persistence·HOW·Dev (✓ᴿ¹⁶)** — the two guide literals
  compiled when scored and broke with v2.20.0's `on_unreadable` (no `Default` on the struct); the release fixed
  the in-tree examples and not the guides. A regression of a swept class, caught one run later. The structural
  answer stays the one run 16 named: guide snippets that are exhaustive literals should be doctests or
  examples CI compiles.

- **2026-10-06 (run 20) — Layer II · HOW·Dev** read `Clear` in every run while both SDK READMEs' quick starts
  and guide 10 emitted a signal **before** subscribing, so a literal run waited forever (the subscription
  registers when the stream opens). The fourth class of *instruction present, does not work as written*, after
  the wire constant, `GOSSIP_CLUSTER_NAME` and the config literals. Found by an auditor reading the snippet
  against the handler's registration. Structural answer: an SDK quick start is a live test — the TypeScript
  suite now runs `onSignal` after subscribing (#539).
- **2026-10-06 (run 20) — Adaptive stability · HOW·Ops** (✓ᴿ¹⁸) while `dynamic-scaling.md`'s live-tuning `curl`
  posted `{"writer_channel_depth":4096}`, which the route refuses (`intent must set 'enabled' or 'params'`;
  the key is `writer_depth`), and `tuning.md` said three params were hot-reloadable when five are. Found by
  reading the handler; confirmed against a node.
- **2026-10-06 (run 20) — Security · HOW·Ops** (✓ᴿ¹⁹) while `sso.md` said an off-list `jwks_uri` refuses the
  start: only a *configured* one does; a discovered one starts and 401s every token — Google's IdP, in the
  page's own table, is the common case. Found by reading `lifecycle.rs` against the claim.
- **2026-10-06 (run 20) — Gateway / SDK receipt parity · HOW·Dev** (✓ᶻ⁷) while TypeScript `scatterGather` had
  never worked against a node. Not a receipt cell's subject, but the cell vouched for the SDK surface; the
  verb was outside every review and test that ran (`.log/2026-10-06-ts-sdk-021-sweep.md`).

- **2026-10-07 (run 21) — Reasoning / LLM · HOW·Dev** read `Clear` in every run while guide 05's prompt-skill
  snippets could not compile or run: `register_prompt_skill` / `call_prompt_skill` on `GossipAgent` (they are on
  `agent.llm()`), a Python client with a URL constructor and a `register` that does not exist, and a skill call
  through `agent.rpc_call` that the gateway has refused **403** `protected_kind` since 2.15.0. Fifth hit of the
  *instruction present, does not work as written* class. Found by an auditor checking #550's split against the
  chapter that registers prompt skills. Structural answer, again: SDK snippets in the guides are untested text.
- **2026-10-07 (run 21) — Adaptive stability · HOW·Ops** (✓ᴿ²⁰, after run 20 corrected the *body* of the same
  `curl` and "confirmed against a node"): the three governance `curl`s in `dynamic-scaling.md` sent no
  `Content-Type`, which axum's `Json` extractor answers **415** — so the run-20 check ran a different command from
  the page's. Same class in `cert-rotation.md`, `federation.md`, `rbac.md`, `sso.md`. Structural answer: a
  must-work check runs the page's text, pasted, not a reconstruction.
- **2026-10-07 (run 21) — Federation / AgentFacts · HOW·Dev** (✓ᵀ¹; A2A had no row of its own): guide 08's Python
  `tasks/send` snippets were never JSON-RPC and read a card field that does not exist. A2A now has a row.
- **2026-10-07 (run 21) — Security · HOW·Ops** (✓ᴿ²⁰): `rbac.md` § failure modes advised granting the scope or
  `"*"` for any 403, wrong for `protected_kind` since 2.15.0 and `protected_key` since 2.26.0; `tuning.md` said
  every governance POST is audited.
- 2026-10-07 (run 21), not misses but recorded: three Clear cells broken **by the window's own PRs** — guide 04's
  consistent-set example (#549 wrote a `group` field the route ignores), guide 21's revoked-key sentence (#543's
  round 2 reversed the rule and the guide kept round 1), and `companions.md`'s `cn/` compaction over HTTP (#549
  closed the door). The verification policy's rule 2 (*plan rows close on evidence*) applies to a PR's own
  doc lines too.

## Re-run guidance

The audit was a one-time systematic sweep; a re-run should be a **diff**. Re-audit a concept only
when its code/docs changed since the last run (run 21 baseline: `a68f7e12`, #551 on `main` —
`git log a68f7e12..HEAD -- docs/ src/ mycelium-*/src/ mycelium-core/src/`; #552's `group_sizes` is the first
thing a run 22 audits). The matrix
above is the baseline: any cell dropping below ✓ is a regression. The method (four auditors, the
Clear/Thin/Missing rubric, the exact prompts) is reproducible from this session's transcript. New
concepts (a new sub-handle, a new companion, a new external standard) each need a fresh row audited
across all five cells.
