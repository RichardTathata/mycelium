# Guarantees and the rule catalogue — a supported secure profile, and decisions you can trace

**Status:** proposed, rev 0.2, 2026-10-02 (rev 0.1 the same day; rev 0.2 after a read-only review that resolved
five points — §3 G4–G6, G10–G12, §4 I2–I3 and §5 carry them). Nothing here is built. Source baseline: `main` after v2.18.0
plus #468–#471 (the fail-closed token table, the activation stderr drain, two test timing fixes, the
positioning revision).

**Inputs, both of 2026-10-02:** an independent *360 deep dive* of the public and private repositories
(its top recommendation: *a supported profile that validates gateway authentication and TLS, peer identity
anchors, proof requirements, evaluator attachment, provider enforcement, durable revocation epochs, egress
limits, persistence and evidence retention together … and refuses configurations that claim a guarantee
they cannot provide*), and a design note, *Mycelium Rule Catalogue and Decision Tracing* (an
implementation-linked catalogue of the substrate's organisation rules, a bounded decision-trace format, and
a stem-provisioning pilot). This plan carries both, because they rest on one inventory.

## 1 · Why one plan

The two pieces of work ask different questions of the same code:

| | The secure profile | The rule catalogue and decision trace |
|---|---|---|
| Question | *Can this node, built and configured as it is, provide what its deployment claims?* | *Which rule decided this, on what evidence, and what happened next?* |
| When | at startup, and on demand | at runtime, per decision |
| Effect | **changes behaviour**: a profile that cannot be met refuses to start | **must change nothing**: no extra RNG draw, no decision-clock read, no lock held while recording |
| Urgency | a defect class — F1 (a token table the build could not enforce, so the gateway ran open) was one instance, found by accident | an observability and research investment |

What they share is the **inventory**. A guarantee is enforced *somewhere* — a gateway predicate, a provider
check, a startup validation — and that somewhere is a decision point the catalogue's *authority*
responsibility names. F1 would have been one catalogue row: *gateway authorisation; enforced at
`gateway_auth`; needs feature `compliance`; config `gateway_named_tokens`; nothing checks the build at
start.* Auditing the enforcement dependencies for that row could have exposed it — the inventory supports
discovery; it does not guarantee it. So the inventory comes first, it is shared, and each track reads from
it.

## 2 · What already exists

- **`src/agent/confinement.rs` — the prototype.** `ConfinementReport` states one profile's node settings as
  `Set` / `Unset` / `NotInBuild`, says what it cannot see (`NetworkConfinement::Unverified`,
  `ClockSync::Unverified`), and lists what is `unmet()`. It reports; it refuses nothing; it covers six
  settings of one profile. The profile here generalises it rather than starting beside it.
- **`docs/operations/production-readiness.md`** — the guarantee list in prose, in eight sections (identity
  and transport · authorisation and the gateway edge · persistence and restart · sizing · observability ·
  coordination integrity · evolution and supply chain · companions). Every row that says *set X* is a
  candidate guarantee entry; the checklist becomes the profile's documentation, generated or checked.
- **`docs/design/confined-fleet.md`**, **`docs/threat-model.md` §7** (the safety-sensitive consensus
  profile: fixed voter set, fixed strict-majority quorum), **`docs/operations/shared-responsibility-matrix.md`**
  — what the operator owns, which the profile must report as *not verifiable here*, never as met.
- **Start-time refusals already in code:** `GossipConfig::validate()` (field-level), the token-table refusal
  (#468), `with_a2a()`'s warning, `with_provider_enforcement()` failing closed without an evaluator. Each is
  a guarantee check written ad hoc; the profile gathers them.
- **For the trace:** `EventRing` and `ViewConfidence` (`src/agent/emergent.rs`), the sim seams
  (`mycelium-core/src/sim_seam.rs`), the replay bundle (`mycelium-sim/src/bundle.rs`).

## 3 · Decisions

| # | Decision | Why |
|---|---|---|
| G1 | **One descriptor registry, two kinds of entry**: a *rule* (a decision point: trigger, inputs, outcomes and reasons, guards, relationships, trace policy) and a *guarantee* (a claim: what it promises, the features and settings it needs, its enforcement points, how a node checks it, what a node cannot check). Stable IDs, a semantic revision, the owning subsystem, test references. | One inventory, two readers. A guarantee's enforcement points are rule IDs, so the two cannot drift apart silently. |
| G2 | **Descriptors are metadata, registered explicitly, and execute nothing.** No universal `evaluate_and_apply`, no rule engine, no linker tricks. | The design note's discipline. A registry that schedules is a coordinator. |
| G3 | **A profile is a named set of guarantees** — at least `dev` (nothing required, loudly labelled) and `secure-single-domain` (the readiness checklist's production set). Federation's bounded profile follows. | The review's three recipes: a local demo, a supported secure deployment, a bounded federation. |
| G4 | **Two kinds of guarantee, resolved differently.** A **node-enforced** guarantee resolves at start to `Enforced` / `NotConfigured` / `NotInBuild` / `NotApplicable`. An **external prerequisite** — what a node cannot see: network confinement, clock sync, key custody beyond this host, the operator's policy — resolves only to `NotVerifiableHere`, is listed in the report as *unresolved*, and is never counted as met. | `ConfinementReport`'s three states, plus an honest fourth for role (G10) and a separate class for what the node cannot vouch for. Mixing the two is how a report comes to imply a deployment is verified. |
| G5 | **A node started under a profile refuses to start when any node-enforced guarantee the profile requires is `NotConfigured` or `NotInBuild`**, naming each by ID, the missing feature or setting, and the doc that says how to provide it. The report then says **"node requirements satisfied"** — never "deployment verified" — and lists every external prerequisite as unresolved beside it. A node started under no profile behaves as today and logs its report. | Startup validates what the node can check; the report states what it cannot. Refusing by name is the codebase's convention. |
| G6 | **Unconditional refusals stay unconditional.** A configuration that is *self-contradictory* — a token table the build cannot enforce (#468), a `llm:*` scope — refuses under every profile, `dev` included. | A setting that silently does nothing is a defect whatever the deployment intends. |
| G7 | **The trace changes no decision.** It reuses values the code already produced; no RNG stream, no shared HLC advance, no network, no authority decision; nothing serialised under a subsystem lock; bounded by bytes *and* count; off by default; never blocks work. | The design note's §*Behaviour preservation*, adopted whole. |
| G8 | **Coverage is stated, never implied.** A rule is *instrumented*, *catalogue only* or *unsupported*; a replay bundle without the decision attachment means *trace unavailable*, not *no decisions*. | The project's evidence discipline (`what-is-proven.md`). |
| G9 | **Controller-interaction experiments are research, not a deliverable here** — recorded as a §13-style question with the trace as their instrument. | Open-ended; the combined-feedback question already has a dated bound in the evidence ledger. |
| G10 | **Every guarantee carries an applicability predicate over the node's role**, evaluated from the configuration and the attached components (a gateway configured, a provider serving protected kinds, a companion present). A guarantee that does not apply resolves `NotApplicable` **with the role fact that made it so** in the report. `NotApplicable` is never a waiver: a required guarantee whose predicate *does* hold and is unmet refuses. | A node with no gateway must not fail a gateway guarantee; a node that quietly declares a guarantee irrelevant must say what made it so. |
| G11 | **Profiles reference stable guarantee IDs, and an unknown required ID fails validation.** A profile that requires `ae.evidence_journal` on a node where nothing registered that ID refuses to start — a missing registration (a companion not attached, a feature not compiled) does not make the requirement disappear. The registry rejects duplicate IDs, and an extension's registration cannot override a core guarantee's result. | The failure the reviewer named: a requirement that vanishes with its registrar. Core results stay core's. |
| G12 | **Profiles are versioned, like descriptors.** Each profile has a revision and a resolved guarantee set; both appear in the startup report and in every decision record (I4). Adding a required guarantee to `secure-single-domain` is a new revision with an upgrade note, under the release policy: a MINOR may add a requirement that an existing deployment can meet by configuration; one that needs a rebuild or a new component is announced one release ahead, as the removal ledger does. | Adding a requirement can stop an existing deployment from starting; that is a behaviour change and is released as one. |
| G13 | **The profile is validated at one lifecycle boundary, over a frozen effective configuration.** Order: construct → attach evaluators, journals, enforcement and companions → resolve the effective configuration (file, env, builders) and take its digest → validate the selected profile → admit traffic. A relevant setting or attachment that changes after that boundary is **rejected** where the API allows it (the `set`-once attachers already warn and ignore a second call; they return an error under a profile) and otherwise triggers **revalidation**, with the report regenerated and the change logged; a revalidation that fails refuses the affected operations rather than the whole node. The report carries the configuration digest it was computed over, so a stale report is detectable. | A startup report must not silently become stale. |

## 4 · Increments

Each is a separately reviewable change or small series, with a regression test seen failing first where it
fixes a defect.

| Row | Deliverable | Exit gate |
|---|---|---|
| **I1 · the inventory** ✅ 2026-10-03 (the guarantee half shipped with I2 in v2.19.0; the rule half: `mycelium_core::rule` — `RuleDescriptor`, four responsibilities, typed outcomes with `snake_case` reasons, guards, the three relation kinds as hypotheses, `TracePolicy`; the entries `mycelium::rules::RULES` (11: propagation, expiry, signal admission/forwarding/suppression, capability match, the membership governor, gateway auth, AE preflight, provider enforcement, a2a admission) and `mycelium_wasm_host::rules::RULES` (15: the pilot's health pass, promotion, demand response, presence floor, shed, provenance, loadable, eligible, rights admission, self-election, install, activation, probe, advertise, withdraw); the generated catalogue `docs/reference/rule-catalogue.{json,md}` checked by `mycelium-wasm-host/tests/rule_catalogue.rs` — structure, every relation resolving, every named test present in the tree, the checked-in document current. **Every entry is `CatalogueOnly`** until I5 attaches a sink. **Deferred:** the readiness-checklist rows map to guarantees via the I2 registry, not through this module) | The descriptor types (G1), a registry, and the first entries: **every guarantee in `production-readiness.md`** and the settings in `ConfinementReport`, each with its enforcement points; and **the rules the stem provisioning pilot touches** (unmet-demand response, eligibility, provenance, acceptance and shadow, self-election, activation and probe, withdrawal), plus catalogue-only entries for propagation, admission, membership and expiry. A generated catalogue (JSON + one doc page). **The audit is the point:** every guarantee whose enforcement depends on a feature or setting that nothing checks at start is recorded as a finding, and each finding gets a regression test and a fix in its own PR. | CI checks duplicate IDs, unknown references, missing reason definitions, missing test references. Every pilot rule maps to a real decision point and a behavioural test. Every readiness-checklist row maps to a guarantee or is marked *operator-owned* with a reason. The finding list is published, including *none found* where that is the result. |
| **I2 · the startup report** ✅ 2026-10-03 (`src/agent/guarantee.rs`; the registry — I1's guarantee half — with it; **deferred to I3:** the checked-in golden and the per-guarantee state matrix, since I3 changes what a profile requires) | `GossipAgent::guarantee_report()` generalising `confinement_report()`: every registered guarantee resolved per G4 (node-enforced to its four states, with the role fact behind any `NotApplicable`; external prerequisites as unresolved), the profile revision and resolved set (G12), the configuration digest it was computed over (G13), as a typed value, a log block at start, and a gateway route (read-only, scoped). `ConfinementReport` becomes a view over it, unchanged in shape. | A test per state per guarantee, including `NotApplicable` for each role predicate; the report for a default node matches a checked-in golden; the confined-fleet test still passes unchanged; a changed setting after the boundary is rejected or revalidated as G13 says, by test. |
| **I3 · the profile** ✅ 2026-10-03, first revision (`dev`, `secure-single-domain` rev 1; selection by `profile` / `GOSSIP_PROFILE`; refusal by name at the G13 boundary; unknown ids and names refused). **Deferred:** the per-route counter test under the profile (the C7 bypass matrix covers the doors today, not under a profile), the readiness checklist *generated* from the profile (it is checked against it by a pinned-set test), the Ops Console label, and a `federation` profile | Named, versioned profiles (G3, G12) selected by config and env, validated at the G13 boundary; refusal at start (G5) naming each unmet node-enforced guarantee; an unknown required ID refuses (G11); unconditional refusals gathered (G6). Negative tests across the CI feature matrix: for each profile × feature set × role, start succeeds exactly when every required node-enforced guarantee that applies resolves `Enforced`, and the report lists every external prerequisite unresolved. An unauthenticated request to every protected route, by the handler's own counter, under `secure-single-domain`. | The matrix test; the counter test; the readiness checklist generated from, or checked against, the profile, with its external-prerequisite rows marked as such. The `dev` profile visibly labelled in the report and the Ops Console. Ships in a MINOR with an upgrade note. |
| **I4 · the trace core** ✅ 2026-10-03 (`mycelium_core::decision`: `DecisionRecord` with every field in the schema below — the profile stamped per G12, `Completeness` flags that read as *unknown* never *none*, `ViewStatus`/`Provenance` stated by the decision point; `DecisionSink` bounded by count **and** bytes, `try_lock` only (a contended record is dropped and counted), the newest dropped at a bound so a trace is a prefix plus its drop counts; `to_jsonl()` and `DECISION_ATTACHMENT = decisions.jsonl` for a bundle, absent ⇒ *trace unavailable*; lock-order row 53; no clock, RNG or seam call in the module — the seam lint covers it. **Deferred to I5:** the *trace off vs on* equivalence test needs an instrumented decision point, and the bundle writer's attachment) | The design note's increment 2: a bounded, nonblocking `DecisionRecord` sink (a leaf module or crate with no runtime dependency), the record schema (rule ID and revision, build and config digest, node, incarnation, local sequence, target, trigger and parent references *when known*, bounded inputs with age and provenance, view status, outcome and typed reason, effect reference, completeness flags), off by default. | Trace off vs on: the same decisions and effects under identical replayed inputs, no extra RNG draws or decision-clock events. Saturation: work continues; drops and truncations visible; memory bounded by bytes. |
| **I5 · the provisioning pilot** ✅ 2026-10-03, bounded (`Provisioner::with_decision_trace`, `StemOptions::trace`, `mycelium-stem --trace-dir`): nine rules instrumented from the values the round already produced — health pass, promotion, demand response, presence floor, shed, eligible (the live verdict's reason, never a second evaluation), self-election (the one draw, recorded after it), rights admission, install (from the task, after `hosted` is released) — guard order, draw and policy unchanged; `eligible()` and `start_install_as()` now return the reason they had (`Result`, `StartOutcome`) so a record carries a typed one. **Shown:** `the_trace_changes_no_decision_and_says_what_the_round_decided` (same scenario with and without a sink: same count, same hosted state; the traced one reads eligible → elected → `unmet_demand_live` → `completed`, then `all_healthy` and `already_hosted`) and `a_saturated_sink_drops_and_counts_and_the_round_still_installs`. **Not shown, stated:** draw-count equivalence under `sim` (this crate has no `sim` build); the survivor-after-origin-failure explanation at stem level and the shadow variant (the rules are instrumented, the assertions are not written); install → activation → probe are **not** linked by an operation id (activation and probe stay catalogue-only — they decide inside a runtime hook, and the record's `parent_seq` is unused); `incarnation` is 0 (the node holds no start counter and a start stamp would be a clock read); `at_ms` is absent (the round reads no clock) | The design note's increment 3: instrument the stem lifecycle without changing guard order, randomness or policy — observed deficit, candidate checks, self-election using the existing draw, live or shadow or defer or decline, install → activation → probe linked by a local operation ID, advertisement and withdrawal only when they happen. | A survivor acquiring and serving a permitted capability after an origin fails, explained end to end; shadow-before-acceptance and failed-health variants; an activation failure never recorded as success. |
| **I6 · replay and explanation** | The design note's increment 4: an optional, versioned decision attachment and coverage manifest on replay bundles; one catalogue export and one explanation renderer on the existing CLI (names checked against the command tree first). | Deterministic comparison of canonical decision tuples where seams cover the inputs; *unattributable* stated where they do not; old bundles still read. |
| **I7 · membership and admission** | The design note's increment 5. | Settling, RNG, scope, suppression and forwarding semantics preserved, by the same off-vs-on test. |

**Order and release shape.** I1 → I2 → I3 is the security track and ships first, as one MINOR (I1's findings
may ship earlier as PATCHes). I4 → I5 → I6 can start once I1's schema is fixed, in parallel with I2–I3. I7
follows I6.

## 5 · Candidate guarantees to verify in I1

A starting list, from the review, the readiness checklist and the code read for this plan — **each is a
hypothesis until I1 maps it to code**:

- Gateway authentication and scope enforcement (`compliance`; the token tables; OIDC) — F1's class.
- `/a2a` admission: a bearer's scopes are dropped and an absent bearer is anonymous, so admission rests on an
  attached evaluator or federation credentials; with neither, anonymous dispatch reaches skills.
- Provider enforcement, two distinct states: with `with_provider_enforcement()` **on** and no evaluator
  attached, protected work is **refused** (fails closed); with it **off**, nothing is checked at the provider
  at all, whatever the gateway does. A profile must require the first and must not mistake the second for it.
- Peer identity: `require_identity_proofs` is default-off; *proofs required* is not *identity authenticated*.
- Transport: mesh TLS, gateway TLS, the CA key's location (`ca_key_off_node`).
- Durable revocation epochs and the persistence `SyncMode` the evidence journal assumes.
- Evidence: an evaluator attached without a journal **still enforces but records nothing** (today a warning
  at attach time). The guarantee is two entries — *authorised at the seam* and *recorded before dispatch* —
  because they fail separately.
- Egress: an empty allow-list means allow-all.
- The consensus safety profile: fixed voter set and strict-majority quorum — today a documented condition,
  not a validated one.
- Public routes: `/stats` is public in code and missing from the checklist's list of public probes.

## 6 · What this does not claim

- **A profile is not a deployment.** `Enforced` means this node, built and configured as it is, runs the
  check; "node requirements satisfied" is the strongest thing the report says. Network confinement, clock
  sync, key custody and the operator's policy are external prerequisites, listed unresolved, and the
  shared-responsibility matrix stays the operator's.
- **A catalogue entry is a claim about code, checked by a test, not proved by being written down.**
  Generated documentation cannot establish semantic accuracy; behavioural scenarios must.
- **A dependency edge between rules is a hypothesis** until code or traces support it; a diagram of edges
  proves neither causation nor controller stability.
- **Decision traces are diagnostics, not the audit trail.** The evidence journal and audit sink keep their
  guarantees; a trace buffer evicts.
- **Replay coverage is per rule.** The pilot passing does not make the substrate replay-covered.

## 7 · Open questions

- **Q1.** Profile selection: a `GossipConfig` field, an env var, or both — and does the `mycelium` binary
  default to `dev` (today's behaviour) or refuse without one? Proposed: `dev` by default, the report always
  logged, and the binary's `--help` naming the secure profile.
- **Q2.** Whether the guarantee report is gossiped (fleet-wide visibility, a new `sys/` key family) or stays
  node-local behind the gateway. Proposed: node-local first; gossip only with a reason.
- **Q3.** The private companions (AE, RA): do they register guarantees into the same registry through the
  public API? Proposed: yes — the registry is public API, and the private repo's `COMPATIBILITY.md` names
  what it registers.

## 8 · I1 audit — findings (2026-10-02, read-only sweep of the baseline)

The first exit gate of I1 says the finding list is published. The sweep covered every `#[cfg(feature)]`
arm in the gateway, lifecycle, A2A, MCP, provider-enforcement, federation and TLS code, every `GossipConfig`
field whose doc names a feature, every attached component, the consensus profile and the public routes.
IDs are the proposed stable IDs for the registry.

| ID | Guarantee | Depends on | What happened when missing | Verdict | State |
|---|---|---|---|---|---|
| `gw.token_tables` | scoped/named tokens close the gateway | `compliance` | parsed, ignored, gateway open | FINDING | fixed #468 |
| `gw.oidc` | SSO closes the gateway | `compliance` | **dropped at parse time**, gateway open | FINDING (critical) | fixed, this PR |
| `gw.tls` | gateway serves HTTPS | `tls` | plaintext, bearers in cleartext, no warning | FINDING (high) | fixed, this PR |
| `mesh.tls` + `domain.enforced` | mTLS gossip; the enforced profile | `tls` | plaintext, unauthenticated; `validate()` passed on a dead field | FINDING (high) | fixed, this PR |
| `a2a.admission` | `/a2a` is not anonymous dispatch | an evaluator | warning required *no bearer* too; exposed case silent; attach-order spurious | FINDING | fixed, this PR (warning; the profile refuses, I3) |
| `audit.chain` / `audit.sink` | governance actions are sealed and exported | `compliance` + `config.tls` | nothing sealed, sink receives nothing, report said `Set` | FINDING (medium) | open — I2 |
| `report.identity_proofs` / `report.ca_key_off_node` | report truthfulness | `tls` | said `Set` in a build where both are inert | FINDING (medium) | fixed, this PR |
| `id.ca_key_off_node` (I3, 2026-10-03) | the fleet CA's private key is not on this node | `tls::load_or_create_ca` | loads the key from the node's directory and **regenerates a whole CA without it** — every TLS node holds the key at start, so the guarantee can be reported but never enforced on a running node | FINDING (medium; the C5 removal argument rests on custody the init forces onto every node) | open — not in `secure-single-domain` rev 1; needs a TLS init that starts from a CA cert and a pre-issued node cert |
| `report.egress` | *substrate outbound fails closed* | `egress.allow_hosts` | federation client, bulk fetch and OIDC JWKS are ungated; report overclaims | FINDING (medium) | open — I1 fix, own PR |
| `at_rest.cipher` | WAL/snapshot encrypted | attach before `start()` | attached after: plaintext, no warning | FINDING (low) | open — G13 |
| `ae.recorded_before_dispatch` | the decision is journalled | `with_evidence_journal` | enforces, records nothing; warns at attach | WARN-ONLY | I3 requires it |
| `gw.tls_runtime` | a bad gateway TLS config is fatal | — | HTTP task dies, `start()` returns Ok, node reports ready | WARN-ONLY | open — I2/I3 |
| `persist.replay` | restart recovers KV and acceptor memory | `persistence` | replay failure warns, continues, then snapshots | WARN-ONLY (fail-open) | open — needs a test; may compact over unreadable state (unverified) |
| `prov.enforcement` | protected work authorised where it runs | `with_provider_enforcement` | **on** + no evaluator refuses (closed); **off** checks nothing | OK / DOC-ONLY | I3 requires *on* |
| `ae.authorised_at_seam` | gateway dispatch authorised | an evaluator | inert without one, by design | DOC-ONLY | I3 requires it |
| `authz.durable_epochs`, `authz.execution_authority`, `gw.caller_attest`, `gw.not_open`, `id.peer_authenticated`, `mesh.frame_sig` | documented conditions | — | nothing validates them | DOC-ONLY | I3 candidates |
| `cons.safety_profile` | fixed voter set, strict majority, trust slices | `ConsensusConfig` | defaults `quorum_size: 0`, `use_trust_slices: false`; nothing validates | DOC-ONLY | I3; membership-fixed and fencing are `NotVerifiableHere` |
| `gw.extra_routes_auth` | merged companion routes are gated | path under `/gateway/` | a route outside `/gateway/`, `/a2a`, `/federation/` is public without notice | DOC-ONLY | document as a public class; I3 lists them |
| public routes | the checklist names every public probe | — | `/stats`, `/bulk/{id}`, `/.well-known/agent.json` were missing from `production-readiness.md` | DOC drift | fixed, this PR |

**The pattern behind the critical one:** a `#[cfg]`'d field plus a serde that tolerates unknown keys is a setting
that *vanishes*; a field present in every build whose consumers are `#[cfg]`'d is a setting that *lies*. I2's
report resolves both to `NotInBuild` by construction, which is why it comes before the profile.

**Found by I4's reconnaissance of the pilot's path (2026-10-03), fixed before I4:** a failing initial
activation probe was recorded as a live install (advertised, counted complete) until the next health
pass — now an activation error; and the self-election draw was raw `fastrand` in a crate the seam lint
does not scan — now the `select` stream, with an RNG-only lint over `mycelium-wasm-host`. **Still open,
a baseline decision:** the host's ~140 file-system and timing call sites are outside the seam scan, so a
stem's recording does not cover artifact fetch, placement or activation timing (I6 must state them
unattributable).

**Not audited:** `mycelium-wiki` with `execution-authority` off, `mycelium-effects` with `envelope` off.

