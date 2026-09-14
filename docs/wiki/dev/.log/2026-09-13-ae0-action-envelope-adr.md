## [2026-09-13] ingest | AE0 — the action-envelope ADR

Up: [dev](../dev.md) §Planned AE · record `docs/design/action-envelope-ae0.md` · plan §6.8, D36–D38 ·
handover `docs/plans/external/2026-09-12-novuslens-runtime-handover.md` (seams 1, 3, 4; §1 envelope).

**Why now.** Queue §10.12.3: the AE0 ADR beside item 1's, so AE-T never mints a second identity scheme. Item 7
(the verified actor) and item 1 PR 1 (`operation_id`/`attempt_id`, the receipt ladder) are the two facts the
envelope binds; both exist as of today.

**Decisions fixed.** The envelope is assembled *only by the enforcement point* from facts it verified (the item 7
rule generalised). Three verdicts; **indeterminate is never permit**, and covers missing facts, evaluation
errors and unrecognised clauses — errors are the decision's data. Deterministic (no clock, no network: time
enters as `validity`). **Cedar in-process** is adopted as the one real adapter (D37 was provisional) — no sidecar,
no policy server; Rego stays a replaceable integration. Five evidence records, never inferred from one another;
`deny` + `completed` is exported as an *enforcement gap*. **Catalogue identity is the consumer's**
(`activity_catalogues`); generic tools are `unmapped`, never guessed. Three strength profiles named in the
guardrails tier vocabulary — the gateway preflight is *SelfImposedPrevention* for one route and says so with
`coverage.complete: false`; only AE2's resource fence is *HardPrevention*. The standards matrix pins subsets
with fixtures; no lossless ODRL/XACML/Cedar/Rego translation is promised; an export that would weaken the
required boundary is rejected. Eleven negative fixtures are the CI gate the seam and every evaluator must pass.

**Deliberately open.** Replay past validity, argument substitution after the check, and the check/use race are
*named* at the gateway and closed only inside AE2's effect boundary. Mandate and budget facts are absent until
items 5 and 4 exist — a clause that needs them yields `Indeterminate`, not a guess.

**External review before merge (2026-09-14), three P2 gaps, all taken.** (1) The draft routed evidence "through the
audit chain and out the sink" — but `seal_and_write` stores the whole signed record in gossip KV first, contradicting
§6.7's *never gossip*. Now: a **node-local evidence journal** holds the records; the gossiped chain carries only a
**safe reference record** (kind, ids, verified principal, verdict, policy revision, content hash), so the chain still
covers the evidence without disseminating it. (2) `AuditSink::export` returns nothing, runs on a drain task and drops
on saturation, so it cannot be the strict profile's durability barrier. Now: the journal's `append -> LocalSync`
(item 1's receipt, `OnDisk` before the effect) is the ack-capable contract; the exporter is a separate cursor reader;
three failure tests are named (saturation, persistence failure, lost acknowledgement ⇒ refuse in the strict profile).
(3) "deny + no execution ⇒ effect none" was an inference from silence. Now: `none` only from an explicit **blocked**
attestation scoped to the enforcement point and `attempt_id` (`execution: not_dispatched`, proposed to the consumer as
a contract-1.2 addition); otherwise `unknown` / `unobserved`.

**Next.** The evaluator seam (public, on item 7's `ResolvedPrincipal`): types, the hook between `gateway_auth`
and the MCP dispatch, the reference evaluator, the fixtures as tests, secure-profile refusal. Then AE-T T2–T4 in
the private companion.
