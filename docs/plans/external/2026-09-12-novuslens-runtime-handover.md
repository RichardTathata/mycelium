> **Vendored external handover — NovusLens (Novus-i2), 2026-09-12 (re-vendored the same day at commit 144d7ae7). Body unmodified.**
> Source of record: `Novus-i2/docs/contracts/mycelium_runtime_handover.md`. Change from the first vendoring: the Cedar
> model reads the resource from `context.resource` (Cedar exposes no entity id as an attribute) and both artifacts are
> engine-checked against the Cedar CLI and OPA in the consumer's test gate — not evaluated in any runtime, which is AE2's job.
> What it fixes for the AE slice (plan rev 1.9 §6.8 / D36): the evidence contract the runtime side must produce, what comes
> back from a NovusLens policy export, the two decisive demonstrations as NovusLens reads them, the four joint seams to settle
> in AE0/AE3, conventions, and what NovusLens never does. Relative links in the body point into the Novus-i2 repository and
> are expected to dangle here.
> Do not edit below this line; changes to our plan go in `v3-contracts-axis.md`.

---

# Handover to Mycelium — what the runtime side needs from NovusLens (2026-09-12)

For the maintainers implementing the AE slice (Mycelium plan rev 1.9 §6.8, D36: AE0–AE4). This page
says what NovusLens already fixes, what it will read, what it will show, and the four things that
must be agreed jointly. Everything here is in the repository; nothing is claimed that a gate does
not exercise. Source of truth for the records: [`agent_evidence_v1.md`](agent_evidence_v1.md).

## 1. The evidence contract you produce

**Envelope.** One signed batch: `{document, alg: "ed25519", public_key_b64, signature_b64}` where
`document = {org, source, batch_id, records[], cursor?}`. The signature is over the canonical JSON of
`document`: `json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)`, UTF-8. Raw
32-byte Ed25519 keys, base64. Bounds: 1–1000 records, ≤ 2 MiB. Ids are yours, stable, namespaced by
the source; a retry is the same `batch_id` with byte-identical content (a different body under a
reused id is refused). `cursor = {stream, previous, next}` (previous may be null; must advance).
Receipt: `stored-pending-projection` means durable storage only. Event time `at` is yours;
`received_at` is replaced by server time; `at` may not lead the server by more than 5 s.

**Record.** `{version: 1, id, type, source, subject, at, received_at, payload}`. `subject` is the
logical agent, and it must equal a subject the operator bound to a cell in the source's enrolment.
Unknown payload fields are refused ("conditions must not be silently ignored").

**The types the runtime side emits, and what each establishes**

| Your state (§6.8) | NovusLens record | Payload (required · optional) |
|---|---|---|
| a routine action requested, permitted / denied / indeterminate, executed or not | `activity_observation` (contract 1.2) | `operation, resource, evidence_ref, coverage:{complete, streams}` · `destination, execution_identity, detector:{name, rule_version}, classification, decision: permit|deny|indeterminate, execution: attempted|completed|failed|unknown, policy:{revision, digest?, enforcement_point?}, mapping:{catalogue, revision, status: mapped|unmapped|ambiguous}` |
| a bounded operation with a value (procurement) | `operation` | `operation, resource, state: attempted|completed|denied|failed` · `value, unit` |
| the authority the runtime holds | `authority_grant` | `issuer, grant_version, operation, resource, unit` · `valid_from, valid_until, upper_bound` |
| an intervention asked for, decided, executed | `intervention_request` / `intervention_authorisation` / `intervention_execution` | request: `case_id, action, resource, requester, unit` · `value`; authorisation: `request_id, authoriser, grant_id, decision: approved|refused`; execution: `request_id, authorisation_id, executor, state, action, resource` |
| what then happened, observed independently | `outcome_observation` | `execution_id, criterion, unit` · `window_start, window_end, complete, value` |
| a policy revision activated (contract 1.3) | `policy_deployment` | `policy_revision, deployed_by` · `activated_at, policy_digest, enforcement_points[], coverage:{complete, notes?}, implements[], note` |
| a record superseded or retracted | `correction` / `withdrawal` | correction: `target_id, replacement_id, reason` (same source, subject and type); withdrawal: `target_id, reason` |

**How NovusLens reads decision and execution** (never anything else): `effect` is `completed` or
`failed` when you said so; `none` when `deny` with no execution or `attempted`; `unknown` when
`attempted`/`unknown`, or a refusal whose execution is unknown; `unstated` when neither is given.
`deny` + `completed` is named an *enforcement gap*. A refused attempt outside a remit is shown and
never counted as drift. Indeterminate is never read as permit. A native call you could not map
(`mapping.status` unmapped/ambiguous) is never read as a business operation.

**Enrolment (the operator's act, not yours).** Source id, your public key, subject→cell bindings,
the record types you may publish, and the principals you may name (`grant_issuers`, `authorisers`,
`acceptors`, `remit_issuers`), plus `environment: production|simulated`. Every field is a
restriction; a source cannot widen its own enrolment. Keep your private key; NovusLens never holds it.

## 2. What NovusLens sends you, and what must come back

- **Remit declaration.** Authored and approved in the product (two recorded checks), published as a
  signed `remit_declaration` from the product source. Its record id is what your deployment report
  should list under `implements`.
- **Policy export.** `GET /remits/export?format=cedar|rego` renders that declaration. The header says
  *generated, not deployed*, carries `digest = sha256:<hex of the policy body>` and lists the clauses
  the target cannot carry. Assumed Cedar model: `principal == Agent::"<subject>"`,
  `action in [Action::"<operation>"…]`, `context.resource` and `context.destination` (`==`, or `like` for a
  trailing star) — both supplied by the enforcement point; `forbid` for exclusions; validity dates are **not** in
  the Cedar artifact. Both artifacts are engine-checked against the Cedar CLI and OPA in our test gate (the
  renderer's clause set agrees with our comparator on every fully-observed input); they are not evaluated in
  your runtime, which is AE2's job.
  Rego: `package novuslens.remit.<subject>`, `default allow := false`, `input.subject/operation/
  resource/destination`, validity by `time.now_ns()`. Neither is claimed lossless.
- **What must come back for the page to say "deployment reported".** A `policy_deployment` whose
  `policy_digest` equals the export digest *or* whose `implements` names the declaration record id,
  and whose `policy_revision` is the same string your activity observations carry in
  `policy.revision`. `activated_at` must not be later than the activities it governs. State
  `coverage.complete` honestly; name the enforcement points.

## 3. The two decisive demonstrations, as NovusLens will read them

**Procurement authority (§6.8 scenario 1).** `authority_grant` with `upper_bound` in a unit;
`operation` records for completed approvals with `value`; the over-limit attempt as an `operation`
with `state: denied` (no business effect) or, where the resource ran it, `completed` — the latter
is the misconfigured-route case and NovusLens raises it; the exception as an
`intervention_request` → `intervention_authorisation` (approved, naming the grant) →
`intervention_execution`; an `outcome_observation` with an explicit window and completeness; then a
`correction` of one observation. NovusLens shows authorisation, execution and outcome apart, and
"Currently supported: N of M" after the correction, history kept. Fixture that already exercises
this shape: `apps/graph/fixtures/agent_evidence_reference.json`.

**Functional remit (§6.8 scenario 2).** A remit (yours as a policy-store source, or the product's);
`activity_observation` records with `mapping` under an agreed catalogue revision; the benign job
outside the remit with `decision`/`execution`; an explicit prohibition; an incomplete allow-list
(`complete: false` → *authority not established*, never a finding); a missing destination
(*cannot be stated*); a shared identity (no `execution_identity` → group attribution); an unobserved
route (`coverage.complete: false` → never an all-clear); a partial intervention. Fixture:
`apps/graph/fixtures/agent_evidence_drift.json` (includes `refused-1` and `unmapped-1`).

**The conformance checker you can run locally** (your D31 stub consumer): `uv run python -m
conway_connectors --org <org> agent-source check --batch <unsigned batch> --enrolment <enrolment>`
says exactly why each record would be refused; `common_py.agent_evidence.validate` is the schema.
The worked exporter (`docs/contracts/reference_exporter_walkthrough.md`) shows batching, cursors,
retries and durable progress.

## 4. The four joint seams — agree these in AE0/AE3, or the pages will lie

1. **Catalogue identity (AE0).** The customer's business-activity catalogue lives in NovusLens as a
   named, versioned, owned object (`activity_catalogues`). Your action catalogue must be the same
   object or carry its identity: every action envelope and every `activity_observation` names
   `mapping.catalogue` and `mapping.revision`. Do not invent a second identity. NovusLens marks a
   superseded revision on the page and on the finding; it never re-maps.
2. **The signed exporter (AE3).** Today's `agent-source mycelium-audit` turns a node's audit chain
   into *unsigned* `operation` records and nothing else. The AE exporter must sign as an enrolled
   source and emit the table above — `activity_observation` with decision/execution/policy/mapping,
   `policy_deployment`, the intervention triple, outcomes, corrections. Unsigned is ineligible for
   any customer conclusion.
3. **Generic tools (AE0).** For a tool like `execute_shell`, say in the catalogue what observation
   (process, file, network, protected-service record) a catalogue action requires, or declare the
   tool outside the enforcement guarantee. Emit `mapping.status: unmapped` rather than a guess.
4. **Identity binding.** Your logical agent id is the `subject`; the operator binds it to a cell.
   A shared service principal is reported as such (no `execution_identity`). The delegation chain
   from your item 7 is not in this contract; carry it in the native record behind `evidence_ref`,
   and tell us if the page should show it (a contract 1.4 candidate).

## 5. Conventions to fix together

`org` (one NovusLens organisation per customer domain; the operator names it); source ids
(suggest `mycelium:<domain>:<component>`); `policy.revision` as a string you control; `environment`
honest (`simulated` for the co-op worlds); clock discipline (`at` is event time); correction
identity (same source, same subject, same type); and the joint pack's collectors — who runs them on
AWS and GCP, and that the four runs are recorded as joint acceptance evidence, never closed by a
fixture or a stub.

## 6. What NovusLens will never do

Deploy or enforce a policy; resolve overlapping remits by the more permissive; infer a mapping
from a tool name, an explanation or intensity; read indeterminate as permit; treat a deployment
report as proof of coverage; or rewrite history on correction. Export generated, deployment
reported and enforcement observed stay three statements.
