# dev/history — the delivery ledger

↑ [dev/](dev.md) · full execution records: `docs/plans/README.md` (the canonical index)

Reconciled current state of *what shipped when* — so no session re-derives it from git.
As of 2026-06-21 all v1.x/v2.0 engineering plans were shipped. Since then, **Legible Emergence
(diagnosability) is COMPLETE — all phases 0–5 shipped** (2026-07-02/03; see
[diagnostics.md](diagnostics.md) and `docs/plans/legible-emergence.md`):

- **Phase 0** — the pathology taxonomy design record (RT1–RT4 red-team baked in).
- **Phase 1** — the five coordinator-free emergent detectors + `/stats`/`/metrics`.
- **Phase 2** — `GET /gateway/fleet`, the relational fleet snapshot (throttle graph, cross-node
  store-convergence, commit-conflict hot slots).
- **Phase 3** — the HLC-stamped `EventRing` + `GET /gateway/explain`, cross-node causal
  reconstruction (best-effort fan-out naming non-responders; the #56 narrative).
- **Phase 4** — `GET /gateway/diagnose`, the `diagnose_fleet` rule engine (the "why is the fleet
  in this state" narrative, one rule per pathology).
- **Phase 5** — the operator surface: public `fleet_snapshot()`/`fleet_diagnosis()` API,
  `docs/operations/diagnostics.md` runbook + Prometheus alert recipes, guide pattern 11, and the
  coop `diagnostics` demo (induce-and-diagnose, Docker-free in CI).

The three-verb operator spine — **localize** (`/fleet`) · **explain** (`/explain`) · **diagnose**
(`/diagnose`) — is shipped, tested, and documented for both audiences.

## v2.14.0 release — 2026-09-25 (tag `v2.14.0`) — coordination that says what it means

Wire **v12** unchanged (`PREV = 11`); additive on the 2.x line.

**Read this entry as two halves.** The first is what the release set out to do — P10 and the
election rule, below. The second is what it found on the way, and is much the larger: an attempt to
flip `require_identity_proofs` was reverted, the failure it was blamed on turned out **not to be its
doing** (the flag is inert without TLS, and the suite's nodes configure none), and going back to
read the election path properly turned over five defects in the agreement protocol — an election
with no electorate deciding alone, votes not bound to what they voted for, a node equivocating with
itself, a higher ballot overwriting an accepted value, and none of that memory surviving a restart.
All five are closed here. The wrong diagnosis is recorded alongside the right one, because it is the
more instructive of the two: *"it was the only change in that commit"* is a prior, not a mechanism.

**The centrepiece started as a question in a design note** — *is role accumulation constrained
anywhere I did not look?* The answer was **no**, twice over, and the second half is the one that
mattered. *Not prevented* is the design working: **detection, not prevention** is this substrate's
law, so a missing `max_roles` is expected. **Not detected** is not. Every single-writer ring elected
by *lowest candidate node id wins*, so the same rule over the same candidates put **every
single-writer job on one node** — deterministically, on first election and again after every
restart — and the seven existing detectors were all watching other axes: **P2 churn** (a node calmly
holding everything produces none), **P6 gaps** (everything has a provider, merely the same one).
A concentrated fleet read as **perfectly healthy by every measurement that existed**, until the node
it all depended on went away, taking every role with it *as a block*.

The general lesson, worth more than the fix: **a catalogue of pathologies is not a catalogue of
axes.** Seven detectors over two axes leave every other axis unwatched, and nothing in the catalogue
says which axes it covers.

Both halves shipped together, deliberately — because **rendezvous spreads but does not bound**:

- **P10** makes it visible: `detect_role_concentration`, hysteresis-confirmed, with the
  **partition guard** that is the subtle part (a node which has lost sight of its peers sees only
  its own roles, *the pathology's exact shape*, so the reading is withheld below two visible
  holders — otherwise a partitioned node's first act is to accuse itself). Plus the operator half:
  `mycelium_emergent_role_concentration_pct`, a `diagnose_fleet` finding that says *"nothing is
  failing: this reads as healthy on every other detector, which is why it is easy to miss"*, a
  `narrate` gloss, a runbook recipe and a `for: 30m` warning alert — a standing structural
  condition, not a page.
- **`mycelium::election`** stops producing it: rendezvous ordering (`hash(ring, node)`), so
  different rings pick different winners and a failover relocates one role rather than all of them.

**The rollout was the real difficulty**, and it is the part to remember. The companions' safety
rests on every node computing the *same* answer — the wiki's sentinel says so outright. Deploy a new
rule node-by-node and an old node and a new one each believe they are the winner and **neither
resigns**: a *stable* two-holder state for the length of the rollout, in the one place the design
has no coordinator to break the tie. So the rule is **negotiated from the candidate set**: each
candidate advertises what it can compute (`election_rule`, an ordinary capability attribute — no new
gossip), and every elector takes the **minimum across live candidates**. A ring is only as new as
its oldest member. What remains is a *convergence-length* window (seconds), not a rollout-length one.

**`require_identity_proofs` was flipped to `true`, and reverted before release** — the release's
second lesson rather than its second feature. The argument was sound: every TLS node has written
`sys/identity-proof/{self}` unconditionally since Phase 2 (v2.3.0), so the two-release rollout the
old caveat prescribed finished ten releases ago. What it missed is that identity and proof are
**two separate `kv_set` calls**, hence two gossip messages with no ordering between them. A peer
that learns the identity first rejects it and holds no key for that node. The *key* heals itself
(`start_identity_watcher` subscribes to the broader `sys/identity` prefix precisely so a late proof
re-validates), so the window is transient — **but a leader election decided inside it is not**, being
one-shot. The Docker suite split on `S12 leader election … Nodes disagree on leader` after twelve
consecutive greens.

Flipping it **broke no test**, and that is the transferable part: every test exercising the
behaviour sets the flag explicitly, and the in-process suites have no cross-process ordering window
to lose a race in. *A config default whose only failure mode is a race between processes is not
testable by the suite that gates the PR* — a green `make check` on a default flip is not evidence,
it is the absence of a gate. What survives the revert: the default is pinned **with its reason**
(flipping it back means editing a test that explains itself), the end-to-end join is pinned beside
it, and a third test states the boundary the flag never closed (first sighting is still
trust-on-first-use; anchors close that, not proofs), written to fail if that window is ever shut.
A future flip's precondition is an **atomic** identity+proof record, not "every node writes a proof
anyway".

Also: `RELEASING.md` **step 8** — a tag is not an announcement. The Releases page had said *"Latest:
v2.4.4"* since 2026-09-12 while eleven tags shipped behind it, four carrying security fixes, because
**nothing fails when the publish step is skipped**. All eleven were backfilled from their own tag
messages.

**Upgrade notes:** `FleetSnapshot` gained `role_concentration`; election behaviour changes only
once every candidate advertises its rule; **no identity-proof action** — the default is unchanged.

## v2.13.0 release — 2026-09-23 (tag `v2.13.0`) — the axis' last open questions, and a gateway that was not closed

> **Published to the Releases page 2026-09-23, along with the ten tags before it.** The page had
> said *"Latest: v2.4.4"* since 2026-09-12 while eleven tags shipped behind it — four carrying
> security fixes — because a tag is not an announcement and nothing fails when the publish step is
> skipped. `RELEASING.md` gains **step 8** so the omission cannot recur silently; the release body
> is the annotated tag message, which was written for exactly this and does not drift from itself.

Wire **v12** unchanged (`PREV = 11`); every change additive on the 2.x line. Three threads.

**Item 2's row 11 is complete**, and the interesting part is how much of it could not be *stated*
before. The **SDK verbs** (`with_federation_clients`, five `/gateway/federation/*` routes behind
`federation:read` / `federation:invoke`) put the node in the consumer's seat rather than porting
credential minting into two more languages — one trust story, not three — and the credential names
the **local caller**, never the node and never anything in the request body, which is item 7's
confused deputy at one more boundary. A **hostile network** closed in two halves (body binding
09-22, TLS pinned on an SPKI in the trust bundle 09-23, because the design holds no X.509 to anchor
on). And **more than two domains**: with exactly two, a catalogue *filtered for the asker* is
indistinguishable from the export list, a grant to one partner from a grant, and *trust is not
transitive* has no third party to fail to reach. A chain — alpha grants to beta, beta grants to
gamma, alpha and gamma strangers — states all three.

**The two open design decisions, decided and built.** `mycelium-commitment`'s offers and awards
carry provenance: the crate's first rule (*no component assigns another participant's obligation*)
had nothing enforcing it, and a forged offer would have **passed the award's own check** — a pure
rule over forgeable inputs verifies that the rule was applied, not that the inputs were real.
Signing beat binding a record to its writer on a fact about the substrate: **anti-entropy carries no
author**, so writer-binding would hold on the gossip path and evaporate on the repair path. And the
**federation edge meters calls per partner** — the mirror of `GatewayPool` on the other side of the
same edge, with the M7 shared-observation alternative rejected because it would put foreign domain
names into `sys/` (D7) and is unbuildable without this counter anyway.

**The security fix, and why it survived two audits.** A deployment whose only credential model was
`gateway_named_tokens` — the configuration `GossipConfig`'s own docs tell operators to **prefer** —
had been running an **open gateway** since 2.10.0: `gateway_auth`'s token-model predicate counted
the legacy and positional tables and not the named one, so no bearer was required at all and
`open_gateway_scopes` granted each route exactly the scope it asked for. It survived the Phase-C
audit and the fuzz campaign because **the tokens worked**: every positive assertion passed, and the
only way to see it was to present *nothing*. Found by a scope test that returned 504 where 403 was
expected. **Operators on 2.10.0–2.12.0 with named tokens only: an unauthenticated
`GET /gateway/kv/keys` answers 200 today and 401 after this release.**

Also: §12.2 closed (the SDK receipt narrative — including that `MyceliumCheckpointSaver.put()`
returns a **rung-1 receipt**, which the flagship demo already knew, waiting for replication by
*reading from node B*), two wiki pages (`architecture/contracts`, `testing/replay`), the plan of
record's **rev 1.15**, and `CallRefusal` / `CommitmentRefusal` marked `#[non_exhaustive]` before
their next addition rather than after it.

**Upgrade notes**, all in v2.8.0's class — a struct gained a field or an enum gained a variant:
`CallPolicy` (`max_in_flight_per_partner`), `Offer` and `Award` (`signature`),
`CommitmentRefusal::AlreadyAwarded` now `Box<Award>`, and the two `#[non_exhaustive]` enums whose
`_` arms **must fail closed**.

**Not claimed:** a commitment signature's strength rests on `require_identity_proofs`, which is
**default-off**; and the per-partner cap is **per gateway** (N gateways ⇒ N × cap) and bounds
concurrency, not rate. Both are written beside the mechanisms rather than left to be discovered.

## v2.12.0 release — 2026-09-23 (tag `v2.12.0`) — authenticating the caller is not authenticating the call

Wire **v12** unchanged. Ten workspace crates move to 2.12.0.

**Two defects, one shape.** Both are compositions where every part behaved exactly as documented
and the join did not — which is the failure this axis was created to find, arriving twice more.

**The federated credential authenticated *who* and *which export*, and not *what*.** Its signature
covered the origin domain, the principal, the export and the validity window. An attacker between
two domains could rewrite a call's body, leave the header untouched, and the receiving gateway would
accept the altered call as authentic — then run the AE preflight and write an evidence record about
the attacker's text. `FederatedCaller::body_sha256` is inside the signature now; the client
serialises once and signs those bytes, and `POST /a2a` reads the body as `Bytes` and parses
afterwards, because a digest over a re-serialisation compares our encoder against theirs.

**`preflight` was crate-private**, so an enforcement point that is not this gateway had no public
path but `ActionEvaluator::evaluate` — which performs none of the seam's five checks: expiry, stale
policy revision, a `Permit` carrying evaluation errors, an unmapped operation, a panicking adapter.
In each, a *correct* evaluator returns a permit and the seam refuses. It is public now, gated as a
**difference**: the test asserts `evaluate` permits *and* `preflight` refuses, because asserting
only the refusal would prove nothing about which layer carries the guarantee.

**What is deliberately not claimed.** This is integrity, not confidentiality: an on-path observer
still reads every federated call. TLS on the edge needs a trust anchor that does not exist —
partner trust here is an Ed25519 key with no X.509 material anywhere — and that is a decision, not
an implementation.

**Process.** `make check` now builds the examples, after CI caught a gallery example the gate never
compiled. That is the third instance of one pattern and it is recorded as such: *a gate that does
not run what CI runs is not a prediction, it is a hope.* Cut from a commit whose CI run was checked
by id (`35820109743`, fuzz job included).

## v2.11.1 release — 2026-09-22 (tag `v2.11.1`) — four defects, and the gate that had never run

Wire **v12** unchanged; no API change. Ten workspace crates move to 2.11.1.

**The defects were the smaller half.** Every one was found by §12.6's own fuzz targets:

| Target | Defect |
|---|---|
| `presented_call` | the `/a2a` credential gate checked `is_ascii()` on the **encoding** — `"\u0809"` is an ASCII header carrying a non-ASCII principal, so it passed, and `to_header_value` re-emitted it unescaped. A credential this node accepted, it could not re-parse; and an identity field holding arbitrary Unicode admits confusables |
| `replay_trace` | `str::lines()` strips `\r` only before `\n`, so a line ending in a bare carriage return lost it on the round trip. A trace is what a recording replays from — what it decodes to *is* the run |
| `replay_bundle` | `parse_object` trimmed the value **inside** its quotes; the separator had already eaten the opening quote |
| `replay_bundle` | `split_once("\": \"")` found its separator inside an **escaped key** — a key of `"` came back as `\`. Unpatchable in principle: a substring split cannot tell a real delimiter from one inside a quoted string. Replaced by a scanner |

**Why they were still there to find, which is the actual lesson.** The fuzz job runs its twelve
targets **sequentially and stops at the first crash**. `presented_call` is fifth, and it began
failing in the very commit that *added* the trust-edge targets (2026-09-20). So targets six through
twelve never executed at all; `main` was red for **22 consecutive runs**, through every AE commit
and through v2.11.0's tag. Each fix merely let the queue advance to the next defect behind it.

So §12.6's "nine trust-edge fuzz targets" was weaker than it read, in two independent ways:

- **Three never reached their own invariants.** Random bytes essentially never form a parseable
  credential. Measured: a 20,000-input noise pass reached **0 of 7** assertion-bearing targets.
  `presented_call` had asserted round-trip stability since the day it was written and had never
  executed it; `trust_bundle` and `catalog_reply` had no valid seed at all.
- **Most were never run**, per the queue above.

Both closed. Every target now has a valid seed *asserted to parse*, plus mutations and truncations;
a **reachability registry** names each target beside the seed that reaches it and claims
completeness the way the lock-order table does; and the fixint sweep gained bit flips (796 of ~2,500
flipped inputs decode and reach the assertion — a negative result that is actually a result).

**Process.** `RELEASING.md` gains **step 2b — check CI on the branch you are releasing *from***. A
green `check-full` and a green PR do not imply a green `main`: the fuzz job is `main`-only. v2.11.0
was tagged on exactly that gap. v2.11.1 was cut from a commit whose run was checked by id
(`35718298201`, fuzz job included) — the first green `main` since 2026-09-20.

**Also shipped:** §12.1's **AE gallery row** — `procurement_authority`, on the public seam and
reference evaluator, CI-run in `coop/ci_smoke.sh`. Six steps, two of which carry it: an action no
rule covers is *authority not established* and never a denial; and a denied action that ran anyway
is reported as an **enforcement gap** rather than reconciled away. (CLI half only; the Docker half
and the cloud runs are not done, and the plan's row says so.)

## v2.11.0 release — 2026-09-21 (tag `v2.11.0`) — the AE slice's evidence and contract halves

Wire **v12** unchanged. Ten workspace crates on the shared train move to 2.11.0.

Two things this release is about.

**A composed claim finally has an artefact that carries it.** The axis' headline sentence — *a
durable, attributed, cross-domain effect* — had been proved leg by leg and nowhere as a whole: the
receipt tests and the federation tests had zero overlap, and no single artefact held all four
properties, so the sentence could only be believed. The execution record now carries item 1's own
`LocalDurability` and the origin domain, and `states_a_composed_effect()` checks all four legs from
one record. **What that establishes, exactly:** the sentence is *reconstructable*, not *enforced* —
nothing here stops a durable effect being attributed to the wrong principal.

**The evaluator seam stopped being replaceable in principle only.** AE0 §9's negative cases existed
and passed, but each was written inline against `ReferenceEvaluator`'s own rule types, so nobody
else's evaluator could run one of them. They were tests of an implementation wearing the name of a
contract, and the gap was invisible *because* they all passed. `mycelium::ae_contract` states them
once, evaluator-neutrally, and is public because fixtures an adopter cannot see hold nobody to
anything.

**The suite immediately paid for itself.** Putting a real adapter through it found the suite's own
last assumption: it handed every evaluator a revision *string* and expected it back, which an
adapter whose revision is a policy **digest** can no more do than a file can be told its own hash.
Two evaluators had passed without noticing; only a third could surface it.

| Landed | What |
|---|---|
| #340 | `RecordKind` and `Execution` marked `#[non_exhaustive]` **before** the variants arrive |
| #341/#342 | the composed guarantee's gate, and its wiki ingest |
| #343/#344 | AE3 — an evidence record can correct another, and the two crash points through the real journal |
| #345 | AE4 — contract fixtures a replacement evaluator can actually run |
| #346 | `revision_for` / `RevisionBinding` — a real adapter cannot be told its revision |

**Upgrade notes**, both one class — an exhaustive `match` needs a `_` arm: `RecordKind` and
`Execution` are now `#[non_exhaustive]`. For `Execution` that arm **must fail safe**: an execution a
reader does not recognise is *we did not look*, never *nothing ran*; omitting the field is the
stronger claim and belongs only to `None`.

## v3 contracts axis — the composed guarantee gets a gate — 2026-09-21 (unreleased)

The Phase-C audit's **composition finding**, open since v2.9.1, closed. PR #341; record
[`docs/design/composed-effect.md`](../../design/composed-effect.md).

**What was wrong, and why it was hard to see.** The axis' headline claim is *a durable, attributed,
cross-domain effect*, and posture rule 6 says a composed guarantee needs an argument **and** a gate.
The audit found it *"proved leg by leg and nowhere as a whole"* — the receipt tests and the
federation tests had zero overlap. **Every leg worked. Nothing was broken.** No single artefact ever
held all four properties, so the sentence could not be checked, only believed.

**The fix is a join, not a mechanism.** A receipt knows how durable a write was and is then
*returned and gone*; an evidence record knows who asked and *survives*. Neither half could state the
sentence alone. The execution record now carries item 1's own `LocalDurability` — carried, not
restated as a string, because a second spelling of a rung is a second vocabulary — and the origin
domain as a fact rather than an inference from the principal's spelling.

**A correction worth keeping.** I first argued the audit had overstated the finding, because
`EvidenceState` looked like the missing durability rung. It is not: it sits on `AeReference` and
describes whether **the evidence record** reached disk, not whether **the effect** did. Two
durabilities one name apart. `AeEvidence` had zero references to `LocalDurability` — a one-line
check that would have settled it before the argument rather than after.

**Two judgements, both deliberate.** `OnDisk` only: `Buffered` survives a process crash and is lost
to a power failure, and item 1 added it precisely to stop a receipt claiming a durability the node
never established — a composed claim resting on it would re-make that mistake one level up.
Completion only: `Attempted` is the honest answer when a dispatcher did not watch.

**Ordering was the thing to check before committing to the design**, and it resolves: the receipt
rides the `Execution` record, which was always post-effect, while AE0 §5's pre-effect barrier is on
the `Decided` record. Nothing is added in front of an effect.

**The gate was written first and observed failing** (`no method named with_effect_durability`),
because a gate written after the thing it gates tends to describe it.

**What it establishes, exactly:** the sentence is reconstructable from one artefact rather than
believed across four. It does **not** make the composition enforced — item 7 decides attribution at
its own strength and this record carries its verdict. Closing a *"you cannot check this"* finding by
making something checkable is the right scope; claiming prevention would be the same overreach in a
new place.

## v2.10.0 release — 2026-09-20 (tag `v2.10.0`) — the axis auditing itself, and §12.6 closed

Wire **v12** unchanged. Ten workspace crates on the shared train move to 2.10.0.

**The shape of this release is that every new defect in it was found by the gates built to prevent
them** — none was reported from outside. Three Phase-C audit findings v2.9.1 had named were closed,
and five more surfaced:

| Found by | Defect |
|---|---|
| §12.6's trust-edge fuzz targets | a wire `DomainId` bypassing its own validating constructor |
| the same | a replay bundle silently corrupting fields (a newline truncated a witness assertion) |
| auditing those targets' coverage | `count_records` counting a torn tail — a seek past EOF succeeds |
| the same | an unbounded allocation from a raw `u32` length prefix |
| writing AE1's private half against the public seam | the mandate types public but unreachable from another crate |

The last is the one no in-crate test could have caught: `MandateBinding` and `MandateState` were
public types on a public field and absent from the crate's `pub use`, so a **replaceable** evaluator
— the premise the whole AE seam rests on — could receive a mandate and had no way to name it. The
external-adapter test that exists for exactly this premise did not use mandates. It does now.

**§12.6 complete:** the front door, the companion onboarding checklist, the Phase-C adversarial
self-audit, **nine trust-edge fuzz targets**, and migration notes per deprecation — the last now an
adopter-facing page, `docs/guide/deprecations.md`, because the ledger had no home an adopter would
find and three of its seven entries had never been announced at all.

**AE1** (Phase C): the action envelope binds a scoped mandate, and a refused one denies **before**
policy runs. Ordering is the whole point — evaluating after the allow-list lets a matching allowance
turn a refusal into a permit, which for `Superseded` is exactly the laundering item 5 forbids. The
plant is vivid: disable the fence and a permit-everything Cedar policy answers
`verdict: Permit, reason: "a policy allows this action"` for a superseded mandate.

**Upgrade notes**, all one class — an exhaustive `match` needs a `_` arm:

- `CatalogRefusal` gains `StaleRevision { seen, offered }` and is `#[non_exhaustive]`.
- Two deprecations **announced late and now written down**: `system_propose` (deprecated in code
  since 2.1.0 and never in the changelog), and **reading isolation into `cluster_name`**, which
  never provided any. An adopter relying on it to keep deployments apart **does not have that
  separation**; the mechanism that does is a federated domain. Neither is removed.

**A release-process defect found while cutting this.** `RELEASING.md` step 4 hard-coded seven crate
paths; the axis had added three more, so following it literally would have left
`mycelium-commitment`, `mycelium-effects` and `mycelium-sim` on 2.9.1 while its `expect 7` check
passed. The step now derives the list from the tree. Same family as the session's other findings: a
green step that had quietly stopped covering what it named.

## v3 contracts axis — §12.6 complete: the deprecation ledger gets an adopter — 2026-09-20 (unreleased)

§12.6's last deliverable, and the one that had quietly not been done at all. PR #333.

**The requirement.** *"Every §6.6 ledger entry ships with a `CHANGELOG` migration note and a guide
paragraph the day it is deprecated, not the day it is removed — the ledger is a promise to adopters,
and the migration text is how the promise is kept."*

**The audit of the seven entries.** `system_propose` had been `#[deprecated]` in code since
2026-07-15 and appeared in `CHANGELOG.md` **zero times** — two months overdue. `cluster_name`
appeared zero times too, never announced at all. `persisted: bool` had field-addition notes but
nothing saying it was on the removal ledger. There was **no `### Deprecated` section in any
release**, and **no migration, upgrade or deprecation page anywhere in `docs/`**. Four of seven had
no guide paragraph: `BoardConfig` appears nowhere under `docs/guide/`, and the gateway legacy
profile's only migration prose lived in `operations/rbac.md`.

**The structural gap.** The ledger had no adopter-facing home. The plan's §6.6 table is
authoritative but is a planning document, and `ROADMAP.md` — which the plan says *maintains* the
ledger — carried one stale parenthetical naming five of seven, still listing the inferred `>=` ack
as open (resolved 2026-09-15) and predating the `BoardConfig`/`GossipConfig` entries entirely.

**[`docs/guide/deprecations.md`](../../guide/deprecations.md)** is that home: per entry, the
replacement, a concrete before/after, and — stated plainly — **whether the compiler will warn you**.
Mostly it will not: **only 2 of the 7 entries carry `#[deprecated]`**, so the page is the whole
notice for the rest.

**A drift defect it corrected.** `building-on-mycelium.md` told adopters *"the old is
`#[deprecated]`"*. The code does not keep that promise, so an adopter trusting that sentence was
waiting for a build warning that never arrives. Corrected to what the code does.

**The entry worth reading is `cluster_name`.** It never provided isolation, so anyone who used it to
keep deployments apart **does not have the separation they believe they have** — two nodes with
different `cluster_name`s that can reach each other and pass CA admission will gossip. The mechanism
that does separate is a federated domain.

**Known and not fixed:** the four unmarked deprecations. Adding `#[deprecated]` is an API-surface
change (internal call sites need `#[allow(deprecated)]` under `-D warnings`), so it is named rather
than folded into a documentation PR.

With this, **§12.6 is complete** — front door, companion checklist, the Phase-C adversarial
self-audit, nine trust-edge fuzz targets, migration notes. The analysis series keeps running and the
research track stays an explicit candidate rather than a commitment. Log:
[`.log/2026-09-20-measuring-the-risk-list.md`](.log/2026-09-20-measuring-the-risk-list.md).

## v3 contracts axis — measuring the trust-edge risk list — 2026-09-20 (unreleased)

The four parsers §12.6's sweep named and did not cover: one fixed, three measured. PRs #330, #331.

**The journal reader stopped trusting a length prefix (#330).** Two defects in the same four bytes.
`count_records` decided a record was complete by seeking past it and checking the seek returned `Ok`
— but **seeking past the end of a file is legal and succeeds**, so a torn tail counted as a record.
It drives the next append's sequence number while `read_journal_from` stops *at* the torn record, so
the two disagreed and the next append took a seq no reader would hand out — a hole on the
evidence-journal path that exists for compliance export. And a `u32` length prefix went straight
into `vec![0u8; want]`, so one flipped bit asked for up to 4 GiB before anything checked the file
could supply it.

**The rest of the list was measured, not left as a worry (#331).** An unqualified list of "unfuzzed
parsers" reads as a list of vulnerabilities, and this one overstated the risk:
`PublishedRightsHead::decode` cannot panic and has only `#[test]` callers — a reserved surface;
`serde_fixint` under `RightsLedger::open` is live but **survived** 20k mutations (3,808 decodes, no
panic, the unchecked `remaining()` subtraction unreachable); `mycelium-commitment` uses `serde_json`,
so its gap is **provenance rather than a decoder bound** and stays open. `serde_fixint` got a fuzz
target regardless, because a hand-rolled binary decoder reading off disk is the M2 Run-20 shape.

**Two tests proved less than their names, and both tells were numbers that did not move.** The
journal's allocation bound has **no failing test** — deleting it left every assertion passing,
because `alloc_zeroed` is satisfied with lazy zero pages and the outcome is identical either way; the
rule was extracted into a falsifiable `record_fits` predicate and the limit written into the test's
doc comment. And a first `serde_fixint` sweep of 40,000 *random* inputs produced **zero successful
decodes**, so "0 panics" measured nothing until it mutated a valid encoding instead. Every mini-fuzz
seed now asserts its own reachability. Log:
[`.log/2026-09-20-measuring-the-risk-list.md`](.log/2026-09-20-measuring-the-risk-list.md).

## v3 contracts axis — §12.6's trust-edge fuzz gate, and the two defects it found — 2026-09-20 (unreleased)

§12.6's last requirement: *fuzz targets for every new parser on a trust edge (trust bundles, the edge
protocol frames, replay bundle decoding)*. Eight shipped, across all three surfaces. PR #329.

**The discipline, and why it differs from the older input-fuzz gate.** That gate says *no panic on
untrusted arithmetic*. This one says **assert the invariant the parser is relied on for** — byte
conservation for the caller-context frame, signing-field and round-trip stability for every object
whose signature covers bytes rebuilt from the parsed fields. A crash is the easy case; a
wrong-but-well-formed parse is the one that ships.

**A `DomainId` from the wire did not obey its own constructor.** `new` enforces `[a-z0-9.-]` and 253
bytes, and the type documents why — *two ids differing only in case are one domain to a human and two
to a `HashMap`, and the place that difference surfaces is a trust decision.* A derived `Deserialize`
on a newtype validates nothing, so the rule held only for **constructed** ids while most are
**parsed** from partner bytes before verification. It **could not forge authority**; what it admitted
was `Depot`/`depot` read as one domain, a `/` making `federation:{domain}/{principal}` ambiguous, a
newline reaching a pre-authentication log line, and unbounded length. `PrincipalId`/`TermId` shared
the gap. **The general rule now recorded: a newtype whose constructor validates needs a manual
`Deserialize`.**

**`mycelium-sim`'s bundle codec lost fields.** `unquote`'s `trim_matches('"')` stripped every trailing
quote rather than the delimiter, and `quote` never escaped newlines although the reader is
line-oriented — so a newline-bearing value was truncated and the rest dropped silently.
`witness.assertion` is free text, so a replay would check a weaker assertion than the one recorded and
report success: a silent divergence, in the crate built to make divergence loud. Bundles already on
disk read back unchanged.

**Two of the eight exist because the first pass stopped a layer short** — `split_frame` only
classifies, while the envelope JSON, the `via` id and both base64 fields are read before
`verify_bytes`; and `Trace::parse` takes a result as the verbatim line remainder, so
`FsOutcome::decode` is a second parse it never reaches. **And the first bundle invariant was the wrong
one**: `parse → write → parse` passes over already-mangled text, because a stably-lossy reader
reproduces its own mangling.

**Four trust-edge parsers remain unfuzzed and are named rather than omitted:** the journal's
length-prefix framing (`agent/journal.rs:332`), `PublishedRightsHead::decode` and `RightsLedger::open`
(`control/ledger.rs`), and the commitment companion's unsigned offer/award decoding. Log:
[`.log/2026-09-20-trust-edge-fuzz-gate.md`](.log/2026-09-20-trust-edge-fuzz-gate.md).

## v3 contracts axis — three more audit findings closed — 2026-09-20 (unreleased)

The findings v2.9.1 named as *known and not fixed*, now fixed. PRs #325–#327. One changes a public
type, so **the next release is a MINOR**.

**Replica sync stopped claiming what a dropped append broke.** The rung rested on *the store holds
it, therefore its record was appended*; the inbound path applies and **then** appends, and in
`Async`/`Os` that append is a `try_send` which drops on a full queue, answers `Ok`, and had its
verdict discarded at both call sites. A peer under backpressure held a value with no log record and
answered `Persisted`. `WalHandle` now counts skipped appends and a node that has skipped any declines
the rung — it cannot tell which record it dropped, and per-entry tracking is what §1a declined.

**A rotation's overlap reaches the call path.** `acceptable_keys` existed, honoured the window, and
was called from **nowhere in production**; both verifiers used `key_for`. A partner mid-rotation was
refused as `BadSignature` — the refusal reserved for *someone is forging*. The gate asserts three
legs, because a fix that only widened acceptance would be worse than the bug.

**A catalogue revision never goes backwards.** The monotonicity rule was documented on the field and
implemented nowhere; a signed reply carries no expiry or nonce, so a captured one re-granted a
withdrawn export in the client's *view*. `CatalogRefusal` gains `StaleRevision` and becomes
`#[non_exhaustive]` — conflating it into the transport error would have been the refusal-conflation
this module forbids.

**Two remain, and neither is a patch.** The per-partner budget is enforced consumer-side only (the
edge has no slot accounting). And **the composed guarantee still has no gate** — a receipt carries no
principal, the evidence journal no durability rung, and a receipt is never persisted, so closing it
means deciding whether a receipt should be *recorded* rather than merely returned. Log:
[`.log/2026-09-20-phase-c-adversarial-self-audit.md`](.log/2026-09-20-phase-c-adversarial-self-audit.md).

## v2.9.1 release — 2026-09-20 (tag `v2.9.1`) — the Phase-C adversarial self-audit and its findings

§12.6's adversarial self-audit at Phase C exit, over **items 1 + 2 + 7 together** because those three
compose into the plan's first *composed* guarantee. The audit was the deliverable; **four defects in
shipped code** were the result, plus a fifth that surfaced only once the first was fixed. Wire **v12**
unchanged (verified by diff), no API change.

**The composition finding — the audit's actual subject, and NOT fixed.** *A durable, attributed,
cross-domain effect* has **no gate**: the receipt tests and federation tests have zero overlap, and
the Docker federation suite mentions durability zero times. Underneath is a structural gap —
`WriteReceipt` carries no principal, `AeEvidence` carries no durability rung, and **a receipt is never
persisted by the substrate**. So after the fact you can prove who asked, but not what durability the
resulting write established. Posture rule 6, about the plan's own centrepiece. Needs a decision, not
a patch.

**The four fixed.** A durability receipt reported `on_disk` for a **failed** write (tokio's
`sync_data` completes the in-flight write and *discards its error*; `set_requiring_sync` returned
`OnDisk` then applied and gossiped — the codebase already knew, `do_snapshot` flushes for exactly this
reason). A federated partner could **read or cancel any caller's tasks** (only `tasks/send` authorised
against the export). A client could **supply its own caller-context frame** through the raw-emission
routes. And a **remote panic** in item 7's own refusal path.

**The fifth, and the most instructive.** `a_replayed_write_does_not_touch_the_disk` opened a read-only
file to prove no I/O happened; the replay path *does* re-perform a recorded success, so the write was
failing with `EBADF` every run and the swallowed error hid it. **The test measured nothing and
passed.** Fixing the swallow made it fail — working correctly for the first time.

**Two process notes.** A fix requiring a signature for the node-principal mapping was **tried and
reverted** — it breaks a node's own unsigned self envelope on a non-`tls` mesh, which an existing test
asserts; the reason is commented in place. And `make check` **lints** the `sim` feature but does not
**run** its tests, which is why defect 5 was caught by CI rather than locally.

**A correction to the release notes.** v2.9.1 listed `Buffered`'s "survives a process crash" as an
open over-claim; the same flush closes it, so the notes under-sold their own fix. `CHANGELOG.md`
carries a dated correction; the tag is left as the historical record. Log:
[`.log/2026-09-20-phase-c-adversarial-self-audit.md`](.log/2026-09-20-phase-c-adversarial-self-audit.md).

## v3 contracts axis — §12.3–§12.6: the delivery surfaces, and the drift they exposed — 2026-09-19 (unreleased)

Where §12.2 wrote what did not exist, this stretch mostly **corrected what did**: the axis shipped in
v2.8.0/v2.9.0 and the surfaces around it never moved. PRs #313–#321. Shipped: the **federation
runbook** (`docs/operations/federation.md`), §12.3's rows across eight runbooks and the
shared-responsibility matrix, both **decks**, the **front door** (root README, plans index,
philosophy), chapter 17's restructure into its two edges, and the **companion onboarding checklist**.

**The one code change, and not because the plan said so.** §12.3 told the runbook to document receipt,
control-envelope and per-verdict decision counters; going to write those rows, the metric names **did
not exist**, and rev 1.12 says of its own additions *"delivery surfaces only; no engineering change"*.
The real gap sat underneath: the evidence journal records permits *and* refusals (deliberately — *"an
evidence stream that omits its permits cannot support any statement about what an agent was allowed to
do"*), while the **metrics plane counted refusals only**, leaving a dashboard with a numerator and no
denominator. Three counters built on that reasoning — `mycelium_ae_decisions_total{verdict,mapping}`
(the denominator; `permit`+`unmapped` should always be zero, so non-zero is a defect),
`mycelium_control_decisions_total{decision,class}` (`would-hold` counted as itself, never folded into
`proceed`), `mycelium_kv_receipts_total{local_durability}`. Labels are the evidence document's own
vocabulary, asserted equal to its serialised form. Evidence freshness deliberately **not** built: there
is no exporter in the public tree to be behind.

**Ten drift findings**, every one from reading prose against the thing it describes. Receipt rung 1
given as two variants (four in code); `Buffered` omitted from rung 2; the receipt types described as
not yet landed; federation called transport-less after v2.8.0; `CLAUDE.md` listing two closed gaps as
open and missing the seam rule; `deployment.md` calling the per-write receipt a future plan item; the
root README mentioning receipts and federation **zero** times; the plans index six items behind; the
guide sized at 17 chapters (25); `philosophy.md` asking for *"these three questions"* above five. And
**chapter 17 contradicted itself** — one paragraph said the gate's last caveat was closed, the next
said the gate was not met. One finding was mine: chapter 18's first draft mis-stated what `kv().set`'s
bool proves.

**§12.5 was already done** — Property 8, litmus tests 4 and 5, and epistemic symmetry extended to
evidence all existed; only the question count was stale. Recorded so nobody redoes it.

**The companion checklist found four gaps and closed all four**, including `mycelium-effects` having no
runnable demonstration anywhere — closed by `destination_commit.rs`, which is a gate (planting the
dedup row so it survives a failed business change fails it by name). `mycelium-sim`'s missing operator
row closed *by stating the absence is deliberate*.

**The honest gap, unchanged:** nothing in CI enforces doc-vs-code accuracy; the three lints are
operator-run. Log:
[`.log/2026-09-19-section-12-3-to-12-6-delivery-surfaces.md`](.log/2026-09-19-section-12-3-to-12-6-delivery-surfaces.md).

## v3 contracts axis — §12.2: the seven how-to chapters and the axis vocabulary — 2026-09-19 (unreleased)

§12.2 ties each how-to chapter to **its item's release gate** and the concept vocabulary to **its item's ADR**.
Every one of those gates had passed in v2.8.0/v2.9.0, so this was debt already incurred, and S2's phase-exit
condition (no gap in the matrix) makes it a Phase C item rather than a tidy-up. Shipped as PRs #304, #305, #306,
#311: **chapters 18 contracts & receipts · 19 replay & simulation · 20 authorising actions at the gateway ·
21 mandates · 22 stability & control · 23 knowledge · 24 commitments**, plus eight concept-pair paragraphs and ten
glossary rows in `00-concepts.md`, `ReceiptError` in the error taxonomy (it was absent entirely), six cookbook
recipes, and chapter 14's one-workload comparison of the three coordination models.

**The axis's thesis got written down for the first time:** *refusals are typed by what the caller should do next*
— a refusal a retry loop would consume is a different type from one it would not (`MandateSuperseded` ≠ `Conflict`,
`Indeterminate` ≠ `Deny`, `InsufficientEvidence` ≠ `Rejected`, `DeliveryUnknown` ≠ "nothing happened").

**Seven drift defects, found by checking prose against code**, six of them pre-existing: `00-concepts.md` gave
rung 1 as two variants (four in code) and omitted `Buffered` from rung 2 entirely; it still said the receipt types
*would land*; the guide index called federation "contract only, no transport yet" (shipped v2.8.0); `CLAUDE.md`
listed the federation transport and scheduler seam as open and lacked §12.2's seam rule. The seventh was **mine**:
chapter 18's first draft said the bool from `set` means "applied here, now" — the code says *queued for gossip*,
and its `false` is ambiguous. Writing from memory also produced three wrong signatures and missed
`retry_with_receipt`; from chapter 19 onward every name was verified **before** writing, which caught two errors in
my own research notes (`Stale` not `TooStale`; `Divergence` is a struct).

**A process correction mid-flight.** Chapters 21–24 were first opened as four *stacked* PRs (#307–#310), meaning
five full CI runs for Markdown-only changes with four competing for runners. Collapsed into one (#311), the four
closed. Batching, not stacking, for docs.
**The honest gap:** nothing in CI enforces any of this. `/doc-coverage` and `/wiki-lint` are operator-run skills,
not gates, and six pre-existing corrections after two releases is what silent drift looks like. Log:
[`.log/2026-09-19-section-12-2-guide-chapters.md`](.log/2026-09-19-section-12-2-guide-chapters.md).

## v3 contracts axis — item 4 PR 5: admission control at the companions' queues, reported — 2026-09-18 (unreleased)

The last row of item 4's table (`docs/design/adaptive-stability.md` §9). The tuple space's watermark already
refused a `put`; the refusal was a silence — `put_total`, `take_total`, `hot_total` and no count of what was
turned away. Now each stage counts `rejected_total`; `TupleSpace::admission` reports it beside `admitted` and
`taken` at the primary (`None` elsewhere: the depth RPC's fixed encoding cannot grow, and §3 says the primary
owns the deficit); the metrics writer publishes it. The blackboard had no bound; it gets `BoardConfig.high_watermark`
(`None` = unbounded), refusing `post` with a counted `Backpressure` that crosses its RPC as itself — a free
status code — while replication and replay never refuse. Both are Tier B: self-imposed, two-step, not hard.
**The mechanism was there; the report was the gap.** With this, item 4 is complete: PRs 1, 2, 3a, 3, 4a, 4b, 4c,
5. Example `examples/admission`, runbook `docs/operations/admission-control.md`, §6.6 names `BoardConfig`. Log:
[`.log/2026-09-18-item4-pr5-admission-control.md`](.log/2026-09-18-item4-pr5-admission-control.md).

## v3 contracts axis — item 6: the scheduler seam sized (a design note) — 2026-09-18 (unreleased)

The inventory's coverage map claimed the kernel owns `select!` readiness; no site is routed, and CN2 measured the
consequence. §3.1 now says what a `select!` wrapper could do (detect an interleaving change) and cannot (reproduce
one), and what reproduction needs: the seams' replay arm advancing tokio's *paused* clock by the recorded wait
instead of yielding — so tasks keep their relative order on a `current_thread` runtime — and a network seam for
any node with peers. The first arm is gated by a test that already exists: CN2's pin flips when it lands. Log:
[`.log/2026-09-18-item6-scheduler-seam-design-note.md`](.log/2026-09-18-item6-scheduler-seam-design-note.md).

## v3 contracts axis — item 6 PR 7: the replay corpus and its gate — 2026-09-18 (unreleased)

The last PR of item 6's sequence. A checked-in bundle — scenario A, thirteen effects, at
`mycelium-core/tests/replay-corpus/scenario-a-wal-snapshot/` — and the gate that makes it mean something: in every
`--features sim` run, the committed schedule must replay on the running machine without divergence **and** a fresh
recording there must ask for the same effects in the same order, so a change in what the code does fails the
suite until the corpus is re-recorded on purpose (`MYCELIUM_RECORD_CORPUS=1`, reviewed as a diff of the text
trace). This is the claim D14 makes — *a bundle is a reproduction artefact* — tested across machines for the first
time; PR 4's round trip proved the format in one process. `Build::current` records what is honestly known at
compile time (`unknown` for the compiler: no build script). *Not built:* the minimiser and a replay binary; the
corpus has one entry — B and C are pure sweeps with no kernel trace. Log:
[`.log/2026-09-18-item6-pr7-replay-corpus.md`](.log/2026-09-18-item6-pr7-replay-corpus.md).

## v3 contracts axis — item 4 §7: the profile ladder through every governor, and the rollout runbook — 2026-09-18 (unreleased)

The Phase E item "shadow-mode rollout documented" — and the wiring the document needed to be true. Writing
`docs/operations/control-profiles.md` against the code showed §7 held for the membership governor only: 4b's
spacing and settling and 4c's rights refusal acted under every profile, so "`Legacy` leaves today's governors
untouched" and "`Observe` changes nothing but counts" were false for three of four. Now the tuning gate skips its
contract under `Legacy`, counts under `Observe`, holds under the enforcing profiles (its own copy of the profile,
fanned out by `set_control_profile`); the opacity release spacing answers *spaced or not* and the profile says
whether that is a hold; the provisioner refuses only under `EnforceAllocated` — a rights-backed bound is Tier C —
counting `rights_would_refuse` otherwise and still recording the rejection. **`Legacy` is the ladder's own
witness:** scenario C's "settles" sweep runs under the enforcing profiles, and under `Legacy` the same breakers do
nothing and the schedules flap; under `Observe` they flap while counting. 4b's defaults thereby move back to
opt-in, which is what "untouched" meant. The counters keep their names across the ladder; `GovernorSnapshot.profile`
says which reading they are. Over the gateway: `GET /gateway/govern` carries a `control` block (the profile and
its tripwires) and `POST /gateway/govern/profile` steps the ladder by name (`govern:write`; an unknown name is
`400`, never read as `legacy`) — the Ops Console's panel for this concept. Log:
[`.log/2026-09-18-item4-control-profiles-rollout.md`](.log/2026-09-18-item4-control-profiles-rollout.md).

## v3 contracts axis — CN3: the award under the acceptor's mandate — 2026-09-18 (unreleased)

`commit_award_under_mandate`: item 5's fence at the award. The mandate must be the acceptor's own — another
holder's is refused by name — and the requirement's `ResourceAuthority` must authorize `accept` for it at its
installed epoch, now: a stale holder's award, minted under an epoch the resource has moved past, is
`Mandate(Superseded { installed, presented })`, and the check runs **before any write**, so a refused award leaves
nothing behind. A passing check commits linearizably. The plan's "skipped, not faked, where item 5 is absent"
needed no skip: item 5 is in this tree, the negative case runs in CI, and a deployment without mandates uses the
mandate-free linearizable commit — a check not called rather than one faked. With CN1–CN3 the commitment
companion's three steps are landed (CN2's replay half pinned as a gap); the CN-gate — §13.3's four-arm harness —
is the research track's, not this session's. Log:
[`.log/2026-09-18-cn3-award-under-mandate.md`](.log/2026-09-18-cn3-award-under-mandate.md).

## v3 contracts axis — CN2: the linearizable award, and a gap pinned — 2026-09-18 (unreleased, partial)

`commit_award_linearizable`: the award through a consensus round on `cn/{req}/award` — of two declarers racing
exactly one commits and the other is `AlreadyAwarded` with the committed award; a round with no commit is
`AwardUnknown`, never silence. The double-award witness is an **explicit interleaving** (the kernel has no
scheduler seam): two declarers plan, then commit — the plain KV path lets both commit and LWW keeps one silently;
the linearizable path refuses the second. **The replay half is not landed, and is pinned as a gap rather than
skipped:** a whole-node recording of a linearizable award — same identity for both runs — diverges at seq 7,
the recording holding the membership governor's `rng jitter` draw where the replay holds the round's
`consensus/defer` timer. That interleaving is the scheduler's, the inventory's one unrouted row; a single
task's effects replay (scenario A, the corpus) and a node's do not. The test asserts the divergence and names
it, so the seam's arrival flips it into the claim. (A first attempt diverged earlier on a fresh port: the gossip
shard hashes the key and the key carries the node id — a different node, not nondeterminism.) `mycelium`
re-exports `sim_seam` under `sim` for companions. Log:
[`.log/2026-09-18-cn2-linearizable-award.md`](.log/2026-09-18-cn2-linearizable-award.md).

## v3 contracts axis — CN1: the commitment companion — 2026-09-18 (unreleased)

The third coordination model of the epoch (`docs/plans/v3-contracts-axis.md` §6.9, §13.2), built as the plan
insists — a composition, not a subsystem: `mycelium-commitment`, five records with a mechanism each on the public
API. Announce is a declarer-owned head (`cn/{req}`); offers an `append` stream; the award the tuple-space
election's lowest-participant rule, written **with a receipt** under `cn/{req}/award` — *an award that is not a
receipt is a KV write with a hopeful name* (D39); reports an `append` stream whose outcome may be *unknown*;
assessments Ed25519-signed by whoever has standing, unsigned meaning unproven. **One award per requirement:** a
second is refused (`AlreadyAwarded`), never written over — the local half of the rule; two declarers racing to the
key is CN2's replay witness. `NoOffers` and a late offer are visible states, not retries; nobody assigns another
participant's obligation. The gallery entry re-runs the redistribution workload the tuple-space and blackboard
examples run (`examples/redistribution_cn.rs`, CI job `commitment`), so the three models meet on one workload.
*Not done:* CN2, CN3; participants are named, not authenticated. Log:
[`.log/2026-09-18-cn1-commitment-companion.md`](.log/2026-09-18-cn1-commitment-companion.md).

## v3 contracts axis — item 4 PR 4c: the provisioner against the rights ledger — 2026-09-18 (unreleased)

The ledger's first live user (`docs/design/adaptive-stability.md` §9 row 4c), in `mycelium-wasm-host` on the
public API only. `Provisioner::with_install_rights(ledger, holder, resource, signing_key)`: before an
`Installing` reservation the round asks the ledger whether the node holds units for one more concurrent install
(**a unit in flight is as consumed as one serving**), and refuses otherwise — counted on the provisioner and
**recorded** in the ledger as `admission.rejected`, off the synchronous admission path, because
`provision_round` must not block on a journal write; a busy ledger is a refusal, not a wait. The head goes into
`rights/head/{holder}` as `PublishedRightsHead { head, signature }` on attach and after every refusal; unsigned
means unproven (`verify_published_head` is `false` for it). **Decided:** the provisioner never allocates to
itself — an unallocated node is refused every install, visibly; per-install rights and state transitions on the
right are not modelled, consumption is the provisioner's own count against the units held. *Not shown:*
revocation or expiry while an install is live (the next round refuses; the live install is not torn down).
Lock-order table row 37 (`InstallRights::ledger`, `try_lock` on the admission path, never held with `hosted`).
Log: [`.log/2026-09-18-item4-pr4c-install-rights.md`](.log/2026-09-18-item4-pr4c-install-rights.md).

## v3 contracts axis — item 6 PR 6: replay scenario C, the interacting governors — 2026-09-18 (unreleased)

The combined-feedback harness (`docs/design/adaptive-stability.md` §5, D19: *replay stage 6, built once, reusing
the governors' pure decision functions*), `src/control/scenario_c.rs`, test-only. A schedule sweep in scenario B's
shape — 48 schedules × 2 profiles, invariants after every step, a witness, a size assertion — over the shipped
decisions of all three governors with their real spacing, settling and hysteresis, coupled through a small plant.
**With the breakers on every run settles; with them off, schedules flap and chatter; the decisive rule holds under
`EnforceLocal` and not under `Legacy`.** Three findings on the way: the objective belongs on *releases* (a release
re-shed 100 ms later is the decisive rule, not a flap); a first-only violation report masks (the witness said
"no flap" while the same schedules chattered); and a plant that is not caught is a finding — removing the
hysteresis in shipped code did not fail the sweep, because the 1 s release spacing alone bounds the release rate,
so the sweep does not prove hysteresis is load-bearing. *Not shown:* loops oscillating together while each is
stable alone. Phase E's "combined-feedback scenario green" is met in the bounded sense above. Log:
[`.log/2026-09-18-item6-pr6-scenario-c.md`](.log/2026-09-18-item6-pr6-scenario-c.md).

## v3 contracts axis — item 1 PR 7: gateway/SDK receipt parity — 2026-09-18 (unreleased)

The last PR of item 1 (`docs/design/contracts-receipts.md` §5, landed note). The two consensus-backed gateway
verbs — `POST /gateway/overlay/consistent/set` and `/gateway/consensus/cross_group_propose` — answer
`"local_durability"` beside `"persisted"`, in the receipt's own names (`LocalDurability::tag`: `on_disk` ·
`buffered` · `not_configured` · `failed`, with `"local_durability_error"` only for the last), and both SDKs read
it (`mycelium-py` `CommitResult.local_durability` / `.on_disk`; `mycelium-ts` `CommitResult.localDurability` and
the `LocalDurability` type; absent → `None` / `null`).

**One mapping, not two.** The handlers now go through the same `receipt_from` as `cluster_propose_receipt`
(made `pub(crate)`), so HTTP cannot drift from Rust; `"persisted"` is read off the `ConsensusResult` *before*
it becomes a receipt, so the old field is exactly what it was and its regression-floor pin does not move. The
live test shows the collapse undone — a node *without* persistence answers `persisted: true` **and**
`"not_configured"`; one with `Flush` persistence answers `persisted: true` **and** `"on_disk"`. *Not shown
live:* the `failed` shape (a stopped writer is not inducible from outside) — unit-pinned in `commit_json`'s
test. A gate finding on the way: `mycelium-py/tests/test_commit_result.py` was node-free since 2026-09-05 and
never in CI's explicit pytest list; it is now. Log:
[`.log/2026-09-18-item1-pr7-gateway-sdk-parity.md`](.log/2026-09-18-item1-pr7-gateway-sdk-parity.md).

## v3 contracts axis — item 4 PR 4b: the two local-input governors through the contract — 2026-09-18 (unreleased)

The tuning governor and the opacity gate (`docs/design/adaptive-stability.md` §9 row 4b): spacing and settling
only — the confidence predicate does not apply to a view that is this node's own. **Tuning** (`gate_at` /
`acted_at`, pure in time): the reconcile step is the *knob's readback* — the tuner already passes `cur`, so an
action is pending until a later gate sees the knob at the applied value, or the settle timeout passes and it
settles as *unknown* (counted, warned); a value equal to `cur` is not an action; a change inside the spacing is
held. `acted` is separate from `gate` so a policy-rejected value runs no clock. Timing comes from
`start_cluster_tuner` (two ticks each); both `0` by default, which is the old gate. **Opacity:** only the
**release** is spaced (`OpacityHint.release_spacing_ms`, 1 s) — going opaque is protective shedding and the
decisive rule says it is never held; no settle state, because the loop's own input is the effect channel.
Tripwires: the three snapshot counters and `opacity_releases_spaced`. *Not shown:* the tuner loop end to end and
the combined behaviour (PR 6). Log:
[`.log/2026-09-18-item4-pr4b-local-governors.md`](.log/2026-09-18-item4-pr4b-local-governors.md).

## v3 contracts axis — item 4 PR 1: the adaptive-stability ADR — 2026-09-18 (unreleased, PR #273; #270 was auto-closed by the stacked-PR trap)

Record `docs/design/adaptive-stability.md`; reservations `rights/head/{holder}` (namespace table +
`kv_ns::RIGHTS_HEAD`). The record behind the governors, and the one D30 makes the resource-accounting slice wait
on. **Three promises kept apart** — hard bounds · stability objectives · service objectives — each with a strength
in the guardrails tiers (D17), and a hard bound is `HardPrevention` *only* with exclusive, durably accounted
rights; otherwise it is a convergence target. **The decisive rule:** uncertainty holds speculation and routine
scale-down, never protective shedding or rescue from zero — `ViewConfidence` per input, four action classes, a
pure swept predicate at PR 2. **The ledger's shape decided:** rights cannot live in gossip KV (evaporation issues
a right twice; LWW overwrites one); a node-local fsynced never-gossiped journal in the `EvidenceJournal`'s shape,
heads only in the medium, native units, item 5's term identity, five states incl. `unknown`, persisted before
acting, never reclaimed on discovery loss, `admission.rejected` first-class; exclusivity by allocation. One owner
per actuator and per deficit, named; loop-breakers named (existing `gate`/cooldown kept, spacing/settling new,
combined test = replay stage 6); depth signals consumed (D18); four profiles, shadow first. Log:
[`.log/2026-09-18-item4-pr1-adaptive-stability-adr.md`](.log/2026-09-18-item4-pr1-adaptive-stability-adr.md).

**PR 2 (#271) — the contract types, `src/control.rs`.** Pure decisions, no governor changed. `ActionClass` ×
the rule `holds_on_uncertainty`, written twice (rule and hand table) and pinned; `decide` over the real, public
`ViewConfidence` with a named `Uncertainty` — **an isolated node is uncertain, not fresh**, so WP5's
`staleness_known` is now consequential; four profiles with `Observe` as a distinct `WouldHold` decision rather than
a flag; `ControlSpec`, a stable `ActionId`, and spacing/settling as pure checks. Both decisive properties verified
by planting their inversion. Log: [`.log/2026-09-18-item4-pr2-control-contract.md`](.log/2026-09-18-item4-pr2-control-contract.md).

**PR 3a (#272) — the journal split, a pure move.** The AE evidence journal's mechanism lifted to
`agent::journal` with `EvidenceJournal` a thin profile over it; every public name unchanged, the replay stream
still `ae/journal` (pinned). Gated on its first user's features until the ledger — its second, ungated user —
lands in PR 3: the first cut ungated it early and the no-default-features clippy failed on every item, the
dead-code trap doing its job. **Found on the way: the forbidden-call check's skip swallows the rest of an
enclosing block when `#[cfg(test)]` sits on an inner item** — a test-only method inside `impl Journal` hid
`append`'s timeout and the baseline dropped 5 → 4 for a file whose sites had not changed. Test-only helpers now
live in a top-level `#[cfg(test)] impl`; the script's header records the rule. Log:
[`.log/2026-09-18-item4-pr3a-journal-split.md`](.log/2026-09-18-item4-pr3a-journal-split.md).

**PR 3 (#274) — the rights ledger, `src/control/ledger.rs`.** On the ungated journal (`sha2` unconditional
from here). `Right` with five counted states; **persist-then-apply** — a record that did not reach disk
allocates nothing; **no method takes a peer set**, so discovery loss cannot reach the ledger, a live term is
`Duplicate` to reissue and a released/expired/revoked one is reissuable (both halves tested — passing only the
first would never free anything); `admission.rejected` a journal record beside completions; a fail-closed open
over an undecodable journal; `RightsHead` over `serde_fixint` bytes, `tls`-gated verify. Reserve → act →
reconcile and publishing the head are PR 4. Log:
[`.log/2026-09-18-item4-pr3-rights-ledger.md`](.log/2026-09-18-item4-pr3-rights-ledger.md).

**PR 4a (#275) — the membership governor through the contract.** The first shipped governor wired to the
predicate: a pure `classify` (join at 0 → rescue, join below `min` → **deficit fill**, leave over `max` → routine
scale-down, drain → no class), the predicate per pass on the real `ViewConfidence`, settling observed against the
group's membership, the cooldown as spacing unchanged in meaning; the node's profile as an atomic
(`set_control_profile`, default `Legacy` — nothing changes until an operator opts in) with the `Observe`
tripwire. **The ADR gained a fifth class by dated amendment**: the governor's own primary action fitted none of
the four, and by cost it is a rescue. The governor's `fastrand`/`Instant`/`sleep` sites routed through the seams
(baseline 6 → 1). PR 4 split into 4a/4b/4c in the ADR's table. Log:
[`.log/2026-09-18-item4-pr4a-membership-governor.md`](.log/2026-09-18-item4-pr4a-membership-governor.md).

## v3 contracts axis — item 1 PR 5: the effects companion — 2026-09-18 (unreleased, PR #277)

`mycelium-effects/` — the fourth receipt, *destination commit*, as a reference destination; the substrate never
provides it (`docs/design/contracts-receipts.md` §2 rule 1). `EffectDestination::apply` commits the `operation_id`
dedup row and the caller's business change **in one transaction** and returns
`DestinationCommit { destination, dedup: Fresh | Replayed }`. `SqliteDestination` is the reference: `rusqlite`
bundled and quarantined to the crate (its own CI job); `IMMEDIATE` transactions so racing appliers serialise at the
database and exactly one is `Fresh`; the business change is a handler run *inside* the transaction, so a failure
rolls back whatever it wrote and leaves **no dedup row** — a retry starts clean. Refusals say what is true
afterwards: `Conflict` (same id, different content — the first version stands), `Failed` (nothing committed),
`DeliveryUnknown` (the deadline passed; the apply is not cancelled and may still commit — the retry resolves it as
`Replayed`). The effect's hash is the vocabulary's own `content_hash(operation_id, payload, false)`. Justified
where the exactly-once tracker overlay was declined (§6): beyond both companions, at the resource, coupling
neither. Seven tests; dedup and rollback-on-failure verified by planted absence. Log:
[`.log/2026-09-18-item1-pr5-effects-companion.md`](.log/2026-09-18-item1-pr5-effects-companion.md).

**PR 6 (#278) — the tuple-space consumer with effect recovery** (`tuple_consumer`, feature `tuple-space`).
`take` → commit the effect at the destination under `tuple/{ns}/{stage}/{id}` → *then* `ack`/`complete`. The
tuple id survives lease expiry and a WAL restart, so a re-delivered item is a new attempt of the same operation
and replays; a refused effect is **not** acknowledged and the item stays in flight (`Conflict` surfaced as the
poison pill it is — dead-lettering is the caller's). Three live tests; the one that pins the *ordering* is the
refused-effect-then-restart case, because the replay test alone would pass for a consumer that acked first.
Log: [`.log/2026-09-18-item1-pr6-tuple-consumer.md`](.log/2026-09-18-item1-pr6-tuple-consumer.md).

## v3 contracts axis — item 3: the knowledge layer, complete — 2026-09-18 (unreleased, PRs #254–#255, #264–#267)

Record `docs/design/knowledge-layer.md` (PR 1, the ADR, #243, 2026-09-17); code `src/knowledge/` behind `tls`.
**PR 2 (#254)** the four typed records — claim · observation · **assessment** (judging is not recording) ·
acceptance decision — with six link kinds and `RecordId { issuer, digest }` so a retraction is checkable
without a fetch; **PR 3 (#255)** the store: heads in the gossip medium, records in an authorized store, so
LWW moves a pointer and cannot erase a competing statement. **PR 4 (#264)** evidence-aware resolution
(`resolution.rs`): wraps `resolve_for_caller` after the native gates; `ReleaseId` binds evidence to one
release; independence is a **reader-configured control group**, never inferred; four outcomes with reasons;
`filter_accepted` *filters and never reorders*, so evidence decides eligibility and the router decides choice.
**PR 5 (#265)** expiry and correction (`correction.rs`): a `DependencyIndex` so a retraction *reaches* what was
derived from it; **withdrawing a basis is not withdrawing the conclusion** — `BasisWithdrawn` is a fact about
support, not a verdict, because issuer A has no standing to retract issuer B's record; nothing is deleted;
expiry needs no timer. **PR 6 (#266)** the semantic gate (`gate.rs`, `make gate-knowledge`, its own CI step):
misleading evidence cannot *erase* a conflicting observation (fifty supporters do not bury one challenge —
there is no vote), cannot *refresh* expired evidence, cannot *confer* authority — plus two positive controls,
because a refuse-everything resolver passes all three negatives (checked by planting it); the example
`examples/knowledge_layer.rs` closes by listing what it does not show. **PR 7 (#267)** adapters: a
`TraceEvent` as an **observation** with `derived_from` only where asserted (the batch form is link-free by
construction — §6 made structural); a verified AgentFacts document as a **claim** issued by its signing key,
which resolution can never count as evidence. `mycelium::hlc` made public (additive) on the way. **The
behavioural claim — that evidence-aware selection picks better providers — is research-track (§13) and
unmade.** Log: [`.log/2026-09-18-item3-knowledge-layer.md`](.log/2026-09-18-item3-knowledge-layer.md).

## v3 contracts axis — item 2: federated domains, PRs 1–7 — 2026-09-17 (unreleased, #242, #245–#253)

Record `docs/design/federated-domains.md` (the ADR, #242); code `src/federation.rs` +
`src/federation/{catalog,call,gateway,session}.rs`. A domain is **one independently admitted gossip mesh**;
federation connects *exported services* and **never joins transports**. **PR 1 (#245)** the enforced domain
profile, the two-mesh harness (*scaffolding* — it stands in for a transport that does not exist), and
`scripts/check-kv-namespaces.sh`: the plan claimed a namespace sweep existed and it did not, so D7 (no
`federation/` KV prefix) became a checked invariant in `make check` + CI. **PR 2 (#246)** `DomainId`,
`DomainDescriptor`, `DomainPolicy`, `TrustBundle`/`PartnerTrust` with rotation and revocation, pinned test
vectors; a **length-prefixed canonical encoding with domain-separation tags** rather than canonical JSON (we
own both ends, so the encoding has no freedom left in it), so a policy signature can never authenticate a
descriptor, and **the trust bundle decides which key** — a self-signed descriptor is not authorised by being
internally consistent. **PR 3 (#249)** `filtered_catalog` + `RemoteResolver` — exported services only.
**PR 4 (#250)** `FederatedCaller` / `verify_federated_call`: **D5, the invocation edge *is* A2A**, not a
second protocol; **D6, reuse OIDC's cryptography, never its trust**. **PR 5 (#251)** `GatewayPool`,
two-gateway operation, budgets, outcomes. **PR 6 (#252)** `PartnerLink` — partition, reconnect (a
`Refreshing` state: *reconnected is not ready*), revocation, rotation. **PR 7 (#253)**
`examples/federated_domains.rs` — runs the lifecycle and **ends by printing what it did not demonstrate** —
and the guide. **Not done, stated plainly:** the federation transport. The record's release gate (*prove from
membership tables, consensus state and traces that the meshes never merged*) is not met, because there is no
transport to sever. Wiki: [security](security.md) → *Federated domains*.

## v3 contracts axis — item 2 PR 8: the federation transport, first arm — 2026-09-18 (unreleased)

The sentence every item 2 page ended on — *no bytes cross a network* — retired for the simplest topology.
`src/federation/edge.rs` (provider side): the credential's wire form `PresentedCall` (one JSON header,
`x-mycelium-federation-call`, on an ordinary `tasks/send` to `/a2a` — D5 kept: no second protocol),
`FederationEdge` attached with `GossipAgent::with_federation_edge`, and `GET /federation/catalog` under a
credential for the reserved export `federation.catalog`, answering the *filtered* list. **Authenticate at the
auth layer, authorise in the handler:** the header is read before the body, so `verify_federated_credential`
(new in `call.rs`: signature, lifetime, expiry, skew — binds nothing) runs in the gateway's optional-auth layer
and `verify_federated_call` (export binding + grant) runs once the body has named the skill. The provider is
told `federation:{origin}/{principal}` (`federation_principal`, re-exported). Two rules the HTTP layer owns
(`src/agent/federation_http.rs`): **present-and-refused is a refusal, never anonymous** — otherwise a revoked
partner would quietly become an anonymous A2A caller — and two identities on one request is a 400.
`src/federation/client.rs` (consumer side): `FederationClient` sequences `PartnerLink` → `RemoteResolver` →
`GatewayPool` → HTTP, two microsecond critical sections around the await (lock-order rows 38–39); a silent
gateway goes through `on_gateway_silent`, so at-most-once is `DeliveryUnknown` and repeatable fails over — over
real connection refusals. **The test** (`lib_tests.rs::federation_transport`): two meshes, domain A's first node
a gateway with the A2A and federation edges; B discovers (`["demo/whoami"]`, not the ungranted `demo/secret`),
calls, and the provider answers with B's principal; eight plants at the gateway (wrong-export credential,
ungranted, forged key, tampered field, malformed header, bearer+credential, streaming, call-credential-for-the-
catalogue) each refused **with no dispatch** (a counter on the provider); revocation at the edge mid-session
refuses the next call at the auth layer; then the PR 1 harness's `assert_never_merged` — lifted to a free
function over borrowed nodes — runs *after* bytes crossed. Streaming under a credential is refused: federated
calls are unary (§5), and extending D5 to `tasks/sendSubscribe` is a revision, not a silent extension. **Not
claimed:** the release gate. Its traces leg and its partition choreography (sever every link, keep working
locally, change permissions mid-partition, reconnect) are PR 9; the catalogue reply is unsigned in this arm;
the test's edge is plain HTTP (production: behind `gateway_tls`). Log:
[`.log/2026-09-18-item2-pr8-federation-transport.md`](.log/2026-09-18-item2-pr8-federation-transport.md).

## v3 contracts axis — item 2 PR 9: the release gate's choreography — 2026-09-18 (unreleased)

**The gate is met in its in-process form.** `lib_tests::federation_transport::the_release_gates_choreography_over_the_transport`:
every node under the enforced profile (§9: TLS, SWIM off), two meshes under two auto-generated CAs (two
`auto_cert_dir`s), the provider on a plain node so the gateways are replaceable. Discover, invoke, a consensus
round in each mesh; lose the only gateway — at-most-once and repeatable both `DeliveryUnknown` (the latter having
tried every gateway: *unknown, not failed*), discovery cannot refresh, link `Down`; keep working locally —
gossip and consensus on both sides; change the grant while no link exists; bring up the replacement gateway;
reconnect — refused until discovery refreshes, and the refreshed catalogue *is* the changed grant; repeatable
fails over past the dead gateway, at-most-once pays it once (**finding:** the pool keeps no health memory by
PR 5's design, so retirement is explicit — `GatewayPool::retire` / `FederationClient::retire_gateway`, new);
a credential issued before the partition is honoured, an expired one refused (*issued authority lasts only to
its expiry*). **Three legs, before, during and after:** the membership tables; the `cap/ grp/ sys/ consensus/`
namespaces over keys and values, plus explicit `consensus_get` cross-checks; and the **connection tables** —
`GossipAgent::connected_peers` (new, the transport's own record beside `peers`, membership's belief) — the
*traces* leg, with the harness's non-vacuity test extended to show a merged pair in that table. A rogue node
holding B's CA cannot join A (timing-bounded; gw2's join is the positive control). **Three findings** recorded in
testing.md: TLS formation needs fast pings; a secure gateway waits for the provider's caller-context marker,
which gossips like the capability; an export needs a skill behind it. **Not claimed:** process isolation and a
real network severance (the Docker suite, PR 10), a signed catalogue reply, SDK verbs. Log:
[`.log/2026-09-18-item2-pr9-gate-choreography.md`](.log/2026-09-18-item2-pr9-gate-choreography.md).

## v3 contracts axis — item 2 PR 10a: the signed catalogue reply — 2026-09-18 (unreleased)

Row 10's smallest part. A catalogue over plain HTTP was attributable only to the gateway asked (PR 3's
*observation*); now `CatalogReply` carries **the partner it was filtered for**, an issue time and a signature
under the domain's key over a tagged canonical form (`TAG_CATALOG`, the same length-prefixed shape as the
descriptor and policy), so a reply cannot be replayed to another partner or minted by a gateway without the key.
`FederationEdge::with_signing_key` signs; `FederationClient::with_partner_key` requires — unsigned, forged,
wrong-domain or misaddressed is `ClientError::Catalogue` and the link stays `Down`. Bindings are checked before
the signature so a refusal names the cheaper reason. Freshness deliberately stays the resolver's window: a
signature says who issued a list, not how long to trust it. Plants: a client holding the wrong key is refused
`BadSignature` before anything is relied on; a requiring client against an unkeyed edge is `Unsigned`; a
re-addressed reply breaks its own signature. The wire gained three fields, `signature` optional, so a PR 8
reader still parses. Remaining in row 10: the Docker two-mesh suite and SDK verbs. Log:
[`.log/2026-09-18-item2-pr10a-signed-catalogue.md`](.log/2026-09-18-item2-pr10a-signed-catalogue.md).

## v3 contracts axis — item 2 PR 10b: the two-mesh Docker suite; the gate met without a caveat — 2026-09-18 (unreleased)

PR 9 met item 2's release gate in one process and named what that left open: a shared address space, and a
"severance" that was a shut-down gateway. This suite removes both — one container per node, and the federation
link cut with `docker network disconnect`. `make test-federation` (CI job `federation`);
`examples/federation_node.rs` (one binary, four roles: `member`, `gateway`, `probe`, `keys`),
`docker/docker-compose.federation.yml`, `docker/Dockerfile.federation`, `tests/integration/run_federation.sh`.
Two meshes of real containers under two auto-generated CAs, the enforced profile everywhere; every assertion
reads a node's own `/fed-admin/tables` (membership, the `cap/ grp/ sys/ consensus/` entries with keys *and*
values, `connected_peers`), never a log line. **Two design findings, both caught by reading the first draft
rather than by running it.** *Four networks, not three:* the runner severs the link by disconnecting the probe
from `edge`, so it must not drive the probe over `edge` — the probe also sits on `control`, which only the
runner shares, and the runner is not on `edge` at all; the harness's plane is deliberately not the path under
test. *The runner image has `docker-cli`, not the compose plugin*, and the compose file is not mounted into it,
so the admission plant (a node holding beta's CA, bootstrapped at alpha) starts with `docker run` — which is
why the image and the CA volumes carry pinned names. The plant asserts the rogue is *up* before asserting it
has no peers, so the negative is about admission rather than a dead container. **The defect it found, which
is the point of building it:** with the edge disconnected the client *hung* rather than returning — a refusing
partner sends a TCP reset and fails fast, a **blackholed** one (interface gone, default route still present)
sends nothing, and an unbounded connect waits forever; since a silent gateway is contractually
`DeliveryUnknown`, a client that never returns cannot deliver that verdict. Fixed by bounding the client's HTTP
(5 s connect / 30 s request, `FederationClient::with_timeouts`) and pinned in-process against RFC 5737
TEST-NET-1 by `a_blackholed_gateway_is_unknown_within_a_bound_rather_than_hanging`, which asserts the *bound*
rather than the error. **The in-process choreography could not have found it** — its severance is a shut-down
gateway, and a refusal fails fast; that is precisely why the record asked for this suite. Keys are **derived**
(`FED_ROLE=keys`, `make federation-keys`): a mistyped public key would read as `BadSignature`, a defect in the
thing under test rather than in its fixture. **With this the gate is met without a caveat.** Still open (row
11): SDK verbs, TLS on the federation edge itself, a hostile network between domains, more than two domains.
Log: [`.log/2026-09-18-item2-pr10b-docker-two-mesh.md`](.log/2026-09-18-item2-pr10b-docker-two-mesh.md).

## v3 contracts axis — item 5: scoped mandates, PRs 1–5 + D4 + two follow-ons — 2026-09-17/18 (unreleased, #244, #256–#259, #261–#263)

Record `docs/design/scoped-mandates.md` (the ADR, #244; log
[`.log/2026-09-17-item5-pr1-mandates-adr.md`](.log/2026-09-17-item5-pr1-mandates-adr.md)); code `src/mandate.rs` +
`src/mandate/{handover,restart,scenario_b,lock_audit,partition}.rs`, `mycelium-wiki/src/mandate_fence.rs`.
Everything is judged by one sentence: *once the resource acknowledges epoch E2, nothing authorized only under
E1 can commit — even if its holder refreshes, retries, reconnects or restarts.* **PR 2 (#256)** the contract:
`Mandate`, `ResourceAuthority::check` (**epoch first**), `MandateRefusal::Superseded` — **never `Conflict`**,
because `Conflict` is the retry loop's input and the loop would launder a revocation; epoch and term identity
kept separate; the three `LifecycleEvent`s recorded apart. **PR 3 (#257)** the fence *inside* `GitStore`'s own
transactions (D26): `update_ref_stdin` puts a `verify` of the mandate ref in the same `update-ref --stdin`
transaction as the content write; `push_args` adds `--atomic` + `--force-with-lease` on **every** push. The
remote half (pre-receive hook) is untested — stated. **PR 4 (#258)** `HandoverJournal::inherit()` — the
successor inherits **history, not conclusions**; every inherited conclusion is attributed; incumbency rules.
**PR 5 (#259)** `RestartGuard` fail-closed (a test demonstrates that assuming epoch `0` on restart readmits
every revoked holder at once); durable proposals via the existing `KvHandle::append` under `log/wiki/`, not a
service database. **D4 discharged (#261)** — `lock_audit.rs`: the audit read `distributed_lock` and found the
premise wrong: it reads back the converged value and hands a guard **only** to the proposer whose value
survived, so losers hold no token — **no second fence**; what `LockService` lacks is *entitlement* (no
appointer, no scope), not exclusion. **§6 of the adopted record was amended, dated**, rather than rewritten;
D2's baseline (owner-authorized appointment) stands for a narrower reason. **#262** the mutation-fence gate
(`scripts/check-wiki-mutation-fence.sh`, in `make check` + CI) makes "every mutation path protected"
checkable and **names the two exempt sites** — `refresh` (adopts the fenced remote head) and `publish`'s
splice retry (moves the *local* ref without re-verifying; the push is fenced, so the remote is protected;
recorded §7.2). **#263** the partition table (`partition.rs`) — **not a fence**, a client-side refusal to
*promise*; derived from *expiry is locally decidable, revocation is not*. The decisive test is scenario B
(item 6 PR 5, #260, below). Wiki: [companions/wiki](companions/wiki.md) → *The mandate fence*.

## v3 contracts axis — item 6 PRs 4–5: the two replay scenarios — 2026-09-17 (unreleased, #241, #260)

**PR 4 (#241)** scenario A, the WAL/snapshot race (`mycelium-core/src/persistence.rs`, log
[`.log/2026-09-17-item6-pr4-wal-snapshot-scenario.md`](.log/2026-09-17-item6-pr4-wal-snapshot-scenario.md)): the
`cfg(test)` **merge-removed witness** (`MergeRemoved`, an RAII guard) that must fail; the fs seams restructured
to **decide before acting** (`kernel_fs` + `planned_fs`) so an injected fault actually prevents the effect;
and the **canonical entry sort** — replaying found that two nodes with identical logical state wrote
byte-different snapshots, because papaya's iteration order leaked into the file. Three bugs found *by
replaying*: replay suppressed writes so a run could not read its own; effect requests embedded absolute paths
so no bundle replayed elsewhere; a fault did not prevent the effect. Phase A exit gate met: the race replays
from a bundle and its witness fails. **PR 5 (#260)** scenario B (`src/mandate/scenario_b.rs`): a **schedule
sweep, not five hand-written tests** — the invariant asserted after *every step of every schedule*; the ADR's
named case (revocation with **no** subsequent write — idle schedules that knock much later) included;
non-vacuous twice (a resource that ignores its installed epoch is caught; a *current* mandate still commits);
the sweep asserts its own size. Wiki: [testing](testing/testing.md) → *Replay scenarios A and B*.

## v3 contracts axis — item 7 gateway caller identity — 2026-09-13 (unreleased, main)

The first code item of the v3 queue (plan rev 1.10 §10.12.1; private WP1). `GatewayCaller` on every
gateway dispatch path (`tools/call`, `/a2a`, `rpc/call`, `scatter`, `emit_reliable`, `llm/*`): auth-layer
constructed, node-attested over the request digest, carried inside the RPC payload (wire v12 unchanged),
verified at the provider, authorised by *client principal* via `request_authorized` (guardrails +
SkillRunner switched). `gateway_caller_profile` secure/legacy; `sys/caller-context/{node}` marker; the four
negative cases + the `authorized_callers` gate + `/a2a` in CI. SDK parity: `RpcRequest.caller` on
`rpc_serve` / `rpcServe`, READMEs; `docs/operations/rbac.md` §7. Wiki: [security](security.md) §WS1.5,
[`.log/2026-09-13-item7-gateway-caller-identity.md`](.log/2026-09-13-item7-gateway-caller-identity.md).
## v3 contracts axis — the AE evaluator seam — 2026-09-14 (unreleased)

AE0's code half, on the merged item 7 (queue §10.12.4). `src/agent/action_evaluator.rs`: the
`ActionEvaluator` trait (deterministic, replaceable), `ActionEnvelope` assembled only from facts the
gateway verified, `Decision` with three verdicts, `preflight` enforcing what an adapter might get wrong
(permit-with-errors, permit over an unmapped operation, stale revision, expired envelope, a panicking
adapter — all refuse), and `ReferenceEvaluator` where an uncovered action is *authority not established*
rather than a denial. Hooked at the MCP `tools/call` dispatch between `gateway_auth` and
`gateway_rpc_call`; inert until `with_action_evaluator`. Gated on `gateway` + `tls` — the seam lives
where it is enforced, and the argument digest has no non-cryptographic fallback. Gates: 13 AE0 §9
fixtures + 2 live-gateway tests. Log:
[`.log/2026-09-14-ae-evaluator-seam.md`](.log/2026-09-14-ae-evaluator-seam.md).

## v3 contracts axis — AE0: the action-envelope ADR — 2026-09-13 (unreleased)

Queue §10.12.3, beside item 1's ADR. `docs/design/action-envelope-ae0.md`: the three questions a protected action
answers; the envelope assembled only by the enforcement point from verified facts (no second identity scheme —
item 1's identities, item 7's actor); the evaluator contract (three verdicts, indeterminate never permit,
deterministic, replaceable; Cedar in-process adopted as the one adapter, D37); authority facts with their issuers;
five evidence records through the `AuditSink`; the consumer's catalogue identity, generic tools, the deployment
report and coverage as a field; three strength profiles in the guardrails tier vocabulary; the pinned standards
matrix (SPIFFE · XACML-as-architecture · Cedar · ODRL profile · OAuth RAR · PROV); eleven negative fixtures. No
code; the seam lands on item 7. Wiki: [dev](dev.md) §Planned AE,
[`.log/2026-09-13-ae0-action-envelope-adr.md`](.log/2026-09-13-ae0-action-envelope-adr.md).

## v3 contracts axis — item 1 PR 1: the contracts-and-receipts ADR — 2026-09-13 (unreleased)

The plan's "this unblocks everything" item (§10.1). `docs/design/contracts-receipts.md`: the site-by-site
inventory of what an ack proves today; four receipts kept separate with independent visibility /
durability / effects / post-failure truth (D8 rev 1.1); `operation_id` + `attempt_id` (the identities AE0
binds); `Conflict`; `DeliveryUnknown`; apply→persist kept, persist-first only under the WAL-tail merge;
the reconciliation with `exactly-once-effect.md` (D11); the mapping of `emit_reliable` / mailbox / tuple
lease / min-acks into the vocabulary. Code: the regression floor (`floor_*` pins) and the V2 golden
on-disk fixtures (`tests/fixtures/persistence/fixint-v1`, replayed in CI). Docs: concepts vocabulary,
philosophy Property 8 + litmus tests 4–5, the compatibility rule, the CLAUDE.md ack invariant. Wiki:
[runtime-invariants](architecture/runtime-invariants.md) §Persistence, [testing](testing/testing.md),
[`.log/2026-09-13-item1-pr1-contract-adr.md`](.log/2026-09-13-item1-pr1-contract-adr.md).
## v3 contracts axis — item 8: threat model revision 2 — 2026-09-13 (unreleased)

`docs/threat-model.md` §5 Boundaries D–G (domain edge · authenticated-but-abusive client · evidence confidentiality /
hash-as-credential · compromised former holder / forged epoch) and §6 (verified claims · scoped attestations · redaction
· protected reproduction artefacts). A document, a Phase A gate: items 2, 3, 5 cite it from their PR 1 ADRs. Plan §6.5
marked done. Wiki: [security](security.md), [`.log/2026-09-13-threat-model-rev2.md`](.log/2026-09-13-threat-model-rev2.md).
## v3 contracts axis — WP5: cooldown parameter + `staleness_known` — 2026-09-13 (unreleased)

Item 4's standalone honesty fix (plan §10.12.5). `membership_cooldown_secs` replaces the unexported
`3 × health_check_interval` constant (default preserved; env override; ≥ 1 s; read at start — live timing intents
do not alter it, decided here); `ViewConfidence::staleness_known()` — a derived accessor plus a JSON key, **not** a new public field
(review, 2026-09-14: the struct is publicly constructible and not `#[non_exhaustive]`) — so an isolated node's
`max_staleness_ms: 0` reads as unknown. Two pins; a §6.6 ledger entry schedules `#[non_exhaustive]` for the
operator-constructed config structs, the break that every config-field addition has been making quietly. [`.log/2026-09-13-wp5-cooldown-parameter.md`](.log/2026-09-13-wp5-cooldown-parameter.md).
## v3 contracts axis — item 6 PR 1: the nondeterminism inventory — 2026-09-13 (unreleased)

`docs/design/replay-nondeterminism-inventory.md`: the measured inventory by kind and module (production paths on
`main`), the coverage map (kernel seams · Loom for CAS retries · fuzz · Docker suites), the sleeps whose duration is
a correctness assumption, the choices-trace and bundle schema (D14), the static check moved to PR 3 (D12), D13's
additions (CAS retries to Loom, hash iteration order, lease-expiry clock reads). Concepts: *seam*, *bundle*.
Wiki: [testing](testing/testing.md), [`.log/2026-09-13-item6-pr1-nondeterminism-inventory.md`](.log/2026-09-13-item6-pr1-nondeterminism-inventory.md).

## v3 contracts axis — AE-T: evidence is node-local, only a reference gossips — 2026-09-16 (unreleased, PR #225)

**Corrects the entry below, from the same day.** #224 recorded gateway decisions by sealing the whole decision
document into the tamper-evident audit chain — an ordinary signed KV entry, so every node received the exact
resource each call targeted, the policy's reason and the checked constraints. AE0 §5 forbids that in those words,
having adopted the rule after reviewing its own first draft, which proposed the same thing. Missed because §11
lists the journal as outstanding and that reads as *an addition* rather than *a correction of what you just wrote*;
no test could have caught it, since they asserted the evidence was recorded and it was. No exposure — the path is
inert without an attached evaluator. The correction: a node-local `EvidenceJournal` (append-only, fsynced, never
gossiped, item 1's `LocalDurability`), and an `AeReference` in the chain carrying the journal record's **content
hash** — tamper-evidence without dissemination, and no field on the type to put a resource in. Three failure
behaviours tested (saturation and persistence failure refuse; a lost acknowledgement is `DeliveryUnknown`, never
`Failed`), with `EvidenceProfile` choosing whether they gate the effect. The pin is a substring sweep over
everything that gossips, not a field-by-field check, because the latter would pass against a type that quietly
regained a `resource`. Wiki: [dev](dev.md) §AE,
[`.log/2026-09-16-ae-evidence-is-node-local.md`](.log/2026-09-16-ae-evidence-is-node-local.md).

## v3 contracts axis — item 6 PR 3: the channel and monotonic-clock seams — 2026-09-17 (unreleased, PRs #235–#239)

Five merged increments taking the forbidden-call baseline **199 → 159 sites across 43 files**. The routing was the
smaller half; what the routing *found* was the rest.

**Channels.** Twelve bounded sends now record their verdict and replay it: the WAL append queue (a full queue skips
a record — one of the three consequences the inventory names), the per-handler signal channel, the `StateRequest`
writer, the pong, the audit export drain, the AE evidence journal, and the peer writers. Stream identity is **per
destination**, because `targets` is an `AHashSet` whose iteration order is not stable across processes — one shared
stream would have handed peer A's recorded verdict to peer B, and the symptom would have read as *a frame lost*.
Forwards and pings are separate stream families although they share a channel: two tasks, and the interleaving of
two tasks on one channel is itself nondeterministic.

**A cost the seam was imposing on production.** `chan_try_send` takes its stream as `&str`, and an argument is
evaluated whether or not a kernel is installed — so the `format!("gossip/shard{n}")` introduced with the seam cost
a heap allocation **per frame dispatch in ordinary builds**, on the hottest path in the system. Nothing failed;
nothing would have, until someone benchmarked forwarding against v2.7.0. Fixed with a static name table (guarded by
a test, because a typo in entry 37 would silently split that shard's trace onto a stream nobody reads) and per-peer
names cached beside the sender. **A harness that makes the system slower in order to watch it has changed the thing
it was measuring** — the rule this produced.

**A whole lint scope nobody was running.** `mycelium-sim` was clippy-gated, but `mycelium-core --features sim` —
the arm that *routes* through it, where every call site's sim path lives — was only ever compiled by the test job.
Three warnings had been sitting there unseen. Third instance of one family (compliance 2026-09-04, core tests
2026-07-21): **a feature whose code is tested but never linted.** Running a suite is not the same claim as linting
the scope.

**The monotonic clock**, kept separate from the wall clock because every site it replaces measures an *interval*:
`Instant` is monotonic, so a backwards NTP step cannot make one negative, and `SystemTime` gives no such guarantee.
`writer.rs`'s reconnect backoff, `connection.rs`'s rate window and anti-entropy cooldown, the peer table
(**BREAKING**: `CoreCtx::peers` is `HashMap<NodeId, u64>`), `SwimMembership`, and `signal.rs`'s ten interval sites
(**BREAKING**: `MeshHandle::last_signal` returns the *age*, consistent with its sibling `last_signal_persistent`).
The peer table and the membership table had to move **together** — `merge_gossip` reads the clock once and hands the
same `now` to both.

**Two conversions landed on code nothing was checking.** Breaking `mono_since` left all 180 core tests green and
failed one test in the outer crate, about replica restarts. `seed_sender_log` had no test in *either*
representation. Both have one now, each verified against **both** failure directions — a gate checked in one
direction catches one kind of mistake. The general form: **converting unguarded code is not a safe refactor, it is
an untested change to production behaviour**, and the honest cost includes writing the test that should already
have been there.

**The representation's own cost, and what it bought.** Monotonic nanoseconds count from process start, so there is
no `Instant::now() - 600s` for a test process alive for milliseconds. That made `compute_view_confidence`'s
staleness branch unreachable — so it gained an injected `now`, the pattern `SwimMembership` had used all along, and
is now tested both ways. And `SignalLog::seed` reconstructs "this arrived `age_ms` ago" *at startup*, which needs a
point **before** the run: hence `sim_seam::MONO_ORIGIN_NS`, with `mycelium-sim`'s `Sources` starting at the same
origin, because a harness that disagreed there would disagree in the direction that hides the bug.

**A bug that was not one, recorded because the reasoning was sound.** `SignalLog::trim` does `Instant::now() -
window`; `Instant - Duration` panics on underflow; the window defaults to 600 s; `Instant` is `CLOCK_MONOTONIC`.
That argues for a reachable panic on any node started within ten minutes of boot, inside a task `tokio::spawn`
swallows. Probing rather than filing it: `Instant::checked_sub` returns `Some` even for `u64::MAX / 2` seconds,
because Rust's `Instant` is internally signed on Linux and offset on macOS. **Not real** — and the probe is what
revealed the origin constraint above.

Known gap, recorded rather than implied: the legacy non-SWIM staleness eviction in `tasks.rs` has no direct test.
Deliberately outside the seam, with the reason in the inventory rather than as debt: the A2A SSE channels, which
are per-request and cannot be full, one of them in a detached task whose scheduling the kernel exists to remove.
Wiki: [dev](dev.md), `.log/` entries dated 2026-09-17.

**The timer seam, first arm — 2026-09-18 (PR #269).** `sim_seam::sleep_ms`: the inventory's §2.3 row for
*fixed sleeps whose duration is a correctness assumption*, headed by the 1 s "let the winning commit converge"
after `distributed_lock`'s commit — the path the D4 audit could only model. `Record` sleeps and records that the
wait elapsed; **`Replay` never wall-waits** — the recorded effective duration advances both simulated clocks
(`Sources::advance_ms`; both, unlike a wall *jump*) and the task yields once. Routed: `lock/converge`,
`elect/converge`, `consensus/defer`, `consensus/suggest-defer`; `consensus_handle.rs` in the baseline 9 → 3.
**A boundary found by writing the test the other way first:** an authored timer result is honoured by the
timer, but in exact replay the clock reads that follow are replayed too — so **exact replay reproduces; it
cannot explore**. Exploring 0 / exact / beyond is scenario replay, the plan's third mode, which the two-mode
kernel does not have; the seam, the kernel and the inventory say "the hook, not the exploration", and the test
pins both halves. Log: [`.log/2026-09-18-item6-pr3-timer-seam.md`](.log/2026-09-18-item6-pr3-timer-seam.md).

**The timer seam, second arm — intervals — 2026-09-18 (PR #276).** `sim_seam::interval_ms` → `Ticker::tick`,
the inventory's §2.3 row 2. The recorded decision is the **nominal schedule** (an immediate first tick, then one
period each), so a replay ticks the loop's written cadence and never wall-waits; a tick and a sleep are
different requests. Nine `src/agent` sites routed with one stream per loop (per kind for opacity, per key for the
intent reconciler). **Found on the way:** the forbidden-call check does not see `time::interval` through a
`use tokio::time` alias — three of the nine had never been counted — the same alias gap its header already
closes for `fs`; recorded there, and its own change. *Closed the same day:* `alias_pattern` now counts
`<alias>::sleep|interval|timeout|Instant` in any file that imports the module under any spelling, which admitted
19 pre-existing sites in eight files (baseline 166 → 185) — the debt was always there, only uncounted. *The
remaining seven tickers followed the same day:* `kv-persist/{key}`, `mesh/emit/{kind}`, `mesh/stale/{kind}`,
`persistence/snapshot`, `rate/decide` (the one `Delay` loop), `swim/probe` (its 1 ms floor kept) and
`membership/tick` — **every periodic loop in the tree now ticks through the seam**; the inventory's §2.3 row 2 has
no "not yet routed" clause left. Log:
[`.log/2026-09-18-item6-pr3-interval-seam.md`](.log/2026-09-18-item6-pr3-interval-seam.md).

## v3 contracts axis — AE-T: the seam records what it enforces — 2026-09-16 (unreleased, PR #224)

Three gaps on the AE line, found by building the private exporter *against* the seam rather than by reviewing it:
the exporter needed records and there were none. **The gateway wrote nothing** — `ae_preflight` refused, logged,
counted and returned, so a node enforcing a declared remit produced no evidence at all. Every evaluated dispatch,
**permit and refusal both**, is now sealed into the audit chain as an `AeEvidence` document
(`mycelium.ae/evidence/1`) in `detail`; the document exists because a three-valued `AuditOutcome` collapses
*prohibited* into *not established*. A record that cannot be written **refuses the dispatch**
(`PreflightRefusal::NotRecorded`, `-32032`), permit included — enforcement without attribution is an unlogged gate.
**`/a2a` was unguarded** (both paths now run the same preflight under `gateway:a2a`; the preflight takes a `TaskCtx`
so the routes share one implementation). **The stale-policy check could never fire** — `expected_policy_revision`
was hardcoded `None`; `set_deployed_policy_revision` supplies it. Plus a gate that could never run: no standard
build combined `compliance` (the chain) with `a2a` (the route), so a test needing both would have passed locally and
never run in CI — the make-check-vs-CI-green family, one layer down at the *feature combination*. Additive
throughout. Wiki: [dev](dev.md) §AE,
[`.log/2026-09-16-ae-gateway-records-what-it-enforces.md`](.log/2026-09-16-ae-gateway-records-what-it-enforces.md).

## v3 contracts axis — hysteresis is load-bearing; the sweep gets a per-breaker plant — 2026-09-19 (unreleased)

Scenario C had recorded, the day before, that *hysteresis is not load-bearing while the release spacing is on*.
That sentence stood as an open question against a shipped mechanism, so it was **measured**, and it was wrong —
an artefact of schedule coverage.

Three measurements. **(1)** Removing hysteresis alone changed *nothing*: 40 boundary transitions with and
without, across all 48 schedules, zero differing. **(2)** Yet the band *is* visited — 124 of 282 opaque ticks —
so the proposals differ while the outcomes do not, meaning something downstream swallows the difference. **(3)**
What swallows it: the per-tick fill step reaches **0.575**, nearly three times the 0.200 band, so the fill steps
clean over it; and every schedule drops inbound to 100‰ before the quiet tail, so a *persistent* hover never
coexists with the window where *at rest* is judged. Hold the hover instead — a lone member, since a three-member
group drains faster than any share it can receive — and at 400‰ the shipped breakers give **one** transition
where removing hysteresis alone gives **twelve**.

Shipped: `Breakers::without_hysteresis()`, a **per-breaker** plant (`Breakers::off()` removes everything at once
and can never name which breaker did the work); `hover_schedules()`, kept separate because the main sweep's
contract is *the disturbance ends, then the loops settle* and these never end theirs; and three tests — the
comparative claim, the plant, and one asserting the release spacing does *not* fire here, since if it did the
two breakers would overlap. **The bound, also measured:** hysteresis damps a hover, it does not settle one — at
350‰ it takes 15 transitions to 11. A first draft asserted rest, failed, and that is where the bound came from.
The ADR §5 note is struck through and corrected rather than quietly edited. Log:
[`.log/2026-09-19-hysteresis-is-load-bearing.md`](.log/2026-09-19-hysteresis-is-load-bearing.md).

## v3 contracts axis — item 4's decisive demonstration; §12.1's gallery complete — 2026-09-19 (unreleased)

`examples/control_envelope_viz.rs` + `.html` (`:8096`, `--features metrics`). **This completes the plan's §12.1
gallery** — one decisive demonstration per item, which §12 makes a gate on phase exits rather than decoration.

**The design problem.** §12.1 asks for allocated rights and budgets under load, enforce-allocated versus
advisory, and the combined-feedback scenario. The three real governors are **crate-private loops fed by a live
cluster** — an example cannot drive them, and one that pretended to would be showing its own scaffolding. What
is public is the contract all three call (`decide`, `spacing_allows`, `may_propose`, `ConfidenceBound`,
`Profile`, `RightsLedger`), so the dashboard drives that, under a load generator, and says so on screen.

**The shape that makes it watchable.** The same proposal stream runs past the envelope under **all four
profiles at once**, as four columns — one run producing `legacy` 12 proceeded / 0 held, `observe` 12 proceeded /
6 would-hold / 0 held, and both enforcing rungs 6 proceeded / 6 held. That is the whole argument for
shadow-first in one picture: `observe` and `legacy` take the *same* actions, and the difference is a number an
operator reads before deciding. Stepping to `enforce-allocated` consumes the granted budget and then refuses —
`granted 8 · admitted 8 · refused 3 · recorded_rejections 3` — with the rejections **in the ledger**, because a
refusal nobody wrote down is indistinguishable from work nobody asked for.

**Not shown, on screen as well as in the record:** the combined-feedback scenario is replay **scenario C**,
whose claim is about *every* interleaving in a sweep; a dashboard shows one, so it would be a weaker thing
wearing the same name. Browser showcases run continuously and are built rather than run in CI, as the examples
index already says; the four CLI demonstrations are the ones CI runs. Log:
[`.log/2026-09-19-item4-control-envelope.md`](.log/2026-09-19-item4-control-envelope.md).

## v3 contracts axis — item 5's decisive demonstration, and the defect it found — 2026-09-19 (unreleased)

`mycelium-wiki/examples/curator_handover.rs` (feature `git-store`, run in CI): a council curator appointed as a
git ref, writing under it, the council re-appointing mid-stream, the replaced curator's next write refused with
content and `HEAD` shown unchanged either side, the new curator writing, and the git log carrying both names —
authority moved, history did not.

**The defect, and why §12 is a gate rather than decoration.** The fence puts the appointment check inside the
write's own git transaction — verify the mandate ref, update the content ref, both or neither — and that part
worked. But a failed transaction returned `RefMoved`, reported as `WikiError::Conflict`: *"compare-and-swap
version conflict (re-read and retry)"*. So **a curator who had been replaced was told to retry**, and retrying
refuses forever. A conflict and a revocation have opposite remedies; item 5's own record says a refusal naming
something else invites a fix that does not help, which is why `ResourceAuthority::check` is ordered epoch-first.
The store was checking correctly and describing it wrongly — the kind of thing only a consumer notices. Fixed
with `WikiError::mandate_revoked` / `as_mandate_revoked` carrying `MandateRevoked { refname, expected, found }`
inside `WikiError::Io`, the same additive shape as `GateRefusal`; a genuine lost CAS race still reports
`Conflict`, pinned by a plant so this is not "every failure now says revoked".

**Why nobody had seen it:** the fence's tests check the transaction **text**, never running it against real git.
The demonstration was its first end-to-end exercise and surfaced this in minutes. *A mechanism tested only at the
string level is a mechanism whose reported behaviour is untested.* Two API-shape lessons the example paid for
are in the log: the store is manifest-last (a section with no manifest entry is invisible to `read` — the
torn-write guarantee working), and `write_section` is a compare-and-swap, so the example must read the version
token first or a CAS conflict masquerades as the fence refusal. Log:
[`.log/2026-09-19-item5-curator-handover.md`](.log/2026-09-19-item5-curator-handover.md).

## v3 contracts axis — item 6's decisive demonstration: replay a bundle — 2026-09-19 (unreleased)

`mycelium-sim/examples/replay_a_bundle.rs`, run in CI. A depot's surplus-food sweep recorded through the
kernel, written as a bundle, read back and replayed clean; then the **same bundle** against a version that
drops the grace window a depot allows a late van, which diverges at seq 4 with the recorded effect printed
beside the replayed one; then the fix, green. A bug stops being *"it failed on the third run yesterday"* and
becomes *"effect 4 differs, here is what changed"*.

**Why the example owns its own bug.** §12.1 asks for the WAL/snapshot race replayed to its failing witness. A
binary cannot do that, and should not be able to: reproducing that failure means setting
`persistence::WITNESS_SKIP_WAL_MERGE`, which **removes the WAL-tail merge**, and exposing it outside `cfg(test)`
would put a data-loss switch in a shipped binary — worse to own than the demonstration is good. The example
therefore injects its own fault and points at
`the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording` for the real one.

**The distinction it ends on.** A bug whose effects reach a seam is caught as a **divergence** and localised to
one effect; a bug whose effects never reach a seam replays *perfectly* and is caught by the bundle's **witness**
assertion instead. That is why a bundle names a witness, and why one with an unknown witness cannot prove its
own fix. **An overclaim caught in review of my own draft:** the trace records a sha256 *digest* of a payload,
not the payload, so the divergence lines localise the effect but cannot name the offer — a trace is a log of
decisions, not a copy of the data. The example now says *the trace localises; the code names*. Log:
[`.log/2026-09-19-item6-replay-a-bundle.md`](.log/2026-09-19-item6-replay-a-bundle.md).

## v3 contracts axis — item 1's decisive demonstration: the receipt ladder — 2026-09-19 (unreleased)

The plan's §12 is an **alignment gate**, not decoration: no phase exit is declared while its lines are open, and
one line is a decisive demonstration per item. `examples/receipt_ladder.rs` is item 1's. The same write, four
ways, side by side — `set` (a bool naming no rung) · `set_with_receipt` with no persistence (`NotConfigured`) ·
`set_requiring_sync` there (**refused**, and the refusal carries whether persistence was configured at all,
which separates a misconfigured node from a failed disk) · `set_with_receipt` on a buffered node (`Buffered`) ·
`set_requiring_sync` there (`OnDisk`) · `set_with_replica_sync` (rung 3, somebody else's word). Then the peer is
shut down and the next write reports it **unknown, never failed**, and the persisted node's directory is
reopened so what replayed is read back rather than asserted. Every rung already had tests; what no test gave was
the *contrast*, which is the ladder's whole point: `Buffered` is not a weaker `OnDisk`, it is a different fact.

**The overclaim caught on the way.** The first draft called the reopen "a real process crash" and said it showed
`Buffered` surviving one. It does not — the shutdown is **clean**, so the bytes had every chance to reach the
file, and surviving that shows the WAL replaying, not durability under failure. Both failures `Buffered` declines
to survive, a kill and power loss, are outside one cooperating process. The example now says so in its header and
at the step. Weaker than the draft claimed, and accurate.

**CI now runs the decisive demonstrations** (`receipt_ladder`, `knowledge_layer`, `federated_domains`) rather
than only building them: building proves the API still compiles, running proves the demonstration still
demonstrates. Gallery rows added for all three. Still owed by §12.1: item 4's control-envelope viz, item 5's
curator handover, item 6's replay-a-bundle CLI. Log:
[`.log/2026-09-19-item1-receipt-ladder.md`](.log/2026-09-19-item1-receipt-ladder.md).

## v3 contracts axis — item 6: the scheduler seam's first arm — 2026-09-19 (unreleased)

The last unbuilt piece of the public axis, and the pin it was gated on **flipped**.
`mycelium_core::sim_seam::pause_clock_for_replay` (paired with `resume_clock_after_replay`) takes each recorded
wait on tokio's paused clock instead of collapsing it to one `yield_now`, so a wait is ordering information
again: no wall time, but a task's wait still orders it against every other waiting task. With it, the whole-node
recording of a linearizable award replays without divergence, and `mycelium-commitment`'s day-old pin is the
claim (`a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam`), with the unarmed
replay of the same trace kept beside it as the plant. CN2 is complete.

**The finding, and it is the reason to build rather than reason.** The design note (§3.1) predicted the paused
clock would restore the interleaving; built exactly that, and the pin **did not flip** — the divergence stayed
at seq 7. The real blocker was an asymmetry nobody had noticed: `Record` wrote its trace entry **after** a wait
while `Replay` checked its request **before** one, so a recording's order was the order waits *completed* and a
replay's was the order tasks *entered* them. Those differ exactly when waits overlap, which is the only case the
arm exists for. Both modes now check in at the same point. A two-task unit test (50 ms spawned before 10 ms, so
spawn order and completion order disagree) is what made it legible; the whole-node run could only say *seq 7
differs*. Both tests are kept, for localisation and for the claim respectively. A second consequence is recorded
in the seam's rustdoc: the replayed wait is the caller's **nominal** duration, because the effective one is only
knowable by consuming the trace entry, which is the check itself.

**Now owned:** task interleaving on one node, `current_thread`, where the order comes from seamed waits.
**Still not:** a multi-threaded runtime, real peers (the network seam), and two tasks runnable at the same
instant with no wait between them. Log:
[`.log/2026-09-19-item6-scheduler-seam-first-arm.md`](.log/2026-09-19-item6-scheduler-seam-first-arm.md).

## v2.9.0 release — 2026-09-19 (tag `v2.9.0`)

**The axis proving itself.** A MINOR whose headline is not new capability but new *evidence*. Wire **v12**
(`PREV = 11`) unchanged; on-disk format unchanged; the rolling upgrade holds.

**What it contains.** The **scheduler seam's first arm** — the last unbuilt piece of the public axis — so a
whole-node recording of a linearizable award now replays without divergence and CN2 is complete. **§12.1's
demonstration gallery complete**: one decisive demonstration per item, each ending by naming what it does not
establish, with the four CLI ones *run* in CI rather than only built. And a recorded doubt resolved by
measurement: hysteresis **is** load-bearing, the earlier reading was schedule coverage, and the sweep gained a
per-breaker plant plus the measured bound (it damps a hover, it does not settle one).

**The part worth remembering.** Two of the six demonstrations **found real defects** — which is the argument
for §12 being a gate rather than decoration, made by the gate itself:

- A **revoked wiki curator was told to "re-read and retry"**, advice that refuses forever. The fence was
  checking correctly and describing it wrongly, and only a consumer could notice. Its own tests check the
  transaction *text*; the demonstration was its first end-to-end exercise.
- A **federated call against a blackholed gateway hung** instead of returning `DeliveryUnknown` within a bound.
  A refusing partner fails fast; a blackholed one sends nothing. The in-process test could not have found it —
  its severance is a shut-down gateway, which refuses.

**Three overclaims of my own were caught before shipping**, and each correction made the artefact weaker and
accurate: a clean shutdown described as a crash; a trace's divergence lines said to name the record they
concern when the trace carries only a digest; and a hysteresis gate that asserted *rest* when the measurement
only supports *damping*.

**The one upgrade note.** A `mycelium-wiki` write refused by the mandate fence returns `WikiError::Io` carrying
`MandateRevoked` instead of `WikiError::Conflict`. Retry logic keyed on `Conflict` stops retrying that case —
the point of the change — but it is a change in what a caller sees. A genuine lost CAS race still returns
`Conflict`, and a store with no fence configured is unaffected byte for byte.

**Release gates.** `make check` clean on the bumped tree; the three library suites re-run on it; core with and
without `sim`; the sim, commitment and wiki suites; the wire back-compat gate
(`read_frame_accepts_prev_wire_version`) green, with the constants verified identical to `v2.8.0` by diff
rather than from memory. Bumped to 2.9.0: root, core, agentfacts, blackboard, commitment, effects, sim,
tuple-space, wasm-host, wiki; `mycelium-reason` stays on its own track. Log:
[`.log/2026-09-19-v2.9.0-release.md`](.log/2026-09-19-v2.9.0-release.md).

## v2.8.0 release — 2026-09-18 (tag `v2.8.0`)

**The v3 contracts axis, in one MINOR** — the largest since 2.0, and all of it additive. Wire **v12**
(`PREV = 11`) unchanged; on-disk format unchanged; the rolling upgrade holds. Plan of record:
`docs/plans/v3-contracts-axis.md` rev 1.12.

**Why one tag and not eight.** The axis was sequenced so each item's first PRs were usable on their own, but
the items constrain each other — item 6's seams are what let item 4's governors be replayed, item 1's receipt
vocabulary is what items 2 and 5 hand back, item 7's caller identity is what item 2 carries across a domain
edge. Tagging them separately would have shipped a vocabulary before the things that speak it. What a consumer
picks up here is one coherent surface, and the CHANGELOG's `[2.8.0]` section is its map.

**The item that finished hardest, and what finished it.** Item 2's release gate asks for a *two-mesh
demonstration* proving from membership tables, consensus state and traces that the meshes never merged. For
most of the axis that gate could not be met at all: PRs 1–7 built the contract and every page ended on the
sentence *no bytes cross a network*. PR 8 built the transport, PR 9 ran the whole choreography in one process
and **said plainly what that left open** (a shared address space; a "severance" that was a shut-down gateway),
PR 10a signed the catalogue and bound it to its asker, and PR 10b removed the caveats with a Docker suite —
one container per node, the link cut with `docker network disconnect`. That suite immediately found a real
defect: a *blackholed* partner (interface gone, default route still present) never refuses, so the unbounded
client hung instead of returning, and a client that never returns cannot deliver the `DeliveryUnknown` the
contract promises. **The in-process test could not have found it** — its severance refuses, and a refusal
fails fast. That is the argument for the gate being written the way it was.

**Three new crates.** `mycelium-sim` (the replay kernel, gated in `make check` and CI from its first commit),
`mycelium-effects` (the external-effect adapter and its destination-commit receipt), `mycelium-commitment`
(the contract net as five records — announce, offer, award, report, assess).

**What is knowingly unfinished, and stays unfinished in this tag.** The scheduler seam's first arm, pinned by
CN2's whole-node replay divergence rather than papered over. Item 2's row 11 (SDK verbs, TLS on the federation
edge itself, a hostile network, more than two domains). V1, the nightly scale runner. The private RA slice
beyond RA0. Each is named in the plan rather than implied by silence.

**The upgrade notes** are all one class: a struct gained a field, so an exhaustive struct literal breaks and
nothing else does — `GossipConfig.domain_profile`, `GovernorSnapshot` (four fields) and `ParamSnapshot`
(`pending`), `OpacityHint.release_spacing_ms`, `BoardConfig.high_watermark` and `BoardStats.rejected`. Reading
and matching on fields is unaffected, as is the documented `..Default::default()` pattern.

**Release gates.** `make check` clean on the bumped tree; the three library suites re-run on it (654 tls /
704 compliance / 480 gateway-only); the two-mesh Docker suite green (40 checks) both locally and as the CI
`federation` job. Bumped to 2.8.0: root, core, agentfacts, blackboard, commitment, effects, sim, tuple-space,
wasm-host, wiki; `mycelium-reason` stays on its own track (0.6.2). Log:
[`.log/2026-09-18-v2.8.0-release.md`](.log/2026-09-18-v2.8.0-release.md).

## v2.7.0 release — 2026-09-16 (tag `v2.7.0`)

A small **MINOR**, and the second defect this day found the same way: **by building a consumer against the
substrate rather than by re-reading it**. The exporter needed an `at` for the consumer's `activity_observation`,
`AeEvidence` carried no timestamp, so the exporter stamped its read time — and its own retry test failed within
minutes. Read time is wrong twice over: record ids derive from journal position, so an exporter that loses its
cursor and re-reads produces the *same* `batch_id` with a *different* body, which the consumer refuses under its
*same id, byte-identical content* rule; and the contract asks for **event** time, not read time. The fix was one
field the substrate was already holding — `ActionEnvelope::issued_at_ms`, which `for_decision` discarded. Both
records of one attempt carry the same value, because they are about one attempt.

The shape worth keeping: **a timestamp a record does not carry is one its reader has to invent, and an invented
one cannot be stable.** True of any field a downstream contract requires — if the producer does not carry it,
every consumer makes one up and they disagree. §5's remaining records (`requested`, `blocked`) should be checked
against the consumer's required fields *before* they are written.

`AeEvidence` also became `#[non_exhaustive]`: it is a type an exporter turns into a record another organisation
parses, and the next field addition should not break anyone. Wire **v12** unchanged. Release gates: `make check`
clean on the bumped tree, the five wire gates green, suites re-run on the release tree. Wiki:
[`.log/2026-09-16-ae-evidence-event-time.md`](.log/2026-09-16-ae-evidence-event-time.md).

## v2.6.0 release — 2026-09-16 (tag `v2.6.0`)

The **AE evidence MINOR**. Wire **v12** (PREV 11) unchanged; on-disk format unchanged; backwards-compatible
rolling upgrade, and every addition is inert for a node that attaches no action evaluator.

The arc of the day, told honestly because the shape of it is the lesson. The gateway enforced a declared remit
and recorded **nothing** — logged, counted, returned. The first fix (#224) recorded by sealing the whole decision
document into the tamper-evident audit chain, which is an ordinary signed KV entry: it **gossips to every node**.
AE0 §5 forbids that in those words, having adopted the rule after reviewing its own first draft. Missed because
§11 lists the journal as outstanding and that reads as *an addition* rather than *a correction of what you just
wrote*; no test could have caught it, because they asserted the evidence was recorded and it was. Corrected the
same day (#225): a node-local `EvidenceJournal` — append-only, fsynced, never gossiped, returning item 1's
`LocalDurability` — with an `AeReference` in the chain carrying the journal record's **content hash**, so
tamper-evidence survives without dissemination, and no field on the type to put a resource in. Then the two
pieces that make it usable: a cursor-based reader (#226, §6.7's outbox shape) because moving evidence out of the
chain had left an `AuditSink` exporter seeing only references — durable and unreachable; and the execution record
(#227), because every permitted call had been exporting as `effect: unknown` while the gateway watched the
provider answer, so the evidence could say what an agent was *allowed* to do and never what it *did*.

Also in this release: `/a2a` enforced on both dispatch paths (previously a remit could be walked around by
choosing the other door), a live stale-policy check (`expected_policy_revision` was hardcoded `None`, so it could
never fire), and a CI gate that could never run — no standard build combined `compliance` with `a2a`.

Three failure behaviours are implemented and tested rather than described: queue saturation refuses and never
drops silently; persistence failure refuses; a lost acknowledgement is `DeliveryUnknown`, **never** `Failed`,
because the record may well be on disk. `EvidenceProfile` decides whether they gate the effect.

Release gates (RELEASING.md §2–3): `make check-full` green; the five wire gates green
(`rolling_upgrade_read_frame_version_boundaries`, `rolling_upgrade_data_round_trips_losslessly_both_directions`,
`rolling_upgrade_forwarding_re_encodes_at_current_version`, `read_frame_accepts_prev_wire_version`,
`prev_wire_version_kv_write_is_applied_and_converges`). Seven shared crates bumped 2.5.0 → 2.6.0. Logs: the
`.log/` entries dated 2026-09-16.

## v2.5.0 release — 2026-09-15 (tag `v2.5.0`)

The **contracts MINOR** — the v3 contracts axis' first tranche, ten items merged between 2026-09-13
and 2026-09-15. Wire **v12** (PREV 11) unchanged; on-disk format unchanged.

**What an acknowledgement proves is now a receipt.** `mycelium-core/src/receipt.rs` carries the
vocabulary — `OperationId` / `AttemptId`, `LocalApplication::{Applied, AlreadyCurrent, Superseded,
Refused}`, `LocalDurability::{OnDisk, Buffered, Failed, NotConfigured}`, `ReplicaSync` and
`DestinationCommit` (vocabulary until PR 4b and PR 5), `ReceiptError` and `CommitError` with **no
variant meaning "nothing happened"**. New verbs return it: `set_with_receipt`, `retry_with_receipt`,
`set_requiring_sync` (persist → apply → gossip, so a failure means *this attempt applied nothing*),
`prepare_write` / `commit_prepared` (a caller-held stamp, so a lost acknowledgement can be retried
without clobbering a newer value), and `{cluster,group}_propose_receipt`. `ConsensusResult::Committed`
is deliberately **unchanged** — the receipt arrives on new verbs because growing a
non-`#[non_exhaustive]` variant is not additive (D24).

**A gateway call now carries who made it.** `GatewayCaller` (item 7): the auth layer constructs it,
the node attests it over the request digest, and the provider verifies it — so `authorized_callers`
judges the **client principal**, issuer-qualified, never the gateway node. `rpc_rx` verifies at the
receive boundary, so every companion serve loop is covered. **Upgrade note:** a deployment that listed
a gateway node to admit its clients must now list their principals.

**An action can be refused before it reaches a provider.** The AE evaluator seam
(`src/agent/action_evaluator.rs`): `ActionEvaluator` with permit / deny / **indeterminate**, an
`ActionEnvelope` assembled only from verified facts, a deterministic `ReferenceEvaluator`, and the
AE0 §9 negative fixtures as tests. Inert until `with_action_evaluator` attaches one; a **route-level
preflight**, never enforcement at the effect.

Also: threat model **revision 2** (boundaries D–G, and what identity/evidence/replay artefacts may
carry); the replay **nondeterminism inventory** with its coverage map and trace schema; the membership
governor's **cooldown as an explicit parameter** and `ViewConfidence::staleness_known()`; and
**rustls 0.23.45** (RUSTSEC-2026-0285), which the `v2.4.4` tag ships vulnerable.

**Nine external review rounds** ran across these, every finding fixed with a regression before merge.
The recurring class was one thing: a receipt, a decision or a document claiming slightly more than the
underlying operation established. Logs: the `.log/` entries dated 2026-09-13 → 2026-09-15.

## v2.4.4 release — 2026-09-12 (tag `v2.4.4`)

Durability PATCH on the 2.4 line: the snapshot rename is fsynced at the directory before the WAL is
truncated (`237352a`; the contracts axis Phase-0 item), so a power loss after truncation can no longer
leave the old `snapshot.bin` beside an empty `wal.bin`. Also shipped since v2.4.3: `/consensus/{*slot}`
tail capture (additive, plan §9), `mycelium-reason` 0.6.1 / 0.6.2 (own line), `set_with_min_acks`
documented honestly, the Python stub server backlog fix. Wire **v12** (PREV 11) unchanged; on-disk
format unchanged; no public-API change. Cut at the NovusLens consumer's request: their durable canon
rides the snapshot/WAL path and their manifest had carried "fsync-of-snapshot-rename still unreleased"
since 2026-09-06. Release gate: CI green on the release PR before tagging. Log
`.log/2026-09-12-v2.4.4-release.md`.

## v2.4.3 release — 2026-09-05 (tag `v2.4.3`)

A **durability PATCH** cut the same day as v2.4.2 (wire **v12**/PREV 11 unchanged; on-disk format
unchanged; no `mycelium` API change). Why: the v2.4.2 snapshot merge's WAL read-back used
`unwrap_or_default()` — a transient read error during a snapshot would have installed a snapshot *without*
the tail and then truncated it, a data-loss path one step past the race v2.4.2 fixed. Found by the
deterministic-replay design review's storage section ("schedule this failure explicitly"), confirmed and
fixed the same day (#181); the snapshot now aborts on read failure. Release gate: **CI-green on `a439f69`
and on the release PR before tagging**. Log `.log/2026-09-05-v2.4.3-release.md`. Also between the tags:

- **SDK gateway bearer + `persisted`** (2026-09-05, #178 / #179; tags `mycelium-py-v0.2.4`,
  `mycelium-ts-v0.1.1`): every handle takes `token=` / `{ token }` (fallback `MYCELIUM_GATEWAY_TOKEN`),
  riding pooled + SSE clients; `consistent_set` / `cross_group_propose` return `CommitResult { persisted }`.
  `jest` now CI-gated. Logs `.log/2026-09-05-sdk-bearer-token.md`.
- **`mycelium-reason` 0.6.2** (2026-09-06, tag `mycelium-reason-v0.6.2`, #198): the OpenAI façade omits the
  unknown prompt/completion split instead of reporting `0` (`mycelium.usage.split_known: false`); RA0 of the plan.
- **Contracts-axis plan rev 1.6** (2026-09-06, #197): the **RA slice** — attributable resource accounting composed
  from items 1, 4, 6, 7 (§6.7; D29–D32: minimal contract with money out of the hard-bound vocabulary, RA1 after item
  4's ADR, stub consumer in CI, responsibilities not roles); proposal vendored under `docs/plans/external/`.
- **Contracts-axis plan rev 1.5** (2026-09-06, #195/#196): item 3's two gates (semantic + behavioural); §13 the
  composition hypothesis — recorded, *not a v3.0 deliverable* (D28: a commitment is a composition of five records,
  no planner); both decks' positioning sentences.
- **Contracts-axis plan rev 1.4** (2026-09-06, #194): §12 delivery surfaces — examples (one decisive demonstration
  per item + refresh), dev/ops documentation re-alignment, the two decks under `/publication-lint`, the philosophy
  revision; no phase exit while its §12 lines are open.
- **Directory fsync after the snapshot rename** (2026-09-05, #183) + write-site WAL-failure warns; analysis
  **Run 61** (M2; floor 6/7/7 — WAL-error legibility, three probe gates, #185); doc-coverage **run 16**.
- **`mycelium-reason` 0.6.1** (2026-09-06): the router's rank-then-reserve race fixed — rank and reserve under
  one lock per attempt, one pure selection rule with a unit gate; the 0.6.0 reservation damped staggered herds
  only. Reserve-before-act (contracts-axis item 4) applied where CI was flaking.
- **Contracts-axis plan rev 1.3** (2026-09-06): the reviewer approved rev 1.2 as the strategic baseline; their four
  implementation requirements recorded (D26 atomic remote enforcement, one compatibility rule + D24 additive, phase
  gates with owners, secrets vs observability), D27 item-7 contract, D25 the NANDA boundary, the per-PR five-part
  statement. Reason-router reservation flake root-caused (reserve after rank) — follow-up.
- **Contracts-axis plan rev 1.2** (2026-09-06): items 7 (gateway caller identity) + 8 (threat model rev 2), the
  `3.0.0` removal ledger, the parity gate + public-surface-as-code rules, verification infrastructure named, the
  research-track link; `set_with_min_acks` documented honestly at seven sites (propagation, not receipt/durability).
- **Contracts-axis plan rev 1.1** (2026-09-06): the reviewer's response folded in — the decisive mandate invariant +
  one-transaction spec (D1), D2 conditional on D4, D8 visibility-vs-durability, D24 `persisted` tri-state, D6
  crypto-not-trust, D14 minimum bundle, posture rules 5–6, Phase B peer-durability gate. **Our correction:** the
  "live" membership-cooldown coupling did not exist (fixed at start from the config snapshot). PDF rev 1.1 produced.
- **The contracts axis consolidated into one plan of record** (2026-09-05): `docs/plans/v3-contracts-axis.md`
  (posture, dependency graph, phase gates, decision register D1–D23, corrections, next steps); the six external
  plans vendored under `docs/plans/external/`; ROADMAP § v3.0 restructured (two-axis naming note, index table).
  Same log, final section.
- **ROADMAP v3.0 — adaptive stability discipline recorded as proposed** (2026-09-05): the sixth external
  plan (`mycelium-control`, seven PRs — shared admission contract, actionable `ControlView`, fixed allocated
  rights, loop-breaking points, shadow-before-enforce); verified `max_staleness_ms` = 0 with no peers heard and
  the membership cooldown scaling with the health-check interval; seven reconciliations (guardrails tiers,
  companion depth signals, harness = replay stage 6, fix the cooldown coupling). Same log, addendum.
- **ROADMAP v3.0 — federated domains recorded as proposed** (2026-09-05): the fifth and last external plan
  (seven PRs — independently admitted meshes, HTTPS federation at gateways, three trust relationships,
  allowlist catalogs, `RemoteCapability`, no leader, `DeliveryUnknown`); anchors verified (SWIM is
  unauthenticated UDP); **my "touches the wire / 3.0.0 trigger" note withdrawn** — v1 leaves the wire
  untouched; seven reconciliations (compose with A2A, OIDC verifier, `EgressPolicy`). Same log, addendum.
- **ROADMAP v3.0 — scoped mandates recorded as proposed** (2026-09-05): the fourth external plan (seven
  PRs — mandate contract, CAS ≠ authorization, three lifecycle events, handover journal, incumbency rules);
  anchors verified; **one architectural reconciliation** (no new authority daemon — fence inside the store's
  own atomic boundary; establishment as a leased consensus slot; durable proposals via the log verb) + six
  more. Same log, addendum.
- **ROADMAP v3.0 — the knowledge layer recorded as proposed** (2026-09-05): the third external plan
  (`mycelium-knowledge`, seven PRs — claim/observation/assessment/acceptance records, evidence-aware
  resolution); anchors verified (trace lacks parent links; `knowledge/` unreserved); six PR-1
  reconciliations. Same log, addendum.
- **ROADMAP v3.0 — the contracts axis recorded as proposed** (2026-09-05): the third-party six-enhancement
  proposal + seven-PR contracts plan, our verification of its anchors (the `>=` quorum ack; no directory
  fsync), four PR-1 reconciliations, two ordering adjustments. `.log/2026-09-05-v3-contracts-axis.md`.

## v2.4.2 release — 2026-09-05 (tag `v2.4.2`)

A **security + durability PATCH** on the 2.4 line, cut the day after v2.4.1 from a single external
code review whose five findings were each reproduced by a probe before being fixed and each gated
by a regression test. Wire **v12**/PREV 11 unchanged; on-disk persistence format unchanged. **One API
note:** `ConsensusResult::Committed { persisted }` — additive, every in-tree matcher uses `{ .. }`,
but a downstream exhaustive destructure must add `..`. Release gate: **CI-green before tagging**
(the three merges #169 / #172 / #171 on `main`, then the release PR). Log
`.log/2026-09-05-v2.4.2-release.md`. What went in (the three PRs, merged in this order):

- **`langgraph-checkpoint-mycelium` 0.1.1** (2026-09-05, branch `fix/checkpointer-async-rows`):
  `alist` ran the sync row-selection driver on the event loop (finding 5 of the external
  review). Now a pure window/filter core + sync and async drivers, parity-gated without a node
  (`tests/test_alist_async.py`). Log `.log/2026-09-05-checkpointer-async-rows.md`.

- **Node-level routes behind gateway auth** (2026-09-05, branch
  `fix/public-routes-behind-gateway-auth`, stacked on the persistence fix): `/mcp`,
  `/signals/{kind}`, `/consensus/{slot}` answered without a bearer (finding 4 of the same
  external review) — `tools/call` with the node's identity. Now behind `gateway_auth` with
  `mcp:invoke` / `mesh:read` / `consensus:read`; `/bulk/{id}` stays a nonce-capability URL.
  Security PATCH material. Log `.log/2026-09-05-public-routes-behind-auth.md`.

- **Persistence durability — three P1 fixes** (2026-09-05, branch `fix/persistence-durability-p1`):
  an external review reproduced (1) a threshold snapshot erasing an acked, fsynced write (writer
  acks then snapshots in the same poll; callers applied after the ack), (2) replay filtering WAL
  records by HLC as if it were a log position, (3) `Ok` acks from a dead writer + `append_sync`
  not syncing outside `Flush` + consensus discarding the result. Fixed with apply-then-append at
  every write site, a WAL-tail LWW merge inside `do_snapshot`, LWW replay of every record,
  `BrokenPipe` acks, forced fsync, and `ConsensusResult::Committed { persisted }`. Invariants:
  [runtime-invariants §Persistence](architecture/runtime-invariants.md); log
  `.log/2026-09-05-persistence-durability-p1.md`.

## v2.4.1 release — 2026-09-04 (tag `v2.4.1`)

A **security PATCH** on the 2.4 line (wire **v12**/PREV 11 unchanged; no `mycelium` public-API
change — rolling upgrade holds). Release gate: **CI-green before tagging** (`65e0df6`). Why a
release: the v2.4.0 tag ships wasmtime 46.0.2 (RUSTSEC-2026-0269) and has every companion
`/gateway/…` surface open to unauthenticated callers when a gateway token is set. Cut from
CHANGELOG `[Unreleased]`; the auth fix carries an upgrade note for scoped-token deployments
(companion scope families). What accumulated since v2.4.0:

- **Two 360° review passes over `v2.4.0..HEAD`** (2026-09-02/03): mycelium-py 0.2.1 → 0.2.3
  (pooling bugs, lifecycle unification, uncapped long-poll pool), `FsStore` erase-vs-write
  serialization (lock-order row 35), one ref-CAS retry driver in `GitStore`; two RUSTSEC bumps.
  Log: `.log/2026-09-02-360-review-fixes.md`.
- **The nightly runner read correctly** (2026-09-03): stale checkout for 17 days, TCC timing,
  FORWARD-chain ceiling — `.log/2026-09-03-nightly-stale-checkout-and-ceiling.md`.
- **`mycelium-reason` 0.6.0 — the PAIR imports** (2026-09-04, tag `mycelium-reason-v0.6.0`):
  router **local reservations** (row 36), the **OpenAI-compatible façade** `/gateway/reason/v1/*`,
  the **`llm_meta` vocabulary** + `ollama` collector + `ollama_serve`. **Core fix:** routers merged
  via `with_http_routes` had bypassed the gateway auth layer — prefix-guarded layer + companion
  **scope families** (`llm`/`wiki`/`board`/`tuple`). Position (plan addendum): PAIR = GPU plane,
  Mycelium = agent plane, stackable. Coherence assessment + log:
  `.log/2026-09-04-pair-imports.md`; ledger entry (Security 8 at Run 59) in `docs/analysis/ratings.md`.
- **Same day:** `openai_serve` (the stacking example, mock-engine runnable); analysis **Run 60**
  (floor 7/7/7 — Modularity/Configurability/Robustness; yanked `chacha20` fixed); a **wiki-lint**
  pass (8 findings incl. the compliance clippy CI gap and the `audit/` KV row; 3 ledger entries);
  advisories h2 0.4.16 (RUSTSEC-2026-0258) and wasmtime 46.0.3 (RUSTSEC-2026-0269) from the
  review days.

## v2.4.0 release — 2026-08-16 (tag `v2.4.0`)

The **wiki-substrate** MINOR since v2.3.0. Wire **v12** (PREV 11) unchanged — a fully
backwards-compatible rolling upgrade (rolling-upgrade + prev-wire gates green). Release gate:
**CI-green before tagging**. Cut from CHANGELOG `[Unreleased]`. Highlights:

- **`GitStore`** (feature `git-store`) — the git-as-truth `WikiStore`, built strictly inside the
  E1–E4 eligibility envelope of `design/wiki-git-store.md` (first qualifying deployment: a
  public-record council-minutes corpus, `design/transparency-council-substrate.md`). Content-hash
  CAS tokens that never appear in the document; plumbing commits behind an atomic `update-ref`
  branch-head CAS; six-phase hardening with **recorded measurements** (reads: 330 ms/600 pages;
  contention: 5.5/3.0 batches/s over ten councils, zero spurious failures — the gate surfaced and
  fixed four real defects, incl. the merge-tree→subtree-splice falsification) — full trail:
  `plans/council-substrate-hardening.md`.
- **`GitMirror`** (feature `git-mirror`) — the git audit-projection `ChangeSink` for
  store-as-truth deployments: one reviewable commit per curated round, `EgressPolicy`-gated push,
  divergence tripwire, `rebuild()` as the erasure path. The keep-all decision (2026-08-16): both
  git shapes stay — the mirror is the general answer, the store the envelope exception.
- **Bulk ingest** — the claim-check stack: `IngestBatch`/`BatchSource`/`apply_batch` (batch-atomic
  through the write gate; byte-identical to a serial writer; resubmit is a no-op),
  `Wiki::submit_batch` RPC, and the boundary surface a consumer's-eye pass demanded:
  **`POST /gateway/wiki/ingest`** + `ingest` verbs on both SDKs. A batch = one meeting (the
  sizing contract).
- **Failover over node-local stores** — `WikiStore::refresh`/`publish` default methods;
  pull-on-promote (a curator that cannot refresh never serves), push-per-round, the ≤1-round
  un-published-tail residual tested rather than hidden.
- **`PageFormat`** — the pluggable entity codec (byte-exact round-trip; orphans-must-survive),
  proven end-to-end with a custom format; a deployment's own schema plugs in.
- **Exactly-once work distribution across companions** — tuple-space leases × idempotent ingest,
  with the kill at the worst point (after submit, before ack).
- **Security note:** the first tagged release carrying the **wasmtime RUSTSEC-2026-0222** fix
  (46.0.2) — the v2.3.0 tag was cut from a lineage predating the 2026-07-16 bump and ships 45.0.3
  with that low-severity advisory open.

Full notes: `CHANGELOG.md` § [2.4.0].

## v2.3.0 release — 2026-07-24 (tag `v2.3.0`)

Wire **v12** (PREV 11) unchanged — a fully backwards-compatible rolling upgrade; additive public API
throughout (minor bump). Also the **R1** step of the identity Phase-3 rollout: `require_identity_proofs`
ships default-off. The complete adopter-facing SOC 2 / pentest gap closure — plan
[`docs/plans/soc2-audit-gap-closure.md`](../../plans/soc2-audit-gap-closure.md) (✅ complete), all
CI-verified. Pure-library path; each workstream flips a
[shared-responsibility-matrix](../../operations/shared-responsibility-matrix.md) cell:

- **WS-A gateway TLS** — native server-side HTTPS (`GossipConfig::gateway_tls`) so bearer tokens
  aren't cleartext; hand-rolled `tokio-rustls`+`hyper-util` acceptor (no new compiled crate).
- **WS-B compromise remediation** — `rotate_identity_on_compromise` + `POST /gateway/identity/revoke`
  (`identity:write`); revocation was already consulted on all verify paths incl. consensus.
- **WS-C audit export** — pluggable `AuditSink` (SIEM/WORM) off the write path.
- **WS-D audit retention** — signed `AuditCheckpoint` (`sys/audit-checkpoint/`) → export → prune,
  verify-from-checkpoint.
- **WS-E `sys/identity` authentication** — the security-critical one: 1a extraction primitive · 1b
  CA-cert **anchor** harvest + `identity_anchor_conflicts` tripwire · 2 signed
  `sys/identity-proof/` (**prevention** — reject an overwrite not chained to a trusted key) · 3
  `require_identity_proofs` config flag (reject unsigned; **not** a wire bump — no frame change).
  Closes the forged-consensus-quorum vector. Full design
  [`design/identity-authentication.md`](../../design/identity-authentication.md).
- **WS-F GDPR erasure** — `SubjectKeyRegistry` crypto-shred (per-subject DEK; erase = destroy key),
  [`design/data-lifecycle-and-erasure.md`](../../design/data-lifecycle-and-erasure.md).
- **Process fix:** `make check` now clippies the `compliance` feature (it previously went un-linted
  locally — the local-vs-CI gap); three CI gates (compliance suite, consensus-free embed, core
  clippy) added. New direct deps `ring`/`hyper-util`/`tower-service` were all already in-tree.

## Companion re-versioning + distribution reality — 2026-07-26

Not a substrate release — a correction to two things that had drifted by neglect while the substrate
walked 2.1 → 2.3:

- **Re-versioned the two v3.0 companions by actual maturity, on independent version lines** (they
  compose the public `mycelium` 2.x API only — *not* the 2.x train): `mycelium-guardrails`
  **0.1.0 → 1.0.0** (tag `mycelium-guardrails-v1.0.0`) — an **API-stability commitment**; its scope is
  feature-complete, the remaining limits (promise-strength, eventually-consistent policy, coarse
  revocation) are **by-design** of a coordinator-free model, not gaps. `mycelium-reason`
  **0.1.0 → 0.5.0** (tag `mycelium-reason-v0.5.0`) — mature but deliberately **pre-freeze**: real-LLM
  backend / chunked-blob-past-8-MiB / conversation-memory / run-level-evals still open and may shape
  the API. No code change; per-crate CHANGELOGs added. **"v3.0" is a work epoch, not a version** — the
  substrate stays 2.x (ROADMAP v3.0 now says so explicitly).
- **Distribution is by git tag, not crates.io.** The `mycelium`/`mycelium-core` names on crates.io
  belong to an **unrelated, dormant 2019 project** (`gitlab.com/matthew.bradford/myceliumdds`, 0.1.1) —
  they are not this crate, and crates.io has no forced-transfer / abandoned-name reclaim path (only a
  voluntary owner handoff). So the supported install is git-tag deps (companions resolve the
  workspace-internal `mycelium` automatically); a `cargo add mycelium` would need the substrate
  **renamed** to a free name. This also fixed a latent bug: `building-on-mycelium.md` §1 had told
  adopters `mycelium = "2"`, which resolves against the 2019 crate. Install story:
  [`building-on-mycelium.md`](../../guide/building-on-mycelium.md) §1.

## v2.2.0 release — 2026-07-16 (tag `v2.2.0`)

A hardening MINOR since v2.1.0. Wire **v12** (PREV 11) unchanged — a fully backwards-compatible
rolling upgrade (rolling-upgrade + prev-wire gates green). Release gate **CI-green** — *not* just
`make check-full`: this cycle taught that the local gate misses the live-node/cross-language CI jobs
(see [testing](testing/testing.md) § "`make check-full` is NOT the whole CI gate"), which is how a
`mycelium-reason` trace-replay regression sat red for ~25 commits. Highlights:

- **Five-pass adversarial self-audit** (`docs/analysis/ratings.md` Runs 50–58) — ~40 correctness fixes,
  each with an executable regression gate. Consensus: cross-group quorum split-brain on even N,
  `elect_leader`/overlay split-brain, acceptor equivocation, vote double-count + impersonation, lease
  clock-domain. Convergence: **value-blind anti-entropy digest** (certified diverged nodes as
  converged → permanent silent divergence), HLC saturation/wrap, the store live-entry cap
  (tombstone-counting + overwrite-drop). Membership/connection: SWIM self-incarnation overflow,
  self-peering, writer reap/evict orphan, **snapshot tombstone-resurrection across restart**. Gateway:
  two unauthenticated **node-abort** inputs (`from_secs_f64`, `parse_hex32`), **JWT `aud`/`iss`
  bypass**, rate-aggregate overflow (limiter bypass), unclamped `fill_ratio` (584M-year sleep), the
  inert signal reorder buffer, and more. Companions: blackboard startup-lag split-brain + backfill,
  and the reason trace-replay CI regression.
- **Input-fuzz gate** — a proptest suite under overflow-checks (`store`/`config`/`capability`/`rate`/
  `hlc`/`swim_membership`) + the nightly `frame_apply` cargo-fuzz target: unchecked arithmetic on a
  gossiped/config value fails the build. The invariant: *arithmetic on untrusted values must
  saturate/clamp*. Not yet comprehensive — Robustness in `ratings.md` stays floored pending a clean pass.
- **Identity-authentication — Phase 1a** (`tls::ed25519_key_from_cert_der`, zero-dep) + the phased
  design `docs/design/identity-authentication.md` (the anchor for closing the `sys/identity` poisoning
  gap; CFT-not-BFT, defense-in-depth). The "signed by the old key" overclaim in the rotation docs was
  corrected — the entry is unsigned.
- **`/ready` semantics changed** — startup-complete, not soft-state-advertised; a no-capability node is
  no longer un-deployable behind a k8s readiness gate. Plus new public API `Blackboard::is_primary`/
  `is_secondary`.

Full notes: `CHANGELOG.md` § [2.2.0].

## v2.1.0 release — 2026-07-15 (tag `v2.1.0`)

The first MINOR since v2.0.0 (tag 2026-07-04). Wire **v12** (PREV 11) unchanged — a fully
backwards-compatible rolling upgrade. Cut from CHANGELOG `[Unreleased]`; release gate `make
check-full` green (clippy feature-matrix + wasm-host clippy + **794 tests, 0 failed**). Highlights:

- **`LockService`** (`agent.consensus().locks()`) — the ergonomic distributed-lock service: blocking
  acquire (`lock(name, ttl, wait)`), scoped `with_lock(...)` (release guaranteed on every exit path),
  and a **monotonic-HLC fencing token** (the ballot regressed under gossip lag; the token is now the
  winning commit's HLC — monotonic across successive holders).
- **#164 — `distributed_lock` correctness** (two *Critical*, execution-confirmed): (A) acquire
  returned on the local optimistic commit, so two racers both got a guard (no mutual exclusion,
  reproduced `winners == 2`); (B) release tombstoned the plain key while the authoritative lock lives
  at `consensus/committed/lock/{name}`, so a taken lock was **permanently unreleasable**. Fixed with
  the converged-holder discipline + a real consensus lease; the HTTP gateway lock got the same fix.
  Three regression gates, all verified failing pre-fix.
- **`connect_peer` / `disconnect_peer`** — pin + actively warm a direct forwarding route to an
  RPC-heavy peer (survives forwarding-target rebuilds); the tuple-space pins both directions. Plus
  the other Fixed items: self-targeted Individual-signal flood, tuple-space discovery-wait +
  late-secondary backfill, HTTP `SO_REUSEADDR`.
- **CI-gated Docker cluster suites** (`cluster-suites.yml`) — `make test` (13 scenarios) +
  `make test-overlay` on substrate PRs/merges/nightly, no retries by design.
- **Examples/docs rework** — the single faceted **capability matrix** front door (every example
  fingerprinted by layer + facet, each linking to its run-doc); two artifact-library **browser
  showcases** (`provisioning_viz` autonomic self-heal · `catalog_viz` origin-death survival); the
  `## Loads` banner (each runtime-loading demo declares what it installs); the **UI-example contract**
  (every browser demo: gateway+metrics, Ops Console link, concepts box); and `philosophy.html` →
  GitHub-readable `philosophy.md`.

Full notes: `CHANGELOG.md` § [2.1.0].

## Post-v2.0: downstream on-ramp + hardening (2026-07-04/06)

- **`mycelium-wiki` curator step-down** (#127): the companion (group-scoped LLM-curated wiki,
  control-plane/data-plane — shipped 2026-07-03) gained a split-brain guard. The election settles on a
  fixed window, so a lost gossip race could leave two nodes self-elected — both writing the shared store
  with no recovery. A curator **sentinel** now applies lowest-id-wins *continuously* (a higher-id curator
  resigns → returns to the reader failover-watch), with the deterministic canary
  `dual_curators_reconcile_to_a_single_writer`. Root-caused as a single-writer defect (analysis Run 34,
  Major); red-before/green-after on the CI `Wiki (data plane)` job.
- **Downstream-integrator on-ramp** (#125, #126 + direct docs): a two-audience front door —
  `docs/guide/faq.md` (human orientation: is-this-for-me / which-primitive / why-not-X) and
  `docs/guide/building-on-mycelium.md` (the integrator contract: public-API-only rule, reserved KV
  prefixes, the invariants, a copyable `CLAUDE.md` snippet) — linked from the README (two-audience split)
  and the crate-root doc (surfaces on docs.rs). Plus the tuple-space **`redistribution`** worked example
  (equal footing with blackboard `microgrid` / wiki `wiki_chat`), the README four-paper corpus DOIs, and
  `/wiki-lint` **extended** to guard the front-door docs that *restate* code facts against doc-vs-code
  drift (caught a `schema()`→`schemas()` slip on its first pass).
- **Coop suite hardening** (#128): the `elastic_intent` demo's CI-load flake fixed structurally — a
  bidirectional-signed-propagation readiness gate (keeps the TLS identity-exchange window out of the
  convergence poll) + a self-heal window sized past the ~12 s governor cooldown. Verified 14/14 local +
  CI green (the previously-flaking `Food-Rescue Co-op suite` job).
- **Opacity control-signal-shed fix** (#129, 2026-07-06): a *real liveness bug* hiding behind a 10-run
  "flaky" test. The opacity governor emits `BOUNDARY_OPAQUE`/`TRANSPARENT` at `System` scope, and
  `ops::deliver_locally` probabilistically sheds non-`Individual` signals by `combined_fill`; under CI
  gossip-drain starvation the governor's single boundary-transition emission could be shed from *local*
  delivery — the "I'm now shedding" signal dropped by the shedding mechanism, precisely under load.
  Fixed by exempting boundary-transition kinds from the local shed (like `Individual`); deterministic
  regression `ops::delivery_shed_tests::boundary_transition_signals_are_never_locally_shed` (verified to
  fail without the fix). Root-caused by a deliberate dig (analysis Run 37, Major) after three prior
  "resolutions" mis-treated it as scheduling latency.
- **v3.0 positioning** (2026-07-05/06): a pattern-landscape scan established the
  substrate covers the *coordination* pattern space **natively or by composition of native primitives**
  (only ANP wire-protocol conformance needs new code; orchestrator is a non-goal). Recorded **two
  primary v3.0 deliverables** — `mycelium-reason` (LLM-authoring DX) and `mycelium-guardrails`
  (structural, coordinator-free guardrails) — plus packaging candidates. RAG / HITL / *content*
  guardrails are framed as **use-case functions** (external services accessed *through* the mesh — the
  wiki precedent), not substrate work. Homes: `ROADMAP.md` → v3.0 · `docs/wiki/domain/pattern-coverage.md`
  · `docs/plans/mycelium-{reason,guardrails}.md`. **Both primaries shipped 2026-07-08 (#130–#139) — see
  the two entries below;** this bullet records the positioning that preceded them (was "PROPOSED, not
  started" when written).

- **Artifact library — steps 1–5 shipped** (2026-07-07, commits `910c1ff`…`22ac02b`; design record
  `docs/design/artifact-library.md`): the durable origin tier + install generalization for
  `mycelium-wasm-host`. **Data:** `FsLibrarySource` (content-addressed blob dir, complete-or-absent
  writes) + the signed **manifest** (the library's own catalogue; publisher keys stay in CI) + a
  clean-slate versioned entry encoding with an explicit `ArtifactKind`, provenance now binding the
  *whole entry* (version‖kind‖artifact‖capability — closes a re-labeling hole). **Roles:** the
  **librarian** (`spawn_librarian` — serve + one `artifact/librarian` cap + stateless manifest→KV
  reconcile, signature-scoped) and `MeshArtifactSource::resolving` (holders discovered via the
  capability ring — no hardcoded provider ids). **Install:** `ArtifactRuntime`/`Installed` traits —
  `WasmHost` is now the engine inside *one* runtime; `BlobRuntime` places models/data
  (ranged/streamed pull via `RangedArtifactSource`, temp+rename, activation hook, pluggable probe);
  the `Provisioner` gained a kind registry, eligibility (kind + size budget + **resource
  headroom** — signed per-entry `requires`, `ResourceProbe`, in-flight reservations counted;
  §4.4, step 4b) with a tripwire counter, async `Installing→Live` reservations (token-checked),
  and **real** `{ns}/loading` pct tiers driven by actual bytes. **Honest demos:** `catalog` (runtime-read library → librarian →
  discovered pull → origin killed + library deleted → late joiner installs from a peer cache) and
  `mcp_toolgrowth` (the converter's arithmetic **arrives** as a new committed WASM fixture,
  bridged over MCP; activation-vs-installation taught explicitly); `llm_agent`'s percent loops
  stay simulated by decision (wasmtime must not enter `make check` via root dev-deps) and say so.
  Lock-order rows 20–22. **Complete** — step 6 shipped (`BlobFetcher`/`PrefetchingSource`/`HttpLibrarySource`: any HTTP(S) blob store, egress-gated, vendor SDKs via the trait); step 7 declined-with-evidence (three async faces already serve every consumer — note §10). **Session tail (same day):** the coverage review found `Installed::probe` was exposed but consumed by nothing — a **probe health pass** now opens every `provision_round` (fail → withdraw → the normal machinery reinstalls once the retracted ad clears the local view; probes are cheap-under-lock by contract); four lifecycle/concurrency tests landed (full per-kind lifecycles incl. blob probe-self-heal + shed-deletes-the-file; failed-install reservation-drop-retry; withdraw-during-install stale teardown), and the **`model_deploy` manual demo** proves the Blob path with a real 19 MB GGUF — **weights + deployment profile as two signed artifacts** (profile → weights by content address; failed-activation-retry is the ordering — note §4.3.1), streamed with honest percent, resolved + activated via `ollama create` (with `ollama show` asserting the arrived SYSTEM prompt is the one running), probe-gated, then generating real tokens (`ArtifactKind` note: a closed crate-owned enum — custom *runtimes* are the open axis, not custom kinds). Open: the crate-naming question only. **Run-38 floor fixed same day** (typed `InstallError` by stage; `mycelium_artifact_*` metrics-facade tripwires + recorder-backed test; the CI **flake tier** — `scripts/ci-retest.sh`, failed-tests-only retry with mandatory flake annotations, the class-level prevention Run 37 asked for).

- **`mycelium-reason` — v3.0 primary #1, LLM-authoring DX, COMPLETE**
  (2026-07-08, PRs #130–#136; plan `docs/plans/mycelium-reason.md` + `…-examples.md`, positioning
  `docs/wiki/domain/pattern-coverage.md` → the LLM-DX axis, guide **chapter 15**). The first *built* v3.0
  deliverable. Preceded by a **code-verified pre-implementation reassessment**
  (five bindings; corrected the 2026-07-07 addenda's overstatement that an attributed
  `cap/{node}/llm/inference` convention existed — it did not; and that resolution consults opacity — it
  does not). **PR #130 — the `mycelium-reason` crate** (public-API-only companion, no `mycelium-wasm-host`
  dep): ① **capability-routed inference** (`serve_model` = model-is-a-prompt-skill `llm/{model}` + a
  parallel attributed `llm-meta/{model}` ad; `InferenceRouter` = resolve → drop opaque nodes → rank by
  pheromone `peer_load` fill → failover — the routing layer the load-blind `resolve` deliberately
  omits), ② **fleet-reasoning traces** (`TraceRecorder`/`replay`/`narrate` on the log overlay, optional
  WS2 audit-chain anchoring under `compliance`), ③ **artifact-aware resume** (demand half:
  `require_model` + structural `await_ready` + `llm/loading` progress), plus the **content-addressed
  blob tier** (`FsBlobStore`/`MeshBlobStore`/`spawn_blob_server` — SHA-256 ids, verify-on-read, verified
  peer fetch, ≤ 8 MiB single-frame v1) and `/gateway/reason/{blob,trace}` routes. Implementation caught a
  real plan error — a single shared trace stream collides same-millisecond HLC keys across writers (the
  HLC's per-node logical counter) and LWW-drops records — fixed with **per-writer substreams**
  `reason/{run_id}/{node}`, merged on HLC at replay. Zero new locks. **PR #131 — the Python tier**
  (Tiers 1+2): **`langgraph-checkpoint-mycelium`** (a `BaseCheckpointSaver` — index rows in gossiped KV
  `ckpt/`/`ckptw/` with metadata inline for payload-free `list`, payloads in the blob tier with one blob
  per channel value so unchanged values dedup across super-steps; sync + async; **cross-node `StateGraph`
  resume proven in CI** — node B continues what node A checkpointed) and **`mycelium.call_typed`** (a
  through-the-mesh prompt-skill call with a balanced-brace JSON scanner + pydantic validation-feedback
  retry; pydantic via the `typed` extra). Landed the repo's **first Python CI job** (`python-sdk`: builds
  the `reason_node` example, boots a two-node mesh, runs both pytest suites — 14 tests). A checkpointer
  edge exposed and fixed the crate's empty-blob path (a typed `None` serializes to zero bytes = `SHA-256("")`;
  an empty fetch reply means *miss*, so `MeshBlobStore::get` answers it from the address alone). Reserved
  prefixes claimed: KV `ckpt/`·`ckptw/`·`log/reason/`, capability `reason/blob-cache`, RPC
  `reason.blob.fetch`. **PRs #132–#136 completed the LangGraph example ladder** (`docs/plans/mycelium-reason-examples.md`,
  built flagship-first): **#132** the routing gateway surface (`POST /gateway/reason/route` + Python
  `ReasonClient`) — needed because `/gateway/llm/call` is single-shot; **#133** the echo-CI **deploy/reheal
  flagship** (a graph's model dependency follows it across node death: checkpoint on A → gossip to B →
  kill A → B reheals the model via the mesh blob fetch + `serve_model` bridge → resume routes to B);
  **#134** a real router-robustness fix the flagship's de-risking surfaced — a killed node poisoned
  routing for ~90 s (capability-freshness window; mesh RPC has no fast-fail), fixed with a **live-SWIM-membership
  filter** (`InferenceRouter` routes only to `peers()`+self) + a **`RouterConfig::failover_timeout`** (8 s;
  non-final attempts fail over fast, the last gets the full budget); canary `liveness_filter_drops_a_non_peer_cap`;
  **#135** rungs 0/1/2/3/5 (`examples/langgraph/`) + the ladder README + a small trace-recording surface
  (`run_id` on the route endpoint); **#136** guide chapter 15 + the **Ollama-manual** real-model variant
  (`examples/coop/src/bin/reheal_deploy.rs` — real GGUF via `model_deploy`'s `BlobRuntime`, `supervise(min=1)`-driven
  reheal, node-unique Ollama names; manual/not-CI, compile-verified only). All CI-green. Open: the
  `mycelium-reason` crate-naming question (shared with the artifact library); the Ollama variant is
  compile-verified but unrun (needs a live Ollama + GGUF).

- **`mycelium-guardrails` — v3.0 primary #2, structural coordinator-free guardrails, COMPLETE**
  (2026-07-08, PRs #137–#139; plan `docs/plans/mycelium-guardrails.md`, positioning
  `docs/wiki/domain/pattern-coverage.md` → Structural guardrails, guide **chapter 16**). *What an agent
  may do* — packaged on the public API only. Preceded by a **code-verified reassessment** (six bindings)
  whose headline reshaped the plan: the mechanisms are real but deliver **three distinct strength tiers**,
  so an honest policy must say which clause compiles to which. **PR #137 — the crate**: a tier-labelled
  `Policy` → `apply()` compiling one declaration to **Tier A** boundary (`join_group` — drop-before-handler,
  self-imposed prevention), **Tier B** `AgentPolicy` (tool allow/deny + budgets, self-imposed at state
  transitions), **Tier C** `authorized_callers` (**hard prevention** — an unauthorized invoke is rejected
  at the provider, the one gate that's real prevention not promise-strength); `Policy::strength_report()`
  is the legibility (it discloses each clause's tier); the **self-imposed stance** is a decision (no remote
  policy authority — a central policy server is the chokepoint non-goal). It ships the reusable Tier-C gate
  + **denial sealing** (`check_caller`/`guarded_rpc_serve` seal `Invoke`/`Denied` into the tamper-evident
  chain) that previously only SkillRunner had. **PR #138 — the policy-audit verification tool**
  (`prove_denials`/`narrate_proof`): reconstruct a provider's chain, re-verify it, and prove the guardrail
  fired — with **honest framing** encoded in the output (it PROVES *this provider tamper-evidently sealed
  stopping X*; it DOES NOT prove *X could not have done Y anywhere* — per-node chains, only guarded caps
  seal) + the watchable `guardrail_wedge` example. **PR #139 — chapter 16 + `guardrail_fleet`** (all three
  tiers *actually firing* in a constructive co-op fleet; the Tier-A boundary *drop* — a non-event — proven
  by a positive/bounded-negative/bracket sequence). Revocation is **self-sovereign** (`revoke_identity_key`
  — a node revokes only its own keys; the levers over a misbehaving peer are narrowing its allowlist or
  dropping its role, never pushing policy in). All CI-green; a `Guardrails (v3.0)` CI job. Zero new locks.
  Open: broader packaging refinements + the crate-naming question.

## v2.0 (2026-06-21) — all 16 milestones M1–M16, acceptance gate met, no deferrals

| Workstream | Delivered | PRs |
|---|---|---|
| WS-A crate/API | M1 `mycelium-core` split · M2 `consensus` gate · M3 handle pushdown | #8 |
| WS-B scale/transport | M4 partial mesh · M5 SWIM (default **on**) · M11 codec (bincode retired, RUSTSEC-2025-0141) + Merkle anti-entropy, wire **v12**/PREV 11 | #19, #21, #22 |
| WS-C metabolism | M8 auto-derivation · M9 hot-reload/ClusterTuner + governor · elastic MembershipGovernor · M7 distributed rate-limit · M10 fence-free live timing | #26–#27, #105–#107 |
| WS-D security | M6 capability authz + CT revocation log | #77–#82 |
| WS-E code mobility | M12/M15/M14 — `mycelium-wasm-host` autonomic provisioning | #32–#42 |
| WS-F federation | M16 AgentFacts + schema migrations — `mycelium-agentfacts` | #44–#49, #83–#88 |
| WS-G coordination | M13 keyed take · `mycelium-blackboard` | #89–#100 |

Declined-with-evidence (kept as decisions, not debt): WS-G exactly-once overlay
(`docs/design/exactly-once-effect.md`), M10 consensus fence, WS-E epoch limits +
strict-consensus singleton, OR-Map for gcap (`docs/design/or-map-gcap-evaluation.md`).

## v1.x production readiness (complete)

WS1 RBAC/identity · WS2 tamper-evident audit · WS3 crown-jewel (feature-free) · WS4 OIDC
SSO · WS5 hot cert rotation — see [security](security.md); plan
`docs/plans/v1x-completion.md`. Support/SLA is commercial-track
([strategy](../domain/strategy/strategy.md)).

## Earlier landmarks

Sub-handle facade + gateway feature gate (pre-release remediation) · fuzz harness ·
locality/topology Phases 0–7 · cross-group consensus (Phase 8) · watcher C2 · signal
reorder buffer (wire v11 `hlc_seq`) · semantic coordination + schema registry · TupleSpace
companion (2026-06-11) · CI/test hygiene 2026-06-19 (shared `alloc_port`, PR #50; wgpu
dev-dep removed, PR #40; ephemeral-floor fix, PR #110).

## The self-audit series

`docs/analysis/ratings.md` — 37 runs; methodology M2 since Run 16 (execution-evidence gate,
falsification probes, calibration ledger). Run 28 (2026-07-02): 5 findings (3 Major), all
fixed same day — the oversized-write family, the state-machine commit race, RUSTSEC-2026-0188.
Run 34 (2026-07-05): the `mycelium-wiki` curator split-brain (Major, single-writer, #127). Run 37
(2026-07-06): the opacity control-signal-shed (Major, #129). 27 calibration-ledger entries.
**Methodology upgraded 2026-07-06 (bright line at Run 37):** *current score = current state* — a bug
found + fixed + deterministically gated in the same run scores its fixed end-state (not the old cap-at-6),
and finding-and-fixing a bug never lowers a score (accountability for past over-scoring lives in the
ledger); an *unknown-unknowns reserve* + *carried-score decay* temper confident 8s; and **past run scores
are never retroactively rewritten** (a time-series is only meaningful if its measurements stand). Pre-37
runs are dated snapshots under the prior rule.
