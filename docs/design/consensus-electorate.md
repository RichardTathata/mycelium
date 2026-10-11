# Discovery is not an electorate (ADR, post-360 hardening D1)

**Status:** **adopted** 2026-10-10. Records a decision; changes no code. Plan of record:
`docs/plans/post-360-hardening.md` (PR #593) §2 and decision D1, rows **P2**, **C1**, **C2** and **Φ**
(**P2 built** 2026-10-10, §8; **C1, C2 delivered** in PR #600; Φ not built). It builds on the threat model's supported profile
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
  (§8): the supported profile names an **electorate group**, whose member set is a decision of the group itself, frozen
  once decided (a change is a migration to another group); a trust slice (`declare_trust`) stays available beneath it as an optional narrowing, no longer the
  profile's voter set.
- **No deployment shape, configuration default or document may make a named node set the place agreement happens.**
  Whichever nodes participate in a group run the protocol; roles form per ballot and dissolve when the decision
  completes; the state is ordinary keys and signals; Layers I and II know nothing of it.
- **Roles and outputs decay.** Leadership is leased by default and acceptor state is collected once its decision is
  over (rows C1, C2 — **delivered**, PR #600). `elect_leader` commits on a 30 s lease (`DEFAULT_LEADER_LEASE`,
  `src/agent/overlay_consistent.rs`), renewed by calling again; `release_leadership` steps down; permanence is
  `LeaderTerm::Permanent`, by explicit opt-in. Each consensus listener's collector (`run_acceptor_collector`,
  `src/consensus.rs`) collects acceptor state whose decision ended at a ballot `e` and that promises nothing above
  `e`, after the decided floor is on stable storage at `e` — the exact condition is in
  [runtime-invariants](../wiki/dev/architecture/runtime-invariants.md). A permanent decision's state is kept.

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
| **An electorate is pinned by identity, and frozen.** A proposal to an electorate group is refused `ElectorateMismatch` unless the roster equals the member set by identity (a swap is refused, counted on the roster tripwire); declaring a changed roster is refused `MembershipFrozen`; promises and votes count only from the members; the quorum is at least a strict majority, opacity and `quorum_size` notwithstanding | `ConsensusEngine::electorate_door`, `electorate_vote_filter`, `src/consensus.rs`; `electorate::declare_outcome` | `a_swap_without_a_migration_is_refused`; `votes_from_outside_the_electorate_are_not_counted`; `the_roster_is_the_vote_filter_and_a_trust_slice_only_narrows_it` |
| **Acceptors answer only their own electorate.** `PrepareIn`/`ProposeIn` name the electorate's epoch and digest; an acceptor holding another refuses `StaleElectorate` | `ConsensusEngine::electorate_admits`, `refuse_stale`, the listener, `src/consensus.rs` | `an_undecided_genesis_is_superseded_by_one_naming_the_live_roster` |
| **A genesis is a consensus decision of every member it names**, certified by their votes; one that cannot complete is superseded by one naming a strict majority of it; a forged record is not adopted | `electorate::declare_outcome`, `check_genesis`; `ConsensusEngine::electorate_view`, `genesis_acceptable`, `certificate_holds` | `an_undecided_genesis_is_superseded_by_one_naming_the_live_roster`; `a_forged_electorate_record_is_not_adopted`; `an_electorate_certificate_verifies_under_tls`; `a_genesis_supersedes_only_the_epoch_before_it_with_a_majority_of_it` |
| **Membership change is migration, predecessor first.** The successor is declared first, naming its predecessors, and decides nothing until each has retired to it (door and acceptor, fail closed); a retirement — a decision of a majority — is accepted only into a successor that names the retiree, and closes the group at its promise; the successor carries each slot from a majority of every predecessor until it has decided it | `electorate::declare_outcome`, `retire_outcome`; `ConsensusEngine::successor_ready`, `successor_names`, `claim_gated`, `retirement_view`, `predecessor_chain`, `prepare_phase` (the carry), `answer_carry` | `a_value_chosen_in_a_retired_group_is_carried_by_its_successor`; `a_successor_decides_nothing_until_its_predecessors_retirement_is_visible`; `a_merge_carries_every_predecessor`; `a_successor_waiting_on_a_predecessor_that_never_retires_has_an_escape`; `two_concurrent_retirements_decide_one_successor_and_a_non_member_cannot_retire`; `a_proposer_closed_mid_ballot_does_not_complete`; `a_close_waits_for_an_ordinary_claim_in_flight`; `a_refused_claim_through_the_gate_lowers_the_fence_again`; `a_retirement_completes_a_pending_one` |
| **Nothing resizes an electorate group.** The governor does not roll on it; the emergent watcher defers to it; `/gateway/mesh/group` refuses it **403** `governed_group` | `membership_governor::converge`; `emergent_groups::governor_owned_groups`; `is_governed_group`, `src/agent/http.rs` | `the_governor_and_the_watcher_leave_an_electorate_group_alone` |
| **The fleet's exclusive electorate is a fleet record.** The cluster-scoped exclusive verbs decide in the live default: declared once (a second refused while one holds it), moved only by retirement into the successor whatever its own flag; a disagreeing `consensus_electorate` is refused | `electorate::live_default`, `exclusive_electorate`, `holds_default`; `exclusive_propose`, `src/agent/consensus_handle.rs`; `overlay_exclusive_propose`, `src/agent/http.rs` | `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused`; `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`; `a_value_chosen_in_a_retired_group_is_carried_by_its_successor`; `a_second_live_default_is_refused_by_name` |
| **A declaration retries an undecided attempt**, with a bounded backoff, on a sparse mesh too | `electorate::retrying` | `a_declaration_on_a_sparse_mesh_retries_until_decided` |
| **A cross-group proposal does not decide for an electorate group** | `electorate_door` (`Groups`) | `a_cross_group_proposal_does_not_decide_for_an_electorate_group` |
| **The secure profile checks it.** `cons.safety_profile` (rev 2) resolves `enforced` on `consensus_require_electorate`; `secure-single-domain` rev 4 requires it | `src/agent/guarantee.rs` | `the_consensus_safety_profile_resolves_on_the_electorate_requirement`; `the_secure_profile_refuses_an_open_node_by_name` |

**What is not enforced — the residual, stated exactly** (as recorded before P2; each bullet now says what P2 changed):

- **The governance boundary is the gateway's.** An embedded caller's `mesh().join_group` (`mycelium-core/src/mesh_handle.rs:100-107`)
  or an embedded KV write to `grp/{group}/{node}` still moves a governed group's membership, and a peer's write to
  `grp/` is accepted by LWW like any other — detection, not prevention (the philosophy 360 finding Φ3). *Since P2:*
  still true, and now **detected**: for an electorate group, a roster that differs from its member set refuses
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
  that does not evaporate, the governor and the watcher leave it alone, and its membership is **frozen**: a change is
  a migration to another group (§8).
- **Today's profile names its voter set by node identity.** `declare_trust(group, &[NodeId])` is how §7's eligible set
  is stated. D1 says the electorate is *named* as a governed group; reconciling the two is part of P2, and this record
  does not decide whether trust slices remain the vote filter underneath. *Since P2:* decided — the electorate's
  member set, decided by the group, is the vote filter; a trust slice may narrow it, and no longer names it (§8).

## 4. What is planned, and what is a later plan

### 4.1 In the post-360 plan — P2 *built* (§8)

| Row | Promise (quoted from the plan) | State |
|---|---|---|
| **P2** | "safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities" | *not built* |
| **C1** | "`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)" | delivered (#600): `DEFAULT_LEADER_LEASE`, `release_leadership`, `LeaderTerm::Permanent`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}` |
| **C2** | "acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on" | delivered (#600): `ConsensusEngine::collect_finished`, bounded per tick, on its own task |

### 4.2 A later plan — *not built, not scheduled*

**Versioned electorates — live reconfiguration** — membership epochs carried on every ballot, and a transition
during which a proposal must win a quorum of the old electorate *and* of the new (joint consensus), so an electorate's
members can change under a live slot. Recorded as protocol work for a later plan under D1 (the post-360 plan's §5). P2
does not build it: rounds 2 and 3 of #601 built one-member steps with a drain before each step, and round 4 found a
race between the drain's report and the step's fence and a bound that could hold a fleet electorate for good; the
maintainer decided (2026-10-11) to **freeze membership** and change it only by migration (§8.3). Live reconfiguration
would not change §2.1 — discovery would still not be an electorate, and the epoch would still name a group, not a node
set.

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
| **One-member steps with a drain before each step** (built in #601's rounds 2–3, withdrawn 2026-10-11) | Round 4 found the drain's report could be taken before the step's fence was raised, so a value chosen in between was not carried; and a group with more open slots or bytes than the drain carries could never step, so a dead member of a fleet electorate could never be removed. Migration carries lazily, slot by slot, with no bound, and its reports are read only after the retirement is decided (§8.3). |

## 7. For reviewers

> Discovery membership and the consensus electorate are separate capabilities in Mycelium, by design: discovery is
> dynamic and eventually consistent, and quorum intersection is a property of the electorate, which for any
> safety-sensitive decision is a fixed set for the life of that decision (threat model §7). Today the code refuses an
> empty or below-floor electorate, refuses a proposer outside the group, and refuses a governed group's membership
> change at the gateway except through an audited governance route. Since plan row P2, a node that sets
> `consensus_require_electorate` (required by the `secure-single-domain` profile, rev 4) refuses a lock, a leader
> election or any safety-sensitive proposal unless an **electorate group** decides it — one pinned by member identity,
> decided by every member it names, and **frozen**: acceptors answer only their own electorate, and a change of members
> is a **migration** — the group retires to a successor by a decision of its majority, closed at the retirement's
> promise, and the successor carries each slot from a majority of the retired group the first time it touches it. That
> gives single-decree safety per slot across a migration — a value chosen and never committed included — with no bound
> on the slots carried.
> Versioned electorates with live (joint-consensus) transitions are recorded protocol work for a later plan, and a consensus
> *service* — a fixed tier of nodes everyone must reach to agree — is rejected: consensus is a Layer III protocol run by
> whichever nodes are in the group, never a place.

## 8. P2's design — the electorate group

*Added 2026-10-10 with row P2's implementation; revised the same day after #601's adversarial review found the first
design pinned the electorate by member **count**, which a swap or two chained steps defeat; revised again in rounds 2
and 3, which built **live one-member steps with a drain before each step**. Round 4 found that design still unsafe (a
race between taking the drain report and raising the fence) and unable to move a fleet electorate that had decided more
than its bound. **The maintainer's decision (2026-10-11): freeze membership.** An electorate's members never change
once adopted; changing who decides is a **migration** to another group. Live reconfiguration is not built (§4.2). Wire
v12, new message variants appended (§8.6).*

### 8.1 What an electorate group is

A group whose electorate is an `ElectorateDecl { group, epoch, members, exclusive_default, supersedes, predecessors }` —
the member set by identity, sorted — decided by **consensus** on the slot `electorate/{group}/{epoch}`
(`src/agent/electorate.rs`):

- **Genesis** names the group's roster as its members and is decided by **every** member it names; each acceptor checks
  the set is the roster it sees. The **epoch counts genesis attempts**: 1, or one more than a pending genesis it
  supersedes (§8.4). Nothing follows a genesis: once adopted, the electorate is **frozen**.
- **A certificate** — the decided decision envelope (row A, PR #600: the value Paxos decides, whose `value` is the
  record) and the deciding votes for it, signed under `[tls]` — is written to `consensus/electorate-cert/{group}/{epoch}`
  before the commit leaves. A node adopts a genesis only when its certificate holds a vote from every member it names,
  at one ballot. A record that does not verify is not adopted and is counted (`mycelium::electorate_records_refused`):
  a forged declaration does not change the electorate.
- **A successor** names its **predecessors** (a set; two or more is a merge) in its genesis — declared *before* they
  retire (§8.3).
- **A retirement** (`Retirement { group, epoch, successor }`, slot `electorate/{group}/retire`, certificate
  `…/{group}/retire`) is a decision of a strict majority of the group's members, naming a **successor** that already
  names it. It is the only way a group's electorate stops deciding (§8.3).

`GossipAgent::declare_electorate(group, exclusive_default)` / `POST /gateway/govern/electorate` (`govern:write`,
audited when something is decided) proposes a genesis from the roster this node sees. On a group that has an
electorate it decides nothing: it returns the electorate when the roster still matches, and
`ElectorateError::MembershipFrozen` (HTTP 409 `membership_frozen`) when it does not.
`GossipAgent::declare_successor_electorate(group, predecessors)` / `{"group", "predecessors": [...]}` declares a
successor; `GossipAgent::retire_electorate(group, successor)` / `{"group", "retire_to"}` retires a group into one. All
retry an undecided attempt with a bounded backoff — eight tries, 250 ms doubling to 4 s (round 4, finding 8) — and
return `NotDecided` only after it; a refusal no retry changes — a door refusal, a second default, a successor that does
not name the retiree — is `Refused` at once, and a retry that finds the retirement already decided reports it (round 5,
Q3).

The electorate is named by the group; which nodes are in it is what the group decided. Nothing in configuration names
a node (D1). Trust slices stay as an optional narrowing beneath the member set; they no longer name it (tension (b)).

### 8.2 What a proposal on an electorate group must satisfy

At the engine's door (`ConsensusEngine::electorate_door`, reached by the library and the gateway alike):

- the proposer is a member (`NotAMember` otherwise — a roster member outside the electorate included);
- the group is not **closed** — retired, or its retirement promised by this node (`ElectorateMismatch`, naming the
  successor when the retirement is known);
- the group roster equals the member set **by identity** — otherwise `ElectorateMismatch`, counted on the roster
  tripwire (`mycelium::electorate_roster_mismatches`), never decided by the roster it saw (a retirement is exempt: the
  roster moving away is what it is for);
- the quorum is at least a strict majority of the members (every member, for a genesis), whatever `quorum_size` asks,
  and opacity does not reduce it;
- promises and votes count only from the members, and a refusal moves the proposal only when a member sent it;
- **the slot is this group's to decide**: a `leader/{g}` or `electorate/{g}/…` slot only in group `g`, and a
  fleet-exclusive slot (`lock/`, `consistent/`, `capauthz/`, a commitment award, a consumer-group log claim) only in
  the fleet's exclusive electorate, never by a cross-group proposal — at the proposer's door and at every acceptor, so
  two groups never decide one `consensus/committed/{slot}`;
- every Prepare and Propose names the electorate's epoch and digest (`PrepareIn` / `ProposeIn`).

At every acceptor (`ConsensusEngine::electorate_admits`): it answers only a proposal naming **its own** electorate, from
a member, for a slot the group may decide — otherwise `StaleElectorate`, naming the electorate it holds. A legacy
(untagged) proposal on an electorate group is refused the same way, and so is one on a group whose genesis this node
has touched but not adopted (an electorate in formation).

**The gate.** Every claim on an electorate group — an acceptor's promise or acceptance and the proposer's own, a
retirement's, a genesis attempt's, an adoption, and a carry report — passes one `compute` on the group's key
(`ConsensusEngine::claim_gated`, `FenceState`). The `compute` only **decides**; the claim runs after it committed:

- an **ordinary** claim passes while the group is open and registers itself as **in flight**; it deregisters when done;
- a **retirement's** claim (and a carry report, §8.3) **closes** the group only when no ordinary claim is in flight,
  then claims; afterwards the key keeps only what is durable — the acceptor's promise or acceptance of the retirement,
  or the verified retirement — so a claim the acceptor refused, having promised nothing, leaves the group open
  (round 4, finding 4);
- a **genesis** claim passes while this node has adopted no electorate for the group and touched no later attempt,
  registered as *opening*; an adoption waits out an opening claim and is refused when this node touched a later
  attempt.

So every ordinary claim either completed before the close or is refused after it, a proposer closed after it passed
the door stops at its next claim (`ElectorateMismatch`, naming the close), and an acceptor whose gate refused sends no
promise.

### 8.3 Membership change is migration, predecessor first — and why it is safe

*Revised in round 5 (2026-10-11) after #601's review of 98ff2a93 found the round-4 order — retire, then declare the
successor naming the retirement — unsafe under gossip lag: a successor declared by members that had not yet received
the retirement formed without a predecessor and carried nothing (D1), a successor declared the default and then
retired into gave one lock two values (D1), the exclusive verbs could wedge for good (D2), and a merge carried only
one predecessor (D3). The order is now reversed, so lag can only **delay** the carry, never skip it.*

**The procedure** (guide 04 has the calls). To move from `G1` (and, for a merge, `G3`, …) to a group with other members:

1. **Declare the successor first**, naming its predecessors as a set: `declare_successor_electorate("G2", &["G1"])`.
   Its genesis is decided by every member it names, as any genesis is; each member checks that every named
   predecessor is an electorate it can see and is not retired to another group. **G2 decides nothing** — no slot,
   ordinary or exclusive, at the proposer's door or at any acceptor — until every named predecessor's verified
   retirement **naming G2** is visible there (`successor_ready`). A proposer is refused by name (`ElectorateMismatch`,
   "waiting for its predecessor …"); an acceptor does not answer and counts it (`electorate_waiting_refusals`). Fail
   closed. G2's own electorate slots (its genesis, its retirement) are exempt, so G2 can always retire (§ escapes).
2. **Retire each predecessor into it**: `retire_electorate("G1", "G2")` — a decision of a strict majority of G1. The
   proposer and every G1 acceptor accept it only if **G2's decided genesis is visible to it and names G1**; not visible
   yet is gossip (the call retries), visible and not naming G1 is refused by name. Every G1 member closes G1 at the
   retirement's **promise**.
3. **G2 carries lazily.** A G2 proposer's phase 1 for a slot — unless this node has seen G2 decide it, or the slot names
   a group — also sends `CarryPrepare` to the members of **every** predecessor G2 names (and, transitively, of each of
   their predecessors that retired to them), and needs a report from a strict majority of each, besides G2's promise
   quorum. A retired member answers only for its retired group, only when the retirement verifies there, only to a
   proposer in a successor: it closes the group through its gate, puts the retirement on stable storage, and reports its
   acceptor memory for the slot, read-only. The proposer moves above any reported promise at or above its ballot and
   adopts the **highest acceptance across all the reports**. Once this node has seen G2 decide the slot — its own commit
   or a learned one — it stops asking (in memory: after a restart it carries again, which is safe).

**The ordering argument.** Every G2 decision needs, at its proposer and at a quorum of G2's acceptors, the verified
retirement of each predecessor **to G2**; a retirement of G1 to G2 can be decided only by G1 members that see G2's genesis
naming G1. So the facts arrive in one order — G2's genesis, then G1's retirement to it, then G2's first decision — and
each step is checked by the nodes that take the next one. Gossip lag can hold a step back (a member that has not
received the record refuses, and the operation is retried), but it cannot make a step happen without the one before it:
there is no state in which G2 decides while a predecessor it names is still deciding, and no retirement that names a
successor which does not carry from it. That is what the round-4 order lacked — there the successor's genesis was
checked against whatever retirement its members happened to have seen.

**Why no value is lost** (one slot `s` decided only in the electorate — an exclusive slot, or one that names its group;
crash faults; messages authenticated under `[tls]`):

1. **After G1's retirement is chosen, no G1 value can be chosen.** The retirement was promised by a strict majority `P`
   of G1, each of which closed G1 at its promise, durably; a later G1 majority would need a closed member, and the gate
   orders a close after every ordinary claim in flight.
2. **G2's carry reads only after the retirement is decided**: G2 runs no phase 1 until the retirement is verified at the
   proposer (step 1), and each reporter closes G1 and persists the retirement before it reports. A reporter outside `P`
   closes G1 at its report, through the same gate.
3. **A value chosen in G1 is in the carry**: chosen at ballot `b` means a strict majority `M` of G1 accepted it; the
   carry reads a strict majority `R` of G1; a member of `R ∩ M` reports an acceptance at a ballot `≥ b`, which by G1's own
   phase 1 is the chosen value.
4. **G2's ballots end up above every G1 ballot that matters** (a reporter promised any G1 ballot before its report, and
   the proposer moves above it), so re-reading G1 after a restart finds nothing that outranks a G2 decision.
5. **Within G2**, by induction over its ballots: each proposer before G2 decides `s` carries and proposes the chosen
   value; after, G2's own phase 1 finds it. A member of both groups has one acceptor memory; its G2 promise reports its
   G1 acceptance too.
6. **Merges and chains.** G2 carries from **every** predecessor it names — each was required to retire to it before G2
   decided anything — so a merge of G1 and G3 carries both. A chain carries transitively through predecessors that
   retired to their successor; a predecessor's own predecessor that never retired to it is not read, because that
   predecessor decided nothing (it waited), and the group that did not retire is still deciding on its own.

**The claim is for slots decided only in the electorate** (round 5, Q1). An ordinary slot is not homed: a legacy or
untagged proposal under another scope (the whole cluster, another group) can reach G1's members as plain acceptors,
outside G1's gate, and this design does not order those acceptances against the retirement. For an ordinary slot used
only through the electorate group, the argument above holds as written; for one also proposed under other scopes it is
not claimed.

**Liveness, stated.** Migrating needs a reachable strict majority of each predecessor: to retire it, and for the
successor's first touch of each slot. Losing a majority of an electorate is unrecoverable, here and in any consensus
protocol.

**Escapes from every wait** (round 5, D2):

- *A successor waits on a predecessor that never retires* (it named one by mistake, or one whose majority is gone): G2
  decides nothing, but its own retirement is exempt from the wait. Declare `G4` naming G2, retire G2 into it. G4 carries
  from G2 and from each predecessor that did retire to G2 (and holds the default if one of them held it).
  *Test:* `a_successor_waiting_on_a_predecessor_that_never_retires_has_an_escape`.
- *A retirement promised by a majority whose proposer died* closes the group until a member completes it:
  `retire_electorate` from any member adopts what was accepted, or decides its own; an attempt that ended undecided after
  this node promised it is counted (`electorate_retirements_undecided`) and, at the gateway, audited.
  *Test:* `a_retirement_completes_a_pending_one`.
- *Two live defaults* (§8.6): merge them — declare one successor naming both, retire both into it.

*Tests:* `a_value_chosen_in_a_retired_group_is_carried_by_its_successor` (the carry, the default following the
retirement, and the skip — E decides a leased slot again with `old`'s majority gone; it fails with the skip removed),
`a_successor_decides_nothing_until_its_predecessors_retirement_is_visible` (D1: the retirement not yet at D and E —
D's door refuses, E does not answer, then the carry), `a_second_live_default_is_refused_by_name` (D1's default
interleaving), `a_merge_carries_every_predecessor` (D3, and a retirement to a successor that does not name the retiree
refused), `a_close_waits_for_an_ordinary_claim_in_flight`.

### 8.4 A genesis that cannot complete is superseded (round 4, finding 3)

A genesis is decided by **every** member it names, so a genesis accepted by some members and never by one that is gone
would hold its group in formation for good. A declaration from a member that holds it **supersedes** it: a genesis at
the next epoch, naming the live roster, with `supersedes = { epoch, digest, members }` of the pending one.

- **Its rule** (`check_genesis`): the superseding genesis names a **strict majority** of the one it supersedes, and is
  the epoch after it.
- **Its acceptance**: each member checks, besides the roster, that it has not adopted any electorate for the group,
  has touched no later attempt, and — when the superseded genesis names it — that it accepted exactly that genesis.
  It accepts one value per attempt.
- **Its adoption**: through the gate — a node never adopts an attempt below one its acceptor touched.

**Why at most one attempt is ever in force** (adopted by a strict majority of its members, so able to decide): a
superseding genesis at epoch `k + 1` is decided by every member it names, which includes a strict majority of the
attempt `T` at epoch `k`; each of those promised epoch `k + 1` before accepting it, so none had adopted `T` (adopted
nodes refuse) and none ever will (a later touch refuses the adoption). So `T` never has a majority of adopters. Earlier
attempts are out of force by induction: the members of `k + 1` named by `T` each **accepted** `T`, so `T`'s proposer
completed its phase 1 — every member of `T`, a strict majority of attempt `k - 1` among them, promised epoch `k` — so
`k - 1` never has a majority of adopters either, and so on down. The members that **accepted** an attempt and never
adopted it release it when another is chosen: they adopt the later one, and leave formation. The argument assumes, as
genesis always has, that the values proposed for one attempt name one roster (§8.5). *Test:*
`an_undecided_genesis_is_superseded_by_one_naming_the_live_roster` — {A,B,C} accepted by A and B, C gone; the
declaration of {A,B} decides epoch 2, and a proposal naming epoch 1 is refused, naming epoch 2.

**What "adopted" rests on after a restart** (round 5, Q2). The gate's `adopted` is held in memory; after a restart it
is re-derived from the committed genesis record and its certificate, which reach a node by gossip and are WAL-appended
without an fsync of their own. A member that adopted attempt `k`, then lost those records in a crash before they reached
its disk, could — until gossip re-supplies them — take part in a superseding attempt `k + 1` that it would otherwise
refuse. Its acceptor memory (fsynced) still records that it accepted `k`, so it can only support a supersession of the
attempt it holds; the argument above then needs every other adopter of `k` to refuse, which holds unless a majority of
`k`'s members lost the records at once. Stated, not closed: persisting adoption would need a durable record per member.

### 8.5 What does not hold — stated

- **The guarantee starts when genesis is chosen.** A member that has touched a genesis attempt refuses an untagged
  proposal on the group until one is adopted.
- **Genesis assumes its members agree on the roster.** Each acceptor checks a genesis against the roster it sees, and
  accepts one value per attempt; two values for one attempt naming different rosters — a membership view split while
  a genesis is declared — are not ordered against each other, and could each be decided by members that do not
  overlap. The second commit for one slot is counted by the commit-conflict tripwire. The roster is the governed
  group's, moved only through the governance route: declare a genesis once the roster is settled.
- **Two default declarations within gossip lag.** A genesis declaring the default is refused, at the door and at
  every acceptor, while any group holds it (§8.6) — but two groups declared the default by members that had not yet
  seen each other's genesis can both be decided. Until a node sees both it may decide an exclusive slot in either; once
  it sees both, every exclusive verb is refused by name (fail closed, never decided across the cluster), and the escape
  is a merge (§8.6). Declare the default once, and only when every node can see the fleet's records.
- **Ordinary slots under other scopes** are outside §8.3's claim (Q1 there).
- **Without `[tls]` nothing is authenticated**, the certificates and the carry reports included.
- **A forged commit record stops a proposer.** A `consensus/committed/electorate/…` entry written straight into the
  store is not adopted, but a proposer for that slot reads it as decided and ends `Superseded`, as for any slot — so a
  member writing one can hold a genesis attempt or a retirement up (liveness, not safety; counted on
  `electorate_records_refused`). Removing the forged entry, or a genesis at the next attempt, moves on.
- **An embedded or peer write to `grp/`** is not prevented; the next proposal refuses the roster it finds — and since
  membership is frozen, such a group decides nothing until it is migrated or the write is undone.

### 8.6 Wire, mixed fleets and the fleet's exclusive electorate

`PrepareIn`, `ProposeIn`, `StaleElectorate`, `CarryPrepare` and `CarryReport` are appended to `ConsensusMsg` (wire v12,
as `Prepare` was in 2.30.0): a node predating them ignores them, so an electorate with an un-upgraded member **times
out rather than commits**. `PrepareIn` and `ProposeIn` are row A's `PrepareTerm` and `ProposeTerm` naming an electorate:
the value is the decision envelope, and they are answered with `PrepareAckTerm` and `PromiseTerm`; a carry report
flags an envelope the same way.

The cluster-scoped exclusive verbs (`distributed_lock`, `LockService`, `consistent_set`, the lock and consistent routes,
the log claim, `mycelium-commitment`'s linearizable award, `set_capability_authz_via_consensus`) decide in the **live
default** (`electorate::live_default`) — a fleet record — or across the cluster when no group ever declared one:

- **Declared once.** A genesis may set `exclusive_default` only while no group holds the default — refused by name
  otherwise, at the door and at every acceptor (round 5, D1).
- **Moved only by retirement.** A group **holds** the default if it declared it, or if a predecessor that held it
  retired to it — whatever the successor's own flag says (the round-4 coupling between the flag and the retirement is
  gone). The live default is the one holder that is not retired.
- **Fail closed.** While a holder has retired to a successor this node cannot see yet, or two live groups hold it, every
  exclusive verb is refused by name, and every acceptor refuses a cluster-scoped proposal for an exclusive slot —
  tagged or not (round 5, D5). `consensus_electorate` may only restate the answer. An acceptor also refuses a
  group-scoped proposal for an exclusive slot on any group but the live default.

**No persisting state with zero or two defaults** (round 5, D2). *Zero:* before any declaration there is no default,
and the verbs decide across the cluster (off the secure profile) or are refused (on it). After the first declaration,
holding is conserved: a holder stops being live only by a retirement, and a retirement names a successor already
declared, naming the retiree — so the successor holds the default from the moment the retirement is visible. The only
zero a node can see is a retirement whose successor has not reached it yet: transient, refused meanwhile, ended by
gossip. A successor still waiting for another predecessor holds the default but decides nothing; its escape is §8.3's
(retire it into a successor naming it). *Two:* only by two default declarations within gossip lag (§8.5); retirements
cannot create a second, since a successor holds only what its predecessors held and a merge of holders is one group.
Two persist until the escape: **merge them** — declare one successor naming both, retire both into it; it holds the
default once, and its carry reads both, so a slot decided in each window is carried at its highest acceptance.

**Why the marking is not itself a cluster-scoped consensus decision (round 2, finding 2a).** A node that has not yet
learned the mark can still propose, and answer, a cluster-scoped exclusive slot, so while the mark propagates a lock
can be decided across the cluster and in the marked group. Deciding the mark by a cluster-scoped slot would close that
window only as far as two cluster majorities intersect — and the cluster is discovery, which this record says is never
an electorate (§2.1). The window is closed by the supported profile instead: with `consensus_require_electorate` on
every node, no node proposes a cluster-scoped exclusive slot at all (`ElectorateNotGoverned`), before the mark or after
it. A node off the profile is outside the claim; the rolling-upgrade note says to mark a group only once every node
runs this release, and with the requirement on.

### 8.7 Requirement, guarantee, profile

`consensus_require_electorate` refuses a safety-sensitive proposal (flagged, or in `lock/`, `leader/`, `consistent/`,
`capauthz/`, `electorate/`, or a commitment award) outside an electorate group, `ElectorateNotGoverned`.
`cons.safety_profile` (rev 2) resolves on it; `secure-single-domain` rev 4 requires it. A cross-group proposal is never
an electorate: one naming an electorate group is refused `ElectorateMismatch`, and a safety-sensitive one
`ElectorateNotGoverned` under the requirement.
