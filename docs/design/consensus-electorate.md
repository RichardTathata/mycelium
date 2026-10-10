# Discovery is not an electorate (ADR, post-360 hardening D1)

**Status:** **adopted** 2026-10-10. Records a decision; changes no code. Plan of record:
`docs/plans/post-360-hardening.md` (PR #593) §2 and decision D1, rows **P2**, **C1**, **C2** and **Φ**
(**P2 built** 2026-10-10, §8; C1, C2 and Φ not built). It builds on the threat model's supported profile
([`threat-model.md` §7](../threat-model.md#7-safety-sensitive-agreement-the-supported-profile)), guide 04's
*Changing an electorate* ([`guide/04-consensus.md`](../guide/04-consensus.md#what-a-successful-election-means)) and the
philosophy's corrected litmus ([`philosophy.md`](../philosophy.md#the-corrected-litmus)).

> Posture, once: **who can be found and who decides are two capabilities.** Discovery — gossip membership, capability
> groups, emergent groups — is dynamic and eventually consistent, by design. A consensus electorate is a fixed set
> for the life of one decision. Quorum intersection is a property of the electorate, never of discovery.

---

## 1. Context — the review comment this answers

Third-party reviews keep raising the same point, in roughly these words:

> *Consensus is safe only with a fixed electorate. The project's own ledger says a roster change can break quorum
> intersection; membership epochs and joint consensus are not implemented. Dynamic discovery and dynamic consensus
> membership must be treated as separate capabilities. Prevent membership changes for consensus groups, or implement
> versioned electorates with joint-consensus transitions.*

The first three sentences are correct, and the project states them itself: the threat model §7 says quorum
intersection across a membership change "is an assumption the protocol does not enforce", guide 04 § *Changing an
electorate* says how a roster change breaks it, and [`what-is-proven.md`](../operations/what-is-proven.md) lists
*agreement across an electorate change* as not shown. What the reviews could not find was the decision behind those
sentences, recorded in one place. This is that record.

How Layer III counts today (`src/agent/consensus_handle.rs:214-242`): a proposer reads the group's roster from
`grp/{group}/*` as **this node observes it**, refuses if the roster is empty or below a declared floor, and computes
`floor(n/2)+1` (or a fixed `quorum_size`). Two proposers with different views compute different quorums, and nothing
makes the two views one view. The prepare phase (2.30.0, `src/consensus.rs:1169-1177`) makes a later proposer adopt
what a quorum accepted **when the two quorums intersect**; it does not make them intersect.

## 2. Decision

### 2.1 Discovery and the electorate are separate capabilities, by design

- **Discovery** — gossip membership, SWIM liveness, capability groups, emergent groups (`src/agent/emergent_groups.rs`),
  the elastic membership governor (`src/agent/membership_governor.rs`) — answers *who is here and what can it do*. It is
  dynamic and eventually consistent, and that is the point of a coordinator-free substrate: nodes join, leave and are
  found without anyone's permission.
- **The electorate** answers *whose votes decide this slot*. For a safety-sensitive decision it is a **fixed set for the
  life of the decision**. Quorum intersection — the property single-decree safety rests on — is a property of that set.

The project agrees with the reviewer's first sentence: it is the design, not a gap. A group's roster *serves* as the
electorate only while it is held still; the substrate does not, and will not, derive an electorate's safety from
discovery converging.

### 2.2 The supported profile for safety-sensitive agreement is a fixed electorate per decision

Locks, leader election and any exclusive outcome — anything a second holder would corrupt — run the profile stated in
the threat model §7: an explicit strict-majority `quorum_size` of the fixed voter set; `use_trust_slices` with every
voter declaring the same set; `count_opaque_as_absent` off; membership **fixed for the life of the slot** (drain and
re-form to change it); and the effect fenced **at the resource** with the commit's token. Outside that profile, Layer III
is coordination that says what it means, fit for work distribution and for elections whose loser merely idles.

Why a fixed electorate, rather than a moving one made safe: a safe transition between electorates is a protocol of its
own (membership epochs, joint quorums that must each be met during the change). Building it is protocol work; this
decision does not reject it (§4.2), but the supported profile is the one that ships, and it is the one the code can
already be held to.

### 2.3 Consensus is a Layer III protocol, never a service (D1)

Layer III may run any agreement protocol — the corrected litmus allows "ballots, quorums, roles, listeners, an explicit
lifecycle" ([`philosophy.md`](../philosophy.md#the-corrected-litmus)). What it may never do is demand a permanently
privileged node. So:

- **The electorate is named as a governed group, never as a list of node identities** in a deployment's shape. Which
  nodes are in it is a governance act on the group; no configuration names "the consensus nodes". Built by P2
  (§8): the supported profile names an **electorate group**, whose member set at each epoch is a decision of the group
  itself; a trust slice (`declare_trust`) stays available beneath it as an optional narrowing, no longer the
  profile's voter set.
- **No deployment shape, configuration default or document may make a named node set the place agreement happens.**
  Whichever nodes participate in a group run the protocol; roles form per ballot and dissolve when the decision
  completes; the state is ordinary keys and signals; Layers I and II know nothing of it.
- **Roles and outputs decay.** Leadership is to be leased by default and acceptor state collected once its decision is
  over (rows C1, C2). **Neither is built**: today `elect_leader` commits permanently (it proposes with
  `ConsensusConfig::default()`, `src/agent/consensus_handle.rs:645`, whose `committed_lease_secs` is `None`,
  `src/consensus.rs:177`), and acceptor memory (`AcceptorMemory`, `src/consensus.rs:2283`, durable under
  `sys/consensus-accepted/`) is never collected — it grows with slots
  ([runtime-invariants](../wiki/dev/architecture/runtime-invariants.md)).

## 3. What is enforced today

Each line is something the code does today; the citations are to `main` at `0123ab0c`.

| What | Where | Test |
|---|---|---|
| **An empty roster is not an electorate.** A group this node sees no members of is refused `ElectorateUnavailable`, not decided alone | `resolve_electorate`, `src/agent/helpers.rs:83-88`; applied at `src/agent/consensus_handle.rs:225-236` and, for the gateway's election route, `src/agent/http.rs:3597-3606` | `an_electorate_is_established_not_inferred` (`src/lib_tests.rs:9738`) |
| **The electorate floor.** A fresh `MembershipIntent.min` (`src/agent/membership_governor.rs:44`) is a floor: a node seeing fewer members refuses, rather than computing a smaller quorum from a partial view. The floor binds only while asserted (30 s, `ELECTORATE_INTENT_TTL_MS`, `src/agent/helpers.rs:44`) | `declared_electorate_min`, `src/agent/helpers.rs:100-110` | as above |
| **A proposer must be in the electorate.** A `Group` proposal from a node not in `grp/{group}/` is refused `NotAMember` at the engine's door, so every caller reaches it | `src/consensus.rs:1064-1070` (variant at `:286`) | `a_non_member_cannot_propose_to_a_group_on_either_surface` (`src/lib_tests.rs:9848`) |
| **The prepare phase** (2.30.0): a proposer asks a quorum what it accepted before proposing, and carries the highest-ballot value reported | `src/consensus.rs:1169-1177` | runtime-invariants § Layer III |
| **A governed group's membership moves only through a governance route at the gateway** (2.29.0): `POST`/`DELETE /gateway/mesh/group` refuses a group under a live membership intent **403** `governed_group`; `POST`/`DELETE /gateway/govern/group` (`govern:write`, audited) moves the node; `units/declare` does not redefine a governed group | `is_governed_group` / `refuse_governed_group`, `src/agent/http.rs:1646-1664`; applied at `:1608` and `:1627`; `units/declare` at `:2195-2201`; scopes at `:772`, `:777-778` | `a_governed_groups_membership_moves_only_through_a_governance_route` (`src/agent/http.rs:6870`) |
| **The raw KV routes do not write `grp/`** — a node joins a group itself | `OWNED_KV_PREFIXES`, `src/agent/http.rs:3015-3018`; door named at `:3069-3070` | `every_namespace_in_the_table_is_classified_for_the_raw_kv_routes` (`src/agent/http.rs:4906`) |
| **Emergent membership defers to governance.** The capability-match watcher neither auto-joins nor auto-leaves a group under a live membership intent | `governor_owned_groups`, `src/agent/emergent_groups.rs:129-146`; applied at `:196-197` and `:241` | — |
| **The trust-slice filter.** With `use_trust_slices`, the tally counts only votes from the declared set | `src/consensus.rs` module doc; `declare_trust`, `src/agent/consensus_handle.rs:140` | `test_trust_slice_filters_votes` (`src/lib_tests.rs:2913`) |

Since P2 (2026-10-10, §8):

| What | Where | Test |
|---|---|---|
| **A safety-sensitive proposal requires an electorate group.** With `consensus_require_electorate`, a proposal flagged `safety_sensitive` or in the `lock/`, `leader/`, `consistent/` families whose scope is the cluster or an undeclared group is refused `ElectorateNotGoverned` at the engine's door, nothing sent; gateway **403** `electorate_not_governed` | `ConsensusEngine::refuse_ungoverned` and the door in `propose_inner` / `cross_propose_inner`, `src/consensus.rs`; `is_safety_sensitive`, `src/agent/electorate.rs` | `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group` (`src/lib_tests.rs`); `an_exclusive_outcome_at_the_gateway_needs_an_electorate_group` (`src/agent/http.rs`) |
| **An electorate is pinned by identity and epoch.** A proposal to an electorate group is refused `ElectorateMismatch` unless the roster equals the epoch's member set by identity (a swap is refused, counted on the roster tripwire); promises and votes count only from the members; the quorum is at least a strict majority, opacity and `quorum_size` notwithstanding | `ConsensusEngine::electorate_door`, `electorate_vote_filter`, `src/consensus.rs` | `a_swap_without_a_declared_step_is_refused`; `votes_from_outside_the_electorate_are_not_counted`; `the_roster_is_the_vote_filter_and_a_trust_slice_only_narrows_it` |
| **Acceptors answer only their own epoch, and a step fences the one before it.** `PrepareIn`/`ProposeIn` name the proposer's epoch and digest; an acceptor holding another refuses `StaleElectorate`; one that accepted a step no longer answers the epoch before it | `ConsensusEngine::electorate_admits`, `refuse_stale`, the listener, `src/consensus.rs` | `chained_steps_refuse_a_stale_proposer` |
| **A step is a consensus decision of the current electorate**, one member at a time, certified by the deciding votes; a forged record is not adopted | `electorate::declare`, `propose_step`, `check_step`; `ConsensusEngine::electorate_view`, `certificate_holds` | `two_concurrent_steps_cannot_both_commit_and_a_non_member_cannot_step`; `a_forged_electorate_record_is_not_adopted`; `an_electorate_certificate_verifies_under_tls`; `a_step_moves_one_member_and_one_epoch` |
| **Nothing resizes an electorate group but a decided step.** The governor does not roll on it; the emergent watcher defers to it; `/gateway/mesh/group` refuses it **403** `governed_group` | `membership_governor::converge`; `emergent_groups::governor_owned_groups`; `is_governed_group`, `src/agent/http.rs` | `the_governor_and_the_watcher_leave_an_electorate_group_alone` |
| **The fleet's exclusive electorate is a fleet record.** The cluster-scoped exclusive verbs decide in the group marked `exclusive_default`; a disagreeing `consensus_electorate` is refused | `electorate::exclusive_electorate`; `exclusive_propose`, `src/agent/consensus_handle.rs`; `overlay_exclusive_propose`, `src/agent/http.rs` | `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused`; `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group` |
| **A cross-group proposal does not decide for an electorate group** | `electorate_door` (`Groups`) | `a_cross_group_proposal_does_not_decide_for_an_electorate_group` |
| **The secure profile checks it.** `cons.safety_profile` (rev 2) resolves `enforced` on `consensus_require_electorate`; `secure-single-domain` rev 4 requires it | `src/agent/guarantee.rs` | `the_consensus_safety_profile_resolves_on_the_electorate_requirement`; `the_secure_profile_refuses_an_open_node_by_name` |

**What is not enforced — the residual, stated exactly** (as recorded before P2; each bullet now says what P2 changed):

- **The governance boundary is the gateway's.** An embedded caller's `mesh().join_group` (`mycelium-core/src/mesh_handle.rs:100-107`)
  or an embedded KV write to `grp/{group}/{node}` still moves a governed group's membership, and a peer's write to
  `grp/` is accepted by LWW like any other — detection, not prevention (the philosophy 360 finding Φ3). *Since P2:*
  still true, and now **detected**: for an electorate group, a roster that differs from the epoch's member set refuses
  every proposal to the group (§8). The write itself is not prevented.
- **Nothing requires a governed group for a safety-sensitive proposal.** The profile is chosen per proposal in
  `ConsensusConfig` and nothing validates it: the guarantee `cons.safety_profile` reports `not_verifiable_here`
  (`src/agent/guarantee.rs:852`; [`guarantee-catalogue.md`](../reference/guarantee-catalogue.md)). That is row **P2**.
  *Since P2:* closed on a node with `consensus_require_electorate`; the guarantee is node-enforced (§8). Off by
  default — a node that does not set it runs as before, counted.
- **"Governed" is not "fixed".** A governed group's membership still moves — through the governance route, and, on
  nodes that opt in with `start_membership_governor`, by the governor's probabilistic self-election toward the
  intent's `[min, max]` (`src/agent/membership_governor.rs:1-13`). An intent also evaporates: one not re-published
  within `MEMBERSHIP_INTENT_TTL_MS` (5 min) leaves the group ungoverned. The supported profile's *fixed for the life
  of the slot* is the operator's to keep; the code makes moving a governed group a named, audited act at the gateway,
  not an impossible one. *Since P2:* an **electorate group** answers this — its electorate is a consensus decision
  that does not evaporate, the governor and the watcher leave it alone, and a change is one member per decided step
  (§8).
- **Today's profile names its voter set by node identity.** `declare_trust(group, &[NodeId])` is how §7's eligible set
  is stated. D1 says the electorate is *named* as a governed group; reconciling the two is part of P2, and this record
  does not decide whether trust slices remain the vote filter underneath. *Since P2:* decided — the epoch's member
  set, decided by the group, is the vote filter; a trust slice may narrow it, and no longer names it (§8).

## 4. What is planned, and what is a later plan

### 4.1 In the post-360 plan — P2 *built* (§8); C1 and C2 *not built*

| Row | Promise (quoted from the plan) |
|---|---|
| **P2** | "safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities" |
| **C1** | "`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)" |
| **C2** | "acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on" |

### 4.2 A later plan — *not built, not scheduled*

**Versioned electorates with joint-consensus transitions** — membership epochs carried on every ballot, and a
transition during which a proposal must win a quorum of the old electorate *and* of the new. Recorded as protocol work
for a later plan under D1 (the post-360 plan's §5). It would let an electorate change under a live slot; it would not
change §2.1 — discovery would still not be an electorate, and the epoch would still name a group, not a node set.

## 5. Consequences

- Adopters read one sentence for safety-sensitive use: **fix the electorate per decision, and fence at the resource.**
  Guide 04, the FAQ, the threat model §7 and `what-is-proven.md` say it and link here.
- Elastic membership (the governor, emergent groups) stays a discovery feature. Using it to grow a group that also
  decides exclusive outcomes is outside the supported profile; since P2 such a group is an **electorate group**, which
  the governor does not resize and which moves only through the governance route.
- Review comments asking to *prevent membership changes for consensus groups* are answered in part today (the gateway
  refuses a governed group's change except through governance) and, since P2, by the proposal itself on a node that requires an electorate group (§8);
  comments asking for joint consensus are answered by §4.2.
- No Layer I write guard is added on `grp/` or `consensus/`: teaching the substrate a Layer III law inverts the
  dependency ([`philosophy.md`](../philosophy.md#the-corrected-litmus), *the inverted dependency*). Tripwires and
  refusals above it, not guards below.

## 6. Rejected alternatives

| Alternative | Why rejected |
|---|---|
| **A consensus service** — a fixed tier of nodes (an etcd or ZooKeeper shape) everyone else must reach to agree | It is the coordinator trap: symmetry breaking baked into the laws — permanent, privileged, load-bearing. It would make a named node set the place agreement happens, which D1 forbids. A deployment that already runs such a service may use it beside Mycelium; Mycelium does not become one. |
| **Derive the electorate from discovery, and make discovery converge first** | Convergence is eventual and unobservable from inside: a node cannot know its view is the view. That is the failure §7 describes, not a fix for it. |
| **Freeze membership of every group** | Discovery's dynamism is the substrate's purpose; work distribution and capability groups need it. The constraint belongs on the decision, not on the group. |
| **Name the electorate as node identities in configuration** | It makes "these nodes are the consensus nodes" a deployment fact — the service shape by another route. The group is the name; which nodes are in it is governance. |
| **Build joint consensus now** | It is a protocol in its own right, and the supported profile already gives a safe answer. Recorded for a later plan (§4.2), not refused. |

## 7. For reviewers

> Discovery membership and the consensus electorate are separate capabilities in Mycelium, by design: discovery is
> dynamic and eventually consistent, and quorum intersection is a property of the electorate, which for any
> safety-sensitive decision is a fixed set for the life of that decision (threat model §7). Today the code refuses an
> empty or below-floor electorate, refuses a proposer outside the group, and refuses a governed group's membership
> change at the gateway except through an audited governance route. Since plan row P2, a node that sets
> `consensus_require_electorate` (required by the `secure-single-domain` profile, rev 4) refuses a lock, a leader
> election or any safety-sensitive proposal unless an **electorate group** decides it — one pinned by member identity
> and epoch, each change a one-member step decided by the electorate before it, acceptors answering only their own
> epoch, and each step draining the epoch it leaves: before a step is proposed, every slot of the group its promise
> quorum reports is run to completion at the old epoch. That gives single-decree safety per slot across any number of
> one-member steps — a value chosen and never committed included — bounded (256 slots a group, 4 KiB a value; past
> it the step is refused by name).
> Versioned electorates with joint-consensus transitions are recorded protocol work for a later plan, and a consensus
> *service* — a fixed tier of nodes everyone must reach to agree — is rejected: consensus is a Layer III protocol run by
> whichever nodes are in the group, never a place.

## 8. P2's design — the electorate group

*Added 2026-10-10 with row P2's implementation; **revised the same day** after #601's adversarial review found the
first design pinned the electorate by member **count**, which a swap or two chained steps defeat. The decision taken
then: **order the electorate's steps** — pin the electorate by identity and epoch, and make every change a consensus
decision of the current electorate. Wire v12, new message variants appended (§8.5).*

### 8.1 What an electorate group is

A group whose electorate is an `ElectorateDecl { group, epoch, members, exclusive_default }` — the member set by
identity, sorted — and whose every epoch is **a consensus decision**, the slot `electorate/{group}/{epoch}`
(`src/agent/electorate.rs`):

- **Genesis** (epoch 1) names the group's roster as its members and is decided by **every** member it names; each
  acceptor checks the set is the roster it sees.
- **A step** (epoch `e` → `e + 1`) adds or removes **one** member and is decided by a strict majority of the
  **epoch-`e`** members. Two concurrent steps are one slot, so at most one commits; a non-member's step is refused at
  its own door and by every acceptor; an acceptor refuses a step that changes more than one member.
- **A certificate** — the deciding votes, signed under `[tls]` — is written to
  `consensus/electorate-cert/{group}/{epoch}` before the commit leaves. A node adopts an epoch only when the record
  follows its chain one member at a time and the certificate holds a strict majority of the previous epoch's members
  (every member, for a genesis). A record that does not is not adopted and is counted
  (`mycelium::electorate_records_refused`): a forged declaration does not change the electorate.

`GossipAgent::declare_electorate(group, exclusive_default)` / `POST /gateway/govern/electorate` (`govern:write`,
audited when a step is decided) proposes the next epoch from the roster this node sees. Changing an electorate is:
move one node (`/gateway/govern/group`), then declare. Between the two the roster differs from the member set and
every proposal to the group is refused — the group decides nothing while it changes.

The electorate is named by the group; which nodes are in it is what the group decided. Nothing in configuration names
a node (D1). Trust slices stay as an optional narrowing beneath the member set; they no longer name it (tension (b)).

### 8.2 What a proposal on an electorate group must satisfy

At the engine's door (`ConsensusEngine::electorate_door`, reached by the library and the gateway alike):

- the proposer is a member of its epoch (`NotAMember` otherwise — a roster member outside the electorate included);
- the group roster equals the epoch's member set **by identity** — otherwise `ElectorateMismatch`, counted on the
  roster tripwire (`mycelium::electorate_roster_mismatches`), never decided by the roster it saw;
- the quorum is at least a strict majority of the members, whatever `quorum_size` asks, and opacity does not reduce
  it (`count_opaque_as_absent` is ignored for an electorate group);
- promises and votes count only from the members, and a refusal (`Promise`, `Nack`) moves the proposal only when a
  member sent it;
- **the slot is this group's to decide**: a `leader/{g}` or `electorate/{g}/…` slot only in group `g`, and a
  fleet-exclusive slot (`lock/`, `consistent/`, `capauthz/`, a commitment award, a consumer-group log claim) only in
  the fleet's exclusive electorate, never by a cross-group proposal (round 3, finding 3) — so two groups never decide
  one `consensus/committed/{slot}`;
- every Prepare and Propose names the proposer's epoch and the electorate's digest (`PrepareIn` / `ProposeIn`).

At every acceptor (`ConsensusEngine::electorate_admits`): it answers only a proposal that names **its own** epoch and
digest, from a member, to a member, for a slot the group may decide — otherwise it refuses with `StaleElectorate`,
naming the electorate it holds. A legacy (untagged) proposal on an electorate group is refused the same way.

**The fence.** An acceptor that has **promised** the step out of epoch `e` (and not had the promise abandoned, §8.4)
no longer answers epoch `e`'s ordinary proposals. Every claim — an acceptor's promise or acceptance, and the
proposer's own — passes a gate on the group's fence key (`ConsensusEngine::claim_gated`). The gate's `compute` only
**decides**; the claim runs after it committed (round 3, finding 5): an ordinary claim registers itself as in flight
on the key, claims, and deregisters; a step's promise raises the fence only when no ordinary claim is in flight. So
every ordinary claim at `e` completed before the raise or is refused after it, a proposer fenced after it passed the
door stops at its next claim (`ElectorateStale`, round 2, finding 1), and an acceptor whose gate refused sends no
promise. A proposer that hears a member name a later electorate stops
too; a member that is behind, or fenced mid-step, simply does not vote.

### 8.3 Exactly what holds — and why: drain before step

**The drain.** Every slot an acceptor answers for on an electorate group is recorded, durably, against **that** group
(`sys/consensus-slot-group/{node}/{group}/{slot}`, one entry per group the slot was proposed in — round 3, finding 4).
A step's promise is answered with a `StepPrepareAck` carrying that acceptor's **drain report**: every **open** slot of
the group — its highest acceptance (ballot, digest, the value: from memory, or, when only the digest survived a
restart, from the slot's commit record) and the ballot its commit was decided at. A slot whose acceptance belongs to a
decision the acceptor sees is over is not open: it is not reported, and its index entry is collected. The report is
taken **before** the promise: an acceptor whose report would exceed the bound (§ below) sends `StepRefused` instead of
promising, so an over-bound step raises no fence. When a quorum of the epoch has promised the step — each promise
fencing its acceptor — the step's proposer **drains**: for every reported slot whose highest acceptance does not belong
to a decision **the proposer itself** sees is over (round 3, finding 6), it runs that slot to completion at the epoch it
is leaving (`DrainPrepare` / `DrainPropose`, answered by fenced members too), so the highest-ballot value is accepted by
a strict majority of that epoch's members. **A committed slot is drained like any other** (round 3, finding 1): the
drain takes no shortcut on a commit it can see — the commit record is gossip, not held by a quorum — so the committed
value (the highest acceptance) is brought to a majority all the same. A drain re-proposes only a reported value and
commits nothing — no COMMIT, no record, no lease renewed; a later proposer adopts the value and commits it on its own
terms. Only then is the step proposed.

**The invariant.** Let `M_f` be epoch `f`'s members. For one slot of the group, crash faults, messages authenticated
under `[tls]`:

> **Inv(f)** — while epoch `f` is current, every value that may have been chosen for the slot is accepted, at a ballot
> no lower than any other acceptance of the slot, by a strict majority of `M_{f-1}` or of `M_f`.

1. **Base.** A value chosen at epoch `f` is accepted by a strict majority of `M_f` (the vote filter and quorum), and the
   prepare phase makes every later acceptance carry it (2.30.0).
2. **No choice outside the epoch.** Once a quorum of `M_f` has promised the step out of `f`, no new epoch-`f` value can
   be chosen except by a drain (an epoch-`f` majority must include a fenced acceptor); a stale proposer is refused on
   its epoch tag; and the fence is checked inside the claim, so no claim slips between door and step (§8.2).
3. **The drain carries it.** The step's promise quorum is a strict majority of `M_f`. By Inv(f) the slot's chosen value
   is held by a majority of `M_{f-1}` or `M_f`; either intersects the promise quorum (sets one member apart), so some
   promiser reports it at the top ballot — committed or not, since a report omits only a slot whose decision that
   acceptor sees is over — and the drain re-runs it at epoch `f` (its phase 1 over a majority of `M_f` adopts it, and
   no commit it can see ends it early) until a majority of `M_f` holds it.
4. **The step.** Epoch `f + 1` is a consensus decision of `M_f`, one member from it; it is proposed only after the drain.
   So when `f + 1` is current a majority of `M_f` holds every value that may have been chosen: Inv(f + 1). A proposer at
   `f + 1` prepares a majority of `M_{f+1}`, which intersects that majority, and adopts it.

So **single-decree safety holds per slot across any number of steps**, each one member, ordered by consensus. That
covers the case the first design did not: **a value chosen and never committed** — its proposer died before the COMMIT,
or the commit never reached the next quorum — is carried by acceptance, not by its commit record. And an acceptor that
has not learned epoch `f + 1` cannot hurt it: it may still answer epoch `f`, but an epoch-`f` quorum needs a fenced
acceptor. Steps themselves are single-decree decisions of the epoch before (two cannot both commit), made only by a
member, adopted only with their certificate. *Test:* `a_value_chosen_two_steps_ago_is_carried_by_the_drain` — `v`
accepted by {A,C} at epoch 1 and never committed, two steps, then a proposer whose quorum is {B,D,E} adopts `v`; and
`a_committed_slot_is_drained_like_any_other` — the same with `v` committed and its record missing on B, D and E.

**The bound, and what happens past it.** An acceptor reports at most 256 **open** slots of one group and at most
4 MiB of values; finished slots do not count and are collected (until #600's acceptor collection lands, this is the
collection). Past the bound the acceptor **does not promise** — it answers `StepRefused` — so the step is refused by
name (`ElectorateError::DrainRefused`, HTTP 409 `drain_refused`, `drain_too_large`) with no fence raised. A slot whose
highest acceptance is known only by its digest, with no commit record to fetch it from (every holder restarted and it
was never committed), refuses the step `drain_blocked`; a slot whose drain does not complete refuses it
`drain_incomplete`. In both cases promises were given, and the proposer releases them (`StepAbort`, §8.4). The
electorate does not move; the group keeps deciding at the old epoch. The cost is one round per open slot per step.
*Tests:* `a_large_committed_value_does_not_wedge_the_group`, `finished_slots_do_not_count_against_the_drain_bound`,
`a_refused_step_releases_its_fence`.

### 8.4 What does not hold — stated

- **The guarantee starts when genesis is chosen.** A member that has accepted the group's genesis treats the group as
  an electorate in formation: it refuses an untagged proposal on it until the genesis is adopted (round 3, finding
  4), so every value chosen after genesis was chosen passed a gate and is indexed. A value accepted before the
  genesis was chosen is not electorate business and is in no report. A genesis that is never decided keeps its
  members refusing untagged proposals on the group until a declaration decides it.
- **A slot whose decision is over is not drained.** An acceptance at or below a decision a member sees has ended (a
  lease expired, a lock released) belongs to that decision, and every proposer's phase 1 sets it aside already; a
  member that has not seen the end may re-commit the old value once — a delay, never a second value.
- **Genesis is unanimous among the members it names, and not ordered against another genesis** naming a disjoint set;
  the second commit for `electorate/{group}/1` is counted by the commit-conflict tripwire.
- **Without `[tls]` nothing is authenticated**, the certificate included: it is a shape check.
- **The way out of a fence** (round 3, finding 2). A step proposer that abandons its ballot — its drain refused — sends
  `StepAbort` for that ballot. A member that **promised** that ballot and accepted nothing at it marks the promise
  abandoned (durably: its `promised_to` becomes a value no proposer holds), so it no longer fences and the ballot's
  accept is refused from now on; a member that **accepted** the step keeps the fence. *Why this is safe:* the
  abandoned ballot can never be accepted by a member that released it, its proposer sends no accept for it, and no
  other proposer holds that ballot (a promise names one proposer), so the step at that ballot is never chosen; a
  later step needs fresh promises, which fence again. Nothing about the drain's argument changes, since the epoch the
  members return to is the one they never left. A step that was **accepted** by some members and then stalled
  (its proposer died after the drain) keeps those members fenced until a member declares again, which completes the
  step slot — adopting what was accepted, or deciding a no-op epoch — and reports it (`200`, `"adopted": true`,
  audited as adopted); the completing step drains again, and since a fenced group chose nothing new, a drain that
  passed once passes again. The one input that can keep it from passing is a value known only by digest everywhere
  (the bullet below).
- **A slot whose highest acceptance is known only by its digest, never committed,** blocks every step (`drain_blocked`)
  until it is decided (a proposal on the slot adopts by digest only if it proposes the same value). The group keeps
  deciding its other slots at the current epoch; only the electorate cannot move.
- **An embedded or peer write to `grp/`** is not prevented; the next proposal refuses the roster it finds.

### 8.5 Wire, mixed fleets and the fleet's exclusive electorate

`PrepareIn`, `ProposeIn`, `StaleElectorate`, `StepPrepareAck`, `DrainPrepare`, `DrainPropose`, `StepRefused` and `StepAbort` are appended to
`ConsensusMsg` (wire v12, as `Prepare` was in 2.30.0): a node predating them ignores them, so an electorate with an
un-upgraded member **times out rather than commits**.

The cluster-scoped exclusive verbs (`distributed_lock`, `LockService`, `consistent_set`, the lock and consistent routes,
the log claim, `mycelium-commitment`'s linearizable award, `set_capability_authz_via_consensus`) decide in the group
whose current epoch is marked `exclusive_default` — a fleet record — or across the cluster when none is.
`consensus_electorate` may only restate it. Every acceptor refuses a cluster-scoped proposal for any exclusive slot —
`lock/`, `leader/`, `consistent/`, `capauthz/`, a commitment award — once it knows a group is marked (round 2,
finding 2b), and a group-scoped one on any group but the marked one (2c).

**Why the marking is not itself a cluster-scoped consensus decision (round 2, finding 2a).** A node that has not yet
learned the mark can still propose, and answer, a cluster-scoped exclusive slot, so while the mark propagates a lock
can be decided across the cluster and in the marked group. Deciding the mark by a cluster-scoped slot whose acceptors
are fenced from cluster-scoped exclusive slots would close that window only as far as two cluster majorities
intersect — and the cluster is discovery, which this record says is never an electorate (§2.1). The window is closed by
the supported profile instead: with `consensus_require_electorate` on every node, no node proposes a cluster-scoped
exclusive slot at all (`ElectorateNotGoverned`), before the mark or after it, so there is never a second electorate for
one slot. A node off the profile is outside the claim; the rolling-upgrade note says to mark a group only once every
node runs this release, and with the requirement on.

### 8.6 Requirement, guarantee, profile

`consensus_require_electorate` refuses a safety-sensitive proposal (flagged, or in `lock/`, `leader/`, `consistent/`,
`capauthz/`, `electorate/`, or a commitment award) outside an electorate group, `ElectorateNotGoverned`.
`cons.safety_profile` (rev 2) resolves on it; `secure-single-domain` rev 4 requires it. A cross-group proposal is never
an electorate: one naming an electorate group is refused `ElectorateMismatch`, and a safety-sensitive one
`ElectorateNotGoverned` under the requirement.
