## [2026-09-16] ingest | the AE seam records what it enforces — evidence, `/a2a`, a live stale-policy check

Up: [dev](../dev.md) §AE · record `docs/design/action-envelope-ae0.md` §11 · plan §6.8 · PR #224 ·
code `src/agent/action_evaluator.rs`, `src/agent/http.rs`, `src/agent/a2a.rs`.

**How it was found.** Not by review of this code, but by building the private exporter *against* it.
The exporter needed records to export; there were none. That is the general lesson worth keeping: the
gap was invisible from inside the seam's own tests, all of which passed, because they asserted what the
preflight **decided** and nothing asserted what it **left behind**.

**1. The gateway wrote nothing.** `ae_preflight` refused, logged, incremented a counter and returned —
no record for a refusal, none for a permit. A deployment enforcing a declared remit produced no evidence
at all. Every evaluated dispatch is now sealed into the node's tamper-evident chain as an `AeEvidence`
document (`mycelium.ae/evidence/1`) under `detail`.

**Both outcomes, and the permit is the one people forget.** Recording only refusals leaves the
interesting half invisible: an evidence stream that omits its permits cannot support any statement about
what an agent was *allowed* to do, which is most of what a governance page claims.

**Why a document beside the audit record's own fields.** An `AuditRecord` carries a three-valued
outcome. A decision carries a verdict, a policy revision, the constraints actually checked, whether the
action ran, and the reviewed activity it maps to. Rounding those into `Success | Denied | Error`
collapses *prohibited* into *not established* — the precise distinction the whole slice exists to
protect. The document travels beside the summary and a reader takes the document; the coarse outcome is
a summary and is documented as one.

**A record that cannot be written refuses the dispatch** — `PreflightRefusal::NotRecorded` (`-32032`),
**permit included**. Enforcement without attribution is not governance, it is an unlogged gate, so the
failure is visible rather than silent. A build without `compliance` has no chain at all; rather than
refusing every dispatch and breaking gateways that never asked for an evaluator, `with_action_evaluator`
warns at attach time — an operator should not have to infer it from an empty stream.

**2. `/a2a` was unguarded.** The evaluator ran on `tools/call` only, while `/a2a` is the other edge
foreign agents act through and the plan names both as the wedge. A remit enforced at one door could be
walked around by choosing the other — and the evidence would have gone on saying `coverage.complete:
false`, truthfully and uselessly. Both A2A paths now run the same preflight. The preflight takes a
`TaskCtx` rather than an `HttpCtx` so the routes share one implementation instead of growing a second
copy that drifts; that refactor *is* the fix's durable half.

**3. The stale-policy check could never fire.** `expected_policy_revision` was hardcoded `None` with a
comment saying it awaited a deployment report — so the seam had no second opinion and a gateway running
a superseded policy was undetectable, the one condition the check exists for.
`set_deployed_policy_revision` supplies it (an `ArcSwapOption`, not a lock: wholly replaced on reload,
read on the dispatch path, and no new row in the lock-order table). Unset it still does not fire — now a
reported fact rather than a structural impossibility.

**A gate that could never run.** The A2A evidence test needs `compliance` (the audit chain) *and* `a2a`
(the route). No standard gate built the pair, so the test would have been written, passed locally, and
silently never run in CI. CI's compliance job now builds both. This is the make-check-vs-CI-green family
again, one layer down: not a gate that is missing, a **feature combination** no gate covers.

**Gates.** `make check` clean; 506 (`compliance,a2a`) + 447 (`tls,metrics,a2a,llm`) + 343
(`--no-default-features --features gateway`) + 179 (`mycelium-core`). Additive throughout —
`PreflightRefusal` is `#[non_exhaustive]`, and a node with no evaluator attached behaves exactly as
before.

**What is still owed on this line.** The remaining gateway dispatch paths, and the node-local evidence
journal with its `append -> LocalSync` receipt and three failure tests (saturation, persistence failure,
lost acknowledgement). Today's sealing is a direct audit-chain write: honest, tested, and it gives the
enforcement point no durability receipt to act on beyond success or failure. Recorded in AE0 §11 rather
than quietly counted as done.
