# One record per decision: the lifecycle of a lock, a lease and a leadership (row A)

**Status:** **adopted** 2026-10-10 (rev 3: the implementation review on PR #600 — D1–D7, the per-slot sentinel and
index; rev 2 after the design review; rev 1 the draft at `a0acdf20`).
Plan of record: `docs/plans/post-360-hardening.md`, row **A** with **C1** and **C2**. It replaces the reading rules
PR #600 built over three rounds, each broken by its review. The C1 API is unchanged: `elect_leader` /
`elect_leader_receipt` / `elect_leader_with`, `LeaderTerm`, `DEFAULT_LEADER_LEASE`, `release_leadership`,
`ReleaseOutcome`, and the gateway's `POST` / `DELETE /gateway/overlay/elect[/{group}]`.

## 1. Why the reading rules kept failing

A decision lived in three keys, each merged by Layer I's last-writer-wins on HLC:

| Key | Says | Written by |
|---|---|---|
| `consensus/committed/{slot}` | the value | the committer; every learner, from the COMMIT signal, with its *own* HLC |
| `consensus/lease/{slot}` | the window | the committer; the releaser |
| `consensus/decided/{slot}` | the ballot | the committer; every learner, from the COMMIT signal |

They travel independently (different keys; the KV frame and the consensus signal go out on different gossip
shards), so a node can hold any combination of their versions, and LWW can keep a combination for good under
clock skew. Every rule over *the pair* (value, window) has a combination it reads wrong: a newer value under an
older window (round 2, two holders), an older value under a newer window (round 1, live for ever), a newer ballot
under an older window after adoption with skew (round 3, wedged), a release that names a ballot taken from
`decided` (round 3, two leaders). **The decision and its lifecycle were separate LWW records, and HLC order is not
decision order.** This design removes the pair.

## 2. The decision

### 2.1 The envelope — what Paxos decides

Paxos (prepare, accept, vote binding, adoption) decides opaque bytes today. Upgraded proposers make those bytes a
**decision envelope** (`0x4C 0x31` ‖ fixint):

| Field | Meaning |
|---|---|
| `slot` | the slot it decides — inside the digest, so an envelope cannot be replayed into another slot |
| `value` | the caller's value |
| `term` | `Permanent`, or `Lease { ms, expires_at_ms }` — `expires_at_ms` = the original proposer's **wall clock** at the first proposal + `ms` (Q1) |
| `lineage` | the first ballot of the decision's lineage: the proposal's own first ballot, or — for a renewal — the lineage it renews (Q2) |
| `proposer` | the original proposer's `NodeId` |
| `token` | the fencing token, fixed by the original proposer at the first proposal (§2.6) |

Because the envelope *is* the value Paxos carries, **every acceptance carries the whole lifecycle**: a vote binds to
the envelope's digest, a promise reports the envelope, and an adopter adopts the envelope — the holder's
`expires_at_ms`, lineage, proposer and token included (§4). The envelope's digest (SHA-256 of its bytes) is the
decision's identity.

### 2.2 The decision record and the release marker

The slot name is escaped (`%` → `%25`, `/` → `%2F`) so it contains no `/`; then, under the prefix
`consensus/life/{esc}/`, exactly three remainders are valid and everything else is ignored (finding 8):

```
consensus/life/{esc}/{ballot:016x}                       the decision record: the envelope decided at that ballot
consensus/life/{esc}/end/{lineage:016x}-{proposer:016x}  a release marker: ends that lineage of that proposer
consensus/life/{esc}/v/{digest:064x}                     a large value, content-addressed (§2.5)
```

**Every writer of a decision record writes the same bytes** — the committer and every learner apply the envelope
they were given, and nothing in it is local to the writer. LWW between two versions of one ballot's record is
between identical content and decides nothing; **no two decisions share a key**, so LWW never orders two decisions.
Ordering is by the ballot in the key, read by the reader (§2.3). Layer I learns nothing: these are ordinary keys.

A release marker's value is `digest(value ‖ proposer ‖ lineage) ‖ kind ‖ expires_at_ms` — the identity of the
lineage it ends, and the term kind the collector needs (§8). It is a **separate key**, so released beats live by
structure: no write of a decision record can remove or overwrite a marker.

### 2.3 How a node reads it

`top(slot)` = the decision record with the **highest ballot** among the slot's valid record keys that this node
holds with data. Then:

- **ended** iff a marker exists at `end/{top.lineage}-{top.proposer}` whose identity equals `top`'s, **or**
  `top.term` is a lease and **this node's wall clock** is past `top.expires_at_ms` (finding 4: HLC drift can only
  *lengthen* a lease, never end it early);
- **live value** = `top.value` when not ended;
- **ended at** = `top.ballot` when ended — what prepare sets acceptances aside by and acceptors refuse at or below.

`consensus/decided/{slot}` stays as a refusal floor for acceptors and a draw floor for proposers. It is **never**
read to decide liveness.

**Fallback (Q3).** A node falls back to the legacy reading (`committed` + an 8-byte `lease` measured from the
committed entry) **only when the slot has no decision record and no sentinel**. Every writer of a decision record
also writes the slot's **sentinel** `consensus/life/{esc}/s` (the same two bytes, never collected), so a slot that
has ever had a record never falls back, even after its lower records are tombstoned and swept. A slot with the
sentinel but no record held (the top not yet arrived) reads *not live, unknown*: a proposer proceeds to prepare, and
the promise quorum reports what was accepted.

**Reading cost (review D3).** Layer I files every key under `consensus/life/{esc}/` under one scope in a read index
(`KvStore::scope_index`, the same reconcile step as the capability index; a read index, never a write condition), so
`read_slot` costs O(the slot's keys) — in steady state three (§8) — instead of a scan of every `consensus/` key.
Measured (debug build, 5,000 slots × 100 records each, 500,000 records): the prefix-bucket scan the first
implementation used took **136.7 ms** per read; the scope-index read takes **1.35 ms** with all 100 records of the
slot still present, and less after collection.

### 2.4 Why the max-ballot record is the latest decision

Decisions on a slot are decided at strictly increasing ballots: a new decision needs a promise quorum; each quorum
contains an acceptor that accepted the previous decision's ballot `b`, or whose node-owned acceptor record holds a
floor `≥ b` (C2 shrinks to the floor instead of deleting, §8); and an acceptor promises only above both. So the
highest-ballot record a node holds is the latest decision it knows of, whatever the frame order and the writers'
clocks. A node missing the latest record reads an older one — stale as gossip, never wrong in kind: it may report
an ended decision as live (a liveness cost), or an older ended decision as the last one, and then the promise
quorum reports the newer acceptance and the proposer adopts it.

### 2.5 Large values

An envelope over `LIFE_VALUE_CAP` (4 KiB) is written with its value replaced by its digest, and the value under
`v/{digest}`, content-addressed (finding 6): immutable, verified by digest on read, written by the committer and
every learner from the same COMMIT. Until it arrives the decision reads live for every refusal purpose and
`consensus_get` returns `None`. The shared `committed` key is never the source of a value for an upgraded reader.

### 2.6 The fencing token

`token` is set once, at the first proposal: `hlc.tick()` after `hlc.observe(top.token)` for the top record the
proposer holds. Every adoption and every learner copies it. It is therefore above the prior decision's token
whenever the prior decision's record reached the **original proposer** before it proposed — narrower than rev 1
said (finding 9). An adoption keeps the original holder's token, so a holder's guard and the record agree. Fencing
refuses the *older* holder's late writes, which is what it is for.

## 3. What travels on the wire (wire v12; appended variants only)

| Appended variant | Carries | For upgraded peers, replaces |
|---|---|---|
| `PrepareTerm { slot, ballot, proposer }` | phase 1 from an upgraded proposer | `Prepare` |
| `PrepareAckTerm { slot, ballot, voter, accepted_ballot, accepted_digest, accepted: Option<Bytes>, accepted_is_envelope, committed_digest }` | the acceptance, envelope or legacy value, flagged | `PrepareAck` |
| `ProposeTerm { slot, ballot, envelope, proposer }` | phase 2 with the envelope | `Propose` |
| `PromiseTerm { slot, seen_ballot, accepted_ballot, accepted, accepted_is_envelope }` | a refusal's report | `Promise` |
| `CommitTerm { slot, ballot, envelope }` | **the whole decision**: a learner writes the record from it | `Commit` |

An older node decodes an unknown variant as `None` and drops it (the 2.30.0 shape). Votes are the existing
`VoteForValue`, bound to the digest of the envelope.

**A learner never holds a decision without its window**: the window is a field of the envelope, the record is the
only thing an upgraded reader reads, and both ways the decision can arrive — `CommitTerm`, or the record's own KV
frame by gossip or anti-entropy — carry all of it. A crash between the KV frame and the signal leaves survivors with
one complete copy or none.

**The committer**, after quorum: applies the decision record (and the large-value key), `append_sync`s them, then
writes the legacy keys for older readers (`committed` = the raw value, the 8-byte `lease` when leased, `decided`),
then emits `CommitTerm` **and** the legacy `Commit`. Both commit paths do this — `propose` and `cross_propose`
(finding 9).

**An upgraded learner** applying `CommitTerm` writes the record (and value key) unless it holds that exact key as a
tombstone (collected), and raises `decided`. It **ignores the legacy `Commit`** except to note its ballot for the
tripwire (finding 2): it never writes `committed` or `decided` from it. A record whose ballot is implausibly far
above every ballot the node observed is counted by the floor tripwire and still written (detection, not
prevention; finding 9).

## 4. Adoption keeps the original holder's lifecycle (Q1)

A proposer builds its envelope once, at its first ballot: `term` from its config, `expires_at_ms` from its wall
clock, `lineage` = that ballot, `proposer` = itself, `token` per §2.6. Phase 1's rule is unchanged in shape: the
highest-ballot acceptance the promise quorum reports decides, and the proposer **adopts that envelope whole**. An
adopter's own TTL plays no part: re-committing B's lock at ballot 9 re-commits B's `expires_at_ms`, lineage,
proposer and token. An adoption neither shortens nor extends the holder's lease, and the holder's release ends the
adopted re-commit (same lineage, same proposer).

A legacy acceptance (a raw value accepted from an older proposer, flagged `accepted_is_envelope = false`) is adopted
by wrapping it in an envelope with the adopter's term — the one place Q1 is not closed, stated in §7.

**Renewal (Q2)** is explicit. A proposer renews only a decision it **originally proposed**: its live top record's
`proposer` is itself and its `value` equals the value it proposes. The renewal envelope keeps `lineage` — the
lineage it renews — and `value` and `proposer`, and takes a fresh `term` and `token`. Phase 1 decides on the **full**
report set: the renewal keeps its own envelope only when the **highest** acceptance the promise quorum reports is
that lineage's (review D1 — dropping the own-lineage reports and adopting the best of the rest let a renewal adopt a
*lower* value over its own higher decided acceptance); otherwise it adopts the highest, as any proposer does. A release marks the lineage, so it ends the decision **and every renewal of it**, in flight or later
(`release_leadership` can no longer be overtaken by its own renewal); a new election after a release starts a new
lineage.

An acceptance known only by digest (a restarted acceptor whose value was over the record cap) still blocks. The
durable acceptor record gains a flag for envelope acceptances (tag `0x03`).

## 5. Release names exactly the decision it ends

A release writes the marker `end/{lineage}-{proposer}` (and, while older readers exist, the legacy release form —
an 8-byte `lease` of 0, which a 2.31 reader reads as expired; finding 3). It names no ballot, reads nothing from
`decided` or from acceptor memory, and tombstones nothing.

- **`LockGuard`** keeps its envelope's `(lineage, proposer)` and identity from the grant; its release is valid
  whatever this node's view is now. It also exposes the lease's wall-clock `expires_at_ms` and a **monotonic** local
  deadline (proposal instant + TTL) by which the holder must stop (finding 4); a guard is never issued past its own
  deadline — `distributed_lock` polls for convergence rather than sleeping a fixed second, and a grant that
  converges after its deadline is not issued.
- **`release_leadership(group)`** reads `top("leader/{group}")`; releases iff `top.value` is this node's id,
  `top.proposer` is this node, and `top` is not ended; durable (`append_sync`) before `Released`.
  `ReleaseOutcome::{Released, Refused, Unrecorded}` and the gateway's 200 / 404 / 500 are unchanged.

**A release can never end another decision.** A marker ends only records whose `(lineage, proposer, value)` match.
Another decision has another lineage or proposer or value. Two elections of the same node are two lineages.

**Stated limit (finding 10):** a permanent decision whose original proposer is gone for good cannot be released by
anyone else — the release path is the proposer's. A permanent leadership of a dead node therefore stays until the
operator removes the node from the group and the group elects anew under a new slot, or the value is re-decided by
a governance act. Leases do not have this limit.

## 6. Permanent decisions

`term = Permanent`: no window; the marker is the only end, and a permanent marker is **never collected** (§8). A
permanent decision is the top record until a later decision exists, and a later one can only follow a release (a
live permanent decision makes every proposer `Superseded`). A late `CommitTerm` for a released permanent decision
re-writes its record, and its marker still ends it.

## 7. The mixed fleet (2.31.0 and older)

| Older node as… | What happens |
|---|---|
| **acceptor** | ignores `PrepareTerm` / `ProposeTerm`; an upgraded proposer **times out rather than commits** until a quorum is upgraded — the 2.30.0 shape. An upgraded acceptor answers an older proposer's `Prepare` with the legacy `PrepareAck`, reporting an envelope acceptance **by digest only**, so the older proposer is blocked rather than committing envelope bytes as a value. |
| **learner** | reads the legacy keys the committer still writes, and the legacy `Commit`, the 2.31 way. |
| **proposer** | writes only legacy keys; an upgraded node reads that slot the legacy way only while it holds no decision-record key for it. |
| **reader of a release** | sees the 8-byte `lease` of 0: released. |

**An upgraded proposer never sets an acceptance aside by `decided`** (review D2): only the top decision record's
own end does. A decision made by an older proposer is therefore never set aside by an upgraded one; its acceptance
is adopted (wrapped with the adopter's window) and re-committed as a record, which then ends by that window — a
liveness cost of one window in a mixed fleet, never a second value.

**The visibility gap, stated (review D7):** an upgraded node reads a slot the legacy way only while the slot has no
sentinel. A decision an **older** proposer makes on a slot that already has records is invisible to upgraded
readers, which keep reading the slot's top record — upgrade every proposer before relying on a slot.

**Residuals for 2.31 readers, stated:** K3 (a late `Commit` re-stamps `committed` over a release, reviving it for
2.31 readers until the lease rewrite reaches them), round 1's live-for-ever pairings and round 2's two-holder pairing
remain *for 2.31 readers*, which still read the three legacy keys. Upgraded readers are unaffected. The guarantee
holds once every node is upgraded; the legacy keys stop being written at a later, governed MINOR (Q3), recorded in
`deprecations.md`. Q1 is not closed for a value first proposed by an older node (§4).

## 8. C2 against the new record

- **Acceptor state** for a slot whose `top` is ended at `e`, promising nothing above `e`: the node first
  `append_sync`s the top record (and its marker) it holds — a learner's copy is otherwise gossip-durable only
  (finding 5) — then **shrinks** its memory and its node-owned acceptor record to `{promised: e}` (no proposer, no
  acceptance) under the compare-and-set re-check and `acceptor_records`. The floor that refuses `≤ e` is this node's
  own record, never the shared LWW `decided` key, which can regress (finding 5).
- **Decision records below the top** are **tombstoned** by a node holding the top (and their large values, unless
  the top shares the digest), in sorted key order (replay-deterministic); the tombstone sweep removes them. A
  learner **never overwrites a tombstone** it holds with a late `CommitTerm`; a lower record resurrected after the
  sweep sits below the top, or reads as ended (its marker or its window), or is the same identity as the top.
  **Steady state per slot: the sentinel, the top record, the top's marker** — O(1), whatever the history (a test
  shows 1,000 acquisitions of one lock leave at most three keys).
- **Markers:** a permanent decision's marker is **never** collected (finding 1) — **the trade:** one marker per
  released *permanent* decision stays on every node for good, the price of a permanent decision never reviving; a
  lease's marker is collected only once its `expires_at_ms` plus `max_clock_drift_ms` has passed on the collector's
  wall clock (after which the window ends the record without it), and never while its lineage is the top's (so a
  later renewal's longer window is not orphaned). The marker carries the kind and expiry the collector needs.
- **Bounded:** 64 slots a tick, on the collector's own task (first pass one interval after start), candidates
  sorted by slot before a seam-drawn rotation; a state already shrunk is skipped before any record scan, and at most
  1,024 slots are scanned a pass (review D3).
- **The shrunk floor** promises `e` to no proposer (a sentinel `promised_to`), so it refuses a prepare and an
  accept *at* `e` as well as below it.

## 9. Every interleaving, and why it cannot happen

Each row is a test (listeners on every node, structural readiness polls, exact refusals asserted).

| # | Source | Interleaving | Why the design makes it impossible |
|---|---|---|---|
| 1 | K1 | H releases; old state is collected; B's prepare adopts H's value and re-commits it | The release is a marker, never a tombstone; a permanent marker is never collected and a lease marker only after its window has ended the record anyway; the top record is never collected; B reads `top` ended and sets acceptances at or below it aside |
| 2 | K2 | A lease lapses; a proposer holding the commit but not `decided` reopens and adopts the expired value | The end ballot is the top record's own; `decided` is not consulted |
| 3 | K3 | A late COMMIT at a learner missing the floor or the entry revives a released lock | An upgraded learner writes only the record at the COMMIT's ballot, with the committer's bytes; it cannot remove a marker or outrank a higher record. For 2.31 readers K3 remains (§7) |
| 4 | R1-1(a) | A lapsed guard's release lands over a newer holder's state, frames reordered | It writes the marker of its own lineage; the newer holder's record has another lineage or proposer |
| 5 | R1-1(b) | A late COMMIT below a newer record re-stamps the old value | It writes the lower record; readers take the max ballot |
| 6 | R1-1(c) | A learner holds a newer record ahead of its commit, beside an older value | The record carries the value; there is no separate commit for an upgraded reader |
| 7 | R1-2 | A 2.31 learner's re-stamp of `committed` over a newer holder | Upgraded readers do not read `committed` once the slot has a record key; 2.31 readers keep this residual (§7) |
| 8 | R1-3 | `release_leadership` races a renewal by the same node | A renewal keeps the lineage, and the marker ends the lineage: the renewal, in flight or later, is ended too |
| 9 | R1-5 | A crash right after a permanent leader's release | The marker is `append_sync`ed before `Released` |
| 10 | R1-6 | A permanent slot with no `decided` key is released at ballot 0 | A release names lineage and proposer, never a ballot; a legacy (record-less) decision is released the legacy way |
| 11 | R2-1 | A learner holds a newer `committed` + `decided` beside an older window; a third proposer commits beside the holder | Value and window are one record |
| 12 | R2-2 | A permanent commit pairs with a stale window elsewhere and expires | A permanent record has no window and outranks every older record |
| 13 | R2-3 | A live permanent leader is refused its release after a stale-view re-commit higher | The re-commit adopts the envelope — same lineage and proposer; the marker ends both |
| 14 | R2-6 | The C2 floor is lost or regresses (gossip race, shared LWW `decided`) | C2's floor is the node's own acceptor record, shrunk to `{e}` and fsynced; `decided` is not relied on |
| 15 | R2-7 | Failing slots fill every collection budget | Sorted, then a seam-drawn rotation each pass |
| 16 | R2-9 / R3-5 | A forged record far above, a member writing `decided = top + 1`, or a member's COMMIT at `top + 1` | `decided` does not affect liveness; an implausible record ballot is counted by the floor tripwire; a member *signing* a decision is outside CFT (plan §6) and is counted |
| 17 | R3-1 | C adopts vB at 9 while B's record 7 carries a later HLC; wedged | 7 and 9 are different keys; the top is 9 by ballot; 9 is B's envelope, ending at B's window or B's release |
| 18 | R3-2 | A released permanent leader, given `decided = 9` before C's commit, writes `Rel(9, "A")` | A release names only its own lineage and proposer; it cannot address ballot 9 |
| 19 | R3-3 | A crash between the record's KV frame and the COMMIT signal | Either frame carries the whole envelope; the record is fsynced before the COMMIT leaves |
| 20 | R3-4 | An ended decision reads live again when `decided` arrives before a newer commit | Liveness is read from `top` only, and the fallback applies only to a slot with no record key |
| 21 | R3-6 | Collection selects slots in hash order; a replay differs | Sorted before the seam-drawn rotation |
| 22 | Q1 | C adopts B's lock with a shorter TTL; D acquires inside B's TTL | The adopted envelope carries B's `expires_at_ms`; the adopter's TTL plays no part (for values first proposed by upgraded nodes) |
| 23 | Q2 | A late COMMIT on a node with neither `decided` nor a record re-stamps over a permanent decision | The COMMIT writes the record at its own lower ballot; the permanent record outranks it; an upgraded node never writes `committed` from a COMMIT |
| 24 | Review 1 | A permanent leadership's marker is collected; a lower-record tombstone arrives before the top; a late COMMIT resurrects the released record | A permanent marker is never collected |
| 25 | Review 2 | The legacy fallback brings `decided`-driven liveness back (a legacy `Commit` applied first; a member writing `decided`) | Fallback only with no record key; upgraded learners ignore the legacy `Commit` |
| 26 | Review 4 | A fast peer pulls the reader's HLC forward and ends a lease early | Lease expiry is judged on the reader's wall clock |
| 27 | Review 5 | Collection relies on a shared `decided` that regresses, and on a learner's unsynced record | The floor is the node's own acceptor record; the record is fsynced first |
| 28 | Review 8 | A `consensus:write` principal injects a top record into `lock/a` via slot `lock/a/<16hex>` | Slots are escaped, remainders must match exactly, and the slot is inside the envelope's digest |

## 10. What this design does not change, and what it costs

- **Unchanged:** the C1 API; the prepare phase, vote binding and promise rules (over envelope bytes); the floor
  tripwire; the collector's task, budget and lock (`acceptor_records`, lock-order row 56).
- **Cost:** three keys per slot in steady state (§8), one scope read per live read, five appended wire variants, an
  fsync of the record before COMMIT, the legacy keys until a later governed MINOR, and one marker per released
  permanent decision.
- **One number for lease and deadline (review D4).** A lock's lease is its TTL rounded **up** to whole seconds, and
  the holder's monotonic deadline is the acquisition's start plus the TTL — so the deadline is never after the lease
  ends (a 1.9 s TTL leased as 1 s outlived its lease by ~0.9 s).
- **The skew bound, stated (and §10's direction corrected).** Readers judge a lease on their own wall clock against
  the original proposer's wall-clock `expires_at_ms`. A reader whose clock runs **ahead** of the proposer's by δ ends
  the lease **δ early** — and another holder can then be granted while the first is still inside its own deadline.
  The holder must therefore stop δ before its deadline, where δ is the deployment's wall-clock skew bound; nothing in
  the substrate knows δ, so fencing on `token` at the resource is what covers it. A reader whose clock runs behind
  only lengthens the lease.
- **Limits, stated:** a member signing a forged decision is outside CFT — and a forged `CommitTerm` above a live
  decision is now written and counted (review D5: a legitimate handoff whose release marker lags looks the same); a
  permanent decision of a vanished proposer cannot be released (§5); Q1 for values first proposed by an older node
  (§7).
- **Pre-existing, stated: cluster quorum shrinks with the roster.** A cluster-scoped proposal's quorum is a majority
  of `peers + 1` as this node observes them, so when nodes die the quorum shrinks, and two sides of a partition can
  each form one. Several rows here are shown on a cluster scope; their guarantees hold for a stable roster. A fixed,
  governed electorate is post-360 row **P2** (#601); for other cluster-scoped consensus this is the stated limit.

## 11. Decisions on the review questions

1. **Expiry** is fixed at the first proposal on the proposer's wall clock and judged on the reader's wall clock.
2. **Renewal** is explicit (§4): only the original proposer renews, keeping the lineage; a release ends the lineage.
3. **Legacy keys** are written until a governed later MINOR; the fallback is narrowed now to slots with no record key.
