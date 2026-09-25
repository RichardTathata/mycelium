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
    mandate: Option<MandateBinding>, // chapter 21's appointment, and what was established
}
```

Nothing here is taken on the caller's word. `actor` is chapter 09's verified principal, `via` is the
signature-verified sender, and `scopes` is an intersection rather than a claim.

**Why the arguments are split in two.** A digest establishes integrity but carries no meaning: no
policy can decide *"quantity is within the depot's limit"* or *"the destination is an approved
partner"* from a hash. So declared argument names cross in the clear, and only declared ones. A name
the policy declared but the request did not carry is **absent**, and absence must produce
`Indeterminate`.

Defaults are fail-closed: no scopes, a zero digest, an `unmapped` mapping, no mandate.

## The mandate a decision rests on

A rule can require that the caller be acting under a live [mandate](21-mandates.md):

```rust
Rule::new("oidc:idp/dispatcher", "tools/call", "tool:reroute@depot")
    .requiring_mandate("depot-ops")
```

The envelope carries *which* appointment — holder, `term`, `scope`, `epoch` — and what the
enforcement point **established** about it, which is three answers and not two:

| `MandateState` | Verdict | Reading |
|---|---|---|
| `Established` | the rule can permit | checked against the resource's installed epoch |
| `Refused(MandateRefusal)` | **`Deny`**, by name | checked, and the fence said no |
| `Unknown(why)` | `Indeterminate` | the fence could not be consulted |

`None` — the action claims no mandate at all — is also `Indeterminate` against a rule that needs
one. *We did not find out* is not *we looked and it is bad*, and an operator told the wrong one
fixes the wrong thing.

**A refused mandate denies before the policy runs.** It is a boundary the policy cannot open, not a
fact the policy weighs. If the allow-list ran first, a rule that legitimately permits this call
every other day would turn a refusal into a permit — and for
[`Superseded`](21-mandates.md) that is the same laundering chapter 21 describes when it explains why
a superseded mandate is not a `Conflict`. A retry loop must not be able to launder a revocation, and
neither must a policy engine.

**The gateway binds no mandate.** It is a route-level preflight and holds no fence to consult, so it
passes `None` and a mandated rule reads that as authority not established. Enforcement at the
resource, where a fence exists, is a later and stronger claim than this chapter makes.

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

## Which tools an agent may actually call

The MCP path is where this is most often wanted, because a scope is such a coarse answer there.
`POST /mcp` requires `mcp:invoke` — binary. An agent granted *"can use tools"* can use **every**
tool the fleet advertises.

The preflight sits in `mcp_handler`, between the auth layer and the dispatch, and decides per call:

```bash
cargo run --example mcp_tool_authority --features tls,compliance
```

Five acts against a real gateway, one agent, three real tools and one purchasing remit:

| # | Call | Verdict |
|---|---|---|
| 1 | `purchase.place`, £120 | **permitted** — dispatched, the tool ran, it answered |
| 2 | `purchase.place`, £900 | **denied** — over the ceiling, decided on an argument *value* |
| 3 | `ledger.export` | **denied** — outside the remit; a standing answer a retry will not change |
| 4 | `weather.lookup` | **indeterminate → refused** — no clause covers it |
| 5 | `purchase.place`, no amount | **indeterminate → refused** — a rule that cannot be applied |

Act 4 is the one to read twice. The policy has *no opinion* about `weather.lookup`, so authority was
never established — and the gateway **refuses** rather than falling through to allow because nothing
said no. The evidence records *could not decide*, not *denied*. Those are different facts, and an
engine that conflates them lies in both directions: reporting a gap as a prohibition, or admitting a
call nobody authorised.

**The evaluator sees only what it declares.** The envelope carries a **digest** of the whole argument
set plus the values named in `security_relevant_arguments` — here just `amount_pence`, because a
ceiling is not something a digest can check. Attaching a policy is therefore not a licence to read
every payload crossing the gateway; declaring less is seeing less.

**And it remains a route-level preflight.** It governs what *this gateway* dispatches. A provider
reached another way is not covered — which is why [`procurement_authority`](../../examples/coop/README.md)
spends an act on a misconfigured route that denied a call **and it happened anyway**, with the
evidence saying both.

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

### What a refusal tells the caller

Both surfaces send the same machine-readable `data` block. They did not always: `/mcp` carried it and
`/a2a` sent only the code and an English sentence, so the path a **partner domain** calls across the
federation edge was the poorer one, and a client written against one broke on the other. Both now
call one function, which is the only way two doors stay described the same.

```json
{ "jsonrpc": "2.0", "id": 1,
  "error": { "code": -32031,
             "message": "authority not established: ...",
             "data": { "reason": "authority_not_established",
                       "policy_revision": "procurement-2026-q3.r7",
                       "checked": ["allowance actor=... operation=..."],
                       "errors": ["fact not established: mandate"] } } }
```

**`policy_revision` is the field to branch on**, and the reason it is worth carrying: without it a
caller cannot tell a *stale policy* refusal — this enforcement point expected a different artifact —
from a real denial. Those call for opposite responses: redeploy and retry, versus stop.

### In the SDKs

Both SDKs raise a typed refusal rather than a generic error, because *"the call failed"* is precisely
the collapse this chapter is about. Python raised a bare `KeyError` with the code inside the message
until this landed; TypeScript threw a plain `Error`.

```python
from mycelium.a2a import A2aClient, ActionRefusedError

try:
    reply = client.send("procurement/purchase", "raise a PO for 12 pallets")
except ActionRefusedError as refused:
    if refused.denied:
        ...                       # an authority said no; do not retry
    elif refused.authority_not_established:
        ...                       # nobody decided — NOT a violation to report
    elif refused.evidence_not_recorded:
        ...                       # permitted, but unrecordable; retry when recording is healthy
    print(refused.reason, refused.policy_revision, refused.checked)
```

```typescript
import { A2aClient, ActionRefusedError } from "@mycelium/client";

try {
  await client.send("procurement/purchase", "raise a PO for 12 pallets");
} catch (e) {
  if (e instanceof ActionRefusedError) {
    if (e.authorityNotEstablished) { /* nobody decided — not drift */ }
    console.log(e.reason, e.policyRevision, e.checked);
  }
}
```

The middle branch is the one worth writing out. **An action no rule covers is not a violation.** A
dashboard that counts `authority_not_established` as a policy breach reports drift that never
happened — and the operator who trusts it will go looking for an attacker who does not exist.

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

### Enforcing somewhere else: call `preflight`, not `evaluate`

Everything above happens behind this gateway. If you are enforcing at your **own** boundary — a
resource that decides for itself rather than trusting a preflight at one route — use
`mycelium::preflight`, not `ActionEvaluator::evaluate`.

```rust
use mycelium::{preflight, PreflightRefusal};

match preflight(Some(&evaluator), &envelope, now_ms) {
    Ok(Some(decision)) => { /* admitted — and record `decision` */ }
    Ok(None)           => { /* no evaluator attached; the seam is inert */ }
    Err(refusal)       => { /* refused — `refusal.reason()` says which of the three */ }
}
```

The difference is not stylistic. An evaluator asked directly answers only the policy question, and
**five checks are the seam's, not its**:

| The seam checks | An evaluator called directly would |
|---|---|
| an expired envelope is denied *first* | permit it — it holds no clock, and that is not its job |
| a decision from an unexpected `policy_revision` is stale | act on a policy you never deployed |
| a `Permit` carrying evaluation errors is downgraded | trust a permit its own adapter could not fully evaluate |
| an unmapped or ambiguous operation cannot be permitted | name an unreviewed action as a business operation |
| a panicking adapter is caught | turn an adapter bug into an admission |

In each of those cases a correct evaluator returns **permit** and the seam refuses. It is not being
overridden; it was never asked that question. Skipping the seam is the *every leg correct, the
composition wrong* failure, one layer out — and it is silent, because the permits look right.

`preflight` does **not** record anything. An admitted action still needs its evidence written, and
an enforcement point that decides without recording is an unlogged gate; that is what
`PreflightRefusal::NotRecorded` exists to refuse when the recording fails.

### Checking your evaluator against the contract

Writing an evaluator is the easy half. The hard half is knowing it agrees with the seam about the
three verdicts — and in particular that it never reports *nobody decided* as *someone said no*.

`mycelium::ae_contract` is that check, and it is public for exactly this reason: fixtures an adopter
cannot run hold nobody to anything.

```rust
use mycelium::ae_contract::{self, EvaluatorUnderTest, PolicyIntent, PolicyClause, Unexpressible};
use mycelium::{ActionEvaluator};
use std::sync::Arc;

struct MyEvaluator;               // your ActionEvaluator lives here

struct MyEvaluatorUnderTest;

impl EvaluatorUnderTest for MyEvaluatorUnderTest {
    fn name(&self) -> &str { "MyEvaluator" }

    // Translate the neutral policy statement into whatever your evaluator holds.
    fn build(&self, intent: &PolicyIntent) -> Result<Arc<dyn ActionEvaluator>, Unexpressible> {
        // ... build from intent.clauses, reporting intent.revision as policy_revision ...
        Ok(Arc::new(MyEvaluator))
    }
}

#[test]
fn my_evaluator_meets_the_contract() {
    let report = ae_contract::run(&MyEvaluatorUnderTest);
    assert!(report.conformant(), "{}: {:#?}", report.summary(), report.failed);
}
```

Three things worth knowing before you run it:

- **It judges outcomes, not wording.** Where a case requires your refusal to report what it could
  not establish, it checks that the *identifier* appears — the fact's name, the clause's name, the
  revision. Your evaluator may say it however it likes.
- **Declining is not passing.** If your evaluator genuinely cannot express a clause, return
  `Unexpressible` and list the case id in `declared_limits`. `conformant()` requires the declined
  set to equal the declared set *exactly*: an undeclared decline is a silent gap, and a declaration
  you have since outgrown tells an operator you have a weakness you do not have.
- **It is not all of AE0 §9.** Nine of its eleven rows are gated; `ae_contract::COVERAGE` says which
  are not and why. Two of them are not properties of a decision at all, so no evaluator fixture can
  speak to them.

The same suite is what holds the shipped `ReferenceEvaluator` and a deliberately unrelated
replacement to the same bar, which is the only reason "replaceable" is a claim rather than a hope.

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
