# 20 · Authorising Actions at Gateway, Provider and Resource

Chapter 16 was about *what an agent may do* as a structural guardrail. This one is about the
narrower, harder question at the moment of dispatch: **this caller, this operation, this resource,
these arguments — yes or no, and on what record?**

It is grounded in
[`src/agent/action_evaluator.rs`](../../src/agent/action_evaluator.rs) and
[`src/agent/evidence_journal.rs`](../../src/agent/evidence_journal.rs). The design record is
[`docs/design/action-envelope-ae0.md`](../design/action-envelope-ae0.md).

The gateway is the starting point. Provider admission and continuation, then the
resource's commit boundary, have separate checks and evidence. Follow the
[authority tutorial](tutorials/04-authority-boundaries.md) to run the complete
teaching sequence; a gateway permit alone does not authorise a destination effect.

Two questions the gateway portion answers:

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

**The gateway establishes a *presented* mandate — when an authority is attached.** Since 2.15.0 a
caller may present a signed grant in `params._meta.mandate` (`/mcp`) or the A2A message metadata;
with `with_execution_authority` attached the gateway verifies the grant, checks revocation freshness
and binds the result into the envelope, so `Rule::requiring_mandate` reads *established* rather than
*not established*. With no authority attached the envelope carries `None`, as before. The mandate
also **travels to the provider**, carried not verified, so the provider can decide again where the
work runs — chapter [21](21-mandates.md) § *Presenting and enforcing a mandate*.

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

**And it is a route-level preflight — with the other doors now closed (2.15.0).** It governs what
*this gateway* dispatches; the raw routes refuse protected kinds (`403 protected_kind`), and a
provider that runs `with_provider_enforcement()` decides again where the work runs, so a member
reaching a provider directly is refused as the gateway would refuse it. What a *misconfigured* route
still shows is the evidence rule, which is why [`procurement_authority`](../../examples/coop/README.md)
spends an act on a misconfigured route that denied a call **and it happened anyway**, with the
evidence saying both.

## The other door, and why it is the one that matters

`/mcp` is where your own agent code calls in, and it is gated: a bearer carrying `mcp:invoke` is
required before the evaluator is ever consulted. **`/a2a` is not.** An A2A client discovers
`/.well-known/agent-card.json` and calls; the gateway resolves a bearer if one is offered and calls the
caller `anonymous` if not, but it does **not refuse for want of one**. The route is public by design
— that is what makes it an interoperable A2A endpoint.

So at that door the per-call decision is the *whole* of the gate, and the consequence is worth
stating plainly: **an enforcement point that can be walked around by choosing a different door is
not an enforcement point.** A remit enforced on `tools/call` and not on `skill.invoke` is a remit an
agent escapes by speaking A2A instead of MCP — and the evidence journal would have gone on recording
true sentences about the guarded door while the remit went unenforced at the other one.

The preflight therefore runs on **both** A2A paths, `tasks/send` and the `tasks/sendSubscribe`
stream, with the same refusal body `/mcp` sends:

```bash
cargo run --example a2a_skill_authority --features tls,a2a
```

| # | Call | Verdict |
|---|---|---|
| 0 | `tools/list` and `tasks/send`, neither with a bearer | `/mcp` → **401**; `/a2a` → answered, caller `anonymous` |
| 1 | `depot/dispatch`, in-area | **permitted** — the skill ran, and the remit names `anonymous` explicitly |
| 2 | `depot/dispatch`, out-of-area | **denied** — decided on the message text |
| 3 | `ledger/export` | **denied** — `-32030`, with `data.reason` and `data.policy_revision` |
| 4 | `weather/lookup` | **indeterminate → refused** — advertised, covered by no clause |
| 5 | `depot/teleport` | **`-32001` skill not found** — refused *before* the evaluator ran |
| 6 | `ledger/export` over `tasks/sendSubscribe` | **denied** — arrives as a `failed` task-status event, and the stream closes |

Act 1 is the one that changes how you write policy here. Permitting `anonymous` **by name** is a
decision; permitting it because no clause mentioned it is an accident, and act 4 is what the second
one would have looked like had the profile not refused.

Act 5 is in the demonstration on purpose. Skill resolution precedes the evaluator, so an
unadvertised skill is refused as *not found* and **no evidence record claims a policy decided it** —
a refusal that is a fact about discovery, not about authority. A gallery in which every refusal
looks alike teaches the wrong thing.

Act 6 is the failure mode a streaming edge invites. A refusal that simply dropped the subscription
would leave the client waiting on silence, which is indistinguishable from a slow skill; instead it
learns exactly what a unary caller learns.

**What this does not cover:** authentication. Every act runs as `anonymous`, because that is what the
door hands the evaluator by default. `anonymous` is a *principal*, not a bypass. An operator wanting
named principals here configures `gateway_named_tokens` and the client presents a bearer — nothing
in the seam changes, and [`gateway_caller`](09-security.md) is where that half is described.

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

## Deciding on what an actor has already done

A rule such as *at most three posts in ten minutes* needs more than the request: it needs the actor's history. An
evaluator that asks for it (`history_request`: how far back) is given it beside the envelope
(`evaluate_with_history`), and the reference evaluator expresses the limit on a rule:

```rust
use mycelium::{ReferenceEvaluator, Rule};

let policy = ReferenceEvaluator::new("rev-posts")
    .with_catalogue("cat-posts", "1")
    .map_action("tools/call", "tool:post@10.0.0.5:7946")
    .allow(Rule::new("oidc:idp/alice", "tools/call", "tool:post@10.0.0.5:7946").at_most(3, 600_000));
```

**Whose history.** The enforcement point's, at this node: the decisions its evidence journal recorded about this actor
(`HistoryScope::EnforcementPoint`). The gateway's doors — `/mcp`, `/a2a`, the LLM and federation routes — count
together (`point=gateway`); the provider check counts apart (`point=provider`), so a call the gateway decided and the
provider re-checks on one node is counted once at each. The journal is node-local and never gossiped, so the count is
exact *here* and needs no quorum — and an actor that spreads its calls over three gateways can be admitted up to three
times the limit. Every history decision names the scope and the point (`history scope=enforcement_point point=gateway
… counted=2 limit=3`), and so does the evidence.

**What the history is.** Read from the journal in the background, once an evaluator that asks for history is attached
(a node whose evaluator never asks never reads it), so a restart keeps it; until the read completes a limited call is
not established. Kept current by every decision recorded after. Bounded: each actor's records within 24 hours, at most
1 024 of them, at most 65 536 actors per point (past it the oldest sixteenth are evicted, and an actor that then
appears at that point is not established over windows reaching back past what was evicted — so under a sustained churn
of new principals at one point, its quiet and new actors are refused, closed, until the churn stops) — so a limit above 1 024 a
day is not expressible, and **a rule whose window is longer than 24 hours never permits**. After a crash, the
journal's torn tail is repaired at open, and every limited rule is not established for one window, since what was lost
is not known. A rule is judged on its
own window: a view says the newest record it does not hold (`omitted_through_ms`), and a count *below* the limit over a
window reaching back past it is not established. Records lost to a journal repair at open, or to an append a lenient
journal failed to make, count as not held. A journal that cannot be read, or none at all, gives `History::Unavailable` —
and a limited rule decides `Indeterminate` on it, never as though the actor had done nothing. `at_most(n, 0)` counts
nothing and is not established; `at_most(0, w)` is an explicit never.

**Check, then count.** The gateway decides one actor's history-limited calls one after another, reading the clock once
it is that call's turn, and counts a permit into the history as it is decided, so ten concurrent calls under *at most
three* admit three. A permit whose evidence is then not recorded is taken back out. It errs towards counting: a permit
whose client went away before its evidence was written, or that a later check downstream refused, stays counted.

**On the record.** The decision's evidence carries the history it read — the scope, the window, the attempts it was
given by id, and whether that was all of them. That is what a replay would decide again from; no replay of gateway
decisions consumes it yet (`ae.preflight` is not in the decision trace), and an attempt it names may be one whose own
evidence was then not written.

**Across the fleet (row K2).** A rule may count across the fleet instead, naming one of two scopes and a **counter
id** — the policy chooses per rule, and the per-point count stays the default:

```rust
use mycelium::{HistoryScope, ReferenceEvaluator, Rule};

let posts = |scope| Rule::new("oidc:idp/alice", "tools/call", "tool:post")
    .at_most(3, 600_000)
    .scoped(scope, "c-7f3a"); // an opaque counter id: what other members see, never the rule's operation or resource
let policy = ReferenceEvaluator::new("rev-posts")
    .allow(posts(HistoryScope::FleetConsensus)); // or HistoryScope::FleetTally
```

| | `FleetConsensus` — exact | `FleetTally` — approximate |
|---|---|---|
| What is counted | three *seats*, each a consensus slot held a window from its permit's admission, then released by its holder; leased for the window plus a margin of about 19 s (the longest a claim is allowed to take) in case the holder crashes | the permit times every enforcement point publishes for this actor and counter |
| When a call is admitted | after its permit **claims a seat** by a consensus write whose lease still covers a whole window from admission; every seat held → `Deny` | when the fleet's sum, as this node has heard it, is below the limit |
| Without a quorum | the claim cannot be decided → `Indeterminate` (closed) | available — no quorum is needed |
| Guarantee | the fleet admits at most three in any window, whichever gateways the calls reach — to within the nodes' wall-clock skew | **detection, not prevention**: two gateways can each admit before either's tally reaches the other |
| Cost | a consensus round per limited call (more while seats are contended) | a gossiped write per permit, and every node's heartbeat |
| Actor | must be named — the reference evaluator does not establish `*` (see *Growth*) | any, `*` included |
| Limit and window | at most 1 024 seats, a window of 1 ms to 24 h — outside them, not established (see *Bounds*) | the same |

The evaluator names the counter its deciding rule relies on (`fleet_counters`, a third method with a default), and the
enforcement point reads its state into the view (`HistoryView::fleet`) before the evaluator decides: for the exact
scope, the seats this node sees held — a full set is a decided denial, anything less a hint, because the seat claimed
afterwards is what admits the call; for the tally, the sum over this node and every enforcing peer, and who they were.
A count at another scope never satisfies a fleet rule. As at one node, every gateway's doors count together and the
provider check apart: a call a gateway admitted and its provider re-checks holds a seat (or a tally entry) in each
count, never two in one. Both scopes need the evidence journal, as the per-point count does: the fleet count travels in
the history view, and without a journal every view is unavailable.

*Claiming without holding anyone up.* The gateway decides one actor's history-limited calls one after another under a
striped per-actor gate — but it releases the gate **before** claiming a seat, counting the permit locally first and
taking it back if the claim fails; consensus is what orders the seat's contenders. So a claim waiting out a lost
quorum (up to 2 s per phase × 4 ballots per seat tried, plus a second's convergence poll) holds up no other call — not
another actor on the same stripe, and not a call no fleet-consensus rule covers. At a provider, the check decides
from the envelope it builds — the verified caller's scopes, the derived arguments, the presented mandate — and, when
its permit must claim a seat, hands the claim to run beside the serve loop (the MCP tool loop and `rpc_rx`); every
other check stays inline, in arrival order. At most 64 claims wait beside a loop: with that many, the loop stops
draining its channel, so the channel's bound pushes back on senders again. A claim that panics is answered
`authority_not_established`, and one still waiting when its loop ends (the tool withdrawn, the receiver dropped) is
answered the same way — no caller is left to time out. Handlers still run one at a time; a claimed call is yielded
when its claim completes, and holds its admission (a cohort-budget slot, if one is attached) from then until its
handler returns. No built-in protected kind's consumer depends on arrival order across callers (`mcp.invoke`,
`skill.invoke`, `llm.invoke` each answer per request); an operator-listed kind that does should not be put under an
exact fleet limit.

*A seat admits once a window.* A seat's lease is fixed when the claim is proposed, and the call is admitted after the
round. So the lease is the window **plus a margin, the longest a claim is allowed to take**: 19 225 ms. That covers
four ballots that each wait out both 2 s phases, the convergence poll, and a 2 s allowance for the claim's local work.
That allowance is not a timeout; nothing stops local work that runs past it. A seat whose remaining lease is less than
a whole window at admission is refused (`Indeterminate`, the seat held until its lease ends). The same is checked on
the monotonic clock, so a backward wall-clock step cannot admit a slow claim. Otherwise a slow claim's seat could
reopen, and admit again, within one window.

*A seat is released a window after its permit is admitted.* The holder writes the decision's release marker one window
after admission, timed on its own monotonic clock. A node reads the seat free either when that marker reaches it or
when the lease lapses on its own wall clock. The release path needs no clock agreement. The lease path absorbs skew of
up to about the margin (19 s). So the bound holds, as before, to within the nodes' wall-clock skew. A saturating actor
gets close to its limit every window: at a one-second window, about one admission a second per seat, plus the claim's
own round. The lease ends the seat only if the holder crashes or shuts down first; a used seat then stays held for the
window plus the margin. A seat is held for its whole window even when its dispatch fails. A permit whose evidence is
not recorded schedules no release, so its seat is held for the whole lease. Both err towards admitting fewer.

*Not claimed:* a seat's release marker is not checked against its owner. Any mesh member can write one, as for a lock,
because consensus here tolerates crashes, not lies (row A).

*Coverage, strictly.* Every node built with the `gateway` **and** `tls` features publishes a heartbeat from
`start()`, saying whether it enforces (has an evaluator), re-asserted every third of `control_max_staleness_ms`. At a
graceful shutdown its last word is a heartbeat marked **departed**, and its entries are kept: peers go on counting
its recent permits until they age out, and do not wait for it. A tally
is not established — `Indeterminate`, never an empty tally — when this node's own view of the fleet is not (it has
heard from fewer than `control_min_peers_heard` peers, or the freshest is older than the bound: the governors'
confidence bound, reused), or when any peer's coverage is unknown: no heartbeat, a stale one (not departed), one dated
more than `max_clock_drift_ms` in this node's future, or an entry this node cannot read; the refusal names the peer.
A peer that enforces nothing admits nothing under a tally rule and is not counted. The departed heartbeat travels on
shutdown's best-effort flush; a node whose last word is lost, or that crashes rather than leaving, reads as stale —
closed, not open — until failure detection declares it dead; from then its entries still count, as a departed node's
do, until they age out — but only for a node this node can verify: an identity record it trusts (`trusted_keys`) and
not removed. A heartbeat and entries written under a node id nobody issued an identity for count for nothing. A
non-peer whose heartbeat is older than the counter's window plus the staleness bound plus the clock-drift bound is not
read at all, and a decision lists the nodes it knows of from the store's scope index — its cost grows with nodes, never
with the entries they hold. **Residue is collected at its own stamp.** Once a non-peer's heartbeat is older than the
longest window among its entries plus both bounds, any node tombstones its keys *at the timestamp of the record it
holds*: under last-writer-wins that removes that record and loses to anything the owner wrote after it, so it cannot
erase a returned node's newer heartbeat or permit, wherever it is. A returning node re-asserts its own heartbeat and
withdraws its own aged entries locally before its connections come up, so a collector's tombstone reaches it older than
what it holds, is not applied, and its `sys/` tripwire does not count it.

*What "verified member" rests on.* With `[tls]` and `require_identity_proofs` (the default), a member is one whose
sealed identity record or certificate this node holds and has not seen revoked or removed. **Without `[tls]`** this
node can verify nobody, so a non-peer inside the horizon is unknown coverage: the tally decides `Indeterminate`, naming
it, until its heartbeat passes the horizon — closed, never an under-count of a crashed node's permits. **With
`require_identity_proofs = false`** a bare identity record is accepted, so any member can mint one for a made-up id
and that id's entries count again: the fabricated-id defence assumes proofs. **Not built — a residual:** tally entries
are not signed, so a member that writes forged entries under a *real* member's namespace after that member has died
is not detected (the `sys/` tripwire runs only at the owner, which is gone). While the owner lives, its tripwire
counts the foreign write and its next publish overwrites it.

So a fleet using the tally runs every member with both features. **Upgrading:** tally rules fail closed until every
member runs this release built with `gateway` and `tls` — an older member, or one built without them, publishes no
heartbeat and is unknown coverage. A forged `departed` heartbeat stops peers waiting on a live node that has gone quiet,
until its next real heartbeat overwrites the forgery (its entries are still counted) — no more than a forged
`enforces: false` already does, which stops them counting it; both are detection only, counted by the `sys/` tripwire
at the owner. Every tally decision carries the sentence
*approximate: detection, not prevention — concurrent calls at different gateways can exceed the limit before the
tallies converge*, and its coverage (`publishers=[…] stalest_ms=…`).

*Which electorate decides a seat.* The exclusive verbs' (`exclusive_propose`): the fleet's `exclusive_default`
electorate group when one is declared, the cluster otherwise. Under `consensus_require_electorate` (required by
`secure-single-domain`) a seat outside an electorate is refused, so the rule is not established until one is declared.
Without a declared electorate the cluster's roster is the peers a node currently sees, and a partitioned minority that
has declared the rest dead can decide a seat alone — exactness across a partition needs the electorate.

*Where the counts live, and who sees them.* Seats are decision records under `consensus/life/` (slot
`ae-seat/{counter}/{actor}/{n}`); tallies are `sys/ae-tally/{node}/c/{counter}/{actor}` and a heartbeat
`sys/ae-tally/{node}/hb`. In a key, `{counter}` is a 128-bit SHA-256 digest of the declared counter id with the kind of
point, and `{actor}` one of the principal; the values are an attempt id or permit times. Every member holds them, as it
holds every gossiped key, and the gateway's raw KV routes refuse both namespaces. A member learns *that* an actor counted
against *some* counter, and when — the audit chain's reference record already tells every member, under `compliance`,
the principal, the attempt and its time; the counter id is the policy's own and never encodes the operation or
resource. A guessable counter id or principal is a pseudonym, not a secret. Tally entries are unsigned: another member
writing one is counted by the `sys/` tripwire, not refused.

*Growth.* A tally entry is withdrawn once every time in it has aged out of its window, so tallies hold only the actors
active within a window. A seat is not collected: its decision record and its slot's sentinel are kept for good, by the
lock lifecycle's design (`docs/design/lock-lifecycle.md` §2 and §8 — a slot that has had a record must never fall back
to the legacy reading, where a committed value with no lease reads live for ever, and the top record is what later
decisions and the acceptors' shrunk floor are ordered against). So each (counter, kind of point, actor) that ever
claims holds, on every member, at most `count` slots of three to five keys — sentinel, top record and its marker, and
the legacy committed and lease keys — and, on every voter, `count` shrunk acceptor entries, for good. That is why the
reference evaluator does not establish an exact fleet limit over `*`: the actors that can grow it are the ones the
policy names. An evaluator of your own that names exact counters for open-ended actors lets each new principal add
that much.

*Bounds.* A fleet limit names at most `MAX_FLEET_SEATS` (1 024) and a window of 1 ms to `MAX_FLEET_WINDOW_MS` (24
hours — the horizon the enforcement point keeps an actor's own history for). The count is a cost on every request the
rule covers, not only a limit: for the exact scope the enforcement point reads every seat — a slot digest and a KV read
each — under the actor's gate on an async worker, and a claim may propose to each in turn; a tally entry keeps up to
`count` permit times. A limit of `u32::MAX` once pinned a worker for minutes per request. A counter outside the bounds
is **refused, not clamped**, wherever it comes from: the reference evaluator decides `Indeterminate` naming the bound
(as it does a wildcard actor at the exact scope); the enforcement point reads none of its seats or tallies and claims
no seat (`FleetState::NotEstablished`); and the seam turns any evaluator's permit relying on it into *not
established*. `FleetCounter::try_new` and `check_fleet_bounds` let an adapter refuse such a policy when it loads it,
which is better than refusing every request it covers. A policy wanting more than 1 024 in a window wants a per-point
limit or a shorter window.

*On the record.* A fleet decision's evidence names the fleet scope (`history.scope`) and carries each counter with its
scope and state — seats held, or the sum and its publishers — and an admitted call's seat (`slot`, seat number,
fencing token, lease expiry).

**An evaluator that wraps another** forwards `history_request`, `evaluate_with_history` and `fleet_counters` as well as
`evaluate`; one that does not hides the inner request, and its limited rules decide `Indeterminate`.

## Deciding where data may go: inference routing as policy

`mycelium-reason`'s router picks one provider of `llm/{model}` among several, by load and reservation. Before
post-360 row R nothing could say *this prompt may not leave the domain*: the evaluator saw the chosen resource and
nothing about where its inference runs or what the request carries. Now a routing door tells the preflight both, per
candidate, and the router dispatches to the first candidate the policy permits:

```rust
use std::sync::Arc;
use mycelium::{
    EchoBackend, EvidenceJournal, EvidenceProfile, GossipAgent, Locality, PromptTemplate, ReferenceEvaluator, Rule,
};
use mycelium_reason::{DataClassPolicy, FsBlobStore, ModelProfile, ModelReg, RouterConfig, reason_router_classified, serve_model};

async fn routing_policy(
    agent: Arc<GossipAgent>,
    dir: &std::path::Path,
) -> Result<ModelReg, Box<dyn std::error::Error>> {
    // Gateway door, mounted before start: classes least to most restrictive. An unclassified request — and any
    // caller with no floor bound — is the most restrictive class; `web-agent`'s credential may send `public`.
    let classes = DataClassPolicy::new(["public", "internal", "confidential"])?
        .floor_for(format!("token:{}/web-agent", agent.node_id()), "public")?;
    let store = Arc::new(FsBlobStore::open(dir.join("blobs"))?);
    agent.with_http_routes(reason_router_classified(Arc::clone(&agent), store, RouterConfig::default(), classes));

    // Evidence and policy, attached before start: from the first request on, every route is decided and
    // recorded. (A classifying router refuses to route while no evaluator is attached.)
    agent.with_evidence_journal(EvidenceJournal::open(dir.join("evidence.journal"), EvidenceProfile::Strict)?);
    // Policy, as an allow-list: public and internal data may go anywhere; confidential only in-domain.
    // A class not listed here (a typo, a class added later, `restricted`) matches no allowance: refused.
    agent.with_action_evaluator(Arc::new(
        ReferenceEvaluator::new("rev-route")
            .with_catalogue("cat-route", "1")
            .map_action("llm.invoke", "*")
            .allow(Rule::new("*", "llm.invoke", "*").for_class("public"))
            .allow(Rule::new("*", "llm.invoke", "*").for_class("internal"))
            .allow(Rule::new("*", "llm.invoke", "*").for_class("confidential").at_locality(Locality::InDomain)),
    ));
    agent.start().await?;

    // Serve side: each provider declares where its inference runs (the `locality` attribute of its llm-meta ad).
    let template = PromptTemplate {
        system: "You answer briefly.".into(),
        user_template: "{{input}}".into(),
        max_tokens: 256,
        temperature: 0.0,
        metadata: Default::default(),
    };
    Ok(serve_model(&agent, ModelProfile::new("qwen3").in_domain(), template, Arc::new(EchoBackend)).await?)
}
```

This is `guide_20_routing_policy` in `mycelium-reason/tests/gateway.rs`, compiled and run in CI (a confidential
request routes to the in-domain model and the evidence journal records the decision); in production the
`EchoBackend` is your engine's backend and the issuer in the token principal is your gateway's.

**Where the class comes from.** Every caller has a **floor**: the class the operator bound to its verified principal
(`floor_for`, or `floor_for_all` for everyone), or, when none is bound, the policy's **default** — the most restrictive
class unless `unclassified_as` says otherwise. The client may **declare** a class in the `x-mycelium-data-class`
header, once; a declaration at or above the floor is taken, one below it is overruled by the floor. So an unclassified
request is the default class, no caller can declare itself below the default, and only a principal the operator bound
lower (above, `web-agent` → `public`) can send a lower class. A class the policy does not have is `400`, so is the
header given twice, and so is any declaration to a node whose router classifies nothing (`reason_router` without
classes) — a declaration that cannot be honoured is not silently dropped. **No SDK declares a class yet**: neither
`mycelium-py`'s `reason.route()` nor `mycelium-ts` takes one, so an SDK caller is classified by its floor (set the
header yourself with a raw HTTP client or an OpenAI client's default headers). The envelope carries the class and who declared it (`DataClass { class, source }`,
`ClassSource::{Principal, Declared, Default}`).

**Where the destination is.** The candidate's `llm/{model}@{provider}` and the provider's own declaration of where its
inference runs — `ModelProfile::in_domain()` (an engine on a fleet node) or `external()` (the backend sends the prompt
off the mesh), the `locality` attribute of its `llm-meta/{model}` ad. A gateway client cannot write an `llm-meta` ad at
all — `capability/advertise` and `units/declare` refuse the namespace `403 reserved_namespace` — and a catalogue entry
whose `provides` is one is never installed (`prov.eligible`, `reserved_namespace`). And no client can stand in for a
model: `GET /gateway/rpc/serve/{kind}` refuses `llm.invoke`, `mcp.invoke` and `skill.invoke` `403 protected_kind`, and
any kind the node already serves in process `403 served_in_process` — a signal fans out to every receiver on a node,
so a serve stream beside one would receive every request the node was sent ([rbac](../operations/rbac.md)). A provider that declares nothing is
`Locality::Unknown`, and a rule about locality reads it as **not established** — never as in-domain.

**Decided, or not routed.** A classifying router (`reason_router_classified`) routes only under an action evaluator:
with none attached — before `with_action_evaluator`, or in a build without `tls` — every request is refused `403
authority_not_established` and nothing is dispatched (a classification no one decides on is no control). Attach the
journal and the evaluator before `start()`, as above. The plain `reason_router` is unchanged.

**Per candidate, in the router's order.** The preflight runs for each candidate before the router commits to it. A
refusal (`action_denied`, `authority_not_established`) passes that candidate over without counting as an attempt; the
first permitted one is dispatched. So a confidential request reaches an in-domain model when one exists — the response
names the destinations passed over (`refused`) and the class (`data_class`) — and when none does it is `403`, with
`data.refused_destinations` listing each candidate's destination, locality and reason, and `data.data_class`. A
decision that could not be recorded (`evidence_not_recorded`) stops the route at once.

**On the record.** Each candidate's decision is its own evidence record: a refused one is `Decided` with `execution:
none`; the permitted one carries `route` — the class and its source, the destination with its locality, and the
destinations refused before it — and `destination` names where it went.

**The rule types** (reference evaluator; additive): `Rule::for_class(c)` applies a rule only to a request of class `c`,
`Rule::at_locality(l)` only to a destination at `l`. **A class rule names exactly one class** — the evaluator knows
nothing of the classifier's order — so a *prohibition* on `confidential` does not cover a stricter `restricted`, and a
misspelt class makes it inert. Write the policy as an **allow-list**, as above: a class it does not list matches no
allowance and is refused. (`at_locality(Locality::Unknown)` can never decide: an unknown locality is not established.) A class or locality the enforcement point **did not establish**
matches, so the rule reports it as not established instead of silently not applying. That is what keeps the policy
from being walked around: the substrate's own `llm.invoke` doors, `/gateway/llm/call` and `/gateway/llm/stream`, establish
no class and no destination, so under any rule about class or locality a call through them is not established —
refused, even for data that would have been public (the raw RPC routes refuse `llm.invoke` as a protected kind, and
`/a2a` does not send it). Route classified calls through the façade. The contract suite states the same (`PolicyClause::ForbidClassAt`,
the `route_*` cases).

**What this does not establish.**
- **No content inspection.** The class is declared, never detected. A client that sends confidential text marked
  `public`, with no floor above `public`, routes it as public. A floor bound to the credential is the control.
- **No observation of the backend, and no binding to the writer.** The locality is a declaration in a capability ad,
  not an observation: a member that declares `in_domain` and forwards prompts off the mesh is outside the guarantee.
  And an ad's key names its node without anything binding the key to the member that wrote it, so a fleet **member**
  can write another node's `llm-meta` ad (a gateway client cannot — the namespace is refused at its doors). The
  provider's own egress allow-list (`[egress]`) is the complementary control on what its backend can reach.
- **A provider refreshing its ad reads as unknown meanwhile.** `ModelReg::refresh_meta` (the Ollama collector drives it
  as `warm` and the VRAM figures change) retracts the old ad before writing the new one; between the two the provider's
  locality is unknown, and a class rule about locality refuses it — closed, but a sole in-domain provider can answer
  `403` for that moment.
- **Timeouts and mixed outcomes.** The router asks the policy lazily, so an attempt gets the short `failover_timeout`
  while untried candidates remain even if the policy then refuses all of them — a slow sole permitted model ranked
  first can time out early. A route where some candidates failed and the rest were refused answers `502 exhausted`,
  the refusals only in the evidence.
- **Floors are exact principals.** `floor_for` keys are the gateway's principal strings (`token:{issuer}/{name}`,
  `oidc:{issuer}/{subject}`); a key that matches no principal binds nothing, silently — its caller gets the default.
- **A declared class and a bound one decide alike.** `ClassSource` is on the record, but the reference rules do not
  distinguish a declared class from a principal-bound one. (The private companion's Cedar adapter expresses
  `ForbidClassAt` and passes the five `route_*` cases since its re-pin onto v2.36.0, private #33.)
- **The refusal names the candidates.** `refused_destinations` and `refused` give the client each candidate's provider
  address and declared locality.
- **No control of what a permitted model does with the data** — exfiltration through a permitted model, or through
  its output, is out of scope.
- **Not the node's own calls.** `InferenceRouter::call` (the node acting in-process) runs no preflight, so neither class
  nor locality is consulted; nor do `SkillRunner`'s own OpenAI-compatible client, a `[[serve]]` backend or any other
  in-process `LlmBackend`. A `[[serve]]` skill advertises no `llm-meta` ad, so its locality is unknown and a class rule
  never permits it through the façade.
- **Not at the provider.** With `with_provider_enforcement`, the provider's own evaluator re-checks `llm.invoke` without a
  class or destination (neither travels), so a class rule placed there decides not established — closed. Put class rules
  at the gateway.
- **One evidence record per refused candidate.** A route over many providers that the policy refuses writes one denied
  decision per candidate tried.

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
**six checks are the seam's, not its**:

| The seam checks | An evaluator called directly would |
|---|---|
| an expired envelope is denied *first* | permit it — it holds no clock, and that is not its job |
| a decision from an unexpected `policy_revision` is stale | act on a policy you never deployed |
| a `Permit` carrying evaluation errors is downgraded | trust a permit its own adapter could not fully evaluate |
| an unmapped or ambiguous operation cannot be permitted | name an unreviewed action as a business operation |
| a panicking adapter is caught | turn an adapter bug into an admission |
| a permit relying on a fleet counter outside the bounds (`MAX_FLEET_SEATS`, `MAX_FLEET_WINDOW_MS`) is not established | permit on a count no enforcement point read |

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

## Checking authority before deployment: the `wire-check` overlay

Everything above decides at run time. `mycelium wire-check <units-dir>` can find, before anything
runs, the edges that no declaration could ever authorise. This is its **authority overlay** (W4,
`src/wire_check.rs`). It does not evaluate policy; it reads the units' `[[rule]]` and `[[mandate]]`
sections.

For each wired edge whose requirer declares a `[[mandate]]` or a `[[rule]]`, the check tries the
edge's two call shapes — `skill.invoke` on `skill:{ns}/{name}` and `tools/call` on `tool:{name}`:

- **`unauthorisable edge`** (error): no declared rule admits the requirer's principal on either
  shape (actor, operation and resource, with `"*"` as a wildcard), or the rule that would admit it
  sets `requires_mandate` to a scope in which no declared mandate held by that principal enumerates
  the operation. A governed requirer with no `principal` is reported the same way, because no rule
  can name it.
- **`ungoverned edge`** (warning): the requirer declares no mandate and no rule, while other units
  do. Its edge is outside the authority vocabulary.

An edge with no provider is skipped here; it is already reported as unwired. The overlay is on by
default and `--no-authority` turns it off. An error makes the check exit 1. The whole
declare-check-deploy loop is [the capability lifecycle](../operations/capability-lifecycle.md).

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

## The guarantee report — what this node enforces, and why not the rest

Everything above is conditional: the seam needs an evaluator, the journal needs attaching, the provider
check needs `with_provider_enforcement`, and each needs the feature that compiles it. The node says where
it stands, once, at `start()` — after validation and before it admits traffic — and on demand:

```rust
let report = agent.guarantee_report();            // also GET /gateway/guarantees (scope fleet:read)
for e in &report.entries {
    println!("{:<28} {:?}", e.id, e.resolution); // Enforced · NotConfigured{missing} · NotInBuild{feature}
}                                                 // · NotApplicable{because} · NotVerifiableHere{evidence}
assert!(report.node_requirements_satisfied());    // every node-enforced guarantee that applies is enforced
assert_eq!(report.unresolved(), ["net.confinement", "clock.sync"]); // yours to evidence
```

`NotApplicable` carries the role fact — a node with no `http_port` does not fail a gateway guarantee —
and is never a waiver. `NotInBuild` is the state the four start-time refusals of 2.18.1 were missing:
a setting whose enforcing code this build does not have reads so here rather than silently nothing. The
strongest sentence the report makes is *node requirements satisfied*; what a node cannot see (network
confinement, clock sync) is listed as unresolved and never counted. The consensus safety profile was on
that list until P2 (2026-10-10); it is a node fact now — `cons.safety_profile` resolves on
`consensus_require_electorate` (guide 04 § *Discovery is not an electorate*).

A companion registers its own guarantees **before `start()`**; a duplicate id, a core id, or a late
registration is refused by name:

```rust
use mycelium::{GuaranteeDescriptor, GuaranteeKind, GuaranteeResolution as R};
let store = my_store.clone();
agent.register_guarantee(GuaranteeDescriptor::new(
    "wiki.execution_authority", 1, "wiki", GuaranteeKind::NodeEnforced,
    "a wiki write is refused unless a live, unrevoked mandate authorises it",
    "feature `execution-authority`; `GitStoreConfig::authority`",
    &["mycelium_wiki::execution::WriteAuthority"], "docs/guide/21-mandates.md",
    |_view| None,                                     // applies on every node that hosts the store
    move |_view| if store.has_authority() { R::Enforced } else { R::NotConfigured { missing: "GitStoreConfig::authority" } },
))?;
```

An attachment made after `start()` is counted (`agent.late_attachments()`) and warned about: the block
logged at the boundary did not see it. Attach before `start()` in production. The registry and the report
are `src/agent/guarantee.rs`; the plan is `docs/plans/guarantees-and-rule-catalogue.md` (I2); a profile
that *refuses* to start on an unmet guarantee is its I3.

**Making the report binding — a profile.** This is the **guarantee profile** — not the control
profile (guide 22) or the domain profile (guide 17); [operations/control-profiles.md](../operations/control-profiles.md)
has the table of the four. Set it before `start()`:

```rust
cfg.profile = Some("secure-single-domain".into());   // or GOSSIP_PROFILE=secure-single-domain
```

`start()` then returns `Err(InvalidField { field: "profile", .. })` naming each guarantee the profile
requires that is not `enforced` (or `not_applicable` to this node's role), what is missing and where to
read; `dev` (the default) requires nothing and the startup log says so. The required set is
`mycelium::SECURE_SINGLE_DOMAIN_PROFILE` (rev 5: twenty ids — rev 3 added `gw.exposed_closed`, rev 4
`cons.safety_profile`, both 2.32.0; rev 5 `mesh.signed_writes`, which resolves on what `mesh.tls` does, post-360 row F2), `mycelium::check_profile` runs it against any report, and
`profile_named` resolves a name. Rev 4's `cons.safety_profile` is `consensus_require_electorate = true`, so a lock
or an election is decided only by an electorate group. The profile needs a `compliance` + `tls` build and — since
rev 2 — a node certificate **issued off-node**, so the fleet CA's key is on no node:

```rust
// On the issuer's host (the `mycelium tls issue` CLI wraps this):
let (cert, key) = mycelium::issue_node_cert(&ca_dir, &node_id, &out_dir)?;
// On the node: start from the issued pair; `auto_cert_dir` holds ca-cert.pem only.
cfg.tls = Some(TlsConfig { cert_pem: Some(cert), key_pem: Some(key), auto_cert_dir: node_dir, ..Default::default() });
```

The operator's side — the checklist, the catalogue of every guarantee's state under reference
configurations, and the issuing runbook — is `operations/production-readiness.md` §2,
`reference/guarantee-catalogue.md` and `operations/cert-rotation.md`.

## Where to go next

| You want | Read |
|---|---|
| the full decision record: envelope, verdicts, evidence, catalogue identity, strength profiles | [`design/action-envelope-ae0.md`](../design/action-envelope-ae0.md) |
| who the caller is, before any of this runs | [09 · Security](09-security.md), [`operations/rbac.md`](../operations/rbac.md) |
| the three strength tiers this chapter's promise sits in | [16 · Guardrails](16-guardrails.md) |
| the identities the envelope carries | [18 · Contracts & receipts](18-contracts-and-receipts.md) |
| why a deterministic evaluator matters | [19 · Replay & simulation](19-replay-and-simulation.md) |
