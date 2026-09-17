## [2026-09-17] ingest | item 5 PR 1 — the scoped-mandates ADR, and the axis's one architectural disagreement

Up: [dev](../dev.md) · record `docs/design/scoped-mandates.md` · plan
`docs/plans/v3-contracts-axis.md` §6.3 (D2, D4, D26) · code `mycelium-core/src/signal.rs`, `src/lib.rs`.

Last of the three PR-1 ADRs Phase A's exit gate names. Items 2 and 3 landed earlier today.

### The sharpest sentence in the item

> **Once the protected resource acknowledges installation of epoch E2, no operation authorized only under E1
> can commit there — even if its holder refreshes the content revision, retries, reconnects or restarts.**

Adopted verbatim from the reviewer. The tail is what gives it teeth: *refreshes, retries, reconnects, restarts*
are the four ways a revoked holder ordinarily gets a second chance, and every one of them is a normal,
blameless thing for a client to do.

### CAS is not authorization, and the failure has a name

The wiki's CAS defeats stale **content**. It does nothing about a stale **mandate**:

> A former curator who re-reads the fresh content and re-submits **passes CAS**. Its bytes are current. Its
> authority is not.

So the classification matters more than it looks. **A stale mandate is `MandateSuperseded`, never
`Conflict`** — because `Conflict` is the *retry loop's input*. A caller receiving it is supposed to re-read and
try again. Classify a revoked mandate as `Conflict` and you hand the revoked curator to the exact loop that
refreshes content and re-submits, and the write eventually succeeds. **The retry loop launders the
revocation.**

Today's entitlement is `is_curator: AtomicBool` (`mycelium-wiki/src/agent.rs:214`) — a boolean on the *acting
node*, answering "do I believe I am the curator". That is an inference: not bound to an epoch, not checked by
the resource being written, not invalidated when the appointment moves.

### The disagreement, recorded with both sides

The reviewer proposed a **SQLite-backed daemon per wiki scope** holding the mandate and the canonical commit.
The record declines: that is a control plane for the scope, and the philosophy's line is *"No daemon, no
orchestrator, no control plane"*. **A process that holds the truth is the coordinator arriving through the side
door.**

But the reviewer's *criterion* is accepted in full — **the resource must enforce** — and met by the store that
already serialises the bytes. The mechanism is precise enough to be checkable:

- **`GitStore`:** the mandate check and the content commit are **one** `update-ref --stdin` transaction. Two
  separately successful CAS operations cannot interleave, because there are not two.
- **the shared remote:** `git push --atomic` with `--force-with-lease` on the mandate ref **on every push,
  including content-only writes**. That "including" is the load-bearing word — it makes the mandate check part
  of the same remote ref transaction as the content update, instead of an earlier hook-time read that a
  concurrent appointment can invalidate between read and write. **Fail closed**: a remote that does not honour
  atomic pushes is refused, never downgraded.
- **`FsStore`:** its mutator `Mutex` is per store instance and does not serialise separate processes, so the
  strict profile there is **out of scope**. Stated, rather than left for a deployment to infer from silence.

### Two places the record deliberately holds back

**D2 is conditional on D4.** A mandate *may* become a leased consensus slot — but only after the replay gate
shows its safety assumptions hold. The reason is in the consensus module's own commentary: it describes
near-simultaneous optimistic commitments that *later converge*. **Eventually agreeing on a holder does not
prove that conflicting holders could never both act**, and a commit HLC orders epochs without proving its
bearer was authorized to establish one. Owner-authorized appointment stays the baseline.

**`LockService` gets audited, not replaced.** Its fencing token is the commit HLC (#166). The record declines
to certify its converged-view issuance *and* declines to replace it unexamined — scenario B first.

### The case a hand-written test would omit

Scenario B covers competing appointments, delayed holders, resource restart, expiry, and **revocation with no
subsequent content write**. That last one is the one worth naming: a revocation followed by a write is easy to
observe; a revocation followed by *nothing* is where a mandate silently remains effective, and only an
exhaustive schedule looks there.

### Reservations

`mandate/{scope}` and `log/wiki/{group}/proposals`, in both front doors, before any code writes them. With
item 3's `knowledge/head/`, that is three prefixes reserved today at the ADR stage rather than at first use.

### Gates

`make check` clean · core **186**. Docs plus reservation constants; no behaviour change, no wire change.
