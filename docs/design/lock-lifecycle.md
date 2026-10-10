# One record per decision: the lifecycle of a lock, a lease and a leadership (row A)

**Status:** **draft for review**, 2026-10-10. Changes no code. Plan of record:
`docs/plans/post-360-hardening.md`, row **A** with **C1** and **C2**. It replaces the reading rules that PR #600
built over three rounds and that each review broke (the round-1, round-2 and round-3 comments on PR #600). The
C1 API is unchanged by this design: `elect_leader` / `elect_leader_receipt` / `elect_leader_with`, `LeaderTerm`,
`DEFAULT_LEADER_LEASE`, `release_leadership`, `ReleaseOutcome`, and the gateway's `POST` / `DELETE
/gateway/overlay/elect[/{group}]`.

## 1. Why the reading rules kept failing

A decision today lives in three keys, each merged by Layer I's last-writer-wins on HLC:

| Key | Says | Written by |
|---|---|---|
| `consensus/committed/{slot}` | the value | the committer; every learner, from the COMMIT signal, with its *own* HLC |
| `consensus/lease/{slot}` | the window (and, in #600, ballot, digest, released) | the committer; the releaser |
| `consensus/decided/{slot}` | the ballot | the committer; every learner, from the COMMIT signal |

The three travel independently — different keys, and the KV frame and the consensus signal go out on different
gossip shards (`framing.rs` shard hashing) — so any node can hold any combination of versions of the three, and
LWW can keep a combination for good once clocks are skewed. Every rule that reads *the pair* (value, window) has
a combination it reads wrong: a newer value under an older window (round 2: two holders), an older value under a
newer window (round 1: live for ever), a newer ballot under an older window from adoption with skew (round 3:
wedged for ever), a release that names a ballot it took from `decided` (round 3: two leaders). The cause is
structural — **the decision and its lifecycle are separate LWW records, and HLC order is not decision order** —
so this design removes the pair rather than patching the reading.

## 2. The decision record

### 2.1 What it is

One record per **decided ballot**, written under its own key:

```
consensus/life/{slot}/{ballot:016x}            the decision record      (written once per ballot; immutable content)
consensus/life/{slot}/end/{origin:016x}        the release marker       (written once, by the holder)
```

The decision record's value, `0x03 ‖ fields` (fixint, like the acceptor record):

| Field | Meaning |
|---|---|
| `ballot` | the ballot it was decided at (also in the key; a mismatch is no record) |
| `origin` | the ballot at which this *(value, term)* pair was **first proposed** — kept by every adoption (§4) |
| `term` | `Permanent`, or `Lease { ms, expires_at_ms }` — `expires_at_ms` is an absolute time on the HLC physical domain, fixed by the original proposer (§4) |
| `digest` | SHA-256 over `value ‖ term ‖ origin` — the decision's identity |
| `token` | the fencing token, fixed by the committer (§2.5) |
| `value` | the value, inline up to `LIFE_VALUE_CAP` (4 KiB); above it, absent (§2.4) |

The release marker's value is the decision `digest` it ends.

**Every writer of a decision record at a given ballot writes the same bytes** — the committer, and every learner
that applies the COMMIT (which now carries all of it, §3). Nothing in the record is local: no HLC of the writer,
no time of arrival. So LWW between two versions of one ballot's record is between identical content and decides
nothing. **No two decisions share a key** — so LWW never compares two decisions. Ordering between decisions is by
the ballot in the key, read by the reader (§2.2). Layer I learns nothing: these are ordinary keys, merged as
every key is.

The release marker is a **separate key**, so "released beats live" is structural: no write of the decision
record — however late, by whatever HLC — can remove or overwrite a marker, and a marker can only ever be *added*.
It is keyed by `origin`, not by ballot, so it ends every re-commit of the same decision (an adoption keeps
`origin`, §4) and nothing else (§5).

### 2.2 How a node reads it

`top(slot)` = the decision record with the **highest ballot** among `consensus/life/{slot}/*` that this node holds
with data (a prefix scan; the prefix index already serves `scan_prefix`). Then:

- **ended** iff a marker exists at `end/{top.origin}` whose value equals `top.digest`, **or** `top.term` is a
  lease and `causal_now_ms > top.expires_at_ms`;
- **live value** = `top.value` when not ended;
- **ended at** = `top.ballot` when ended (what prepare sets acceptances aside by, what acceptors refuse at or
  below, what collection collects at or below).

`consensus/decided/{slot}` stays, as the **refusal floor** only (acceptors refuse at or below
`max(decided, ended_at)`; proposers draw above it). It is **never** read to decide whether a decision is live —
round 3's findings 4 and 5 came from letting it.

A node that holds no decision record for the slot (never decided, or decided only by a node older than this
design — §7) falls back to the legacy reading, unchanged.

### 2.3 Why the max-ballot record is the right one

Decisions on a slot are decided at **strictly increasing ballots**: a new decision needs a promise quorum, every
quorum contains an acceptor that accepted the previous decision's ballot `b` or has a floor `≥ b` (C2 raises the
floor before it forgets, §8), and an acceptor grants a prepare only above both. So the highest-ballot decision
record a node holds is the latest decision it knows of — whatever order the frames arrived in and whatever the
writers' clocks said. A node that has not yet received the latest record reads an older one, which is exactly as
stale as gossip and never *wrong in kind*: it can say "live" for a decision that has ended (refusing proposals,
a liveness cost), or name an ended decision as the last one (and then prepare's quorum reports the newer
acceptances and the proposer adopts them).

### 2.4 Large values

Above `LIFE_VALUE_CAP` the record carries the digest and the term but not the value. A reader takes the value
from `consensus/committed/{slot}` **only when its bytes hash to `top.digest`** (after re-deriving with `term` and
`origin`); otherwise the value is *not yet here* — `consistent_get` returns `None`, `consensus_get` returns
`None`, but the decision is still read as live for every refusal purpose. Locks and leaderships are always below
the cap (`{holder}:{nonce}`, a node id).

### 2.5 The fencing token

`token` is fixed once, by the committer, and copied by every learner. The committer sets it to `hlc.tick()` after
`hlc.observe(prior)`, where `prior` is the highest `token` among (a) the top decision record it holds and (b) the
`latest_token` each promiser reports in its phase-1 answer (§3). It is therefore above the prior decision's token
whenever that decision's record reached the proposer or any member of its promise quorum. That is today's
guarantee ("each observes the prior release") made explicit; the residual — a prior decision whose record reached
none of them — is stated in the guide, and fencing still refuses the *older* holder's writes, which is the
direction fencing exists for.

## 3. What travels on the wire (wire v12; appended variants only)

Four `ConsensusMsg` variants are **appended** — an older node decodes an unknown variant as `None` and drops it,
the shape 2.30.0's `Prepare` used:

| Variant | Carries | Replaces, for upgraded peers |
|---|---|---|
| `ProposeTerm { slot, ballot, value, term, origin, proposer }` | the full candidate decision | `Propose` |
| `PrepareAckTerm { …PrepareAck fields…, accepted_term, accepted_origin, latest_token }` | the acceptance *with its term and origin*, and the acceptor's highest known decision token | `PrepareAck` |
| `PromiseTerm { …Promise fields…, accepted_term, accepted_origin }` | the refusal's report, likewise | `Promise` |
| `CommitTerm { slot, ballot, value, term, origin, token }` | **everything a learner needs to write the decision record** | `Commit` |

`VoteForValue`'s `value_digest` becomes the decision digest (`value ‖ term ‖ origin`) when the proposal was a
`ProposeTerm`; an upgraded proposer counts only votes bound to that digest, as today.

**A learner never holds a decision without its window.** The window is a field of the decision record; the
record is the only thing an upgraded reader reads; and both ways it can arrive — the `CommitTerm` signal, from
which the learner writes the record, or the record's own KV frame by gossip or anti-entropy — carry the whole of
it. There is no ordering between two frames to get wrong, so round 3's finding 3 (a crash between the KV frame and
the signal) has nothing to break: whichever arrives is complete.

The committer: applies the decision record (and, for older readers, `committed`, the 8-byte `lease`, `decided`),
hands it to the WAL with `append_sync`, then emits `CommitTerm` **and** the legacy `Commit`. A learner applying
`CommitTerm` writes the decision record (identical bytes) and raises `decided`; it never writes `committed`'s
liveness or any end state. Apply to the store, then the WAL — unchanged.

## 4. Adoption keeps the original holder's window (Q1)

The candidate a proposer carries is the **triple** `(value, term, origin)`. A fresh proposal sets `origin` to its
first phase-2 ballot and `term` from its config — for a lease, `expires_at_ms = causal_now_ms + ms`, fixed **at
the first proposal**, not at commit, so every later copy of the decision means the same instant.

Phase 1's rule is unchanged in shape and extended in content: the highest-ballot acceptance a promise quorum
reports decides, and the proposer **adopts the whole triple**. An adopter's own `committed_lease_secs` plays no
part: re-committing B's lock value at ballot 9 re-commits B's `expires_at_ms` and B's `origin`. So an adoption
neither shortens nor extends the holder's lease (Q1), and a release by the holder — a marker at `end/{origin}` —
ends the adopted re-commit too (round 2's finding 3).

**Renewal** is the one case where the triple is *not* adopted: a proposer whose own `value` equals the reported
acceptance's value **and** equals its live top record's value proposes `(value, new term, new origin)` at a higher
ballot — a new decision replacing its own live one. Values that can be renewed are holder-unique (a node id, a
`{holder}:{nonce}`, a log claim's holder id), so no other proposer can take this branch for them. A renewal ends
nothing; the old decision is simply no longer the top record. A release of the old origin does not end the
renewal — which is why `release_leadership` documents "stop renewing first" (§5).

An acceptance known only by digest (a restarted acceptor whose value was over the record cap) still blocks, as
today. The durable acceptor record gains `term` and `origin` (a new record tag, `0x03`); a pre-row-A record reads
as a legacy acceptance with no term (§7).

## 5. Release names exactly the decision it ends

A release writes one marker, `consensus/life/{slot}/end/{origin} = digest`, and nothing else — no `committed`
tombstone, no lease rewrite, no ballot taken from `decided` or from acceptor memory.

- **`LockGuard`** keeps the `(origin, digest)` of the decision it was granted (from `ConsensusResult::Committed`,
  which already returns the ballot; the engine returns the triple's origin and digest beside it). Its release is
  valid whatever this node's current view is: the marker ends that decision and only it.
- **`release_leadership(group)`** reads `top("leader/{group}")`; releases iff `top.value` is this node's id and
  `top` is not ended; writes `end/{top.origin} = top.digest`; durable (`append_sync`) before `Released`.
  `ReleaseOutcome::{Released, Refused, Unrecorded}` and the gateway's 200 / 404 / 500 are unchanged.

**It can never end another decision.** A marker is honoured only for a record whose `origin` *and* `digest` match.
Another decision has another origin (it was first proposed at another ballot) or another digest (another value or
term). Two decisions of the same node's leadership have different origins, so releasing one leaves the other
(round 3's finding 2 is impossible: A's release names A's origin and digest; C's decision at 9 has neither). A
stale guard's release (round 1's finding 1(a)) writes a marker for its own, already-superseded origin — a key no
reader consults once a higher record is top.

A permanent decision is released the same way; it has no window, so the marker is its only end.

## 6. Permanent decisions

`term = Permanent`. The record is the top record until a higher-ballot decision exists, and a higher-ballot
decision can only come after the permanent one ended (a live permanent decision makes every proposer
`Superseded`, and its acceptors report it as `DecidedOtherwise`). So a permanent decision cannot be paired with a
stale window anywhere: there is no window, and nothing older can outrank it. The legacy `lease` tombstone a
permanent commit writes today is kept only for older readers.

## 7. The mixed fleet (2.31.0 and older)

| Older node as… | What happens |
|---|---|
| **acceptor** | ignores `ProposeTerm` / `Prepare` (2.31 understands `Prepare` since 2.30; not `ProposeTerm`). An upgraded proposer **times out rather than commits** until a quorum is upgraded — fail-closed, the 2.30.0 shape. |
| **learner** | receives the legacy `Commit` and the legacy keys the committer still writes (`committed`, the 8-byte `lease`, `decided`), and reads them the 2.31 way. Its K3 re-stamp of `committed` changes nothing an upgraded node reads. |
| **proposer** | its decisions write only the legacy keys. An upgraded reader uses the legacy reading for the slot **only when the slot's `decided` is above its top decision record's ballot, or it holds no decision record** — a legacy decision newer than any new one. An upgraded acceptor answering a 2.31 proposer's `Propose` records an acceptance with no term (`term = None` in its acceptor record); an upgraded proposer that adopts such an acceptance proposes it with the legacy semantics of today (lease from the adopter's config) — the one place Q1 is not closed, and only for a value first proposed by an older node. |
| **releaser** | releases by tombstones; only its own guards, which only it holds, and only for decisions it committed the legacy way. |

The guarantee therefore holds **once every proposer is upgraded**, as 2.30.0's did; the upgrade note says so and
says what a mixed fleet does meanwhile. Legacy decisions released by tombstones keep K1's residual (one
re-commit to the old holder for one TTL after the tombstones are collected); new decisions do not have it.

## 8. C2 against the new record

- **Acceptor state** for a slot is collected when `top` is **ended** at ballot `e` and the state promises nothing
  above `e` (re-checked inside the removal's compare-and-set), after the decided floor is on stable storage at
  `≥ e` (`record_decided`, then `append_sync` of the held floor in every case). Unchanged in substance; the end it
  relies on is now the record's, which no pairing can fake.
- **Decision records below the top** are collectable by the node holding the top: tombstone
  `consensus/life/{slot}/{b}` for `b < top.ballot`, and `end/{o}` for markers whose origin belongs to no record at
  or above the floor. A collected lower record that anti-entropy resurrects is harmless — readers take the max.
  **The top record and its marker are never collected**: they are the slot's end, and K1 was the end being
  collected.
- The pass is bounded per tick (64), on its own task, with candidates **sorted by slot** before a seam-drawn
  rotation, so a replay selects the same slots (round 3's finding 6).

## 9. Every interleaving, and why it cannot happen

Each row becomes a multi-node test (a listener on every node, structural readiness polls, frames withheld by
partitioning nodes rather than by writing the store directly, and assertions that distinguish `Superseded` from
`Timeout`).

| # | Source | Interleaving | Why the design makes it impossible |
|---|---|---|---|
| 1 | K1 | H releases; tombstones are GC'd; B's prepare adopts H's value from acceptor memory and re-commits it | The release is a marker (data), never a tombstone; the top record and its marker are never collected; B reads `top` ended at its ballot and sets acceptances at or below it aside |
| 2 | K2 | A lease lapses; a proposer that has the commit but not `decided` reopens and adopts the expired value | The end ballot is the top record's own ballot; `decided` is not consulted |
| 3 | K3 | A late COMMIT at a learner missing the floor or the entry re-stamps a released lock | A COMMIT writes only the decision record at its own ballot, with the committer's bytes; it cannot remove a marker and cannot outrank a higher record |
| 4 | R1-1(a) | A lapsed guard's release lands over a newer holder's state, frames reordered | The release writes `end/{its origin}`; the newer holder's record has another origin and digest and is unaffected |
| 5 | R1-1(b) | A late COMMIT below a newer record's ballot re-stamps the old value | It writes the record at the lower ballot; readers take the max ballot |
| 6 | R1-1(c) | A learner holds a newer record ahead of its commit, beside an older value | There is no separate commit: the record carries the value |
| 7 | R1-2 | A 2.31 learner's K3 re-stamp of `committed` over a newer holder | Upgraded readers do not read `committed` while they hold a decision record at or above `decided` |
| 8 | R1-3 | `release_leadership` races a renewal by the same node | A renewal is a new decision with a new origin, so the release cannot end it — stated, unchanged API |
| 9 | R1-5 | A crash right after a permanent leader's release | The marker is `append_sync`ed before `Released` |
| 10 | R1-6 | A permanent slot with no `decided` key is released at ballot 0 | A release names `origin` and `digest` from the record, never a ballot from `decided` or memory; a legacy (record-less) decision is released the legacy way (§7) |
| 11 | R2-1 | A learner holds a newer `committed` + `decided` beside an older window; a third proposer commits beside the live holder | Value and window are one record; a learner holding B's decision holds B's window |
| 12 | R2-2 | A permanent commit pairs with a stale window elsewhere and expires | A permanent record has no window and outranks every older record |
| 13 | R2-3 | A live permanent leader is refused its release after a stale-view re-commit at a higher ballot | The re-commit adopts the triple, so it has the leader's origin and digest; the marker ends both |
| 14 | R2-6 | Gossip raises the floor between collection's check and its write, skipping the fsync | The held floor is `append_sync`ed after `record_decided` in every case |
| 15 | R2-7 | Failing slots fill every collection budget | Sorted, then a seam-drawn rotation each pass |
| 16 | R2-9 / R3-5 | A forged record or `decided` far above, or a member's COMMIT at `top + 1` | `decided` no longer affects liveness; a record far above every observed ballot is counted by the floor tripwire; a member *signing* a decision is outside CFT (plan §6's recorded limit) and is still counted |
| 17 | R3-1 | C adopts vB at 9 while B's record 7 carries a later HLC; LWW keeps 7; `decided = 9`; wedged | 7 and 9 are different keys; `top` is 9 by ballot; 9 carries B's term and origin, so it ends at B's window or B's release |
| 18 | R3-2 | A released permanent leader, given `decided = 9` before C's commit, writes `Rel(9, "A")`; D commits beside C | A release names only its own origin and digest; it cannot address ballot 9 |
| 19 | R3-3 | A crash between the record's KV frame and the COMMIT signal leaves survivors with the commit and no window | Either frame carries the whole decision record |
| 20 | R3-4 | An ended decision reads live again when `decided` arrives before a newer commit; a caller acts as leader | Liveness is read from `top` only; `decided` is a refusal floor |
| 21 | R3-6 | Collection selects slots in hash-map order; a replay differs | Candidates sorted by slot before the seam-drawn rotation |
| 22 | Q1 | C adopts B's lock value with a shorter TTL; D acquires inside B's TTL | Adoption carries B's `expires_at_ms` and `origin`; the adopter's TTL plays no part |
| 23 | Q2 | A late COMMIT on a node with neither `decided` nor a record re-stamps over a permanent decision | The COMMIT writes the record at its own (lower) ballot; the permanent decision's record outranks it |

## 10. What this design does not change, and what it costs

- **Unchanged:** the C1 API; `LeaseRecord`'s 8-byte prefix for older readers; the prepare phase, vote binding and
  promise rules; the floor tripwire; the collector's task, budget and lock (`acceptor_records`, lock-order row 56).
- **Cost:** one more key per decided ballot (collected below the top), one prefix scan per live read (the prefix
  index serves it), a decision record of up to `LIFE_VALUE_CAP` per slot on every node, and four appended wire
  variants. The legacy keys are still written until the 2.31 window closes (a later MINOR, recorded in
  `deprecations.md` with this change).
- **Stated limits, carried:** "lapsed" is read on the causal clock under bounded skew; fencing is the guarantee
  that survives the skew; a forged decision signed by a member is outside CFT.

## 11. Review questions

1. Should `expires_at_ms` be fixed at the first proposal (this draft) or at the first commit? At proposal the
   holder's window starts ~one round early; at commit, an adoption of an *uncommitted* acceptance would have no
   instant to inherit.
2. Is the renewal branch (§4) narrow enough — holder-unique values only — or should renewal become an explicit
   flag on `ProposeTerm`?
3. Should the legacy keys stop being written once every node in `peers()` advertises the new variants, rather than
   at a later MINOR?
