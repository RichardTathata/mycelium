> **Vendored external proposal — third-party, 2026-09-06. Body unmodified.**
> Our assessment, adopted elements and every divergence: [`../v3-contracts-axis.md`](../v3-contracts-axis.md) §6.7 and §7 (D29–D32).
> The companion document it links (`novuslens-ai-finops-implementation-plan.md`) is the consumer's plan and is **not** vendored; that link is expected to dangle here.
> Do not edit below this line; changes to our plan go in `v3-contracts-axis.md`.

---

# Mycelium V3 addition: attributable resource accounting and budget enforcement

Proposed implementation plan · 6 September 2026 · Not an adopted roadmap change.

Companion plan: [NovusLens AI FinOps](novuslens-ai-finops-implementation-plan.md). This document owns the proposed shared event contract, **Resource Accounting Contract 0.1 (RAC)**. The NovusLens plan consumes that contract; it does not define a competing format.

## 1. Decision and intended outcome

Extend V3 items 1 (contracts), 4 (adaptive stability), 6 (replay), and 7 (gateway identity) with attributable resource accounting. Reuse item 8's threat model and item 2's federation boundary. Keep this as one cross-cutting delivery slice, provisionally **RA**, rather than a new substrate mechanism or autonomous optimisation programme.

An application can identify which attempt consumed resources, for which logical operation, under whose authority, and with what remaining uncertainty. In the supported allocated-rights profile, an enforcing adapter reserves allowance durably before it dispatches work. NovusLens can consume this evidence without participating in execution.

Suggested roadmap paragraph:

> **RA — attributable resource accounting.** Compose operation receipts, gateway caller identity and item 4's allocated rights to reserve resources before dispatch, record every billable attempt, and reconcile observed usage without treating timeouts as zero consumption. Export versioned records through a durable application-layer feed. Detailed records remain outside gossip KV. Exact attempt bounds and provider-specific spending bounds carry separate promise strengths. Replay covers concurrent admission, restart, duplicate reports and late completion. Business acceptance and invoice reconciliation are application concerns.

## 2. Repository baseline and integration points

Read-only inspection: Mycelium HEAD `8f2498d447f10bd74497a56757ddef05c65fcc79`; `docs/plans/v3-contracts-axis.md` rev 1.5. This plan does not certify its release claims or modify repository files.

| Existing surface | Planned change |
|---|---|
| `mycelium-reason/src/route.rs` | Capture each provider attempt, including failed and failover attempts; preserve logical operation identity across attempts. |
| `mycelium-reason/src/trace.rs` | Link traces to accounting records; extend today's token count without pretending it is a complete billing record. |
| `mycelium-reason/src/http.rs` | Add a truthful structured usage representation. Current compatibility responses populate prompt/completion tokens with zero while returning a total; unknown splits must remain unknown in the new API. Preserve old public shapes through additive migration. |
| `mycelium-guardrails/src/policy.rs` | Compose existing tool-budget checks with item 4's admission interface. A tool count alone is not a fleet spending ceiling. |
| V3 item 1 | Reuse durable receipts, retained operation status and stable identities. Accounting evidence does not become a fifth business-effect guarantee. |
| V3 item 4 | Own the resource ledger and admission decisions; avoid a second budget authority. |
| V3 items 6 and 7 | Inject time/storage/faults; bind the initiating principal and acting gateway to every covered dispatch. |

Paths above are relative to `/Volumes/Scratch/Mycelium/`. Proposed new module names and interfaces below are design targets, not existing APIs.

## 3. Scope and promise profiles

**Observe:** export available usage. No blocking and no hard spending claim. Missing telemetry, export drops and uninstrumented paths are visible.

**Enforce local:** a durable local authority admits work against its assigned envelope. It is only a bound over dispatch paths it actually controls.

**Enforce allocated:** fixed, disjoint rights are assigned to independent authorities. All covered calls traverse enforcement; rights ownership survives restart; uncertain work retains its reservation. Map the exact assurance to the existing guardrail tiers instead of inventing a new strength scale.

Hard bounds apply to native units with enforceable maxima: attempts, capped tokens where the backend actually honours the cap, or a documented provider-specific maximum charge. A price estimate is not a monetary guarantee. If an adapter ignores an output limit, charges uncapped external tools, or cannot bound retries performed downstream, it cannot offer the corresponding strict profile.

No changes to unconditional forwarding, a global billing database, a policy DSL, dynamic rights transfer, automatic model selection, or automatic coordination redesign. A rejected work item remains a visible outcome. Cleanup has an explicitly reserved allowance within the total; it is not an unbounded exception.

## 4. Shared Resource Accounting Contract 0.1

Use a versioned JSON schema with language bindings and golden fixtures. Mycelium owns producer semantics; NovusLens owns its ingestion mapping and analytical projections. Both consume the same fixtures. Pin schema digests and external telemetry versions at implementation time.

### Identity envelope

Every event carries `schema_version`, `event_id`, `event_type`, `producer_id`, `producer_epoch`, `producer_sequence`, `occurred_at`, `recorded_at`, `domain_id`, and an authenticated export scope. The consumer maps the scope to its organisation server-side; it never trusts an arbitrary payload organisation identifier.

Correlation fields: `task_id`, optional `case_id`, `operation_id`, `attempt_id`, optional `parent_attempt_id`, `trace_id`, `reservation_id`, `allocation_id`, `policy_version`, `mandate_ref`, and `source_artifact_ref`. Parent links form a DAG and are causal references, not conclusions inferred from adjacent timestamps. Root tasks need no artificial parent.

`operation_id` identifies a logical action and survives retries. `attempt_id` identifies each potentially chargeable submission. `event_id` identifies one immutable assertion about it. A genuine resubmission receives a new attempt ID even when the business operation uses destination deduplication. Redelivery of an event retains its event ID. Same ID with different content is quarantined as a conflict.

Identity separates `origin_principal`, `acting_gateway`, and `provider_principal`. These reference verified identities, never copied client assertions. Optional business tags are claims until the customer approves their mapping to organisational cells. Do not export bearer tokens, prompts or raw reasoning by default.

### Event families

| Family | Required meaning |
|---|---|
| `reservation.created`, `.released`, `.settled` | Resource quantities, native units, authority, durable receipt reference, and release/settlement basis. |
| `attempt.dispatched`, `.completed`, `.failed`, `.outcome_unknown` | The dispatch and execution lifecycle. Request digest binds content without exposing it. A terminal response does not imply acceptance of the business outcome. |
| `usage.observed`, `usage.corrected` | Provider/request identity, meter, quantity, measurement interval, report basis and source reference. Correction explicitly supersedes a prior report. |
| `admission.rejected` | Requested allowance, applicable policy and rejection reason; no fabricated provider attempt or usage. |
| `export.coverage` | Covered adapters, instrumentation version, missing ranges, export freshness and known gaps. |

Usage meters include uncached input, cache-read input, cache-write input, output, provider-defined additional meters, and tool units when reported. Specify whether a meter is an exclusive category or a subset of another total. Never sum overlapping counts. Missing is null/unknown, not zero. Provider report semantics explicitly identify cumulative snapshots versus deltas; streaming partials must not be repeatedly added.

Money uses an integer amount plus explicit decimal scale and ISO currency, or a decimal string with defined precision; no binary floating-point arithmetic. Unit prices, rounding stage, effective dates and rate-card versions accompany estimates. Separate `estimated`, `provider_reported_charge`, and later application-owned `invoice_reconciled` amounts. They are alternative evidence of a charge, not three charges to add together.

Business acceptance is an external record linked by task/case and policy version. RAC allows its reference; Mycelium does not decide that a successful RPC satisfied a customer requirement. Resource settlement likewise does not wait for business acceptance.

## 5. Admission and reconciliation state machine

1. Resolve authenticated caller, task scope, policy and an adapter's capability to enforce requested limits.
2. Atomically reserve the vector of required resources. Reject the entire reservation if any required dimension is unavailable. Atomically write the reservation, attempt dispatch intent and durable export-outbox record before dispatch.
3. Dispatch through the enforcing adapter using the stable business operation ID and unique attempt ID.
4. On a definitive usage report, settle actual consumption within the reserved maximum and release the verified unused remainder. On incomplete reporting, preserve the unresolved portion.
5. On timeout, mark outcome unknown and retain exposure. No automatic retry with recycled rights. A new attempt needs new rights; a supported provider status API can resolve the old attempt.
6. On recovery, reconcile prepared/dispatched records with external status where possible. A crash between recorded intent and the network send cannot always reveal whether dispatch occurred. Preserve uncertainty instead of automatically reissuing.

For one resource dimension, maintain `available + outstanding_reservations + settled_consumption = assigned_rights`, with each unit in exactly one bucket. Settlement transfers between buckets; it never adds consumption while leaving the same amount reserved. Apply it per currency/native unit, not by mixing unlike quantities. Credits and allocation increases require separately authorized entries.

Two processes must not spend one allocation concurrently. The reference local ledger needs durable transactions and single-owner enforcement through the protected resource or exclusive process access. Copying a ledger to a second active machine is outside its promise. Restart and restore must not roll spending rights backward: restore invalidates old active credentials/ownership and re-establishes authority before admission. Never reclaim rights because discovery expired.

If actual cost exceeds a claimed enforceable maximum, record a contract violation and fail closed for new admissions in that profile. Preserve the real cost even when it breaks an invariant; accounting must not clip it to the budget.

## 6. Storage and export

Implement the ledger as item 4's application-owned store interface, initially with one transactional embedded backend chosen by its ADR. Reuse an existing backend when its atomicity and recovery contract match. Do not introduce a service process solely for accounting.

Ledger events and an outbox commit atomically. Export immutable batches over authenticated pull with opaque cursors and at-least-once delivery. A consumer acknowledges only after durable ingestion. Expose retention floors and explicit gap responses; a stale cursor cannot silently skip history. Recovery can serve an authorized snapshot plus subsequent events with a consistent cut.

Raw usage and bills remain outside gossip KV. If discovery needs a pointer, use an existing scoped service capability and reserve any new namespace through the current registry. OpenTelemetry spans/metrics are a derived export: sampled traces are not an authoritative ledger. Durable usage completeness still depends on the covered provider paths and reports.

A NovusLens outage must not stop already authorized local operations while ledger/outbox capacity remains. Strict mode rejects new admissions when it cannot durably preserve the required accounting; observe mode follows a documented loss policy and reports gaps. Cross-domain export uses item 2's explicit permissions. Denied evidence access does not become evidence of zero spend.

## 7. Implementation sequence and gates

| PR | Deliverable | Acceptance gate |
|---|---|---|
| RA1 | ADR, RAC schema, trust/coverage matrix, amount arithmetic, paired producer/consumer fixtures | Retry, duplicate, null usage, corrected cumulative report and conflicting-ID fixtures have unambiguous expected projections. |
| RA2 | Attempt instrumentation in reasoning and one reference external-call adapter; additive SDK fields | Every covered retry/failover has its own attempt. Failure with unknown usage stays unknown. No synthetic zero splits. Gateway negative identity tests cover accounting. |
| RA3 | Durable event outbox and authenticated cursor export | Crash after ingest-before-ack replays safely; retention gap is explicit; export outage preserves local evidence; secrets absent from fixtures and bundles. |
| RA4 | Item 4 admission/ledger integration; enforceable native attempt cap plus one documented charge-capable adapter | Concurrent branches cannot exceed allocated rights; failed attempts consume rights; unsupported hard-money profile is rejected. |
| RA5 | Replay/reconciliation, streaming corrections, clock and restore cases | All adverse scenarios below replay deterministically with invariants checked on every transition. |
| RA6 | Co-op gallery, SDK/operator parity, release pins and NovusLens end-to-end handoff | Two independently budgeted domains continue while NovusLens is disconnected; reconnect produces reconciled totals without double counting or membership merge. |

RA1–RA3 can proceed after item 1 identities/local durability and item 7 caller context are usable. Fold schema work into Phase A; deliver observation/export alongside Phase C, subject to those prerequisites. RA4–RA5 align with item 4 and item 6 in Phases D/E. RA6's federated variant additionally needs item 2. Early observations do not imply enforcement has shipped. Cross-reference this slice from phase contents, exit gates, §12 gallery/docs and release manifests; do not renumber existing items.

## 8. Decisive constructive scenario

Two co-op domains prepare catalog entries for member requests. Domain A holds seven attempt rights and B holds five; no transfers. Each attempt reserves a conservative provider-charge maximum independently of the attempt count. One worker retries after a known failed call; another times out with unknown charge. Two branches race for A's last right. Kill the ledger owner after reservation and restart it. Disconnect the evidence consumer, then deliver duplicated, out-of-order cumulative usage reports and a later correction.

Required results: at most twelve admitted attempts across the original allocation; no negative available rights; unknown work is not released; replay yields identical accounting; event redelivery adds no charge; legitimate retry costs remain separate. Estimated price is not shown as an invoice. NovusLens learns both accepted and unsuccessful task costs after catching up. A deliberately disabled reservation guard must cause the overspend test to fail.

Also test a stalled provider, partial cancellation, invalid origin scope, obsolete provider limits, outbox exhaustion, evidence deletion policy, unsupported schema, restore from stale backup, and failure-domain independence. Bounded dispatch does not claim eventual cancellation of all external activity.

## 9. Ownership, release and deferred work

Contracts maintainer owns RAC and compatibility; control maintainer owns enforcement; adapter maintainer owns provider-meter semantics and maximum-charge claims; replay maintainer owns fault coverage; NovusLens connector owner owns consumer conformance. Assign named people in RA1. Estimates follow that inventory, not a guessed calendar.

Ship observe mode first with covered adapters explicitly listed. Strict mode is opt-in and requires configuration validation, supported provider limits, durable ownership and a recovery runbook. Rollback disables new enforcement admissions safely; it does not delete reservations or erase past evidence. Keep legacy SDK contracts additive and publish migration examples in the same release. Retention, storage sizing, export access, backups and stale-backup handling belong in operator docs.

Defer invoice processing, exchange-rate policy, customer chargeback, business acceptance scoring, automatic model/topology changes, dynamic rights reallocation and any claim of complete fleet cost without coverage evidence. NovusLens supplies business interpretation; Mycelium supplies bounded execution and attributable records.

## 10. External alignment

Map observed usage to a pinned [OpenTelemetry GenAI convention](https://opentelemetry.io/docs/specs/semconv/gen-ai/), retaining provider-specific fields when no lossless mapping exists. [FOCUS](https://focus.finops.org/) supplies billing-normalization vocabulary for the consumer; RAC remains an operational event contract rather than a replacement billing standard. Both specifications evolve, so adapters state versions and conformance scope.
