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
**Policy over history (post-360 row K, 2026-10-17):** an evaluator can decide on what an actor has already done —
`history_request` / `evaluate_with_history` (defaults keep every evaluator as it was), the history built from this
node's own evidence journal by `HistoryIndex` (`evidence_journal.rs`; loaded at attach, fed by `ae_record`, bounded),
**counted at this enforcement point** by decision (the scope named on every history decision, `HistoryScope`), an
unreadable history `Unavailable` and so `Indeterminate`, never empty. Two lessons: a history-limited decision is a
check-then-count, so one actor's are serialised (lock-order row 61) and a permit counted as it is decided; and a view
capped near the limit is filled by the actor's *other* records, so a counting evaluator asks for the whole window. The
decision trace does not carry `ae.preflight`, so the history input is recorded on the evidence record.
**Inference routing as policy (post-360 row R, 2026-10-17):** a routing door hands the preflight what it established
— `gateway_preflight_routed` with `RouteFacts` (the destination and its `Locality`, the request's `DataClass` and its
`ClassSource`, the destinations already refused) — onto `ActionEnvelope::{destination, data_class}` and
`AeEvidence::route`. `mycelium-reason`'s router asks per candidate, in its own order, and dispatches to the first the
policy permits. The design rule that keeps it closed: a class or locality the door **did not establish** matches a rule
about it and is reported *not established* — so a door that classifies nothing (`/gateway/llm/call`) cannot pass a class
rule, and an undeclared locality is never in-domain. Declared, not detected: the class is the client's or the operator's
statement, the locality the provider's.
**Causal records (adopter-surface X4, 2026-10-23, first PR):** two fields a request carries into every record and no
record mints — `params._meta.correlation` (one key segment, the run) and `params._meta.parent` (the content hash of
the record the work depends on, the hash `journal_sha256` cites) — read by `ae_decide` beside `operation_id` and
`mandate` (`agent/causal.rs`), onto `ActionEnvelope::{correlation, parent}` and every `AeEvidence` the point writes;
`/mcp` forwards the two keys inside the `tools/call` payload and `provider_enforcement::check` reads them there.
The rule that keeps it honest: **the application payload and only there** — the caller envelope's signature digests
the payload, so a header or an envelope field could be re-pointed under a signature that still verifies; both are
counted (`causal_misplaced`) and never read, and a malformed value is `PreflightRefusal::Malformed` before policy,
never corrected. The reader (`closure.rs`): a record's identity is the sha256 of its bytes as written (a journal
frame's body, a trace line); `Corpus::chain` marks captured / reconstructed (by `operation_id`) / unattributable,
`Corpus::closure` follows parents breadth-first, visits each record once (a hostile cycle terminates — fuzz target
`closure_corpus`), names holes with the sinks' drop counts, stops at `gateway:federation/call`, and says first when
fewer journals were given than the records name nodes. Authoritative over the journal, best-effort over the trace.
The second PR (#672): `Preflight::Proceed { journal_sha256 }` and `/mcp` forwarding its own decision's hash as the
provider's `parent` (the caller's where no record was established — two meanings, told apart by the gateway's record);
the door registry `scripts/causal-doors.txt` with its gate `check-causal-doors.py` (`make check`, CI; the scan ends
only at a `#[cfg(test)] mod`, and the self-test plants a door after an inner cfg(test) item); the direct-call, `/a2a`
and LLM doors shown; the SDK verbs (A2A send, prompt-skill call, federation call, set) and the reason `run_id` join;
`tests/causal_closure.rs` over three nodes. Still not: an execution record at the provider, the stem activation's
parent, the tuple-space and blackboard doors.
**Fleet-wide history scopes (post-360 row K2, 2026-10-17):** a rule may name `HistoryScope::FleetConsensus` or
`FleetTally` (`Rule::at_most(..).scoped(..)`, the policy choosing per rule; per-point stays the default). The evaluator
names the counter its deciding rule relies on (`fleet_counters`), the point reads it into `HistoryView::fleet`
(`src/agent/fleet_history.rs`). **Exact** = N consensus *seats* (`ae-seat/…` slots, leased for the window, decided by the
exclusive verbs' electorate): a permit claims one by `exclusive_propose` after the evaluator permits and before
admission, read back from the converged decision record (the #164 discipline) — the seat, not the evaluator's reading, is
what bounds the fleet; no quorum ⇒ `Indeterminate`. **Approximate** = per-point permit times under `sys/ae-tally/`
(self-owned) plus a heartbeat; the sum over self and fresh peers; coverage judged by the governors' confidence bound
and per-publisher staleness — stale or unknown is *not established* (the strict-eligibility style). Gateway and
provider count apart fleet-wide, as at one node. The per-actor gate (lock-order row 61) is now held across the claim's
consensus round — no longer: #642's review (round 1) moved the claim after the gate (finding 1), leased a seat for
the window + 2 s (since 2026-10-19: + 19 225 ms, the longest a claim is allowed to take, with the holder releasing the seat
a window after admission) and refused one whose remaining lease is under a window (finding 2), started every node's heartbeat at
`start()` and withdrew it in the shutdown sweep (3), tombstoned aged tally entries and kept seats only for named actors
(4 — row A forbids collecting a top record or sentinel), keyed counters by a declared opaque id (5), and made a peer
without a heartbeat unknown coverage (6). Round 2: the provider decides whether to defer from the real envelope
(`ae_decide` / `Settling`, `provider_enforcement::check` → `Checked`), bounded and always answered; a graceful leave is a departed heartbeat
(entries kept to age out); tally writes under one leaf lock (row 67). Not built: early seat release, seat collection,
signed tallies.
**Residue** (round 4): a non-peer's tally keys past its heartbeat's horizon are tombstoned at the stamps held
(`collect_residue`, `kv_delete_held`), so a newer write by a returning owner wins. Since 2026-10-20 an entry that
reaches a collector *after* its node's heartbeat was collected is dated by its own times against the same bounds; before,
a node with no heartbeat was skipped, and such an entry stayed in the store and the scope roster for ever (found as a
CI flake of `residue_is_collected_and_a_decision_does_not_read_it`).
A reader holding a verified member's entries **without its heartbeat** reads it as unknown coverage, not zero, while
the entry is inside the window plus both bounds — the collector's own bound (the same when the writer's window is the
counter's), so the node stops being consulted when its entry is collected (#656's review, finding 1;
`deprecations.md` §56). Since #656's second review a non-peer's keys go each by its own times and the heartbeat last
(past `MAX_FLEET_WINDOW_MS` plus both bounds — a collector cannot see windows held only elsewhere), and a time past
now + drift is forged: it dates nothing anywhere (reader, owner, collector).
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
