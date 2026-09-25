# Step 5 closed: exclusivity is fenced, not leased — 2026-09-25

**Status: CLOSED by decision.** The last open question from the agreement repair, retired by asking
where the proposed mechanism would actually apply.

## The question

#383 made `elect_leader_receipt` name the rung it reached (`Decided` · `Observed`) and hand back a
fencing token, which solved the *honesty* problem: a provisional local preference is no longer
presented as an exclusive grant. What it left open is whether the substrate should go further and
offer **exclusive ownership** — an answer that stays true for a stated duration.

Even `Decided` is historical. It asserts *a quorum voted for my value at that ballot, bound to it by
digest, and no other value can have been committed at that ballot*. It does **not** assert that no
higher ballot has since committed something else: the node read its own replica after a bounded
poll. The strongest rung available is therefore **"as of my last look"**, with no expiry on it.

Two positions followed:

- **A — the token is the whole story.** Bound votes, one vote per ballot, accepted-value
  preservation, a monotonic epoch; exclusivity enforced **at the resource** by refusing a lower
  token. This is what shipped.
- **B — a lease makes it true for a duration.** The machinery exists (`committed_lease_secs`,
  read-side expiry against the commit's HLC, `LockGuard` already carrying the same token). A leased
  leadership would say *"mine until T"* — a real bounded claim, bought with a **clock assumption**.

## What closed it

One question, asked in review: **if the fleet is all Mycelium, what node cannot refuse a stale
token?**

**None.** Every node holds an HLC and can compare two `u64`s. Inside the fleet, fencing is not
merely sufficient, it is trivial — so B buys nothing there.

The unfenceable thing is never a *node*. It is the **effect at a boundary**: a third-party API, an
LLM provider, an actuator, a legacy service, an email, a payment. None of those know what an HLC is.

**But a lease does not help there either**, and item 1 settled why before this question was asked.
`mycelium-effects` says it in its own crate doc: the substrate provides three rungs and **never** the
fourth, because *a resource is the only thing that can say whether an effect happened there*. The
instrument at a boundary is not a token saying who may act; it is **`operation_id` + idempotent
merge** — *at-least-once + idempotent = exactly-once effect* — committed in one transaction with the
business change.

So:

| | fencing | a lease |
|---|---|---|
| **inside the fleet** | works, trivially | buys nothing |
| **at a boundary** | not applicable | does not address the actual problem, which is *did the effect land* |

A lease would purchase a clock assumption to solve a problem that exists in neither place.
**Position A is the answer, and step 5 is finished** — not "finished pending a decision".

## The shape worth keeping

This is D40's shape again, a day later: **a proposal that looked necessary until someone asked
where exactly it would apply.** In both cases the mechanism was coherent, the need was assumed, and
the assumption dissolved on contact with a location. Asking *where* before *how* would have saved
both.
