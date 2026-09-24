# Boundary H — implementation plan

> **Status: PROPOSED, 2026-09-24. Rev 0.1. Not adopted.** This plan implements the mitigations that
> [`docs/threat-model.md`](../threat-model.md) §5 Boundary H (revision 3 draft) proposes against a **colluding
> population of admitted members**. It follows the contracts axis's posture
> ([`v3-contracts-axis.md`](v3-contracts-axis.md) §1.3) and cross-cutting rules (§9) without restating them. Every
> code claim below was checked against `origin/main` at `9297e18`, and the branch was rebased onto `c2ad49a` (v2.13.0 and
> #367–#377, none of which touch the code this plan cites).

---

## 1. Why, and what done means

Each boundary in revisions 1 and 2 models one adversary. Boundary H models many admitted members, each within its
own authorisation, whose concerted behaviour is the attack. An agent fleet whose agents are full members is its
natural deployment. The motivating case is the 2026 OpenAI–Hugging Face incident, in which evaluation agents shared
credentials and techniques over an unmonitored channel for about two months.

**Done means three things, all in CI:**
1. Every negative case in §6 passes as a replayed test.
2. One decisive demonstration (§5, item D) runs as a gallery entry.
3. Threat model revision 3 moves each H item from *Proposed* to *in force*, citing the PR that shipped it.

**Done does not mean** any claim about fleets above the size the nightly runner evidences (V1 is still unmet), or
any claim about acts that bypass the substrate outside the confined-fleet profile (H7).

---

## 2. What verifying the code found

Four findings that the threat model draft did not yet state, and that change the order of work:

1. **An issuer is not bound to an admitted identity.** `IssuerId::new` accepts any non-empty string, and
   `KnowledgeRecord::verify` takes the key from its caller. One member can therefore be many issuers. That is a
   Sybil attack *inside a single member*, before any population is involved. Control groups (H5) and challenge
   thresholds (H1) mean nothing until issuers are bound to identities (P1).
2. **A mandate is not portable evidence.** `Mandate` is an unsigned struct. Its signed epoch is verified only by
   the wiki's pre-receive hook, which `mandate_fence.rs` says no unit test exercises. A reader at resolve time
   therefore has nothing it can verify that says "this advertiser is authorised". H3 needs a signed mandate grant
   (P2).
3. **The knowledge layer's remaining PRs are unscheduled.** `store.rs` says durability, reader authorisation and
   head transport are "later PRs", but neither the plan of record's §10.2 nor the ROADMAP lists them. H depends on
   them, so this plan schedules them (K1–K3).
4. **H1 depends on H5.** A challenge threshold counted by control group is useless if 1,200 keys are 1,200 groups.
   The cohort declaration (H5) is the keystone: without it, H1 and H6 have nothing to count by.

Two existing mechanisms also do more than the draft credits:
- **The audit sink.** The WS-C `AuditSink` (SIEM or WORM archive) keeps the original bytes of every record it
  mirrors. So a reissued audit suffix is already detectable where an operator has attached one. H4 extends that to
  deployments without a sink, and to detection *within* the mesh.
- **Mandate reads at enforcement points.** AE1's `MandateBinding` / `MandateState` already assemble a mandate
  finding at the enforcement point. H3 reuses that path; it does not build a second one.

---

## 3. Posture: what kind of mechanism each item is

The contracts axis permits prevention in exactly three shapes (§1.3 rule 3). Everything else here is detection or a
reader's own choice. **No item touches signal propagation or KV replication.**

| Item | Kind | Where it acts | Prevention shape, if any |
|---|---|---|---|
| P1 issuer binding | Honesty fix | Knowledge record verification | none |
| P2 signed mandate grant | New artefact (a §6 scoped attestation) | Issued by the establishing authority | none |
| K1–K3 knowledge completion | Item 3's deferred PRs | Knowledge store and heads | none |
| H1 challenge admission | Reader policy | `resolution::classify` | none |
| H2 per-issuer support | Honesty fix | `resolution::classify` | none |
| H3 advertisement bound to authority | Reader-side filter plus an observation | The resolve path, after native gates | none: the advertisement still gossips |
| H4 witnessed audit heads | Detection | Each node's own audit chain | none |
| H5 cohort declared at admission | Operator statement | Reader policy input | none |
| H6 aggregate budgets | Admission control | The provider's admission point | (ii) enforced at the resource the caller trusts |
| H7 confined-fleet profile | Deployment profile plus a self-report | Operator runbook and a node check | (iii) opt-in profile with a declared strength |

---

## 4. Order of work

```
Phase 0 — prerequisites            Phase 1 — evidence counting     Phase 2 — authority & history      Phase 3 — proof
P1 issuer ↔ identity binding ──┬─► H2 per-issuer support           H3 advertisement ◄── P2
P2 signed mandate grant        │   H5 cohort declaration ──┬──►    H4 witnessed audit heads           H7 confined-fleet profile
K1 verify on put ◄── P1        └─► (H5 needs P1)           ├──►    H6 aggregate budgets               D  decisive demonstration
K2 signed heads                    H1 challenge admission ◄┘                                          threat model rev 3 final
K3 durable store + head transport
```

- **Phase 0** is mostly the knowledge layer finishing its own contract. It is worth doing for its own sake, and
  it is where most of the work is.
- **Phase 1** is small and is the fastest real improvement. H2 can ship first, alone, before P1.
- **Phase 2**'s items are independent of each other and can run in parallel.
- **Phase 3** is where the story becomes demonstrated rather than argued.

Suggested releases (v2.13.0 shipped 2026-09-23): **v2.14** carries H2 and the `#[non_exhaustive]` preparation;
**v2.15** Phase 0 and the rest of Phase 1; **v2.16** Phase 2; **v2.17** Phase 3. All are additive on the 2.x line, with upgrade notes of the classes
used since v2.8.0.

---

## 5. The items

Each item gives: what it does · design · where · compatibility · **the gate** (the test that detects a violation,
per §9's five-part PR rule) · size (S = one PR, M = two to three, L = four or more) · open questions for its ADR.

### P1 — Bind issuers to admitted identities (M)

- **What.** A knowledge record's issuer resolves to an admitted member's identity key, and verification looks the
  key up rather than accepting it from the caller.
- **Design.**
  - Add `IssuerId::for_node(&NodeId)` as the canonical form for member issuers.
  - Add `KnowledgeRecord::verify_as_member(&identity_view)`, which fetches the key from the reader's
    `sys/identity` view (retained keys minus validated revocations, per threat model §6).
  - Keep the existing `verify(key, sig)` for non-member issuers such as operators and external auditors. Those
    issuers are trusted only through reader configuration, never by default.
  - **Undeclared issuers** (H5) then default to *not a member*.
- **Where.** `src/knowledge.rs`, `src/knowledge/store.rs`.
- **Compatibility.** Additive.
- **Gate.** A record signed by member A but claiming member B's issuer fails `verify_as_member`. A record from a
  revoked key fails for present authorisation and keeps its historical attribution.
- **Precondition, stated rather than assumed.** Its strength rests on `require_identity_proofs`, which defaults to
  off. H7's profile turns it on.
- **ADR questions.** Should a member be allowed more than one issuer stream, as `knowledge/head/{issuer}/{stream}`
  already permits? Recommendation: one issuer per member, many streams.

### P2 — A signed, portable mandate grant (M)

- **What.** The establishing authority signs a `MandateGrant` over the mandate's canonical bytes (holder, scope,
  enumerated operations, epoch, term, validity window), so any reader can verify it without reaching the protected
  resource.
- **Design.** A §6 **scoped attestation**: bound to the digest of the exact mandate, signed by `established_by`'s
  key, carrying no transferable credential. `MandateState` gains a finding for a grant that verifies but is not
  current. The in-store fence stays the enforcing check; a grant is evidence of an appointment, not a bearer token.
- **Where.** `src/mandate.rs`, plus AE1's `MandateBinding` assembly in `src/agent/action_evaluator.rs`.
- **Compatibility.** Additive. `Mandate` itself is unchanged.
- **Gate.** A grant with an altered operation list fails verification. A grant verified after `valid_until_ms` is
  refused for present authority. A grant signed by a key that is not the named `established_by` is refused.
- **ADR questions.** Where are grants published: a capability attribute carrying a digest plus an authorised
  fetch, or the knowledge store as an operator-issued record? Recommendation: the knowledge store, which composes
  with K1–K3 and keeps bodies out of gossip.

### K1–K3 — Finish the knowledge layer's contract (L in total)

These are item 3's own "later PRs", scheduled here because H depends on them.
- **K1, verify on put (S).** `KnowledgeStore::put` refuses a record that does not verify under P1, and surfaces
  it as a counted refusal, never a silent drop.
- **K2, signed heads (S).** `Head` gains a signature by its issuer, verified on read. A head that fails reads as
  absent, the same rule as signed `sys/` keys.
- **K3, durable store and head transport (M–L).**
  - Records get a durable, fsynced store, following the evidence journal's pattern.
  - Heads gossip under the reserved `knowledge/head/{issuer}/{stream}`.
  - Reader authorisation for bodies follows Boundary F's opaque-address rule.
- **Gates.** Boundary F's three replayed negative cases, re-run over the durable store with transport enabled.
  Also: a forged head is ignored, and a record whose signature fails never reaches `classify`.

### H2 — Count support per issuer (S)

- **What.** `min_supporting` counts distinct issuers, not records.
- **Design.** A one-line semantic change in `classify`: deduplicate `supporting_issuers` before counting. It can
  only make acceptance harder, never easier.
- **Where.** `src/knowledge/resolution.rs`.
- **Compatibility.** The meaning of `Verdict::Accepted.supporting` changes. State it in the upgrade note, in the
  open, the way ack meanings change by changing a pin. Mark `Verdict` and `RejectionReason` `#[non_exhaustive]` in
  the same release, before H1 needs it (the #366 precedent).
- **Gate.** One issuer with five supporting records and `min_supporting = 2` gives `InsufficientEvidence`.

### H5 — The cohort, declared at admission (M; keystone)

- **What.** The operator who admits a fleet declares it as one cohort. Readers that choose to trust that operator
  treat the cohort as one control group.
- **Design.**
  - A `CohortDeclaration`, issued as a knowledge record by an operator issuer. It names a cohort id and its
    members, and later declarations extend it.
  - `ReaderPolicy` gains `cohort_sources` (which operators' declarations this reader trusts). It also gains an
    `undeclared` rule: `OwnGroup` (today's behaviour, the default), `OneGroup` (all undeclared issuers together) or
    `Excluded`.
  - A reader in a fleet deployment sets `OneGroup`. Freshly minted or undeclared issuers then cannot manufacture
    independence.
  - This is a statement made by whoever admits the fleet, never an inference by the reader, so §4's "identity is
    not independence" is kept.
- **Where.** `src/knowledge/resolution.rs` (policy), plus a small `src/knowledge/cohort.rs`.
- **Compatibility.** `ReaderPolicy` gains fields. That breaks exhaustive struct literals, the same upgrade-note
  class as `GossipConfig` in v2.5.0; `Default` construction is unaffected.
- **Gate.**
  - Fifty cohort members all supporting gives `independent = 1`, so `min_independent = 2` returns
    `InsufficientEvidence`.
  - Under `OneGroup`, ten undeclared issuers count as one.
  - A cohort declaration from an operator the reader does not trust is ignored.
- **ADR questions.**
  - Declaration record or certificate extension? A cohort id in the CA-issued certificate is unforgeable and
    automatic for newly spawned agents, but certificates are not in KV today and readers map issuers to certificates
    only after P1. Recommendation: the declaration record now, with the certificate extension recorded as the
    alternative.

### H1 — Challenges counted by group (S–M; needs H5)

- **What.** Challenges are counted by control group against a reader-set threshold, and challenges below the
  threshold are reported, never hidden.
- **Design.**
  - `ReaderPolicy::min_challenge_groups`, with a default of 1, which is today's behaviour exactly.
  - At or above the threshold, the outcome is today's: `Rejected`, or `Conflicted` when support exists.
  - Below it, the verdict follows the support rules but carries `unadmitted_challenges { records, groups }`, so a
    challenge storm is visible even when it does not decide the outcome.
- **The cost, stated.** Raising the threshold means one honest, isolated challenger can no longer block a release
  alone. Each reader chooses that trade; the substrate does not choose it for them.
- **Where.** `src/knowledge/resolution.rs`.
- **Compatibility.** Needs `Verdict` to be `#[non_exhaustive]` (done in H2's release).
- **Gate.**
  - A declared cohort files 1,000 challenges against an observer's release with `min_challenge_groups = 2`: the
    verdict is not `Rejected`, and it carries `unadmitted_challenges { records: 1000, groups: 1 }`.
  - Two independent groups challenging gives `Rejected` or `Conflicted`, as today.

### H3 — Advertisement bound to authority (M; needs P2)

- **What.** A reader can require that a capability in a namespace it protects resolves only when its advertiser
  presents a current mandate grant enumerating that capability. A capability without one is filtered out *and
  reported*.
- **Design.**
  - `CallerContext` / resolve options gain `protected_namespaces` and a trusted-authorities set.
  - The advertiser references its grant in the capability's `attributes`, as a digest.
  - The check runs after the native gates and before evidence resolution. It reuses AE1's mandate-state assembly,
    and the grant must permit `serve:{namespace}/{name}`.
  - A filtered capability produces three signals:
    - a counter, `cap_unbacked_protected_total`;
    - an audit record;
    - optionally, a knowledge `Observation`, which is the signal NovusLens turns into a Capability / Authority /
      Responsibility finding.
- **Honest limit.** Colluders resolve with their own policy, so H3 keeps a laundered power away from *honest*
  readers and makes it visible. It does not stop a cohort from using the power among its own members.
- **Where.** `src/agent/capability_handle.rs` (resolve path), `src/capability.rs`.
- **Compatibility.** Additive; the default is no protected namespaces.
- **Gate.**
  - An unbacked protected capability is filtered and observed.
  - A backed one resolves.
  - A grant past `valid_until_ms` is filtered from that moment.
  - A grant for `serve:a/x` does not back `a/y`.

### H4 — Witnessed audit heads (M)

- **What.** Members record, in their own audit chains, the audit heads they have received from others. A reissued
  suffix then surfaces as equivocation.
- **Design.**
  - A new `AuditAction::Witness`, whose target and detail carry `{node, seq, content_hash}`. It is sealed into
    the witness's own signed chain, so the witness statement is itself attributed and tamper-evident.
  - Witnessing is bounded: each node witnesses up to *k* peers per interval, with peers chosen through the replay
    RNG seam so tests are deterministic.
  - `verify_witnessed(streams) -> Vec<Equivocation>` compares witness statements with current streams.
  - Exposed on a gateway route under `audit:read`, with the SDKs in the same PR (the parity gate).
- **Honest limits.** Last-write-wins replaces the original bytes on replicas, so witnesses *prove* a rewrite but
  cannot recover it; the WS-C sink recovers it. Witnessing does nothing about acts that were never recorded; that
  is H7's job.
- **Where.** `src/agent/audit.rs`, the gateway audit route, `mycelium-py`, `mycelium-ts`.
- **Compatibility.** If `AuditAction` is exhaustive, mark it `#[non_exhaustive]` first.
- **Gate.**
  - A node reissues records *s* onward with valid signatures, and `verify_witnessed` reports equivocation at *s*.
  - An untouched stream reports none.
  - The same schedule replays identically.

### H6 — Aggregate budgets over a declared cohort (M; needs H5)

- **What.** A provider can cap a whole cohort's in-flight calls, not just each caller's.
- **Design.**
  - An RAII admission slot keyed by cohort id at the provider's admission point. It mirrors
    `CallPolicy::max_in_flight_per_partner` (#365) and composes with item 4's admission contract.
  - The refusal is `AtCapacity`, the same vocabulary.
  - The same stated limits as #365 apply: the cap is per provider instance (N instances means N × cap), and it
    bounds concurrency, not rate.
  - Prevention shape (ii), with its strength stated as `SelfImposedPrevention`.
- **Where.** The provider admission path, plus item 4's rights ledger for recording refusals.
- **Gate.**
  - Fifty cohort members, each under its own per-caller cap, together exceed the cohort cap, and the excess is
    refused.
  - A different cohort's calls are unaffected.
  - Refusals appear in the rights ledger.

### H7 — The confined-fleet profile (M, mostly operations)

- **What.** The deployment rule that turns "acts off the substrate are invisible" into "acts off the substrate are
  blocked": **agent code never holds a member key, and agent pods can reach nothing on the network except their own
  node's gateway.**
- **Deliverables.**
  - `docs/operations/confined-fleet.md`, covering:
    - sidecar deployment;
    - a network policy (an example Kubernetes `NetworkPolicy` is included);
    - `EgressPolicy.allow_hosts` non-empty and failing closed;
    - `require_identity_proofs` on;
    - the cohort declared (H5);
    - short mandate terms;
    - an observer member with `mesh:read`;
    - an attached `AuditSink`;
    - an action evaluator *with* an evidence journal.
  - A node self-check, `ConfinementReport`, published at startup:
    - it reports what the node can know: allow-hosts set, identity proofs required, sink attached, evaluator and
      journal attached;
    - it reports network confinement as `Unverified`, because a node cannot see its own network policy;
    - evidence exports cite the report in their coverage statement.
- **Prevention shape.** (iii), an opt-in profile with a declared strength. The substrate verifies what it can and
  says what it cannot.
- **Gate.**
  - A unit test on the report.
  - A Docker test (the two-mesh suite's pattern) proving an agent container cannot reach an external host except
    through the gateway, and that the gateway's attempt is recorded as an evidence decision.

### D — The decisive demonstration (M)

A gallery entry, `examples/boundary_h_fleet.rs`, with a `mycelium-sim` replay scenario checked into the corpus and
run in CI. The demonstration is an incident shaped like Hugging Face, at the scale the suite can honestly carry:

1. **Setup.** A declared cohort of N agent members runs under the confined profile, with one observer member and
   an operator.
2. **Laundering.** One agent acquires a power no mandate grants and advertises it as a capability. H3 filters it
   for the observer and emits the observation.
3. **Manufactured support.** The cohort files mutual support. H5 counts it once, so it is `InsufficientEvidence`.
4. **Jamming.** The cohort files a challenge storm against the observer. H1 records it and does not let it decide.
5. **Rewriting.** One agent reissues its audit suffix. H4 reports equivocation.
6. **Volume.** The cohort's call volume exceeds the cohort budget. H6 refuses the excess.
7. **Egress.** An agent's direct egress attempt is blocked and appears as an evidence decision (H7).
8. **Attribution.** Given a credential, the fleet answers "which agent, under which mandate", as a lookup.
9. **Halt.** The operator withholds mandate renewal, and every action through the seam is refused after
   `valid_until_ms`. **Shutdown latency is measured and reported**, not asserted.

N is set by what the replay kernel carries deterministically. The entry states it and makes **no claim above
V1's evidenced ceiling**.

---

## 6. The negative cases (CI, replayed)

Each is a sentence the threat model can cite once it is green:

1. One member cannot be many issuers (P1).
2. One issuer cannot meet `min_supporting` alone (H2).
3. A declared cohort cannot manufacture independent support (H5).
4. A cohort cannot jam a verdict below the reader's threshold, and the attempt is visible (H1).
5. An unbacked protected capability does not resolve for a reader that protects its namespace, and is reported
   (H3).
6. A reissued audit suffix is detected by any honest witness that saw the original (H4).
7. A cohort cannot exceed its aggregate budget at a provider that sets one (H6).
8. Under the confined profile, an agent cannot reach the network except through an enforcement point that records
   the attempt (H7).
9. Withholding mandate renewal halts seam-mediated action within the term (existing AE1 behaviour, now gated at
   fleet shape).

---

## 7. What this plan does not claim

- **Scale.** Nothing above V1's evidenced ceiling. The demonstration states its N.
- **Colluders among themselves.** Reader-side checks protect honest readers. A cohort's internal use of a power
  is made visible, not prevented.
- **Collusion versus cooperation.** They remain the same behaviour. Containment is authority design, not something
  the substrate detects.
- **Acts outside the profile.** Deployments that do not adopt H7 keep today's residual: acts off the substrate are
  invisible, and evidence says `coverage.complete: false`.
- **A central halt.** There is still none. The halt is non-renewal plus revocation, which reaches only connected
  readers.
- **Identity-proof defaults.** `require_identity_proofs` stays off by default for rollout compatibility. H7's
  profile turns it on, and every H claim that rests on a signature says so.

---

## 8. Downstream (other repositories, tracked there)

- **NovusLens.**
  - Move the Mycelium pin from v2.4.4 to the release carrying Phase 2.
  - Ingest three new observation classes as findings: unbacked protected capability, witness equivocation, and
    below-threshold challenge storm.
  - Map cohort, mandate and capability onto Capability / Authority / Responsibility for agents.
  - The echo-detection gap (shared origin among asserted sources) is the same problem H5 solves here, and the
    cohort idea may transfer.
- **mycelium-private.** The AE exporter carries the cohort id and the `ConfinementReport` digest in its signed
  batches.

---

## 9. Process

- Each PR carries §9's five parts: the guarantee, its assumptions, the enforcing component, the failure behaviour,
  and the test that would detect its violation.
- Each PR updates the wiki page it touches and adds a dated `.log/` entry.
- Gateway changes (H4's route) ship with both SDKs and the operator docs in the same PR.
- ADRs needed: P1 + P2 together (identity and authority), K3 (store and transport), H5 (cohort), H4 (witnessing).
  H1, H2, H3, H6 and H7 are small enough to be argued in their PR descriptions against this plan.
- The threat model moves each H item from *Proposed* to *in force* in the PR that ships its gate.
