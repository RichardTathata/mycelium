# Knowledge cohorts and challenge admission (ADR, Boundary H items H5 and H1)

**Status:** H5 **adopted and implemented** 2026-09-24 (`src/knowledge/cohort.rs`, `src/knowledge/resolution.rs`); H1
**adopted and implemented** 2026-09-24 (§4). Plan:
[`docs/plans/boundary-h.md`](../plans/boundary-h.md) (rev 0.4, proposed) §6 H5. It builds on the
[issuer-binding ADR](knowledge-issuer-binding.md) (P1), whose configured-external path authenticates operators, and
the [validity ADR](knowledge-validity.md) (K1b), whose exclusions H5 extends.

> Posture, once: **when in doubt, merge; never split.** Merging lowers counted independence; splitting would invent
> it. And **a cohort captures control dependence, not evidential lineage**: two independent organisations repeating
> one underlying report share an *origin*, and grouping issuers does not detect that.

---

## 1. The problem

"Identity is not independence" was already the rule: a reader configures control groups, and issuers in one group
are one voice. Three things were missing.

- **Nobody declared fleets.** A fleet of agents is admitted and retired continually. Asking every reader to
  configure it by hand is unworkable, and a colluding population is exactly where a reader must not have to guess.
  The Boundary H plan calls this the keystone: without it, H1's challenge threshold and H6's cohort budgets have
  nothing to count by.
- **Grouping depended on order.** `ReaderPolicy::group_of` returned the **first** configured group containing an
  issuer. With overlapping groups, independence depended on the order they were listed in.
- **There was no lifecycle.** The plan's review asked for defined handling of conflicting declarations, overlaps,
  removal, rotation, staleness in a partition, and agents admitted before their declaration arrives.

## 2. Decision

**Declarations.**
- A `CohortDeclaration { operator, cohort, seq, members, valid_from_ms, valid_until_ms }` has its own tagged
  canonical bytes. Members are listed by issuer identity, not by key, and in sorted order.
- It is signed by the operator and verified through P1's configured-external path.
- A reader accepts declarations only from the operators in its `CohortView::trusting(…)` set.

**Grouping as connected components.** The graph's nodes are issuers, configured groups and declared cohorts. Its
edges are:
- every member of every configured group;
- every member of every declaration in force now;
- each counted record's own placement.

Two issuers are dependent if any path joins them, including through a member who has said nothing. The root of
each component is its smallest node, so the result never depends on insertion order. This replaces first-match:
readers with overlapping configured groups may see lower independence, which is a narrowing change and stated in
the changelog.

**The lifecycle:**

| Case | Handling |
|---|---|
| Two trusted operators disagree | Both hold; the cohorts merge for this reader |
| Same `(operator, cohort, seq)` with different content | Both kept (`ConflictingSameSeq`); membership is their union |
| Overlapping cohorts or groups | Merged, as components |
| Removal | Only by a **superseding** declaration (higher `seq`); in force from its `valid_from_ms` |
| Expiry, or a partition with no refresh | **Membership is kept.** Staleness is reported (`Placement::any_current`), never treated as removal |
| Key rotation | No effect: membership is by issuer identity |
| Agent admitted before its declaration | The reader's `UndeclaredRule` |

**Historical grouping.** A record is grouped by the union of its issuer's membership **when the record was issued**
and **now**. A later removal therefore cannot make earlier evidence independent.

**Reader rules.** New `ReaderPolicy` fields:

| Field | Options | Default |
|---|---|---|
| `undeclared: UndeclaredRule` | `OwnGroup` · `OneGroup` (unknown issuers together) · `Excluded` (reported as `Exclusion::Undeclared`) | `OwnGroup`, today's behaviour |
| `stale: StaleRule` | `Retain` · `Exclude` (evidence whose only placements are stale is reported as `Exclusion::StaleCohortOnly`; it never falls back to looking independent) | `Retain` |

The confined-fleet profile will require `OneGroup` or `Excluded`.

## 3. What this does not claim

- **Lineage.** Cohorts group control, not origin. NovusLens's echo-detection gap is a lineage problem.
- **Truth of a declaration.** A cohort is as accurate as the operator who declared it. A reader trusting an
  operator that under-declares will over-count.
- **Durability.** `CohortView` is in memory. A reader that restarts must be offered its declarations again, and
  until then undeclared issuers fall under its `UndeclaredRule`, which is why the confined profile makes that rule
  conservative.
- **Challenges.** H1 (§4) counts them by the same components.

## 4. H1: challenge admission

**Problem.** One challenge from any non-provider issuer made a verdict `Rejected`, or `Conflicted` when support
existed. There was no threshold and no grouping of challengers. A colluding population could hold every honest
release, including an observer's, in `Rejected` or `Conflicted` indefinitely: denial of evidence at population
scale. The cautious reading for one member became an attack for many.

**Decision.**
- **A threshold counted by the same components as support.** `ReaderPolicy::min_challenge_groups` defaults to 1,
  which is exactly the behaviour before H1. A declared cohort's thousand challenges are **one** group.
  - At or above the threshold, the outcome is as before: `Conflicted` with support, `Rejected` without.
  - Below it, support decides, and `Classification::challenges` (a `ChallengeReport`) says what the challenges
    were: records, groups, whether admitted, and a bounded sample.
- **Two routes past the threshold, kept distinct.** A signature establishes *who*; trust policy establishes *decisive*.
  1. **Mechanically verified invalidation:** the provider itself, verifiably and currently, challenging its own
     release (`RejectionReason::DisownedByProvider`). No trust decision is involved. *The plan's other form, a
     revocation by an authority P2 shows is entitled, waits for P2.*
  2. **A policy-decisive source:** a challenger in `ReaderPolicy::decisive_sources`
     (`RejectionReason::DecisiveSource`), named as a policy choice.

  Either one rejects, whatever the support.
- **Evidence-bearing outsiders are not dismissed.** A below-threshold challenge that cites evidence (a
  `DerivedFrom` link to a record the store holds) is listed in `evidenced_unadmitted`, apart from bare challenges.
  It decides nothing alone, and the report shows it.
- **Bounded cost.** A threshold stops a storm deciding the verdict. It does nothing about the storm's cost, so:
  - `max_examined` caps the records a resolution reads about one subject. Past it, the verdict is
    `InsufficientEvidence` and `budget_exhausted` is set: nothing is concluded from a partial count.
  - `max_reported` (default 64) caps the individually reported exclusions, samples and rejection reasons; totals
    stay complete.
  - Rejection reasons are one per challenger, not one per record.

**The cost of raising the threshold, stated.** An isolated honest challenger with no evidence the reader can verify,
outside the decisive sources, no longer blocks a release alone. Each reader chooses that trade.

**Moved, not dropped.** The plan's per-issuer **ingestion quota and storage cap** belong to the store, which K3a
restructured on a sibling branch, and to transport. They are recorded under K3c in the
[validity ADR](knowledge-validity.md).

**Not claimed.**
- Per-subject budgets bound *examination*, but building the currency index still scans the store. That cost is
  O(store), not O(subject).
- Measured: a 100,002-record storm resolves in about 2.9 s in an unoptimised test build. That is a measurement,
  not a bound.

## 5. Gates

- `resolution::tests::h5`:
  - a declared cohort of fifty is one independent voice;
  - independence does not depend on configuration or arrival order, and components join through a silent shared
    member;
  - **the reviewer's case exactly**: expiry and partition cannot manufacture independence, and under
    `StaleRule::Exclude` the stale-only issuer is excluded and reported;
  - a removal does not make earlier evidence independent;
  - undeclared issuers follow the reader's rule.
- `resolution::tests::h1`:
  - a cohort storm below the threshold is reported, with a bounded sample, and does not decide;
  - the default threshold still lets any challenge decide;
  - independent challengers that reach the threshold decide;
  - the provider disowning its release decides;
  - a decisive source decides and is named;
  - an evidence-citing challenge below the threshold is reported separately;
  - a 100,000-record storm is bounded by budget and in output.
- `cohort::tests`:
  - an untrusted operator is ignored;
  - a forged declaration is refused;
  - a superseding removal applies only to later evidence;
  - an expired declaration keeps membership and reports staleness.
