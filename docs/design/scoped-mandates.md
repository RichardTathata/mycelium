# Scoped mandates — who may act as curator, and what stops a former one (ADR, item 5 PR 1)

**Status:** adopted 2026-09-17 · **item 5 PR 1** of `docs/plans/v3-contracts-axis.md` §6.3 (decisions D2, D4,
D26). A contract, not an API. It cites [`threat-model.md`](../threat-model.md) (item 8), depends on
[`contracts-receipts.md`](contracts-receipts.md) (item 1, receipts) and on item 6's replay harness, whose
**scenario B is this record's decisive test**. It is the last of the three PR-1 ADRs Phase A's gate names.

> **Posture, once.** Claims are held at *"enforces configured eligibility rules"* — not at "prevents
> unauthorized writes" in general. Every mechanism below is judged against one sentence (§1), and the
> mechanisms that do not yet meet it are named as such rather than implied to.

## 1. The decisive invariant

Adopted verbatim from the reviewer's response, and everything else in this record is judged against it:

> **Once the protected resource acknowledges installation of epoch E2, no operation authorized only under E1
> can commit there — even if its holder refreshes the content revision, retries, reconnects or restarts.**

Read the tail of that sentence carefully. *Refreshes, retries, reconnects, restarts* are the four ways a
revoked holder ordinarily gets a second chance, and each is a normal, blameless thing for a client to do.

## 2. CAS is not authorization

The wiki's section and manifest CAS defeats **stale content**. It does not defeat a **stale mandate**, and the
difference is the whole reason this item exists:

> A former curator who re-reads the fresh content and re-submits **passes CAS**. Its bytes are current. Its
> authority is not.

So every canonical mutation checks **both** the current revision **and** the current mandate.

**A stale mandate is `MandateSuperseded`, never `Conflict`.** This is not a naming preference. `Conflict` is
the retry loop's input — a caller that receives it is *supposed* to re-read and try again. Classifying a
revoked mandate as `Conflict` would hand a revoked curator to the exact loop that refreshes content and
re-submits, and the write would eventually succeed. **The retry loop would launder the revocation.**

**Every mutation path is protected**: apply, bulk ingest, erase, bootstrap, imports, admin, raw credentials. A
single unprotected path is the whole invariant.

## 3. Today's entitlement, and why it is indicted

`mycelium-wiki/src/agent.rs:214` — the curator's write entitlement is:

```rust
is_curator: AtomicBool
```

A boolean on the *acting node*. It answers "do I believe I am the curator", which is an **inference**, not an
authorization: it is not bound to an epoch, it is not checked by the resource being written, and nothing
invalidates it when the appointment moves. That is the gap §1 closes.

`GitStore` serialises through an `update-ref` CAS plus push (`git_store.rs:656`) under an explicit
**single-writer assumption** (`git_store.rs:168`); `FsStore` returns `WikiError::Conflict` on version alone
(`fs.rs:161`). Proposals are **evaporating KV** — a delivery hint, never a record.

## 4. The mandate contract

One contract shared by curator, primary and proposer — **without shared powers**: holder · establishing
authority · purpose · scope · enumerated operations · **authority epoch** · a separate **term identity** ·
validity · renewal, revocation and outstanding-operation policies · provenance.

**Epoch and term identity are separate fields on purpose.** The epoch orders authority; the term identifies
*which appointment* this is. Collapsing them makes "the same holder reappointed after a gap" indistinguishable
from "the appointment never lapsed", and the three lifecycle events below become unrecordable.

**Three lifecycle events, recorded separately**, because they have different consequences and conflating them
loses the difference:

1. **role expiry** — the term ran out;
2. **permission withdrawal** — the authority revoked it;
3. **outstanding-operation invalidation** — work authorized under the old epoch is now void.

A **handover journal** the successor inherits **as history, not as conclusions**, behind a readiness gate. The
distinction is deliberate: a successor that inherited conclusions would be adopting the predecessor's judgment
without its context, which is what §6.1 of the knowledge record separates as assessment from observation.

Plus **incumbency rules** — consecutive terms, cumulative tenure, cooling-off, eligibility, affiliated
principals — and a **fail-closed authority restart**.

## 5. D26 — the enforcement lives inside the resource's atomic boundary

**This is the one architectural disagreement of the axis, and the record states both sides.**

The reviewer proposed a **SQLite-backed daemon per wiki scope** holding the mandate and the canonical commit.
We decline. That is a control plane for the scope, and the philosophy's line is *"No daemon, no orchestrator,
no control plane"*. **A process that holds the truth is the coordinator arriving through the side door.**

But the reviewer's *criterion* is accepted in full — **the resource must enforce** — and it is met by the store
that already serialises the bytes:

**(i) `GitStore` — one ref transaction.** The mandate check and the content commit are a **single**
`git update-ref --stdin` transaction (`start` / `prepare` / `commit`), updating `refs/mycelium/mandate/{group}`
and the content ref together, each with its expected old value. Two separately successful CAS operations can
never interleave, because there are not two.

**(ii) The shared remote — atomic push, asserted every time.** The curator pushes with **`git push --atomic`**
(the remote updates all requested refs or none) and asserts the mandate ref's current value with
**`--force-with-lease=refs/mycelium/mandate/{group}:<expected>`** on **every** push — *including ordinary
content writes that leave the mandate unchanged*. That "including" is the point: it makes the mandate check
part of the same remote ref transaction as the content update, rather than an earlier hook-time read that a
concurrent appointment can invalidate between read and write. A **pre-receive hook** additionally verifies the
signed epoch and rejects the whole atomic push on any mismatch.

**Fail closed.** A remote that does not honour atomic pushes — which the client learns from the push result —
**is not a supported strict-profile remote**. The write is refused, never downgraded to a non-atomic push.

Locally, every content transaction carries `verify refs/mycelium/mandate/{group} <expected>` inside the same
`update-ref --stdin` transaction, so an *unchanged* mandate is still checked **through commit**, not before it.

**(iii) `FsStore` — out of scope, and said so.** Its mutator `Mutex` is per store instance and does **not**
serialise separate processes. The strict profile on `FsStore` is therefore **out of scope** unless an OS-level
exclusive lock is added. Legacy semantics are **declared, not implied** — this record would rather name a gap
than let a deployment infer a guarantee from silence.

A SQLite service is acceptable **only as an application-owned reference resource** (the effects companion's
destination shape), which is a different thing from a control plane for the scope.

## 6. D2 is conditional on D4 — consensus does not yet establish mandates

A mandate **may** be a leased consensus slot `mandate/{scope}` whose committed value is `(holder, epoch)`, with
the commit HLC as epoch and `committed_lease_secs` as term — **only after** its safety assumptions (membership,
quorum overlap, durable state) and the resource's installation protocol are shown to satisfy the handover
contract under the replay gate.

**Until then, owner-authorized appointment is the supported baseline.** The reason is in the consensus module's
own commentary: it describes near-simultaneous optimistic commitments that *later converge* on a holder.
**Eventually agreeing on a holder does not prove that conflicting holders could never both act** — and a commit
HLC orders epochs without proving its bearer was authorized to establish one.

> **Amendment, 2026-09-17 (the D4 audit — §7.1).** The audit read `distributed_lock` rather than its summary,
> and **the first half of that paragraph is wrong as applied to `LockService`.** The service does not stop at
> optimistic commitment: it reads back the converged value and hands a guard **only** to the proposer whose own
> value survived. Losers receive `Superseded` and never hold a token, so conflicting holders *cannot* both act.
>
> **The second half stands, and is now demonstrated rather than asserted**: the winner is whoever won an
> LWW-HLC race, so the mechanism contains no appointing principal at all. The baseline is unchanged — but for
> the narrower and better-founded reason that **the gap is entitlement, not exclusion.**

## 7. Durable proposals, and not a second fence

- **Durable proposals use the existing log verb** — `KvHandle::append` → `log/wiki/{group}/proposals` — plus
  item 1's receipts. Not a service database. The evaporating queue becomes the discovery hint it always
  effectively was.
- **Do not build a second fence beside `LockService`.** Its fencing token is the commit HLC (#166). This record
  declines to *certify* its converged-view issuance and declines to *replace* it unexamined: it is audited
  under the replay harness (scenario B) before either.

### 7.1 The audit, and its verdict (D4, discharged 2026-09-17)

`src/mandate/lock_audit.rs`. Read from `ConsensusHandle::distributed_lock`, the mechanism is:

1. propose **optimistically** — `Committed` is *not* mutually exclusive (#164 bug A);
2. wait for convergence and **read back the authoritative value** (commit keys are LWW-resolved by HLC);
3. hand a `LockGuard` **only** to the proposer whose own value survived — everyone else gets
   `ConsistencyError::Superseded` and *no token*;
4. the token is the commit HLC, and the resource rejects anything below the highest it has seen.

**Verdict, part 1 — no second fence.** The resource-side rule is structurally the same fence as
`ResourceAuthority`, it already delivers the decisive invariant, and it delivers it for a reason that does not
depend on issuance: the check is at the resource, so it does not care what any holder believes. A second
mechanism beside it would add nothing to the property that matters. **The fence is for the *stale* holder —
Kleppmann's case — not for a concurrent one, which step 3 has already excluded.**

**Verdict, part 2 — what is missing is not a fence.** A token cannot carry an **appointer** (nothing
corresponds to `Mandate::established_by`; winning is a fact about timestamps) and cannot carry **purpose,
scope or enumerated operations** (a token is a `u64`). `WrongScope` and `NotEnumerated` are refusals *no token
could produce* — a correctly-fenced holder passes the fence and is still refused by the mandate. So the lock
supplies **ordering and exclusion**, the mandate supplies **entitlement**. That is a layering, not a duplicate.

Six tests, proved non-vacuous in both directions: breaking the fence fails exactly the two fence tests;
inverting the convergence fails exactly the two issuance tests, and neither break fails the other's tests.

### 7.2 "Every mutation path protected" — what makes that checkable, and the two exempt sites

The plan (§6.3) claims **every mutation path protected**. What makes it true today is that `GitStore` funnels
its content writes through **one chokepoint**, `commit_files`, which carries
`mandate_fence::update_ref_stdin`; and `publish` carries `mandate_fence::push_args`. What makes it *stay* true
is `scripts/check-wiki-mutation-fence.sh`, a baseline-diffing gate in `make check` and CI: a file that gains a
ref-moving site without gaining a fence site fails, and so does a file that loses a fence site. Both directions
are verified by planting the breakage.

Two sites move a git ref **without** the mandate `verify`. Naming them is the point — an "every path" claim with
unnamed exceptions is the thing that decays.

- **`refresh`** — `update-ref <ref> <remote_sha>`, adopting the remote head as local truth. It creates no
  content; it adopts what the remote's pre-receive hook already fenced. **Not a mutation path** in the mandate
  sense.
- **`publish`'s splice retry** — CAS-moves the **local** ref to a two-parent commit that re-parents our
  already-fenced scope files onto theirs. The subsequent push *is* fenced (`--atomic` + `--force-with-lease` on
  the mandate ref), so **the remote is protected**. But the local ref moves without re-verifying the mandate, so
  **a revocation that lands mid-retry-loop is not observed locally until the push fails.** The exposure is a
  local reader served from the local head in that window. Recorded rather than carried silently; closing it
  means threading the fence through the splice path, which is a change to `publish`, not to the contract.

**What the gate cannot see**, stated rather than glossed: it greps for `"update-ref"` literals and
`mandate_fence::` paths. A ref moved through an unknown helper, by `git push` alone, or by writing `.git/refs`
directly is invisible to it; it does not parse Rust and cannot prove the fenced site is the one on the write
path — only that the counts have not drifted.

### 7.3 The partition table

`src/mandate/partition.rs`. §6.3 states the policy in one sentence — *a disconnected curator may prepare
proposals but cannot promise canonical acceptance without reaching the enforcing resource* — and this is that
sentence made checkable.

**It is not a second fence.** D4 was discharged with "no second fence", so the distinction has to be explicit: a
fence is an enforcement point *at the resource*, and it decides what commits. This is neither. It is a
**client-side refusal to promise**, and it cannot stop a curator that ignores it. `ResourceAuthority` remains
the only thing that decides what commits. Without the table the failure is not a safety violation — the fence
still refuses the write — it is a **lie to a submitter**: *"your content is accepted"*, said by someone whose
authority may have been withdrawn ten minutes ago, with the refusal arriving only when the partition heals.

**The table is derived, not chosen**, from an asymmetry that falls out of §4's three-way lifecycle split:

- **Expiry is locally decidable** — `valid_until_ms` is *in* the mandate;
- **revocation is not** — `PermissionWithdrawn` happens at the establishing authority, and a partitioned
  curator cannot distinguish *"still mandated"* from *"revoked ten minutes ago"*.

So: **an action may proceed while unreachable exactly when its correctness does not depend on the mandate still
being current.** That is the payoff for keeping `RoleExpired` and `PermissionWithdrawn` apart — had they been
one event, this distinction would be unstateable.

| Action | Unreachable | Why |
|---|---|---|
| `ReadLocal` | **permitted** | asserts nothing about who may write |
| `PrepareProposal` | **permitted** | an intention, not an acceptance; what it is *worth* is decided later, at the resource |
| `ObserveOwnExpiry` | **permitted** | the window is in the mandate, and expiry only ever *narrows* what this curator claims |
| `PromiseAcceptance` | refused | false the moment the mandate is not current |
| `CommitCanonical` | refused | ditto |
| `RenewOwnMandate` | refused | establishment needs the authority |
| `HandOver` | refused | you cannot transfer what you cannot prove you hold |

The rule and the table are **written twice, independently, and pinned against each other** — a change to either
alone fails the test. Verified non-vacuous by inverting an entry. Six tests, including that the policy neither
refuses everything (which would satisfy every safety statement and make a partitioned curator useless) nor
permits everything.

## 8. The decisive test is replay scenario B

Built once, in item 6, and covering: competing appointments · delayed holders · resource restart · expiry ·
**revocation with no subsequent content write**.

The last case is the one a hand-written test would omit. A revocation followed by a write is easy to observe;
a revocation followed by *nothing* is where a mandate silently remains effective, and only an exhaustive
schedule looks there.

## 9. Reservations made at PR 1

Reserved before any code writes them — `kv_ns` (`mycelium-core/src/signal.rs`) and the crate-doc namespace
table (`src/lib.rs`):

| Prefix | For |
|---|---|
| `mandate/{scope}` | `(holder, authority epoch)` — an **announcement**; the enforcing check is inside the resource (§5) |
| `log/wiki/{group}/proposals` | durable proposals via the existing `append` verb (§7) |

## 10. What this record refuses

- **A resource-authoritative service process** — no daemon, no control plane for a scope (§5).
- **`Conflict` for a stale mandate** — that hands a revoked holder to the retry loop (§2).
- **Consensus-established mandates, for now** — conditional on D4 and the replay gate (§6).
- **A second fencing mechanism beside `LockService`** — audit before certify or replace (§7).
- **A strict profile on `FsStore`** — out of scope until an OS-level exclusive lock exists, stated rather than
  left to be inferred (§5 iii).
- **Any claim stronger than "enforces configured eligibility rules."**

## Appendix — anchors verified at adoption (2026-09-17)

| Claim | Where |
|---|---|
| the curator's write entitlement is a local boolean — the indicted inference | `mycelium-wiki/src/agent.rs:214` (`is_curator: AtomicBool`) |
| `GitStore` serialises through `update-ref` CAS, single-writer assumption | `mycelium-wiki/src/git_store.rs:656`, `:168` |
| store CAS returns `Conflict` on version only | `mycelium-wiki/src/fs.rs:161` (`WikiError::Conflict`) |
| `LockService`'s fencing token is the commit HLC | `src/agent/lock_service.rs` (#166) |
| `mandate/` and `log/wiki/` reserved | `mycelium-core/src/signal.rs` (`kv_ns`), `src/lib.rs` |
