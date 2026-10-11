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
  at genesis (changing it is not built in this release, §8.3); a trust slice (`declare_trust`) stays available beneath it as an optional narrowing, no longer the
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
| **Nothing resizes an electorate group.** The governor does not roll on it; the emergent watcher defers to it; `/gateway/mesh/group` refuses it **403** `governed_group` | `membership_governor::converge`; `emergent_groups::governor_owned_groups`; `is_governed_group`, `src/agent/http.rs` | `the_governor_and_the_watcher_leave_an_electorate_group_alone` |
| **The fleet's exclusive electorate is a fleet record.** The cluster-scoped exclusive verbs decide in the default: set once, at genesis (a second refused by name while one is held, at the door and every acceptor); a disagreeing `consensus_electorate` is refused; a cluster-scoped exclusive slot is refused, tagged or not, while the default cannot be named | `electorate::live_default`, `exclusive_electorate`; `exclusive_propose`, `src/agent/consensus_handle.rs`; `overlay_exclusive_propose`, `src/agent/http.rs` | `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused`; `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`; `a_second_default_is_refused_by_name`; `a_tagged_cluster_proposal_for_an_exclusive_slot_fails_closed` |
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
  that does not evaporate, the governor and the watcher leave it alone, and its membership is **fixed at genesis**;
  changing it is not built in this release (§8.3).
- **Today's profile names its voter set by node identity.** `declare_trust(group, &[NodeId])` is how §7's eligible set
  is stated. D1 says the electorate is *named* as a governed group; reconciling the two is part of P2, and this record
  does not decide whether trust slices remain the vote filter underneath. *Since P2:* decided — the electorate's
  member set, decided by the group, is the vote filter; a trust slice may narrow it, and no longer names it (§8).

## 4. What is planned, and what is a later plan

### 4.1 In the post-360 plan — P2 *built* (§8)

| Row | Promise (quoted from the plan) | State |
|---|---|---|
| **P2** | "safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities" | delivered in 2.32.0 (#601), **without migration**: an electorate group's members are fixed at genesis and changing them is not built (§8.3; the follow-up is branch `fix/p2-migration`) |
| **C1** | "`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)" | delivered (#600): `DEFAULT_LEADER_LEASE`, `release_leadership`, `LeaderTerm::Permanent`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}` |
| **C2** | "acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on" | delivered (#600): `ConsensusEngine::collect_finished`, bounded per tick, on its own task |

### 4.2 A later plan — *not built, not scheduled*

**Versioned electorates — live reconfiguration** — membership epochs carried on every ballot, and a transition
during which a proposal must win a quorum of the old electorate *and* of the new (joint consensus), so an electorate's
members can change under a live slot. Recorded as protocol work for a later plan under D1 (the post-360 plan's §5). P2
does not build it: rounds 2 and 3 of #601 built one-member steps with a drain before each step, and round 4 found a
race between the drain's report and the step's fence and a bound that could hold a fleet electorate for good; the
maintainer decided (2026-10-11) to **freeze membership**; a migration to another group was built in rounds 4–5 and,
by the maintainer's decision the same day, ships later (branch `fix/p2-migration`, §8.3). Live reconfiguration
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
| **One-member steps with a drain before each step** (built in #601's rounds 2–3, withdrawn 2026-10-11) | Round 4 found the drain's report could be taken before the step's fence was raised, so a value chosen in between was not carried; and a group with more open slots or bytes than the drain carries could never step, so a dead member of a fleet electorate could never be removed. Membership was frozen instead, and a migration designed as the follow-up (§8.3). |

## 7. For reviewers

> Discovery membership and the consensus electorate are separate capabilities in Mycelium, by design: discovery is
> dynamic and eventually consistent, and quorum intersection is a property of the electorate, which for any
> safety-sensitive decision is a fixed set for the life of that decision (threat model §7). Today the code refuses an
> empty or below-floor electorate, refuses a proposer outside the group, and refuses a governed group's membership
> change at the gateway except through an audited governance route. Since plan row P2, a node that sets
> `consensus_require_electorate` (required by the `secure-single-domain` profile, rev 4) refuses a lock, a leader
> election or any safety-sensitive proposal unless an **electorate group** decides it — one pinned by member identity,
> decided by every member it names, and **fixed at genesis**: acceptors answer only their own electorate, and changing
> its members is not built in this release (the migration is the follow-up). A dead member lowers the group's fault
> tolerance; a lost majority stops it.
> Versioned electorates with live (joint-consensus) transitions are recorded protocol work for a later plan, and a consensus
> *service* — a fixed tier of nodes everyone must reach to agree — is rejected: consensus is a Layer III protocol run by
> whichever nodes are in the group, never a place.

## 8. P2's design — the electorate group

*Added 2026-10-10 with row P2's implementation; revised the same day after #601's adversarial review found the first
design pinned the electorate by member **count**, which a swap or two chained steps defeat; revised again in rounds 2
and 3, which built **live one-member steps with a drain before each step**. Round 4 found that design still unsafe (a
race between taking the drain report and raising the fence) and unable to move a fleet electorate that had decided more
than its bound. **The maintainer's decisions (2026-10-11): freeze membership, and ship P2 in 2.32.0 without
migration.** An electorate's members are fixed at genesis; changing them is not built in this release (§8.3); the
migration rounds 4–5 built is the follow-up, on branch `fix/p2-migration`. Live reconfiguration is a later plan (§4.2).
Wire v12, new message variants appended (§8.6).*

### 8.1 What an electorate group is

A group whose electorate is an `ElectorateDecl { group, epoch, members, exclusive_default, supersedes }` —
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

`GossipAgent::declare_electorate(group, exclusive_default)` / `POST /gateway/govern/electorate` (`govern:write`,
audited when something is decided) proposes a genesis from the roster this node sees. On a group that has an
electorate it decides nothing: it returns the electorate when the roster still matches, and
`ElectorateError::MembershipFrozen` (HTTP 409 `membership_frozen`) when it does not.
It retries an undecided attempt with a bounded backoff — eight tries, 250 ms doubling to 4 s (round 4, finding 8) — and
returns `NotDecided` only after it; a refusal no retry changes — a door refusal, a second default — is `Refused` at once
(round 5, Q3).

The electorate is named by the group; which nodes are in it is what the group decided. Nothing in configuration names
a node (D1). Trust slices stay as an optional narrowing beneath the member set; they no longer name it (tension (b)).

### 8.2 What a proposal on an electorate group must satisfy

At the engine's door (`ConsensusEngine::electorate_door`, reached by the library and the gateway alike):

- the proposer is a member (`NotAMember` otherwise — a roster member outside the electorate included);
- the group roster equals the member set **by identity** — otherwise `ElectorateMismatch`, counted on the roster
  tripwire (`mycelium::electorate_roster_mismatches`), never decided by the roster it saw;
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
genesis attempt's, and an adoption — passes the group's gate (`ConsensusEngine::claim_gated`, `FenceState`), whose
`compute` only **decides**; the claim runs after it committed:

- an **ordinary** claim passes while this node holds, as its adopted electorate, the epoch the claim names — so a
  proposer whose electorate was superseded since its door stops at its next claim, by name;
- a **genesis** claim passes while this node has adopted no electorate for the group and touched no later attempt,
  registered as *opening*; afterwards it records the attempt as touched **only if** the acceptor now holds a promise or
  acceptance for it — a claim the acceptor refused promised nothing and raises nothing (round 4, finding 4); an
  adoption waits out an opening claim and is refused when this node touched a later attempt.

An acceptor whose gate refused sends no promise.

### 8.3 Changing an electorate's members is not built in this release

**The maintainer's decision (2026-10-11): P2 ships in 2.32.0 without migration.** An electorate's membership is fixed
at genesis, and nothing in this release changes it: declaring a changed roster decides nothing
(`ElectorateError::MembershipFrozen`), and the roster tripwire refuses every proposal to a group whose roster has moved
away from its members. Rounds 4 and 5 of #601 built a migration — retire the group into a successor, which carries
each slot from a majority of the retired group — and the review of round 5 found the first order of it unsafe under
gossip lag and the second still in review; the work is preserved on branch `fix/p2-migration`, the follow-up.

**What that means for an operator, until the follow-up ships:**

- **A dead member lowers the group's fault tolerance.** A group of `n` decides with a strict majority, so it tolerates
  `⌊(n − 1)/2⌋` members down; a dead member uses one of those for good. Size an electorate for the failures it must
  survive *for its lifetime*: three members tolerate one, five tolerate two.
- **A lost majority stops the group.** It decides nothing again — as in any consensus protocol, a minority cannot know
  what the majority decided. Nothing in this release recovers it, and nothing in consensus can: its repair is a
  **disaster-recovery procedure** — an operator reads the survivors' durable acceptor records off their persistence
  directories, forces a genesis from them, and states every decision only the lost members had accepted as possibly
  lost. That procedure is **not built** (`docs/operations/what-is-proven.md`, the third table).
- **The options, then:** size the group with the margin it needs from the start; and for work that can move, declare
  a **new** electorate group with the members you want and move the work there yourself — new slots, decided by the
  new group. Its slots start empty: nothing decided in the old group is carried, so an exclusive slot (a lock held, a
  value chosen) is not continued in the new group. That is the follow-up's work.
- **A member that goes down is restored with its state or retired — never reissued over an empty store.** An earlier
  version of this bullet said *replace hardware under the same node identity*; that is unsafe as stated. A member is
  its `NodeId` (a socket address, `mycelium-core/src/node_id.rs:20-22`), and `mycelium tls issue` can reissue its
  certificate off-node, but since 2.30.0 the member's acceptor memory is **durable and node-local**: each slot's
  promised ballot, the proposer it was promised to and the accepted value (`AcceptorSlot`) are written as
  `sys/consensus-accepted/{node}/{slot}` (`accepted_key`, `src/consensus.rs:3552`) by `persist_acceptor`
  (`src/consensus.rs:1331-1357`), handed to the WAL with a forced `fdatasync` before the promise or vote leaves
  (`answer_prepare`, `src/consensus.rs:4121-4128`), and restored at the next start by `prewarm_accepted`
  (`src/consensus.rs:3725-3743`) from the local store after the snapshot and WAL replay
  (`src/agent/lifecycle.rs:215-217`, `:274`, `:349-354` — the directory is `{base_path}/{node_id}/kv/`,
  `PersistenceConfig::base_path`, `mycelium-core/src/config.rs:394-397`). **There is no empty-acceptor guard:**
  `prewarm_accepted` restores whatever it finds, the call site logs only when it found something, and the memory is
  never fed from anti-entropy — so a node that takes the old `NodeId` with an empty store answers the next
  `Prepare` as a fresh acceptor. It can promise a ballot below one it had already promised (so a slower proposer's
  older ballot is granted after a newer one was), or report no acceptance where it had accepted a value (so a
  proposer at a higher ballot is free to choose another) — and a quorum counting it decides a **second value** for the
  slot. An older backup is the same hazard with a shorter reach: every promise and vote after the copy is forgotten.
  Hence the fork, written into every operator page:
  - **Restore** — same `NodeId`, same or off-node-reissued certificate, **and** `{base_path}/{node_id}/kv/`
    (`snapshot.bin` + `wal.bin`) at its **latest** state, copied quiesced. Safe; no migration. (The record's
    crash-survival is `an_acceptors_record_survives_a_crash_without_a_snapshot`.)
  - **Retire** — the state is gone or not provably latest: never reissue the identity empty; sign a `MemberRemoval`
    naming the `NodeId` and every key it held (`src/agent/membership_ops.rs:22-55`, published under
    `sys/membership/removed/{node}`) and count the group one member smaller. Migrating the group is the follow-up.
  - **Removal forecloses restore.** The ledger is monotonic — no un-removal (`src/membership.rs:20`); a removed key is
    refused at the TLS handshake (`mycelium-core/src/tls.rs:570-579`) and a frame from a removed `NodeId` is dropped at
    the connection whatever key it carries (`mycelium-core/src/connection.rs:392`, `:678`), so a certificate reissued
    for the address does not readmit it. Decide restore or retire before signing.
- **The fleet's exclusive default is set once, at genesis, and cannot move in this release.** A genesis declaring it is
  refused while any group holds it. If the default group must change — its majority is lost, or its members must be
  replaced — nothing is supported: the cluster-scoped exclusive verbs (locks, `consistent_set`, the commitment award,
  the capability-authz policy, the log claim) stay where they are, and stop if that group stops. Moving the default is
  the follow-up's work.

**No wedge without an escape in what ships.** A genesis that can no longer complete (a member it names is gone) is
superseded by declaring the live roster from a member that holds it (§8.4). A second default is refused at genesis by
name. The one stop with no escape is a lost majority — of any electorate, the default's included — which no consensus
protocol escapes; the two-defaults race of §8.5 is the other, and it needs two operators declaring defaults at once
within gossip lag.

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
  it sees both, every exclusive verb is refused by name (fail closed, never decided across the cluster). **That stop
  has no escape in this release** (merging them is the migration follow-up's work), and a tie-break would trade it for
  a second lock holder. Declare the default once, from a node that sees the fleet's records, and never twice at once.
  Group-scoped work outside the exclusive families keeps running during the stop; only the exclusive verbs halt.
- **A `safety_sensitive` slot outside the pinned families has two homes.** A slot in `lock/`, `leader/`,
  `consistent/`, `capauthz/` or `electorate/` (and a commitment award) is decided only in its electorate — refused
  across the cluster while a default is marked. A slot the caller marks `ConsensusConfig::safety_sensitive` under any
  other name is checked only at the proposer that marks it: an untagged cluster-scoped proposal for the same slot,
  from a node with `consensus_require_electorate` off, is answered by every acceptor, so the slot can be decided both
  in the electorate and across the cluster. Use the pinned families for an exclusive outcome, or keep
  `consensus_require_electorate` on fleet-wide and propose through the exclusive verbs (`exclusive_propose`).
- **Changing members** is not built (§8.3); a lost majority stops a group for good, and **its repair is not built
  either** — a disaster-recovery genesis forced from the survivors' durable records, with the possibly-lost
  decisions stated, is the follow-up's follow-up (`docs/operations/what-is-proven.md`, the third table).
- **An acceptor restarted over an empty or older store is not detected.** The guarantee rests on each member's
  durable acceptor record (§8.3): `prewarm_accepted` restores what it finds and `start()` does not refuse a known
  electorate member whose store holds no records. A member reissued its `NodeId` over an empty directory, or
  restored from an older backup, can promise below what it promised and forget what it accepted, and two values can
  be decided. The rule is operational — restore with the latest directory, or retire onto the removal ledger — and a
  start-time refusal (or warning) for an electorate member with no acceptor records would be the mechanical half.
- **Without `[tls]` nothing is authenticated**, the certificates included.
- **A forged commit record stops a proposer.** A `consensus/committed/electorate/…` entry written straight into the
  store is not adopted, but a proposer for that slot reads it as decided and ends `Superseded`, as for any slot — so a
  member writing one can hold a genesis attempt up (liveness, not safety; counted on
  `electorate_records_refused`). Removing the forged entry, or a genesis at the next attempt, moves on.
- **An embedded or peer write to `grp/`** is not prevented; the next proposal refuses the roster it finds — and since
  membership is fixed, such a group decides nothing until the write is undone.

### 8.6 Wire, mixed fleets and the fleet's exclusive electorate

`PrepareIn`, `ProposeIn` and `StaleElectorate` are appended to `ConsensusMsg` (wire v12,
as `Prepare` was in 2.30.0): a node predating them ignores them, so an electorate with an un-upgraded member **times
out rather than commits**. `PrepareIn` and `ProposeIn` are row A's `PrepareTerm` and `ProposeTerm` naming an electorate:
the value is the decision envelope, and they are answered with `PrepareAckTerm` and `PromiseTerm`.

The cluster-scoped exclusive verbs (`distributed_lock`, `LockService`, `consistent_set`, the lock and consistent routes,
the log claim, `mycelium-commitment`'s linearizable award, `set_capability_authz_via_consensus`) decide in the fleet's
**default** electorate (`electorate::live_default`) — a fleet record — or across the cluster when no group declared one:

- **Set once, at genesis.** A genesis may set `exclusive_default` only while no group holds the default — refused by
  name otherwise, at the door and at every acceptor (round 5). It cannot move in this release (§8.3). Its members are
  under §8.3's restore-or-retire rule like any electorate's, with the wider blast radius: a default member reissued
  over an empty store endangers every cluster-scoped exclusive verb, not one group's slots.
- **Fail closed.** While the default cannot be named here — two groups marked (§8.5), or a `consensus_electorate` that
  disagrees — every exclusive verb is refused by name, and every acceptor refuses a cluster-scoped proposal for an
  exclusive slot, tagged or not (round 5, D5). `consensus_electorate` may only restate the answer. An acceptor also
  refuses a group-scoped proposal for an exclusive slot on any group but the default.
- **A slot that names a group is decided only in that group.** `leader/{g}` and every `electorate/{g}/…` slot are
  refused — at the door and at every acceptor, whatever the default state or `consensus_require_electorate` — under
  any scope but exactly `Group(g)`: under the cluster scope or `Groups`, the votes would be signed at that scope and
  could assemble a certificate the group's gate never saw (#601's final review, D1). A tagged `PrepareIn`/`ProposeIn`
  is answered only under `Group` scope, since the scope is not inside the signed payload (D2).

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
