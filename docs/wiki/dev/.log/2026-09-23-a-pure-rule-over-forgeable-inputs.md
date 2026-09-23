## [2026-09-23] ingest | a pure rule over forgeable inputs checks nothing — CN provenance decided

Up: [dev](../dev.md) · pages touched: [companions/commitment](../companions/commitment.md) · code
`mycelium-commitment/src/lib.rs`, `mycelium-commitment/examples/redistribution_cn.rs` · docs guide
24, the crate README, `CHANGELOG.md`.

One of the two open design decisions of the contracts axis, closed. The other (the federation edge's
per-partner budget) is its own entry.

## The finding, which is sharper than "records are unsigned"

The companion's headline rule is **no component assigns another participant's obligation**. It had
nothing enforcing it. An offer names its participant in a *field*; any member able to append to
`log/cn/{requirement}/offers` could post an offer naming somebody else; `LowestParticipant` would
award that participant work they never offered.

The part that makes it a design defect rather than an annoyance: **the award would then pass its own
check.** The rule is pure precisely so that every reader of the streams reaches the same conclusion
and an award can be *checked rather than trusted* — and a pure rule over forgeable inputs verifies
that the rule was applied, not that the inputs were real. The forgery is invisible to the mechanism
built to make the award trustworthy.

## Why signing, and not binding the record to its writer

The alternative was provenance by construction: take the authenticated writer of the log entry and
refuse any offer whose `participant` disagrees. It fails on a fact about the substrate, not about
effort:

```text
StoreEntry { data, timestamp }                      ← what a node keeps
SyncEntry  { key, value, timestamp, is_tombstone }  ← what anti-entropy transfers
GossipUpdate { …, sender: u64, … }                  ← the only place an origin appears
```

`sender` is an `id_hash` for echo suppression, never persisted — and **anti-entropy carries no
author at all**. So writer-binding would hold on the gossip path and evaporate on the repair path,
with a reader unable to tell which path an entry arrived by. A node that was offline and caught up
would see every offer as unattributable.

That is the replication model, not a gap in it: **LWW + anti-entropy preserves value and timestamp,
deliberately, and authorship is not a property the medium carries.** Which is why `sys/role/{node}`
signs its claim and verifies it at read against `sys/identity/{node}` — the substrate made this
decision once already, in the same situation. Signing is composition with that precedent; the
alternative would have been a store, WAL, snapshot and fixture change that still did not work.

## The shape, and the one seam worth noticing

`offer_signed` / `Award::signed` / `verify_offer` / `verify_award` mirror `Assessment` exactly. The
teeth are two methods, kept apart on purpose:

- `offers_verified(requirement, resolve)` — the candidate set whose signature verifies under the key
  the caller's directory gives **for the participant the offer names**. An offer signed by its forger
  fails, which is the attack worth stopping.
- `plan_award_from(requirement, rule, now, candidates)` — the same rule over a set the caller chose.

Splitting them makes **provenance the declarer's decision rather than the crate's policy**, and it
keeps the companion out of the identity business: `resolve` is a closure, because a participant may
be a person or a vehicle rather than a node, and a coordination companion that started minting
identity would be doing someone else's job.

## What it does not establish, recorded so the signature does not read as more than it is

A verifying signature proves the holder of that key made the record. Whether the key belongs to the
participant named is `sys/identity/{node}`'s question, and that is **only as strong as
`require_identity_proofs`, which is default-off** — without proofs an admitted node can append its
own key to another's identity entry. So: `SelfImposedPrevention` in the guardrails tiers, and no
higher on its own. Whether that default should flip is a separate decision affecting every
deployment, not this crate; it is named here rather than smuggled in.

And nothing stops a forged offer being **written**. It stops it being **awarded** — refusal at the
decision point, not prevention at the medium, which is the substrate's standing posture.

## The test asserts both halves

`a_forged_offer_is_not_a_candidate_when_the_declarer_checks_provenance` first shows the **unchecked
path awarding the forgery**, then the checked path refusing it. A test that showed only the fix
would leave a reader unable to tell what the fix is for — and the unchecked path is still shipped
behaviour, so asserting it is not theatre, it is the contract for callers who pass `offers()`.
