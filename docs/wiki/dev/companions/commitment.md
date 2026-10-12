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
- **An unsigned record verifies `false`** — *unproven*, not forged, and specifically not
  true-by-absence. Anyone may assess, not only the declarer, and an assessment names its assessor.
- **Offers and awards are signed too, since 2026-09-23 — and that is the crate's first rule finally
  acquiring a mechanism.** *"No component assigns another participant's obligation"* had nothing
  enforcing it: an offer names its participant in a **field**, so any member able to append to the
  stream could post an offer naming somebody else, and `LowestParticipant` would award that
  participant work they never offered — with every reader, checking the award against the offers
  exactly as the design intends, **agreeing it was correct**. The forgery is invisible to the check
  that was supposed to make the award trustworthy.

  `offer_signed` signs; `offers_verified(requirement, resolve)` is the candidate set whose signature
  verifies under the key the caller's directory gives for the participant *the offer names* — so an
  offer signed by its forger fails, which is the realistic attack rather than the lazy one.
  `plan_award_from` takes that set, which keeps provenance **the declarer's decision** rather than
  this crate's policy: pass `offers()` and behaviour is unchanged. Awards sign symmetrically
  (`Award::signed`, `verify_award`), because an award names a declarer and an unsigned one makes
  that a claim.

  **Three things it does not establish**, and they are the reason this is `SelfImposedPrevention`
  and not more. A signature proves the holder of a key made the record. Whether that key *belongs*
  to the named participant is `sys/identity/{node}`'s question, and only as strong as
  `require_identity_proofs` — **default-off**, without which an admitted node can append its own key
  to another's identity entry ([security](../security.md)). Where a participant is a person or a
  vehicle rather than a node, the directory is the operator's and the mesh knows nothing about it.
  And nothing here stops a member *writing* the forged offer: it stops it being awarded, which is
  detection-then-refusal at the decision point, not prevention at the medium.
- **The declarer's close decides which offers were in time, since 2026-10-12 (post-360 row H, #609).** No timestamp
  an offerer writes can: `offered_at_ms` is a field of its record, and the HLC in an offer's log key is a key segment
  any member can `set` (#609's first head judged on that and its review backdated one to `0000000000010000`). The
  declarer `close`s at or after `deadline_ms` on its own clock, appending a `Close` to `cn/{req}/closes` that names
  every offer its node holds by log position and SHA-256 and binds the announcement (deadline, digest); an award
  counts only named offers, records the close's digest (`Award::close_sha256`), and is refused `NotClosed` before a
  close. **A stream, not one key** (round 2): the checked path takes the earliest close verifying under the pinned
  declarer — earliest by its signed `closed_at_ms`, then digest, never by stream key (round 3) — so a forged close
  cannot pre-empt it; an overwrite is recovered by **re-asserting** the same close (`close` again on the same handle,
  `reassert_close(&kept)` after a restart), never by re-deciding — the round-3 review showed a re-close that
  recomputed admitted a late offer. The unverified path takes the earliest naming the announcement's declarer at or
  after the deadline; a forged unsigned close dated at or after the deadline still steers it. **Residual:** the
  declarer decides; a signed close shows which offers it counted, not when it closed (`closed_at_ms` is its claim);
  and a close names offers by log position and hash, and the award re-reads them from the stream — so a member who deletes or rewrites another participant's named offer removes it from the candidates, and the award does not show it (the close and `close_sha256` are untouched). **Follow-up, not built:** carry each named offer's signed bytes inside the close and award from that copy.
  The lessons: *a key segment is data, not a clock*, and *one LWW key cannot hold both a forgery and the truth*.
- **Reads are checked like offers (row H).** Offers, reports and assessments are read only when they name the
  requirement whose stream holds them (a signed offer copied from another stream verified there). `award_of` reports
  only a record naming its requirement and operation; `award_of_verified(requirement, declarer, resolve)` takes the
  declarer **the caller pins** — a check against the writable head fell in two writes. `plan_award_verified` and
  `commit_award_linearizable_verified` are the checked path; on the unverified path any member that can write the
  award slot can block an award, and the docs say so.
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
| **a forged offer is not a candidate when the declarer checks** — and *is* one when it does not | `a_forged_offer_is_not_a_candidate_when_the_declarer_checks_provenance` |
| a signed award proves its declarer; an unsigned one proves nothing | `a_signed_award_proves_its_declarer_and_an_unsigned_one_proves_nothing` |
| **CN2** — a whole-node recording replays deterministically | `a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam` |
| a backdated log key does not make a late offer a candidate; the close decides (row H) | `a_backdated_log_key_does_not_make_a_late_offer_a_candidate` |
| no award before the close; only the declarer closes, at or after the deadline, once (row H) | `an_award_before_the_requirement_is_closed_is_refused`, `no_offers_and_late_offers_are_visible_states_not_awards` |
| offers, reports and assessments are read only for their own requirement (row H) | `an_offer_report_or_assessment_naming_another_requirement_is_not_read_as_this_ones` |
| a slot holding another requirement's award says so (row H) | `a_slot_holding_another_requirements_award_says_so` |
| the checked path needs a signed close and is not blocked by a forged award (row H) | `the_checked_path_needs_a_signed_close_and_is_not_blocked_by_a_forged_award` |
| `award_of_verified` checks the declarer the caller pins (row H) | `award_of_verified_checks_the_declarer_the_caller_pins_not_the_writable_head` |
| a forged close written first does not stop the declarer's signed close (round 2) | `a_forged_close_written_first_does_not_stop_the_declarers_signed_close` |
| an overwritten close is recovered by closing again (round 2) | `an_overwritten_close_is_recovered_by_the_declarer_closing_again` |
| a re-close after an overwrite never admits an offer the first close did not (round 3) | `a_reclose_after_an_overwrite_never_admits_an_offer_the_first_close_did_not` |
| a second signed close at a backdated key does not change the award (round 3) | `a_second_signed_close_at_a_backdated_key_does_not_change_the_award` |
| a restarted declarer re-asserts the close it kept (round 3) | `a_restarted_declarer_reasserts_the_close_it_kept` |
| on the unverified path an early close or another declarer's is not a close (round 2) | `on_the_unverified_path_a_close_dated_before_the_deadline_or_naming_another_declarer_is_refused` |
| an oversize close is refused by name, not reported as written (round 2) | `an_oversize_close_is_refused_by_name_not_reported_as_written` |
| an award names its close; a close binds its announcement (round 2) | `an_award_names_its_close_and_a_close_binds_its_announcement` |
| `award_of` reports only a record naming its requirement and operation (row H) | `award_of_reports_only_a_record_that_names_this_requirement_and_its_operation` |
| `award_of_verified` reports only an award the pinned declarer signed (row H) | `award_of_verified_reports_no_award_the_pinned_declarer_did_not_sign` |

The third row is the honest one: the plain award path **does** admit two winners under a race, and
the test asserts that rather than hiding it. Reach for the linearizable form when that matters.

The forged-offer test asserts **both halves** for the same reason: step 1 shows the unchecked path
awarding the forgery, because a test that only showed the fix would leave a reader unable to tell
what the fix is for. The gallery example (`redistribution_cn`) carries the same scene end to end.

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
