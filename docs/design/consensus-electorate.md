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
  at genesis (changing it is a migration to a successor group, §8.3); a trust slice (`declare_trust`) stays available beneath it as an optional narrowing, no longer the
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
  changing it is a migration (§8.3).
- **Today's profile names its voter set by node identity.** `declare_trust(group, &[NodeId])` is how §7's eligible set
  is stated. D1 says the electorate is *named* as a governed group; reconciling the two is part of P2, and this record
  does not decide whether trust slices remain the vote filter underneath. *Since P2:* decided — the electorate's
  member set, decided by the group, is the vote filter; a trust slice may narrow it, and no longer names it (§8).

## 4. What is planned, and what is a later plan

### 4.1 In the post-360 plan — P2 *built* (§8)

| Row | Promise (quoted from the plan) | State |
|---|---|---|
| **P2** | "safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities" | delivered in 2.32.0 (#601), **without migration**: an electorate group's members are fixed at genesis; the migration — close, enumerate, carry, complete — delivered after it (§8.3) |
| **C1** | "`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)" | delivered (#600): `DEFAULT_LEADER_LEASE`, `release_leadership`, `LeaderTerm::Permanent`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}` |
| **C2** | "acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on" | delivered (#600): `ConsensusEngine::collect_finished`, bounded per tick, on its own task |

### 4.2 A later plan — *not built, not scheduled*

**Versioned electorates — live reconfiguration** — membership epochs carried on every ballot, and a transition
during which a proposal must win a quorum of the old electorate *and* of the new (joint consensus), so an electorate's
members can change under a live slot. Recorded as protocol work for a later plan under D1 (the post-360 plan's §5). P2
does not build it: rounds 2 and 3 of #601 built one-member steps with a drain before each step, and round 4 found a
race between the drain's report and the step's fence and a bound that could hold a fleet electorate for good; the
maintainer decided (2026-10-11) to **freeze membership**; a migration to another group — one that terminates — is built
(§8.3). Live reconfiguration
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
> its members is a migration to a successor group that terminates (§8.3). A dead member lowers the group's fault
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
migration.** An electorate's members are fixed at genesis; changing them is a **migration** to a successor group, built
after 2.32.0 — the parked lazy carry of rounds 4–5 replaced by one that **terminates**: close, enumerate, carry,
complete (§8.3). Live reconfiguration is a later plan (§4.2). Wire v12, new message variants appended (§8.6).*

### 8.1 What an electorate group is

A group whose electorate is an `ElectorateDecl { group, epoch, members, exclusive_default, supersedes, predecessors }`
— the member set by identity, sorted; the groups it succeeds, if any (§8.3) — decided by **consensus** on the slot
`electorate/{group}/{epoch}` (`src/agent/electorate.rs`):

- **Genesis** names the group's roster as its members and is decided by **every** member it names; each acceptor checks
  the set is the roster it sees. The **epoch counts genesis attempts**: 1, or one more than a pending genesis it
  supersedes (§8.4). Nothing follows a genesis: once adopted, the electorate is **frozen**. A genesis that names
  predecessors is a **successor**, which decides nothing until each has retired to it (§8.3).
- **A certificate** — the decided decision envelope (row A, PR #600: the value Paxos decides, whose `value` is the
  record) and the deciding votes for it, signed under `[tls]` — is written to `consensus/electorate-cert/{group}/{epoch}`
  before the commit leaves. A node adopts a genesis only when its certificate holds a vote from every member it names,
  at one ballot. A record that does not verify is not adopted and is counted (`mycelium::electorate_records_refused`):
  a forged declaration does not change the electorate.

`GossipAgent::declare_electorate(group, exclusive_default)` / `POST /gateway/govern/electorate` (`govern:write`,
audited when something is decided) proposes a genesis from the roster this node sees; `declare_successor_electorate`
(`"predecessors": […]`) one that names the groups it succeeds. On a group that has an electorate it decides nothing:
it returns the electorate when the roster still matches, and `ElectorateError::MembershipFrozen` (HTTP 409
`membership_frozen`) when it does not; on a retired group `ElectorateError::Retired` (409 `retired`).
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

### 8.3 Changing an electorate's members is a migration — close, enumerate, carry, complete — and why it is safe

*Built 2026-10-11, after the maintainer's decision of the same day on the parked branch's design: the lazy carry
(the successor reading a majority of its predecessor at its first touch of each slot, for ever) left the successor
dependent on the predecessor's majority for every slot not yet touched — and, since the "already carried" set was
memory, for every slot after a restart — so the predecessor could never be decommissioned and member decay never
ended. **A migration must terminate.** The order below is successor first (the round-5 finding: a successor declared
after the retirement could form without it); its last step is a decision of the successor after which the
predecessor is never consulted again.*

**The procedure** (guide 04 has the calls; `POST /gateway/govern/electorate` carries each as one body):

1. **Declare the successor first**, naming its predecessors as a set — `declare_successor_electorate("G2", &["G1"])`,
   `{"group": "G2", "predecessors": ["G1"]}`; two or more is a merge (`ElectorateDecl::predecessors`). Its genesis is
   decided by every member it names, as any genesis is; each member checks that every named predecessor is an
   electorate it can see and is not retired to another group. **G2 decides nothing** — no slot, ordinary or
   exclusive, at the proposer's door or at any acceptor — until every named predecessor's verified retirement
   **naming G2** is visible there (`ConsensusEngine::successor_ready`; a proposer is refused by name,
   `ElectorateMismatch` "waiting for its predecessor …"; an acceptor does not answer and counts it,
   `electorate_waiting_refusals`). Fail closed. G2's own electorate slots — its genesis, its retirement, its
   completions — are exempt.
2. **Retire G1 to G2** — `retire_electorate("G1", "G2")`, `{"group": "G1", "retire_to": "G2"}`: a decision of a
   strict majority of G1 on `electorate/G1/retire`, certified like a genesis (`Retirement`). The proposer and every
   G1 acceptor accept it only if G2's decided genesis is visible to them and names G1 (not visible yet is gossip —
   retried; visible and not naming G1 is `Refused`), only while G1 **may retire** — every predecessor G1 itself names
   is complete or dropped *as seen by that acceptor*, a retirement not visible there counting as incomplete (chains,
   (vi)) — and only when `passes_default` says what they see: `true` exactly when G1 holds the fleet's exclusive
   default and is itself live (uniqueness is not required: two live holders are merged exactly this way, each
   retiring into one successor — §8.5) — the default follows the retirement whatever G2's genesis flag says. **The
   prepare is checked the same way, before it closes** (the review of #606, finding 1): a retirement's `PrepareIn`
   carries no value, so an acceptor promises it — and closes G1 — only if G1 may retire as seen there and some live
   successor naming G1 is visible there; otherwise it refuses without promising, and nothing closes. **At the
   retirement's promise every G1 acceptor closes G1**, durably — the promise is its acceptor record, fsynced before it
   leaves (`GateKind::Retire`, after every ordinary claim in flight). After the retirement is chosen, no G1 value can
   be chosen (i).
3. **Complete** — `complete_migration("G2")`, `{"group": "G2", "complete": true}`, from any member of G2,
   idempotent (`agent::migration`), for each predecessor whose retirement to G2 is visible:
   - **enumerate**: ask a strict majority of G1 for a **manifest** — for every slot the member holds acceptor state
     for *in G1* (the acceptor memory is tagged with the electorate group a claim passed through,
     `AcceptorSlot::groups`, in the durable record), its promise and its highest acceptance. A member answers only
     for a group it is in, only when the retirement to G2 verifies there and the asker is a member of G2, and only
     after it has closed G1 durably itself (`close_for_carry`: the gate, then the retirement on stable storage). The
     union over the majority is the carry set **S**, keyed by slot, keeping the highest acceptance. Manifests are
     paged (`CarryManifestRequest` / `CarryManifestPage`, 256 entries or 2 MiB a page); a value over 1 MiB, or one
     a restarted member holds only by digest, travels by digest and is resolved from a decision record the reporter
     or the completer holds. **No cap on |S|.**
   - **carry**: for each slot in S with an acceptance, one ballot of G2 proposing the acceptance's decision
     envelope **whole** (`ConsensusEngine::propose_carry`, on the wire `CarryPrepare` / `CarryPropose` — `PrepareIn` /
     `ProposeIn` naming the predecessor, so an acceptor admits it as soon as *that* predecessor's retirement is
     visible to it, while an ordinary proposal on G2 waits) — value, window, lineage, original proposer and token
     kept, as any adoption keeps them (`lock-lifecycle.md` §4): a lock keeps its holder's window; a released or
     lapsed decision stays ended, because its marker names the lineage and its window is inside the envelope. A
     carry ballot does not stop at a live decision record (the record may be G1's commit, held here by gossip and
     by no G2 majority — the committed-but-unpropagated value is what a carry brings over); G2's own phase 1 applies,
     so an acceptance G2 already holds above it wins. A slot in S with promises only has nothing to carry: no value
     was chosen in G1 (ii). Sequential, one ballot per slot — a one-off cost proportional to |S|, stated.
   - **completion**: a decision of G2 on `electorate/G2/carried/G1` (`Completion { group, epoch, predecessor,
     predecessor_epoch, passes_default, slots: |S|, digest(S) }`), certified by a strict majority of G2 and adopted
     only when the certificate verifies. It **stands as proof of the retirement**: `successor_ready`, `holds_default`
     and the retire check read a verified completion as they read the retirement, so a G2 member that no longer holds
     G1's records is not held up once G1 is gone; and at the completion each G2 member — the completer, and every
     learner of the `CommitTerm` — hands G1's genesis and retirement records and certificates and the completion's
     own to the WAL with `append_sync` (`sync_migration_records`; the review of #606, finding 4: they otherwise reach
     a successor member by gossip, appended under the default `Async` mode).
     **Before it, G2 decides nothing outside S** — the proposer's door and every acceptor refuse an ordinary
     `PrepareIn`/`ProposeIn` on G2 naming the completion as the missing step (fail closed at both ends; a
     completion visible at the proposer and not yet at an acceptor is a timeout, retried); only carry ballots and
     G2's own electorate slots pass (iv). **After it, G2 decides any slot without consulting G1**, G1's members may
     be removed (the removal ledger, `sys/membership/removed/`), and G3's migration from G2 reads only G2 (vi).
   - **a merge** (two or more predecessors) completes each predecessor as its retirement becomes visible — its carry
     ballots are admitted per predecessor — and decides outside the carry sets only once every predecessor has
     retired or been dropped and every carry is complete. When two predecessors both hold a slot (two defaults
     merged, each with the same lock), the first carried wins and the second carry adopts it — **the lexically-first
     predecessor's value stands, silently** (predecessors are carried in name order); stated, not resolved, since
     two defaults were already a safety failure. A predecessor that never retires holds G2 in that state; the escape
     is the drop, below.
4. **Drop a predecessor that cannot retire into G2 any more** — `drop_predecessor("G2", "P", force)`,
   `{"group": "G2", "drop_predecessor": "P"}`: a decision of a strict majority of G2 on `electorate/G2/drop/P`
   (`PredecessorDrop`, certified), admitted — at the door and at every G2 acceptor — **only when P's verified
   retirement to another successor is visible there** (the review of #606, round 2, A: a drop admitted on mere
   absence of a retirement raced the retirement, since the two quorums are in different groups and nothing
   intersects). That is the losing successor's way on: two successors declared for one predecessor compete on its
   retirement slot, one wins, and the other — its `successor_ready` naming where the predecessor went — drops it
   and goes on as a fresh group. After the drop G2 no longer waits on P (nor does its retirement). Nothing of P is
   carried. **`force: true` is an operator's assertion outside the protocol's guarantee** — P's majority is gone for
   good, so P will never retire anywhere (disaster-recovery territory, `what-is-proven.md`): the drop is then admitted
   with no retirement elsewhere visible, audited with the flag, and P's retirement into G2 is refused at P's door and
   every P acceptor where the drop is visible. If the assertion is false — P's majority is alive and retires into G2
   before the drop reaches it — both are decided: **the retirement stands and the drop is void** (`drop_view` is
   `None` while a retirement of P to G2 is visible), G2 carries P after all, and any slot G2 decided meanwhile is a
   **detected conflict** on the carry (below), not a prevented one — unless P′ has meanwhile closed for its own
   retirement: a P′ that force-drops Q, retires (its acceptors seeing every predecessor dropped or complete), and then
   sees Q's raced retirement into it become visible has a void drop it can neither complete nor retire past — **a
   permanent stop under a false `force` assertion**, disaster-recovery territory, not a recorded conflict. A drop
   while P is mid-prepare (its retirement
   promised by some members, decided by none) leaves P closed on those members: P's slots are stopped until a new
   successor naming P is declared and P retires into it.
5. **A carry that finds another value is a conflict, recorded** (round 2, A(ii)). A carry ballot that returns
   `Superseded` with a different decision identity above the carried one — two values were chosen for one slot: a
   forced drop that raced a retirement, a 2.32.0 member that voted for the retirement without closing (§8.6), two
   merged defaults that both held the slot — is counted on the commit-conflict tripwire (`commit_conflicts`,
   `mycelium_electorate_carry_conflicts_total`, an emergent event), logged by name, and reported in the completion as
   `conflicts` with the first 64 slot names — never as carried. **The counter is a tripwire, the completion record is
   the count:** a completion run that stops and is run again re-enumerates and re-carries the slot, so the conflict is
   counted again on the tripwire and the metric, while the completion's own `conflicts` stays exact (one per slot). **The completion still decides.** A conflict is a
   fact about the past that no carry can un-choose; refusing the completion would freeze G2 for good with nothing an
   operator could do, while deciding it leaves the value G2 holds standing and the conflict on the record, in the
   tripwire and in `GET /gateway/govern`. The merge's tie-break is therefore no longer silent: when two merged
   predecessors both held a slot, the second carry reports it.

**Restart before completion:** re-enumerate — every step is idempotent, and nothing of the carry's progress needs to
survive (the in-memory progress is only what `GET /gateway/govern` shows as *carrying n of |S|*). A slot carried
before the restart is carried again: G2's phase 1 finds its own acceptance and re-decides the same envelope.

**The safety argument** (one slot `s` decided only in the electorate — an exclusive slot, or an ordinary one used only
through the group; crash faults; messages authenticated under `[tls]`). Each sentence is a multi-node test, seen
failing first with its mechanism disabled:

- **(i) No G1 value is chosen after the retirement is chosen.** The retirement was promised by a strict majority `P`
  of G1, each of which closed G1 at its promise, durably; a later G1 majority would need a closed member, whose gate
  refuses every ordinary claim, and the gate orders a close after every ordinary claim in flight.
  *Tests:* `a_proposer_closed_mid_ballot_does_not_complete` (disabling the close at the promise: the proposal
  committed `before-the-close` after the retirement was accepted), `a_close_waits_for_an_ordinary_claim_in_flight`.
- **(ii) S contains every value possibly chosen in G1.** Chosen at ballot `b` means a strict majority `M` of G1
  accepted it; the manifests come from a strict majority `R` of G1, each read after the member closed G1; a member of
  `R ∩ M` reports an acceptance at a ballot `≥ b`, which by G1's own phase-1 rule is the chosen value — and that
  member still holds it, because acceptor memory is collected only once a decision is over (C2) and a closed member
  accepts nothing new. The tag that files the slot under G1 is in the durable record, so a restarted member's
  manifest is complete. *Test:* `a_value_chosen_in_a_retired_group_is_carried_by_its_successor` — v accepted by A and
  B, never committed; with the manifest reporting no acceptances, D's lock value was decided over it.
- **(iii) Each slot in S is decided in G2 with the carried value, or with a value G2's own Paxos chose above it.** The
  carry is an ordinary G2 ballot: a promise quorum of G2 reports what G2 accepted, and the highest acceptance wins
  (the row-A phase-1 rule, unchanged); G2 held nothing for `s` before the first carry (it decided nothing until the
  retirement was visible, and `s` was not G2's), so the first carry decides the carried value, and a later carry of
  the same slot re-decides what G2 holds. *Tests:* the carried locks, consistent write and chosen value of
  `after_the_completion_the_successor_survives_the_predecessors_departure_and_its_own_restart`;
  `a_completion_run_that_stops_mid_carry_is_resumed_by_the_next` (the next run re-carries the first slot and keeps
  its value; with a run trusting the progress it remembered instead of re-enumerating, the completion was decided
  over zero slots and the second value never carried).
- **(iv) Before the completion G2 decides nothing outside S; after it G1 is irrelevant.** The door and every acceptor
  refuse every ordinary proposal on G2 while a predecessor has not retired to it (nor been dropped) or has no
  completion (`migration_complete`), and a carry ballot names its predecessor on the wire; a slot outside S
  has no G1 state at all (ii), so a fresh G2 decision after the completion contradicts nothing G1 chose; and every
  value G1 chose is held by a G2 majority (iii), so G2's own phase 1 finds it without G1. *Test:*
  `a_value_chosen_in_a_retired_group_is_carried_by_its_successor` — `work/early` refused "not complete" after the
  retirement; after the completion, with A's and B's listeners stopped (G1 has no majority), `work/fresh` commits and
  the lock still reads v (with the completion check removed from the door, `work/early` was decided before the
  completion).
- **(v) The default never has zero or two holders across the handover.** A genesis declares the default only while
  no live group holds it (door and acceptor); a retirement of the holder is accepted only with `passes_default =
  true` and from the live holder, and the successor holds the default from that record on (`holds_default`, one
  level — never a chain); a retirement that drops it, or one that claims it for a group that does not hold it, is
  refused by every acceptor; while a holder's retirement is visible and its successor is not, every exclusive verb
  is refused by name (fail closed). *Test:* `a_second_live_default_is_refused_by_name` — every node names `new` after
  the retirement and a lying retirement is refused (with `passes_default` forced false, `exclusive_electorate`
  answered `Ok(None)` and the lock decided across the cluster).
- **(vi) Chains: G3 from G2 needs only G2 after G2's completion.** G2's carry acceptances are G2's own, tagged G2,
  held by a G2 majority (iii); a group whose carry from a predecessor is incomplete — or whose predecessor's
  retirement is not yet visible, or retired elsewhere and is not dropped — cannot retire: the door, and every acceptor
  at the **prepare** (`retirable`), so no successor ever enumerates a group that still owed a carry, and no bare
  prepare closes a group that may still have to carry. The chain never grows: a successor's
  retirement passes what it holds, and a predecessor's own predecessors are never read. *Test:*
  `a_group_with_an_incomplete_carry_cannot_retire_and_a_chain_reads_only_its_predecessor` — the early retirement
  refused at the door and the acceptor; A and B shut down; G3 reads G1's value through G2 alone (with the refusal
  removed, G2 retired before completing: `Ok(Retirement { group: "g2", … successor: "g3" })`).

*#606's review, each a test:* the bare prepare on a successor with an incomplete carry (finding 1 — a wedge with no
escape: E and F closed G2 on a prepare from a lagging D, the propose was refused, and G2 could neither complete nor
retire) — `a_retirement_prepare_closes_nothing_while_the_carry_into_the_group_is_incomplete` (with the prepare
closing unconditionally: `E promised the retirement's prepare and closed g2`; the merge variant inside it); the drop
— `a_successor_waiting_on_a_predecessor_that_never_retires_has_an_escape` (with a dropped predecessor still able to
retire in: `a dropped predecessor cannot retire into the group: Ok(…)`); a 2.32.0-shaped record carried (finding 2)
— `a_slot_claimed_before_the_tag_existed_is_carried` (tagged slots only: the chosen value lost); the merge of two
defaults (finding 3) — `two_defaults_are_merged_into_one_successor` (uniqueness required of the retiree: `one retires
although it is not the unique holder: Refused(…)`); the completion as proof (finding 4) —
`a_completion_stands_as_proof_when_the_predecessor_is_gone_and_the_successor_crashed`.

*The round-5 review's interleavings, each a test:* the declare-before-retirement race (D1) —
`a_successor_decides_nothing_until_its_predecessors_retirement_is_visible` (the retirement taken out of D's and E's
stores: D's door and E's acceptor refuse, counted; it arrives, the completion carries); the two-defaults wedge (D2) —
`a_successor_waiting_on_a_predecessor_that_never_retires_has_an_escape`; the merge (D3) —
`a_merge_carries_every_predecessor`; the report-versus-fence race (round 4, finding 1) —
`a_close_waits_for_an_ordinary_claim_in_flight`; a committed-but-unpropagated value —
`a_committed_but_unpropagated_value_is_carried` (the decision's records taken out of G2's stores with anti-entropy
slowed: a fresh proposal of `w` is superseded by the carried commit; with the carry set restricted to what the
completer's own decision records showed as decided, `w` was decided). The maintainer's acceptance test —
`after_the_completion_the_successor_survives_the_predecessors_departure_and_its_own_restart`: G1's three nodes shut
down for good, G2's three restart from disk, G2 decides a fresh slot and a fresh lock, A's lock is still A's, A's
released lock is free, the consistent write reads, the chosen value stands.

**Liveness, stated.** A migration needs a reachable strict majority of G1 — to retire it and to enumerate it — and a
strict majority of G2. **A G1 that lost its majority cannot migrate; disaster recovery is not built** — its slots
are stopped for good, as in any consensus protocol. Two migrations of one G1 compete on its retirement slot: at most
one succeeds, and each caller is told which (`two_concurrent_retirements_decide_one_successor_and_a_non_member_cannot_retire`).
A G2 member down during the carry: the carry waits (a G2 majority is needed per ballot) and the completion run
returns `NotDecided` naming the slot; it is retried with a bounded backoff and may be run again from any member. A
manifest entry whose value only a digest survives (a restarted G1 member holding an acceptance over the acceptor
record's 4 KiB cap) is resolved from a decision record; when none is held anywhere the carry stops at that slot,
named, until gossip supplies the record. A 2.32.0 member of G1 ignores a manifest request: G1 needs a strict
majority of upgraded members to be enumerated.

**Escapes from every wait.**
- *A successor waits on a predecessor that never retires* (named by mistake, its majority gone, or retired to another
  successor): G2 decides nothing and cannot retire (a predecessor not retired to it counts as incomplete). **Drop
  it** (step 4): a G2-majority decision, after which G2 completes what did retire, decides, and may retire; the
  predecessor goes on deciding on its own, and its late retirement to G2 is refused.
- *A retirement promised by a majority whose proposer died* closes the group until a member completes it:
  `retire_electorate` from any member adopts what was accepted, or decides its own; an attempt that ended undecided
  after this node promised it is counted (`electorate_retirements_undecided`) and, at the gateway, audited.
  *Test:* `a_retirement_completes_a_pending_one`.
- *Two live defaults* (§8.6): merge them — declare one successor naming both, retire both into it, complete.

**The operator's fork for a lost member** (PR #604, kept here unchanged in substance): a member that goes down is
**restored** with its persistence directory at its latest state, or **retired** onto the removal ledger — never
reissued over an empty or older store — and now a group can also be **retired as a whole** into a successor with the
members you want, which is this section. The restore-or-retire bullet below is #604's text; its "the follow-up" is
this section.

- **A member that goes down is restored with its state or retired — never reissued over an empty store.** An earlier
  version of this bullet said *replace hardware under the same node identity*; that is unsafe as stated. A member is
  its `NodeId` (a socket address, `mycelium-core/src/node_id.rs:20-22`), and `mycelium tls issue` can reissue its
  certificate off-node, but the member's acceptor memory is **durable and node-local**: each slot's promised
  ballot, the proposer it was promised to (added in 2.30.0) and the accepted value (`AcceptorSlot`) are written as
  `sys/consensus-accepted/{node}/{slot}` (`accepted_key`, `src/consensus.rs:3552`) by `persist_acceptor`
  (`src/consensus.rs:1331-1357`), and since 2.32.0 (#585) handed to the WAL with a forced `fdatasync` before the
  promise or vote leaves — the guards at `src/consensus.rs:4146` (the promise) and `:4428` (the vote); before
  2.32.0 the record reached disk only by snapshot. It is restored at the next start by `prewarm_accepted`
  (`src/consensus.rs:3725-3743`) from the local store after the snapshot and WAL replay
  (`src/agent/lifecycle.rs:215-217`, `:274`, `:349-354` — the directory is `{base_path}/{node_id}/kv/`,
  `PersistenceConfig::base_path`, `mycelium-core/src/config.rs:394-397`). **An electorate member must configure
  `[persistence]`:** without it `persist_sync` returns `true` with nothing on disk ("no promise was made",
  `src/consensus.rs:3094-3096`) and the node answers anyway, so the hazard below recurs on every restart. **There is
  no empty-acceptor guard:** `prewarm_accepted` restores whatever it finds and the call site logs only when it found
  something. Anti-entropy does not close the gap either — the record is gossiped (`src/consensus.rs:3659-3662`),
  and a peer's copy of this node's *own* record is applied to a fresh store as a repair
  (`mycelium-core/src/connection.rs:66-68`, `anti_entropy_repair_after_a_restart_is_not_a_namespace_violation`) —
  but the memory is loaded once at start, so a copy arriving later reaches the store and not the running acceptor
  until the next restart, and the node's first own write overwrites it by LWW. So a node that takes the old
  `NodeId` with an empty store answers the next `Prepare` as a fresh acceptor. It can promise a ballot below one it
  had already promised (so a slower proposer's
  older ballot is granted after a newer one was), or report no acceptance where it had accepted a value (so a
  proposer at a higher ballot is free to choose another) — and a quorum counting it decides a **second value** for the
  slot. An older backup is the same hazard with a shorter reach: every promise and vote after the copy is forgotten.
  Hence the fork, written into every operator page:
  - **Restore** — same `NodeId`, same or off-node-reissued certificate, **and** `{base_path}/{node_id}/kv/`
    (`snapshot.bin` + `wal.bin`) at its **latest** state, copied quiesced. Safe; no migration. (The record's
    crash-survival is `an_acceptors_record_survives_a_crash_without_a_snapshot`.)
  - **Retire** — the state is gone or not provably latest: never reissue the identity empty; sign a `MemberRemoval`
    naming the `NodeId` and every key it held (`src/agent/membership_ops.rs:22-55`, published under
    `sys/membership/removed/{node}`) and count the group one member smaller — and **leave its `grp/` entry
    alone**: `electorate_door` refuses every proposal on a group whose live roster differs from the member set
    (`ElectorateMismatch`, `src/consensus.rs:2299`), so tidying the dead member out of the group stops the group.
    Migrating the group — retiring it as a whole into a successor — is the procedure above.
  - **Removal forecloses restore.** The ledger is monotonic — no un-removal (`src/membership.rs:20`); a removed key is
    refused at the TLS handshake (`mycelium-core/src/tls.rs:570-579`) and a frame from a removed `NodeId` is dropped at
    the connection whatever key it carries (`mycelium-core/src/connection.rs:392`, `:678`), so a certificate reissued
    for the address does not readmit it. Decide restore or retire before signing.

**What this changes in the acceptor record.** A slot claimed on an electorate group carries the group's name in the
node-owned acceptor record (`sys/consensus-accepted/{node}/{slot}`, tag `0x04`; a slot never claimed on one still
writes 2.32.0's `0x02`/`0x03` bytes) — a few bytes per slot per node, never removed, so a manifest after a restart is
complete.

**The claim is for slots decided only in the electorate** (round 5, Q1). An ordinary slot is not homed: a legacy or
untagged proposal under another scope (the whole cluster, another group) can reach G1's members as plain acceptors,
outside G1's gate and untagged, and this design does not order those acceptances against the retirement nor
enumerate them. For an ordinary slot used only through the electorate group the argument holds as written; for one
also proposed under other scopes it is not claimed.

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
  it sees both, every exclusive verb is refused by name (fail closed, never decided across the cluster). **The escape
  is a merge** (§8.3): declare one successor naming both, retire both into it, complete — a tie-break would trade the
  stop for a second lock holder. Declare the default once, from a node that sees the fleet's records, and never twice
  at once.
  Group-scoped work outside the exclusive families keeps running during the stop; only the exclusive verbs halt.
- **A `safety_sensitive` slot outside the pinned families has two homes.** A slot in `lock/`, `leader/`,
  `consistent/`, `capauthz/` or `electorate/` (and a commitment award) is decided only in its electorate — refused
  across the cluster while a default is marked. A slot the caller marks `ConsensusConfig::safety_sensitive` under any
  other name is checked only at the proposer that marks it: an untagged cluster-scoped proposal for the same slot,
  from a node with `consensus_require_electorate` off, is answered by every acceptor, so the slot can be decided both
  in the electorate and across the cluster. Use the pinned families for an exclusive outcome, or keep
  `consensus_require_electorate` on fleet-wide and propose through the exclusive verbs (`exclusive_propose`).
- **Changing members** is a migration (§8.3), which needs a reachable strict majority of the retired group; a lost
  majority stops a group for good, and **its repair is not built** — a disaster-recovery genesis forced from the
  survivors' durable records, with the possibly-lost decisions stated (`docs/operations/what-is-proven.md`, the
  third table).
- **A forced drop is an assertion, not a guarantee** (§8.3 step 4). `force: true` says the predecessor's majority is
  gone for good; the protocol cannot check it. If it is false and the predecessor retires into the successor before
  the drop reaches it, the successor may already have decided slots the predecessor chose — detected by the carry's
  conflict tripwire and recorded in the completion, never prevented; and if the successor had already closed for its
  own retirement when that retirement became visible, it is stopped for good, with nothing recorded (§8.3 step 4).
  That is the disaster-recovery residual, stated.
- **A 2.32.0 member in a retiring group.** Such a member votes for the retirement as an ordinary slot of its group,
  **without closing** (it has no `Retire` gate kind): with one such member, (i) is false — a value can be chosen in G1
  after the retirement by a majority that includes it — and safety rests on (ii): a value chosen in a group of three
  needs two acceptances, so with one old member every chosen value has an upgraded acceptance, the carry set is
  enumerated from the upgraded members' manifests (the old member ignores the request) and the manifest majority is
  the two upgraded members — every chosen value, including one chosen after the close, is in a manifest and is
  carried before G2 decides anything outside S. What is lost is only (i)'s timing, not (ii)'s completeness. With two
  2.32.0 members in a group of three, no strict majority of manifests can be gathered: the migration is **stuck, not
  unsafe**. Migrate once every member runs this release.
- **The migration and #605's empty-store detection.** A successor member that crashed before the predecessor's
  records reached its disk is a *lost-gossip* member, not an empty store: its own acceptor records are intact, and
  what it lacks are the predecessor's genesis, retirement and certificates — which is why the completion stands as
  proof and the records are fsynced at completion (§8.3). #605's refusal of a known electorate member whose store
  holds no acceptor records is about the member's own answers and is orthogonal: it refuses the start, this fsyncs
  what the start will need.
- **An acceptor restarted over an empty or older store is not detected — nor a member without persistence.** The
  guarantee rests on each member's durable acceptor record (§8.3): `prewarm_accepted` restores what it finds,
  `start()` does not refuse a known electorate member whose store holds no records, and a member with no
  `[persistence]` answers with nothing on disk (`persist_sync` returns `true`). A member reissued its `NodeId` over
  an empty directory, restored from an older backup, or simply restarted without persistence, can promise below
  what it promised and forget what it accepted, and two values can be decided. The rule is operational — configure
  persistence; restore with the latest directory, or retire onto the removal ledger. The mechanical half, queued as
  a code PR from #604's review: refuse genesis adoption for a member without `[persistence]`; warn by name when a
  member of an adopted electorate holds zero own `sys/consensus-accepted/` records for it; and at
  `connection.rs:66-68` compare an inbound copy of this node's own record against memory and warn when it holds a
  promise or acceptance the node has forgotten — stated with what that cannot see (a promise whose gossip reached
  no survivor; a stale copy whose forgotten answers were on slots since ended).
- **An acceptor restarted over an empty or older store is detected, not prevented — and a member without
  persistence is refused.** The guarantee rests on each member's durable acceptor record (§8.3): `prewarm_accepted`
  restores what it finds, and a member reissued its `NodeId` over an empty directory, restored from an older backup,
  or restarted without persistence, could promise below what it promised and forget what it accepted. Three
  detections, each by name and counted, none a guard on a Layer I write (`src/agent/acceptor_guard.rs`; the
  mechanical half #604 queued):
  - **Persistence required** (deterministic). A node that would adopt, declare, propose or answer a genesis for an
    electorate it is a member of with no `[persistence]` refuses by name — `ElectorateError::PersistenceRequired`,
    `409 persistence_required` at `/gateway/govern/electorate`, a `DoorRefusal` at the proposer's door, no answer at
    the acceptor's (so a genesis naming such a member is never decided) — counted on `electorate_persistence_refused`.
    `persist_sync` returns `true` with nothing on disk when no WAL is configured, so such a member would forget on
    **every** restart. A node outside the roster, and consensus outside an electorate, are untouched.
  - **An empty store, warned.** Every member accepted the group's genesis, and the collector's `shrink_acceptor` keeps
    `{promised}` rather than deleting, so a member that kept its store always holds at least the genesis slot's record.
    A member of a *committed* genesis whose memory holds nothing for that slot is warned once per group and counted
    (`acceptor_memory_empty_for_electorate`) — at `start()` from the records the store holds, and when the committed
    record first arrives by gossip (an empty store has none at start). Warned, not refused: the record is read
    unverified there, and a brand-new member legitimately holds nothing until its genesis is decided — firing only on a
    committed genesis is how *not yet decided* is told from *decided, and I hold nothing*.
  - **A forgotten promise withholds the slot.** On the core's inbound seam (`CoreCtx::inbound_observer`, called by
    `connection::apply_inbound` for every inbound copy of a key under a prefix the agent named — core reports, decides
    nothing), a peer's copy of this node's **own** acceptor record is compared with its memory for the slot,
    independently of what LWW made of it in the store. Memory never lowers (a promise only rises; `shrink_acceptor`
    raises `promised` to the ended ballot), so a copy whose promised ballot is **above** memory's, or whose acceptance
    memory lacks at a ballot **at or above** memory's promise (memory `{b to Y, none}` against the copy `{b to X,
    (b, vX)}` — a vote cast at `b` this node would cast again for another value; #605's confirming review), unless
    memory's promise is the collector's floor (a collected slot's older acceptance-bearing copy at exactly `ended`), is
    an answer this node gave and no longer holds: the slot is withheld — no prepare answered, no vote cast, no proposal
    made by this node, as acceptor or proposer — **until the next start**, counted on `acceptor_promises_forgotten`;
    re-checked at the top of every ballot and before the proposer's own vote, so a slot withheld mid-proposal ends it
    at the next step. **The disk is repaired at detection** (#605's review, finding 1): the merged record — the higher
    promise and whom it was promised to, the higher acceptance — is written to this node's own key under
    `acceptor_records`, with memory **re-read and merged again under that lock** (a listener promise can land between
    the compare and the write; the record must never say less than an answer that is out), a Layer III write of a key
    the agent owns. Without it, a node that had already promised a ballot
    empty-handed before the copy arrived would have written `{promised}` with a fresh HLC and beaten the copy by LWW
    locally and fleet-wide, and the next start would read the empty-handed record. `persist_acceptor` writes nothing
    for a withheld slot, so no answer in flight lands over the merged record. A withheld slot never resumes without a
    restart: memory is not refreshed live (that would race the acceptor's own writes); the start reads the merged record
    (`a_forgotten_promise_repairs_the_record_so_the_next_start_holds_it`). The same wire is reachable by a forgery — a
    peer writing a copy of this node's own record with a higher ballot withholds that one slot at once; liveness, one
    slot, within the trusted-domain model, and the write is a counted `sys/` namespace violation.

  **What this cannot see, stated.** The third detection needs the copy to **arrive** — a promise whose gossip reached
  no survivor is forgotten unseen — and to arrive **before** this node answers a higher ballot for the slot: once it
  has promised `b+1` empty-handed, a copy carrying `b` is below memory and reads as a stale record. Between the restart
  and the repair's arrival the hazard is open, and the test that shows the interleaving
  (`a_forgotten_promise_withholds_the_slot_so_a_second_value_is_not_decided`) waits for the repair before the second
  proposer prepares — detection with a fail-closed consequence on one slot, not a guarantee. The second detection
  cannot tell an older backup from a kept store when the backup holds the genesis slot: forgotten answers on later
  slots are the third detection's to see, under its limit. The operator rule is unchanged — configure persistence;
  restore with the latest directory, or retire onto the removal ledger.
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
the value is the decision envelope, and they are answered with `PrepareAckTerm` and `PromiseTerm`. The migration
(§8.3) appends four more: `CarryManifestRequest` / `CarryManifestPage` (under `PROPOSE` and `VOTE`, to one node each)
and `CarryPrepare` / `CarryPropose` (`PrepareIn` / `ProposeIn` naming the predecessor a carry ballot carries from); a
retirement and a completion are ordinary `PrepareIn`/`ProposeIn` on `electorate/{g}/retire` and
`electorate/{g}/carried/{p}`, a drop on `electorate/{g}/drop/{p}`. **What a 2.31.0 or 2.32.0 node makes of it:** it
ignores the four variants, so a predecessor with a strict majority of un-upgraded members cannot be enumerated (the
completion run reports no majority) and a successor with one times out its carry ballots; **a 2.32.0 member of the
retired group votes for the retirement as an ordinary slot of its group — without closing**, since it has no
`Retire` gate kind: a retirement decided with such votes closes only the upgraded members, (i) is false with one such
member and safety rests on (ii), and two such members leave the group's manifests short of a majority — stuck, not
unsafe (§8.5); migrate only once every member of the retired group runs this release; it decodes a genesis that names predecessors in
the 2.32.0 layout — ignoring them — and then computes another electorate digest, so it never answers a successor's
proposals and a successor never answers its; a genesis with no predecessors is byte-identical to 2.32.0's, and the
node-owned acceptor record of a slot never claimed on an electorate group is unchanged. An un-upgraded member's
acceptor record carries no group tag; its slots are reported untagged by an upgraded reporter that shares them and
skipped only when the successor may not decide them at all.

The cluster-scoped exclusive verbs (`distributed_lock`, `LockService`, `consistent_set`, the lock and consistent routes,
the log claim, `mycelium-commitment`'s linearizable award, `set_capability_authz_via_consensus`) decide in the fleet's
**default** electorate (`electorate::live_default`) — a fleet record — or across the cluster when no group declared one:

- **Declared once, at genesis; moved only by retirement.** A genesis may set `exclusive_default` only while no live
  group holds the default — refused by name otherwise, at the door and at every acceptor (round 5). A retirement of
  the holder passes it (`Retirement::passes_default`, checked by every acceptor against what it sees), and the
  successor holds it from that record on, whatever its own flag (§8.3 (v); `electorate::holds_default`). Its members are
  under §8.3's restore-or-retire rule like any electorate's, with the wider blast radius: a default member reissued
  over an empty store endangers every cluster-scoped exclusive verb, not one group's slots.
- **Fail closed.** While the default cannot be named here — two live groups hold it (§8.5), a holder retired to a
  successor not yet visible here, or a `consensus_electorate` that disagrees — every exclusive verb is refused by name, and every acceptor refuses a cluster-scoped proposal for an
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
