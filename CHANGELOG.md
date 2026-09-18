# Changelog

All notable changes to this project will be documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added — the rights ledger (item 4 PR 3)

- **`mycelium::control::ledger`** — the ledger of allocated rights the ADR's §4 decided, on the node-local
  journal (`agent::journal`, now **ungated**, with `sha2` unconditional: a minimal build gains one small
  pure-Rust crate already in every `tls` tree). `Right { holder, resource, units, allocated_by, term, state,
  valid_until_ms }` — native units, item 5's `TermId`, five states **all of which count** (`Unknown` is a
  state, not a zero).
- **Persist, then apply.** Every mutation is appended and fsynced first and applied to the view only on an
  `OnDisk` receipt; a record that did not reach disk allocates nothing and the refusal carries the journal's own
  reason (`NotDurable`). **No method takes a peer set** — a holder that vanishes from discovery keeps its right,
  and re-allocating a live term is `Duplicate`; a released, expired or revoked one *is* reissuable. Both halves
  are tests, because a ledger that passed only the first would never free anything.
- **`admission.rejected` is a journal record**, beside completions: `admit` refuses an over-request *and*
  records it, and the count survives a reopen. `may_admit` is the pure check; consumption of units against a
  right (reserve → act → reconcile) is PR 4.
- **A journal the ledger cannot decode fails the open** rather than starting empty over a full file — which
  would readmit nothing and reissue everything.
- **`RightsHead`** — the bounded claim that may enter the medium (`rights/head/{holder}`): live units per
  resource, sorted, at the last recorded sequence; canonical bytes via `serde_fixint` (golden-pinned, already
  the byte layer under the audit chain); `verify` under `tls`. Publishing it is PR 4.
- Nine tests (one `tls`-gated). The dead-code trap bit once more on the way — `Journal::path()` had only a
  gated caller — and the fix is a real use (the ledger folds from the journal's own path), not an `allow`.

### Changed — the node-local journal is its own mechanism (item 4 PR 3a, a pure move)

- **`agent::journal`** now holds the append-only, fsynced, never-gossiped journal — the file, the bounded queue,
  the writer, the reader and its cursor — and **`EvidenceJournal` is a thin AE profile over it**. Every public
  name the evidence journal exported is still exported from the same place, unchanged; the AE journal still
  records its queue under the replay stream `ae/journal`, so a trace recorded before the split still matches.
  The rights ledger (item 4 PR 3) is the mechanism's second user; two fsynced journals side by side is how
  guarantees drift, which is why the move comes first.
- The replay-seam **stream is now a parameter of `Journal::open`**, not a constant — the inventory's rule is
  *stream identity per destination*, so a replay of one journal can never hand another its verdict.
- **Gated on its first user's features for now.** An ungated module with no user in a minimal build is dead code
  there, and `--no-default-features` clippy said so on the first attempt — the feature-gated dead-code trap,
  caught by the gate built for it. PR 3 ungates the journal and makes `sha2` unconditional in the same change
  the ledger lands, when there is a user in every build.
- The eleven mechanism tests moved with the mechanism; one wrapper pin added. The seams baseline was
  regenerated because five forbidden sites moved file, one to one.
- **A blind spot in the forbidden-call check, found by the move.** The first regeneration counted *four*
  sites in the new file for the same five calls: the check skips from a `#[cfg(test)]` to the next brace at
  column 0, so a test-only method gated *inside* the `impl` hid everything after it — including `append`'s
  `tokio::time::timeout` — from the gate. The test-only helpers now live in a separate top-level
  `#[cfg(test)] impl` block, and the script's "what it cannot see" section records the rule: a baseline that
  moves when no site moved is the signal.

### Added — the timer seam, second arm: periodic ticks (item 6 PR 3 tail)

- **`sim_seam::interval_ms(stream, period_ms, missed) → Ticker`**, with `Ticker::tick()` a drop-in for
  `tokio::time::Interval::tick`. Without `sim` it is the interval it replaced. Under a kernel, the recorded
  decision is the **nominal schedule** — an immediate first tick, then one period each — so a replay ticks the
  loop's written cadence rather than the wall's jitter, and **never wall-waits**. The stream is owned, built once,
  so a loop that runs per kind gets a name per kind (stream identity per destination).
- **A tick and a sleep are different requests** (`tick(30ms)` vs `sleep(30ms)`): the kernel's `Timer` choice now
  carries the operation, so a trace can never replay a periodic loop from a one-off wait or the reverse — pinned
  by a test that replays a recorded sleep as a tick and diverges.
- Nine sites routed in `src/agent`: the cluster tuner, opacity (per kind), the emergent detectors, `gcap` reassert,
  the intent reconciler (per key), the A2A sweep, the health ticker (both constructions) and GC. `membership_governor`
  waits for the item 4 stack; `swim.rs` and `mycelium-core`'s four tickers are the next batch.
- **A second blind spot in the forbidden-call check, found by the routing.** Its `tokio::time::*` pattern does not
  see `time::interval` through a `use tokio::time` alias, so three of the nine sites had never been counted and the
  baseline moved for only four files. The same alias gap the check's header already closes for `fs`; recorded there,
  and its own change, since closing it regenerates a baseline of pre-existing sites across the tree.
- Three seam tests (nominal schedule recorded; replay advances the clocks without waiting; sleep-as-tick diverges).

### Added — the adaptive-stability contract types (item 4 PR 2)

- **`mycelium::control`** — pure decisions only; no governor changes, no actuator touched, the rights ledger is
  PR 3. `ActionClass` (speculative scale-up · routine scale-down · protective shed · rescue from zero) and **the
  decisive rule as one function**, `holds_on_uncertainty`: *uncertainty holds speculation and routine scale-down;
  it never holds protective shedding or rescue from zero.* The tests write the same table by hand and pin the two
  against each other, so the rule cannot drift from its statement.
- **`decide(class, &ViewConfidence, &ConfidenceBound, Profile)`** takes the real, public `ViewConfidence` — no
  parallel struct — and reports *why* it held (`Uncertainty::{StalenessUnknown, Stale, TooFewHeard,
  SelfDegraded}`). **An isolated node is uncertain, not fresh**: the WP5 `staleness_known` correction is now
  consequential rather than advisory.
- **Four profiles, shadow first.** `Legacy` never consults the predicate; `Observe` **records a would-hold and
  proceeds** — a distinct `Decision::WouldHold` variant, not a flag, so a caller cannot mistake it for `Proceed`;
  `EnforceLocal` and `EnforceAllocated` hold.
- `ControlSpec` (one governor, one actuator, spacing, settle timeout, bound, profile), a stable `ActionId`, and
  the two new loop-breakers as pure checks: **spacing** (saturating — a clock that reads earlier than the last
  action refuses rather than wrapping into "long ago") and **settling** (no proposal while the last action is
  unobserved and inside the timeout; past it, settled as `unknown`).
- Nine tests; the rule and the observe-proceeds property each verified by planting their inversion.

### Added — the adaptive-stability ADR (item 4 PR 1)

- **`docs/design/adaptive-stability.md`** — the contract behind the governors, and the record that unblocks the
  resource-accounting slice (D30: the ledger's vocabulary and backend are item 4's decisions). Three promises
  kept apart with their strength in the **guardrails tiers** (D17): a hard bound is `HardPrevention` *only* when
  backed by exclusive, durably accounted rights; otherwise it is a convergence target, the membership governor's
  own honest phrase.
- **The decisive rule:** *uncertainty holds speculation and routine scale-down; it never holds protective
  shedding, and never holds rescue from zero capacity.* `ViewConfidence` becomes actionable and per-input through
  a predicate over four action classes — PR 2 makes it pure and swept.
- **The rights ledger's shape is decided:** rights **cannot live in gossip KV** — soft state that evaporates is
  right for an advertisement and issues a right twice when its holder drops out of discovery. The ledger is a
  node-local, fsynced, never-gossiped journal in the `EvidenceJournal`'s shape on item 1's durability; only a
  bounded signed head gossips. `Right { holder, resource, units, allocated_by, term, state }` — native units,
  item 5's term identity, five counted states including `unknown`; persisted before acting; **never reclaimed
  because an owner vanished from discovery**; `admission.rejected` a first-class outcome. Exclusivity is by
  allocation, not by consensus.
- One owner per actuator and **one owner per deficit**, named; loop-breaking points named (the existing `gate`
  hysteresis and WP5 cooldown kept; spacing and settling new); the combined-feedback test is replay stage 6
  (D19); the workload probe consumes the companions' depth signals (D18); four profiles, shadow first.
- `rights/head/{holder}` reserved in the namespace table and `kv_ns::RIGHTS_HEAD`. Nothing writes it until PR 3.

### Added — the timer seam, first arm: fixed sleeps, and the converge sleep routed (item 6 PR 3 tail)

- **`sim_seam::sleep_ms(stream, ms)`** — the replay inventory's §2.3 row for *fixed sleeps inside protocol
  logic whose duration is a correctness assumption*. Without `sim` it is `tokio::time::sleep`, nothing else. With
  a kernel: `Record` really sleeps and writes down that the wait elapsed; **`Replay` never wall-waits** — the
  recorded effective duration advances both simulated clocks (`Sources::advance_ms`, new; unlike a wall *jump*
  the monotonic clock moves too) and the task yields once, so the await point stays and the wait goes. A
  request that differs from the recording is a divergence printed with both sides.
- **The 1 s "let the winning commit converge" sleep after `distributed_lock`'s optimistic commit is now
  routed** as `lock/converge`, its twin in `elect_leader` as `elect/converge`, and the ballot defers as
  `consensus/defer` / `consensus/suggest-defer`. `consensus_handle.rs` drops from 9 forbidden sites to 3 in the
  seams baseline. The D4 audit could only *model* that path because this seam did not exist; it now can be
  recorded.
- **A boundary found by writing the test the other way first.** Because replay checks the request but supplies
  the result, an edited `timer` line makes the replayed wait return `0` or `5000` — but in exact replay the
  clock reads that follow are supplied from the trace too, so they still say what the recording said. **Exact
  replay reproduces; it cannot explore.** Re-deriving those reads after an authored wait is *scenario replay*,
  the inventory's third mode, which the two-mode kernel does not have. The test pins both halves so the
  assertion changes deliberately when that mode lands, and the seam, kernel and inventory say "the hook, not the
  exploration" rather than the claim I first wrote.
- Four seam tests; the no-wall-wait and clock-advance properties each verified by planting their absence.

### Added — the knowledge layer's adapters (item 3 PR 7 — item 3 complete)

- **The trace adapter** (`mycelium-reason`, new feature `knowledge`): a `TraceEvent` becomes a knowledge
  **observation** — a report of what happened, which §1's record-type split keeps from ever counting as
  evidence. §6's rule — *add `derived_from` explicitly, never infer it from HLC adjacency* — is **structural,
  not a convention**: `observation` takes derivation as an argument the caller must assert, and the batch form
  `observations` has no parameter through which links could be supplied, so it cannot invent any. Planting the
  violation (chaining a run by HLC order) fails exactly the one test about it.
- **The AgentFacts adapter** (`mycelium-agentfacts`): a self-signed document becomes a knowledge **claim** —
  what a node says about itself — **after** its signature verifies; a tampered or key-swapped document yields no
  record at all. The **issuer is the signing key**, not the `node_id` string anyone can type. The point is the
  kind: a perfectly signed claim filed under a release with a `supports` link is *still* not evidence, because a
  node cannot vouch for itself — and the kind, not the signature, is what enforces that. Swapping the kind
  fails exactly the two tests about it.
- **`mycelium::hlc` is now public** (additive). It was `pub(crate)`, which left a companion crate that hands
  out packed HLC timestamps (`TraceEvent.hlc`, log keys) with no public way to read them short of copying the
  bit layout. `mycelium-core` already had `pub mod hlc`, so this commits to nothing that crate did not.
- Record-id determinism is pinned: the same JSON detail built in two key-insertion orders yields one id. Cargo
  unions features across the graph, so a dependency enabling `serde_json/preserve_order` later would silently
  break canonical bytes — that test is what would catch it.
- Ten tests across the two crates, three claims verified non-vacuous by planted breakage. Item 3 (the knowledge
  layer) is complete at PRs 1–7.

### Added — the knowledge layer's semantic gate and demonstration (item 3 PR 6 — the contract completes)

- **The semantic gate is now a named, separately-runnable thing** (`make gate-knowledge`, and its own CI step),
  because §8 calls it a Phase D exit condition and an exit condition buried among hundreds of tests is not
  locatable. Three negative cases: misleading evidence cannot silently **erase** a conflicting observation,
  cannot **refresh** expired evidence, cannot **confer** authority.
- **Two positive controls, because all three negative cases are refusals.** The record names the trap — *"a
  resolver that rejects everything looks safe while being useless"* — and all three would pass against a
  resolver that refused unconditionally. Verified: making `classify` refuse everything leaves the three negative
  cases green and fails **only** the positive control.
- The erasure case is the sharp one: **fifty supporters do not bury one challenge.** The verdict is `Conflicted`,
  the challenging record is still in the store, and it is still `Current` — §9's refusal of *"consensus over
  truth"* made executable. There is no vote.
- `examples/knowledge_layer.rs` runs the lifecycle end to end (claim → observation → assessments → resolution →
  the same reader learning two labs share a funder → a challenge → retraction reaching what was built on it) and
  **closes by printing what it does not demonstrate**: no transport, no signature checking, no reputation score,
  no behavioural claim, and the disagreement is *not* resolved.

### Added — expiry and correction, with the dependency index (item 3 PR 5)

- **A retraction now reaches what was derived from it**, rather than waiting to be noticed.
  `DependencyIndex` inverts the `DerivedFrom` links (which point *upward*, from a record to its bases), so
  `transitively_affected` answers "what does this retraction touch?" without rescanning every record.
- **Withdrawing a basis is not withdrawing the conclusion** — the decision the module turns on. A `Retracts`
  link is same-issuer only, but records by *other* issuers may have been derived from the withdrawn one. Those
  become `Standing::BasisWithdrawn { basis, hops }` — **a fact about the record's support, not a verdict on the
  record**. Issuer A withdrawing its observation gives A no standing to withdraw B's conclusion; B may stand by
  it on other grounds. Collapsing the two would let any issuer silently invalidate anyone's conclusions by
  retracting something they had cited, which is erasure wearing a correction's clothes.
- **Nothing is deleted.** `standing` computes a view; the store's length is unchanged, pinned by a test, because
  "a retraction reaches downstream records" is one short step from "a retraction deletes downstream records".
- **Expiry needs no timer.** It is a pure function of the reader's `now_ms` and the record's own `at_ms` — there
  is no wait to reproduce and nothing to schedule, so a replay gets it right without the clock seam being
  involved at all.
- Also: the **nearest** withdrawn basis is reported (the most actionable one); an **absent** basis is not
  treated as a withdrawal (otherwise a partial replica looks like a wave of retractions); retraction is reported
  ahead of expiry; and a forged/corrupted `DerivedFrom` cycle terminates — stated plainly as the only way a
  cycle can arise, since a content-addressed id cannot honestly contain a link that depends on itself.
- `KnowledgeStore::records()` added (additive) — an iterator over held records, explicitly unordered.
- Eleven tests; the transitive reach and the `Retracted`/`BasisWithdrawn` split each verified non-vacuous by
  breaking them.

### Added — evidence-aware resolution (item 3 PR 4)

- **It wraps `resolve_for_caller`; it does not replace it.** The native gates (`is_fresh`, schema id) run first
  and an entry that fails them never reaches evidence evaluation. `mycelium::knowledge::resolution` then binds
  to the exact release, classifies, and returns one of four outcomes **with reasons**.
- **Evidence decides eligibility; the router decides choice.** `filter_accepted` *filters and never reorders*,
  so a well-evidenced but overloaded provider cannot beat a better-placed one (or the reverse). Interleaving the
  two is the thing this ordering exists to prevent.
- **Evidence never grants what authorization denies** — it can only narrow. The module takes already-authorized
  candidates and has no way to add one.
- The four rules are mechanisms rather than prose: **no reputation scalar** (nothing aggregates across subjects,
  so "good at X" cannot imply "good at Y" — no value spans them); **identity is not independence** (independence
  is a reader-configured control group, never inferred from issuer keys); **missing evidence is uncertainty**
  (`InsufficientEvidence` is distinct from `Rejected` so a reader can tell "we do not know" from "we looked and
  it is bad"); and **refreshing an advertisement never refreshes evidence** (evidence ages on its own record
  timestamps, so a liveness heartbeat cannot launder a stale assessment).
- Also enforced: only an **assessment** counts as evidence — a claim is what someone said about themselves and
  an observation is a report; **a provider cannot vouch for itself** (the issuer is in the record id, so this is
  checkable without a fetch); and **evidence binds to one release**, with a length-prefixed subject so a crafted
  release name cannot collide with another's.
- Twelve tests; the independence-by-group and self-assessment rules verified non-vacuous by breaking each.

### Added — the partition table: what a disconnected curator may do (item 5)

- **The policy sentence made checkable.** §6.3 says *a disconnected curator may prepare proposals but cannot
  promise canonical acceptance without reaching the enforcing resource*. `mandate::partition` is that sentence
  as a table, with `Reachability`, `CuratorAction` and `permitted`.
- **It is not a second fence** — D4 was just discharged with "no second fence", so the distinction is explicit.
  A fence is an enforcement point *at the resource* and decides what commits; this is a **client-side refusal to
  promise**, and it cannot stop a curator that ignores it. Without it the failure is not a safety violation (the
  fence still refuses the write) but a **lie to a submitter**, with the refusal arriving only when the partition
  heals.
- **The table is derived, not chosen.** The root asymmetry: **expiry is locally decidable** (`valid_until_ms` is
  in the mandate) but **revocation is not** — a partitioned curator cannot distinguish "still mandated" from
  "revoked ten minutes ago". So an action may proceed while unreachable exactly when its correctness does not
  depend on the mandate still being current. This is the payoff for keeping `RoleExpired` and
  `PermissionWithdrawn` apart: had they been one event, the distinction would be unstateable.
- **The rule and the table are written twice, independently, and pinned against each other** — changing either
  alone fails. Verified non-vacuous by inverting an entry. Six tests, including that the policy neither refuses
  everything (which would satisfy every safety statement while making a partitioned curator useless) nor permits
  everything.

### Added — the D4 audit: `LockService` under scenario B, with a verdict (item 5)

- **D4 discharged: no second fence.** The audit (`mandate::lock_audit`) read `distributed_lock` rather than its
  summary, and **found the premise of the concern wrong.** The service does not stop at optimistic commitment:
  it reads back the converged value and hands a `LockGuard` **only** to the proposer whose own value survived.
  Losers get `Superseded` and *never hold a token*, so two concurrent holders cannot both stamp writes. The
  fence exists for the **stale** holder — Kleppmann's case — and it delivers the decisive invariant for a reason
  independent of issuance, because the check is at the resource. A second fence beside it would add nothing.
- **What `LockService` does not supply is entitlement, and no fence reaches it.** A token cannot carry an
  appointer (the holder is whoever won an LWW-HLC race; nothing corresponds to `Mandate::established_by`) and
  cannot carry purpose, scope or enumerated operations (it is a `u64`). `WrongScope` and `NotEnumerated` are
  refusals *no token could produce* — demonstrated against the real `ResourceAuthority`, where a
  correctly-fenced holder passes the fence and is still refused. The lock supplies ordering and exclusion; the
  mandate supplies entitlement.
- **This amends the adopted record.** `docs/design/scoped-mandates.md` §6 claimed that eventual agreement "does
  not prove that conflicting holders could never both act". For `LockService` the read-back filter does prove
  exactly that, so §6 carries a dated amendment and the verdict is §7.1. **D2's baseline is unchanged** — but
  now for the narrower, better-founded reason that the gap is *entitlement, not exclusion*.
- Six tests, non-vacuous in both directions: breaking the fence fails exactly the two fence tests; inverting the
  convergence fails exactly the two issuance tests; neither break disturbs the other's.

### Added — replay scenario B, the decisive test for scoped mandates (item 6 PR 5)

- **A schedule sweep, not five hand-written tests.** Five tests check five orderings someone thought
  of; the invariant is about *every* ordering, and the ones that break it are the ones nobody
  pictured — which is the entire argument for item 6 existing. The sweep generates schedules from
  §8's five cases (competing appointments · delayed holders · resource restart · expiry ·
  **revocation with no subsequent content write**) and asserts the invariant after **every step of
  every schedule**, not only at the end.
- **The case the ADR singles out is the one a hand-written test omits.** A revocation followed by a
  write is easy to observe — something fails. A revocation followed by *nothing* is where a mandate
  silently remains effective, because nobody looked. The sweep includes idle schedules that knock
  only much later.
- **The sweep is proved non-vacuous**: a deliberately broken resource — one that checks against
  epoch `0` rather than what it installed — is caught. And a *current* mandate still commits, so the
  invariant cannot be satisfied by refusing everything, which is the failure mode the knowledge
  layer's own gate warns about.
- Expiry is reported as `OutOfWindow`, not as supersession: a current mandate past its window failed
  for a different reason and says so.


### Added — fail-closed authority restart and durable proposals (item 5 PR 5)

- **`RestartGuard` starts closed and admits nothing** until its installed epoch is read back from
  durable state. The alternative that matters is spelled out by a test: had a restart **assumed
  epoch `0`** — which is what a `Default` hands you — every superseded mandate would satisfy
  `epoch >= installed`, and **every revoked holder would be readmitted at once**. The test asserts
  that outcome against an assumed-zero authority, so the cost of the tempting default is on the
  record rather than in a comment.
- Re-establishing is monotonic: a stale durable read cannot walk the epoch backwards, which is the
  decisive invariant's whole subject. Same shape as the federation link's `Refreshing` state —
  *reconnected is not ready, and restarted is not authorised.*
- **Durable proposals use the existing `append` verb**, not a service database. §5 refuses a
  resource-authoritative service process, and a durable proposal queue is exactly where one would
  sneak back in — it looks like storage rather than a control plane. A live test appends a proposal
  through `KvHandle::append`, checks it landed under the `log/wiki/` prefix reserved at PR 1, and
  reads it back byte-identical through `scan_log`.
- A `Proposal` carries the **term** that made it: a proposal outlives its appointment, and a reader
  needs to know which one it was rather than inferring it from a timestamp.


### Added — the handover journal and incumbency rules (item 5 PR 4)

- **"As history, not as conclusions" is a type, not an instruction.** `HandoverJournal::inherit`
  does not return entries — it returns `Inherited`, in which a conclusion is *always attributed*.
  There is no way to obtain a bare conclusion, so a successor cannot adopt its predecessor's
  judgement without its context, nor become its apparent author. The same distinction the knowledge
  layer draws between observation and assessment, applied where it is easiest to lose: at a change
  of personnel.
- **The readiness gate.** A successor is ready when it has read what it inherited, not when it was
  appointed. `NotReady` carries how much is unread, because *"not ready"* with no number is
  indistinguishable from a successor that is stuck. Progress is monotonic — one that could walk its
  own progress backwards could pass the gate and then claim it had not.
- **Incumbency rules**: consecutive terms, cumulative tenure, cooling-off, and **affiliated
  principals** — without the last, rotating between two identities of the same operator would
  satisfy every limit while changing nothing.
- Claims held at the record's own wording: this **enforces configured eligibility rules**. Unset
  rules mean unlimited, because a default limit would be the library deciding an operator's
  governance for them.
- `mandate`'s module doc corrected: it said the enforcement point was a later PR, and PR 3 landed
  it.


### Added — the mandate fence inside `GitStore`'s own transactions (item 5 PR 3)

- `mycelium-wiki::mandate_fence` plus `GitStoreConfig::mandate` (**`None` by default — today's
  behaviour exactly**, so a store that has not opted in is unaffected).
- **One ref transaction, not two operations.** With a fence, the content write becomes an
  `update-ref --stdin` transaction carrying `verify <mandate-ref> <expected>` beside the content
  update. The mandate is checked *through commit* rather than before it, so a concurrent appointment
  cannot slip between the check and the write. Two separately successful CAS operations could
  interleave; one transaction cannot.
- **`--atomic` and `--force-with-lease` on every push — including content-only ones.** That
  "including" is the load-bearing word: the obvious implementation reads the mandate ref, decides
  the curator is current, then pushes — and an appointment moving in between lands a write under an
  authority revoked microseconds earlier, with nothing in the transcript showing it. Asserting the
  mandate's value *as part of the push* closes that window.
- Verified against real git: the 30-test `GitStore` integration suite passes on the new transaction,
  **including** `two_store_instances_race_the_ref_cas_not_each_other` and
  `concurrent_erase_and_batch_write_serialise_through_the_ref_cas` — the two that exercise the CAS
  property the change touches.
- Stated rather than implied: the **remote's** atomicity and the **pre-receive hook** that verifies
  the signed epoch are the other half of the fence, and are not covered by these tests.


### Added — the scoped-mandate contract (item 5 PR 2)

- `mycelium::mandate`: the appointment (holder · establishing authority · purpose · scope ·
  **enumerated** operations · authority epoch · **separate term identity** · window), the three
  lifecycle events, and `ResourceAuthority::check` — the shape the real enforcement point will carry.
- **The decisive invariant is enforceable here**: once a resource has installed epoch E2, a mandate
  authorized only under E1 is refused — *refresh, retry, reconnect and restart all give the same
  answer*, which is the point, since those are the four ways a revoked holder ordinarily gets a
  second chance.
- **`MandateSuperseded` is its own refusal and never a `Conflict`.** `Conflict` is the retry loop's
  input; classifying a stale mandate that way would hand a revoked curator to the exact loop that
  refreshes content and re-submits, and **the retry loop would launder the revocation**. Supersession
  is also reported *before* any other failing check, so the caller gets the answer that matters
  rather than one inviting a fix that will not help.
- **Epoch and term identity are separate fields on purpose.** The epoch orders authority; the term
  says *which appointment*. Collapsing them makes "the same holder, reappointed after a gap"
  indistinguishable from "the appointment never lapsed" — and the three lifecycle events, each about
  a term, become unrecordable.
- An installed epoch never goes backwards: the invariant is about what happens *after* an
  installation, so an installation that could be undone would undo it.


### Added — the authorized knowledge store and its heads (item 3 PR 3)

- `knowledge::store`: records live here; the gossip KV namespace carries **bounded signed heads
  only** (`knowledge/head/{issuer}/{stream}`, already reserved). `Head` is a pointer by
  construction — there is nowhere in it to put a statement.
- **The decision this makes true:** `advancing_a_head_does_not_remove_what_it_pointed_at`. Put
  records in KV and last-write-wins becomes *last-writer-is-right* — two issuers who disagree
  resolve to whichever had the later HLC, and the loser is gone. That is a clock race, not a truth
  procedure. With heads only, an advancing head moves a pointer and both statements survive.
- `KnowledgeStore::about(subject)` is what *"equivocation is preserved"* means operationally: a
  reader is handed **the disagreement**, not the later writer's version of it. Stably ordered, so
  two callers holding the same set cannot disagree about it.
- **Authorized means two checks**, both local: an issuer publishes only its own heads, and **a head
  only advances** — a stale copy re-delivered by anti-entropy cannot roll a stream backwards. The
  refusal carries both sequence numbers, because *"we have newer"* and *"this is a duplicate"* are
  indistinguishable without them.
- A head may point at a record we have not fetched. That is **ordinary, not an error** — refusing it
  would make discovery depend on having already discovered — and `is_resolvable` says so plainly.


### Added — typed knowledge records (item 3 PR 2)

- `mycelium::knowledge` (`tls`): the four record types — `Claim` · `Observation` · **`Assessment`**
  · `AcceptanceDecision` — and the six link kinds. Records only: no store, no resolution, no gossip.
- **Immutability is arithmetic, not a convention.** A `RecordId` is derived from the record's
  content, so *"there is no edit, only a further record"* cannot be violated by forgetting it —
  changing any field produces a different record.
- **An id names its issuer**, which is what makes the layer's sharpest rule *locally* checkable:
  **an issuer retracts only its own statements**. With a bare digest, a reader would have to fetch
  the target to know who issued it — so the rule could be skipped by any reader that had not.
  Retraction is not moderation, and this is enforced from the retracting record alone.
- **Challenging across issuers is allowed and is the point.** Only retraction is issuer-bound;
  disagreement is what the layer preserves, and two issuers contradicting each other produce two
  records that both exist.
- A record's **kind** is in its signed bytes, so an observation's signature can never authenticate
  an assessment — judging is not recording, down to the cryptography.


### Added — the federation example and guide (item 2 PR 7, completing the item)

- `cargo run --example federated_domains --features tls` walks the whole item-2 lifecycle in one
  process — signed descriptor, filtered catalog, discovery with expiry, a call bound to one export,
  per-partner budgets, failover that respects repeatability, a partition/reconnect cycle, a bounded
  key rotation, a revocation — and prints what each step decided **and why**.
- **It also prints what it did not demonstrate.** No bytes cross a network; there is no federation
  transport yet. The record's release gate — a two-mesh demonstration proving *from membership
  tables, consensus state and traces* that the meshes never merged — is **not** met by the example
  and is not claimed by it.
- Guide 17 gains the invoke-edge half, with D25's boundary stated once: *NANDA is what a domain says
  about itself, verifiable by any fetcher; federation is who may invoke what, between partners.* No
  second well-known document, no registry, no trust-registry service.


### Added — partition, reconnect, revocation and rotation (item 2 PR 6)

- **Reconnecting is not being ready.** `federation::session::PartnerLink` has three states, and the
  middle one is the point: work is **refused** while a reconnected link's discovery is still stale.
  Two states would leave a window in which the link is up and the catalogue predates the
  partition — and whatever changed while it was down (a withdrawn export, a revoked grant, a
  rotated key) is exactly what a caller would be acting on. Refused, not queued: a queue delivers
  the same stale-view calls a moment later, having also hidden the reason.
- `Down` and `Refreshing` stay distinct refusals, so a reconnect that never completes cannot
  masquerade as a healthy link that happens to be refusing.
- **Key rotation without a flag day.** `TrustBundle::rotate` accepts the retiring key for a
  **bounded** window — unbounded overlap is not a rotation, it is two keys, and the compromised one
  is still among them. Current key first, since after a rotation nearly all traffic carries it.
- **Revocation is a tombstone, not a deletion.** `is_revoked` distinguishes *"we used to trust them
  and stopped"* from *"we never heard of them"* — different facts, different operator response. It
  beats rotation (a revoked partner has no acceptable keys, mid-rotation or not) and survives a
  reconnect.
- `TrustBundle::partners` is now `Vec<PartnerTrust>` rather than `Vec<(DomainId, [u8; 32])>`;
  `TrustBundle::trusting(..)` builds the simple case. Both types are unreleased, so this is not a
  break against v2.7.0.


### Added — two-gateway operation, budgets and outcomes (item 2 PR 5)

- **Failover only for repeatable exports.** `Repeatability` is declared per export by the domain
  that offers it — a default would be wrong either way round, since one risks doing something twice
  and the other quietly disables failover for everything. An `AtMostOnce` export is **never** failed
  over: the far side may have run it, so the caller gets `DeliveryUnknown` (item 1's vocabulary —
  *a timeout is never a negative*) rather than `Failed`, which would assert something we do not know.
  **That distinction survives running out of gateways**, which is exactly when it is easiest to lose.
- **Budgets are per partner, not per gateway.** A single pool would let a busy partner starve
  everyone else — availability for one bought at the cost of the rest, with nobody deciding it. A
  partner at its limit is refused (`NoCapacity`, naming the limit) while others proceed untouched;
  refused rather than queued, because an unbounded queue is the same starvation with a longer delay.
- **No leader among gateways.** Selection is a fixed local order: no election, no shared state, no
  message exchanged to decide. A leader would be a coordination dependency that must be up and
  agreed upon for two *healthy* domains to keep talking — the thing the substrate exists not to need.
- A silent gateway's slot is released, so a timeout does not leak capacity.


### Added — the authenticated federation call and the provider adapter (item 2 PR 4)

- `federation::call::FederatedCaller` — a credential **bound to the call, not just the caller**. It
  names the export it authorises, so a credential minted for `invoice.status` cannot invoke
  `invoice.submit` from the same partner, in the same second, over the same connection. Item 7 fixed
  the confused deputy inside a domain; this is the same fix one boundary out, where the caller is by
  construction not ours.
- **`AcceptedCall` preserves the origin to the provider** — the domain *and* the principal, verbatim,
  rather than "the gateway called me". A provider that only ever sees the gateway cannot make an
  authorisation decision of its own, nor say afterwards who it acted for. It also carries the
  `policy_revision` that permitted the call, so *"under which rules"* has an answer.
- **Cross-domain expiry is wall-clock, deliberately.** Everything else in the crate pushes intervals
  onto a monotonic clock — but two domains share no monotonic origin, so a cross-domain deadline has
  to be stated on the one clock both sides can name. The cost is skew, bounded explicitly by
  `CallPolicy::skew_tolerance` rather than assumed away.
- **The verifier bounds the lifetime, not the issuer.** `CallPolicy::max_lifetime` is the receiving
  domain's own ceiling; without it a partner could mint a century-long credential and §10's *"issued
  authority lasts only to its expiry"* would be a promise the issuer makes to itself.
- Seven named refusals, kept distinct because each is a different thing for an operator to do — in
  particular **`BadSignature` and `NotPermitted` never merge**: one says someone is forging, the
  other says we chose not to grant it, and reading the first as the second sends the operator to
  edit the wrong file.


### Added — filtered catalogs and the remote resolver (item 2 PR 3)

- **Outbound: filter, then publish.** `federation::catalog::filtered_catalog` gives a partner only
  the exports its policy grants — an ungranted export is **absent, not refused**. A catalog is a
  disclosure, and disclosing that a capability exists already tells a partner something about this
  domain.
- **Inbound: attributable per-gateway observations.** `CatalogObservation` carries *who* saw it and
  *when*, in fields rather than in a comment. Two gateways disagreeing is then a fact about the
  gateways, not a contradiction to resolve; both stay visible to an operator.
- **Discovery expires rather than being extended** (record §10). `RemoteResolver` refuses an
  observation past its freshness window and says *which* failure it was — `UnknownDomain` ·
  `NotExported` · `Expired { age }`. A resolver that quietly served a stale entry would turn a
  partition into a wrong answer. Age comes from `sim_seam::mono_elapsed`, so a recorded run replays
  an expiry instead of waiting for one.
- **`RemoteCapability` is a distinct type** from a native capability. One type for both would make
  "is this ours?" a question about a string prefix.
- **Discovery is not authorisation.** The resolver answers *"was this offered, recently"*; whether
  we may invoke it is the gateway's question at the invocation edge. Merging them would let a cache
  grant something.


### Added — federation identity and policy objects (item 2 PR 2)

- `mycelium::federation`: `DomainId`, the signed `DomainDescriptor`, the revisioned `DomainPolicy`,
  the bilateral `TrustBundle`, and the **canonical bytes** a signature is taken over. Types and
  bytes only — no transport, no discovery, no `federation/` KV prefix, no wire change.
- **A length-prefixed canonical form, not canonical JSON.** Canonical JSON *can* give byte
  agreement and is a known foot-gun doing it — key order, non-ASCII escaping, surrogate pairs,
  number formatting. Here we own both ends, so the encoding has no such freedom: length-prefixed
  fields, fixed-width little-endian integers, one encoding per value.
- **Domain separation is a security property.** Each object's bytes begin with a distinct tag, so a
  policy signature can never authenticate a descriptor — D6's *"reuse the code, never the trust"*,
  applied to our own objects.
- **The trust bundle decides which key, not the descriptor.** A self-signed descriptor is not
  authorised by being internally consistent; an unknown domain is refused however well-formed its
  document is, and a descriptor whose key disagrees with the bundle's is refused rather than
  preferred.
- **Absence is denial** in `DomainPolicy::permits` — no wildcard, no inheritance, no default-allow,
  because each is a way for a grant to exist that nobody wrote down.


### Added — the enforced domain profile and the two-mesh harness (item 2 PR 1, completing it)

- **`DomainProfile::{Open, Enforced}`** (`GOSSIP_DOMAIN_PROFILE`), default `Open` so every existing
  configuration is untouched. `Enforced` turns `federated-domains.md` §9 from prose into a
  **refusal at `validate()`**, before a socket is opened: **TLS required** (admission *is* the
  domain — without a per-node CA root, "independently admitted" has no mechanism behind it) and
  **SWIM off** (its control datagrams are unauthenticated UDP; `swim.rs` signs nothing).
  Operator note: `swim_failure_detector` defaults to *on*, so enabling the profile without also
  turning SWIM off is refused **loudly**, rather than by quietly disabling a liveness mechanism
  underneath you.
- **The two-mesh harness.** Two meshes sharing no bootstrap peer are asserted never to learn each
  other — **from the peer tables and the native `cap/`/`grp/`/`sys/` namespaces**, which is what
  the record's release gate demands instead of a narrative. The test also runs those assertions
  against a **deliberately merged** pair and requires them to *fail*, so a harness that checked
  nothing could not pass.


### Added — the scoped-mandates ADR, and `mandate/` + `log/wiki/` reserved (item 5 PR 1)

- `docs/design/scoped-mandates.md` — the last of the three PR-1 ADRs Phase A's exit gate names.
- **CAS is not authorization.** The wiki's CAS defeats stale *content*; a former curator who re-reads the
  fresh content and re-submits **passes it**. So a stale mandate is `MandateSuperseded`, never `Conflict` —
  because `Conflict` is the retry loop's input, and classifying it that way would hand a revoked curator to
  the loop that refreshes and re-submits. **The retry loop would launder the revocation.**
- **The axis's one architectural disagreement, recorded with both sides.** The reviewer proposed a
  SQLite-backed daemon per wiki scope; that is a control plane for the scope. The *criterion* — the resource
  must enforce — is accepted in full and met by the store that already serialises the bytes: for `GitStore`,
  the mandate ref and the content ref move in **one** `update-ref` transaction, and every push is
  `--atomic` with `--force-with-lease` on the mandate ref **including content-only writes**, so the check is
  part of the same remote transaction rather than an earlier hook-time read. Fail closed on a non-atomic
  remote. `FsStore`'s strict profile is **out of scope** and says so rather than letting silence imply a
  guarantee.
- `mandate/{scope}` and `log/wiki/{group}/proposals` reserved in both front doors before any code writes them.


### Added — the knowledge-layer ADR, and `knowledge/` reserved (item 3 PR 1)

- `docs/design/knowledge-layer.md`: four record types (**judging is not recording**), six explicit link kinds,
  and the load-bearing decision — **heads in the gossip medium, records in an authorized store**. Put records
  in KV and last-write-wins becomes last-writer-is-right: two issuers who disagree would resolve to whichever
  had the later HLC, which is a clock race, not a truth procedure. Equivocation is preserved instead.
- `knowledge/head/{issuer}/{stream}` reserved in **both front doors** at PR 1, before any code writes it:
  `kv_ns::KNOWLEDGE_HEAD` (`mycelium-core/src/signal.rs`) and the crate-doc namespace table (`src/lib.rs`).
- The record states what it refuses (reputation scalars, inferred independence, mandatory LLM judgment,
  consensus over truth) and keeps its two gates apart — a semantic one that is a CI property, and a
  behavioural experiment that can only ever support a bounded claim.


### Added — the federated-domains ADR and the KV namespace sweep (item 2 PR 1)

- `docs/design/federated-domains.md`: what a domain is, the three trust relationships kept apart, and the
  decisions the plan left to this record. **D5 is decided: the federation call *is* A2A**, with domain-bound
  origin credentials — there is no `POST /federation/v1/call`, because the A2A edge already exists and is
  already enforced, and two invocation edges with different auth models is precisely the drift v2.4.1/v2.4.2
  were spent removing. The record also states what would reopen that decision.
- `scripts/check-kv-namespaces.sh`, in `make check` and CI: **foreign state never enters the gossip medium**
  (D7). The plan asserted this check existed; it did not, and an invariant nothing runs is a sentence.
  Verified in both directions — a planted violation in production code is caught with its citation, and the
  same literal inside a test module is correctly ignored.

### Fixed — a snapshot's bytes no longer depend on hash iteration order

- `do_snapshot` built its entry list by iterating the store and extending with the WAL tail, so the
  **file's bytes depended on papaya's iteration order** — which is not stable across processes even
  though the store's hasher is seeded. Two nodes holding identical logical state wrote
  byte-different snapshots, making any byte-level comparison (checksum, dedup, fixture diff)
  unsound. Entries are now sorted by key before encoding: the file is a function of the state it
  represents.
- Found by replaying the WAL/snapshot scenario, which is what the replay harness is for. Old
  snapshots still replay — this changes only what is written.

### Added — item 6 PR 4: the fault sweep

- **A failure at any storage effect must leave an acknowledged record recoverable** — either the
  snapshot carries it or the WAL still does; *neither* is the outcome v2.4.3 and v2.4.4 were both
  about. Swept across all five install effects plus the WAL-tail read, by rewriting one recorded
  outcome in the trace (the bundle format is text, and this is what that buys).
- **A fault is an effect that does not happen.** The write-side seams now **decide before acting**
  in replay: a recorded failure means the effect is skipped, not that an error is handed back after
  the rename already renamed. Recording keeps the opposite order — it must act first, because the
  real outcome is what gets written down.
- `fs_read` honours the kernel's outcome too. Without that a fault injected at a read was silently
  ignored, which matters most here: **v2.4.3 exists because a failed WAL-tail read was treated as an
  empty tail** and the records it could not see were truncated away. That path is now verified by
  injection rather than by a hand-built unreadable file.

### Added — item 6 PR 4: the WAL/snapshot scenario replays from a bundle, with its witness

- **The merge-removed witness**, as a `cfg(test)` toggle rather than a hand edit (the plan requires
  this: a hand edit cannot be named in a bundle, re-run by someone else, or used to show the fix
  still holds). It is the exact inverse of
  `regression_snapshot_retains_wal_record_acked_before_local_apply`.
- **The gate**: the race is recorded, written to a bundle naming that toggle, read back from disk,
  and replayed — with a companion test proving the replay is a *check* (a different run diverges).

### Changed — replay performs storage effects instead of suppressing them

- A write's bytes are its request, so replay used to skip the effect. That breaks for **a run that
  reads its own writes**: `do_snapshot` reads the WAL tail `wal_append` wrote earlier in the same
  run, and a suppressed write left the tail empty, diverging against the run's own recording. The
  kernel now decides the *outcome* while the effect really happens, which also means a recorded
  failure replays as a failure.
- **An effect's request no longer embeds absolute paths.** `fs_rename` recorded
  `/tmp/run-123/snapshot.tmp->…`, so a bundle could only replay in the directory that produced it.
  File names only — both ends still recorded.


### Added — `MeshHandle::last_signal_age`, and the clock seams that own a decision

**Additive. The next release stays a MINOR.** An earlier pass converted the peer table,
`SwimMembership`, `SignalHandlers` and `MeshHandle::last_signal` to `u64` monotonic nanoseconds.
Those are public types, so it forced a MAJOR — for a representation change with **no consumers in
the workspace**: every external use is `agent.peers()`, the method, which never changed. It has been
reverted.

- **`MeshHandle::last_signal_age(kind) -> Option<Duration>`** is new; `last_signal` keeps its
  `Option<Instant>` signature. The age is what callers computed anyway and is what the sibling
  `last_signal_persistent` returns, so the pair reads consistently — but taking the old signature
  away bought nothing.
- **What a replay must reproduce is the decision, not the representation.** Every staleness decision
  is one of three shapes, and each now has a seam, so the stored `Instant` never needs reproducing:

  | Shape | Seam |
  |---|---|
  | "how long since this" | `sim_seam::mono_elapsed(&Instant)` |
  | "how long between these two" | `sim_seam::mono_span(&Instant, &Instant)` |
  | "has this deadline passed" | `sim_seam::mono_before(&Instant, &Instant)` |

  The third matters most: suppression expiry, the sender-log trim cutoff and peer eviction compare
  **two stamps the process took at different moments**. A replay that re-took them would compare its
  own elapsed wall time rather than the recording's, so the *verdict* is what the kernel records.
- Baseline **180 sites across 43 files**. The `Instant::now()` stamp sites are admitted deliberately
  in `replay-nondeterminism-inventory.md` §2.1: nothing branches on them directly.

### Added — the monotonic clock has a non-zero origin, and `seed_sender_log` has its first test

- `sim_seam::MONO_ORIGIN_NS` (one year), so a point *before* the run began is representable on the
  `u64` clock as it is on `Instant`. Kept for any caller that subtracts a window from a fresh
  reading; the caller that first needed it (`SignalLog::seed`) is back on `Instant` and no longer
  does — recorded rather than left with a stale rationale.
- `mycelium-sim`'s `Sources` starts its monotonic clock at the same origin, so the harness does not
  disagree with the thing it models.
- `seed_sender_log` had **no test in either representation**. It has one now.

### Added — `compute_view_confidence_at`, and the branch it made reachable

- `compute_view_confidence_at(ctx, now)` injects the clock — the pattern `SwimMembership` already
  used — so the **staleness branch is reachable from a test at all**, and it is now tested in both
  directions. Added while the peer stamps were briefly `u64` (where there was no "ten minutes ago"
  to construct) and **kept when they went back to `Instant`**: the testability is the point, not the
  representation. `compute_view_confidence` is unchanged.

### Added — the monotonic-clock seam (item 6 PR 3)

- `sim_seam::mono_now_ns` / `mono_since` — **separate from the wall clock, and the separation is a
  correctness property.** Every site these replace measures an *interval*; `Instant` is monotonic, so
  a backwards NTP step cannot make one negative or enormous, and `SystemTime` gives no such
  guarantee. Reusing `wall_now_ms` would have been one function fewer and a new class of bug: a rate
  window that never expires, a backoff that fires instantly.
- `writer.rs`'s reconnect backoff and `connection.rs`'s inbound rate window and anti-entropy
  cooldown now go through it. Baseline **179 sites across 43 files**, from 186.
- Scope stated rather than implied: `tokio::time::Instant` deadlines belong to the **timer** seam
  (converting the reading without owning the sleep would leave the wait real), and the peer table's
  `Arc<papaya::HashMap<NodeId, Instant>>` is a type change across two crates and gets its own
  increment.

### Added — a test for the reconnect backoff, which had none

- Breaking `mono_since` left all 180 `mycelium-core` tests green and failed exactly one test in the
  outer crate, by a route with nothing to do with reconnecting. The backoff now has a direct test,
  verified against **both** failure directions — a backoff that never expires and one that never
  applies.

### Added — the peer writer channels, and a cost the seam was quietly imposing (item 6 PR 3)

- The four **peer-writer** sends — forwards and TCP pings, each with its respawn retry — route
  through the channel seam. Baseline **186 sites across 43 files**, from 190.
- **Streams are per destination.** A drop to peer A and a drop to peer B are different events, and
  `targets` is an `AHashSet` whose iteration order is not stable across processes — so one shared
  stream would have handed one peer's recorded verdict to another, and the failure would have looked
  like a lost frame rather than a misread trace. Forwards and pings get separate stream families
  because they are two tasks sending on one channel.

### Fixed — the seam no longer allocates on the gossip hot path

- `chan_try_send` takes its stream as `&str`, and the argument is evaluated **whether or not a
  kernel is installed** — so the `format!("gossip/shard{n}")` introduced with the channel seam cost
  a heap allocation per frame dispatch in ordinary production builds. Shard names now come from a
  static table with a cold formatted fallback above it; peer names are built once and cached beside
  the sender. A harness that makes the system slower in order to watch it has changed the thing it
  was measuring.

### Fixed — `mycelium-core --features sim` is now linted, not just tested

- `mycelium-sim` was clippy-gated but the arm that *routes* through it — every seam call site under
  `--features sim` — was only ever compiled by the test job. Added to `make check` and CI. This is
  the third instance of the family (compliance 2026-09-04, core tests 2026-07-21): a feature whose
  code is tested but never linted.

### Added — channel coverage across the substrate (item 6 PR 3)

- Eight more bounded sends route through the channel seam: the **WAL append queue** (a full queue
  skips a record — one of the three things the inventory says channel fullness decides), the
  per-handler **signal** channel, the **StateRequest** writer, the **pong**, the **audit export**
  drain, the **AE evidence journal**, and two pings.
- The evidence journal's is the one worth naming: its saturation behaviour is a *stated guarantee* —
  a full queue is refused, never silently dropped — so being able to **replay** the saturation is how
  that guarantee gets tested rather than merely asserted.
- Baseline **190 sites across 43 files**, from 199.
- Left deliberately: four sends inside `tasks.rs` macro bodies and two SSE sends in `a2a.rs`. They
  are recorded debt, not oversights.


### Added — the recovery-read and channel-readiness seams (item 6 PR 3)

- `persistence.rs`'s three recovery reads — the snapshot, the WAL, and the WAL tail the snapshot
  merges — now go through the kernel. Baseline **184 sites across 43 files**.
- **A read is checked, not supplied.** A write's bytes are its *request*, so a replay can skip the
  effect; a read's bytes are its *result*, and putting a snapshot's contents in a line-per-decision
  trace would make the trace the disk image. The bundle already carries disk images, so in replay
  the read really happens against restored state and the seam compares what came back.
- That makes it a divergence check on recovery: *the recovery read returned different bytes than the
  recording did* is the **v2.4.3** class of failure — where a read error was mapped to an empty tail
  and acknowledged records were truncated away. A harness that supplied the recorded bytes would
  have replayed straight past it.
- **The channel-readiness seam.** Whether a gossip shard was full is now a kernel decision, so a
  recorded run *replays the dropped frame* instead of hoping to provoke it again by timing.
- Its shape differs from storage, and has to. A file write in replay can be skipped — the disk is
  restored from the bundle. A channel send **cannot**: its effect is in-process and the replay is
  reproducing that process. So the kernel decides the verdict and the call site honours it: `Sent`
  performs the send, `Full` does not, and a replayed `Sent` that finds the channel full is a
  divergence rather than a quietly dropped frame.
- Per-shard streams (`gossip/shard2`), because a drop on one shard and a drop on another are
  different events and a merged trace could not say which key stopped propagating.
- **The seam owns the send** (`chan_try_send(stream, tx, msg)`), rather than wrapping a closure
  around the caller's `try_send`. Wrapping left the `try_send` in the call site, so the
  forbidden-call check could not tell routed code from unrouted — a seam you cannot enforce is a
  seam that erodes.
- With that, `try_send` joins the forbidden-call pattern. The inventory's §6 list covered clocks,
  RNG and storage but **omitted channels**, though the coverage map assigns them to the kernel.
  Baseline **199 sites across 45 files** — 15 unrouted channel sends were invisible.


### Added — the first replay seams: the HLC wall clock and the WAL writes (item 6 PR 3)

- `mycelium-core`'s `sim_seam` module, and a `sim` feature that routes the nondeterministic reads
  the coverage map assigns to the kernel through it. **Off in every shipped build**: without `sim`
  the module compiles to the calls it replaced, so the substrate pays nothing for the harness.
- The HLC's one wall-clock read — "the clean seam", and the read every write's LWW rank depends on —
  now records and replays. A replay returns the recorded value whatever the machine's clock says,
  and a read the recording never made stops the run rather than inventing one.
- Forbidden-call baseline down to **185 sites across 43 files** (from 190): `hlc.rs` has left it
  entirely, which is what routing a seam is supposed to look like.
- **The WAL's write, file sync and directory sync** are now kernel effects too. Their *order* is
  the durability property — a record is durable only once the sync returns — so dropping the sync
  is a **divergence**, caught by the trace before PR 4's storage model can simulate the loss it
  would cause.
- A replayed write does not touch the disk: the mode is checked before the `await`, so a replay
  cannot mutate state the recording already accounted for.
- **Three bugs found in the forbidden-call check from the previous entry, all by testing the
  checker:** it ignored everything after the first `#[cfg(test)]` (+25 sites), it counted comments
  (−5), and it did not follow `fs as tfs` aliases — which made `persistence.rs`, the module the
  inventory calls "the right first target" with 26 fs references, **entirely invisible** (+14).
  Baseline now **199 sites across 44 files**; it read 165 when the check first went green.
- **The snapshot install is five ordered kernel effects**, and the v2.4.4 power-loss property —
  *the directory sync must precede the WAL truncation* — is now a **test that fails when the order
  is reversed**, printing the offending trace. `snapshot_install_syncs_the_directory` says that
  property "is not observable without a filesystem adapter"; it is now.
- **The RNG seam**, on the inventory's five named streams. `ops.rs`'s nonces and its opacity
  shedding roll now draw from `nonce` and `shed` — named rather than shared, so a new draw in one
  subsystem does not move another's decisions and a scenario replay attributes a divergence to the
  code that moved. `ops.rs` has left the baseline (4 → 0).
- **Peer selection, shuffles and tick jitter** route through the `select` and `jitter` streams; the
  `sim` feature is forwarded from `mycelium` to core so the outer crate can reach the seams. A
  shuffle is **one recorded draw per step** — a single opaque call would record nothing the kernel
  could compare, so a changed shuffle would replay as identical. `tasks.rs` 9 → 3 sites.
- Gated: `make check-full` and CI build and test `-p mycelium-core --features sim`.


### Added — the static forbidden-call check (item 6 PR 3, D12)

- `scripts/check-sim-seams.sh` enforces the inventory's §6 rule: a new `SystemTime::now`,
  `Instant::now`, `fastrand::`, `tokio::time::*`, `tokio::fs`/`std::fs` or `RandomState::new` outside
  the replay seams fails the build. Without it the seams erode while PR 4–7 are built, and a replay
  quietly stops reproducing with nothing to attribute it to.
- Implemented as a baseline diff, **not** the `clippy.toml` `disallowed-methods` the record
  suggested: that lint is workspace-global and cannot be scoped to modules, so it would fire inside
  the seams and every test — which is exactly where these calls belong.
- **Measured baseline: 190 sites across 45 files** — the debt PR 3's adapters and PR 4–7 draw down,
  now visible rather than estimated.
- Runs in `make check` and CI's clippy job.


### Added — `mycelium-sim`, the deterministic replay kernel (item 6 PR 2)

- A new workspace crate implementing the trace schema PR 1 fixed
  (`docs/design/replay-nondeterminism-inventory.md` §5): the **event kernel**, the **seams**
  production reaches it through, the **choices trace**, and the **failure bundle**.
- **It detects divergence rather than re-seeding.** In replay the kernel does not re-run the
  production code's own randomness and hope it agrees — it checks each request against the recorded
  next entry and *supplies* the recorded result. A run that re-seeds reproduces a failure only while
  the code is unchanged, which is exactly when nobody needs it.
- **The gate** (`mycelium-sim/tests/divergence.rs`): record a run, replay it with one WAL record's
  bytes changed **at equal length**, and require a divergence. A harness that passes that unchanged
  is not detecting divergence. A flipped `sync` flag and a moved offset diverge too.
- Two clocks kept separate (wall time jumps, monotonic does not) and **named RNG streams** (so a new
  draw in gossip does not move consensus, and a scenario replay attributes a divergence to the code
  that actually moved).
- Storage outcomes model the **short write** as neither success nor failure, because the durability
  argument turns on it.
- The bundle carries a **witness** — the assertion *and* the `cfg(test)` toggle that must make it
  fail again. A bundle that replays but cannot fail proves only that the harness works.
- Gated from the first commit: `make check`, `make check-full` and CI all run it.


## [2.7.0] — 2026-09-16

A small **MINOR**, and a correction found the honest way — by building a consumer against
2.6.0 rather than by re-reading it. Wire **v12** (`PREV = 11`) **unchanged**; on-disk format
unchanged; a backwards-compatible rolling upgrade.

**The upgrade note:** `AeEvidence` gained a field and became `#[non_exhaustive]`, which breaks
an exhaustive struct literal — the same class as `GossipConfig`'s field additions in 2.5.0.
Reading and matching on fields is unaffected; construct it with `AeEvidence::for_decision`.

### Added

- **`AeEvidence::at_ms` — the decision's own event time**, carried from the envelope's
  `issued_at_ms` instead of being discarded. An exporter needs it, and needs it **stable**: record
  ids derive from journal position, so an exporter that re-reads after losing its cursor re-sends
  the same ids — and a body that differed, because it had stamped its own read time, is refused
  under the consumer's *same id, byte-identical content* rule. A timestamp the record does not carry
  is one the exporter must invent, and an invented one cannot be stable. Found by the exporter's own
  retry test.
- `AeEvidence` is now `#[non_exhaustive]`, so the next field addition is genuinely additive.
  Construct it with `AeEvidence::for_decision`.

### Changed

- **Upgrade note:** `AeEvidence` gained a field and became `#[non_exhaustive]`, which breaks an
  exhaustive struct literal — the same class as `GossipConfig`'s field additions in 2.5.0. Reading
  and matching on fields is unaffected; only literal construction outside the crate is.

## [2.6.0] — 2026-09-16

The **AE evidence MINOR** — the gateway now records what it enforces, and the record is one a
customer could be shown. Wire **v12** (`PREV = 11`) **unchanged**; on-disk format unchanged; a
backwards-compatible rolling upgrade. Every addition is inert for a node that attaches no action
evaluator, so an existing deployment behaves exactly as it did.

**The one upgrade note:** a node that *does* attach an evaluator should also attach an evidence
journal (`GossipAgent::with_evidence_journal`). Without one it enforces and records nothing, and
warns at attach time saying so.

### Added

#### the execution record: what the gateway actually observed

- Every permitted dispatch now produces a second journal record (`RecordKind::Execution`) beside its
  decision, carrying the same `operation_id` / `attempt_id` / principal. Before this, every permitted
  call exported as `effect: unknown` even though the gateway watched the provider answer.
- `Ok(reply)` ⇒ `completed`, downgraded to `failed` when the JSON-RPC reply carries an `error`.
  A dispatch refused before sending ⇒ `none`. **A timeout or transport error ⇒ `unknown`, never
  `failed`** — the call may have run and the provider simply failed to answer.
- Wired on the MCP `tools/call` route and both A2A paths. The mapping is one named function
  (`observed_execution`) so the timeout rule is unit-tested rather than reasoned about.
- Added: `RecordKind`, `AeEvidence::kind`, `AeEvidence::as_execution`.

#### a cursor-based reader for the evidence journal

- `read_evidence_journal_from(path, cursor, max_records, max_bytes)` with `EvidenceCursor`,
  `JournalEntry` and `JournalPage`: §6.7's outbox shape, so an exporter batches, ships and advances
  rather than re-reading the journal from the start. The cursor carries a byte offset, so resuming
  stays cheap however long the journal grows.
- Each entry carries **the content hash the chain's `AeReference` cites**, so an exporter can
  correlate what it ships with what the tamper-evident chain says about it.
- A record larger than the caller's byte bound is returned alone rather than skipped; a truncated
  tail ends the page without advancing past the unfinished record.
- This matters now because the previous entry moved evidence out of the audit chain: an exporter
  built on `AuditSink` sees only references, and must read the journal instead.

- `AeEvidence`, `DecisionKind`, `MappingKind`, `Execution`, `AE_EVIDENCE_SCHEMA` — the evidence
  document a gateway decision produces, and the schema a consumer matches on.
- `GossipAgent::set_deployed_policy_revision` / `deployed_policy_revision`.
- `PreflightRefusal::NotRecorded` (the enum is `#[non_exhaustive]`, so this is additive).
- `with_action_evaluator` warns when built without `compliance`: such a node enforces but cannot
  record, and an operator should not have to discover that from an empty evidence stream.
- CI's compliance job now builds `a2a` alongside it. Neither standard gate built the pair, so a test
  needing both would silently never have run.

- **Contracts axis item 1 PR 4b — the persisted-by-peer receipt** (`src/agent/replica_sync.rs`,
  `GossipAgent::set_with_replica_sync`, `WalHandle::sync`; record
  `docs/design/contracts-receipts.md` §1a, §2.1). PR 4a measured that the origin of a write cannot
  observe that write's propagation, so no predicate over inbound updates could ever acknowledge it.
  This closes the gap by **asking**: the origin sends each peer the operation's identity, and the
  peer answers about its own state. `set_with_replica_sync` returns a `WriteReceipt` whose
  `replica_sync.persisted_by` names peers that answered *persisted* — their store holds this exact
  HLC stamp and content **and** their WAL `fdatasync` returned `Ok`, so they hold the record across
  their own crash and restart. Everyone else is `missing`, which means **unknown, never "did not
  persist"**: unreachable, mid-restart, on a build without the handler, or now holding a newer value
  all land there, and none of them establishes absence.

  Three properties, each the reason a more obvious design was not taken. **No wire change** — the
  exchange rides the existing RPC layer, so `WIRE_VERSION` is untouched and a peer on an older build
  simply never answers. **No retained operation status** — the peer answers from live state, so there
  is no table to size or expire when a query arrives late, the same conclusion §9a reached for the
  prepared write. **No per-entry durability tracking** — records append in order to one file, so the
  new `WalHandle::sync` (an `fdatasync` that appends nothing, handled in the writer's own loop so it
  cannot race an append) establishes durability for everything already appended.

  **The verb moved up a layer, and the old one is deprecated.** `KvQuorumExt::set_with_min_acks` is
  an extension trait on `KvHandle`, which holds only `CoreCtx` — and core knows nothing about RPC,
  deliberately. "Consistency as a service, not a foundation" had put the verb one layer *below* the
  protocol it needed. It is now `#[deprecated]` pointing at `set_with_replica_sync`, and still cannot
  succeed; existing code keeps compiling. `POST /gateway/kv/quorum` asks peers too, so it works
  again, and reports `unknown_peers` alongside `acks_received` on timeout.

  Gates: `a_peer_holding_the_write_acknowledges_it` **replaces** PR 4a's
  `a_peer_holding_the_write_still_produces_no_acknowledgement` — that pin asserted the timeout as
  the contract precisely so closing the gap would have to change it in the open. The Phase B exit
  gate, `an_acknowledged_replica_still_holds_the_record_after_restart`, stops and restarts the peer
  from the same directory with nothing re-gossiped to it: it answers from its replayed WAL, so a
  mechanism answering from memory would pass the first assertion and fail this one.

### Changed

- **Contracts axis item 1 PR 4a — an acknowledgement now requires the payload's identity, and the
  verb says what it cannot do** (`src/agent/kv_quorum.rs`, `kv_quorum_ext.rs`,
  `mycelium-core/src/store.rs`; record `docs/design/contracts-receipts.md` §1a, §8). `set_with_min_acks`
  counted an ack when any distinct origin gossiped **any** update for the key at or after the write's
  HLC, so a **newer overwrite** — a payload that peer never received from us — satisfied the count.
  That overclaim (D9) is gone: an ack now requires the inbound update's `content_hash` to equal this
  write's. `QuorumObserver` gains `observe_update`, carrying the update's origin, stamp, nonce and
  content hash; the default delegates to `observe`, so implementations written before this keep
  compiling and behaving exactly as they did.

  **What implementing it found, which the plan did not know.** The verb **cannot succeed on this
  substrate and never could** — not a weaker guarantee than documented, a different one. Measured on
  two connected nodes with the peer demonstrably holding the value:
  `set_with_min_acks(1) -> Err(Timeout { acks_received: 0 })`. Three correct facts compose into it:
  a `GossipUpdate`'s `sender` is its *originating* node, preserved across every forwarding hop, so a
  peer relaying our write is attributed to **us** and the loopback filter discards it; fan-out
  **excludes the origin**, so the relayed copy never comes back to us anyway; and anti-entropy
  re-attributes delivered entries to the **receiving** node. No inbound frame says *a peer holds your
  write*. The counter could only ever be moved by a different node independently writing the same key
  — which is why the only two tests were a zero-ack case and a no-peers timeout: the success path had
  never been exercised. The rustdoc now opens with this, and
  `a_peer_holding_the_write_still_produces_no_acknowledgement` asserts the timeout **as the contract**,
  with the peer holding the value in the same test, so PR 4b flips it in the open. **PR 4b is no
  longer an enhancement to this verb; it is the only thing that can make it succeed.**

  **Upgrade note.** `min_acks >= 1` still times out as it did before, so callers see no new failure.
  The one behaviour change is that a concurrent writer of a *different* value to the same key can no
  longer produce a false `Ok`. Anything that appeared to succeed was relying on that.

### Fixed

#### AE evidence is node-local, and only a reference gossips

- **Corrects the entry below, from the same day.** That change recorded gateway decisions by sealing
  the whole decision document into the tamper-evident audit chain. The chain is an ordinary signed KV
  entry, so every node received the exact resource each call targeted, the policy's reason, and the
  constraints it checked — which `docs/design/action-envelope-ae0.md` §5 forbids, having adopted the
  rule precisely to prevent it.
- The decision document now goes to a **node-local evidence journal** (`EvidenceJournal`): durable,
  append-only, fsynced, never gossiped, returning item 1's `LocalDurability`.
- The audit chain carries an **`AeReference`** only: kind, `operation_id`/`attempt_id`, principal,
  verdict, policy revision, catalogue id, and the journal record's content hash. The chain's
  hash-linking still covers the evidence, so tampering stays detectable without dissemination.
- Three failure behaviours, tested: queue saturation ⇒ refused, never a silent drop; persistence
  failure ⇒ refused; a lost acknowledgement ⇒ `DeliveryUnknown`, never `Failed`. `EvidenceProfile`
  chooses whether they gate the effect (`Strict`) or are declared in the record (`Lenient`).
- Added: `EvidenceJournal`, `EvidenceProfile`, `JournalError`, `Appended`, `read_evidence_journal`,
  `AeReference`, `EvidenceState`, `AE_REFERENCE_SCHEMA`,
  `GossipAgent::with_evidence_journal`.

#### the AE gateway slice records what it enforces

- **Every gateway authorisation decision is now sealed into the audit chain.** The evaluator preflight
  refused and permitted and wrote nothing, so a node enforcing a declared remit produced no evidence at
  all. Both outcomes are now recorded as an `AeEvidence` document (`mycelium.ae/evidence/1`) in the
  audit record's `detail`, carrying the verdict, policy revision, checked constraints, execution and
  reviewed mapping — facts the record's own three-valued outcome cannot hold without rounding
  *not established* into *denied*. A decision that cannot be recorded now **refuses the dispatch**
  (`PreflightRefusal::NotRecorded`, JSON-RPC `-32032`).
- **`/a2a` is enforced.** Both A2A dispatch paths run the same preflight, under enforcement point
  `gateway:a2a`. Previously only `tools/call` was guarded, so a remit could be walked around by
  choosing the other edge.
- **The stale-policy check can fire.** `GossipAgent::set_deployed_policy_revision` supplies the
  revision an operator reported as deployed; the seam compares every decision against it. The
  envelope's `expected_policy_revision` was hardcoded `None`, so the check was structurally dead.

- **CI: the scale-evidence alarm could not fire, because it was queued behind the silence it
  reports** (`.github/workflows/scale-nightly.yml`). The `evidence-freshness` job added on
  2026-09-15 ran nothing at the first nightly after it shipped. The workflow carried a
  **workflow-level** `concurrency` group, which queues the entire run — so with one run already
  queued against the offline self-hosted box, run `35086785839` (2026-09-16) went `pending` with
  **zero jobs created**, the hosted alarm among them. The group now sits on the `scale` job: two
  suites still never share the one Docker daemon, but a hosted job in a later run is no longer held
  behind a self-hosted job in an earlier one, and a newly pending scale job displaces the previous
  one instead of a queue of runs expiring one at a time. The rule this generalises to: *a check must
  not sit inside the failure domain of the thing it checks.* Dispatched on the fix branch the alarm
  started and reported, for the first time: *"No successful run of the scale suites has ever been
  recorded"* — literally, across all 69 runs of the workflow since 2026-07-10, every one `cancelled`
  with an empty `runner_name`. Every scale figure in the documentation rests on operator-run
  `make test-scale`, never on CI, and should be cited that way until the box is online.

- **CI: a missing scale runner was invisible, because a job nobody runs never goes red**
  (`.github/workflows/scale-nightly.yml`). The scale suites run only on a self-hosted box labelled
  `mycelium-scale`. While that box is offline the nightly does not fail — it **queues**, indefinitely,
  and eventually expires as "cancelled". On the runs list that reads as activity; it is the absence of
  evidence rendered as a spinner. Every nightly between 2026-09-10 and 2026-09-15 produced nothing
  that way, unnoticed, while the contracts work landed. Adds a small hosted job that asks a narrower
  question — *when did this workflow last actually succeed?* — and fails when the answer is older than
  three days or "never", naming the likely cause. It cannot start the box and does not pretend to; it
  makes the silence loud. Until it is green, every scale claim in the documentation is unevidenced,
  which is what the plan has said all along (§10, item 8).

- **CI: the reason-node job dispatched through the gateway before the gateway could dispatch**
  (`.github/workflows/ci.yml`). Two `TestCallTyped` cases failed with HTTP 412
  `provider_without_caller_context` on 2026-09-15. Not a regression and not a gateway bug: under the
  default secure caller-context profile (item 7) a gateway refuses to route to a provider whose
  `sys/caller-context/{node}` marker it cannot see, which is the refusal working as designed. The job
  gated on `/health` — the HTTP server being up — and then dispatched immediately, while the marker
  is published lazily (once a peer connects, or after a 5 s grace, so the write cannot land in a
  reconnect-backoff window) and must then gossip to the other node. Every dispatch in that window is
  a 412. The gate now polls until the dispatching node can see both markers, and says so with the
  count when it cannot. Same defect class as scenario 13's readiness poll: a structural poll that
  proves a weaker property than the one the test depends on.

- **CI: scenario 13's client deadlines were shorter than the server budgets they measured**
  (`tests/integration/scenarios/13_tuple_space.sh`). The 4-node Docker suite failed twice in three
  runs on 2026-09-15 — once on a put, once on take #9 — after a green streak of at least 97
  consecutive runs reaching back to 2026-07-16, the limit of the run listing. Not a regression: the
  PR it first appeared on (#217) is purely additive, `git diff -U0` showing zero lines removed from
  any executing path. The scenario invoked operations whose own budget is up to **16 seconds**
  (`resolve_primary_blocking` waits 3 × `cap_refresh` = 6 s, then one `rpc_call` at 10 s; a take's
  is `timeout_secs + 5 s`) while allowing the client **5 seconds** for a put and **exactly 10** for
  a take. Any request frame lost to an outbound-writer reconnect backoff — routine right after
  scenarios 03/04/05 restart node-a, the whole cluster and node-c — therefore failed the run with
  curl's own impatience (exit 28, HTTP 000) instead of the gateway's answer. The single-shot
  assertions now outlive the server budget they measure; the short deadlines inside `poll_until`
  probes are left alone, because there the retry is the recovery. This is the *opposite* of the
  forbidden "widen the timeout until it goes green": below the server's budget the assertion is
  unobservable by construction, and the test can only ever report that it gave up first.
  Two diagnostics went with it: the put loop now captures the HTTP code and body like the take
  loop has since #150, rather than piping `curl -sf` into `jq` and reporting `jq exited 28`; and
  both loops truncate the body file first, because curl leaves it untouched when no response
  arrives — which is why the failing run printed take #9's result as `{"id":8,...}`, reading as a
  wrong-id bug rather than the timeout it was.

### Known issues

- **The tuple-space role record has no freshness, so `/api/tuple` can report a departed node as
  primary** (`sys/tuple/{node}/{ns}/role`, written by `spawn_metrics_writer`). The backpressure
  pheromone written by the *same* loop stamps `written_at_ms` and evaporates after 3× the cadence;
  the role record does neither, is never cleared on shutdown, and survives a restart in the
  replayed WAL. An operator reading the monitoring endpoint therefore cannot distinguish "is
  primary" from "published that it was primary at some point". This is the shape 2.5.0 closed for
  `ViewConfidence` with `staleness_known`, and it belongs to the same contracts-axis posture: a
  record must not claim more than the underlying state establishes. Found while root-causing the
  scenario-13 failures above, where it was **not** the cause — that phase passed in both failing
  runs. Queued against the axis rather than changed here, because it alters an operator-facing
  response shape.

---

## [2.5.0] — 2026-09-15

Contracts MINOR on the 2.x line — the **v3 contracts axis' first tranche**: what an acknowledgement
proves is now a typed receipt rather than a `bool`, a gateway call carries the identity of the client
that made it, and a policy evaluator can refuse an action at the gateway before it reaches a provider.
Wire **v12** (PREV 11) unchanged — a backwards-compatible rolling upgrade; on-disk format unchanged.

**Two behaviour notes an upgrader must read.**

1. **`authorized_callers` now judges the client, not the gateway node** (item 7). A deployment that
   listed a gateway's *node id* in a provider's allowlist in order to admit its HTTP/SDK clients must
   now list those clients' **principals** (`oidc:{issuer}/{subject}`, `token:{issuer}/{name}`, …).
   That node-listing was the impersonation this release closes: every gateway dispatch previously ran
   under the node's own identity, so a provider could not tell one client from another, or from the
   node. Secure profile is the default; `gateway_caller_profile = legacy` restores the old dispatch
   for a rolling-upgrade window and is a `3.0.0` removal-ledger entry.
2. **`GossipConfig` gained fields**, which is *not* additive by Rust's rules: an exhaustive struct
   literal over its fields stops compiling. The documented construction pattern —
   `GossipConfig::default()` then assignment, or `..GossipConfig::default()` — is unaffected, which is
   why this has passed unremarked through every release that added a config field. The `3.0.0` ledger
   now carries the entry that ends the series (mark the operator-constructed config structs
   `#[non_exhaustive]`). Same applies to `ConsensusResult::Committed`: it is **unchanged** here
   precisely for this reason, and the new receipt arrives on new verbs instead.

Everything else is additive: new types and verbs beside the existing ones, all `#[non_exhaustive]`
with public constructors, and the pre-existing verbs keep their signatures and their meanings.

### Added


- **Contracts axis item 1 PR 3 — the required local sync, and the prepared write**
  (`KvHandle::set_requiring_sync` / `retry_requiring_sync` / `prepare_write` / `commit_prepared`;
  record `docs/design/contracts-receipts.md` §2.1, §2.2, §9a). A write that **must** be durable: it
  forces an `fdatasync` in every `SyncMode`, so its receipt is always `LocalDurability::OnDisk` where
  the ordinary path would report `Buffered`, and the ordering is deliberately reversed —
  **persist → apply → gossip** — so a failure means *this attempt applied nothing*: no store entry,
  no subscriber, no frame. Persist-first is admissible only because the snapshot merges the WAL tail
  before truncating (D8), and the two regressions pinning that dependency are cited at the call site.
  A node with **no persistence configured is refused outright** rather than handed a receipt claiming
  nothing — prevention the caller asked for as a contract (posture rule 3(i)) — through the new
  `ReceiptError::DurabilityNotEstablished`, which separates *this node never could* from *the attempt
  failed*. **`KvHandle::prepare_write`** stamps an operation *before dispatch* and returns a small
  serialisable `PreparedWrite` (identity, key, content binding, HLC); `commit_prepared` and
  `commit_prepared_requiring_sync` reuse that stamp on every attempt, so a caller that **lost the
  acknowledgement** can retry safely — the retry is `Superseded` rather than a silent overwrite of a
  newer value, while re-issuing by `OperationId` alone mints a fresh HLC and does clobber (the two are
  contrasted in one test). **Review corrections (2026-09-15):** a failed required-sync write is
  documented as leaving the **post-restart outcome unknown**, because the WAL writes before it syncs
  and a failed sync may leave a replayable record — the first cut claimed the value could never appear
  here (`regression_an_unsynced_record_still_replays`); and §9a's deferral of retained operation
  status was argued wrongly — remembering a *local* write's stamp claims nothing about an external
  transaction — so it is rewritten around the prepared-write token, which solves the
  lost-acknowledgement case with no node-held state to size or expire.

- **Contracts axis item 1 PR 2 — identities, typed receipts, the failure vocabulary**
  (`mycelium-core/src/receipt.rs`; record `docs/design/contracts-receipts.md`). The vocabulary in
  code: `OperationId` (caller-minted, stable across retries — a *correlation* identity, never
  authority) and `AttemptId`; the four rungs as types — `LocalApplication::{Applied, Superseded}`,
  `LocalDurability::{OnDisk, Buffered, Failed, NotConfigured}`, `ReplicaSync` and
  `DestinationCommit` (vocabulary now, established by PR 4a/4b and PR 5); `ReceiptError::{Conflict,
  DeliveryUnknown, Rejected}` and `CommitError`, with **no variant meaning "nothing happened"**.
  New verbs, all additive: **`KvHandle::set_with_receipt`** returns a `WriteReceipt` naming each rung
  it established and none above; **`KvHandle::retry_with_receipt`** re-submits an operation with its
  **original HLC stamp** (D11 — a retry that ticked a fresh one could outrank and silently undo a
  newer value) and refuses same-identity-different-content with `Conflict`, writing nothing;
  **`ConsensusHandle::{cluster,group}_propose_receipt`** answer `Result<CommitReceipt, CommitError>`,
  separating *the cluster agreed* from *this node has it on disk* and reading a timeout as
  `DeliveryUnknown`. Per D24 these are **new verbs, not new fields**: `ConsensusResult::Committed`'s
  shape is unchanged, since growing a non-`#[non_exhaustive]` variant would break every exhaustive
  destructure. Core also gains `apply_and_notify_reporting` and `make_gossip_update_stamped`.
  **A gap this implementation found in its own record:** `LocalDurability` needed a fourth state.
  `OnDisk`/`Failed`/`NotConfigured` assumed every write is forced to disk or fails, true only under
  `SyncMode::Flush` or `append_sync`; under `Async`/`Os` a successful append is **`Buffered`** — it
  survives a process crash and not a power loss. Claiming `OnDisk` there would have been exactly the
  overclaim this axis exists to end. Gates: nine `receipt_tests`, including the late retry that is
  `Superseded` while the newer value survives — D11's rationale made executable. No wire change.
  **Review corrections (2026-09-15):** the receipt path now appends through a new
  `WalHandle::append_acked` and **awaits the writer's acknowledgement** — `append` is a `try_send`
  in `Async`/`Os` that returns `Ok` even when the queue is full or the writer is gone, so a receipt
  built on it claimed the bytes had reached the operating system when they may never have left the
  process; a dead writer is now `Failed` in every mode. `LocalApplication` gains **`AlreadyCurrent`**
  and **`Refused`**: an idempotent retry and a capacity refusal were both reported as `Superseded`,
  claiming a newer value had won when none had. Attempt identities are **fresh per dispatch**
  (`AttemptId::fresh`, with `*_as` verbs for callers that mint their own) — deriving them from the
  prior receipt made two retries of one receipt indistinguishable, which is what happens when a
  response is lost or replacement workers share a receipt. And the content hash is now **FNV-1a/64
  over a specified canonical encoding with golden vectors**, replacing seeded `ahash`, which is not
  an interchange format: its output may differ across versions and CPU features, so an unchanged
  operation retried on another build could have raised a false `Conflict`.

- **AE evaluator seam at the gateway** (`src/agent/action_evaluator.rs`; the code half of AE0,
  `docs/design/action-envelope-ae0.md`; plan §6.8 / D36–D38, queue §10.12.4). One hook between the
  gateway's auth layer and its dispatch: before a gateway-originated `tools/call` reaches a provider,
  an attached `ActionEvaluator` sees an `ActionEnvelope` — the verified actor (item 7), the granted
  scopes, the operation, the exact resource, a `sha256` argument digest, the reviewed catalogue
  mapping, validity — and answers `Permit` · `Deny` · `Indeterminate`. **Inert unless an evaluator is
  attached** (`GossipAgent::with_action_evaluator`), so every existing deployment is unchanged.
  Ships `ReferenceEvaluator`, a deterministic reference implementation (prohibitions win, then
  allowances; anything uncovered is *authority not established*, never a denial). The seam enforces
  what an adapter might get wrong: a permit carrying evaluation errors, a permit over an unmapped or
  ambiguous operation, a decision from an unexpected policy revision, an expired envelope, and a
  panicking evaluator all refuse. Refusals answer `-32030` (`action_denied`) or `-32031`
  (`authority_not_established`) with the policy revision and what was checked; metric
  `mycelium_ae_preflight_refusals_total{reason}`. **Strength: a route-level preflight**
  (`SelfImposedPrevention` for the routes this gateway fronts) — never enforcement at the effect,
  which only AE2's resource fence earns; anything reaching a provider without traversing this gateway
  is outside the guarantee and the evidence must say so. Gates: the AE0 §9 negative fixtures as unit
  tests, plus two live-gateway tests (a prohibited and an uncovered tool are refused before the tool
  runs; an unattached seam changes nothing). No wire change, no KV prefix; `gateway` + `tls`.
  **Review corrections (2026-09-15):** a prohibition whose own preconditions cannot be established
  now answers `Indeterminate` with the missing fact named, not a definite `Deny` — dispatch was
  refused either way, but the evidence had claimed a prohibition was *established* when it was not;
  a stale-policy refusal now carries `Indeterminate` rather than the evaluator's original verdict
  (a refusal containing `Permit` is a contradiction for the record, and the original is preserved in
  `checked`); the panic guarantee is stated honestly — `catch_unwind` contains an evaluator panic
  only in an unwinding build, and this crate's release profile is `panic = "abort"`, so the trait
  requires evaluators not to panic, and the evaluator's *other* two calls now sit behind the same
  boundary; and **`Decision` / `ActionMapping` / `ActionEnvelope` gained public constructors** —
  being `#[non_exhaustive]` with none, they were unconstructible outside the crate (E0639), so a
  foreign evaluator could never return a permit and the replaceable-evaluator premise was false. A
  new external-crate test (`tests/ae_external_adapter.rs`, CI-run) is the gate for that. The
  envelope also now carries the **security-relevant argument values** an evaluator declares it needs
  (`ActionEvaluator::security_relevant_arguments`): a digest establishes integrity but cannot answer
  *amount ≤ 500*, and only the declared names cross into the envelope or the evidence.

- **Gateway caller identity (v3 contracts axis item 7, `docs/plans/v3-contracts-axis.md` §6.4 / D27).**
  Every gateway-originated dispatch — `POST /mcp` `tools/call`, `POST /a2a`, `/gateway/rpc/call`,
  `/gateway/scatter`, `/gateway/overlay/emit_reliable`, `/gateway/llm/{call,stream}` — used to run under
  the **node's** identity, so a provider's `authorized_callers` saw the gateway node and never the client
  (a confused deputy; the 2026-09-05 `/mcp` fix put the route behind auth but did not tell the provider who
  called). Now the auth layer constructs a `GatewayCaller` on every dispatch — the **originating principal**
  (`oidc:{subject}` · `token:#{index}` · `token:legacy` · `anonymous`, never the credential), the **gateway
  node** acting on its behalf (`via`, checked against the frame's signature-verified sender), and the
  **authority granted for the request** (the credential's scopes ∩ the route's scope, never `*`) — and the
  node **attests it** (Ed25519 over `principal ‖ via ‖ scopes ‖ issued_at ‖ sha256(payload)` under `tls`).
  It rides inside the RPC payload after the nonce; **wire v12 unchanged**. `RpcRequest::payload()` strips
  it, so existing provider loops see exactly the application bytes; providers read it via
  `GossipAgent::request_principal` / `gateway_caller`, and (`compliance`) authorise with the new
  **`request_authorized`** — a gateway client is judged by its *principal* (listing the gateway node admits
  nothing), a direct node call by node id / roles as before. `mycelium-guardrails` `check_caller` /
  `guarded_rpc_serve` and SkillRunner use it; denial seals now name the client principal and the `via` node.
  New: `McpHandle::register_mcp_tool_with_principal`; `/gateway/rpc/serve/{kind}` events carry an optional
  `caller` object (`principal`, `via`, `scopes`, `attested`); `/a2a` resolves an *optional* bearer (valid ⇒
  its principal, none ⇒ `anonymous`, unrecognised ⇒ 401 — never downgraded); `sys/caller-context/{node}`
  marker (`b"1"`) written at start by every node. **Config:** `gateway_caller_profile` (`secure` default ·
  `legacy`; env `GOSSIP_GATEWAY_CALLER_PROFILE`). **Secure profile refuses** — never silently runs as the
  node — a provider without the marker (JSON-RPC `-32021` / HTTP `412` `provider_without_caller_context`,
  naming the provider), a dispatch site without a context (`-32020`), and, at the provider, a forged /
  unsigned / mis-attributed context (`CallerError`; MCP `-32022`). The `legacy` profile is node-as-caller
  for a rolling-upgrade window (gateways upgraded before providers), logged at `warn!`, and a §6.6
  removal-ledger entry. Gates: `gateway_caller_tests` in `src/agent/http.rs` (the four negative cases + the
  `authorized_callers` gate under `tls`+`compliance`, `/a2a`) and `agent::gateway_caller::tests`.
  Metric: `mycelium_gateway_caller_refusals_total{reason}`. Operator page: `docs/operations/rbac.md` §7.
  **Behaviour note:** a deployment that listed the *gateway node* in a provider's `authorized_callers` to
  admit HTTP clients must now list the clients' principals (that node-listing was the impersonation).
  **Hardened after an external review of the first cut (2026-09-13/14), four findings, all closed with
  regression tests:** (1) a raw `/gateway/signal/emit` of RPC-shaped bytes reached a provider *as the node*
  — a secure-profile gateway node now publishes marker `"2"` and wraps its own `rpc_call`s in a signed
  `node:{self}` envelope, so a bare frame from it is `CallerError::Missing`, never its action
  (`raw_gateway_signal_cannot_pass_as_the_node`); (2) `token:#0` named the same identity on every gateway —
  principals are now **issuer-qualified** (`token:{issuer}/{name|#i|legacy}`, `oidc:{idp}/{subject}`;
  `gateway_identity_issuer`, default the node id; new `gateway_named_tokens` for stable names)
  (`token_identities_are_qualified_by_the_issuing_gateway`); (3) the built-in LLM provider stripped an
  envelope without verifying it — **`ServiceHandle::rpc_rx` now verifies at the receive boundary** and
  answers refusals itself, covering every companion loop, and the LLM / MCP / explain receivers verify
  directly (`llm_provider_refuses_an_unverified_context`); (4) malformed or oversized frames read as
  "no envelope" and were admitted as the node — framing is a three-way `Frame::{Unframed, Framed,
  Malformed}`, malformed refuses, and the producer refuses an over-bound envelope (`-32023` / HTTP 413)
  instead of truncating (`malformed_frames_are_refused_never_treated_as_the_node`,
  `producer_refuses_an_oversized_envelope`).
- **Contracts axis AE0 — the action-envelope ADR** (`docs/design/action-envelope-ae0.md`; plan §6.8, D36–D38).
  The contract for runtime authorisation and evidence, written beside item 1's ADR so no second identity scheme
  exists: the action envelope (item 1's `operation_id`/`attempt_id`, item 7's verified actor and `via`, operation,
  exact resource, argument digest, mandate identity/epoch, `policy.revision` = the policy artifact's `sha256`,
  validity, correlation, catalogue mapping), assembled only by the enforcement point; the `ActionEvaluator`
  contract — permit · deny · indeterminate with checked constraints, reason, policy revision and evaluation errors;
  indeterminate is never permit and the secure profile refuses on it; deterministic; replaceable; **Cedar
  in-process as the one real adapter** (D37 adopted); five separate evidence records (requested · decided ·
  attempted · completed/failed/unknown · outcome observed) sealed into the audit chain and exported through the
  `AuditSink`, never gossip; the evidence consumer's catalogue identity carried, not re-minted; deployment reports
  and `coverage.complete: false` for every route the enforcement point does not see; three strength profiles
  (preflight = *SelfImposedPrevention* at one route, resource = *HardPrevention*, adapter = the adapter's own
  contract); a pinned standards matrix; eleven negative fixtures. **No code, no wire change.** The evaluator seam
  lands on item 7; AE-T T2–T4 in the private companion. **Review corrections (2026-09-14):** evidence lives in a
  node-local journal and only a safe reference record enters the gossiped audit chain (the chain gossips whole
  records, so "never gossip" needed that split); the journal's `append -> LocalSync` receipt, not `AuditSink`, is the
  strict profile's durability barrier, with saturation / persistence-failure / lost-ack tests named; and a denial
  implies "no effect" only through an explicit blocked attestation for the attempt, never from silence.
- **Contracts axis item 1 PR 1 — the contracts-and-receipts ADR, the regression floor, golden on-disk
  fixtures** (`docs/design/contracts-receipts.md`; plan `docs/plans/v3-contracts-axis.md` §3, D8/D11/D24).
  The record states what an acknowledgement proves today at every site (`kv().set` = queued for gossip;
  `set_with_min_acks` = propagation with a `>=` overclaim; `Committed { persisted }` = fsynced *or*
  never promised) and fixes the contract the next PRs implement: four receipts kept separate — local
  application · local sync · replica sync · destination commit — each with its own visibility /
  durability / pre-durability-effects / post-failure truth; caller-minted `operation_id` + `attempt_id`
  (the identities the AE0 envelope binds); `Conflict` on same-id-different-content; `DeliveryUnknown` on
  timeout; apply→persist kept as the invariant with persist-first admissible only under the WAL-tail
  merge; the reconciliation with `exactly-once-effect.md`'s declined extraction. **No public type
  changes.** Code: the regression floor — `floor_observe_counts_any_update_at_or_after_write_ts`
  (`src/agent/kv_quorum.rs`) and `floor_committed_persisted_is_true_when_persistence_unconfigured`
  (`src/lib_tests.rs`) pin today's semantics so PR 2 / PR 4a change them in the open — and the **V2
  golden fixtures**: `tests/fixtures/persistence/fixint-v1/` (real `wal.bin` + `snapshot.bin` from the
  format unchanged since v1.0.0) replayed in CI by `golden_fixture_replays_every_released_on_disk_format`
  (`mycelium-core`); a future format adds a directory, never edits one. Docs: concepts vocabulary
  (receipt, `operation_id`/`attempt_id`, `DeliveryUnknown`), the philosophy's **Property 8** and litmus
  tests 4–5, the one compatibility rule in `building-on-mycelium.md`, the CLAUDE.md ack invariant.
  **Review corrections (2026-09-14):** `Failed` means *durability not established*, never *absent* (the WAL
  record is written before it is synced) — the `Committed { persisted: false }` rustdoc that said "not in this
  node's WAL" is corrected; tuple-space `complete` is the pipeline's receipt, never a destination commit; and
  PR 2 delivers `local_durability` through a new receipt-returning propose API rather than a new field on
  `ConsensusResult::Committed`, which would break exhaustive destructures under Rust's rules.

### Changed
- **Threat model revision 2** (`docs/threat-model.md`; v3 contracts axis item 8, plan §6.5). §5 adds the boundaries the
  axis introduces — **D** a foreign principal across a domain edge (item 2), **E** an authenticated-but-abusive client
  (items 7 and 2), **F** evidence confidentiality and the hash-as-credential (item 3), **G** a compromised former
  mandate holder and a forged epoch (item 5) — each with gains, mitigations by item, and the operator's residual, and
  the shared rule that authority is recomputed, never inherited. §6 fixes what identity, evidence and replay artefacts
  may carry: verified claims and scoped attestations, never credentials; replay bundles redacted at the recording seam;
  a named protected-reproduction-artefact class. §1–4 unchanged. Items 2, 3 and 5 cite it from their PR 1 ADRs.
- **Membership-governor cooldown is an explicit, bounded parameter; view staleness says when it is unknown**
  (v3 contracts axis item 4's standalone honesty fix, plan §10.12.5 / private WP5). `GossipConfig::membership_cooldown_secs:
  Option<u64>` (env `GOSSIP_MEMBERSHIP_COOLDOWN_SECS`): unset keeps the historical 3 × `health_check_interval_secs`,
  set bounds oscillation independently of the ping cadence, never below 1 s; read once at governor start — live
  timing intents do not alter it (decided in this change; a restart applies a new value and the doc says so).
  `ViewConfidence` gains **`staleness_known: bool`** (also on `/stats` and `GET /gateway/fleet`): `false` when no
  peer was heard inside the window, so `max_staleness_ms: 0` is read as *unknown*, not *perfectly fresh* — an
  isolated node no longer reports the healthiest view in the fleet. Gates: `cooldown_is_an_explicit_bounded_parameter`,
  `view_confidence_staleness_is_unknown_with_no_peers_heard`.
  **`ViewConfidence` gains no public field:** `staleness_known()` is a derived accessor
  (`peers_heard > 0`) and a hand-written `Serialize` puts the key in the JSON, so existing literals and
  exhaustive destructures still compile — an external review showed that adding the field broke a
  consumer that built the struct, and the type is not `#[non_exhaustive]`.
  **`GossipConfig` does gain a field** (`membership_cooldown_secs`), and that is *not* additive by
  Rust's rules: the struct is publicly constructible and not `#[non_exhaustive]`, so an exhaustive
  literal over its fields stops compiling. The documented construction pattern — `GossipConfig::default()`
  then assignment, or `..GossipConfig::default()` — is unaffected, which is why the break has gone
  unremarked through every release that added a config field (item 7's three in this same section
  included). It is recorded honestly here rather than called additive, and the `3.0.0` ledger now carries
  the entry that ends the series: mark `GossipConfig` and the other operator-constructed config structs
  `#[non_exhaustive]`, after which field additions are genuinely additive.
- **Replay item 6 PR 1 — the nondeterminism inventory, coverage map and trace schema**
  (`docs/design/replay-nondeterminism-inventory.md`; plan §4, D12–D14). Every production site whose behaviour
  depends on something the process did not decide, counted on `main` and assigned an owner: the `mycelium-sim`
  kernel's seams — wall and monotonic clocks kept separate (the HLC's single `wall_now_ms` and consensus's
  `causal_now_ms` lease reads both injected, D13), five named RNG streams (`nonce`, `shed`, `jitter`, `select`,
  `govern`), timers, `select!` readiness, channel fullness as a schedulable fault, storage with the
  volatile/durable/directory/process-kill/power-loss distinctions, recorded and redacted external inputs; papaya
  CAS retries owned by Loom, decoders by fuzz, real timing by the Docker suites; the one unseeded shared hasher
  (`framing.rs` `shard_hasher`) and the order-sensitive `AHashMap` consumers named. The sleeps whose duration is a
  correctness assumption are listed with their witnesses (the 1 s convergence wait after a lock commit first). The
  choices trace (`seq · node · kind · stream/seam · value`) and the minimum failure bundle sufficient for **exact
  reproduction with divergence detection** are fixed for PR 2; the static forbidden-call check lands with the
  adapters in PR 3. No code.

### Security
- **rustls 0.23.40 → 0.23.45, rustls-webpki 0.103.13 → 0.103.15 — RUSTSEC-2026-0285**
  ("TLS 1.3 handshake messages incorrectly accepted across encryption level boundaries"; fixed in
  0.23.45). Lockfile bump, no manifest change and no API change; it reaches the substrate through
  the `tls` feature (gossip mTLS, gateway TLS) and every `reqwest`/`hyper` TLS path. **The `v2.4.4`
  tag ships the vulnerable version** — the fix lands with the next tag, as the wasmtime
  RUSTSEC-2026-0222 note records for `v2.3.0`. Found by the `cargo audit` CI job, which had begun
  failing on every open branch.

---

## [2.4.4] — 2026-09-12

Durability PATCH on the 2.4 line: the snapshot rename is fsynced at the directory before the WAL is truncated (the Phase-0 item of the v3.0 contracts axis). Wire **v12** (PREV 11) unchanged; on-disk format unchanged; no public-API change. Also: `/consensus/{*slot}` captures the path tail (additive); `mycelium-reason` 0.6.1 / 0.6.2 on its own line; `set_with_min_acks` documented honestly. Cut for the NovusLens consumer's re-pin (their durable canon rides the snapshot/WAL path).

### Changed

- **`GET /consensus/{*slot}` captures the path tail.** Every slot the substrate mints is hierarchical
  (`lock/{name}`, `consistent/{key}`, `leader/{group}`), but the route was the one-segment `/consensus/{slot}`,
  so the natural URL the runbooks gave (`/consensus/lock/{name}`) returned 404 and only the percent-encoded
  slot reached the handler. The route is now a tail capture; both forms work and name the same slot. Additive
  (no previously-working URL changes meaning); scope `consensus:read` unchanged; the `required_scope` key follows
  the pattern. Gate: `regression_consensus_slot_route_accepts_hierarchical_slots`. This is the plan's *identifiers in paths* rule
  (`docs/plans/v3-contracts-axis.md` §9, rev 1.7).

### Fixed

- **`mycelium-reason` 0.6.2 — the OpenAI-compatible façade no longer fabricates a token split.**
  `/gateway/reason/v1/chat/completions` reported `prompt_tokens: 0, completion_tokens: 0` beside a real
  `total_tokens` (a client summing the split got `0 ≠ total`). The mesh RPC returns one total and the split
  is genuinely unknown, so the two keys are now **omitted** (an absent field is "unknown"; a `0` is a false
  claim) and the extension block carries `mycelium.usage.split_known: false`, in the JSON body and the SSE
  stop chunk alike. Companion-only; a 0.x shape change assessed on its merits (the false zero was worse than
  the absent key). Gate: `regression_unknown_usage_split_is_omitted_not_zero`. Surfaced by the
  resource-accounting proposal folded into the contracts-axis plan as rev 1.6.

- **`mycelium-reason` 0.6.1 — the router reserves atomically with ranking.** 0.6.0's local reservations
  (the PAIR import) ranked from a *snapshot* of the in-flight map and reserved the chosen provider later, so
  truly simultaneous callers could all snapshot before any had reserved and herd onto one provider anyway —
  the mechanism damped a staggered herd, not a simultaneous one (seen as a CI flake in
  `reservations_spread_concurrent_calls_across_equal_providers`, 2026-09-06). `pick_and_reserve` now ranks
  and increments under one lock, per attempt, with failover excluding tried providers; the selection rule is
  one pure function (`score_and_pick`) shared by the observing `candidates()` and the acting path, with a
  unit gate proving alternation. Item 4's reserve-before-act rule, applied early. Companion line only.

- **`set_with_min_acks` documented honestly** (rustdoc, both SDK READMEs and docstrings, guides 04 and
  error-handling): an ack is any distinct peer's update for the key at or after the write's timestamp —
  propagation (or supersession), **not** receipt of this payload and **not** persistence. The code is unchanged;
  the exact-identity, persisted-by-peer receipt is the v3.0 contracts axis, item 1 (PR 4a/4b).
- **Snapshot install is now crash-durable: the rename is fsynced at the directory before the WAL is
  truncated.** A file `sync_data` covers the snapshot's bytes, not the directory entry, so a power loss or
  kernel panic after the WAL truncation could leave the *old* `snapshot.bin` beside an *empty* fsynced
  `wal.bin` — every acknowledged record since the previous snapshot lost. Process kills never showed it (the
  page cache writes the rename out), which is why the suite could not. The Phase-0 item from the v3.0
  contracts axis; storage assumptions (directory fsync honoured; the macOS `fsync` caveat) documented in
  `deployment.md § Persistence modes`.

---

## [2.4.3] — 2026-09-05

A **durability PATCH** on the 2.4 line, cut the same day as v2.4.2 to close a data-loss path one step
past the race v2.4.2 fixed. Wire **v12** (`PREV = 11`) unchanged; on-disk format unchanged; no `mycelium`
public-API change. Companions already on their own tags: `mycelium-py` **0.2.4**
(`mycelium-py-v0.2.4`), `mycelium-ts` **0.1.1** (`mycelium-ts-v0.1.1`) — their entries below shipped
between the two tags.

### Fixed

- **Snapshot aborts when the WAL tail is unreadable.** The v2.4.2 WAL-tail merge read `wal.bin` back with
  `unwrap_or_default()`, so a transient read error during a snapshot would have produced a snapshot
  *without* those records and then truncated them — a data-loss path one step past the race it fixed.
  `do_snapshot` now returns the read error (absent file = empty tail); nothing is installed or truncated.
  Flagged as a "target for investigation" by the 2026-09-05 deterministic-replay design review, confirmed
  and fixed the same day; gate `regression_snapshot_aborts_when_wal_tail_is_unreadable`.

### Added

- **The language SDKs can authenticate to a token-protected gateway.** `mycelium-py` **0.2.4** —
  every handle (`MyceliumAgent`, `Wiki`, `TupleSpace`, `Blackboard`, `PromptSkillClient`,
  `ReasonClient`, `A2aClient`) takes `token=`; `mycelium-ts` **0.1.1** — every client class takes a
  trailing `{ token }`. Both fall back to `MYCELIUM_GATEWAY_TOKEN`; no token → no header (open
  gateways unchanged). The bearer rides the pooled clients *and* the dedicated SSE / stream clients.
  Before this, neither SDK could send `Authorization` at all, so a node with `gateway_auth_token` set
  — which every operations page recommends beyond loopback — was unreachable from Python and
  TypeScript (found by doc-coverage run 16, 2026-09-05). Gated without a node:
  `mycelium-py/tests/test_gateway_token.py` (stub server records the header, incl. the SSE path) and
  `mycelium-ts/tests/auth.test.ts` (fetch recorder). **CI now runs `jest` for the TS SDK** (the suite
  was type-checked only; the live-node tests skip, the auth tests do not) — closing the TS-CI
  coverage note from the same review.
- **The SDKs surface `persisted`.** `consistent_set` / `cross_group_propose` (py) and
  `consistentSet` / `crossGroupPropose` (ts) now return a `CommitResult { persisted }` — the
  v2.4.2 local-durability flag the gateway already emitted and the SDKs dropped (`None`/`null` from a
  pre-v2.4.2 node). Both previously returned nothing, so this is additive. Gated without a node
  (`test_commit_result.py`, `commit_result.test.ts`).

---

## [2.4.2] — 2026-09-05

A **security + durability PATCH** on the 2.4 line, cut from a single external code review
(2026-09-05) whose five findings were each reproduced before being fixed. Wire **v12** (`PREV = 11`)
unchanged; on-disk persistence format unchanged — a rolling upgrade holds. **One API note:**
`ConsensusResult::Committed` gains a `persisted: bool` field. Every in-tree matcher uses `{ .. }`,
but a downstream `match` that destructures the variant exhaustively (`Committed { slot, value,
ballot }` without `..`) must add `..` to compile. Companions on their own lines:
`langgraph-checkpoint-mycelium` **0.1.1**; `mycelium-reason` 0.6.0 / `mycelium-py` 0.2.3 unchanged.

### Security

- **`POST /mcp`, `GET /signals/{kind}`, `GET /consensus/{slot}` answered without the gateway
  bearer** — with `gateway_auth_token` set, an unauthenticated caller could invoke **any tool in the
  cluster with the node's own identity** via `tools/call` (provider-side `authorized_callers` sees
  the node, not the HTTP caller), stream live mesh signals, and read committed slot values (lock
  holders). Present in every tagged release with the HTTP gateway; the RBAC page and the wiki listed
  only `/health|/ready|/stats|/metrics` as public. Exposure requires the HTTP listener bound beyond
  loopback (default off, default `127.0.0.1`). Fixed: the three routes now carry the same
  bearer-then-scope layer as `/gateway/*` (external review 2026-09-05, finding 4). **Upgrade note
  for scoped-token deployments:** grant `mcp:invoke` (MCP clients), `mesh:read` (`/signals`),
  `consensus:read` (`/consensus/{slot}`); a legacy `gateway_auth_token` covers all three. Open
  deployments (no token model) are unchanged. `GET /bulk/{id}` stays public by design — a
  capability URL whose 64-bit per-call nonce is the credential, fetched peer-to-peer.

### Fixed

- **`langgraph-checkpoint-mycelium` 0.1.1 — `alist` no longer blocks the event loop.** The
  async checkpoint listing ran the *sync* row-selection driver (`_list_rows`: the `kv/keys` scan
  and one `kv` GET per candidate row on `httpx.Client`), so a large history or a slow gateway
  stalled every other task on the loop; only the payload half was awaited. Row selection is now
  factored into a pure window/filter core with sync and async drivers (`_alist_rows` on
  `httpx.AsyncClient`), gated for parity and for "never touches the sync client" by
  `tests/test_alist_async.py` — which needs no running node (external review 2026-09-05,
  finding 5). Companion package on its own line; no Rust change.
- **Persistence: three P1 durability defects** (external review 2026-09-05, each reproduced by a
  probe before the fix; regression gates in `mycelium-core/src/persistence.rs::durability_tests`).
  Wire unchanged; on-disk format unchanged (`snapshot_hlc` is still written, now informational).
  1. **A snapshot could erase an acknowledged, fsynced write.** The WAL writer acked an append and
     ran its threshold snapshot in the same poll, while `kv_set_async` (and the gossip receive path)
     applied the write to the store only *after* the ack — the store scan lacked the key and the
     WAL record was then truncated. Fixed twice over: every write site now **applies to the store
     before it hands the record to the WAL** (`ops.rs`, `connection.rs`, `mailbox.rs`, gateway
     `kv_write`), and `do_snapshot` **merges the on-disk WAL tail into the snapshot under the store's
     own LWW rule** before truncating, so the persistence layer no longer depends on caller ordering.
  2. **Replay treated HLC timestamps as WAL positions.** `replay` skipped every WAL record with
     `timestamp <= snapshot_hlc`, dropping a delayed remote update (older HLC, accepted after the
     snapshot) even for a key the snapshot lacked. Every record is now replayed; `apply_and_notify`'s
     LWW resolves per key.
  3. **Success reported without durable storage.** `append` (Flush) / `append_sync` /
     `trigger_snapshot` returned `Ok(())` when the writer task was gone (`unwrap_or(Ok(()))`); now
     `BrokenPipe`. `append_sync` documented an unconditional `fdatasync` but only synced in `Flush`
     mode; it now forces the sync in every `SyncMode` (consensus committed slots + leases).
     Consensus discarded the append result: **`ConsensusResult::Committed` gains `persisted: bool`**
     (`false` = committed cluster-wide and applied locally, but not on this node's stable storage;
     logged at `error`), and the gateway's propose / `overlay/consistent/set` responses carry
     `"persisted"` alongside `"ok"`. Additive: all in-tree matchers use `{ .. }`.

---

## [2.4.1] — 2026-09-04

A **security PATCH** on the 2.4 line. Wire **v12** (`PREV = 11`) unchanged; no public-API change in `mycelium` — a rolling upgrade holds. Companions on their own lines: `mycelium-reason` **0.6.0** (tag `mycelium-reason-v0.6.0`), `mycelium-py` **0.2.3**.

### Security

- **Routes merged via `with_http_routes` answered without the gateway bearer** — every companion
  `/gateway/…` surface (reason, wiki, tuple-space, blackboard) was open even with
  `gateway_auth_token` set, in every tagged release since companion gateway routes existed
  (v2.0.0 →). Fixed: a prefix-guarded auth layer on merged routers + companion **scope families**
  under `compliance`. **Upgrade note for scoped-token deployments:** companion routes now need
  `llm:*` / `wiki:*` / `board:*` / `tuple:*` (or `*`) — see `docs/operations/rbac.md`. Details
  below (Fixed) and in the calibration ledger.
- **wasmtime 46.0.2 → 46.0.3 — RUSTSEC-2026-0269** (trailing-slash sandbox escape; the v2.4.0
  tag ships the vulnerable version). Whole 15-crate wasmtime family moved together.
- **h2 0.4.14 → 0.4.16 — RUSTSEC-2026-0258** (unbounded empty DATA frames).
- **`chacha20` 0.10.0 (yanked) → 0.10.2** — a yanked crate in the lock; `cargo audit` only warns
  on yanks, so it had passed CI (Run 60 finding).


### Added

- **`mycelium-reason` 0.6.0 — three imports from the NVIDIA PAIR comparison (2026-09-04).**
  A same-day comparative read of NVIDIA's Personal AI Router (a per-node inference placer over
  Ollama/LM Studio, Apache-2.0, 3 Sept 2026) against wedge ① found one lesson to take, one
  adoption path to add, and one convention to fix; the position (PAIR is the GPU plane,
  Mycelium the agent plane, stackable) is recorded in `docs/plans/mycelium-reason.md`.
  1. **Local in-flight reservations in `InferenceRouter`** — the pheromone is the provider's
     self-report and lags dispatch by a gossip hop; between our send and its update the only
     evidence a provider is busier is *what we sent it*. Each open call now counts as a
     node-local reservation weighted into the rank (`RouterConfig::reservation_weight`,
     default 0.1; `0.0` restores the old order). Without it N concurrent callers on one node
     read the same stale fill and, via the deterministic id tiebreak, all chose the same
     provider — the thundering herd PAIR documents its reservations against. Never gossiped,
     never a pheromone (lock-order **row 36**). Gate:
     `reservations_spread_concurrent_calls_across_equal_providers` (fails pre-fix). Trace
     `route` events now record `score` (fill + reservations) where they recorded `fill`.
  2. **The OpenAI-compatible façade** — `POST /gateway/reason/v1/chat/completions` +
     `GET /gateway/reason/v1/models`: any OpenAI-speaking client becomes a mesh client by
     changing its base URL (and using the gateway bearer as its API key). `model` is the
     `llm/{model}` capability; same router, reservations and failover as `/route`;
     `stream: true` honoured as a one-chunk SSE stream; OpenAI's error envelope with the
     statuses clients map. The mapping is documented honestly (last user message → `input`;
     `system`/`history` context keys; template-bound `max_tokens`/`temperature`; unknown
     prompt/completion split). `reason_router_with(…, RouterConfig)` tunes the shared router.
     Python CI exercises it from an ordinary HTTP client.
  3. **The `llm-meta` attribute vocabulary + the Ollama collector** — `llm_meta::{CTX_WINDOW,
     FAMILY, ENGINE, WARM, VRAM_USED_MB, VRAM_FREE_MB, TOKENS_PER_SEC, PARAM_SIZE, QUANT}` with
     types and sources fixed so a constraint written on one node matches an ad written on
     another; `ModelProfile::new/with/set`; **`ModelReg::refresh_meta`** re-advertises the
     dynamic attributes, observing the old ad's retraction before publishing the new one
     (the retract-vs-advertise LWW ordering is otherwise a tokio scheduling detail — it held
     in 60 measured flips, so this is explicitness, not a fix). Feature `ollama`: `OllamaProbe` reads `/api/ps` (warm,
     `vram_used_mb`) and `/api/show` (family, ctx window, param size, quant);
     `spawn_meta_refresher` keeps a served model's ad current. Example `ollama_serve`: one
     binary that serves a local Ollama model into the mesh with a live ad and the façade.
     Example **`openai_serve`** (the stacking path): Mycelium over *any* OpenAI-compatible engine
     — PAIR, LM Studio, vLLM, a cloud API — with a static ad; runs deterministically against the
     repo's mock engine (verified two-node, both façades).
     Gates: the collector against a fake daemon; five warm/cold flips each visible within 3 s.

### Fixed

- **Routes merged via `with_http_routes` bypassed the gateway auth boundary** (core, since
  the first companion gateway routes). The auth `route_layer` wrapped only the library's
  nested `/gateway` router; a merged router's `/gateway/reason/route`, `/gateway/wiki/ingest`,
  `/gateway/tuple/put`, … answered **without a bearer** while `/gateway/kv` demanded one — and
  the companions' docs claimed coverage. Found while adding the façade (2026-09-04). Fix: a
  prefix-guarded layer on merged routers — `/gateway/…` paths get the same bearer-then-scope
  check (an unmapped `/gateway/` route is deny-by-default `admin` under `compliance`), paths
  outside stay public (`/.well-known/agent.json`, `/a2a`). Gates in core
  (`test_merged_app_routes_under_gateway_prefix_require_auth`) and in `mycelium-reason`
  (`reason_routes_require_the_gateway_bearer`), both failing pre-fix. Calibration-ledger entry
  (Security scored 8 at Run 59 while this existed). **Scope families for companion routes
  (`compliance`, follow-up the same week):** `required_scope` now maps the merged companion
  paths — `mycelium-reason` to the `llm` family (`/route` and `/v1/chat/completions` ⇒
  `llm:invoke`; trace/blob-GET/`/v1/models` ⇒ `llm:read`; blob PUT ⇒ `llm:write`), and new
  `wiki:*`, `board:*`, `tuple:*` families for the wiki, blackboard, and tuple-space routes.
  Exact paths only: an unlisted companion path stays deny-by-default `admin`. *Behaviour note
  for scoped-token deployments:* companion routes that were (wrongly) open now need these
  family scopes or `*`. Gate: `test_scoped_tokens_on_merged_companion_routes`; runbook
  `docs/operations/rbac.md`.
- **`mycelium-py` connection-reuse gate: the parked-takes test was timing-based** — it sampled
  the stub's connection count at a fixed 1 s and read 88/120 on a hosted runner (CI red on a
  docs-only commit, 2026-09-04). Now polls the count to its plateau inside a 5 s park window
  before any take returns; the pre-fix pool still plateaus at exactly 100.
- **`mycelium-py` 0.2.0: persistent pooled HTTP client** — the bridge opened a fresh TCP
  connection per gateway call, exhausting macOS ephemeral ports at Group-scale write rates
  (~16k rapid KV calls; found by a downstream test session). All request/response call sites now
  share loop-aware persistent keep-alive clients (`mycelium/_pool.py`); SSE streams stay
  dedicated. New: `MyceliumAgent.close()/aclose()` + context-manager support, `aclose()` on the
  companion clients. Gate: `tests/test_connection_reuse.py` (fails on pre-fix code).
- **`mycelium-py` 0.2.1: pooling fixes from the 2026-09-02 360° review** — three sites the 0.2.0
  conversion missed or got wrong: `TupleSpace.take`/`take_by_key` (the worker hot loop — one
  connection per claimed item) and `MyceliumAgent.set_with_min_acks` now ride the pool with
  per-borrow timeouts; `ClientPool` eviction now checks *loop liveness* instead of evicting every
  non-current loop's client (two threads each running a live loop no longer degrade each other to
  fresh clients per borrow) and all pool-map access is lock-guarded. The connection-reuse gate now
  also runs in CI and covers all three — one targeted test per fix, each proven to fail on the
  pre-fix code (the two-thread test alongside them is a concurrency smoke, not a gate).

- **`mycelium-py` 0.2.2: one client-lifecycle pattern** — `PromptSkillClient`, `ReasonClient`,
  and `A2aClient` migrate onto `ClientPool` (prompt-skill/reason handles previously built an
  eager `AsyncClient` in `__init__`, loop-bound on first use — a second `asyncio.run()` against
  the same handle failed; regression-gated). `ClientPool` grows the SDK-wide `DEFAULT_TIMEOUT`
  (one literal instead of three) and the `PoolOwner` mixin (one `aclose()` definition for
  Wiki/TupleSpace/Blackboard). A2A's SSE `stream()` stays dedicated, per the streaming policy.

- **`mycelium-py` 0.2.3: no connection cap on pooled long-polls** — the second 360° pass
  (2026-09-03) caught a regression of the first's fix: pooling `take()` put parked workers under
  httpx's default `max_connections=100`, so a fleet with >100 tasks parked on one handle had its
  101st take queued behind the pool (worst case `httpx.PoolTimeout`) instead of parked at the
  server; the per-call clients never capped concurrency and now neither does the pool
  (`Limits(max_connections=None, max_keepalive_connections=None)`; gate: 120 concurrent parked
  takes must all hold a connection — exactly 100 did before). Also `emit_reliable` now borrows
  with `timeout_secs + 5.0` like every other server-parked call (pre-existing: a `timeout_secs`
  at or above the client timeout raised `ReadTimeout` instead of returning `"timeout"`).

### Changed

- **`GitStore`: one ref-CAS retry driver** — the 32-attempt/backoff/`Conflict` retry skeleton
  that `write_with`, `write_pages`, and `remove_page` each hand-rolled now lives once
  (`with_ref_cas`), so the contention policy (already retuned once, P6.4) has a single home.
  No behavior change.

### Fixed

- **`FsStore`: erase-vs-write serialization** — a `remove_page` racing a concurrent
  `write_page`/apply in the same process could leave a persistent torn page (a recreated manifest
  surviving the erasure with its section objects deleted). All FsStore mutators now serialize
  through a flat leaf mutex (`FsStore::mutate`, lock-order row 35); per-object CAS remains the
  cross-process backstop and erasure stays idempotent. Tripwire:
  `concurrent_erase_and_write_never_leave_a_torn_page`.
- **`mycelium-wiki`: the erase verb** — `WikiStore::remove_page(page, label)` (a default trait
  method that **fails closed**; implementors: `FsStore` deletes the object bytes strictly,
  `GitStore` commits a **redaction at tip** — history retained by design, per the git-as-truth
  envelope) and the curator-authorized `Wiki::erase_page` (curator-local; deliberately not a mesh
  RPC or gateway route). Completes the store-as-truth right-to-erasure story: erase in the record
  via the same single-writer path as every write, then the projection step
  (`GitMirror` delete + `rebuild()`) per `docs/operations/data-erasure.md`. Found as a gap in the
  2026-08-16 Novus-i2 (org-twin) applicability assessment.

## [2.4.0] — 2026-08-16

Wire **v12** (PREV 11) — unchanged since v2.0.0; a fully backwards-compatible rolling upgrade
(rolling-upgrade + prev-wire gates green). The **wiki-substrate MINOR**: everything here is
additive, dominated by the council-wiki substrate arc — the git-as-truth `GitStore` with its
six-phase hardening (two recorded measurements), the `GitMirror` projection sink, the claim-check
bulk-ingest stack with an HTTP edge + SDK verbs, and the pluggable `PageFormat` codec. Design
records: `docs/design/wiki-git-store.md` · `docs/design/transparency-council-substrate.md` ·
`docs/plans/council-substrate-hardening.md`.

**Security note:** this is the **first tagged release carrying the wasmtime RUSTSEC-2026-0222
fix** (46.0.2) — the v2.3.0 tag was cut from a lineage that predated the 2026-07-16 bump and
ships wasmtime 45.0.3 with that low-severity (3.8) advisory open; upgrade rather than build the
v2.3.0 tag with the `wasm` feature in hardened environments.

### Added
- **`mycelium-wiki` `GitStore` — the git-as-truth `WikiStore`** (feature `git-store`, zero added
  dependencies): pages are real markdown files in a git checkout, every write a commit behind an
  atomic `update-ref` branch-head CAS (plumbing against a private temporary index — the caller's
  staging is never touched; commits carry only the written path, so the scoped-commit discipline is
  the mechanism, not a rule). CAS tokens are content hashes that never appear in the document,
  preserving per-section CAS independence inside the one-file-per-page layout; bodies round-trip
  byte-exactly; reads are at HEAD, never the working tree. Built **only for deployments inside the
  E1–E4 eligibility envelope** of `docs/design/wiki-git-store.md` — Phase 1 of the
  Transparency-Platform council-wiki substrate (`docs/design/transparency-council-substrate.md`).
  Gates: `tests/git_store.rs` (the FsStore contract suite mirrored, 20 tests, incl. a two-instance
  ref-CAS race) + `tests/git_store_curator.rs` (a curator draining proposals into scoped, prefixed
  git commits). **Phase-2 wiring:** `GitStoreConfig::for_group` — the group-per-council convention
  made executable (one repo, one store per group, `councils/{group}` scope; gate: two curators, two
  councils, one repo, concurrent applies, no cross-scope commits). **Phase-6 hardening (all
  M-side items):** batch commits (`write_pages` — one commit per meeting batch, whole-batch-atomic
  gate refusal, the validator runs once per batch with the file list); the corpus-scale read plane
  (persistent `cat-file --batch` child — measured 330 ms for list+query over 600 pages);
  pull-on-promote/push-per-round failover (`refresh`/`publish` default trait methods; a curator
  that cannot refresh never serves; worktree-free **subtree-splice** publish that needs no merge
  base; divergence tripwire); jittered exponential backoff + the measured ten-council contention
  run (5.5/3.0 batches/s shared/deployed, zero spurious failures — the gate surfaced and fixed
  four real defects incl. a cross-instance temp-index collision); the pluggable **`PageFormat`**
  codec (a deployment.s own entity format plugs in — proven end-to-end with a custom codec);
  `submit_batch_with_timeout` + the batch=one-meeting sizing contract; blocking-pool offload for
  ingest/publish. **Phase-3 write gate:**
  `GitStoreConfig::validate_cmd` — a deployment command (e.g. the council-wiki Node validator) run
  pre-commit over the candidate file; nonzero exit refuses with findings
  (`WikiError::gate_refusal`, carried inside `Io` — no new enum variant), worktree restored, no
  commit; the curator **drops** refused proposals (never retries — the queue can't wedge) and
  counts them via `Wiki::gate_refusals()`. **Phase-4 bulk ingest:** the claim-check path —
  `IngestBatch`/`BatchSource` (`FsBatchSource` ref impl; S3 = deployment impl), pure deterministic
  `apply_batch` (byte-identical git trees vs a serial writer, idempotent resubmit), and the
  membership-gated `wiki.{group}.ingest` RPC + `Wiki::submit_batch` — the reference rides the RPC,
  the payload never rides the mesh or KV. **Phase-5 work distribution:** assembly-only — tuple-space
  council leases × idempotent ingest = **exactly-once effect** across two companions (gate: worker
  dies after submit before ack; redelivered lease re-submits in full; zero duplicate
  commits/leaves).
- **`mycelium-wiki` change sinks — git as a projection of the store** (feature `git-mirror`): a
  `ChangeSink` on the `CuratorBrain` is notified after each applied drain round (best-effort, never
  load-bearing); the shipped `GitMirror` renders touched pages as pure markdown (no CAS tokens) into
  a git worktree — **one commit per round** with proposal provenance — and optionally pushes to an
  operator remote, `EgressPolicy`-gated **fail-closed** with a post-push `ls-remote` divergence
  tripwire (`push_divergences()`). `rebuild()` regenerates the whole mirror from the store (also the
  erasure procedure's second step). Zero new dependencies (the `git` CLI). Deliberately **not** a
  `GitStore` backing store — a branch ref is a global sequencer and git history forfeits erasability;
  the rejected as-truth variant is retained behind an eligibility envelope in
  `docs/design/wiki-git-store.md`. Gates: `mycelium-wiki/tests/git_mirror.rs` (5 tests) + the CI Wiki
  job's `git-mirror` steps. Runbooks: `operations/companions.md` § git mirror ·
  `operations/data-erasure.md` (projection retention) · cookbook recipe.

### Security
- **wasmtime 45 → 46 (RUSTSEC-2026-0222,** "Stores can mix up type indices between engines", low)
  in `mycelium-wasm-host` — the 45.x line received no patched release, so this is a major bump
  (lock: 46.0.2; source-compatible, zero code changes). The 46 tree raises the crate's MSRV
  `rust-version` 1.88 → **1.94** (CI pins 1.96.0; `examples/coop` inherits the floor only under its
  opt-in `wasm` feature).

## [2.3.0] — 2026-07-24

Wire **v12** (PREV 11) — unchanged; a fully backwards-compatible rolling upgrade. This is the
**SOC 2 audit-gap release**: the five gaps a pentest / SOC 2 control walkthrough surfaces in an
adopter's audit are closed, with an adopter-facing shared-responsibility matrix as the spine
(`docs/plans/soc2-audit-gap-closure.md`, `docs/operations/shared-responsibility-matrix.md`). Pure
library — no daemon, no control plane; your deployment is the audited system. Additive public API
throughout, so a minor bump. Also the release-1 (R1) step of the identity Phase-3 rollout:
`require_identity_proofs` ships **default-off** — enable it only after the whole fleet runs this
release.

### Added

- **Native gateway TLS (WS-A):** `GossipConfig::gateway_tls` (`GatewayTlsConfig`) — server-side
  HTTPS on the gateway port (reuse the node cert or supply a hostname cert) so bearer tokens/JWTs
  aren't cleartext. Plaintext + front-with-proxy remains the default.
- **Audit export (WS-C):** `AuditSink` trait + `GossipAgent::with_audit_sink` — every sealed record
  mirrored to your SIEM/WORM off the write path; the in-cluster chain stays authoritative.
- **Audit retention (WS-D):** signed `AuditCheckpoint` (`sys/audit-checkpoint/`) +
  `audit_checkpoint` / `audit_prune_to_checkpoint` — export then prune old records while the rest
  still verifies (verify-from-checkpoint).
- **Compromise remediation (WS-B):** `rotate_identity_on_compromise` + operator route
  `POST /gateway/identity/revoke` (scope `identity:write`).
- **`sys/identity` authentication (WS-E):** CA-cert anchor harvest + `identity_anchor_conflicts`
  tripwire, signed `sys/identity-proof/` (reject an overwrite not chained to a trusted key), and
  the `require_identity_proofs` flag (reject unsigned; `GOSSIP_REQUIRE_IDENTITY_PROOFS`). Closes the
  forged-consensus-quorum vector.
- **GDPR erasure (WS-F):** `SubjectKeyRegistry` crypto-shred helper (`mycelium::SubjectKeyRegistry`,
  `tls`) — per-subject DEK; erase = destroy the key.

### Changed

- `make check` now clippies the `compliance` feature (compliance-gated code was previously
  un-linted locally); three CI gates added (compliance suite, consensus-free embed, core clippy).
- New direct deps `ring` (mycelium-core, `tls`), `hyper-util` + `tower-service` (`gateway`) — all
  already present transitively, so no new compiled crate.

## [2.2.0] — 2026-07-16

Wire **v12** (PREV 11) — unchanged from v2.1.0; a fully backwards-compatible rolling upgrade
(rolling-upgrade + prev-wire gates green). This release is dominated by a **five-pass adversarial
self-audit** (`docs/analysis/ratings.md`, Runs 50–58): ~40 correctness fixes across the consensus,
gossip, membership, persistence, gateway, and companion surfaces, plus a structural input-fuzz gate.
The minor bump reflects a small amount of additive public API and one behaviour change (`/ready`).

### Fixed

**Consensus & convergence (audit passes 1–2).**
- **Cross-group quorum split-brain on even N** — `cross_group_quorum` had no `N/2+1` floor, so two
  disjoint quorums could each commit. Now floored; gate `regression_even_n_quorum_intersects`.
- **`elect_leader` / overlay-elect split-brain** — returned the local node id on `Committed` without
  re-reading the LWW-converged slot; two racing proposers both "won". Now converge-then-reread.
- **Acceptor equivocation** — a voter could vote for a *second* value at the same ballot; two
  proposers could both commit different values at one ballot. Fixed (`may_cast_vote`); gate
  `regression_voter_never_accepts_two_values_at_one_ballot`.
- **Vote double-count** — `cross_propose` counted a re-delivered vote twice toward quorum. Per-voter
  `seen` set.
- **Vote impersonation** — signed consensus verified the signer but not that the `voter`/`proposer`
  named *inside* the message was that signer; one key could forge N votes. Now bound
  (`signer_authorized`). **Key revocation is now applied on the consensus verify path** too.
- **Lease clock-domain** — lease liveness compared a reader's wall clock against the writer's HLC
  physical (two domains); skewed nodes could both hold a `distributed_lock`. Now `causal_now_ms` at
  all lease-read sites; the fencing token (commit HLC) remains the correctness guard.

**Anti-entropy, HLC & store.**
- **Value-blind anti-entropy digest** — the digest folded `hash(key) ^ timestamp` only, so two nodes
  holding the same `(key, HLC)` with *different bytes* were certified converged → permanent silent
  divergence. Now folds the value hash; gates `regression_anti_entropy_digest_reflects_value` +
  incremental-matches-recompute.
- **HLC monotonicity** — `tick()` was not strictly monotonic at logical saturation, and a poisoned
  `observe(u64::MAX)` under `max_drift_ms=0` could wrap the clock to 0 via the `pack` left-shift. Both
  fixed (carry-into-physical; `pack`/carry saturate at `PHYS_MAX`).
- **Store live-entry cap** — the `max_store_entries` gate counted tombstones and dropped overwrites,
  freezing a stale value that anti-entropy re-dropped every round (permanent divergence). Now an exact
  inline live count + overwrite exemption.
- **`grp_generation` published before its index**; **`append()` same-ms cross-node key collision**;
  **`KvHandle` log-consumer `offset + 1` overflow** — all fixed (`Acquire`/`Release`, node-salted keys,
  `saturating_add`).

**Membership, connection & persistence.**
- **SWIM self-incarnation overflow/wrap** — a self-`Suspect`/`Dead` rumour at `u64::MAX` (an
  unauthenticated UDP datagram) panicked the listener (overflow-checks) or wrapped to 0 (release),
  getting a live node evicted cluster-wide. Now `saturating_add`.
- **Self-peering** — a spoofed/reflected Ping carrying a node's own id made it peer with itself. Guarded.
- **Writer reap/evict orphaned a re-claimed live writer** (leaked task + double connection). Now
  compute-with-recheck + atomic remove-and-signal.
- **Snapshot dropped tombstones** → deleted keys resurrected across a restart (a stale peer's ancient
  value won LWW against an absent entry). Snapshot now retains in-window tombstones.

**Gateway, rate, opacity & diagnostics.**
- **Unauthenticated node-abort inputs** — `gw_kv_quorum`'s `Duration::from_secs_f64(negative)` and
  `parse_hex32`'s non-char-boundary slice both panicked (a node abort under the release `panic=abort`).
  Now validated → `400`.
- **JWT `aud`/`iss` bypass** — tokens *omitting* the claim passed (jsonwebtoken validates only when
  present); an audience-confusion path to the `*` grant. Now `set_required_spec_claims`.
- **Rate-aggregate overflow** (a forged `sys/rate/` value) — panic or a rate-limit *bypass* on wrap.
  Saturating.
- **Unclamped peer `fill_ratio`** → a `~584M-year` consensus retry sleep. Clamped to `[0,1]` at decode.
- **Signal reorder buffer was inert** — `flush_expired` force-drained the whole buffer, so
  `signal_ordered_delivery` delivered out of order and dropped reordered signals. Now honours
  `max_hold`/`max_depth`.
- **Boundary push-path ignored LWW** (transient wrong Group admission); **`gw_signal_emit` widened an
  unknown scope to cluster-wide**; **`overlay_group_propose` double-counted self** in the quorum;
  **`gw_overlay_log_subscribe` `hlc + 1` overflow** and a **group-subscribe idle-disconnect task+claim
  leak**; **`commit_conflict_slots` unbounded/ungated**; **`opaque_node_pct` false storm**;
  **`system_stats().store_entries` read a stale counter**; **live `TimingIntent` skipped `validate()`**
  (a fleet-wide health-loop stall) — all fixed + (where unit-testable) gated.

**Companions & examples.**
- **Blackboard startup-lag split-brain + single-shot sync** — ported the tuple-space promotion guard
  (`seen_primary` + orphan grace) and the join-time backfill retry it never received.
- **`mycelium-reason` trace replay** — the pass-1 `append`-key salt (`log/{stream}/{hlc}/{node}`) broke
  reason's own `log/`-key parser, silently dropping every trace event (CI-red until caught). Fixed +
  `trace_record_replay_round_trips` gate.

### Added

- **Input-fuzz gate (no panic on untrusted input)** — a suite of proptests that run under
  overflow-checks in `cargo test`, so unchecked arithmetic on a gossiped/config value fails the build:
  `store::fuzz_apply_observe_tick_never_panics`, `config::fuzz_validate_never_panics`,
  `capability::fuzz_is_fresh_never_panics`, `rate::fuzz_reconcile_throttle_never_panics`,
  `hlc::observe_then_tick_never_wraps`, `swim_membership::fuzz_apply_never_panics_on_arbitrary_update`,
  plus the nightly cargo-fuzz `frame_apply` decode→process target.
- **Identity-authentication — Phase 1a** (`tls::ed25519_key_from_cert_der`): extract a peer's
  CA-authenticated Ed25519 key from its validated cert (the anchor for the phased fix of the
  `sys/identity` poisoning gap, designed in `docs/design/identity-authentication.md`). Zero new
  dependency.
- **`Blackboard::is_primary()` / `is_secondary()`** — public role introspection (tuple-space parity).

### Changed

- **`/ready` now reflects startup completion, not soft-state advertisement.** A node that advertises no
  capability (a pure KV/signal node) is ready once `start()` completes, instead of returning `503`
  forever — it was previously undeployable behind a Kubernetes readiness gate. Capability discovery
  gossips independently and no longer gates readiness.

## [2.1.0] — 2026-07-15

Wire **v12** (PREV 11) — unchanged from v2.0.0; a fully backwards-compatible rolling upgrade.

### Added
- **`LockService` — the distributed-lock service** (`agent.consensus().locks()`): the ergonomic
  layer over `distributed_lock` with **blocking acquire** (`lock(name, ttl, wait)` waits out
  contention instead of failing immediately) and a **scoped critical section**
  (`with_lock(...)`, release guaranteed on every exit path). Ships with a runnable
  `examples/distributed_lock.rs` (three nodes contend + fence a resource), a when-to-use table
  (lock vs work-queue vs leader-election vs consistent_set), and the leased-lock/fencing-token
  discipline in guide ch. 04. Gates: `lock_blocks_then_acquires_when_freed`,
  `with_lock_releases_after_section`, `lock_times_out_while_held`.
- **Lock fencing token is now the commit HLC, not the ballot** (found by the new example): the
  ballot regresses under gossip lag (a later holder could get a *lower* token, wrongly fencing a
  legitimate write), so `LockGuard::token` is now the winning commit's HLC — **monotonic across
  successive holders**. Gate: `fencing_token_is_monotonic_across_acquisitions`. The gateway
  returns it as a decimal **string** (the HLC exceeds JS safe-integer range; the TS SDK already
  expected a string, the Python SDK parses it to `int`).
- **`GossipAgent::connect_peer` / `disconnect_peer`** — pin (and actively warm) a direct
  forwarding route to an RPC-heavy peer. The forwarding-target set deliberately de-pins
  non-active peers (seed scalability), which silently degrades Individual-scoped
  request-response RPC to flood-relay latency; a pin survives every target rebuild, and the
  call also spawns the writer + sends a Ping so the connection is established *ahead* of the
  first RPC rather than on its deadline. The tuple-space pins both directions (secondary
  warm-keeper + primary heartbeat). First half of the integration-S13 CI flake (#150, #155).
- **Docker cluster suites are CI-gated** (`cluster-suites.yml`): `make test` (13 integration
  scenarios, 4-node) + `make test-overlay` (S11–S13, 3-node) run on substrate PRs
  (path-filtered), merges to main, nightly, and on demand — no retries by design. Wiring this
  gate is what surfaced both #150 root causes. Harness hardening alongside: scenario ERR trap
  (a red scenario names its dying line), S13 take-loop HTTP-code instrumentation, node-log
  dump on runner failure, and a Phase-0 data-plane readiness barrier (#156). A self-hosted
  nightly for the 100-node scale suites is staged separately (#157).
- **Examples & docs, substantially reworked** — a single faceted **capability matrix** as the
  examples front door (every example fingerprinted by stack layer + facet — level · surface · LLM ·
  audit · metrics — each linking to its run-doc); the artifact library made **watchable** with two new
  browser showcases (`provisioning_viz` — a capability self-provisions then heals onto a standby with
  no coordinator; `catalog_viz` — the origin dies + its library is deleted, yet a late node installs
  from a verified peer cache); a `## Loads` banner on every runtime-loading demo declaring **what it
  installs** (content · type · source); a **UI-example contract** standardising every browser demo
  (gateway+metrics, an Ops Console two-way link, a "what you're seeing" concepts box); and
  `docs/philosophy.html` ported to a GitHub-readable **`philosophy.md`** as the single canonical source.

### Fixed
- **`distributed_lock` is now a correct mutually-exclusive, releasable lock** (#164). Two
  execution-confirmed bugs: (A) acquire returned on its *local optimistic* consensus commit
  without confirming the LWW-converged holder, so two racers both got a guard (no mutual
  exclusion — reproduced `winners == 2`); (B) release tombstoned the plain key `lock/{name}`
  while the authoritative lock lives at `consensus/committed/lock/{name}`, so release was a
  no-op and a lock, once taken, was **permanently unreleasable**. Fixes (the #151 converged-
  holder discipline): the value is `{holder}:{nonce}` with a real consensus **lease** derived
  from `ttl` (the old `expires_ms` field was never enforced); acquire confirms the converged
  value before returning a guard (losers get `Superseded`); release clears the authoritative
  slot + lease, guarded so a stale guard (lease lapsed / another acquire won / same node
  re-acquired) can never clear the live holder's claim. The HTTP gateway lock
  (`POST /gateway/overlay/lock/acquire`) had the same three bugs and gets the same fix. Gates:
  `distributed_lock_grants_single_holder_under_race`,
  `distributed_lock_release_frees_for_reacquire`,
  `distributed_lock_stale_release_does_not_clobber` (all verified failing pre-fix). Coarse-
  grained by design: a consensus round per acquire, ~1 s to let the commit converge.
- **Self-targeted Individual signals no longer flood the cluster**: a frame whose target is
  the local node (a self-emit like mailbox deliver-to-self, or a relayed frame arriving at
  its destination) terminated locally but still entered the forward path — no route to self
  → cluster-wide flood until seen-set/TTL killed it, plus a misleading topology-pressure
  warn naming the node itself and spurious `individual_flood_fallbacks` counts. The gossip
  shard now terminates Individual frames addressed to itself (admission already delivered
  them; this is routing, not scope admission). Found diagnosing #161's node logs. Gate:
  `self_targeted_signal_does_not_flood` (verified failing pre-fix).
- **All tuple-space pipeline ops wait for capability discovery** (`mycelium-tuple-space`):
  `put`/`put_keyed`/`take_by_key`/`complete_keyed`/`ack` failed `NoProvider` *instantly*
  when issued before the primary's advertisement propagated, while `take`/`complete` waited
  (#154 fixed only the read side) — analysis Run 41's API-design finding. All pipeline ops
  now resolve via the bounded blocking path (`BackpressureMode::Raise` means "don't block on
  a *saturated* primary", not "race capability gossip"); `depth` deliberately stays
  fail-fast — it is the discovery probe monitors poll. Gate:
  `client_ops_wait_for_discovery_under_default_config` (verified failing pre-fix).
- **HTTP listener sets `SO_REUSEADDR`**: a fast process restart on a fixed port could hit
  `AddrInUse` from lingering TIME_WAIT tuples and panic the node at `agent.start()` — the
  gossip listener always set it; the HTTP bind did not. Timing-dependent, so fast hardware
  never saw it; a CPU-starved CI runner did (scenario 03's restart killed node-a and 11
  downstream scenarios — named directly by the gate's node-log dump).
- **Tuple-space late-joining secondary now backfills** (`mycelium-tuple-space`): live
  replication only ships records put while a secondary is present, so a secondary joining an
  established (or promoted) primary held a *partial* mirror — a succession chain (A dies → B
  promotes → C joins → B dies) silently lost the pre-join backlog while redundancy *looked*
  restored. A joining secondary now drives the paginated `wal_replay` RPC (WAL primary → WAL
  pages; transient primary → new *state chunks*: live items as `Put` records with id-cursor
  pagination); the mirror's idempotent apply dedupes overlap with concurrent replication.
  Gate: `succession_chain_late_secondary_joins_promoted_primary`, which also executes the
  full ring-driven succession topology (late C pins promoted B, client ops through C,
  C's own second-generation promotion).
- **Tuple-space spurious promotion on startup lag** (`mycelium-tuple-space`): the promotion
  watch treated *never-saw-a-primary* as *primary-evaporated*, so on a CPU-starved host a
  freshly-started secondary promoted on cap-propagation lag — and, never demoting, held a
  permanent split-brain (takes 408 off the impostor's empty mirror while puts landed on the
  real primary; the hosted-CI integration-S13 signature). "Evaporated" now requires prior
  sight; never-seen promotes only after a 10-interval orphan grace (bounded availability).
  Canary-verified gates: `secondary_startup_lag_is_not_evaporation`,
  `never_seen_primary_promotes_after_orphan_grace` (#150, #158).

### Changed
- **Operations docs pass** (same DX lens, 2026-07-10): the topology-pressure warn /
  `connect_peer` / `individual_flood_fallbacks` — this week's new operator surface — gained
  runbook coverage (`tuning.md` §RPC-heavy pairs + `observability.md` counter docs with the
  remedy link); the restructure's config-table append was merged into tuning's canonical
  quick-reference (env-var precedence note + 10 missing fields) instead of duplicating it;
  `/stats` docs now list all tripwire + liveness counters.
- **Docs restructure for third-party DX** (front-page audit, 2026-07-10): `README.md` cut from
  **1,604 → 192 lines** — it is now a true front page (hero, 30-second hello, demos, build/run,
  a layers-at-a-glance table, pointers) and the ~1,100 lines of subsystem reference moved to
  their owning pages as clearly-marked "Reference —" sections (guide ch. 00/01/02/03/04/05/13,
  cookbook, `operations/tuning.md` — which gains the performance baselines + `GossipConfig`
  reference). One home per fact; nothing deleted. Examples fixes alongside: five orphan
  examples indexed (`conway`, `invoke_skill`, `semantic_coordination`, + the two paper
  runners), `conway-gpu/` gains a README, `coop/` gains Objective/How-to-run + the missing
  `reheal_deploy` (M+) block and honest counts (**14 demos: 12 CI + 2 manual** — was
  "eleven"/"12"), the guide's duplicate example table now defers to the canonical
  `examples/README.md` index, and a long-dead `#durability-contract` link in ch. 12 was
  found and repointed.
- **Internal: spawn-task context structs + the `kv.rs` fossil split** (analysis Run 42's
  Conceptual Integrity warts, validated then fixed). `run_gossip_shard` (20 positional
  params), `run_health_monitor` (24), and `run_gc_task` (13) now take
  `GossipShardContext`/`HealthMonitorContext`/`GcContext` — the codebase's own
  `ListenerContext` idiom, applied consistently. Field-name initialization eliminates the
  silent same-typed positional-swap class (`dropped_frames`/`individual_flood_fallbacks` and
  `backoff`/`idle_timeout` were adjacent identical types), and the next field addition is one
  struct line instead of a multi-file positional thread. `src/agent/kv.rs` — which contained
  zero KV methods (its name was a fossil from the v2 M3 core migration) — is split by concern
  into `topology.rs` (peers, connect_peer/disconnect_peer, groups, drop counts) and
  `introspect.rs` (identity, hot tunables, govern_timing, readiness, system_stats, fleet
  views). Pure mechanical moves: no public API, behavior, wire, or lock changes.
- **`InferenceRouter` is now robust to dead nodes** (`mycelium-reason`): routing candidates
  are filtered to live SWIM members (`GossipAgent::peers()`, plus self), so a departed node
  is dropped an order of magnitude faster than the ~90s capability-freshness window; and a
  new `RouterConfig::failover_timeout` (default 8s) caps non-final attempts, so a candidate
  that died inside the failure-detection window costs ~8s to fail over past, not the full
  30s inference budget (the last/lone candidate still gets `call_timeout`). Surfaced by the
  deploy/reheal flagship, which consequently drops its node-id rigging — the surviving node
  can hold either id. Canary: `liveness_filter_drops_a_non_peer_cap`.
- **Run-39 floor fixes (test architecture + observability).** The core's bind-verified,
  process-unique loopback port allocator (`test_util::alloc_port`, confined below the OS ephemeral
  floor) is now exposed under a new **test-only `test-util` cargo feature** and adopted by every
  companion's real-agent integration tests (`mycelium::test_util::alloc_port()` replaces the
  per-crate `free_port` bind-`:0`-read-drop helper) — retiring the `AddrInUse` TOCTOU flake class
  at the source instead of only at the CI retry tier. And `mycelium-reason`/`mycelium-guardrails`
  gained `metrics`-facade counters (route attempts/failovers/no-provider/exhausted; guardrail
  denials-sealed/admits) so inference failovers and Tier-C denials are visible on `/metrics`
  (no-op without a recorder). The `test-util` feature carries no runtime deps and never enters a
  production build.

### Added
- **Example-doc standard + index ([`examples/README.md`](examples/README.md)).** The example READMEs
  had drifted into three names for the same section and re-typed setup in each file; there was no
  index. Now: a front-door index of every example, one **shared-setup** section (Rust / Ollama /
  Python / Docker), and a **doc template** (`Objective` · `How to run` · `What it demonstrates` ·
  `Dev notes`; single-example + suite variants sharing one per-example block). The five drifted
  READMEs (`chat`, `fluid_pipeline`, `a2a_langchain`, `community`, `langgraph`) were normalized to it,
  duplicated setup replaced by a link, and each given verified concept + mechanism links; the root
  README's ~160-line embedded demo walkthroughs (which duplicated `examples/`) collapsed to a
  pointer table (1742 → ~1605 lines). `coop/` is the reference suite shape.
- **Reference Kubernetes deployment ([`deploy/kubernetes/`](deploy/kubernetes/)).** The multi-host
  cluster `deployment.md` describes, now shipped as ready-to-apply manifests: a seed StatefulSet +
  headless Service, a horizontally-scalable worker StatefulSet bootstrapping to the seed's stable
  pod DNS (namespace-correct via downward-API env expansion), `/ready`+`/health` probes, `/metrics`
  scrape annotations, and a management dashboard. `kubectl apply -k deploy/kubernetes`, then
  `kubectl scale statefulset mycelium-worker --replicas=N`. This is the structural escape from the
  single-host Docker-bridge iptables ceiling (`scale-tests.md`): on a multi-node cluster the pods
  spread across hosts, so the O(N²) chain never forms on any one host. Cloud-agnostic (same
  manifests on kind / EKS / GKE / AKS). Validated offline (`kubectl kustomize` → 7 well-formed
  resources); not applied in CI — see [`deploy/kubernetes/README.md`](deploy/kubernetes/README.md).
- **Reference Terraform for the cluster ([`deploy/terraform/`](deploy/terraform/)).** Closes the
  cluster-provisioning gap the manifests assume: `aws/` stands up **EKS + ECR** (via the
  `terraform-aws-modules` VPC/EKS modules), `gcp/` stands up a regional **GKE cluster + Artifact
  Registry**. Full path becomes `terraform apply` → push image → `kubectl apply -k`. Reference
  scaffolding, not hardened product IaC (single node group, public endpoint, local state).
  Authored against AWS/Google provider `~> 5.0` + EKS module `~> 20.0`; **not machine-validated**
  (no `terraform` binary in the authoring env) and **not applied** (needs cloud creds) — run
  `terraform validate && terraform plan` before apply. See [`deploy/terraform/README.md`](deploy/terraform/README.md).
- **Operator docs: metrics reference + audit/transparency tail.** New
  [`docs/operations/metrics.md`](docs/operations/metrics.md) is the single, complete reference for
  every emitted Prometheus series (gossip · emergent · governor · artifact · guardrails · reason),
  wired into `observability.md`; the shipped Grafana dashboard gains emergent/guardrails/reason
  panels; `audit.md` documents `/gateway/transparency` revocation proofs and proving a guardrail
  stopped an agent; `deployment.md` gains a backup/restore note; the operations index gains a
  "Start here" funnel.
- **`mycelium-guardrails`** (new companion crate, PR 1 — the policy API; strategy + code-verified
  bindings in [`docs/plans/mycelium-guardrails.md`](docs/plans/mycelium-guardrails.md)): a
  self-imposed, **tier-labelled** structural-guardrail declaration on the public API only. One
  `Policy` compiles to boundary groups (Tier A), `AgentPolicy` transition guards (Tier B), and
  provider-side `authorized_callers` (Tier C — hard prevention); `Policy::strength_report()`
  discloses which clause is hard-prevented vs self-imposed vs detection. `apply()` configures
  **this** node (no remote policy authority); under `compliance`, `check_caller`/`guarded_rpc_serve`
  gate a served RPC and **seal** each `Invoke`/`Denied` into the tamper-evident audit chain (the
  "prove X was stopped" foundation). The wedge demo, verification tool, and examples are later PRs.
- **`mycelium-guardrails` policy-audit verification tool + worked wedge demo** (PR 2, feature
  `compliance`): `prove_denials`/`narrate_proof` reconstruct a provider's tamper-evident chain and
  prove which unauthorized invocations it sealed as `Invoke`/`Denied` — the honest claim is
  *provable-stopping* (these denials cannot have been forged/reordered/removed without the chain
  failing to verify), **not** a global "X could not have done Y" (the chain is per-node; only gated
  capabilities seal denials). The self-contained `guardrail_wedge` example (an unauthorized agent
  structurally stopped at the provider gate; the proof reconstructed by a neutral observer node) +
  `ci_smoke.sh` earn the wedge at the smoke bar.
- **`mycelium-guardrails` broader worked example + guide chapter 16** (feature `compliance`): the
  self-contained `guardrail_fleet` example composes all three strength tiers in one constructive
  surplus-food-rescue / community-energy co-op fleet and shows each one *actually firing* — a
  region-scoped agent that structurally drops another region's dispatch at its boundary (Tier A), an
  agent refused a denied tool at its `→ Invoking` state transition (Tier B), and an unauthorized
  caller rejected + sealed + proven at a settlement provider (Tier C); `ci_smoke.sh` now gates both
  demos. (Guide chapter 16 · Guardrails is already committed on this branch.)
- **`mycelium-reason`** (new companion crate; strategy + code-verified bindings in
  [`docs/plans/mycelium-reason.md`](docs/plans/mycelium-reason.md)): the v3.0 Tier-3
  differentiators on the public API only — **capability-routed inference**
  (`serve_model`/`InferenceRouter`: model-is-a-prompt-skill `llm/{model}` + attributed
  `llm-meta/{model}` ad; resolve → drop opaque → rank by pheromone fill → failover),
  **fleet-reasoning traces** (`TraceRecorder`/`replay`/`narrate` on per-node log
  substreams `reason/{run_id}/{node}`, optional audit-chain anchoring under
  `compliance`), **artifact-aware resume** (demand half: `require_model` +
  structural `await_ready` + `llm/loading` progress), and the content-addressed
  blob tier (`FsBlobStore`/`MeshBlobStore`/`spawn_blob_server`, ≤ 8 MiB single-frame
  v1) with `/gateway/reason/{blob,trace}` routes for the LangGraph checkpointer.
- **Reason routing gateway + Python client**: `POST /gateway/reason/route`
  (`InferenceRouter`-backed — load-aware, failover; the mesh-native counterpart to
  single-shot `/gateway/llm/call`) and `mycelium.ReasonClient` in `mycelium-py`
  (`route`/`trace`/`blob_put`/`blob_get`), unblocking a load-aware, failover LLM node
  for the LangGraph ladder (rung 4).
- **The deploy/reheal flagship (rung 6, echo variant)** — `mycelium-reason`'s
  `reheal_node` example (`SERVE_MODEL` publishes a content-addressed model artifact +
  advertises its id in KV; `REHEAL` declares the demand via `require_model`, fetches the
  artifact over the mesh with `MeshBlobStore`, and bridges it into a live `serve_model`
  skill) plus a self-contained Python driver `examples/langgraph/06_deploy_reheal.py`,
  wired into the `python-sdk` CI job. Proves end to end, deterministically, that a
  LangGraph graph's model dependency follows it across a node failure: a thread
  checkpointed on node A resumes on node B — which rehealed the model from the mesh —
  after A is killed. Echo fixture (the artifact is a blob, "serving" is `EchoBackend`);
  the real seam (`require_model` → mesh fetch + verify → `serve_model` bridge → routed
  resume) is exercised for real.
- **The deploy/reheal flagship, real-model half (rung 6, Ollama-manual)** —
  `examples/coop/src/bin/reheal_deploy.rs`: a governed GGUF reheals onto the surviving node and
  generates real tokens through routed inference after the origin dies. Composes `model_deploy`'s
  artifact-library machinery (signed weights + profile as content-addressed Blobs, `Provisioner` +
  `BlobRuntime`, live `llm/loading` percent) with `mycelium-reason`'s `serve_model` bridge and
  `InferenceRouter`: two provider depots each `supervise(profile, 1)` the model, so when the origin
  is killed the survivor elects on the bare `min=1` invariant, streams the weights afresh,
  `ollama create`s them, and re-serves the routable `llm/{model}` the app routes to. Manual (needs
  Ollama + a GGUF), excluded from `ci_smoke.sh` exactly like `model_deploy`. Honest single-machine
  caveat: A and B share one local Ollama daemon, so each creates under `{model}-{port}` — the
  streamed bytes + the Mycelium capability follow the thread (per-node Ollama for true multi-machine).
- **The LangGraph example ladder, rungs 0–5 (echo)** — five runnable, self-checking Python
  demos under `examples/langgraph/` completing the series below the flagship: `00_hello_skill`
  (a mesh skill is a LangChain `Runnable`), `01_typed` (`call_typed` through the mesh),
  `02_durable_state` (graph state survives a fresh client), `03_cross_node` (any node resumes
  any thread by gossip), and `05_traces` (replay/narrate routed inference) — plus a
  `examples/langgraph/README.md` ladder index and the echo-rung loop folded into the
  `python-sdk` CI job. Enabling surface for rung 5: `POST /gateway/reason/route` gained an
  optional `run_id` (mirrored by `ReasonClient.route(run_id=…)`) so a Python-driven routed
  call **records** its route + `llm_call` trace, fetchable via `/gateway/reason/trace`.
- **The Python tier of `mycelium-reason` (v3.0 Tiers 1+2)**:
  **`langgraph-checkpoint-mycelium`** (new package) — a LangGraph `BaseCheckpointSaver`
  backed by the mesh (index rows in gossiped KV under `ckpt/`/`ckptw/`, payloads as
  content-addressed blobs with free channel-value dedup; sync + async; cross-node resume
  of a real `StateGraph` proven in CI) — and **`mycelium.call_typed`** in `mycelium-py`
  (pydantic-validated skill output with a validation-feedback retry loop; pydantic via
  the `typed` extra). Driven by the repo's **first Python CI job** (`python-sdk`: a
  two-node `reason_node` mesh + pytest). The mesh blob fetch now answers the empty blob
  from its content address alone (a typed `None` payload serializes to zero bytes; an
  empty RPC reply means "miss", so it could never travel the wire).
- **The artifact library** (`mycelium-wasm-host`; design record
  [`docs/design/artifact-library.md`](docs/design/artifact-library.md)): a durable origin tier for
  content-addressed artifacts — `FsLibrarySource` (blob dir + signed `Manifest`;
  `Manifest::append_entry` is the one-call CI publish step), the **librarian** role
  (`spawn_librarian`: serve + `artifact/librarian` discovery + signature-scoped
  manifest→catalogue reconcile), `MeshArtifactSource::resolving` (holders discovered via the
  capability ring — no hardcoded node-ids), and the HTTP object-store source
  (`BlobFetcher` / `PrefetchingSource` / `HttpLibrarySource`, egress-gated before dispatch).
- **Artifact kinds + node runtimes**: `ArtifactKind` (WasmComponent | Blob) in a clean-slate
  versioned entry encoding; `ArtifactRuntime`/`Installed` traits — `WasmHost` becomes the engine
  inside one runtime; `BlobRuntime` places models/data (ranged streaming via
  `RangedArtifactSource`, complete-or-absent placement, activation hook, pluggable probe). The
  `Provisioner` gains a kind registry, async install reservations, **resource-aware
  eligibility** (signed per-entry `requires{disk,mem}`, `ResourceProbe` + headroom fraction,
  in-flight reservations counted), real `{ns}/loading` percent tiers, and a per-round **probe
  health pass** (fail → withdraw → reinstall).
- **Provenance binds the whole entry** (version‖kind‖artifact‖requirements‖capability): a signed
  artifact cannot be re-labeled under a different capability or kind, and resource requirements
  are tamper-evident (cost hints remain unsigned ranking inputs).
- **Examples**: `catalog` reworked honest (runtime-read library origin, librarian, origin death →
  peer-cache install); `mcp_toolgrowth` now installs real arriving code (new committed
  `unit_convert_component` fixture; activation-vs-installation taught explicitly); new **manual**
  `model_deploy` demo — real GGUF weights **and** their deployment profile as two signed
  artifacts (profile → weights by content address, design §4.3.1), activated into Ollama,
  generating real tokens under the governed profile.
- **Typed install errors**: `InstallError` is now an enum
  (`Fetch | Verify | Place | Activation | Resources | Host`) with a stable `stage()` label —
  callers match on cause instead of parsing strings.
- **Operator-visible artifact tripwires**: the provisioner/librarian emit
  `mycelium_artifact_*` counters through the `metrics` facade (ineligible-skip reasons,
  install started/completed/failed-by-stage, probe withdrawals, librarian publish/tombstone).
- **The CI flake tier** (`scripts/ci-retest.sh`): socket-binding suites re-run only their
  failed tests once — a deterministic failure still reds the build (fails twice); a flake
  keeps it green but emits a loud annotation (a bug report, never silence).

- **`subscribe_log_group` exact-once delivery across nodes** (#149; gate: overlay S11, verified
  6/6 green locally). The gateway consumer-group endpoint's "distributed lock" was a bare LWW
  gossip-KV write that returned a guard unconditionally — no mutual exclusion, so every cross-node
  subscriber "held" the claim and each drained the whole stream (100% double-delivery). Reworked to
  a **single-active** consumer chosen by a **leased consensus claim with converged-holder
  confirmation**: two near-simultaneous proposers can both *optimistically* commit (each checks only
  its local committed view), so the propose return isn't trusted — after committing, the node reads
  the **converged** committed holder (commit-keys are LWW-by-HLC, so exactly one converges) and only
  that node consumes; losers stand by without releasing (a tombstone would clear the winner's claim).
  The winner drains with a **private local offset** (exact-once by construction) and renews the
  lease; on its death the lease lapses and a standby takes over (failover). `SubscribeHandle` and its
  bare-LWW "lock" are removed. Contract clarified: `subscribe_log_group` is a single-active *log
  consumer*, **not** a load-balanced work queue (that is the `mycelium-tuple-space` companion) —
  pinned in the `runtime-invariants` "do not fix these" note so the dead-end isn't re-attempted.
- `mycelium-wiki` integration tests: the `free_port()` bind-race flake class retired
  (pair-granularity bind retries; `AddrInUse` CI failure 2026-07-07).
- `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204; bench-only dependency path).

## [2.0.0] — 2026-07-04

The **v2.0 epoch** — all 16 milestones (M1–M16) shipped and signed off
([`docs/plans/v2.0.md`](docs/plans/v2.0.md)), plus the companion-crate ecosystem. First release since
`v1.2.0`. All workspace crates version in lockstep at `2.0.0` (a unified release train); the companion
crates (`mycelium-wiki` / `-blackboard` / `-tuple-space` / `-agentfacts` / `-wasm-host`) are newer than
the substrate and their APIs may still evolve within the 2.x line — pin exact versions if that matters.

### ⚠ Breaking changes (from `v1.2.0`)

- **Wire protocol `v10` → `v12`.** `v11` added `hlc_seq` to `Signal` (ordered delivery); `v12` replaced
  the full key→timestamp anti-entropy index with a fixed-width Merkle **`bucket_hashes`** digest. A
  `v1.2.0` node **cannot** interoperate with a `2.0.0` node except across the rolling-upgrade window —
  `read_frame` accepts `WIRE_VERSION = 12` **and** `PREV_WIRE_VERSION = 11` only.
- **Workspace restructure** — the substrate (Layers I+II) was extracted into a separate `mycelium-core`
  crate. `mycelium` re-exports an unchanged public Rust API, but consumers reaching internal module
  paths must update; and `consensus` (Layer III) is now behind a feature gate.
- **Serialization dependency** — the wire/WAL codec is the in-tree hand-rolled fixed-int codec
  (`mycelium-core::codec` / `serde_fixint`), **byte-identical** to the former `bincode`; `bincode` is
  fully removed from the dependency tree (RUSTSEC-2025-0141 no longer applies). This is *not* a wire or
  on-disk format change — persisted WAL/snapshots decode unchanged.

### Migration

- **Rolling upgrade is one step only.** A mixed `v11 ⇄ v12` cluster converges (a `v11` anti-entropy
  request downgrades to a full snapshot). Upgrading from `≤ v9` (i.e. `v1.2.0` and earlier) is a
  **stop-the-world** upgrade: drain, stop the cluster, deploy `2.0.0`, restart. WAL/snapshots persisted
  by the former codec load without migration.
- **No public API removals** in `mycelium` itself; new capability arrives behind additive feature flags.
  Before a production go-live, walk [`docs/operations/production-readiness.md`](docs/operations/production-readiness.md);
  for a first customer engagement, [`docs/operations/customer-pilot.md`](docs/operations/customer-pilot.md).

### Added

- **`mycelium-wiki` companion crate** — a group-scoped, LLM-curated wiki: the durable, curated third coordination primitive (long-term-memory sibling of the blackboard's working memory), built on the public `mycelium` API only. **Control-plane / data-plane**: the corpus lives in a node-independent pluggable store (the `WikiStore` trait + an `FsStore` reference impl — manifest-last, torn-read-safe); a single elected **curator** serialises writes while group agents **read the store directly, in parallel** (no curator on the read path). Curator **election + ring-failover** on the capability ring; an evaporating KV **proposal queue** (`wiki/{group}/proposal/`); a **single-writer reconcile** that groups proposals by section (`DirectReconciler` lossless append-merge, or `LlmReconciler` 3-way merge behind `llm`); a **change-driven lint** loop (structural dead-cross-link/empty-section checks always on, LLM self-consistency behind `llm`; runs only after a write); **MCP tools** (`wiki.read`/`query`/`propose`) and an **HTTP gateway** (`/gateway/wiki/*`, feature `gateway`) with Python/TS `Wiki` SDKs; and a membership-gated **access broker** (`Wiki::request_store_access` → `StoreGrant`, RPC point-to-point). `Wiki::shutdown` reclaims the curator's background tasks. Features `control-plane` / `llm` / `gateway`; cross-node `tests/{failover,gateway,access}.rs` + the `wiki_chat` worked example (`ci_smoke.sh`, both use-case corpora). Audited in analysis Run 32 (one Major finding found + fixed same-session). Design/plan: `docs/plans/mycelium-wiki.md`; companion page `docs/wiki/dev/companions/wiki.md`.
- **Legible Emergence — coordinator-free fleet diagnosability** — make an emergent, coordinator-free fleet debuggable by a *non-designer* (Detect → Localize → Explain → Intervene) without a central collector: diagnostics are computed from each node's locally-held KV + HLC causal order + a **bounded** scatter-gather fan-out (`EXPLAIN_MAX_FANOUT`, with the skipped set *named* as `not_queried`, never silently dropped). Emergent tripwires + counters, a fleet snapshot, causal-order reconstruction, a fleet-state narrative, and the operator surface: `GET /gateway/explain` + `/gateway/diagnose` and the `make check` / `make check-full` pre-push gates. Phases 0–5. Plan: `docs/plans/legible-emergence.md`.
- **`mycelium-blackboard` companion crate** — content-routed shared working memory (the peer of the tuple space, routing by a **predicate over fact attributes** rather than lane position): `claim(predicate)` is a competitive, non-blocking destructive claim (Linda's `in`), `read`/`rd` is shared, `ack` is the idempotent terminal, `release`/deadline re-queues (at-least-once). `BoardStore` (WAL, magic `MBBWAL`) + `Blackboard` (roles + RPC + failover, `Post`/`Ack`-only replication). HTTP gateway `/gateway/bb/*`, Python/TS SDKs, the community-microgrid worked example. WS-G / G3. Plan: `docs/plans/v2-wsg-g3-blackboard.md`.
- **`mycelium-tuple-space` companion crate** — Linda-style pull-based pipeline buffer as a workspace member, built entirely on the public `mycelium` API (the composability proof: zero core changes were needed). Workers `take()` when ready, so readiness is self-announcing and the push-predict staleness/misroute failure mode does not exist. Single-lock store hot path; WAL durability with 4 record types (`Complete` is one indivisible record so a stage transition can never half-replay) and epoch'd compaction; `TupleRole::{Primary, Secondary, Auto, Client}` with secondary mirroring via replicate RPCs, heartbeat Signal, and promotion when the primary's capability evaporates; `Auto` elects with a lowest-candidate-id tie-break. Owns the `tuple/inflight/{ns}/{id}` and `sys/tuple/{node}/{ns}/…` KV prefixes. HTTP gateway (`/api/tuple`), Python and TypeScript SDKs, integration scenario 13. Design doc: `docs/plans/mycelium-tuple-space.md`.
- **TupleSpace WAL format header** — every tuple-space WAL now opens with `MTSWAL` magic + u16 LE version (v1). A file with a newer format version, or without the magic, is refused at open **byte-untouched** with an error naming both versions; previously an unrecognised record kind read as a torn tail and was silently truncated — an upgrade data-loss hazard. The header survives compaction, and the secondary replay-chunk cursor clamps past it. Format break is free: no earlier WAL shape was ever in a release.
- **HLC remote clock-drift bound** — `GossipConfig::max_clock_drift_ms` (also `GOSSIP_MAX_CLOCK_DRIFT_MS`; default 300 000 ms = 5 minutes, `0` disables). `Hlc::observe` now clamps remote physical time to `wall_now + bound`, with a rate-limited `warn!` naming the offending drift when the clamp engages. Previously one peer with a far-future clock (NTP failure, or hostile in a non-TLS cluster) dragged every node's HLC forward irrecoverably — the `max` never decays — and read-side evaporation, the substrate's failure detector (including tuple-space secondary promotion), was *silently suspended for the full drift duration*. The cited Kulkarni et al. 2014 HLC algorithm mandates exactly this bound. Documented trade-off in the `hlc` module: stamps beyond the bound waive the "local write after observe dominates remote" guarantee; store-level rejection of out-of-bound updates is deferred to the next wire-policy pass.
- **Symmetric capability-freshness window** — `CapEntry::is_fresh` / `ReqEntry::is_fresh` now also treat entries stamped further in the **future** than the 3× evaporation window as stale. A writer whose clock is persistently ahead by more than 3× its refresh interval quarantines itself instead of becoming un-evaporable; failure detection no longer depends on the sender's clock sanity. Regression gates: `observe_bounds_remote_clock_drift` (hlc), `future_stamped_entry_is_quarantined_not_fresh` (capability).
- **Commit-conflict tripwire** — the consensus listener now refuses to endorse a `COMMIT` carrying a *different* value for a slot whose existing commitment is still live (slots are commit-once; leased slots reopen only after expiry). Conflicts are logged at `warn!` and counted in `SystemStats::commit_conflicts` (also on `GET /stats`). Namespace ownership of `consensus/` remains promise-strength by design — the tripwire makes violations legible without teaching Layer I a Layer III law.
- **Epoch-leased commitments** — `ConsensusConfig::committed_lease_secs: Option<u64>`. When set, the commit also writes `consensus/lease/{slot}` (u64 LE ms) and lease expiry is evaluated **read-side** against the committed entry's HLC timestamp — the same evaporation convention as `CapEntry::is_fresh`, no background task, no renewal RPC. An expired lease reads as not-committed and the slot reopens for re-proposal. Renewal = re-proposing the same value while the lease is live (a fresh quorum round that refreshes the commit timestamp); a different value while live returns `Superseded`. Default `None` = permanent commitment (existing behaviour preserved). Lease-aware readers: `consensus_get`, `consistent_get`, `elect_leader` winner lookup, `GET /consensus/{slot}` (now also returns `lease_ms` + `lease_expired`); `consensus_rx` is deliberately the raw KV view.
- **Proposer-side clobber guard** — `try_commit_if_ready` and `cross_propose` now return `Superseded` instead of overwriting when a *different* live commitment landed between the supersession check and quorum (a lost race with another proposer).

### Security

- **Decode allocation bound (remote DoS fix)** — `bincode_cfg()` now sets `.with_limit::<MAX_FRAME_BYTES>()`. Without it, a frame whose internal length prefix claimed a huge element count drove an unbounded `Vec::with_capacity` and the process OOM-aborted (SIGABRT) — one malformed frame from any connected peer, or a bit-flip on a non-TLS link, killed the node. `read_frame` capped the frame size but not the element counts decoded from inside it. All decoders share the config, so the whole wire surface (gossip, capability, signal, locality, WAL sync) was exposed. Found by a decoder mini-fuzz now kept in-suite (`mini_fuzz_decoders_survive_adversarial_bytes`, `fuzz-internals` feature) and wired into CI — the `fuzz/` targets existed but had never run in CI.
- **Dependency advisories cleared** (lockfile bumps, no manifest changes): `bytes` 1.10.1 → 1.11.1 (RUSTSEC-2026-0007, integer overflow in `BytesMut::reserve` — `read_frame` calls `reserve` on the wire path, though the 10 MiB frame cap already bounded the input), `tracing-subscriber` 0.3.19 → 0.3.20 (RUSTSEC-2025-0055, ANSI-escape log poisoning), `tokio` 1.44.1 → 1.46.1 (RUSTSEC-2025-0023, broadcast-channel unsoundness). `cargo audit` now reports zero vulnerabilities; remaining unmaintained-crate warnings (notably `bincode`, the wire codec) are tracked as a roadmap concern.

### Added

- **A2A agent card: schema-aware skills** — `GET /.well-known/agent.json` now populates each skill's `description` from its gossiped input schema (`skills/{ns}/{name}/{node}/input`, published by SkillRunner) and exposes the raw JSON Schema as an additive `inputSchema` field. Tool-calling frameworks build properly-typed tools from it instead of guessing payload shapes from prose — previously the empty description left LangChain/AutoGen agents passing plain text to JSON-expecting skills, which failed with a parse error and let the agent silently fall back to answering from its own weights. The bundled `examples/a2a_langchain/` agents (ported to LangChain ≥ 1.0 `create_agent` and current AutoGen) now derive their tool signatures from `inputSchema`.

- **Demo smoke in CI** — `examples/community/ci_smoke.sh` runs the community cluster against a deterministic mock LLM (`mock_llm.py`, stdlib-only, OpenAI-compatible) on every push: 4-skill convergence, schema-aware agent card, A2A + dashboard router coexistence, the full orchestrator → researcher → writer tool-call pipeline, and SIGTERM cleanup. Each assertion is a regression gate for one of the four bugs the 2026-06-11 live run-through found; the mock additionally rejects non-string chat content exactly as Ollama does, so the tool-result coercion fix cannot silently regress. SkillRunner now **re-asserts its gossiped input/output schemas periodically** (ttl/4, ≥ 5 s) like capability advertisements — a one-shot startup write could race peer-connection establishment and leave tool discovery incomplete for tens of seconds (observed 1-in-3 under load; 8/8 clean after).

### Added (observability)

- **`individual_flood_fallbacks` counter** — on `SystemStats` and `GET /stats`. Counts Individual-scoped frames (RPC requests/responses, consensus votes) that had no direct sender→target route and fell back to flooding — plus, with a rate-limited `warn!`, the residual case of an Individual frame dropped with zero peers. Non-zero under steady state is correct behaviour but signals topology pressure: RPC-heavy pairs without direct peering pay relay latency. Companion to the flood-fallback fix below; the resilience scale test gains a Phase 1b cross-worker RPC gate that exercises exactly this path under `GOSSIP_MAX_ACTIVE_CONNECTIONS`-capped partial meshes.

### Changed

- **AFN fluid-pipeline demo migrated to the pull pattern** — `examples/fluid_pipeline/` now runs the canonical tuple-space architecture by default: workers `take()` from the deepest stage and `complete()` into the next (fluidity = self-selection against per-stage depth), and the former coordinator collapses into a seeder/sink edge client. The original coordinator-dispatch architecture — the project's own named anti-pattern — is retained behind `PIPELINE_MODE=push` as the comparison baseline for the push→pull refinement (`flow_networks.html`, Paper 2a). New `ci_smoke.sh` runs both modes end-to-end as local processes (3 nodes, 24 items, fresh cluster per mode) and is wired into CI as the `afn-smoke` job, so both distribution models are regression-gated.

- **`/gateway/llm/call` reports failures via HTTP status codes** — 404 (`no_provider`), 502 (provider-side error, incl. `parse_error`), 504 (RPC `timeout`); the `{"error":...,"detail":...}` JSON body is unchanged. The endpoint was the gateway's one 200-on-error outlier (every other handler already used `BAD_REQUEST`/`NOT_FOUND`/`GATEWAY_TIMEOUT`), which made failures invisible to `curl -f` and `raise_for_status()` callers — integration scenario 12's flake diagnostic was an empty `{}` for exactly this reason. The SSE `/gateway/llm/stream` endpoint deliberately keeps in-stream `{"type":"error"}` events: the status line is committed before the stream body. SDKs already throw/raise on non-2xx; the Python docstring now documents the raising behaviour. Regression test: `test_llm_call_no_provider_returns_404`.
- **`writer_channel_depth` default raised 256 → 1024** — both scale tests recorded dropped frames at burst (56 at 100 nodes / depth 2048 override; 92 at 5 000-key bulk / depth 4096 override), and the doc comment's own budget math (`N × F` at fan-out 4, N = 256) says 1024. Channel memory is per in-flight frame, not preallocated, so idle cost is nil. Bulk-write workloads should still override to 4096+ via `GOSSIP_WRITER_CHANNEL_DEPTH`.

### Fixed

- **Fan-out activation was polled, not event-driven — inbound-only nodes were mute for live sends for up to two health-check intervals** — the gossip loop's send-target list comes from a watch channel that only the health monitor published (10 s default cadence, plus a change-check that can skip the first tick), while peers are learned on Ping receipt. A node with no outbound dials — a seed, a tuple-space primary, any pure listener — could not send *anything* live (signals, RPC responses, consensus votes) to freshly-connected peers until the cadences aligned: worst case ≈ 2× `health_check_interval`. Anti-entropy silently healed KV, which is why this never surfaced; one-shot Individual frames (RPC replies!) just timed out. Found by the new random-topology property test (`test_individual_consumers_over_random_partial_meshes`: relay delivery worked at "attempt 7" on one graph — exactly a health tick — and never within 8 s on another). Fix: the connection handler publishes the updated peer list at insertion time; the health monitor remains the steady-state reconciler/evictor. All three graphs now deliver on attempt 0.
- **Individual-scoped signals silently dropped when the target was not in the sender's outbound peer list** — with `group_aware_forwarding` (default on), `ForwardHint::Individual` sent the frame *only* to a directly-peered target and otherwise to nobody: the signal never entered the medium. Individual scope carries RPC requests, RPC responses, and consensus votes, so in any partial mesh — exactly what `GOSSIP_MAX_ACTIVE_CONNECTIONS` (the documented iptables mitigation) and `max_forwarding_peers` produce — RPCs timed out and ballots starved between non-peered pairs, with nothing logged. This also contradicted the architecture's stated model (forwarding is unconditional; only *admission* is scoped — `Boundary::admits`). The targeted send is kept as an optimization when a direct route exists; otherwise the frame now falls back to unconditional flooding (each hop applies the same rule; the seen-set dedups, hop-TTL bounds it). Found during the three-arm experiment bring-up: a synchronized first-`take` volley wedged all workers for the full RPC deadline whenever responses raced route establishment. Regression test: `test_individual_signal_reaches_unpeered_target_via_relay` (line topology A→B→C; fails pre-fix).
- **Tombstone GC never fired since the v9 HLC migration (2026-05-20)** — the GC predicate compared the store entry's *packed HLC* timestamp (`(physical_ms << 16) | logical`) against a wall-clock-*millisecond* cutoff; a packed stamp is ~65 536× any ms cutoff, so the condition was unsatisfiable and every tombstone accumulated forever (unbounded store growth on delete-heavy workloads). Every other timestamp consumer (`CapEntry::is_fresh`, seen-set eviction) unpacks via `hlc::physical_ms`; the GC was the one that didn't. The sweep is extracted to `store::sweep_stale_tombstones` (unpacks correctly, preserves the conditional-remove discipline from the race-family fix below). Found by an M2 Run-21 falsification probe; regression test `tombstone_gc_sweep_unpacks_hlc_timestamps`.
- **TypeScript SDK: `shardFor()` crashed on every call** — it referenced `this._base` (the property is `base`) and omitted the path's leading slash; plus 7 further `tsc` errors from assigning fetch's `unknown` JSON to typed values. CI now runs `tsc --noEmit` over `mycelium-ts` (new `sdk-ts` job) so the SDK can't ship type-broken again, and a dedicated time-boxed `cargo fuzz` job covers the wire/capability decoders that the in-suite mini-fuzz samples more shallowly.
- **`with_http_routes` replaced earlier routers instead of merging** — the extra-routes slot was last-caller-wins, so composing registrations silently dropped all but the final one. Concretely: SkillRunner registers `with_a2a()` and then its management dashboard, and the dashboard erased the A2A endpoints — `/.well-known/agent.json` 404'd in exactly the documented A2A setup. Routers are now merged (`Router::merge`); regression test `with_http_routes_merges_across_calls`.
- **A2A `tasks/send` server-side timeout raised 30 s → 120 s** — A2A skills are frequently multi-step LLM pipelines (orchestrator → researcher → writer takes ~90 s on local Ollama); the 30 s cap made every such composition return `-32603 rpc call failed` while the pipeline was still working.
- **SkillRunner: tool results sent to chat APIs as raw JSON** — tool-role messages carried `content` as a JSON *object* when a tool returned structured output; Ollama rejects non-string content (`invalid message content type: map[string]interface {}`), breaking every skill→skill composition whose callee returns JSON. Tool results are now coerced to strings, same as user input already was.
- **SkillRunner survived SIGTERM indefinitely** — the shutdown task drained the agent but the skill loop never returns, and the consumed signal suppressed the default terminate action. Generations of "stopped" skillrunners accumulated invisibly across demo runs, with `SO_REUSEPORT` letting every generation keep sharing the same ports (old binaries answered a fraction of requests). The shutdown task now exits the process after the agent drains; demo `stop.sh` gained an orphan sweep.
- **Example/demo repairs from a live run-through** — `mesh_demo` referenced manifests at a path renamed long ago (hidden by cargo's incremental cache; examples now built in CI); the community demo's convergence check counted a KV prefix that cannot match the real `cap/{node}/{ns}/{name}` key shape; `invoke.sh`'s fallback caller hardcoded the `llm/hello` smoke-test capability (now driven by `SKILL_CAP`/`SKILL_PAYLOAD`); cold-start bind races in `start.sh`/`demo.sh` (spokes now wait for the seed's port); `a2a_langchain/requirements.txt` used a non-portable `file:` relative reference.
- **Prefix-index divergence under concurrent tombstone/insert** — `apply_and_notify` maintained the secondary structures (`prefix_index`, `cap_ns_index`, `peer_localities`) *after* the lock-free store CAS, derived from the update being applied. Two winning writers to the same key could interleave their index ops in the opposite order of their CASes — e.g. a delete racing a higher-timestamp rewrite arriving on another shard — leaving a live store key permanently invisible to `scan_prefix` and capability resolution. Anti-entropy could not repair it (re-applying the same `(key, ts)` loses LWW and never touches the index); only a later rewrite of the key did. Index maintenance is now a *reconcile*: under a per-key-hash stripe lock (`KvStore::index_stripes`, 64 stripes), the writer re-reads the stored entry and sets membership in every secondary structure to match it, so the final index state always matches the final store state. Found by an M2 falsification probe (86 of 100 000 racing rounds reproduced the loss); the probe and an 8-thread mixed-churn consistency test are kept as regression gates.
- **Signal handler registration could panic under contention** — the `HandlerTable` registration closure moved its sender into the map via a single-use `slot.take().expect(...)` inside a papaya `compute`. papaya re-invokes the closure when the entry changes concurrently, so two tasks registering the same signal kind simultaneously (or one racing the closed-sender eviction in delivery) panicked on the retry. The closure now clones the sender per invocation. Regression test: `concurrent_same_kind_signal_registration_does_not_panic` (reproduced the panic instantly pre-fix).
- **Concurrent `set_with_min_acks` on the same key starved each other** — the per-key tracker slot was single-occupancy: a second concurrent caller overwrote the first caller's tracker, and the first caller's unconditional cleanup then deleted the second's — both could report spurious timeouts while the acks arrived. Each key now holds a copy-on-write *list* of trackers (`kv_quorum::{install_tracker, remove_tracker}`): every inbound update is observed by all in-flight callers and each caller removes exactly its own tracker by `Arc` identity. Applies to both the Rust API and the HTTP gateway endpoint.
- **Prompt-skill registration races** (`llm` feature) — (1) two first registrations racing could both observe an empty registry and spawn two `llm.invoke` dispatch loops, each receiving every invoke signal (duplicate RPC responses); the spawn is now gated by an atomic swap. (2) Dropping a stale `PromptSkillHandle` after the same skill id had been re-registered deleted the *new* backend from the registry; the cancellation path now removes only if the registry still holds the backend it registered.
- **A2A task cleanup could evict a live task** — the 5-minute sweep collected stale task ids and then removed them unconditionally; a status update re-inserting the task with a fresh `created_at` between collect and removal was evicted, and clients polling the task got NotFound. The sweep now uses a conditional `compute` (remove only if still stale at removal time).
- **Tombstone GC could delete a concurrent live write** — the GC task collected stale-tombstone keys and then removed them *unconditionally*; a live write winning the store CAS on the same key between collect and removal was deleted outright (recoverable only via anti-entropy from a peer). Same race family as the prefix-index fix above. The removal is now a conditional `compute`: the entry is removed only if it is still a stale tombstone at removal time.
- **LWW equal-timestamp divergence** — concurrent data writes to the same key carrying *identical* HLC timestamps (two writers in the same wall-clock millisecond whose clocks had not yet observed each other) previously resolved by arrival order: each node kept whichever value it applied first, diverging permanently — and undetectably, because the anti-entropy digest hashes `(key, timestamp)` only and was identical on both sides. `lww_wins` now breaks data-vs-data timestamp ties deterministically (lexicographically greater value wins), so apply order no longer matters. Tombstone tie rules are unchanged (tombstone still wins ties; data never resurrects a tombstone on a tie). Rolling-upgrade note: nodes on older versions lack the tiebreak, so a mixed cluster retains the old exposure on exact ties until fully upgraded — no worse than before.
- **Consensus listener registration race** — `start_consensus_listener` now registers the PROPOSE/COMMIT signal receivers synchronously before spawning the voter task. Previously registration happened inside the task's first poll, so a proposal arriving in the startup window was silently dropped and the node failed to vote on it.

---

## [1.1.0] — 2026-06-07

### Added

- **Per-peer gossip rate-limiting** — `GossipConfig::max_inbound_frames_per_sec` (also `GOSSIP_MAX_INBOUND_FRAMES_PER_SEC` env var). When set to a non-zero value, frames received faster than this rate from a single peer are dropped with a warning log. Prevents a malicious or misbehaving peer from flooding the inbound processing pipeline. Default `0` = unlimited (existing behaviour preserved).
- **`bulk_serve` handler concurrency cap** — `GossipConfig::max_concurrent_bulk_handlers` (also `GOSSIP_MAX_CONCURRENT_BULK_HANDLERS` env var). Limits the number of concurrent per-request background tasks spawned by `bulk_serve` via a `tokio::sync::Semaphore`. When the cap is reached, new bulk signals are dropped with a warning. Default `64`; set to `0` for unlimited.

### Changed

- **`GossipError::Config(String)` replaced by three structured variants** — `InvalidField { field: &'static str, reason: String }`, `FieldConflict { field_a, field_b, reason }`, `NodeIdMismatch { node_id, bind_addr }`. Callers can now match specific configuration failures without parsing error strings. All `validate()` and `apply_env_overrides()` error paths updated.
- **`GossipError::Network(String)` replaced by two structured variants** — `FrameTooLarge { size: usize, limit: usize }` and `UnsupportedWireVersion { received: u8, current: u8, prev: u8, hint: &'static str }`. Framing errors are now fully typed; callers can distinguish oversized frames from version mismatches.

### Added

- **HTTP gateway bearer-token authentication** — `GossipConfig::gateway_auth_token: Option<String>` (also `GOSSIP_GATEWAY_AUTH_TOKEN` env var). When set, every `/gateway/**` request must carry `Authorization: Bearer <token>`; unauthenticated requests receive `401 Unauthorized`. Health, ready, stats, and metrics endpoints are always public. Suitable for deployments where `http_addr = "0.0.0.0"`.
- **Error handling guide** — `docs/guide/error-handling.md` documents all eight public error types (`GossipError`, `ConsistencyError`, `RpcError`, `QuorumError`, `ScatterError`, `SchemaError`, `BulkError`, `ShardError`), their recoverability classification, propagation strategy, and a relationship diagram per handle.
- **100-node scale test** — `make test-scale` starts a 100-node Docker cluster (1 seed + 99 workers + mgmt + runner), validates full gossip convergence, KV propagation (seed write → mgmt read), and zero dropped frames. Override size with `make test-scale SCALE_WORKERS=49`. Compose file at `tests/integration/docker-compose.scale.yml`; runner script at `tests/integration/run_scale.sh`.
- **`LlmHandle`** (via `agent.llm()`) — typed handle for LLM prompt-skill operations: `register_prompt_skill`, `call_prompt_skill`, `update_prompt`, `get_prompt`, `list_prompts`, `delete_prompt`. Available under `--features llm`.
- **`McpHandle`** (via `agent.mcp()`) — typed handle for MCP tool bridge operations: `register_mcp_tool` (server-role tool registration), `connect_mcp_server` (client-role tool discovery and proxying). `connect_mcp_server` requires `--features gateway`.
- **`CapEntry` re-exported** from crate root — allows external tooling and benches to encode/decode capability entries from the gossip KV namespace.
- **`#[non_exhaustive]` on all public error and result enums** — `GossipError`, `ConsistencyError`, `RpcError`, `QuorumError`, `ScatterError`, `SchemaError`, `BulkError`, `ShardError`, `McpError` are now `#[non_exhaustive]`. Adding new variants in future releases will not break exhaustive `match` arms in downstream code.
- **Wire rolling-upgrade test** — `read_frame_accepts_prev_wire_version` in `src/framing.rs` verifies that v10 Signal frames (no `hlc_seq`) are accepted by `read_frame`, decoded via `WireMessageV10`, and converted to `WireMessage::Signal { hlc_seq: None }`.
- **`Capability::encode()` / `CapEntry::encode()` made public** — these were `pub(crate)`; now `pub` so external tooling can serialise capability entries for seeding or testing.
- **Capability-resolve benchmark** (`benches/throughput.rs`) — measures `capabilities().resolve()` against 1/10/50/100 pre-seeded providers; shows O(providers) scan cost.
- **KV payload-size benchmark** (`benches/throughput.rs`) — measures `kv().set()` at 64 / 1 024 / 65 536 byte payloads; exercises the framing encode path at representative sizes.
- **Typed sub-handle facade** — `GossipAgent` exposes eight domain-scoped handles, each a zero-cost `Arc<TaskCtx>` clone: `KvHandle` (via `agent.kv()`), `MeshHandle` (via `agent.mesh()`), `CapabilitiesHandle` (via `agent.capabilities()`), `ConsensusHandle` (via `agent.consensus()`), `ServiceHandle` (via `agent.service()`), `SchemaHandle` (via `agent.schemas()`), `LlmHandle` (via `agent.llm()`), `McpHandle` (via `agent.mcp()`). All domain methods live exclusively on their typed handle; `GossipAgent` retains only lifecycle and utility methods.
- `gateway` Cargo feature (on by default) — gates the Axum HTTP server and its transitive deps (`axum`, `tower-http`, `tokio-stream`, `futures-util`). Disable with `default-features = false` for bare-metal / WASM embeds. All gossip, KV, signal, consensus, capability, and service APIs compile without `gateway`.
- `rust-toolchain.toml` — pins the toolchain to `stable`.

### Changed

- `GossipError::State(String)` replaced by two structured variants: `GossipError::AlreadyRunning` (called `start()` on a running agent) and `GossipError::Shutdown` (called `start()` after shutdown). Callers can now match lifecycle errors without parsing strings.
- `set_quorum` renamed to `set_with_min_acks` — name now reflects the actual semantics (wait for N gossip echo receipts, not consensus quorum).
- Cargo.toml `description` improved: now accurately describes the three-layer substrate.
- `a2a` and `llm` features now imply `gateway` (they expose HTTP endpoints).

### Added

- `Capability::with_schema_id` / `CapFilter::with_schema` — optional contract version gossip-propagated with every capability entry. Resolvers that call `with_schema` only match providers advertising the same `schema_id`; capabilities without a `schema_id` do not match (strict by default).
- `Capability::with_input_schema` / `with_output_schema` — embed JSON Schema strings directly in the gossip-propagated capability entry so callers can inspect the invocation contract from `resolve()` results without a separate KV lookup. SkillRunner now embeds `.skill.toml` input/output schemas in the capability in addition to the existing `skills/.../input` KV keys.
- `GossipAgent::signal_rx_from(kind, trusted)` — delivers only signals whose `sender` is in the trusted list. Addresses the semantic-injection attack vector (arXiv 2511.19699 §5.1) for LLM-driven agents processing signal payloads as prompts. Empty `trusted` list delegates to the unfiltered path with no overhead.
- Speech act taxonomy in the crate-level doc comment: maps FIPA-ACL performatives to Mycelium primitives.
- `examples/semantic_coordination.rs` — in-process example demonstrating all three features.
- `GossipAgent::publish_schema(schema_id, json_bytes)` — validates JSON, conflict-detects against the existing `schemas/{id}` KV entry, and writes only on `Published`. Returns `SchemaPublishResult::{Published, Unchanged, Conflict}`.
- `GossipAgent::force_publish_schema` — overwrites without conflict detection; intended for dev / migration tooling.
- `GossipAgent::get_schema(schema_id)` — retrieves authoritative schema bytes from the KV ring.
- `GossipAgent::list_schemas()` — enumerates the full schema catalogue sorted by ID.
- `GossipAgent::seed_schemas_from_dir(path)` — seeds all `*.json` files from a directory tree; file path relative to `dir` (without extension) becomes the `schema_id`.
- `SchemaPublishResult` / `SchemaError` public types.
- `schemas/{schema_id}` added to the KV namespace ownership table.
- Wire v11: `hlc_seq: Option<u64>` added to `WireMessage::Signal` for causal ordering via `emit_ordered()`. v10 rolling-upgrade shim decodes v10 frames with `hlc_seq = None`.
- `emit_ordered()` — stamps an HLC sequence number on the signal frame; receivers with `signal_ordered_delivery = true` buffer per `(sender, kind)` and deliver in ascending HLC order.
- Watcher C2: consolidated requirement opacity watcher — one task and one `cap/` subscription for all declared requirements on a node (previously one task per `declare_requirement` call).

### Fixed

- `publish_schema` / `force_publish_schema` now validate `schema_id` and reject empty IDs, leading/trailing `/`, `//`, `.`/`..` path segments, and non-ASCII characters. `SchemaError::InvalidSchemaId` variant added.
- GC task now proactively evicts closed `prefix_watchers` and `prefix_predicate_watchers` entries on every GC cycle, preventing accumulation of dead senders when the prefix never receives a write after the subscriber drops.
- GC task now evicts orphaned `quorum_trackers` entries (those whose caller future was dropped mid-wait, leaving a dangling tracker with no live waiter).
- Signal reorder buffer now logs a `warn!` when a depth-based flush degrades causal ordering (`max_depth` exceeded). Previously this was silent.
- `rpc_pending` mutex `.lock()` calls now recover from a poisoned mutex rather than panicking, preventing a cascade failure when a panic occurs in a concurrent task.

---

## [1.0.0] - 2026-06-03

### Added

**Layer I — Gossip KV store**
- Last-write-wins key-value store propagated over TCP gossip
- Hybrid Logical Clock (HLC) causal ordering for all writes
- Anti-entropy sync: nodes reconcile state on reconnect
- Per-key TTL with lazy expiry
- Write-ahead log (WAL) + snapshot persistence; configurable sync modes (none / sync / flush)
- Prefix-based subscriptions with optional predicate filtering

**Layer II — Signal mesh**
- Ephemeral scoped signals with epidemic flood delivery
- Pheromone-style opacity composition: any `sys/load/{node}/...` key with `is_opaque=true` gates signal reception
- Signal scopes: `Node`, `Group`, `Global`, `Groups`
- Dedup via nonce; TTL-bounded forwarding

**Layer III — Epidemic consensus**
- Group-scoped, system-scoped, and cross-group proposals
- `GroupQuorum` for multi-voting-bloc decisions with independent per-group quorum fractions
- Epidemic proposal flood; no coordinator; no external Raft dependency

**Capability and discovery subsystem**
- Node-level `provides` / `requires` capability advertisement via `cap/` KV prefix
- Emergent group membership: nodes self-join groups based on local capability evaluation
- Locality-aware capability resolution with ranking and topology policies
- Group-level opacity and demand pressure tracking
- Inter-group wiring resolved per-emission (`signal_wired_via`)
- Filter opacity watcher with debounce

**Agent state machine**
- `GossipAgent` public API: KV, signals, consensus, capabilities, consistency overlay, sharding
- HTTP management gateway with SSE streaming
- RPC (`rpc_call` / `rpc_respond`), scatter-gather, Actor/Event mailboxes
- Cluster sharding (`shard_for` / `emit_sharded`)

**Consistency overlay (opt-in)**
- `consistent_set` / `consistent_get` — linearisable read-modify-write over gossip KV
- `distributed_lock` — named mutex with TTL-based lease
- `elect_leader` — leader election per named group
- `append` / `scan_log` / `compact_log` / `subscribe_log` / `subscribe_log_group` — ordered durable log with consumer-group cursors

**`--features tls`**
- mTLS peer connections using `tokio-rustls`
- Ed25519 node identity; keypair stored in `sys/identity/{node}`
- Consensus payload signing (`SignedConsensusMsg`)
- `WireMessage::SignedData` for Ed25519-signed KV writes (wire v10)

**`--features metrics`**
- Prometheus scrape endpoint at `/metrics`
- 10 counters, gauges, and histograms covering KV operations, gossip fan-out, signal delivery, consensus rounds
- Grafana dashboard at `dashboards/mycelium-grafana.json`

**`--features a2a`**
- A2A protocol adapter: `/.well-known/agent.json`, `/a2a` JSON-RPC endpoint
- Python and TypeScript `A2aClient`

**`--features llm`**
- Prompt Skills: `PromptTemplate` stored in KV, cross-node invocation via `call_prompt_skill`
- SkillRunner: `.skill.toml` capability-as-skill, OpenAI-compatible LLM driver
- HLC audit trail and OpenTelemetry tracing in SkillRunner
- MCP bridge: server-side tool discovery and routing; client-side tool consumption
- `OpenAiBackend` / `EchoBackend`

**Language bridges**
- Python sidecar bridge (local HTTP, ~1 ms overhead) — see `examples/fluid_pipeline/` and `examples/a2a_langchain/`
- TypeScript sidecar bridge — 28 methods, SSE streaming, full overlay and A2A coverage

**Examples**
- `examples/fluid_pipeline/` — Agentic Flow Networks demo: 10-worker fluid pool, KV ring as distributed buffer, 4-stage article pipeline, PostgreSQL sink. Run with `docker compose up --build --scale worker=10`.
- `examples/a2a_langchain/` — LangChain ReAct agent and AutoGen v0.4 agent auto-discovering Mycelium skills via `/.well-known/agent.json`
- `examples/community/` — 3-node demo cluster with orchestrator, researcher, verifier, and writer skills

**Wire protocol**
- Wire v10 with rolling-upgrade compatibility window (PREV = v9)
- Bincode-encoded framing; version negotiation on every peer connection
