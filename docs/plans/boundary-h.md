# Boundary H — implementation plan

> **Status: ADOPTED 2026-09-24, rev 0.5. Delivery in progress.** M1 is delivered, and M2 is delivered except
> K3b and K3c (§16 records what shipped, and where delivery departed from this text).
>
> **Rev 0.4 status, kept for the record:** proposed, not adopted. Rev 0.2 restructured rev 0.1 after an external design
> review, which found that several guarantees promised more than the design established. The reviewer accepted rev
> 0.2's architecture and asked for six bounded amendments, which rev 0.3 makes. Rev 0.4 makes three small corrections the
> reviewer asked to accompany adoption. §15 records every finding from all three rounds and where it is addressed.
>
> The plan implements the mitigations that [`docs/threat-model.md`](../threat-model.md) §5 Boundary H (revision 3
> draft) proposes against a **colluding population of admitted members**. It follows the contracts axis's posture
> ([`v3-contracts-axis.md`](v3-contracts-axis.md) §1.3) and cross-cutting rules (§9) without restating them. Code
> claims were checked against `c2ad49a`.

---

## 1. Why, and what done means

Each boundary in revisions 1 and 2 models one adversary. Boundary H models many admitted members, each within its
own authorisation, whose concerted behaviour is the attack. They can manufacture apparent support, overwhelm a
provider, or conceal changes to their own history. An agent fleet whose agents are full members is H's natural
deployment. The motivating case is the 2026 OpenAI–Hugging Face incident.

The plan delivers three things, and each is claimed separately:

1. **Trustworthy evidence counting** (§6): support and challenge are counted by authenticated issuer and declared
   control, against evidence that is currently valid, within bounded resources.
2. **Portable authority and defensible audit proofs** (§7): a reader can verify who appointed whom, whether they were
   entitled to, and whether the appointment is current. An accusation of rewritten history carries the accused
   node's own signatures.
3. **A genuinely enforced confinement profile** (§8): agents cannot reach the network except through an enforcement
   point, as shown in a real deployment and not only in simulation.

**"Boundary H in force" means**, for each deliverable separately:
- its negative cases (§10) pass in CI;
- where it claims something about deployment, it has **deployment evidence**, not only replay evidence (§3);
- the threat model moves its items from *Proposed* to *in force*, citing the PR and the gate.

**It never means** a claim about fleets above the size the nightly runner evidences (V1 is still unmet).

---

## 2. What verifying the code found

- **Issuers are not bound to admitted identities.** `IssuerId::new` accepts any non-empty string, and
  `KnowledgeRecord::verify` takes the key from its caller. One member can be many issuers.
- **A mandate is not portable evidence.** `Mandate` is unsigned. Its signed epoch is verified only by the wiki's
  pre-receive hook, which no unit test exercises.
- **Mandates are optional at the enforcement point.** AE1's `ActionEnvelope::mandate` is an
  `Option<MandateBinding>`. The binding records what the enforcement point *established*
  (`Established` / `Refused` / `Unknown`) when it assembled the envelope. It is an assessment made at admission, not
  an expiry that keeps being enforced, and an envelope may carry no mandate at all. Policy can require one
  (`PolicyClause::allow_requiring(…, &["mandate"])`), but nothing requires that policy.
- **Control groups resolve by first match.** `ReaderPolicy::group_of` returns the first configured group that
  contains an issuer. With overlapping groups, independence depends on configuration order.
- **The knowledge layer's remaining PRs are unscheduled.** `store.rs` calls durability, reader authorisation and
  head transport "later PRs". Neither the plan of record's §10.2 nor the ROADMAP lists them.
- **The consensus paths are under repair.** #374 fixed empty-roster authority and unbound votes. The S12
  leader-election intermittency is open and unexplained (wiki security page, 2026-09-24). Anything in H that relies
  on exclusive authority inherits that state (§5).
- **The audit sink already helps.** The WS-C `AuditSink` keeps original bytes wherever an operator attaches one.

---

## 3. The distinctions this plan keeps

Rev 0.1 blurred the first nine; rev 0.3 added the last three. Every item below states which side of each distinction it is on.

1. **A signature proves who issued a statement.** It does not prove the issuer was entitled to issue it, or that no
   conflicting statement exists.
2. **An authentic historical record is not current evidence.** Three questions are separate: is the record
   authentic? Is its issuer currently authorised? Is it current, unretracted support for *this* decision?
3. **Control dependence is not evidential lineage.** Issuers under one operator are dependent. Two independent
   organisations repeating one underlying report share an *origin*. Grouping issuers (H5) handles the first only.
4. **A witness's assertion is not proof.** An equivocation proof needs two incompatible statements, each signed by
   the accused.
5. **Stopping new admissions is not stopping admitted work.** They are measured separately.
6. **Expiry is not revocation.** Expiry can be decided locally, subject to a clock bound. Revocation needs delivery,
   or a freshness rule that fails closed.
7. **A network block is not a gateway decision.** A connection the network blocks never reaches the gateway, so
   the gateway cannot record it.
8. **Replay evidence is not deployment evidence.** Deterministic simulation can show logic. It cannot show that a
   network is confined.
9. **Refusing to decide is not refusing to bear the cost.** A threshold that stops a challenge storm deciding a
   verdict does nothing about the storm's cost in memory, storage and verification.
10. **Losing freshness never creates independence.** A stale relationship is still a relationship (rev 0.3).
11. **Silence is not evidence.** Receiving no revocation does not show that none occurred. A higher sequence number
    does not show that a history continues from the one already held (rev 0.3).
12. **A signature establishes attribution; trust policy establishes decisiveness** (rev 0.3).

---

## 4. Posture: what kind of mechanism each item is

No item touches signal propagation or KV replication. Prevention appears only in the axis's three shapes.

| Item | Deliverable | Kind | Prevention shape, if any |
|---|---|---|---|
| P1 issuer binding | 1 | Honesty fix | none |
| K1–K3 knowledge validity, heads, storage | 1 | Item 3's deferred PRs, with validity rules | none |
| H2 per-issuer support | 1 | Honesty fix | none |
| H5 cohort lifecycle | 1 | Operator statement read by readers | none |
| H1 challenges and ingestion bounds | 1 | Reader policy plus resource bounds | (i) caller-requested bounds |
| P2 signed mandate grant | 2 | Scoped attestation with entitlement and supersession | none |
| A1 authority at execution | 2 | Enforcement at the resource's effect boundary | (ii) enforced at the trusted resource |
| H3 advertisement bound to authority | 2 | Reader-side filter plus an observation | none |
| H4 source-signed audit checkpoints | 2 | Detection with attributable proofs | none |
| H7 confined-fleet profile | 3 | Deployment architecture plus a self-report | (iii) opt-in profile |
| H6 cohort budgets | 3 | Provider admission control | (ii) enforced at the trusted resource |

---

## 5. Milestones and the consensus boundary

Milestones are defined by dependency, not by release number. Each ships in whichever release follows its gate.

| Milestone | Contents | Depends on | Gate |
|---|---|---|---|
| **M1** | H2, plus `#[non_exhaustive]` on `Verdict` and `RejectionReason` | nothing | H2's negative case |
| **M2 evidence counting** | P1 → K1–K3 → H5 → H1 | M1 | Deliverable 1's negative cases |
| **M3 authority** | P2 → A1 → H3 | P1; **the consensus acceptance gate** for anything exclusive | Deliverable 2's authority cases |
| **M4 audit proofs** | H4 | P1 | Deliverable 2's audit cases |
| **M5 confinement** | H7 ADR → reference deployment → deployment test → telemetry adapter; H6 | H5 (for H6) | Deliverable 3's cases, *with deployment evidence* |
| **M6 demonstrated** | D | M2–M5 | §9, each step with its evidence kind |

M3 and M4 can run in parallel, and M5's H7 can start at once.

**The consensus dependency boundary.** Boundary H must not inherit exclusive-authority guarantees from the
election and lock paths while they are under repair.
- **Independent of consensus:** P1, H2, K1–K3, H5, H1, H4 and H6's local admission. These proceed now.
- **Dependent:** wherever a mandate is established through a leased consensus slot, an epoch changes by election,
  or a role claims exclusivity. Until the gate passes, P2 claims **issuance and configured entitlement only**, and no
  H claim rests on "at most one holder".
- **The consensus acceptance gate** is #374's negative cases green, the S12 intermittency explained and fixed, and
  item 5's partition table re-run over the repaired paths. The gate is owned by the consensus work, not by this plan.

---

## 6. Deliverable 1 — Trustworthy evidence counting

### P1 — Bind issuers to identities, with two admissible verification paths (M)

- **Member path.** The issuer is `IssuerId::for_node(&NodeId)`. The key comes from the reader's `sys/identity`
  view: retained keys minus the revocations this reader has received and validated (threat model §6).
- **Configured-external path.** The issuer is in the reader's `trusted_external_issuers`, with its key or keys
  configured. This is for operators, auditors and outside observers. They are trusted by configuration, never by
  default.
- **Anything else is unverifiable.** It is refused and counted, never silently dropped.
- **Rule:** one issuer per member, with many streams under it.
- **Precondition, stated.** Member-path strength rests on `require_identity_proofs`, which is off by default. H7's
  profile turns it on.
- **Gate.**
  - A record claiming member B's issuer but signed by member A fails.
  - A configured external issuer verifies. An unconfigured one is refused.
  - A record signed by a since-revoked key keeps its historical attribution and fails for present eligibility (K1b).

### K1–K3 — Validity, heads and storage (L)

The knowledge layer's deferred PRs, with the rules that make them safe.

**K1, authenticity on put (S).** `put` verifies through one of P1's two paths and records *which* key and path it
used. That settles authenticity, which is historical and does not change. `put` returns a `Result`, and refusals are
counted.

**K1b, present eligibility at resolution (M).** `classify` re-derives, for every record, at read time:
- the issuer is currently authorised (key not revoked in this reader's view; a member is still admitted);
- the record is not retracted or superseded (through the `DependencyIndex`), and no basis it depends on has been
  withdrawn;
- it is within `max_evidence_age_ms` and was judged under the current policy revision.

Eligibility is never cached as "eligible". Verdict reasons name the layer that excluded a record, so "authentic but
no longer eligible" is visible.

**K2, signed heads with rollback and fork rules (S–M).**
- A head carries `(issuer, stream, seq, prev_head_digest, record_digest)`, signed by the issuer. `seq` is
  monotonic per stream.
- The reader retains, per stream, a **checkpoint**: the latest head it has verified *by ancestry*. Checkpoints are
  **durable across restart**. A reader that forgot them on restart would lose its rollback protection.
- A head with a higher `seq` replaces the checkpoint **only if its ancestry is verified**: an unbroken chain of
  signed heads, linked by `prev_head_digest`, from the retained checkpoint to the new head. A higher `seq` alone
  never authorises replacement.
  - **Verified extension:** advance the checkpoint.
  - **Missing ancestry** (the intermediate heads cannot be obtained): keep the checkpoint, and report
    `ContinuityUnavailable`. That is insufficient evidence, not acceptance.
  - **Proven incompatible ancestry** (the new head's chain diverges before the checkpoint): a **fork**. Both are
    retained and reported as `ForkedStream`.
- A valid head with a lower `seq` is a **rollback**: it is ignored and reported as `StaleHead`.
- Two different valid heads at the same `seq` are also a fork, handled the same way, preserving equivocation rather
  than resolving it.
- A head whose body cannot be fetched is `BodyUnavailable`, which counts as insufficient evidence and never as
  absence.

**K3, durable store, head transport and body authorisation (M–L).**
- Records live in a durable, fsynced store, following the evidence journal's pattern.
- Heads gossip under the reserved `knowledge/head/{issuer}/{stream}`.
- Bodies sit behind Boundary F's opaque-address rule.
- Ingestion is bounded per issuer and per cohort (H1).

**Gates.**
- A stale head arriving after a newer one is ignored and reported.
- Forked heads at the same `seq` are both retained and reported.
- A reader holding head 10 is shown head 12 from a branch that diverged at 9. The result is `ForkedStream`, and the
  checkpoint is not replaced.
- Head 12 with heads 11 and 12 unobtainable gives `ContinuityUnavailable`, and the checkpoint stays at 10.
- After a reader restarts, a head older than its pre-restart checkpoint is still refused.
- Retracted support stops counting at the next resolution.
- A revoked issuer's support stops counting for present decisions but stays in history.
- An unavailable body gives `InsufficientEvidence` with its reason.
- Boundary F's three negative cases pass over the durable store with transport on.

### H2 — Count support per issuer (S; milestone M1)

- **What.** `min_supporting` counts distinct issuers.
- **Why it is safe.** It can only make acceptance harder.
- **Compatibility.** The meaning of `Verdict::Accepted.supporting` changes, and the upgrade note says so.
- **Gate.** One issuer with five supporting records and `min_supporting = 2` gives `InsufficientEvidence`.

### H5 — Cohorts: a complete lifecycle, resolved without depending on order (M)

H5 addresses **control dependence** only (§3, distinction 3).

**The declaration.** `CohortDeclaration { cohort_id, operator, members (by member identity, not key), seq,
valid_from_ms, valid_until_ms }`, signed by the operator. It is issued through P1's configured-external path. A
higher `seq` from the same operator for the same cohort supersedes a lower one.

**Grouping is resolved as connected components, not first match.** A reader builds its independence groups from:
- every current declaration from the operators in its `cohort_sources`;
- plus its own configured `control_groups`.

Any two issuers placed together by any of these land in the same group, transitively. The result does not depend on
order, and it replaces `group_of`'s first-match behaviour. This is a behaviour change for readers whose configured
groups overlap, and the upgrade note says so.

**The conservative rule: when in doubt, merge, never split.** Merging reduces counted independence; splitting would
invent it.

| Case | Handling |
|---|---|
| Two trusted operators declare conflicting membership | Union: the member is in both, so the cohorts merge for this reader |
| Overlapping cohorts | Merged, as connected components |
| Removal | Takes effect through a superseding declaration. Evidence issued *before* the removal still counts with the old cohort |
| Reassignment from A to B | During any overlap in validity windows the member is in both, so A and B merge for that window |
| Key rotation | No effect: membership is by member identity |
| Declaration expired or stale in a partition | **Known dependence is retained.** Losing freshness never splits a relationship the reader already knows (see below) |
| Agent admitted before its declaration arrives | Falls under the `undeclared` rule |

**Dependence is sticky.** Once a reader has verified a declaration placing two issuers together, that relationship
ends only through a positive, superseding declaration from the same operator. Expiry, staleness or loss of contact
never ends it.
- The reason: without this rule, if A and B were known members of one cohort, and A's declaration expired while B's
  remained current, A would drop into the undeclared group and B would stay in the declared one. Their support would
  then count as two independent groups. `OneGroup` combines unknown members with *each other*; it does not preserve
  their known dependence on a declared cohort.
- A validity window therefore marks a declaration as **stale**, which is reported, not **void**. A stale declaration
  still merges the groups it names.
- A reader that prefers not to rely on stale relationships may set `stale = Exclude`. Evidence from issuers whose
  only placement is stale is then excluded until the relationship is refreshed or positively resolved. It never falls
  back to looking independent.

**Historical grouping.** Evidence is grouped by the union of the grouping in force when it was issued and the
grouping the reader holds now. A removal therefore cannot make earlier evidence look more independent.

The `undeclared` rule covers issuers that have never been placed: `OwnGroup` (today's behaviour, the default),
`OneGroup` (all undeclared issuers together) or `Excluded`. **The confined profile requires `OneGroup` or
`Excluded`.**

**Gate.**
- Permuting the reader's configuration and the arrival order of declarations gives identical verdicts.
- Overlapping cohorts merge.
- A superseded declaration stops applying to new evidence but not to evidence issued under it.
- **The expiry case, exactly:** A and B are declared in one cohort. A's declaration expires while B's stays
  current, and A and B then both support a release. The result is one independent group, not two. The same holds
  across a partition in which no refresh arrives.
- With `stale = Exclude`, the same scenario excludes A's support and reports why.
- Evidence issued before a removal keeps its historical grouping.
- An agent that arrives before any declaration falls to `undeclared`.
- A declaration from an untrusted operator is ignored.

### H1 — Challenge admission, substantiated facts and bounded cost (M)

**The threshold.** `ReaderPolicy::min_challenge_groups` has a default of 1, which is today's behaviour. Challenges
are counted by H5's groups.
- At or above the threshold, the outcome is today's: `Rejected`, or `Conflicted` when support exists.
- Below it, the verdict carries `unadmitted_challenges { records, groups, sample }`, so the storm is visible
  without deciding the outcome.

**Two kinds of challenge can bypass the threshold. They are different, and they are kept apart.**

A signature establishes *who* said something. Trust policy decides whether it is *decisive*.

1. **Mechanically verified invalidation.** The challenge cites a statement that the reader can verify decides the
   question, with no judgement involved. Examples: the provider's own signed retraction of the release, or a
   revocation signed by an authority P2 shows is entitled for that scope. These decide regardless of the threshold,
   because the reader checks them, not trusts them.
2. **A trusted observer's assessment.** The challenge cites an observation by an issuer in the reader's
   `decisive_sources`. It decides because this reader's policy says that observer's judgement is decisive. That is a
   policy choice, recorded as such in the verdict's reasons.

**Other evidence-bearing challenges are not dismissed.** An honest challenger outside `decisive_sources` can still
hold substantial evidence. A below-threshold challenge that cites verifiable records is reported separately as
`evidenced_unadmitted`, apart from bare challenges, so a person can see it and act on it. It does not decide the
verdict alone, and the verdict says so. Raising the threshold removes the automatic blocking effect of challenges
that neither establish mechanically verified invalidation nor come from a policy-designated decisive source. Their
evidence remains visible. The reader chooses that trade.

**Bounded cost.** A threshold does not stop resource exhaustion (§3, distinction 9), so every stage is bounded:
- **ingestion:** a quota of records per issuer and per cohort per window, refusals counted (K3);
- **storage:** a retention cap per issuer and stream;
- **verification:** a work budget per resolution, with `InsufficientEvidence { reason: BudgetExhausted }` when
  it runs out, never a silent partial count;
- **diagnostics:** `sample` holds at most *k* issuers in deterministic order, and counts are totals.

**Gate.**
- A 1,000-challenge cohort storm below the threshold does not decide the verdict, and is reported with a bounded
  sample.
- A mechanically verified retraction rejects at a threshold of 2.
- A trusted observer's assessment rejects at a threshold of 2, and the reason names the policy.
- An evidence-citing challenge from outside `decisive_sources` is reported as `evidenced_unadmitted` and does not
  decide the verdict.
- A 100,000-record storm stays within the declared memory, time and verification budgets. **Measured, not
  asserted.**

---

## 7. Deliverable 2 — Portable authority and defensible audit proofs

### P2 — Signed mandate grants: issuance, entitlement, currency and possession (M)

- **Issuance.** A `MandateGrant` is a §6 scoped attestation over the mandate's canonical bytes, signed by
  `established_by`, and carrying no transferable credential.
- **Entitlement.** A reader accepts a grant only if the signer is entitled for that scope. Until the consensus gate
  passes, entitlement comes from a configured `scope → authorities` table. After it, entitlement may also come from
  an authority's own grant, chained to a configured root. A signature alone never establishes entitlement.
- **Currency.** A reader retains the highest epoch it has verified per scope.
  - A grant with a lower epoch is `Superseded`.
  - Two different grants with the same scope and epoch are `ConflictingAppointments`. Neither backs an exclusive
    operation, and the conflict is reported.
- **Possession.** A grant is presented together with the holder's signature over the digest of the specific
  request or advertisement. A grant presented by anyone else is refused.
- **Gate (as the review required).**
  - A grant with an altered operation list fails.
  - An expired grant fails.
  - An authentic grant signed by an authority **not entitled** for that scope fails.
  - A grant presented by the **wrong holder** fails.
  - A **superseded** grant fails.
  - Conflicting equal-epoch grants back nothing exclusive and are reported.

### A1 — Authority at execution (M; replaces rev 0.1's "non-renewal halts action")

Under the confined profile, the contract is:

1. **Every protected operation requires an established mandate.** The profile installs policy in which every clause
   for a protected operation uses `allow_requiring(…, &["mandate"])`. An envelope with `mandate: None` for a
   protected operation is denied.
2. **Missing or unverifiable authority prevents execution.** `MandateState::Unknown` is denied, and so is
   `Refused`.
3. **Validity is checked at the execution boundary.** The check runs where the effect happens (AE2's
   effect-boundary check), not only at admission. An envelope's `not_after_ms` may not exceed its mandate's
   `valid_until_ms`. **A1 inherits AE2's tiers:** the execution-boundary check is only as strong as the protected
   resource's own atomicity. Each protected operation names its tier, and an operation whose resource cannot say
   which tier it is in is not covered by A1.
4. **Queued, retried and delegated work keeps the requirement.** Work is re-evaluated at dequeue and on every retry.
   Delegated work carries the original binding and cannot extend it: a child's `not_after_ms` is at most its
   parent's.
5. **Long-running work declares a continuation policy.** It is either `ReauthorizeAt { interval }`, which re-checks
   at checkpoints and cancels on failure, or `RunToCompletion { max_duration }`, which may finish without further
   authorisation. `max_duration` must be **enforced by the resource** (a hard limit whose termination the resource
   can confirm); a declared but unenforced duration is not a bound. The profile's default is `ReauthorizeAt`.

**Two measurements, reported separately:**
- **T_admit:** time from expiry until no new admissions (expected: the clock bound).
- **T_drain:** time from expiry until admitted work has **confirmably stopped**. Its bound depends on the
  continuation policy. In both, *s* is the clock bound defined below, since a reader may detect expiry up to *s*
  late.
  - **`ReauthorizeAt { interval }`:** T_drain ≤ *s* + `interval` (the wait for the next checkpoint) +
    `cancellation_latency` + `confirmation_latency`.
  - **`RunToCompletion { max_duration }`:** T_drain ≤ *s* + the **remaining permitted duration** at expiry (at most
    `max_duration` minus the time already run) + `confirmation_latency`. Nothing is cancelled: the work was
    permitted to finish, and the bound is how long that may take.
  - **An operation class without a demonstrable bound cannot satisfy the bounded-stop claim.** That covers a class
    with no declared policy, an undeclared or unmeasured latency, an unenforced `max_duration`, or a resource that
    cannot confirm a stop. Such a class is reported as `Unbounded` and excluded from A1's T_drain guarantee. It is
    never assumed to be bounded.
  - Reaching a checkpoint and *requesting* cancellation does not prove that work, or its external effects, have
    stopped.
  - Each protected operation class declares a `cancellation_latency` and how a stop is confirmed.
  - The measurement reports three states separately: cancellation requested, acknowledged, and confirmed stopped.
  - Effects the resource cannot confirm are reported as *unconfirmed*, never as stopped.

**Expiry and revocation are separate.**
- *Expiry* is decided locally from `valid_until_ms` under the clock model below. It works while disconnected.
- *Revocation* needs delivery. Present authorisation requires a revocation view no older than a freshness bound
  *F*. A staler view makes the state `Unknown`, which is denied under the profile. **The cost, stated:** under a
  partition longer than *F*, protected work stops. That is the intended failure direction.

**The clock model: one definition, used everywhere in A1.**
- ***s* bounds each clock's deviation from real time.** For every authority and every reader, |C(t) − t| ≤ *s* at
  all times *t*. C is HLC physical time, disciplined by the deployment's time synchronisation. The A1 ADR names the
  synchronisation source and how *s* is monitored, and a node that cannot confirm its synchronisation reports
  `Unknown` for every time-dependent authority check.
- **Consequence:** any two clocks differ by at most 2*s*. Every bound below follows from that.
- **Expiry.** A reader admits while C_r(now) ≤ `valid_until_ms`. Admissions stop no later than *s* after the real
  expiry instant, and may stop up to *s* early.

**How freshness is established: authority-signed revocation checkpoints.**
- Each establishing authority (the P2 entitlement table names them per scope) issues a signed
  `RevocationCheckpoint { authority, scope, seq, issued_at_ms, revoked }` at least once per interval *I* < *F*. It is
  issued **even when nothing has been revoked**. An empty checkpoint is a positive statement that nothing was
  revoked as of `issued_at_ms`.
- **Freshness is measured from `issued_at_ms`, not from when the reader received it.** Receiving an old checkpoint
  again never refreshes its age.
- **Replay protection.** The reader retains, durably, the highest `seq` it has verified per `(authority, scope)`. A
  checkpoint with a lower or equal `seq` adds nothing and cannot refresh freshness.
- **Silence is not evidence.** Receiving no revocations establishes nothing. Only a checkpoint no older than *F*
  does.
- **The freshness predicate, exactly.** Let *a* = `issued_at_ms` (the authority's clock) and *r* = C_r(now) (the
  reader's clock). The reader's retained checkpoint for `(authority, scope)` is **fresh** if and only if its
  signature verified, its `seq` is the highest the reader has retained, and

  &nbsp;&nbsp;&nbsp;&nbsp;(*a* − *r*) ≤ 2*s* **and** (*r* − *a*) ≤ *F* − 2*s*.

  - **Safety:** a checkpoint accepted as fresh has a real age of at most *F*. The measured age *r* − *a* understates
    the real age by at most 2*s*, so real age ≤ (*r* − *a*) + 2*s* ≤ *F*.
  - **Liveness:** a checkpoint with a real age of at most *F* − 4*s* is always accepted. The measured age overstates
    the real age by at most 2*s*, so *r* − *a* ≤ *F* − 2*s*.
  - **Future-dated:** *a* − *r* > 2*s* cannot happen for an honest checkpoint under the model. It is refused and
    reported as a clock fault or a forgery attempt.
- **Required parameters.** *F* > 4*s*, so the guaranteed-acceptance window *F* − 4*s* is positive (*F* > 2*s*
  alone makes the predicate satisfiable but guarantees nothing). The checkpoint interval must fit inside that
  window: *I* + *D* ≤ *F* − 4*s*, where *D* is the maximum delivery delay the deployment tolerates before failing
  closed. Otherwise a connected reader can be denied spuriously at the clock extremes. The profile refuses to start
  with parameters that break either inequality.
- **Binding.** A checkpoint covers only its own authority and scope. A fresh checkpoint for scope X says nothing
  about scope Y.
- This is independent of consensus: it needs only the authority's key and the reader's retained `seq`.

**Gate.**
- Admissions stop within *s* of expiry.
- A queued item dequeued after expiry is refused.
- A retry after expiry is refused.
- Delegated work cannot outlive its parent.
- A `ReauthorizeAt` task is cancelled at the first checkpoint after expiry.
- A stale revocation view is denied.
- **A replayed old checkpoint** (valid signature, lower `seq`, or an old `issued_at_ms` delivered again) does not
  refresh freshness. Authority is denied as soon as the freshness predicate fails for the last genuine checkpoint.
- **A partition with no new revocation messages:** once the predicate fails, the state becomes `Unknown` and
  protected operations are denied. The lack of messages is never read as "nothing revoked".
- **Both clock extremes**, on the replay clock seam:
  - *Authority +s, reader −s* (age understated by 2*s*): a checkpoint of real age *F* is accepted, and one of real
    age *F* + 1 ms is refused. Safety holds at the edge.
  - *Authority −s, reader +s* (age overstated by 2*s*): a checkpoint of real age *F* − 4*s* is accepted. Liveness
    holds at the edge.
  - *Future-dated:* at *a* − *r* = 2*s* it is accepted, and at 2*s* + 1 ms it is refused and reported.
  - *Expiry:* with the reader at −*s*, admissions end by real expiry + *s*. With the reader at +*s*, they end no
    earlier than real expiry − *s*.
- The profile refuses parameters with *F* ≤ 4*s* or *I* + *D* > *F* − 4*s*.
- A checkpoint for scope X does not refresh scope Y.
- A reader that restarts keeps its highest retained `seq` and still refuses a replayed checkpoint.
- A `RunToCompletion` task stops, confirmed, within *s* + its remaining permitted duration + its confirmation
  latency.
- An operation class with no demonstrable bound is reported `Unbounded` and excluded from the T_drain claim.
- T_admit is measured, and so is T_drain per continuation policy, with requested, acknowledged and confirmed stop
  reported separately.

### H3 — Advertisement bound to authority (M; needs P2)

A reader can protect a namespace. A capability in it resolves only if its advertiser presents a P2 grant that
verifies, is entitled, is current and permits `serve:{namespace}/{name}`, together with proof of possession by the
advertiser.
- A filtered capability is reported through a counter, an audit record and, optionally, a knowledge `Observation`.
- **Honest limit, unchanged:** colluders resolve with their own policy. H3 protects honest readers and makes a
  laundered power visible; it does not stop a cohort using the power among its own members.
- **Gate.**
  - An unbacked capability is filtered and observed; a backed one resolves.
  - An expired, superseded, non-entitled or wrong-holder grant is filtered.
  - A grant for `serve:a/x` does not back `a/y`.

### H4 — Source-signed audit checkpoints and attributable equivocation (M)

Rev 0.1's witness record proved only what the witness asserted. A malicious witness could invent a hash and accuse
an honest node. Rev 0.2 makes every accusation rest on the accused node's own signatures.

- **Checkpoints already exist and are reused** (corrected 2026-09-24; earlier revisions proposed a new record
  type). `AuditCheckpoint { node_id, checkpoint_seq, prev_hash, hlc }` is signed by the node's identity key and kept
  under `sys/audit-checkpoint/{node}/{seq}`, a namespace separate from the trail so that pruning never touches it.
  H4 adds no record type. It adds **retention by other members** and the **comparison** that turns two conflicting
  source-signed statements into a proof.
- **Retention.** Members retain the source-signed checkpoints they receive, and optionally full source-signed
  records, within a bounded budget. The witness's own record only *refers* to what it holds. The proof is the
  source's signature, never the witness's.
- **Four outcomes:**

| Outcome | Meaning |
|---|---|
| `Equivocation { proof }` | Two source-signed statements (checkpoints or records) for the same `(stream, seq)` with different hashes. Attributable to the source. |
| `Consistent` | Every retained source statement matches the current stream. |
| `HistoryUnavailable` | The statements needed to compare are not held by this verifier. |
| `InsufficientEvidence` | No retained source-signed statement covers this position. |

- **The guarantee, exactly.** A rewrite that conflicts with a retained source-signed record or checkpoint is
  detectable once the verifier obtains the conflicting signed evidence. **Changes outside retained coverage remain
  unproven.**
  - A checkpoint at sequence 100 commits, through the hash chain, to records up to 100. Rewriting any of them
    conflicts with it.
  - A checkpoint at 100 **cannot** detect a rewrite of records 101–110 that no one retained. The original and
    rewritten versions both extend the same checkpoint.
  - The unwitnessed suffix after the newest retained checkpoint is therefore always unproven. Its length is reported
    as part of coverage, and the checkpoint interval bounds it only where checkpoints are actually retained.
- The WS-C sink, where attached, holds the original bytes, which extends coverage to everything it has mirrored.
  Checkpoints hold only hashes.
- **Honest limit.** Checkpoints do nothing about acts that were never recorded. That is H7's job.
- **Gate.**
  - A malicious witness fabricates a hash, and the result is **not** `Equivocation` (it is `InsufficientEvidence`,
    and the unsupported assertion is reported).
  - **Rewriting covered history produces a proof.** A checkpoint at 100 is retained, and a record at or below 100 is
    rewritten. The result is `Equivocation` carrying two source signatures (the checkpoint, and the new chain's
    statement for that position).
  - **Rewriting an unwitnessed suffix produces insufficient evidence.** A checkpoint at 100 is retained, and records
    101–110 are rewritten with none of them retained. The result is `InsufficientEvidence` for 101–110, with the
    uncovered range reported. It is **never** `Consistent`, and never `Equivocation`.

---

## 8. Deliverable 3 — A genuinely enforced confinement profile

### H7 — The confined-fleet profile (L; needs its own ADR)

Rev 0.1's in-pod sidecar could not deliver the isolation it promised. Containers in one Kubernetes pod share a
network namespace, so ordinary `NetworkPolicy` cannot give the agent container one egress policy and its gateway
sidecar another. Whatever outbound access the gateway has, the agent has too.

**Reference deployment: separate pods.**
- Agent and gateway run in **separate pods**, and therefore separate network namespaces.
- `NetworkPolicy` lets agent pods reach only the gateway `Service`, plus DNS if it is declared. The gateway pods'
  egress follows `EgressPolicy.allow_hosts`, failing closed.
- The CNI must enforce `NetworkPolicy`. The ADR names the ones tested; the profile is invalid on a CNI that does not
  enforce it.

**The surfaces the ADR must specify:**
- **Key custody.** The member key is mounted only in the gateway pod, never in an agent pod.
- **Administrative endpoints.** The gateway's admin, metrics and debug routes are unreachable from agent pods.
- **Cloud credentials.** Agent pods run under a service account with no cloud IAM binding.
- **Metadata access.** Instance metadata (169.254.169.254 and its equivalents) is blocked from agent pods at the
  network layer. That was one of the incident's routes.
- **Kubernetes API.** `automountServiceAccountToken: false` for agent pods, and no RBAC grants.

**Alternatives.** An in-pod mechanism that separates processes (per-process egress enforcement, for example) is not
the reference. If proposed, it needs its own enforcement argument and its own tests.

**Two different evidence records (§3, distinction 7):**

| Record | What produces it | What it shows |
|---|---|---|
| **Network enforcement observation** | An identified telemetry source (the enforcing CNI's flow logs, or cloud VPC flow logs), ingested by a named adapter that emits knowledge `Observation`s | The confinement mechanism blocked a direct connection |
| **Gateway decision** | The AE evidence journal | A request reached the gateway and was permitted or refused |

The adapter is a deliverable. **Without it, the claim is "direct egress is blocked", and its observability limit is
stated:** blocked attempts are not observed.

**The node's self-report.** `ConfinementReport` states what the node can know:
- allow-hosts is set;
- identity proofs are required;
- the audit sink is attached;
- an evaluator and a journal are attached;
- the undeclared rule is conservative.

It reports network confinement as `Unverified`, because a node cannot see its own network policy.

**Evidence.**
- **Deployment evidence:** a test in a real Kubernetes cluster with an enforcing CNI, run in CI (for example, kind
  with such a CNI). It shows that an agent pod cannot reach an external host, the metadata endpoint, the Kubernetes
  API or the gateway's admin routes. It also shows that a permitted request through the gateway is recorded.
- A Docker test remains useful for Docker deployments. **It does not validate the Kubernetes profile**, and the
  plan does not cite it as if it did.

### H6 — Cohort budgets from authenticated membership (M; needs H5)

- **What.** An RAII admission slot per cohort at the provider's admission point, mirroring
  `max_in_flight_per_partner`. The refusal is `AtCapacity`.
- **Membership.** It is derived from the **authenticated caller principal**, resolved through the provider's own
  H5 view of trusted declarations. It is **never taken from a label the caller supplies**. Undeclared callers share
  one `undeclared` budget.
- **Stated limits (as in #365).** The cap is per provider instance (N instances means N × cap), and it bounds
  concurrency, not rate.
- **Gate.**
  - Fifty cohort members, each within its own cap, together exceed the cohort cap, and the excess is refused.
  - A caller-supplied cohort label is ignored.
  - Another cohort is unaffected.
  - Refusals appear in the rights ledger.

---

## 9. The decisive demonstration (D)

A gallery entry, `examples/boundary_h_fleet.rs`, plus a `mycelium-sim` replay scenario, plus the H7 deployment test.
Each step states its claim, its assumptions, where it is enforced, and what the evidence actually is.

| # | Claim | Assumptions | Enforced at | Evidence source | Kind |
|---|---|---|---|---|---|
| 1 | Setup: a declared cohort under the confined profile, plus an observer | A trusted operator's declaration; an enforcing CNI | — | `ConfinementReport` and the deployment test | Replay **and** deployment |
| 2 | A laundered power does not resolve for the observer | Observer protects the namespace | H3, the observer's resolve path | Observation plus counter | Replay |
| 3 | Mutual support counts once | Observer trusts the declaration | H5 in `classify` | Verdict reasons | Replay |
| 4 | A challenge storm is visible and does not decide, within bounds | Threshold of 2 | H1 plus ingestion bounds | Verdict, and measured resource use | Replay, measured |
| 5 | A rewritten history is proven | A retained checkpoint covers the rewritten position; the unwitnessed suffix stays unproven | H4 | `Equivocation` with two source signatures | Replay |
| 6 | Cohort volume is capped | Provider sets a cohort budget | H6 at the provider | `AtCapacity` in the rights ledger | Replay |
| 7 | Direct egress is blocked | Separate pods; enforcing CNI | The network layer, not Mycelium | The flow-log adapter, if built; otherwise "blocked, unobserved" | **Deployment only** |
| 8 | "Which agent used this credential, under which mandate?" is a lookup | Journal attached; grants retained | — | AE journal plus P2 grants | Replay |
| 9 | Expiry stops new admissions, and then admitted work | A1 profile; clock skew *s* | A1 at the execution boundary | T_admit and T_drain, measured separately | Logic by replay; timing by deployment |

The entry states its N, and it makes no claim above V1's evidenced ceiling.

---

## 10. The negative cases

Each is a sentence the threat model can cite once green. **R** means replayed in CI, **D** means deployment test.

1. One member cannot be many issuers. A configured external issuer still verifies. (R)
2. Authentic but revoked, retracted or superseded evidence does not count for a present decision. (R)
3. A rolled-back head is ignored. A higher head that does not verifiably extend the checkpoint never replaces it: a
   divergent one is a fork, and one without obtainable ancestry leaves continuity unavailable. Checkpoints survive
   restart. (R)
4. One issuer cannot meet `min_supporting` alone. (R)
5. Independence does not depend on configuration or arrival order, and overlapping cohorts merge. A known dependence
   survives expiry and partition: two members of one cohort never count as two groups because one declaration went
   stale. (R)
6. A cohort cannot decide a verdict below the threshold. A mechanically verified invalidation, or a policy-decisive
   observer, still decides, and an evidence-citing outside challenge is reported. A storm stays within
   its bounds. (R, measured)
7. An authentic grant from a non-entitled authority, a wrong-holder presentation and a superseded grant are all
   refused. (R)
8. A protected operation without an established mandate is refused. Expired authority stops admission within *s*
   and admitted work is confirmably stopped within its declared bound. A stale revocation view is refused, a replayed
   checkpoint never refreshes freshness, and silence is never read as no revocation. (R; timing D)
9. A fabricated witness hash never yields `Equivocation`. A rewrite that conflicts with retained source-signed evidence
   yields one. A rewrite of an unwitnessed suffix yields insufficient evidence, never `Consistent`. (R)
10. A caller-supplied cohort label never selects a budget. (R)
11. Under the reference deployment, an agent pod cannot reach external hosts, instance metadata, the Kubernetes API
    or the gateway's admin routes. (D)

---

## 11. Compatibility, stated explicitly

This programme is **not** entirely additive. The breaking or behaviour-changing steps are:

| Change | Effect on consumers |
|---|---|
| `Verdict`, `RejectionReason` become `#[non_exhaustive]` (M1) | Exhaustive `match` needs a `_` arm, which must fail safe |
| `Verdict::Accepted.supporting` counts issuers (H2) | Same type, new meaning; stated in the upgrade note |
| `ReaderPolicy` gains public fields (H5, H1) | Exhaustive struct literals break; `Default` construction is unaffected |
| Control groups resolve as connected components (H5) | Readers with overlapping configured groups may see lower independence |
| `KnowledgeStore::put` returns `Result` (K1) | Callers must handle refusal |
| H4 reuses `AuditCheckpoint`; the equivocation outcome type is new | Additive |
| Head format gains `seq` and `prev_head_digest` (K2) | Namespace reserved but unused, so low impact; stated anyway |
| Readers persist per-stream head checkpoints and per-authority revocation `seq` (K2, A1) | New durable reader state; a reader without storage cannot claim rollback or replay protection |
| `RevocationCheckpoint` and `MandateGrant` types (A1, P2) | Additive |
| `stale` rule and historical grouping in `ReaderPolicy` (H5) | New fields (same class as above); may lower counted independence |
| The confined profile denies unmandated protected operations (A1) | Opt-in, but enabling it changes behaviour |
| Gateway route for audit proofs (H4) | Additive, with both SDKs in the same PR |

---

## 12. What this plan does not claim

- **Scale** above V1's evidenced ceiling.
- **Colluders among themselves.** Reader-side checks protect honest readers.
- **Collusion versus cooperation.** They remain the same behaviour. Containment is authority design, not detection.
- **Exclusive authority** before the consensus acceptance gate passes.
- **Evidential lineage.** H5 handles control dependence. Two independent organisations repeating one report are not
  detected by grouping issuers.
- **Unobserved blocks.** A network block without the telemetry adapter is enforced but not observed.
- **Deployments outside the reference profile.** An in-pod sidecar, an unenforcing CNI, or a mounted member key
  keeps today's residual.
- **A central halt.** Expiry stops admission locally; revocation reaches only connected readers, or fails closed
  under the freshness rule.

---

## 13. Downstream (other repositories, tracked there)

**NovusLens:**
- Move the Mycelium pin to the release that carries M3.
- Ingest four observation classes as findings: unbacked protected capability, equivocation proof, below-threshold
  challenge storm, and network enforcement observations.
- Model **control dependence** (who operates whom) and **evidential lineage** (which report derives from which)
  as separate relations.

Rev 0.1 said H5's cohort idea "may transfer" to NovusLens's echo-detection gap. That was wrong. Echo is a *lineage*
problem: independent organisations repeating one underlying report. Grouping issuers does not detect it. It needs
`DerivedFrom`-style origin tracking on asserted sources.

**mycelium-private.** The AE exporter carries the cohort id, the `ConfinementReport` digest and the A1 continuation
policy in its signed batches.

---

## 14. Process

- ADRs:
  - **P1 + P2** (identity, entitlement, currency);
  - **K** (validity layers, heads, storage);
  - **H5** (cohort lifecycle);
  - **A1** (authority at execution);
  - **H4** (source-signed checkpoints);
  - **H7** (confinement architecture: its own ADR, as the review required).
- H1, H2, H3 and H6 are argued in their PR descriptions against this plan.
- Each PR carries §9's five parts, updates the wiki page it touches, and adds a dated `.log/` entry.
- The threat model moves an item to *in force* only in the PR that ships its gate. For deliverable 3, that requires
  the deployment test, not the replay.

---

## 15. Review disposition (external design review, 2026-09-24)

| # | Finding | Disposition | Where |
|---|---|---|---|
| 1 | The in-pod sidecar cannot separate the agent's egress from the gateway's (blocking) | Accepted. The reference is now separate pods. Key custody, admin endpoints, cloud credentials, metadata and the Kubernetes API are specified. A Docker test is not cited for Kubernetes | §8 H7 |
| 2 | A blocked connection produces no gateway evidence (blocking) | Accepted. There are two records, network observation and gateway decision; the telemetry adapter is a deliverable; otherwise the limit is stated | §8, §9 step 7 |
| 3 | Witness records prove only the witness's assertion (blocking) | Accepted. Source-signed checkpoints; equivocation needs two source signatures; four outcomes; detection resolution stated | §7 H4 |
| 4 | "Non-renewal halts action" is too broad (blocking) | Accepted. Replaced by A1's five-part contract, T_admit and T_drain measured separately, expiry separated from revocation | §7 A1 |
| 5 | H5 lacks a lifecycle; cohort is not origin | Accepted. Connected components replace first match; merge-never-split fallback; lifecycle table; budgets from authenticated membership; NovusLens claim corrected | §6 H5, §8 H6, §13 |
| 6 | K1–K3 lack present-validity and rollback rules | Accepted. Three validity layers (K1 and K1b), head rollback, fork and unavailable-body rules, two verification paths | §6 P1, K |
| 7 | Consensus dependency boundary | Accepted. The boundary is named, independent work proceeds, P2 limited to configured entitlement until the gate | §5, §7 P2 |
| — | H1 does not stop resource exhaustion; substantiated facts vs popularity | Accepted. Bounds at every stage; decisive sources bypass the threshold | §6 H1 |
| — | H7 needs its own ADR | Accepted | §14 |
| — | Replay evidence is not deployment evidence | Accepted. Every claim is labelled R or D | §3, §9, §10 |
| — | Fixed release numbers; "additive" overstated | Accepted. Dependency milestones; compatibility table | §5, §11 |

**Round 2 (the same reviewer, on rev 0.2).** Rev 0.2's architecture was accepted. Six bounded amendments followed.

| # | Finding | Disposition | Where |
|---|---|---|---|
| R2-1 | H4 overclaimed: a checkpoint cannot detect a rewrite of the unwitnessed suffix after it | Accepted. The guarantee is restated as conflict with retained signed evidence. Tests added for covered history (proof) and an unwitnessed suffix (insufficient evidence) | §7 H4, §10 case 9 |
| R2-2 | Cohort expiry could manufacture independence | Accepted. Dependence is sticky: staleness marks, never voids. Optional `stale = Exclude`. Historical grouping is the union of grouping at issue and now. The exact expiry and partition test is added | §6 H5, §10 case 5 |
| R2-3 | Signed heads need ancestry, not sequence comparison | Accepted. Advancing requires a verified chain from the checkpoint; missing ancestry gives `ContinuityUnavailable`; incompatible ancestry is a fork; checkpoints are durable across restart | §6 K2, §10 case 3 |
| R2-4 | Revocation freshness needs an authoritative mechanism | Accepted. Authority-signed revocation checkpoints, issued even when empty, aged by `issued_at_ms`, protected against replay by retained `seq`, with clock skew stated. Tests for a replayed checkpoint and a silent partition | §7 A1, §10 case 8 |
| R2-5 | "Substantiated facts" conflated verification with trust | Accepted. Mechanically verified invalidation is kept separate from a policy-decisive observer; evidence-citing outside challenges are reported as `evidenced_unadmitted` | §6 H1 |
| R2-6 | T_drain must include cancellation delay | Accepted. The bound includes checkpoint, cancellation latency and confirmation; requested, acknowledged and confirmed are reported separately; unconfirmable effects are reported as unconfirmed | §7 A1 |

**Round 3 (the same reviewer, on rev 0.3).** Three small corrections were asked to accompany adoption.

| # | Finding | Disposition | Where |
|---|---|---|---|
| R3-1 | H1's closing sentence contradicted the `evidenced_unadmitted` rule | Accepted. The reviewer's wording is used: raising the threshold removes the automatic blocking effect of challenges that are neither mechanically verified nor from a decisive source, and their evidence stays visible | §6 H1 |
| R3-2 | `RunToCompletion` needs its own T_drain bound | Accepted. Its bound is *s* + remaining permitted duration + confirmation latency; `max_duration` must be enforced by the resource; a class with no demonstrable bound is `Unbounded` and excluded | §7 A1 |
| R3-3 | The revocation clock rule must be executable | Accepted. *s* bounds each clock's deviation from real time (pairwise at most 2*s*). The exact predicate is given with safety and liveness bounds; *F* > 4*s* and *I* + *D* ≤ *F* − 4*s* are required and enforced at start-up; both clock extremes are tested | §7 A1 |

---

## 16. Delivery record

What has shipped, in merge order, and where delivery departed from the text above. A departure is recorded here
and in the item's ADR, never silently.

| Item | PR | Commit | Gate |
|---|---|---|---|
| H2, per-issuer support | #381 | `a60a3d1` | `resolution::tests` (H2 cases) |
| P1, issuer binding | #384 | `2235bf5` | `issuer::tests`; two-node live gate |
| K1, verify on storage | #387 (replaced #385) | `c156334` | `store::tests::k1` |
| K1b, present eligibility | #386 | `f6eeade` | `resolution::tests::k1b` |
| K2, signed heads, verified ancestry | #389 | `2981da4` | `heads::tests` |
| K3a, durable stores | #390 | `232a32e` | `durable::tests` |
| H5, cohorts | #391 | `895dec0` | `resolution::tests::h5`, `cohort::tests` |
| H1, challenge admission | #392 | `317852b` | `resolution::tests::h1` |
| H7, confined-fleet profile | #393 | `195e0f9` | `agent::confinement::tests`; deployment test (kind + Calico, CI) |
| H6, cohort budgets | #394 | — | `cohort_budget::tests` |
| P2, signed mandate grants | #395 | — | `mandate::grant::tests` |
| H3, advertisement bound to authority | #395 | — | `mandate::protected::tests` |
| A1, authority at execution | #398 | — | `mandate::authority::tests`; `a1_policy_tests` |

**Departures:**
- **K1 is additive, not breaking.** §11 listed `KnowledgeStore::put → Result`. `put` had 72 call sites building
  trusted local records, so it stays and marks records `Unchecked`, and `put_signed` is the verified path. K1b's
  `UncheckedRule` decides whether unchecked records count. It defaults to `Count`, and the confined profile will
  require `Exclude`.
- **K1b found a live defect.** `classify` never consulted retraction, so a withdrawn assessment kept counting. K1b
  fixed it, and the fix only narrows.
- **K2's durability became a contract, and K3 split into K3a, K3b and K3c.** There is no filesystem seam, so K2
  defined `CheckpointStore` and shipped only an in-memory store. K3a made both stores durable over the existing
  node-local journal, with no new filesystem site and no lock. K3b (transport) and K3c (body authorisation, and the
  caps below) remain.
- **H1's mechanically verified invalidation is concrete:** the provider's own current challenge of its release
  (`DisownedByProvider`). The entitled-authority form waits for P2.
- **H1's storage and ingestion caps moved to K3c.** They belong to the store and to transport.
- **H4 reuses `AuditCheckpoint`** (`sys/audit-checkpoint/`), which already existed. That was found while
  implementing P1, and recorded in rev 0.4.
- **Found by tests while building H5.** A union-find fed only by supporters missed a silent member bridging two
  cohorts. The graph now includes every configured and in-force member.

