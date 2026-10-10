# Discovery is not an electorate (ADR, post-360 hardening D1)

**Status:** **adopted** 2026-10-10. Records a decision; changes no code. Plan of record:
`docs/plans/post-360-hardening.md` (PR #593) §2 and decision D1, rows **P2**, **C1**, **C2** and **Φ**
(none of the rows is built). It builds on the threat model's supported profile
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
  nodes are in it is a governance act on the group; no configuration names "the consensus nodes". This is the
  rule P2 builds to (not built): today's supported profile states its eligible voter set as node identities, through
  `declare_trust` (§3, residual).
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

**What is not enforced — the residual, stated exactly:**

- **The governance boundary is the gateway's.** An embedded caller's `mesh().join_group` (`mycelium-core/src/mesh_handle.rs:100-107`)
  or an embedded KV write to `grp/{group}/{node}` still moves a governed group's membership, and a peer's write to
  `grp/` is accepted by LWW like any other — detection, not prevention (the philosophy 360 finding Φ3).
- **Nothing requires a governed group for a safety-sensitive proposal.** The profile is chosen per proposal in
  `ConsensusConfig` and nothing validates it: the guarantee `cons.safety_profile` reports `not_verifiable_here`
  (`src/agent/guarantee.rs:852`; [`guarantee-catalogue.md`](../reference/guarantee-catalogue.md)). That is row **P2**.
- **"Governed" is not "fixed".** A governed group's membership still moves — through the governance route, and, on
  nodes that opt in with `start_membership_governor`, by the governor's probabilistic self-election toward the
  intent's `[min, max]` (`src/agent/membership_governor.rs:1-13`). An intent also evaporates: one not re-published
  within `MEMBERSHIP_INTENT_TTL_MS` (5 min) leaves the group ungoverned. The supported profile's *fixed for the life
  of the slot* is the operator's to keep; the code makes moving a governed group a named, audited act at the gateway,
  not an impossible one.
- **Today's profile names its voter set by node identity.** `declare_trust(group, &[NodeId])` is how §7's eligible set
  is stated. D1 says the electorate is *named* as a governed group; reconciling the two is part of P2, and this record
  does not decide whether trust slices remain the vote filter underneath.

## 4. What is planned, and what is a later plan

### 4.1 In the post-360 plan

| Row | Promise (quoted from the plan) | State |
|---|---|---|
| **P2** | "safety-sensitive consensus (the threat model §7 supported profile) requires a governed group, whose membership moves only through a governance route; the secure profile checks it; the electorate is named as a group, never as node identities" | *not built* |
| **C1** | "`elect_leader` is lease-based by default with a release path (permanence stays available, opt-in)" | delivered (#600): `DEFAULT_LEADER_LEASE`, `release_leadership`, `LeaderTerm::Permanent`; gateway `ttl_secs` / `permanent` and `DELETE /gateway/overlay/elect/{group}` |
| **C2** | "acceptor state is collected once its decision or lease is over, without weakening the promise a live slot depends on" | delivered (#600): `ConsensusEngine::collect_finished`, bounded per tick, on its own task |

### 4.2 A later plan — *not built, not scheduled*

**Versioned electorates with joint-consensus transitions** — membership epochs carried on every ballot, and a
transition during which a proposal must win a quorum of the old electorate *and* of the new. Recorded as protocol work
for a later plan under D1 (the post-360 plan's §5). It would let an electorate change under a live slot; it would not
change §2.1 — discovery would still not be an electorate, and the epoch would still name a group, not a node set.

## 5. Consequences

- Adopters read one sentence for safety-sensitive use: **fix the electorate per decision, and fence at the resource.**
  Guide 04, the FAQ, the threat model §7 and `what-is-proven.md` say it and link here.
- Elastic membership (the governor, emergent groups) stays a discovery feature. Using it to grow a group that also
  decides exclusive outcomes is outside the supported profile until P2 is built, and after P2 only through the
  governance route.
- Review comments asking to *prevent membership changes for consensus groups* are answered in part today (the gateway
  refuses a governed group's change except through governance) and in full by P2 (the proposal itself requires it);
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
> change at the gateway except through an audited governance route — but nothing yet *requires* a governed group for a
> safety-sensitive proposal (plan row P2, not built), and an embedded caller can still move a governed group's roster.
> Versioned electorates with joint-consensus transitions are recorded protocol work for a later plan, and a consensus
> *service* — a fixed tier of nodes everyone must reach to agree — is rejected: consensus is a Layer III protocol run by
> whichever nodes are in the group, never a place.
