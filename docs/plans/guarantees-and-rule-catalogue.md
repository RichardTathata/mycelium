# Guarantees and the rule catalogue — a supported secure profile, and decisions you can trace

**Status:** proposed, rev 0.1, 2026-10-02. Nothing here is built. Source baseline: `main` after v2.18.0
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
start.* Writing that row is what would have found it. So the inventory comes first, it is shared, and each
track reads from it.

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
| G4 | **Each guarantee resolves at start to `Enforced` / `NotConfigured` / `NotInBuild` / `NotVerifiableHere`.** The last is for what a node cannot see (network confinement, clock sync, an off-node CA key's custody beyond this host); it is reported, never counted as met, and never refuses. | `ConfinementReport`'s three states plus the honest fourth. A node vouching for what it cannot see is the failure this whole axis exists to avoid. |
| G5 | **A node started under a profile refuses to start when any required guarantee is `NotConfigured` or `NotInBuild`**, naming each by ID, the missing feature or setting, and the doc that says how to provide it. A node started under no profile behaves as today and logs its report. | Fail closed where a guarantee is claimed; change nothing where none is. Refusing by name is the codebase's convention. |
| G6 | **Unconditional refusals stay unconditional.** A configuration that is *self-contradictory* — a token table the build cannot enforce (#468), a `llm:*` scope — refuses under every profile, `dev` included. | A setting that silently does nothing is a defect whatever the deployment intends. |
| G7 | **The trace changes no decision.** It reuses values the code already produced; no RNG stream, no shared HLC advance, no network, no authority decision; nothing serialised under a subsystem lock; bounded by bytes *and* count; off by default; never blocks work. | The design note's §*Behaviour preservation*, adopted whole. |
| G8 | **Coverage is stated, never implied.** A rule is *instrumented*, *catalogue only* or *unsupported*; a replay bundle without the decision attachment means *trace unavailable*, not *no decisions*. | The project's evidence discipline (`what-is-proven.md`). |
| G9 | **Controller-interaction experiments are research, not a deliverable here** — recorded as a §13-style question with the trace as their instrument. | Open-ended; the combined-feedback question already has a dated bound in the evidence ledger. |

## 4 · Increments

Each is a separately reviewable change or small series, with a regression test seen failing first where it
fixes a defect.

| Row | Deliverable | Exit gate |
|---|---|---|
| **I1 · the inventory** | The descriptor types (G1), a registry, and the first entries: **every guarantee in `production-readiness.md`** and the settings in `ConfinementReport`, each with its enforcement points; and **the rules the stem provisioning pilot touches** (unmet-demand response, eligibility, provenance, acceptance and shadow, self-election, activation and probe, withdrawal), plus catalogue-only entries for propagation, admission, membership and expiry. A generated catalogue (JSON + one doc page). **The audit is the point:** every guarantee whose enforcement depends on a feature or setting that nothing checks at start is recorded as a finding, and each finding gets a regression test and a fix in its own PR. | CI checks duplicate IDs, unknown references, missing reason definitions, missing test references. Every pilot rule maps to a real decision point and a behavioural test. Every readiness-checklist row maps to a guarantee or is marked *operator-owned* with a reason. The finding list is published, including *none found* where that is the result. |
| **I2 · the startup report** | `GossipAgent::guarantee_report()` generalising `confinement_report()`: every registered guarantee resolved to G4's four states, as a typed value, a log block at start, and a gateway route (read-only, scoped). `ConfinementReport` becomes a view over it, unchanged in shape. | A test per state per guarantee; the report for a default node matches a checked-in golden; the confined-fleet test still passes unchanged. |
| **I3 · the profile** | Named profiles (G3) selected by config and env; refusal at start (G5) naming each unmet guarantee; unconditional refusals gathered (G6). Negative tests across the CI feature matrix: for each profile × feature set, start succeeds exactly when every required guarantee resolves `Enforced`. An unauthenticated request to every protected route, by the handler's own counter, under `secure-single-domain`. | The matrix test; the counter test; the readiness checklist generated from, or checked against, the profile. The `dev` profile visibly labelled in the report and the Ops Console. Ships in a MINOR with an upgrade note. |
| **I4 · the trace core** | The design note's increment 2: a bounded, nonblocking `DecisionRecord` sink (a leaf module or crate with no runtime dependency), the record schema (rule ID and revision, build and config digest, node, incarnation, local sequence, target, trigger and parent references *when known*, bounded inputs with age and provenance, view status, outcome and typed reason, effect reference, completeness flags), off by default. | Trace off vs on: the same decisions and effects under identical replayed inputs, no extra RNG draws or decision-clock events. Saturation: work continues; drops and truncations visible; memory bounded by bytes. |
| **I5 · the provisioning pilot** | The design note's increment 3: instrument the stem lifecycle without changing guard order, randomness or policy — observed deficit, candidate checks, self-election using the existing draw, live or shadow or defer or decline, install → activation → probe linked by a local operation ID, advertisement and withdrawal only when they happen. | A survivor acquiring and serving a permitted capability after an origin fails, explained end to end; shadow-before-acceptance and failed-health variants; an activation failure never recorded as success. |
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
- Provider enforcement: `with_provider_enforcement()` fails closed without an evaluator — a provider that
  attaches none enforces nothing.
- Peer identity: `require_identity_proofs` is default-off; *proofs required* is not *identity authenticated*.
- Transport: mesh TLS, gateway TLS, the CA key's location (`ca_key_off_node`).
- Durable revocation epochs and the persistence `SyncMode` the evidence journal assumes.
- Evidence: an evaluator attached without a journal enforces and records nothing (today a warning).
- Egress: an empty allow-list means allow-all.
- The consensus safety profile: fixed voter set and strict-majority quorum — today a documented condition,
  not a validated one.
- Public routes: `/stats` is public in code and missing from the checklist's list of public probes.

## 6 · What this does not claim

- **A profile is not a deployment.** `Enforced` means this node, built and configured as it is, runs the
  check; network confinement, clock sync, key custody and the operator's policy stay
  `NotVerifiableHere`, and the shared-responsibility matrix stays the operator's.
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
</content>
</invoke>
