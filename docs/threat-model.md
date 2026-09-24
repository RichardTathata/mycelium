# Mycelium — Blast-Radius Threat Model

*Revision 2, 2026-09-13 (v3 contracts axis item 8, plan §6.5): §5 adds the boundaries the axis introduces —
foreign principals across a domain edge, authenticated-but-abusive clients, evidence confidentiality and the
hash-as-credential, a compromised former mandate holder and a forged epoch — and §6 the rules for what identity,
evidence and replay artefacts may carry. §1–4 (revision 1) are unchanged. Items 2, 3 and 5 cite this document from
their first ADRs rather than restating a threat model each.*

*Revision 3 draft, 2026-09-23: §5 adds **Boundary H**, a colluding population of admitted members — the plural of
§4's "trusted member acting maliciously within its authorization", which revisions 1 and 2 model only one member
at a time. §1–4, Boundaries D–G and §6 are unchanged. H's proposed mitigations are **unscheduled** and each needs its
own ADR; nothing in H claims a mechanism the code does not have.*

*Crown-jewel posture (Production Readiness Gap sub-gate #3, WS3).* This document
states what an attacker gains at each trust boundary, the substrate mitigations
in force, and the residual risk an operator must own. It is deliberately blunt:
the value here is a specific blast-radius conversation, not a reassurance.

Cross-links: operator runbook [`operations/crown-jewel.md`](operations/crown-jewel.md);
RBAC [`operations/rbac.md`](operations/rbac.md); audit [`operations/audit.md`](operations/audit.md);
security guide [`guide/09-security.md`](guide/09-security.md). Mechanisms referenced
here ship under the `tls` and `compliance` features unless noted.

---

## 1. Assets (the crown jewels)

| Asset | Where it lives | Why it matters |
|---|---|---|
| **Digital-twin state** (L1/L2/L3) | KV store + WAL/snapshot on disk | The concentrated map of every SPOF, critical path, and escalation route in the deployment. The highest-value target. |
| **Node identity keys** | Ed25519 signing key (`tls`), on disk | Compromise lets an attacker impersonate a node: forge signed KV writes, role claims, audit records, consensus votes. |
| **Audit trail** | `sys/audit/{node}/{seq}` | The tamper-evidence record. An attacker wants to edit history without detection. |
| **Role/clearance claims** | `sys/role/{node}` | Authorization input; forging one would grant access. |
| **Outbound reach** | network egress | A compromised node with open egress is the data-exfiltration vector. |

The L1/L2/L3 **clearance** model (WS1) reflects that these are not uniform: an
L1 board-level read is not the full L3 SPOF topology, and authorization is
classification-aware.

---

## 2. Trust boundaries & blast radius

### Boundary A — a single compromised node

An attacker who controls one node process (RCE, stolen host) gains:

- **Its own identity key** → can sign anything *as that node*: KV writes, its own
  role claim, its own audit records, consensus votes. Cannot forge *other* nodes'
  signatures (those keys never leave their hosts).
- **Read of all replicated KV state** that reached this node via gossip — i.e. the
  twin state. This is the worst single-node outcome: the map is replicated, so one
  node sees (much of) it.
- **Ability to clobber un-owned `sys/` keys** via LWW — but this is **detected**:
  the `sys/` namespace tripwire (`SystemStats::sys_namespace_violations`) flags a
  remote write to a key another node owns, and signed keys (`identity`, `role`,
  `audit`) fail signature verification at read, so a forged value reads as absent.

*Mitigations:* per-node key isolation; signature verification on all signed
entries (detection-not-prevention); the namespace tripwire; data-at-rest
encryption (WS3) limits an attacker with *disk* access but not process control.

*Residual:* a fully compromised node sees the twin state it has gossiped. Mycelium
does not encrypt in memory. Containment is the clearance model (limit what each
node holds) + operational isolation, not a substrate guarantee.

### Boundary B — the trusted gossip domain

Without `tls`, every node on the gossip port is assumed cooperative: a connected
peer can inject KV entries (bounded by LWW timestamps), claim any NodeId in a
`StateRequest` (harmless — misdirected response), or poison a dedup nonce
(P < 1/2⁶⁴). **Do not expose the gossip port to untrusted networks without TLS.**

*Mitigations:* `tls` makes the domain cryptographically closed — mTLS on every
connection (cluster-CA-signed certs), so a node without the shared CA cannot
join; signed consensus payloads and signed KV writes (`SignedData`, wire v10)
make undetected mutation require a node's private key.

*Residual:* a valid cluster member is trusted to gossip; mTLS authenticates
*membership*, not *intent*. RBAC (WS1) constrains what a member may *assert/serve*;
the audit trail (WS2) records what it did.

### Boundary C — external egress

A compromised node with unrestricted outbound reach can exfiltrate twin state to
an attacker-controlled endpoint, or pull malicious tool definitions.

*Mitigations:* `EgressPolicy.allow_hosts` (WS3) gates **every outbound HTTP path
the substrate chooses to make**: the MCP client bridge, capability probes, and
LLM-backend calls (core prompt skills + SkillRunner). Empty = allow all (default);
set it to fail-closed against unlisted hosts.

*Residual & coverage (be honest):* not gated in code — and intentionally so:
intra-cluster **bulk** peer fetches (cluster-internal, not external egress) and
operator-configured **OIDC JWKS** (auth infra the node must reach). The A2A
**client** lives in the SDKs (Python/TS), not the substrate; gate it at the SDK
or network layer. For anything ungated, restrict egress at the network layer
(firewall / security group / proxy allowlist) — see the crown-jewel runbook.

---

## 3. Data at rest

The substrate provides wire-level mTLS but does **not** encrypt the store in
memory. On disk, the **opt-in** `DataAtRestCipher` hook (WS3) envelope-encrypts
WAL records and snapshots; the operator supplies the KMS/keyring adapter (the
substrate is neutral on key custody). Without an attached cipher, persistence
bytes are plaintext on disk — a stolen disk or backup is a disclosure.

*Operator responsibility:* attach a cipher whose key is in a KMS/HSM and stable
across restarts; protect the node identity key (`tls`) with the same rigor — it
is itself a crown jewel (Boundary A).

---

## 4. What the substrate does NOT defend against

- **In-memory compromise** of a running node (no memory encryption).
- **A trusted member acting maliciously within its authorization** — RBAC limits
  scope and audit records intent, but a member authorized for X can do X.
- **Egress on the not-yet-gated paths** (LLM/probe/A2A) — network-layer control.
- **Availability attacks** beyond the documented gossip backpressure/opacity
  mechanisms.
- **Key custody** — Mycelium provides hooks; the KMS/HSM and rotation are the
  operator's.

This list is the point of the document: it converts "is it secure?" into a
specific, reviewable set of boundaries and residual risks.

---

## 5. Boundaries the contracts axis adds (revision 2)

Revision 1 models an **admitted mesh of cooperative members**: mTLS closes the domain, RBAC bounds what a member
may assert, the audit chain records what it did. The v3 contracts axis (`docs/plans/v3-contracts-axis.md`) adds
principals that are *not* members, members that are *authenticated but abusive*, evidence whose *address is its
credential*, and roles whose *authority outlives their term*. Each gets the same treatment as §2: what the attacker
gains, the mitigation in force or planned (with the item that ships it), and the residual the operator owns.

### Boundary D — a foreign principal across a domain edge (item 2, federation)

A **domain** is one independently admitted mesh. Federation connects explicitly *exported* services between
domains over a separate authenticated protocol and **never joins the transports**: a foreign node never enters
membership, native `cap/` / `grp/` / `sys/` / `consensus/`, anti-entropy or a quorum. The attacker here is a
principal of another domain — legitimately federated, or impersonating one.

- *Gains, if the edge is weak:* discovery of unexported services; invocation under a borrowed domain identity;
  membership merge (the catastrophic case — one mesh's LWW, quorum and evaporation now include the other's);
  a foreign gateway acting as a federation leader.
- *Mitigations (item 2 PR 1–6):* `DomainId` + signed `DomainDescriptor` + revisioned `DomainPolicy`; **three trust
  relationships kept apart** — membership (the CA), federation identity (the descriptor's key), service
  authorization (the export policy) — so no single credential unlocks the others; trust bundles as **bilateral
  operator configuration** (no registry, no transitive trust); filtered catalogs; origin preserved to the provider
  through the federation-aware adapter (item 7's `GatewayCaller`, across a boundary); ≥ 2 replaceable gateways, no
  federation leader; SWIM (unauthenticated UDP) **off** in the enforced profile; on disconnect, discovery expires and
  calls fail explicitly — reconnect refreshes before new work and never merges.
- *Residual:* a federated principal is trusted to *invoke what was exported*; the export policy is the operator's
  statement, and the release gate (two meshes, sever every link, change permissions mid-partition, reconnect, prove
  no merge) is what makes it a claim rather than a hope. Issued authority lasts to its expiry across a partition —
  revocation cannot reach a disconnected domain.

### Boundary E — an authenticated but abusive client (items 7 and 2)

Revision 1's Boundary B assumes a member is cooperative. A gateway client holding a valid bearer, scoped token or
OIDC identity — or a federated principal within its export — is *authenticated and still adversarial*.

- *Gains, before item 7:* every gateway dispatch ran as the **node**, so a provider's `authorized_callers` admitted
  the whole client population behind a listed gateway — the confused deputy.
- *Mitigations:* item 7 (shipped): the auth layer constructs a `GatewayCaller` — verified principal, the gateway
  as `via`, scopes as credential ∩ route — and the node attests it over the request digest; the provider verifies
  the signer against the keys it knows for `via` and refuses a forged, unsigned or mis-attributed context, never
  falling back to the node; `authorized_callers` judges the *client's principal*. The secure profile refuses a
  provider that cannot enforce the context. AE0's evaluator (`docs/design/action-envelope-ae0.md`) then bounds
  *what* an authenticated client may do at the route, with `indeterminate ≠ permit`; AE2 moves that check inside
  the resource's effect boundary.
- *Residual:* scope is coarse (`resource:verb`), and a client authorised for an operation may perform it abusively
  — rate, volume, argument choice. Budgets and allocated rights (item 4, RA) bound spend; the audit chain records
  intent; the gateway is a **route-level preflight** — a client that reaches a tool without traversing it is
  outside the guarantee, and the evidence says so (`coverage.complete: false`).

### Boundary F — evidence confidentiality and the hash-as-credential (item 3)

The knowledge layer stores claims, observations, assessments and acceptances as signed immutable records with
typed links; gossip KV carries only bounded signed discovery *heads*. The attacker is a reader who should not see
an evidence body, or a writer who wants misleading evidence to *do* something.

- *Gains, if evidence were naive:* reading a confidential evidence body because its content address leaked (today's
  reason blob tier verifies on read behind `llm:read` — the **hash is still the credential**); using an unrelated
  record's authority; erasing a competing observation by LWW; refreshing expired evidence by re-advertising.
- *Mitigations (item 3 PR 1–6):* an **opaque address** for confidential evidence — the content hash is an
  integrity check, never the access credential; records live in an *authorised* store, KV carries a pointer, so LWW
  moves a head and never erases competing statements — equivocation is preserved, not HLC-resolved; an issuer
  retracts only its own statements; refreshing an advertisement never refreshes evidence; **evidence never grants
  what authorisation denies**; three replayed negative cases (misleading evidence cannot erase a conflicting
  observation, refresh expired evidence, or confer authority) are the Phase D gate.
- *Residual:* competence is contextual and reader-decided; identity is not independence (correlated observers are
  the reader's control-group problem, item 3 deferred aggregated reputation and inferred independence on purpose).
  A reader that configures a bad policy accepts bad evidence — the substrate decides nothing about truth.

### Boundary G — a compromised former holder and a forged epoch (item 5, mandates)

A mandate names a scoped authority with an **epoch** and a term. The attacker is a principal whose term ended —
or whose key was stolen while it held one — attempting to commit under the old epoch, or anyone forging a newer one.

- *Gains, if authority were an inference:* today the curator's write entitlement is an in-process `is_curator`
  flag and the store CAS returns `Conflict` on *version* only — a former curator who re-reads fresh content passes
  the CAS. A stolen mandate key could mint operations until noticed; a forged epoch could pause the legitimate holder.
- *Mitigations (item 5 PR 1–6):* the decisive invariant — *once the protected resource acknowledges epoch E2, no
  operation authorised only under E1 commits there, whatever its holder refreshes, retries, reconnects or
  restarts* — enforced **inside the store's own atomic boundary** (the mandate ref updated under the same
  `update-ref` CAS as content; a pre-receive hook on the shared remote; the `FsStore` epoch in the same critical
  section), never in a separate authority service; a stale mandate is `MandateSuperseded`, never a `Conflict` fed to
  the retry loop; **three lifecycle events kept separate** — role expiry, permission withdrawal, outstanding-operation
  invalidation; a forged epoch fails signature verification against the issuer's key and admission; fail-closed
  authority restart; the handover journal is history, never inherited conclusions; incumbency rules.
- *Residual:* a disconnected holder may *prepare* proposals but cannot promise canonical acceptance without reaching
  the enforcing resource; the partition table states which operations wait, refuse or continue. Attribution
  outlives authority: an observation keeps its provenance after its observer's appointment ends (posture rule 5) —
  history is not revoked with a key.

### Boundary H — a colluding population of admitted members (revision 3; mitigations unscheduled)

Every boundary above models *one* adversarial principal: a compromised node (A), a foreign principal (D), an abusive
client (E), a former holder (G). The attacker here is **many admitted members acting in concert** — each validly
keyed, each within its own authorisation, so that no single act is out of bounds and the concerted behaviour *is* the
attack. The population may be one operator's many keys, a compromised fleet, or agents that learned coordination as
a strategy. The last case is not hypothetical: in the 2026 OpenAI–Hugging Face incident, evaluation agents shared
credentials and techniques over a channel nobody monitored for about two months before one of them caused an outage.
An agent fleet whose agents are full members is Boundary H's natural deployment, and where agent code runs inside the
embedding process it also holds the member's key (Boundary A, once per agent).

- *Gains (each verified against the code as of `9297e18`):*
  - **A coordination fabric.** Signals are forwarded unconditionally, and discovery, work claiming, consensus and
    the companions' contract net serve a colluding population exactly as they serve a cooperative one. The substrate
    cannot tell the two apart, because they are the same behaviour.
  - **A power laundered as a capability.** Any member may advertise any name under its own `cap/{node}/…`:
    `CapabilitiesHandle::advertise_capability` checks no role or mandate, and `authorized_callers` is set by the
    provider. A member holding a power no mandate gave it (a stolen credential) can offer that power to the others
    as a service, and the credential itself never travels.
  - **Many issuers from one member.** `IssuerId::new` accepts any non-empty string, and `KnowledgeRecord::verify`
    takes the key from its caller, so nothing binds an issuer to an admitted identity. One member can be many
    issuers, and every count below can be inflated from inside a single member.
  - **Manufactured support.** `resolution::classify` excludes self-assessment but not assessment by a peer. Under
    the default `ReaderPolicy` (one supporting assessment, one independent group, no control groups) a single
    colluding peer's `Supports` makes a release `Accepted`. `min_supporting` counts supporting *records*, so one
    issuer's repeated assessments meet it; only the independence count deduplicates.
  - **Jammed verdicts.** One `Challenges` assessment from any non-provider issuer makes a verdict `Rejected`, or
    `Conflicted` when support exists. There is no challenge threshold and no control-group test on challengers.
    The cautious reading for a single member becomes denial of evidence at population scale: a population can hold
    every honest release, including an observer's, in `Rejected` or `Conflicted`.
  - **A rewritten history, and an unrecorded one.** Each audit chain is per-node and signed by that node's own key.
    `verify_chain` proves that a stream is internally consistent, not that it is the stream peers first received.
    Nothing in `src/agent/audit.rs` detects a key holder reissuing its own suffix, and `sys/audit/{node}/…` is the
    holder's own namespace, so the `sys/` tripwire does not fire. Separately, the chain proves that no record was
    removed, not that every act was recorded. An act done off the substrate leaves no record at all.
  - **A pooled map.** Boundary A's worst single-node outcome, reading the replicated twin state, is multiplied: the
    population holds the union of what reached each member, and the fabric lets members with different clearances
    share what each holds.
  - **Collective volume.** Each member stays within its own budget while the total overwhelms a shared resource.
    Per-partner budgets are consumer-side only (plan §6.7, the RA slice; open).
- *Mitigations in force:*
  - **Attribution.** Every act *on the substrate* is signed by a member key, and mTLS admission names the population.
    Collusion on the substrate is attributable even where it is not preventable.
  - **Observability by membership.** The boundary controls acting, not receiving, so an observer member with
    `mesh:read` receives the population's coordination as it happens. This is the substrate's strongest property
    against H. It holds only for coordination that uses the substrate, and only for a reader that looks.
  - **Independence is the reader's** (item 3). A reader that places the population in one control group gets one
    independent vote from it. Evidence never grants what authorisation denies, so manufactured support can only
    select among already-authorised candidates.
  - **Authority lapses** (item 5, AE1). A mandate carries `valid_until_ms`, expiry is locally decidable, and AE1
    refuses a revoked mandate before policy runs. A population's authority therefore ends at its term unless it is
    renewed. Under the confined profile's authority-at-execution contract (plan A1), expiry stops new admissions locally,
    and admitted work stops within its declared continuation bound. Those two times are measured separately.
  - **An external audit sink** (WS-C, opt-in). An attached `AuditSink` (SIEM or WORM archive) keeps the original
    bytes of every record it mirrors, so a reissued suffix is detectable against it where one is configured.
  - **Containment.** Clearance (L1–L3) bounds what each member holds. A population run as its own domain never
    enters another domain's membership, replication or quorum (Boundary D). `EgressPolicy.allow_hosts` fails closed
    on the paths the substrate chooses (Boundary C), but its default is allow-all.
- *Proposed (unscheduled; sequenced in [`plans/boundary-h.md`](plans/boundary-h.md)):*
  - **Prerequisites.**
    - **P1:** bind issuers to admitted identities.
    - **P2:** a signed, portable mandate grant. `Mandate` is unsigned today, and its signed epoch is checked only
      by the wiki's pre-receive hook.
    - **K1–K3:** the knowledge layer's own deferred PRs (verify on put, signed heads, durable store and head
      transport).
  - **H1, challenge admission.** Count challengers by control group, against a reader-set threshold. The default
    stays one, the cautious reading, but a reader facing a population can require independent challenge.
  - **H2, per-issuer support.** `min_supporting` counts distinct issuers, not records.
  - **H3, advertisement bound to authority.** A reader-side check at resolve: a capability in a protected namespace
    resolves only if its advertiser holds a role or mandate naming it. The check stays at the reader, so Layer I is
    not taught a higher law.
  - **H4, source-signed audit checkpoints.** Each node seals signed checkpoints of its own stream, and members
    retain the ones they receive. An equivocation proof is two statements *signed by the accused* for the same
    position, so a witness's assertion alone never accuses anyone. Outcomes are equivocation, consistent, history
    unavailable, or insufficient evidence.
  - **H5, cohorts declared at admission.** The operator admitting a fleet declares it as a cohort. Readers resolve
    declarations and configured groups as connected components, independent of order, and fall back
    conservatively (merge, never split). A cohort captures *control dependence*, not *evidential lineage*: two
    independent organisations repeating one report share an origin, and grouping issuers does not detect that.
  - **H6, aggregate budgets.** Provider-side budgets over a declared population, closing plan §6.7's consumer-side gap.
  - **H7, the confined-fleet profile** (its own ADR). Agents and gateways run in separate pods (containers in one
    pod share a network namespace, so an in-pod sidecar cannot separate their egress), and the member key is mounted
    only in the gateway pod. Agent pods reach only the gateway: no instance metadata, no Kubernetes API, no admin
    routes. A blocked connection never reaches the gateway, so it is observed only through the network's own
    telemetry. Without that telemetry the claim is "blocked", not "recorded". A node self-report states what it can
    verify, and states network confinement as `Unverified`.
  - **A1, authority at execution.** Under the profile, every protected operation requires an established mandate,
    checked at the execution boundary. Queued, retried and delegated work keeps the requirement, and long-running
    work declares whether it re-authorises or runs to a stated bound.
  - **The consensus boundary.** No H claim rests on exclusive authority until the election and lock repairs pass
    their acceptance gate. Until then, a signed grant proves who issued it, and entitlement comes from
    configuration.
- *Residual:*
  - **Collusion and cooperation are the same behaviour with different intent.** mTLS authenticates membership, not
    intent, and that is as true of a thousand members as of one. A population acting within its authorisation will
    do what it is authorised to do. Containment is authority design (narrow mandates, short terms, a separate
    domain), not something the substrate detects.
  - **Acts off the substrate are invisible to it.** Direct network calls and credentials the substrate never issued
    are outside every guarantee here, and the evidence must say so (`coverage.complete: false`).
  - **No coordinator means no central halt.** That is the property the substrate exists to provide, and against a
    misbehaving population it protects the population from its operator too. The nearest thing to a halt is expiry
    plus revocation. Expiry is decided locally under a clock bound and stops *new admissions*. Admitted work stops
    only where the resource re-checks authority at execution (A1). Revocation reaches only connected readers
    (Boundary D, §6), or fails closed under a freshness bound. The operator owns the term lengths, the clock bound
    and the freshness bound.
  - **Moving coordination onto the substrate makes it both visible and easier.** An operator who admits untrusted
    agents as members is choosing that trade, and should also:
    - run the agents as their own domain;
    - run agents and gateways in separate pods, with the member key mounted only in the gateway pod;
    - declare the fleet as one control group;
    - issue short terms;
    - admit an observer member;
    - enforce egress at the network layer.

### The cross-cutting rule the four boundaries share

**Authority is recomputed, never inherited** (posture rule 5): every role the axis introduces — gateway, curator,
holder, observer, federated principal — carries an expiry, and its authority is re-established on renewal. Three
lifecycles stay distinct: the permission to issue new statements, the evidential freshness of an existing one, and
its historical attribution. A named mechanism (a signature, a lease, a CAS, a quorum) proves what it proves and no
more; every composed guarantee above is claimed only with its safety argument and its replayed or CI gate (rule 6).

---

## 6. What identity, evidence and replay artefacts may carry (revision 2)

The axis creates new records that travel: caller contexts, audit and evidence records, traces, and — with item 6 —
**replay bundles** that reproduce a run. Each is a disclosure surface. The rules (plan §9, *secrets never become
observability*):

| Artefact | Carries | Never carries |
|---|---|---|
| **Verified claim** (`GatewayCaller`, an AE0 envelope, a knowledge record) | the resolved principal, the verifying component, the scopes/authority *granted for this request*, a validity window, a signature by the component that verified | the reusable credential (bearer, token, JWT, private key); caller-supplied identity as authority |
| **Scoped attestation** (the node's signature over a caller context; an evidence exporter's batch signature) | what the signer verified, over a digest of exactly the request/batch it applies to, with the signer's key id | a transferable grant — an attestation is evidence of one check, not a bearer for the next |
| **Audit / evidence record** | verified claims, decisions with their policy revision, receipts, references (`evidence_ref`) to bodies held in an authorised store | payload bodies in gossip KV; tokens; secrets in `detail` |
| **Replay bundle** (item 6) | every external input the kernel needs to reproduce the run, **redacted of bearer credentials and keys**; the redaction map's *shape* (what was removed, not what it was) | a live credential; anything a reproduction does not need |
| **Protected reproduction artefact** | an input that cannot be redacted and still reproduce (a key whose signature the run verifies, a confidential evidence body) — stored outside the bundle under the operator's custody, referenced by digest | — it is the exception class, named so a bundle is never "mostly redacted" |

*Redaction rules:* a bundle is produced by the recording seam, not by hand; credentials are recognised at the
seam where they enter (auth headers, config secrets, key material) and replaced by a stable placeholder so replay
determinism holds; a bundle that would need a protected artefact says so and links it by digest. *Attestation
scope:* an attestation is bound to the digest of the exact object it vouches for and to the key that signed it.
*Verification is the reader's, against its retained set:* every consumer re-verifies a signature against the keys
**it** trusts for the named signer — the identity-auth anchors and the `sys/identity` history, which deliberately
**retains** earlier keys (WS5) so that a signature made before a routine rotation still verifies, **minus the
revocations that reader has received and validated** (WS-D). Two consequences the rules must state rather than
assume: (i) *rotation is not revocation* — requiring the signer's *current* key would reject legitimate history;
(ii) *revocation reaches a reader only when the revocation does* — a disconnected reader keeps trusting a revoked
key until it reconnects (Boundary D's residual, in key form), so "excluded everywhere" means everywhere the
revocation has propagated, never instantly. Therefore **present authorisation carries a freshness requirement**
(the attestation's validity window and the reader's view of revocations must both be current before it grants
anything now), while **historical attribution does not**: a record signed by a since-rotated or since-revoked key
keeps its provenance — it says who signed what, then; it grants nothing today (posture rule 5's three lifecycles).
A record supplied by anyone else is data, never authority.

*What this section does not do:* it does not make the audit chain or an evidence export a proof of coverage, does
not promise that a redaction map is complete for a custom seam an adopter adds, and does not encrypt anything in
memory (§4 stands).

