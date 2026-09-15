## [2026-09-14] ingest | the AE evaluator seam at the gateway

Up: [dev](../dev.md) §Planned AE · record `docs/design/action-envelope-ae0.md` §3, §9, §11 · plan §6.8,
queue §10.12.4 · code `src/agent/action_evaluator.rs`, the hook in `src/agent/http.rs`.

**Why now.** The queue put the seam directly after item 7, because the envelope's actor *is* item 7's
verified principal; with #208 merged the fact exists. AE-T's T2–T4 (the Cedar adapter, the signed
exporter, the procurement mapping) are private and build on this.

**What shipped.** The contract as code: `ActionEvaluator` (deterministic, replaceable, no clock and no
network — time enters as the envelope's validity), `ActionEnvelope` (`operation_id`/`attempt_id` from
item 1, the verified actor and granted scopes from item 7, operation, exact resource, `sha256` argument
digest, catalogue mapping, validity — assembled only by the enforcement point), `Decision`
(permit/deny/indeterminate with checked constraints, reason, policy revision, evaluation errors), and
`preflight`, which is where the seam refuses to trust an adapter: a permit carrying evaluation errors,
a permit over an unmapped or ambiguous operation, a decision from an unexpected policy revision, an
expired envelope and a panicking evaluator all refuse. `ReferenceEvaluator` is the executable meaning:
prohibitions win, then allowances, and anything uncovered is **authority not established**, never a
denial — so evidence never reads an uncovered action as drift.

**Decisions taken here.** (1) *Gated on `gateway` + `tls`.* The seam lives exactly where it is enforced;
elsewhere `preflight` would be dead code (the feature-gated dead-code trap). More importantly the
argument digest has **no non-cryptographic fallback**: a digest that does not bind is worse than an
absent one, because a decision would look bound while a substitution went undetected. (2) *Fail-closed
catalogue default:* `ActionEvaluator::mapping` defaults to `Unmapped`, which a permit cannot cover — an
evaluator that carries no reviewed catalogue says so, and its actions reach the consumer as unmapped
rather than as business operations. (3) *`operation_id` may come from the client* (`params._meta`): it is
a correlation identity, not authority, and item 1 wants it caller-minted and stable across retries;
everything that *is* authority still comes only from the auth layer. (4) *No expected policy revision
yet:* that comes from a deployment report (AE0 §6), so the stale-policy check exists and is unit-tested
but does not fire at the gateway until the report does.

**Caught while writing it.** Twice I inserted a new item between a `#[cfg]` attribute (or a doc block) and
the item it governs, silently moving the gate onto the wrong method — the same defect an external review
had just reported against WP5's accessor. Worth naming: inserting *above* an attribute block, never
between it and its item, is the rule; the `--no-default-features` build is what catches it.

**External review before merge (2026-09-15), four P2s and one architectural point, all taken.**
(1) *Unknown facts became a definite prohibition:* `requires_facts` was honoured for allowances only,
so a prohibition resting on an absent mandate returned `Deny` with no error. Dispatch was refused
either way — but the **evidence** claimed a prohibition had been established when it had not. Missing-
fact handling now applies to both rule kinds. (2) *A stale permit survived inside its refusal:* the
stale-policy branch kept the evaluator's verdict, so `refusal.decision().verdict` could read `Permit`
— a contradiction for a consumer that reads the decision rather than the control flow. The effective
verdict is now `Indeterminate`, with the superseded one preserved in `checked`. (3) *The panic
guarantee did not hold:* this crate's release profile is `panic = "abort"`, where `catch_unwind`
contains nothing, and `mapping()` ran outside the boundary besides. The trait now requires evaluators
not to panic and says exactly where containment applies; `evaluator_facts` puts the other two calls
behind the same boundary. (4) *Foreign adapters could not construct what the trait returns:*
`Decision` and `ActionMapping` are `#[non_exhaustive]` with no public constructor, so
`impl ActionEvaluator` in another crate failed with E0639 — the private Cedar adapter (AE-T T2)
could literally not have been written, and a foreign evaluator could only ever return
`Indeterminate`. Constructors and a builder were added, gated by a new **external-crate test**
(`tests/ae_external_adapter.rs`, run in CI) — `--lib` alone could never have caught this.
(5) *Architectural:* a digest establishes integrity but carries no meaning, so no adapter could
decide *amount ≤ 500* from the envelope. `ActionEvaluator::security_relevant_arguments` now declares
the names it needs and the gateway lifts exactly those into `selected_arguments`; a declared name the
request did not carry is *not established*, never a silent mismatch. Only declared names cross, so
the payload stays out of the evidence (AE0 §5).

**The lesson worth keeping.** Three of the four were the same shape: a rule applied to one branch and
not its mirror (allowance/prohibition), a verdict corrected in the control flow but not in the record,
and an attribute applied for a good reason (`#[non_exhaustive]`, learned from the WP5 review) without
asking *who constructs this type*. The external-crate test is the structural answer to the third.

**Strength, stated once and everywhere.** A route-level preflight: *SelfImposedPrevention* for the routes
this gateway fronts. Anything reaching a provider without traversing this gateway is outside the
guarantee, and the evidence must say so (`coverage.complete: false`). Resource-side enforcement inside
the effect boundary is AE2 and is a different, stronger claim.
