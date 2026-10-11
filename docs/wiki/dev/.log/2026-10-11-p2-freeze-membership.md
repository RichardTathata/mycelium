## [2026-10-11] ingest | P2 round 4 — membership frozen; a change is a migration

- Round 4 of #601's review found the live steps unsafe and unbounded: the drain's report was taken before the step's
  fence was raised (a value self-voted in between was not carried), and a fleet electorate with more open slots or
  bytes than the drain carries could never step. **The maintainer decided: freeze membership.** The one-member steps,
  the drain (`StepPrepareAck`, `DrainPrepare`/`DrainPropose`, `StepRefused`, `StepAbort`), the
  `sys/consensus-slot-group/` index and `StepTooLarge`/`DrainRefused` were removed before release.
- Now (`src/agent/electorate.rs`, `src/consensus.rs`, decision record §8): an electorate is a genesis decided by every
  member it names and never changes; `declare_electorate` on a changed roster is `MembershipFrozen`. **Migration:**
  `retire_electorate(group, successor)` — a majority's decision on `electorate/{group}/retire`, passing the fleet's
  default; every member closes the group at the retirement's promise (gate kind `Retire`, waiting for ordinary claims
  in flight, keeping only what is durable — finding 4). The successor's genesis names the retirement
  (`ElectorateDecl::predecessor`). **Lazy carry:** a successor's phase 1 also reads a strict majority of each retired
  predecessor (`CarryPrepare`/`CarryReport`; a member closes the group and persists the retirement before it
  reports) and adopts the highest acceptance, moving above any reported promise; once it has seen the successor decide
  the slot (`TaskCtx::electorate_carried`, in memory) it stops. The §8.3 argument: retirement fenced at promise ⇒ no
  old value chosen after; reports read after the retirement is decided ⇒ no report-vs-fence race; a chosen old value
  is held by a majority intersecting any majority the carry reads; successor ballots end up above every old ballot,
  so a re-carry after a restart is safe. No bound on slots. Liveness: a reachable majority of the old group, to retire
  it and for each slot's first touch.
- **Genesis supersession** (finding 3): the epoch counts genesis attempts; a declaration from a member holding a
  pending genesis whose roster changed supersedes it at the next epoch, keeping a strict majority of it
  (`check_genesis`); acceptors named by the superseded one must hold exactly it; the gate's `Genesis` kind and
  adoption are ordered on one key (`FenceState::{touched, adopted, opening}`), so a node never adopts an attempt below
  one it touched (§8.4). `Superseded::digest` is the **value** digest (what a restarted acceptor still holds) — a
  first draft used `ElectorateDecl::digest` and no supersession could pass.
- `declare_electorate` / `retire_electorate` and the gateway route retry `NotDecided` with a bounded backoff
  (`electorate::retrying`: 8 tries, 250 ms → 4 s; finding 8), tested on a line topology (`electorate_line`).
- The records rescan (`electorate::refresh_records`) marks a generation scanned only **after** scanning it: marking
  first let a concurrent caller return before the scan had filled the caches, and read a stale epoch (found as a
  flake of `a_forged_electorate_record_is_not_adopted` in the full suite).
- **Rebased onto #600 (row A, locks and leases)**, which made the value Paxos decides a decision envelope: `PrepareIn`
  and `ProposeIn` are now `PrepareTerm`/`ProposeTerm` naming an electorate (answered with `PrepareAckTerm`/
  `PromiseTerm`), a carry report flags an envelope (`accepted_is_envelope`) and goes through `Carry::report`, the
  door reads the caller's value before it is wrapped, an acceptor reads a genesis or retirement through `raw_of`, a
  certificate is the decided envelope with its votes, and `Superseded` also names the envelope digest a restarted
  member still holds. The branch was squashed to one commit for the rebase: #600 rewrote the engine under every
  round's change.
- Test lore: `ConsensusEngine::accept_for_test` builds the envelope with the ballot as its token, so two acceptors
  given one proposal hold the same bytes; with each node's own clock tick they held two envelopes at one ballot, and a
  carry quorum that read both was `Blocked` (2 of 12 loop runs timed out on the carry test before that).
- A forged committed record under an electorate slot is not adopted but still stops a proposer for that slot
  (`Superseded`), as for any slot — a liveness note in §8.5; the TLS test retires another group for that reason.
- Pages touched: `dev/architecture/runtime-invariants.md` (§ Layer III electorate paragraph), `dev/concurrency/
  lock-order.md` (row 57 now names `electorate_line` too). Outside the wiki: decision record §3/§4.2/§6/§7/§8, guide 04
  (migration how-to), threat model §7, `what-is-proven.md` (live reconfiguration: not built — the versioned-electorates
  plan), the plan's P2 row, FAQ, error-handling, diagnostics, production-readiness, CHANGELOG.
