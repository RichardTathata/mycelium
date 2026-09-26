# mycelium-commitment — the contract net, as five records

The third of Mycelium's three coordination models, beside the tuple space's competitive `take`
(`mycelium-tuple-space`) and the blackboard's shared facts (`mycelium-blackboard`): **commitment** —
declare, offer, accept, fulfil, assess. A composition, not a subsystem (`docs/plans/v3-contracts-axis.md`
§6.9, §13.2): each record has a mechanism the substrate already has, and there is no planner and no
component that assigns another participant's obligation.

| Record | Mechanism | Where |
|---|---|---|
| announce | KV head, declarer-owned | `cn/{requirement}` |
| offer | `append` | `log/cn/{requirement}/offers` |
| award | lowest-participant rule; written with `set_with_receipt` | `cn/{requirement}/award` |
| report | `append`; outcome or *unknown*, never silence | `log/cn/{requirement}/reports` |
| assess | `append`, Ed25519-signed; the declarer is not the only assessor | `log/cn/{requirement}/assessments` |

**Rules.** One award per requirement — a second is *refused*, never written over the first. A requirement
with no offers is a visible state, not a retry loop. An awardee that vanishes leaves an award with no
report, reported as such; re-announcement is the declarer's decision, never automatic reassignment. An
unsigned record is unproven: `verify_assessment` / `verify_offer` / `verify_award` say `false` for one,
never true-by-absence.

**No component assigns another participant's obligation — and since 2026-09-23 that rule has a
mechanism.** An offer names its participant in a field, so anyone able to append could post an offer
naming somebody else and the deterministic rule would award them work they never offered. Sign offers
(`offer_signed`) and award from `offers_verified(requirement, resolve)`, where an offer signed by its
forger fails because the key checked is the *named participant's*. Unsigned stays legal and unchanged
(`offers()` + `plan_award`) — provenance is the declarer's decision, not this crate's policy.

Built entirely on Mycelium's public API (`default-features = false`).

## Run the example

```bash
cargo run -p mycelium-commitment --example redistribution_cn
```

The surplus-food redistribution workload the tuple-space and blackboard examples also run: a hub
announces six pickups, three drivers offer on what they can reach (one pickup contested), each pickup
is awarded once with a receipt, a second award is refused, every awardee reports against its award's
operation, and a food-bank auditor — not the hub — signs the assessments. Exits 0 with
`All assertions passed`.

## Two declarers (CN2)

`award` is for one declarer per requirement: between its plan and its commit another declarer may commit,
and LWW keeps one silently — the crate's tests show it. Where two may race, use `award_linearizable`
(or `plan_award` + `commit_award_linearizable`): the award goes through a consensus round on
`cn/{requirement}/award`, exactly one commits, the other gets `AlreadyAwarded` **with the committed
award**, and a round with no commit is `AwardUnknown` — a retry resolves it. Every reader sees the one
award through `award_of`.

## Under a mandate (CN3)

`commit_award_under_mandate(award, &acceptor_mandate, &authority, now_ms)`: the mandate must be the
acceptor's own, and the requirement's `ResourceAuthority` (`docs/design/scoped-mandates.md`) must authorize
`accept` for it now. A stale holder — a mandate minted under an epoch the resource has moved past — is
refused as `Superseded { installed, presented }` **before any write**; a passing check commits
linearizably. A deployment without mandates uses `commit_award_linearizable` — a check not called, never one
faked.

## What it does not do (yet)

Replaying an award under `mycelium-sim` **does** work since the scheduler seam (v2.9.0):
`a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam` asserts the replay
succeeds and that only the disarmed plant diverges (an earlier version of this line said the opposite).
A mandate revoked
*during* the award's round is not caught by the check, which runs before it. Participants are named, not
authenticated, here: authority lives in the gateway's caller context and in mandates, not in this crate.
