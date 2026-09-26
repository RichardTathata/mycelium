# 21 · Mandates

Chapter 20 asked whether an action is allowed. This one asks a question that sounds the same and is
not: **is the person asking still the one who was appointed?**

It is grounded in [`src/mandate.rs`](../../src/mandate.rs) and, for the enforcement half,
[`mycelium-wiki/src/mandate_fence.rs`](../../mycelium-wiki/src/mandate_fence.rs). The design record
is [`docs/design/scoped-mandates.md`](../design/scoped-mandates.md). The runnable demonstration is
[`mycelium-wiki/examples/curator_handover.rs`](../../mycelium-wiki/examples/curator_handover.rs).

Two questions this chapter answers:

- **"Someone's role ended. How do I stop their in-flight work?"** You install a later epoch at the
  resource. Everything authorised only under the old one becomes terminal there, and no amount of
  retrying, reconnecting or restarting changes that.
- **"Isn't that just a compare-and-swap?"** No, and the difference is the chapter. Compare-and-swap
  asks *did anything change under me*. A mandate asks *am I still allowed*. Answer the second with
  the first and a revoked holder gets handed straight back to the retry loop.

---

## The honest core: the decisive invariant

> Once the protected resource acknowledges installation of epoch E2, no operation authorised only
> under E1 can commit there — even if its holder refreshes the content revision, retries, reconnects
> or restarts.

Note where the enforcement lives: **at the resource**, inside its own atomic boundary. Not in a
daemon that watches, and not in the caller's own good manners. The design record is explicit that
this is the axis's one architectural disagreement with the obvious alternative, and this is why.

---

## Three words people collapse into one

| Word | What it does |
|---|---|
| **mandate** | one appointment: who may do what, under whose authority, for how long |
| **term** | *which* appointment this is (`TermId`) |
| **epoch** | what **orders** authority (`u64`, monotonic per scope) |

Collapsing term and epoch would make *"same holder, reappointed after a gap"* indistinguishable from
*"never lapsed"*. Those are different facts about a person's authority, and a system that cannot tell
them apart cannot answer questions about the gap.

```rust
pub struct Mandate {
    holder: PrincipalId,
    established_by: PrincipalId,  // a mandate with no establishing authority is an assertion
    purpose: String,              // why it exists, in the operator's words
    scope: String,
    operations: Vec<String>,      // ENUMERATED. Absence is denial
    epoch: u64,
    term: TermId,
    valid_from_ms: u64,
    valid_until_ms: u64,
}
```

**There is no wildcard in `operations`, deliberately.** A wildcard is how an operation nobody
reviewed becomes permitted.

---

## Four refusals, and why one of them is not a conflict

```rust
pub enum MandateRefusal {
    Superseded { installed, presented },
    NotEnumerated { operation },
    OutOfWindow { now_ms, valid_until_ms },
    WrongScope { resource, mandate },
}
```

`Superseded` is **terminal for that attempt**. The others are refusals about *this* request;
superseded is a statement about the holder.

This is the sharpest example in the codebase of chapter 00's rule that refusals are typed by what the
caller should do next. A `Conflict` is a retry loop's *input*: read fresh state, re-apply, resubmit.
Classify a stale mandate as a conflict and **the retry loop launders the revocation** — a replaced
curator re-reads, re-applies and resubmits, forever, and the advice the system gave them was to do
exactly that.

The wiki surface makes the same distinction concrete. A refused fenced write returns
`WikiError::Io(MandateRevoked { refname, expected, found })` rather than `WikiError::Conflict`:

- a conflict says *"another writer got there first — re-read and re-apply"*,
- a revoked mandate says *"you are no longer the curator — re-applying will refuse forever."*

That was a real defect, found by running the demonstration rather than by reading the code, and it
shipped as an upgrade note in v2.9.0.

---

## The fence is one transaction, not two operations

The resource-side check and the write happen together:

```text
start
verify <mandate-ref> <expected>
update <content-ref> <new> <old>
prepare
commit
```

Checking the mandate and *then* writing would leave a window in which a concurrent appointment lands
between the two. So the verify is inside the transaction, which means the mandate is checked
**through commit**.

The same reasoning extends to the push: `--atomic` **and** `--force-with-lease` on the mandate
reference go on **every** push, including content-only ones, because otherwise there is a window
between reading the mandate and pushing the content.

**It fails closed.** A remote that does not honour atomic updates is not a supported strict-profile
remote, and the write is refused rather than downgraded.

---

## Three ways an appointment ends, recorded separately

```rust
pub enum LifecycleEvent {
    RoleExpired { term, at_ms },
    PermissionWithdrawn { term, by, at_ms },
    OutstandingOperationsInvalidated { epoch, at_ms },
}
```

**A term that ran out is not a term that was taken away.** Folding them into one "ended" event would
destroy the distinction an auditor most often needs, and the second carries `by` precisely because
someone did it.

---

## Handover: history inherited as history

A successor needs what their predecessor knew. What they must not silently acquire is what their
predecessor *concluded*.

```rust
pub enum Inherited {
    Observed { at_ms, text },
    ConcludedBy { author, term, at_ms, text },   // always attributed, never bare
}
```

**There is no constructor that yields a bare conclusion.** A successor cannot end up holding a
predecessor's judgement as their own, because the type does not let them. `render()` prints the
attribution where attribution is owed, and `is_conclusion()` lets a reviewer filter.

---

## Run the demonstration

```bash
cargo run -p mycelium-wiki --example curator_handover --features git-store
```

A curator is replaced mid-edit. Watch the fence refuse the old holder's write **by name** rather than
as a retryable conflict, and watch the successor inherit observations directly and conclusions with
their author attached.

Two things the demonstration taught that the tests had not. The store is manifest-last, so a section
with no manifest entry is invisible to a read — bootstrapping needs a page write. And section writes
are compare-and-swap, so the current version must be read first, or a compare-and-swap conflict
masquerades as the fence refusal you were trying to observe.

---

## Presenting and enforcing a mandate at the gateway and the provider (2.15.0)

Everything above is the *contract* and the one fence the wiki store keeps. Since 2.15.0 the
substrate also lets a caller **present** a mandate and lets the gateway — and the provider where
the work runs — **enforce** it. This section is the Dev-facing map of that API; the operator's
version, with the same snippet, is [confined-fleet.md §3](../operations/confined-fleet.md).

**The three pieces, on the node that decides.**

```rust
use mycelium::mandate::authority::{ClockModel, ExecutionGate, FreshnessPolicy, ResourceTier};
use mycelium::mandate::grant::{EntitlementTable, GrantVerifier};
use mycelium::mandate::{PrincipalId, ResourceAuthority};
use mycelium::ExecutionAuthority;

// 1. Who may appoint for a scope — the entitlement table the grant verifier consults.
let mut entitled = EntitlementTable::new();
entitled.entitle("depot", PrincipalId::new("operator:acme").unwrap());
// 2. The gate: the resource's epoch, its tier, your clock-skew bound, and the freshness policy
//    (F, I, D) — `strict` refuses a policy that cannot hold (F ≤ 4 s, or I + D > F − 4 s).
let gate = ExecutionGate::strict(
    ResourceAuthority::new("depot", 1),
    ResourceTier::Serialised,
    ClockModel { skew_ms: 500 },
    FreshnessPolicy { freshness_ms: 120_000, interval_ms: 30_000, delivery_ms: 10_000 },
)?;
// 3. Attach. `external` is the `TrustedExternalIssuers` holding the appointing authority's key.
agent.with_execution_authority(Arc::new(ExecutionAuthority::new(gate, GrantVerifier::new(entitled), external)));
// Keep it fresh: the authority's signed revocation checkpoint, at least every I, even when empty.
agent.offer_revocation_checkpoint(&checkpoint);
```

`with_execution_authority` needs `--features gateway,tls` (`compliance` implies both). It also marks
the revocation view as *started now*, so a checkpoint replayed from before the start cannot refresh
it (C8), and `with_durable_epochs` keeps installed epochs across a restart so a **restart does not
restore revoked authority**.

**Presenting one.** A caller puts a `SignedMandateGrant` plus a possession proof in
`params._meta.mandate` (`/mcp`) or the A2A message metadata; the SDKs build it (`mandate=` on
`A2aClient.send`, signing over `mandate_request_bytes(...)`). The gateway verifies the grant, checks
freshness, and binds `MandateState::Established` into the envelope; a rule written
`Rule::requiring_mandate("depot")` then permits. `ReferenceEvaluator::allowances_without_mandate(..)`
lists any allowance that would let a protected operation through *without* one — it must be empty.

**Enforcing where the work runs.** Attach the same evaluator and authority on every provider node and
call `agent.with_provider_enforcement()`: the gateway's own preflight then runs again at enforcement
point `provider`, so a member that reaches the provider without the gateway is refused as the gateway
would refuse it. The presented mandate **travels** with the call (carried, not verified) for exactly
this. Provider enforcement **fails closed without an evaluator**: attaching none enforces nothing.

**Work already running.** When a mandate expires, is revoked or is superseded, admitted work is
cancelled by a sweeper within its class's bound; a cooperative skill races
`req.authority_lapsed()` against its own work in a `select!` (`src/agent/rpc.rs`) and stops on its own. `cargo run --example authority_drain --features
compliance` measures the stop against the bound; it runs in CI.

**Doors.** The raw routes (`rpc/call`, `scatter`, `signal/emit`, `mailbox/deliver`, `shard/emit`,
`overlay/emit_reliable`) refuse `mcp.invoke`, `skill.invoke`, `llm.invoke` and anything in
`protected_rpc_kinds` with `403 protected_kind`; serving an RPC kind needs `mesh:serve`
([deprecations §11](deprecations.md)). The bypass matrix — every door running nothing after a
revocation, by the handlers' own counters — is `test_c7_the_bypass_matrix_no_door_runs_revoked_work`.

**Not claimed:** the check-then-act window is narrowed and stated per site, not eliminated (the wiki
store counts `late_writes()`); `clock_sync` in the confinement report is always *unverified*.

## What this does not establish

- **Not a second fence.** One resource-side fence exists, in the wiki's git store. Another resource
  wanting this guarantee implements it in its own atomic boundary; nothing here does it for you.
- **Not protection for a resource with no fence.** A store with no mandate fence configured behaves
  exactly as before, byte for byte. The invariant is about resources that installed an epoch.
- **Not authorisation.** A mandate says the holder is still appointed. Whether *this* action is
  permitted is chapter 20's question, and the two compose rather than substitute.

---

## Where to go next

| You want | Read |
|---|---|
| the decision record, including the architectural disagreement | [`design/scoped-mandates.md`](../design/scoped-mandates.md) |
| the contract in code | [`src/mandate.rs`](../../src/mandate.rs) |
| whether *this action* is permitted, once the holder is established | [20 · Authorising actions](20-authorising-actions.md) |
| why a refusal's type is the message | [00 · Concepts](00-concepts.md) |
