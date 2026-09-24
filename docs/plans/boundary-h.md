# Boundary H — implementation plan

> **Status: PROPOSED, rev 0.2, 2026-09-24. Not adopted.** Rev 0.2 restructures rev 0.1 after an external design
> review, which found that several guarantees promised more than the design established. §15 records each finding
> and where it is addressed.
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

Rev 0.1 blurred each of these. Every item below states which side of each distinction it is on.

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
- The reader retains the highest verified head per stream as a checkpoint.
- A valid head with a lower `seq` is a **rollback**: it is ignored and reported as `StaleHead`.
- Two different valid heads at the same `seq` are a **fork**: both are retained and reported as `ForkedStream`,
  preserving equivocation rather than resolving it.
- A head whose body cannot be fetched is `BodyUnavailable`, which counts as insufficient evidence and never as
  absence.

**K3, durable store, head transport and body authorisation (M–L).**
- Records live in a durable, fsynced store, following the evidence journal's pattern.
- Heads gossip under the reserved `knowledge/head/{issuer}/{stream}`.
- Bodies sit behind Boundary F's opaque-address rule.
- Ingestion is bounded per issuer and per cohort (H1).

**Gates.**
- A stale head arriving after a newer one is ignored and reported.
- Forked heads are both retained and reported.
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
| Declaration expired or stale in a partition | The member falls under the reader's `undeclared` rule |
| Agent admitted before its declaration arrives | Falls under the `undeclared` rule |

The `undeclared` rule is `OwnGroup` (today's behaviour, the default), `OneGroup` (all undeclared issuers together)
or `Excluded`. **The confined profile requires `OneGroup` or `Excluded`.**

**Gate.**
- Permuting the reader's configuration and the arrival order of declarations gives identical verdicts.
- Overlapping cohorts merge.
- A superseded declaration stops applying to new evidence but not to evidence issued under it.
- A stale declaration, and an agent that arrived early, both fall to `undeclared`.
- A declaration from an untrusted operator is ignored.

### H1 — Challenge admission, substantiated facts and bounded cost (M)

**The threshold.** `ReaderPolicy::min_challenge_groups` has a default of 1, which is today's behaviour. Challenges
are counted by H5's groups.
- At or above the threshold, the outcome is today's: `Rejected`, or `Conflicted` when support exists.
- Below it, the verdict carries `unadmitted_challenges { records, groups, sample }`, so the storm is visible
  without deciding the outcome.

**Substantiated invalidating facts bypass the threshold.** A challenge that cites a verifiable observation from an
issuer in the reader's `decisive_sources` decides regardless of how many groups filed it. Examples of such an
observation: the provider's own retraction, a signed revocation of the release, or an incident observation by a
trusted observer. This keeps a real, isolated fact from being outvoted by a popularity threshold. Raising the
threshold then trades away only *unsubstantiated* single challenges, and the reader chooses that trade.

**Bounded cost.** A threshold does not stop resource exhaustion (§3, distinction 9), so every stage is bounded:
- **ingestion:** a quota of records per issuer and per cohort per window, refusals counted (K3);
- **storage:** a retention cap per issuer and stream;
- **verification:** a work budget per resolution, with `InsufficientEvidence { reason: BudgetExhausted }` when
  it runs out, never a silent partial count;
- **diagnostics:** `sample` holds at most *k* issuers in deterministic order, and counts are totals.

**Gate.**
- A 1,000-challenge cohort storm below the threshold does not decide the verdict, and is reported with a bounded
  sample.
- One substantiated fact from a decisive source rejects at a threshold of 2.
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
   at checkpoints and cancels on failure, or `RunToCompletion { max_duration }`, a stated bound. The profile's
   default is `ReauthorizeAt`.

**Two measurements, reported separately:**
- **T_admit:** time from expiry until no new admissions (expected: the clock bound).
- **T_drain:** time from expiry until no admitted work is still running (bounded by the continuation policies).

**Expiry and revocation are separate.**
- *Expiry* is decided locally from `valid_until_ms` under a stated clock assumption (HLC physical time, maximum
  skew *s*). It works while disconnected.
- *Revocation* needs delivery. Present authorisation requires a revocation view no older than a freshness bound
  *F*. A staler view makes the state `Unknown`, which is denied under the profile. **The cost, stated:** under a
  partition longer than *F*, protected work stops. That is the intended failure direction.

**Gate.**
- Admissions stop within *s* of expiry.
- A queued item dequeued after expiry is refused.
- A retry after expiry is refused.
- Delegated work cannot outlive its parent.
- A `ReauthorizeAt` task is cancelled at the first checkpoint after expiry.
- A stale revocation view is denied.
- T_admit and T_drain are both measured.

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

- **Checkpoints.** Each node periodically seals `AuditAction::Checkpoint { stream, seq, head_hash }`. This is a
  separately signed statement by the node itself, recorded in its own chain.
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

- **What it can detect.** A rewrite is detectable from the first position that a retained checkpoint or record
  covers. The checkpoint interval is the stated resolution, and coverage is reported. The WS-C sink, where attached,
  holds the original bytes; checkpoints hold only hashes.
- **Honest limit.** Checkpoints do nothing about acts that were never recorded. That is H7's job.
- **Gate.**
  - A malicious witness fabricates a hash, and the result is **not** `Equivocation` (it is `InsufficientEvidence`,
    and the unsupported assertion is reported).
  - A rewrite after a retained checkpoint gives `Equivocation` carrying two source signatures.
  - A rewrite before any checkpoint gives `InsufficientEvidence`, **never** `Consistent`.

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
| 5 | A rewritten history is proven | A checkpoint was retained | H4 | `Equivocation` with two source signatures | Replay |
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
3. A rolled-back head is ignored, and a forked stream is reported. (R)
4. One issuer cannot meet `min_supporting` alone. (R)
5. Independence does not depend on configuration or arrival order, and overlapping cohorts merge. (R)
6. A cohort cannot decide a verdict below the threshold. A substantiated fact still decides. A storm stays within
   its bounds. (R, measured)
7. An authentic grant from a non-entitled authority, a wrong-holder presentation and a superseded grant are all
   refused. (R)
8. A protected operation without an established mandate is refused. Expired authority stops admission within *s*
   and admitted work within its continuation bound. A stale revocation view is refused. (R; timing D)
9. A fabricated witness hash never yields `Equivocation`. A real rewrite after a checkpoint always does. (R)
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
| `AuditAction` gains `Checkpoint` (H4) | Exhaustive `match` breaks if the enum is exhaustive |
| Head format gains `seq` and `prev_head_digest` (K2) | Namespace reserved but unused, so low impact; stated anyway |
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
