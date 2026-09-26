# dev/ — how the substrate is built and verified

↑ [wiki root](../wiki.md) · schema: [AGENTS.md](../AGENTS.md)

System knowledge only (routing test → *no*: about our code, tests, infra). Code is canon —
pages here cite `src/` / `mycelium-core/src/` rather than paraphrasing it.

## Areas

- **[architecture/](architecture/architecture.md)** — the three layers, the crate split,
  runtime invariants that keep recurring in review, and **[contracts](architecture/contracts.md)**:
  what an acknowledgement proves, and the floor that is how its meaning gets changed.
- **[concurrency/](concurrency/concurrency.md)** — the lock-order table, lock-free (papaya)
  mutation rules, atomics ordering policy. The discipline that the calibration ledger shows
  is this codebase's recurring bug family.
- **[testing/](testing/testing.md)** — test conventions, the feature matrix, the CI-gated
  Docker cluster suites (+ their self-diagnosing harness), scale-test + Docker-bridge lore,
  the SWIM divergence saga — and **[replay](testing/replay.md)**: the nondeterminism inventory,
  the seams, scenarios A/B/C and the checked-in corpus.
- **[companions/](companions/companions.md)** — the companion crates built on the public API
  (tuple-space, blackboard, wiki, wasm-host, agentfacts, and the axis' three: `mycelium-effects`,
  `mycelium-sim`, `mycelium-commitment`), plus what a new one owes before it counts as landed.

## Leaf pages

- **[security.md](security.md)** — the v1.x WS1–WS5 security surface + crown-jewel posture.
- **[diagnostics.md](diagnostics.md)** — the emergent-detector layer (Legible Emergence, Phases
  1–5, complete): the five coordinator-free detectors + the three-verb operator spine —
  **localize** (`/gateway/fleet`) · **explain** (`/gateway/explain`) · **diagnose**
  (`/gateway/diagnose`), the `ViewConfidence` per-node-estimate posture, and the `/stats` +
  `/metrics` surface.
- **[operations.md](operations.md)** — diagnostics endpoints, task-count reference, feature
  gates, the ops label.
- **[examples.md](examples.md)** — the coop suite, AFN pipeline, A2A community demos; the by-layer
  map and the browser-showcase set.
- **[ui-example-contract.md](ui-example-contract.md)** — what every browser (UI) example must do:
  gateway+metrics features, Ops Console linking, opt-in audit, and the "what you're seeing" concepts box.
- **[history.md](history.md)** — the delivery ledger: v1.x + v2.0 workstreams, PR ranges,
  what was declined-with-evidence.

## V3 runtime authorisation and evidence (the AE slice)

The project owner adopted the AE slice on 2026-09-12 (plan rev 1.9). **Its public half is built**
— the evaluator seam, the evidence journal and its reader, the execution record, the mandate
binding (AE1) and the evaluator-neutral contract fixtures (`mycelium::ae_contract`) all ship; the
adapters, the exporter and the cloud pack are the private companion's. This section's heading said
*"not implemented"* until 2026-09-23, which had been wrong for nine releases: the paragraphs below
had been kept current while the sentence introducing them had not.
See [the plan of record §6.8](../../plans/v3-contracts-axis.md#68-the-ae-slice--runtime-authorisation-and-evidence-rev-19)
and [ROADMAP](../../../ROADMAP.md). It composes receipts, caller identity, resource fences,
allocated rights and replay; policy evaluation is replaceable and no new central control service
is introduced. Local CI uses a stub consumer; the separately required joint pack runs both
scenarios on AWS/GCP with Mycelium participants and NovusLens evidence views.
**The evaluator seam shipped 2026-09-14** (`src/agent/action_evaluator.rs`, `gateway` + `tls`): the
`ActionEvaluator` contract, the `ActionEnvelope` assembled from verified facts, the deterministic
`ReferenceEvaluator`, and the hook at the MCP `tools/call` dispatch — inert until
`with_action_evaluator` attaches one. The AE0 §9 negative fixtures are its unit gates; two live-gateway
tests prove the wiring refuses before a tool runs. A **route-level preflight**, never enforcement at the
effect.
**The seam started recording 2026-09-16** (PR #224): until then the preflight refused, logged, and wrote
*nothing* — a node enforcing a declared remit produced no evidence at all. Every evaluated dispatch,
**permit and refusal both**, is now recorded; a decision whose evidence cannot be established
**refuses the dispatch** (`PreflightRefusal::NotRecorded`). The same PR put the preflight on `/a2a`
(both paths, enforcement point `gateway:a2a`) and gave the stale-policy check its second opinion
(`set_deployed_policy_revision`), which was hardcoded `None` and so could never fire.
**Corrected the same day** (PR #225): #224 recorded by sealing the whole decision document into the
audit chain, which **gossips to every node** — AE0 §5 forbids that and says why. Evidence now goes to
the node-local `EvidenceJournal` (append-only, fsynced, never gossiped, item 1's `LocalDurability`),
and the chain carries an `AeReference` — identities, verdict, policy revision, catalogue id, and the
journal record's **content hash**, so tamper-evidence survives without dissemination. Three failure
behaviours are tested: saturation and persistence failure refuse; a lost acknowledgement is
`DeliveryUnknown`, never `Failed`. `EvidenceProfile` decides whether they gate the effect or are
merely declared.
**The reader seam shipped the same day** (PR #226): `read_evidence_journal_from(path, cursor, …)`,
§6.7's outbox shape — batch, ship, advance, with a byte-offset cursor so resuming stays cheap, and
each entry carrying the same content hash the chain's `AeReference` cites. Needed because moving
evidence out of the chain left an `AuditSink`-based exporter seeing only references: durable and
unreachable. Next on this line: the exporter itself (the private companion's half), and four of §5's
five records.
**The execution record shipped the same day** (PR #227): a permitted dispatch now writes a second
journal record saying what the gateway observed — `completed`/`failed` from the provider's reply,
`none` when refused before sending, and **`unknown` on a timeout, never `failed`** (the call may have
run). Before it every permitted call read as `effect: unknown`, so the evidence could say what an
agent was allowed to do and never what it did.
**AE2 adopted 2026-09-20** — [`design/action-envelope-ae2.md`](../../design/action-envelope-ae2.md):
enforcement *at the resource* and what its strength depends on — the enforcement point that is not the
gateway, which is why `preflight` went public in 2.12.0 and why 2.15.0's `with_provider_enforcement`
exists; the private companion's resource-side point embodies it. (Uncited from this wiki for six days
after adoption — lint 2026-09-26.)
**AE0 adopted 2026-09-13** — [`design/action-envelope-ae0.md`](../../design/action-envelope-ae0.md): the envelope
(item 1's `operation_id`/`attempt_id` + item 7's verified actor, operation, resource, argument digest, mandate,
`policy.revision`, validity, mapping), the `ActionEvaluator` contract (permit · deny · indeterminate; deterministic;
Cedar in-process as the one adapter, D37 adopted), five evidence records through the `AuditSink`, the consumer's
catalogue identity (no second identity), three strength profiles, the pinned standards matrix, and eleven negative
fixtures the seam and every evaluator must pass. Next: the evaluator seam on item 7, then AE-T T2–T4 privately.
**Rev 1.10 (2026-09-13)** pulls a thin slice (**AE-T**) forward to Phase B: caller identity on the
`tools/call` / `/a2a` paths, an evaluator + one in-process Cedar adapter at the gateway, a signed
`AuditSink` exporter, scenario 2 locally. Guarantee stated as a route-level preflight. *The wedge is
the gateway, not the fleet* — plan §1.5.
History: [.log/2026-09-12-runtime-authorisation-evidence.md](.log/2026-09-12-runtime-authorisation-evidence.md)
· [.log/2026-09-13-ae-thin-slice.md](.log/2026-09-13-ae-thin-slice.md)
· [.log/2026-09-13-commitment-companion.md](.log/2026-09-13-commitment-companion.md) (rev 1.11: contract net
restored as the **commitment companion**, plan §6.9; the two senses of "contract", §1.2)
· [.log/2026-09-13-delivery-surfaces-rev1.12.md](.log/2026-09-13-delivery-surfaces-rev1.12.md) (rev 1.12: §12 examples,
chapters, runbooks and decks aligned with revs 1.9–1.11)
· [.log/2026-09-22-a-credential-that-names-the-call.md](.log/2026-09-22-a-credential-that-names-the-call.md)
(federation's confused deputy: the credential authenticated *who is asking* and not *what they asked*, so a
body could be rewritten in flight under a valid header — **authenticating the caller is not authenticating
the call**)
· [.log/2026-09-22-does-stability-compose.md](.log/2026-09-22-does-stability-compose.md)
(the ADR's *loops unstable together, stable alone* stays unshown — two isolation methods measured, neither
can pose the question, because the three loops share one state variable)
· [.log/2026-09-22-the-fuzz-gate-that-had-never-run.md](.log/2026-09-22-the-fuzz-gate-that-had-never-run.md)
(v2.11.1: four defects behind a sequential gate that stopped at its first crash — **a gate that exists is
not a gate that runs, and a gate that runs is not a gate that checks**).
