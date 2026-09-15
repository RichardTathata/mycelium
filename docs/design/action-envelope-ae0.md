# AE0 — the action envelope, the evaluator, and the evidence it leaves (ADR, v3 contracts axis AE slice)

**Status:** adopted 2026-09-13 · **AE0** of `docs/plans/v3-contracts-axis.md` §6.8 (rev 1.9–1.10; decisions D36,
D37, D38; queue §10.12.3). A contract, not an API: the code lands as the **evaluator seam** at the gateway
(public, this repository — the `ActionEvaluator` hook between `gateway_auth` and the dispatch, a deterministic
reference evaluator, the negative fixtures), then **AE-T T2–T4** (the Cedar adapter, the signed exporter, the
procurement mapping — the private companion), then AE1–AE4 in their phases. No wire change, no KV prefix, no
daemon, no callback to the evidence consumer. **Sits beside** [`contracts-receipts.md`](contracts-receipts.md)
(item 1) and on top of item 7 (`GatewayCaller`, shipped): it mints **no second identity scheme** — the envelope
carries item 1's `operation_id` / `attempt_id` and item 7's verified principal.

> Posture, once: every guarantee below is stated at the strength it has (rule 6). A gateway decision is a
> **route-level preflight**; only AE2's resource fence is enforcement *at the effect*. Anything that reaches a
> tool without traversing the gateway is outside the guarantee, and the evidence says so
> (`coverage.complete: false`). Prevention here is shape (ii) of rule 3 — checked at a component the caller
> already trusts — never taught to Layer I.

## 1. The three questions a protected action must answer

1. **Who is asking to do what, to which resource, with which arguments?** — the *envelope* (§2).
2. **May they, under which scoped authority, and says who?** — the *evaluator* and the *authority facts* (§3, §4).
3. **What then happened, and who says so?** — the *evidence* (§5): request · decision · execution · outcome,
   five separate records, never inferred from one another.

Discovery is not permission (a `cap/` advert or an MCP `tools/list` entry proves reachability, nothing more). A
signature authenticates its signer; whether that signer may *grant* the claimed authority is a separate
admission/issuer check. Prompt guidance is not enforcement.

## 2. The action envelope

The envelope is the one object the evaluator sees. It is **assembled only by the enforcement point** (the
gateway today; a protected service under AE2) from facts that component verified itself. Caller-supplied
identity, principal, scopes or catalogue claims never enter it as authority — the same rule as item 7's
context: a struct supplied by anyone else is not evidence.

| Field | From | Verified by |
|---|---|---|
| `operation_id`, `attempt_id` | item 1 (caller-minted, pre-dispatch, stable across retries; one `attempt_id` per delivery) | carried, not verified — the identity of the receipt this decision will be correlated with |
| `actor` | item 7: `GatewayCaller.principal` (`oidc:{sub}` · `token:#{i}` · `token:legacy` · `anonymous`) and `via` (the gateway node) | the auth layer; on a `tls` mesh the node's attestation over the request digest |
| `delegation` | item 7's represented-principal chain when present (AE1 extends: SPIFFE / customer identity bindings) | the auth layer; **workload identity alone confers no authority** |
| `operation` | the native verb: MCP `tools/call {name}` · A2A `tasks/send {skillId}` · `rpc/call {kind}` · … | the route |
| `resource` | the exact target: provider node + tool/skill/kind; under AE2 the protected resource's own identifier | the route; AE2: the resource |
| `arguments` | the security-relevant arguments **and** their canonical digest (`sha256` of canonical JSON) — the digest establishes integrity, the values carry the meaning a condition such as *amount ≤ 500* needs; a digest alone cannot decide one (review, 2026-09-15) | the enforcement point computes the digest itself and lifts **only the argument names the evaluator declared**, so the rest of the payload never enters the envelope or the evidence |
| `mandate` | item 5's mandate identity + epoch when a mandate governs the resource (empty until item 5) | the resource fence (AE2) |
| `policy.revision` | the revision the evaluator will evaluate — the **`sha256` of the loaded policy artifact**, as a string the runtime controls | the evaluator loader |
| `validity` | issued-at (HLC physical ms), not-after | the enforcement point |
| `correlation` | the gateway request id; the audit chain position the decision will be sealed at | the enforcement point |
| `mapping` | `{catalogue, revision, status: mapped \| unmapped \| ambiguous}` from the reviewed catalogue (§6) | the catalogue lookup — never a guess from a tool name |

**Binding.** A decision is bound to `(actor, operation, resource, arguments-or-digest, validity,
policy.revision)`. A reusable bearer, a cached preflight, or a decision for a different argument digest is not a
decision for this action. Replay of a decision past `validity`, **argument substitution** after the check, and the
**check/use race** (the resource changes between evaluation and effect) are named here and closed only where the
decision is enforced inside the effect boundary (AE2); at the gateway they are stated as open, not as solved.

## 3. The evaluator

```text
trait ActionEvaluator {
    fn evaluate(&self, envelope: &ActionEnvelope, facts: &AuthorityFacts) -> Decision
}

Decision { verdict: Permit | Deny | Indeterminate,
           checked: [Constraint],          // what the policy actually tested
           reason:  String,                // human-legible, never the policy text
           policy:  { revision, digest? },
           errors:  [EvaluationError] }    // policy evaluation failures, unrecognised clauses
```

Rules:

- **Three verdicts, and indeterminate is never permit.** `Indeterminate` covers: a fact the policy needs that the
  enforcement point could not establish (no verified actor, an unmapped operation, an unknown resource); an
  evaluation error; an unrecognised clause. The secure profile **refuses the effect** on `Indeterminate`
  (§7); a lenient profile may log-and-permit only where its declared strength says so.
- **Deterministic.** Same envelope + same facts + same policy revision ⇒ byte-identical decision. The evaluator
  holds no clock and no network; time enters as `validity` in the envelope.
- **Replaceable.** The interface is the contract; the **reference evaluator** (public, this repository) is a
  small deterministic allow/deny/constraint matcher used by the CI fixtures; the **one real adapter is Cedar,
  in-process** via the `cedar-policy` crate (D37 — provisional until this record; now adopted): no sidecar, no
  daemon, no policy server (*not a platform*). OPA/Rego stays a replaceable integration, not a claimed adapter;
  no lossless translation between ODRL, XACML, Cedar and Rego is promised.
- **XACML's separation as architecture, not as engine.** Enforcement point (the gateway / the resource),
  decision point (the evaluator), information point (`AuthorityFacts`), administration point (the policy
  artifact + deployment report). No XACML engine, no XML on the wire.
- **Errors are decisions' data**, not exceptions: an evaluator that cannot load its policy returns
  `Indeterminate` with the error, which the evidence then carries.

## 4. Authority facts

What the enforcement point can assert about the authority behind the actor, each with its issuer:

- **Scopes granted for this request** — item 7's `GatewayCaller.scopes` (credential ∩ route).
- **Roles** — `sys/role/{node}` signed claims (compliance), for node actors.
- **Mandate** — item 5's `{mandate, epoch, holder}` once it exists; until then absent, and a policy clause that
  needs it makes the decision `Indeterminate`.
- **Allocated rights / budgets** — item 4's ledger once it exists (strict shared budgets reserve before dispatch;
  local counters cannot establish a fleet bound). Until then absent, same rule.
- **Freshness / validity bounds** — a locally verified policy is usable within its declared freshness; offline
  authority is permitted only where the declared profile allows it; immediate revocation during a partition is
  **not promised** by a disconnected evaluator, and the profile states which operations wait, refuse or continue.

## 5. Evidence — five records, one chain

Every record is written to a **node-local evidence journal** — durable, append-only, never gossiped — and
carries: source identity · event time and receipt time · `policy` and `mandate` revision · `coverage` ·
`correlation` · correction/supersession references. What enters the gossiped, tamper-evident audit chain
(`sys/audit/{node}/{seq}`, `AuditRecord { principal, action, target, outcome, detail }`, Ed25519) is a **safe
reference record** only: the record's kind, the `operation_id` / `attempt_id`, the verified principal, the
decision verdict, the policy revision, and the **content hash** of the journal record — never arguments, argument
digests' pre-images, resource details beyond the catalogue id, or any payload. The chain's ordering and
hash-linking therefore cover the evidence (a journal record that does not match its chained hash is detectable)
while the evidence itself stays where §6.7's rule puts it: outside gossip KV, leaving only through an exporter.
*Why this correction (review of the first draft):* `seal_and_write` stores the complete signed record in gossip KV
before mirroring it to the sink, so "seal it into the chain, export through the sink" would have disseminated
sensitive action evidence to every node.

**The journal contract (the acknowledgement the strict profile needs).** The journal is item 1's local-sync
receipt applied to evidence: `append(record) -> LocalSync` returns `OnDisk` only after the record is fsynced,
`Failed` when durability was not established, `NotConfigured` when the profile has no journal. The **exporter** is
a separate cursor-based reader of the journal (the RA outbox shape, §6.7): it batches, signs and delivers, and it
never gates an effect. The existing `AuditSink::export(&record)` returns nothing, runs on a drain task and can drop
records when its bounded channel saturates — it is a mirror, and it **cannot** provide a durability barrier;
AE0 therefore names the journal, not the sink, as the ack-capable contract. Failure tests the seam must carry:
journal queue saturation ⇒ the strict profile refuses the effect (never drops the record silently); persistence
failure (`Failed`) ⇒ refuse; a lost acknowledgement (the append future is dropped or times out) ⇒ refuse and
record `DeliveryUnknown` for the evidence itself; the lenient profile logs and proceeds, and its evidence says so.

| Record | Establishes | Never implies |
|---|---|---|
| **requested** | the envelope was assembled for `(actor, operation, resource)` | that it was permitted or ran |
| **decided** | `permit` / `deny` / `indeterminate`, the checked constraints, `policy.revision` | that anything executed |
| **blocked** | the enforcement point refused the dispatch for this `attempt_id` — an explicit attestation, scoped to that point | that no other route reached the resource |
| **execution attempted** | dispatch happened (the receipt ladder's first rung: local application at the provider, or `DeliveryUnknown`) | completion |
| **execution completed / failed / unknown** | item 1's receipt for `operation_id` / `attempt_id` — reuse, not a parallel ledger | the intended business outcome |
| **outcome observed** | an *independent* observation of the effect, by a named observer | — acceptance remains a separate attributable decision (item 3) |

Consumers read `effect` as `completed` / `failed` only when the execution record says so; `unknown` on
`attempted` / `unknown`; `unstated` when neither is given. **A denial with no execution record does not mean
"no effect"** — a missing or delayed record is silence, and silence is never reassurance. `none` is read only from
an explicit **blocked attestation**: the enforcement point records, for that `attempt_id`, that it refused the
dispatch (`execution: not_dispatched`, scoped to *this* enforcement point — it says nothing about routes it does not
front). Without that attestation the execution state is `unknown` or `unobserved`. `deny` + `completed` is an
**enforcement gap** and is exported as such, never hidden. **Export failure must not create an unrecorded
effect**: the strict profile's boundary is *journal append acknowledged (`OnDisk`) before the effect proceeds, or
the effect is refused*; a lenient profile declares that it does not make this claim. (`not_dispatched` is proposed to
the evidence consumer as a contract-1.2 addition to its `execution` vocabulary — handover seam 2; until adopted,
the runtime exports `unknown` with the attestation carried in the native record behind `evidence_ref`.)

**Identity in evidence.** The logical agent id is the `subject`; a shared service principal is reported as such
(no `execution_identity`); the item 7 delegation chain is carried in the native record behind `evidence_ref`,
not in the consumer's contract.

## 6. The reviewed catalogue, the deployment report, and coverage

- **Catalogue identity = the consumer's.** The business-activity catalogue is a named, versioned, owned object
  in the evidence consumer (`activity_catalogues`). Mycelium's action catalogue **carries that identity**; every
  envelope and every observation names `mapping.catalogue` and `mapping.revision`. **No second identity** (the
  handover's seam 1). A mapping records its reviewer, revision, basis and coverage gaps; a correction exports the
  supersession link and retains the original. A reviewed mapping is not assumed infallible.
- **Generic tools.** `execute_shell`, `send_email` and their kin cannot establish business purpose by name. The
  catalogue either states what observation a catalogue action requires of them (process, file, network,
  protected-service record) or declares the tool **outside the enforcement guarantee**; the runtime emits
  `mapping.status: unmapped` rather than a guess. LLM suggestions retain attribution and uncertainty and cannot
  activate policy.
- **Declared, deployed, observed are three statements.** An approved declaration or a generated policy export is
  not activation. The runtime emits a **deployment report** binding declaration and mapping revisions, the exact
  policy digest, the enforcement points, activation/effective times, the issuer and the **route coverage**. A
  signed report is attributable testimony about activation, not proof that every alternative route is
  controlled; tests and observations qualify coverage. Missing, conflicting or stale reports are explicit.
- **Coverage is a field, not a hope.** `coverage.complete: false` names the routes the enforcement point does
  not see (AE-T: everything that does not traverse the gateway). No "all clear" is ever derived from an absence
  of records.

## 7. Strength profiles (declared per enforcement point; guardrails tier vocabulary)

| Profile | Where the decision is enforced | Strength | On `Indeterminate` / evaluator error | Claims it may make |
|---|---|---|---|---|
| **preflight** (AE-T, the gateway) | at one route, before dispatch | *SelfImposedPrevention* for the route; nothing for routes it does not front | refuse the dispatch | "refused at the gateway"; `coverage.complete: false` |
| **resource** (AE2) | inside the protected resource's effect boundary, with epoch/constraint check in the same transaction | *HardPrevention* at that resource | refuse the effect | "no effect occurred" for actions through that resource |
| **adapter** (item 1's external-effect adapter, no fence) | before an external API call | the adapter's contract only — **never labelled hard prevention** | refuse the call | the weaker guarantee item 1's adapter states |

The **secure profile** is the default at every enforcement point: it refuses when required authority cannot be
established. A lenient profile is opt-in, named, and its evidence carries its name.

## 8. The standards matrix (pinned subsets, not blanket conformance)

| Concern | Reference | What AE uses | What it does not claim |
|---|---|---|---|
| Identity | SPIFFE (spiffe-specs, latest) via item 7 | trust-domain, subject, delegation bindings for actors; verified workload identity as a *fact* | that identity confers authority |
| Decision / enforcement separation | XACML (OASIS) | the PEP/PDP/PIP/PAP separation as architecture | an XACML engine or XML on the wire |
| Policy language | **Cedar** (`cedar-policy`, in-process; D37) | one real adapter; policy digest = `policy.revision` | lossless translation to/from ODRL, XACML, Rego |
| Remit interchange | ODRL 2.2 | an ODRL-aligned profile for permissions, prohibitions, constraints, with completeness, precedence and unsupported semantics documented | blanket ODRL conformance; silent weakening — an export that would weaken the required boundary is **rejected** |
| Delegated requests | OAuth RAR (RFC 9396) | where the adopter's authorisation infrastructure supports it; otherwise the structured binding used is documented | a mandatory central service; a substitute for mandate establishment, identity verification or resource fencing |
| Lineage | PROV | may inform correction/supersession links | — |

Versions and supported subsets are pinned with **executable positive and negative fixtures** (§9); an
unrecognised clause is an error the decision carries, never an ignored condition.

## 9. Negative fixtures — the CI gate for the seam and for every evaluator

Each is a fixture the reference evaluator and the Cedar adapter must both pass; a replacement evaluator passes the
same set (AE4). Local CI is self-contained (a stub consumer, D31); no fixture, stub or single cloud closes a
scenario gate (§6.8's joint pack).

| Case | Envelope / facts | Required decision |
|---|---|---|
| substitution | the argument digest differs from the one the decision was bound to | `Deny` (bound decision does not apply) |
| impersonation | actor claimed by the caller (`params`, `_meta`, a `caller` block) instead of the auth layer | ignored; the decision is for the verified actor (item 7 negative case 1) |
| missing facts | a clause needs a mandate / budget / role the enforcement point could not establish | `Indeterminate`, secure profile refuses |
| unsupported clause | the policy uses a construct the adapter does not implement | `Indeterminate` with the error; never silently permitted |
| unmapped operation | the tool is not in the catalogue revision | `mapping.status: unmapped`; the policy cannot name it as a business operation |
| stale policy | the loaded artifact's digest ≠ the deployment report's | `Indeterminate` (policy not established) |
| expired validity | `now > not-after` on a cached decision | `Deny` |
| explicit prohibition | a prohibition clause matches | `Deny`, effect `none` |
| incomplete allow-list | allow-list present, operation absent, no prohibition | `Indeterminate` — *authority not established*, never `Deny`-as-drift |
| shared identity | actor is a shared service principal | permitted or denied as the policy says; the record carries no `execution_identity` |
| unobserved route | an effect that did not traverse the enforcement point | no record is invented; `coverage.complete: false` names the route |

## 10. What this record refuses

By decision, not omission (plan §9, D36): a universal policy engine or prompt guards as the authority boundary; a
mandatory fleet-wide decision point; a policy DSL of our own; a semantic intent detector; automatic policy
authoring; a propagation filter on Layer I; a `federation/`, `ra/` or AE prefix in gossip KV; any claim of
complete coverage without coverage evidence; and any conclusion drawn from an undeployed export, a stale
deployment report, an ambiguous mapping or an absence of records.

## 11. What lands next

- **The seam** — ✅ *shipped 2026-09-14* (`src/agent/action_evaluator.rs`): `ActionEvaluator` +
  `ActionEnvelope` + `Decision`, the hook between `gateway_auth` and the MCP `tools/call` dispatch, the
  deterministic `ReferenceEvaluator`, the §9 fixtures as tests, `Indeterminate ⇒ refuse`, and two
  live-gateway tests. Inert until an evaluator is attached. **Still to come on this line:** `/a2a` and the
  remaining gateway dispatch paths; **the node-local evidence journal** with its `append -> LocalSync`
  receipt, the safe reference record into the audit chain, and the three failure tests (saturation,
  persistence failure, lost acknowledgement); the deployment report that gives the enforcement point an
  expected `policy.revision` (until then the seam's stale-policy check has no second opinion to compare
  against and does not fire).
- **AE-T T2–T4** (private companion, on the seam): the Cedar adapter, the signed `AuditSink` exporter into the
  consumer's envelopes (Ed25519 over canonical JSON, ≤ 1000 records / ≤ 2 MiB, cursors, same `batch_id` ⇒
  byte-identical body), the procurement mapping subset; T-gate = scenario 2 locally.
- **Item 8** (threat model rev 2) is cited by AE1's PR; **item 5** supplies the mandate facts; **item 4** the
  budgets; **item 6** the replay of check/use races; **item 2** the domain-crossing variant.
