# 2026-10-08 — consensus: a prepare phase (two values could commit on a stable roster)

**Found** by an independent review of #574 (a docs PR recording the membership-epoch limit), which checked
its claims against `src/consensus.rs` and found a broader one: two concurrent proposers on a **stable**
roster could commit different values for one slot. Verified in code the same day:
`may_cast_vote_digest` granted any strictly higher ballot (`Greater => true`) and overwrote the acceptance;
the only path that reported an accepted value was a *refusal* (`Promise`), which a higher ballot never
gets; and the only guard was the proposer's local `live_committed` check, which depends on the first
COMMIT having arrived. A commits `v1` at ballot 1 with `{A,1,2}`; B, before the COMMIT reaches it,
proposes `v2` at ballot 2 to `{B,2,3}`; acceptor 2 grants; both commit, both told they won, LWW picks
whichever was re-stamped last. v2.14.0's notes claimed "a higher ballot overwrote an accepted value" was
closed; it was closed only on the refusal path (corrected in `history.md`).

**Fail-first:** `two_proposers_cannot_choose_two_values_for_one_slot` — five acceptors, quorum three, the
proposer of the day modelled as `claim_vote` alone — failed (`B` chose `v2`) before the fix.

**Fix (classic single-decree Paxos phase 1):** `ConsensusMsg::Prepare` / `PrepareAck`; acceptor state
`AcceptorSlot { promised, promised_to, accepted }` (`prepare_slot`, `claim_vote` honouring the promise);
`choose_after_prepare` — carry the highest-ballot acceptance the promise quorum reports, refuse to
overwrite one known only by digest. Ballots come from a shared key, so a ballot is promised to **one**
node; a node's own acceptor gates every proposal it sends (`cross_propose` gained the gate it lacked).
The durable record carries the promise and, when it fits, the value; the 40-byte pre-2.30.0 record still reads.

**The first cut was wrong, and the independent review showed how** (rule 4). It retired acceptor state
when a commit was newer than the record (HLC comparison) and kept the listener's erase-on-COMMIT; both
dropped **promises**, so a delayed lower-ballot `Propose` could reach forgetful acceptors and commit a
second value — the very defect, by another path (the review's probe: four votes for `v0` after `v1` was
chosen). It also found: one node's concurrent proposals sharing a promise; a lease renewal skipping the
promise quorum on the first `committed` report; a late COMMIT of a previous lease instance erasing the
next one's state; check-then-act on the memory and a persist that could go backwards; equal-ballot
refusals ignored in phase 2; a slot wedged by a digest-only acceptance; and stale record descriptions in
`lib.rs` and `signal.rs`. The rework: **nothing is erased**; a commit records its ballot
(`consensus/decided/{slot}`) — a floor acceptors refuse at or below, and the line a reopened leased slot
draws under the previous decision; ballots stay monotonic (the ballot key is no longer reset); a
same-value `committed` report is one promise; two digests at the top ballot block; persistence re-reads
and rewrites; phase 2 treats `seen >= ballot` as a refusal; the record carries the value.

**Still open, recorded:** quorums are counted from each proposer's view, so intersection across an
electorate change is not enforced — guide 04 § *Changing an electorate*, threat model §7, what-is-proven.
Leases are judged on each node's clock, so under skew one node can reopen a slot another still renews (the
lease's bounded-skew assumption); a late COMMIT re-stamps the committed key and revives a lease. Proposers
older than 2.30.0 keep their old behaviour against upgraded acceptors. `Promise`/`Nack` are not
signer-bound (CFT, not BFT), and the opacity recompute can shrink the phase-2 quorum below phase 1's.
Mixed fleets: an upgraded proposer times out until a quorum of acceptors is upgraded (`deprecations.md` §21).
