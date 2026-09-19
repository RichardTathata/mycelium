# mycelium-commitment — the contract net

↑ [companions](companions.md) · guide: [24 · Commitments](../../../guide/24-commitments.md) ·
operator: [operations/companions.md](../../../operations/companions.md)

The **third** coordination model beside the tuple space (routes by lane *position*) and the
blackboard (routes by *content*). This one routes by **negotiation**: a requirement is announced,
participants offer, one is awarded. Reach for it when the offer carries information the announcer
lacks — capacity, cost, suitability — and not merely to make a queue feel deliberate: it costs a
round trip and a deadline the other two do not.

Design: `docs/plans/v3-contracts-axis.md` §6.9 (CN1–CN3, D39). Key facts:

- **Five records, one mechanism each.** `Announcement` on a declarer-owned KV head (`cn/{req}`);
  `Offer`, `Report` and `Assessment` appended to log streams; `Award` at `cn/{req}/award`. Time
  enters through the caller (`now_ms`), so **every decision is pure in time** — which is what lets
  a whole run replay under item 6's seams.
- **The award is a receipt-bearing operation, never a KV write alone.** `Awarded` carries item 1's
  `WriteReceipt` under `operation_id = cn/{requirement}/award`, so a retry is recognisable *as* a
  retry and the award's durability is exactly what the receipt says. `AwardedLinearizable` makes
  **two** statements rather than one — the cluster agreed, *and* this node's own durability is
  whatever `commit.local_durability` reports — and the type keeps them apart.
- **The award names the offer it accepted** (`offer_hlc`, the offer's log position). That is the
  mechanical form of *no component assigns another participant's obligation*: an award points at
  something the participant volunteered.
- **The award rule is pure.** `AwardRule::LowestParticipant` — lowest participant id, ties on the
  earlier offer; the tuple space's primary-election rule reused. Deterministic from the offers alone,
  so **every reader of the streams reaches the same conclusion** and an award can be *checked* rather
  than trusted. A rule consulting anything outside the offer log would make the award an assertion.
  Deterministic is **not** fair, and the type name says which one it is.
- **Refusals are visible states, not retries.** `NoOffers` means an empty market, which is
  information about the market. `AlreadyAwarded` returns the existing award — one per requirement, a
  second refused and **the first never overwritten**. `AwardUnknown { ballots_tried }` means the
  award *may or may not* have committed elsewhere; a retry resolves it as `AlreadyAwarded` or a fresh
  commit. Item 1's `DeliveryUnknown` rule, one layer up.
- **`Outcome::Unknown` is a first-class report** — *the participant cannot say*. A contract net whose
  only outcomes were success and failure would force every participant to lie once per ambiguous run.
- **An unsigned assessment verifies `false`** — *unproven*, not forged, and specifically not
  true-by-absence. Anyone may assess, not only the declarer, and an assessment names its assessor.
- **CN3 is the item-5 join.** The acceptor's own mandate is checked **before any write**; a stale
  holder's award is `Superseded { installed, presented }`. `MandateNotTheAcceptors` is separate:
  presenting somebody else's valid mandate is a different mistake from presenting your own expired
  one, and the two refusals say so.

## The gates

| Claim | Test |
|---|---|
| one award per requirement; a second is refused, not overwritten | `one_award_per_requirement_with_a_receipt_and_a_second_is_refused_not_overwritten` |
| an empty or late market is a visible state | `no_offers_and_late_offers_are_visible_states_not_awards` |
| the plain path admits a race the linearizable path refuses | `two_declarers_racing_the_plain_award_both_win_but_the_linearizable_award_refuses_the_second` |
| a stale holder is refused **before** any write | `a_stale_holders_award_is_refused_as_superseded_before_any_write` |
| an unsigned assessment never verifies | `reports_and_assessments_round_trip_and_an_unsigned_assessment_never_verifies` |
| **CN2** — a whole-node recording replays deterministically | `a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam` |

The third row is the honest one: the plain award path **does** admit two winners under a race, and
the test asserts that rather than hiding it. Reach for the linearizable form when that matters.

CN2 was a **pinned gap** until v2.9.0's scheduler seam landed. It is the reason the seam exists: a
recording is only a reproduction if waits order the same way on replay.

## The demonstration

`cargo run -p mycelium-commitment --example redistribution_cn` — the CN1 gate, run in CI. Surplus
food needs collecting, depots offer, one is awarded, it reports, the outcome is assessed.

## What this crate does not do

**Not identity.** It does not mint or check participant identity; authority lives in the gateway's
caller context and in mandates. It records who said what. **Not exactly-once effect** — the award is
durable to the rung its receipt names; making the *work* exactly-once is the destination's job
([effects](effects.md)).
