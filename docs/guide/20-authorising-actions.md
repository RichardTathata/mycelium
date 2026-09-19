# 20 · Authorising Actions at the Gateway

Chapter 16 was about *what an agent may do* as a structural guardrail. This one is about the
narrower, harder question at the moment of dispatch: **this caller, this operation, this resource,
these arguments — yes or no, and on what record?**

It is grounded in
[`src/agent/action_evaluator.rs`](../../src/agent/action_evaluator.rs) and
[`src/agent/evidence_journal.rs`](../../src/agent/evidence_journal.rs). The design record is
[`docs/design/action-envelope-ae0.md`](../design/action-envelope-ae0.md).

Two questions this chapter answers:

- **"I have an existing tool fleet. Where does authorisation go?"** At the gateway, as a preflight
  before dispatch, with a policy engine you supply. You do not rewrite the fleet.
- **"What does that actually buy me, and what does it not?"** It buys enforcement *and* the record of
  why, on the routes this gateway fronts. It buys nothing at all on routes it does not front, and the
  chapter is blunt about that because the evidence is.

---

## The honest core: what a gateway can promise

A check at the gateway is **self-imposed prevention** for the routes that gateway fronts. It is not
hard prevention, which only a check inside the resource's own effect boundary earns. An agent that
reaches a tool without traversing this gateway is outside the guarantee.

That is not a caveat buried in a footnote. It is a field in the exported evidence:
`coverage.complete: false`, naming the routes the enforcement point does not see. The design record
is self-critical about the limits of even that, in a line worth keeping in mind while you read the
rest of this chapter:

> the evidence went on saying `coverage.complete: false` — truthfully, and uselessly.

**Coverage is a field, not a hope.** A reader must never infer an all-clear from silence.

---

## Three verdicts, and the one people collapse

```rust
pub enum Verdict { Permit, Deny, Indeterminate }
```

- **`Deny`** — the policy establishes that this action is refused.
- **`Indeterminate`** — authority could **not be established**. Nothing says the action is forbidden;
  only that nothing says it is allowed.

**`Indeterminate` is never `Permit`.** If a policy declares it needs an argument and the request does
not carry it, the answer is `Indeterminate` — never a guess. A `Decision` that carries evaluation
errors alongside a `Permit` verdict is a contradiction, and the seam resolves it to `Indeterminate`.

The distinction is operational, not philosophical. A deny sends you to the policy. An indeterminate
sends you to the inputs.

---

## The envelope is assembled by the enforcement point

```rust
pub struct ActionEnvelope {
    operation_id, attempt_id,        // chapter 18's identities
    actor: String,                   // the VERIFIED principal — never a client-supplied string
    via: NodeId,
    scopes: Vec<String>,             // the credential's scopes ∩ the route's requirement
    operation, resource,
    arguments_digest: [u8; 32],
    selected_arguments: Map<..>,     // only DECLARED names
    mapping: ActionMapping,
    expected_policy_revision: Option<String>,
    issued_at_ms, not_after_ms,
}
```

Nothing here is taken on the caller's word. `actor` is chapter 09's verified principal, `via` is the
signature-verified sender, and `scopes` is an intersection rather than a claim.

**Why the arguments are split in two.** A digest establishes integrity but carries no meaning: no
policy can decide *"quantity is within the depot's limit"* or *"the destination is an approved
partner"* from a hash. So declared argument names cross in the clear, and only declared ones. A name
the policy declared but the request did not carry is **absent**, and absence must produce
`Indeterminate`.

Defaults are fail-closed: no scopes, a zero digest, an `unmapped` mapping.

---

## Wiring it up

Three attachments, and the second one is the one people forget:

```rust
// 1. The policy engine. Yours, or the in-tree reference.
let policy = ReferenceEvaluator::new("2026-09-19.1")
    .with_catalogue("depot-operations", "7")
    .map_action("dispatch", "surplus-food/collection")
    .allow(
        Rule::new("oidc:coop-idp/depot-scheduler", "dispatch", "surplus-food/collection")
            .requiring_scopes(["depot:dispatch"])
            .requiring_value("destination", json!("partner-kitchen-3")),
    );
agent.with_action_evaluator(Arc::new(policy));

// 2. The evidence journal. Node-local, fsynced, NEVER gossiped.
let journal = EvidenceJournal::open("/var/lib/mycelium/evidence", EvidenceProfile::Strict)?;
agent.with_evidence_journal(journal);

// 3. The revision you actually deployed.
agent.set_deployed_policy_revision("2026-09-19.1");
```

**A node that attaches an evaluator but no journal enforces and records nothing**, and warns at
attach time. Enforcement without attribution is not governance, it is an unlogged gate.

**Until the deployed revision is set, the stale-policy check has nothing to compare against and
simply does not fire** — so a gateway running a superseded policy is undetectable, which is precisely
the condition the check exists for. Set it to the same string your deployment report carries.

### The two evidence profiles

| Profile | When evidence cannot be recorded |
|---|---|
| `Strict` | the dispatch is **refused**. Enforcement and attribution stand or fall together |
| `Lenient` | the dispatch proceeds, and the evidence says it was produced under a profile that does not gate |

`Lenient` is weaker, and the point of naming it in the evidence is that it is **legible as weaker**.

---

## Three refusals, and the one that refuses a permit

```rust
pub enum PreflightRefusal {
    Denied(Decision),         // "action_denied",             -32030
    NotEstablished(Decision), // "authority_not_established",  -32031
    NotRecorded(Decision),    // "evidence_not_recorded",      -32032
}
```

`NotRecorded` is the one to understand. The action is refused **even when the policy permitted it**,
because the decision's evidence could not be established. Refusing is the honest failure mode there,
and it is visible rather than silent.

The codes are stable across the tool-invocation and agent-to-agent surfaces, so a client can
distinguish the three without parsing prose.

---

## Replacing the evaluator

The trait is small on purpose:

```rust
pub trait ActionEvaluator: Send + Sync + 'static {
    fn evaluate(&self, envelope: &ActionEnvelope) -> Decision;
    fn security_relevant_arguments(&self) -> Vec<String> { vec![] }
    fn mapping(&self, operation: &str, resource: &str) -> ActionMapping { /* unmapped */ }
}
```

Two obligations that are easy to miss:

- **Implementations must be deterministic over the envelope.** No clock, no network, no ambient
  state. That is what makes chapter 19's replay and later evidence re-checking possible at all.
- **The default mapping is fail-closed** (`Unmapped`), and a permit cannot cover an unmapped
  operation. An operation outside your reviewed catalogue is never quietly treated as a business
  operation because its name looked familiar.

A note the code itself records: `Decision` needed a public constructor before a foreign evaluator
could return anything but `indeterminate`. The replaceable-evaluator premise depended on a
constructor that did not exist until a review found it.

---

## What this does not establish

- **Not hard prevention.** Only a check inside the resource's own effect boundary, with the epoch
  check in the same transaction, earns that. See chapter 18's note on the one place the substrate
  prevents, and [`design/scoped-mandates.md`](../design/scoped-mandates.md) for the boundary argument.
- **Not coverage of routes this gateway does not front.** The evidence names them rather than
  implying an all-clear.
- **Not a claim about arguments the policy did not declare.** They never crossed, so nothing was
  checked about them.
- **Not protection from a panicking evaluator in a release build.** The seam contains a panic only in
  an unwinding build, and the shipped profile aborts. An evaluator must not panic.

---

## Where to go next

| You want | Read |
|---|---|
| the full decision record: envelope, verdicts, evidence, catalogue identity, strength profiles | [`design/action-envelope-ae0.md`](../design/action-envelope-ae0.md) |
| who the caller is, before any of this runs | [09 · Security](09-security.md), [`operations/rbac.md`](../operations/rbac.md) |
| the three strength tiers this chapter's promise sits in | [16 · Guardrails](16-guardrails.md) |
| the identities the envelope carries | [18 · Contracts & receipts](18-contracts-and-receipts.md) |
| why a deterministic evaluator matters | [19 · Replay & simulation](19-replay-and-simulation.md) |
