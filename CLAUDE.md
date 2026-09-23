# CLAUDE.md — Mycelium session on-ramp

Fast on-ramp for code-assistant sessions. This file is deliberately thin: it holds the
**workflow rules, the build/test gates, and the hottest invariants** — everything else
lives in the **LLM wiki** and the code canon it cites.

## What this is

Mycelium is an embedded, broker-less Rust library — a three-layer substrate for AI agent
fleets and storage replication: **I** gossip KV (LWW + HLC, Merkle anti-entropy) ·
**II** signal mesh (scoped events, admission boundaries, opacity) · **III** epidemic
consensus. Layers I+II are the `mycelium-core` crate; `mycelium` adds III, capabilities,
services, gateway, tls. It is a **library, not a platform** — no daemon, no control plane;
a cluster is emergent from network reachability (peer-exchange + CA admission — **not**
`cluster_name`, which is a cosmetic label). v2.0 complete (all 16 milestones, 2026-06-21); v2.1.0
2026-07-15 (`LockService`, CI-gated Docker suites, #164 lock fixes);
v2.2.0 2026-07-16 (five-pass adversarial self-audit, input-fuzz gate, identity-auth Phase 1a, `/ready`
fix); v2.3.0 2026-07-24 (the **SOC 2 audit-gap** MINOR: gateway TLS (`gateway_tls`) · audit export
(`AuditSink`) + retention checkpointing · `sys/identity` authentication (CA anchor → signed
`sys/identity-proof/` → `require_identity_proofs`, default-off) · rotate+revoke · GDPR crypto-shred
(`SubjectKeyRegistry`) — the shared-responsibility matrix is
`docs/operations/shared-responsibility-matrix.md`); **v2.4.0 released 2026-08-16** (tag `v2.4.0` —
the **wiki-substrate** MINOR: the git-as-truth `GitStore` (`git-store`, six-phase hardening with
recorded measurements) · `GitMirror` projection sink (`git-mirror`) · claim-check **bulk ingest**
(`submit_batch` + `POST /gateway/wiki/ingest` + py/ts SDK verbs) · pluggable `PageFormat` codec ·
curator failover over node-local stores — all additive, wire unchanged; records
`docs/design/transparency-council-substrate.md` + `docs/plans/council-substrate-hardening.md`;
first tagged release with the wasmtime RUSTSEC-2026-0222 fix); **v2.4.1 released 2026-09-04** (tag
`v2.4.1` — a **security PATCH**: merged `with_http_routes` routers now behind gateway auth + companion
scope families · wasmtime 46.0.3 (RUSTSEC-2026-0269) · h2 0.4.16 (RUSTSEC-2026-0258) · `FsStore`
erase serialization; wire unchanged, no API change; companions `mycelium-reason` 0.6.0 /
`mycelium-py` 0.2.3 on their own lines); **v2.4.2 released 2026-09-05** (tag `v2.4.2` — a
**security + durability PATCH** from one external review: `/mcp`, `/signals/{kind}`,
`/consensus/{slot}` behind gateway auth (`mcp:invoke`/`mesh:read`/`consensus:read`) · three P1
persistence fixes (snapshot merges the WAL tail, LWW replay, honest acks + forced fsync,
`Committed { persisted }` — the one API note: add `..` to an exhaustive `Committed` destructure) ·
`langgraph-checkpoint-mycelium` 0.1.1 async row selection; wire unchanged); **v2.12.0 released 2026-09-23** (tag `v2.12.0` — **authenticating the caller is not authenticating the call**. Wire **v12** unchanged. A federated credential's signature covered the origin, principal, export and window and **nothing about the payload**, so an attacker between domains could rewrite a call's body under a header that still verified and the gateway would record an evidence decision about *their* text; `FederatedCaller::body_sha256` is inside the signature now, over the bytes **as received**. And `preflight` is **public**, because an enforcement point that is not this gateway had no path but `ActionEvaluator::evaluate`, which performs **none** of the seam's five checks — in each of them a *correct* evaluator permits and the seam refuses. **Upgrade notes:** `FederationEdge::authorize` takes the request body (§6.6 ledger entry 10, will not compile); `FederatedCaller` gained a field; `/a2a` answers a malformed request as `-32700`; `CallPolicy::require_body_binding` defaults to `false`, a rolling-upgrade window that gives **no integrity guarantee against an active attacker**. **Not claimed:** confidentiality — TLS on the edge needs a trust anchor that does not exist yet); before it **v2.11.1 released 2026-09-22** (tag `v2.11.1` — **four defects, and the gate that had never run**. Wire **v12** unchanged, no API change. All four were found by §12.6's own fuzz targets: the `/a2a` credential gate checking ASCII on the **encoding** not the content (an ASCII header can carry a non-ASCII principal via `\uXXXX`, and it could not be re-emitted); a replay trace losing a **bare carriage return** (`str::lines()` strips `\r` only before `\n`, and a trace is what a recording replays from); and two in the bundle codec — a value trimmed **inside** its quotes, and `split_once` finding its separator inside an **escaped key**, which was unpatchable and is now a scanner. **The lesson is why they survived:** the fuzz job runs twelve targets *sequentially and stops at the first crash*, so `main` was red for **22 consecutive runs** and targets six-to-twelve never executed — through every AE commit and through v2.11.0's tag. Separately, a 20,000-input noise pass reached **0 of 7** assertion-bearing targets, so three had never run their own invariant. Both closed: valid seeds asserted to parse, a **reachability registry** claiming completeness like the lock-order table, and `RELEASING.md` **step 2b — check CI on the branch you release *from*** (a green `check-full` and a green PR do not imply a green `main`). Also §12.1's **AE gallery row**: `procurement_authority` on the public seam, CI-run); before it **v2.11.0 released 2026-09-21** (tag `v2.11.0` — **the AE slice's evidence and contract halves**. Wire **v12** unchanged. Two things: the **composed guarantee gets an artefact that carries it** — the execution record now carries item 1's `LocalDurability` and the origin domain, so *a durable, attributed, cross-domain effect* is **reconstructable from one record** rather than believed across four (*reconstructable*, **not enforced** — nothing here stops a durable effect being attributed to the wrong principal); and the evaluator seam **stopped being replaceable in principle only** — `mycelium::ae_contract` states AE0 §9's cases once, evaluator-neutrally and **public**, because the old ones were written inline against `ReferenceEvaluator`'s own rule types and nobody else's evaluator could run one of them. Putting the real Cedar adapter through the new suite found the suite's own last assumption: it handed every evaluator a revision *string* and expected it back, which an adapter whose revision is a policy **digest** cannot do. Also AE3: an evidence record can **correct** another, citing the hash, never editing in place. **Upgrade notes**, both one class — an exhaustive `match` needs a `_` arm: `RecordKind` and `Execution` are now `#[non_exhaustive]`, and `Execution`'s `_` arm **must fail safe** — an unrecognised execution is *we did not look*, never *nothing ran*); before it **v2.10.0 released 2026-09-20** (tag `v2.10.0` — **the axis auditing itself, and §12.6 closed**: three more Phase-C findings fixed and **five more defects found by the gates built to prevent them** — the wire `DomainId` bypassing its own validating constructor, a replay bundle silently corrupting fields, two in the journal reader (a torn tail counted as a record; an unbounded allocation from a `u32` length), and AE1's mandate types public but unreachable from another crate. §12.6 complete: **nine trust-edge fuzz targets**, the adopter-facing `docs/guide/deprecations.md`, the front door, the companion checklist, the adversarial self-audit. Plus **AE1** — the action envelope binds a scoped mandate, and a refused one denies **before** policy runs, so a policy engine cannot launder a revocation. Wire **v12** unchanged. **Upgrade notes**, all one class — an exhaustive `match` needs a `_` arm: `CatalogRefusal` gains `StaleRevision` and is `#[non_exhaustive]`. And two deprecations announced late: `system_propose`, and **reading isolation into `cluster_name`**, which never provided any — if you relied on it to keep deployments apart, you do not have that separation); before it **v2.9.0 released 2026-09-19** (tag `v2.9.0` — **the axis proving itself**: the scheduler seam's first arm, so a whole-node recording now **replays without divergence** (CN2 complete) · §12.1's **demonstration gallery complete** — one runnable decisive demonstration per item, the four CLI ones run in CI · **two defects the demonstrations found**, both fixed and planted: a revoked wiki curator was told to *re-read and retry* (now refused by name, `WikiError::mandate_revoked`), and a federated call against a blackholed gateway hung instead of returning `DeliveryUnknown` within a bound · and a recorded doubt resolved by measuring it — hysteresis **is** load-bearing; the earlier reading was schedule coverage, and the sweep gained a per-breaker plant. Wire **v12** unchanged. **The one upgrade note:** a `mycelium-wiki` write refused by the mandate fence now returns `WikiError::Io(MandateRevoked)` rather than `WikiError::Conflict`, so retry logic keyed on `Conflict` stops retrying that case — which is the point; a genuine CAS race still returns `Conflict`, and a store with no fence configured is unaffected); before it **v2.8.0 released 2026-09-18** (tag `v2.8.0` — the **v3 contracts axis**, the largest MINOR since 2.0 and all of it additive: item 1 contracts (typed durability receipts, stable operation identity, required local sync, exact-identity ack, persisted-by-peer, `mycelium-effects`) · item 2 federated domains, **release gate met** (the A2A-carried credential, the signed catalogue, and a two-mesh Docker suite proving non-merger across real processes with the link cut at the network layer) · item 3 knowledge layer · item 4 adaptive stability (one admission contract per governor, the rights ledger, the shadow-first profile ladder) · item 5 scoped mandates · item 6 deterministic replay (`mycelium-sim`, the seams, the checked-in corpus) · item 7 gateway caller identity · item 8 threat model rev 2 · `mycelium-commitment` (the contract net). Wire **v12** unchanged. **The upgrade notes**, all one class — a struct gained a field, so an exhaustive struct literal breaks and nothing else: `GossipConfig.domain_profile`, `GovernorSnapshot` + `ParamSnapshot`, `OpacityHint.release_spacing_ms`, `BoardConfig.high_watermark` + `BoardStats.rejected`); before it **v2.7.0 released 2026-09-16** (tag `v2.7.0` — the evidence record carries its **own event time** (`AeEvidence::at_ms`, from the envelope) — an exporter that re-reads after a lost cursor re-sends the same record ids, and a body stamped with its own read time would be refused under the consumer's *same id, byte-identical content* rule; `AeEvidence` is now `#[non_exhaustive]`. Wire **v12** unchanged. **The upgrade note:** `AeEvidence` gained a field and became `#[non_exhaustive]`, breaking an exhaustive struct literal; construct it with `for_decision`); before it **v2.6.0 released 2026-09-16** (tag `v2.6.0` — the **AE evidence MINOR**: the gateway records what it enforces — every decision and every execution into a **node-local evidence journal** (`EvidenceJournal`, fsynced, never gossiped), with only a hash-bearing `AeReference` in the tamper-evident chain · `/a2a` enforced on both paths · a live stale-policy check (`set_deployed_policy_revision`) · a cursor-based journal reader for exporters. Wire **v12** unchanged; inert for a node that attaches no evaluator. **The one upgrade note:** a node that attaches an action evaluator should also attach an evidence journal (`with_evidence_journal`), or it enforces and records nothing — it warns at attach time); before it **v2.5.0 released 2026-09-15** (tag `v2.5.0` — the **contracts MINOR**: typed durability receipts (`set_with_receipt` · `set_requiring_sync` · `prepare_write`/`commit_prepared`, `LocalDurability` incl. `Buffered`, `DeliveryUnknown`) · gateway caller identity (`GatewayCaller`, issuer-qualified principals, `request_authorized`) · the AE evaluator seam (`ActionEvaluator`, permit/deny/**indeterminate**) · threat model rev 2 · the replay nondeterminism inventory · the governor cooldown parameter · rustls 0.23.45 (RUSTSEC-2026-0285). Wire **v12** unchanged. **Two upgrade notes:** `authorized_callers` now judges the *client principal*, not the gateway node; and `GossipConfig` gained fields, which breaks an exhaustive struct literal — the `Default`+assignment pattern is unaffected); before it **v2.4.4 released 2026-09-12** (tag `v2.4.4` — durability PATCH: the snapshot rename is fsynced at the directory before the WAL is truncated; `/consensus/{*slot}` tail capture; wire v12 unchanged); **v2.4.3 released 2026-09-05**
(tag `v2.4.3` — a **durability PATCH**: the snapshot now aborts on an unreadable WAL tail instead of
treating it as empty and truncating; plus the SDK bearer + `CommitResult { persisted }` companion work,
`mycelium-py` 0.2.4 / `mycelium-ts` 0.1.1 on their own tags; wire unchanged); wire **v12** (`PREV = 11`). Scopes are **`Cluster · Group · Individual`** (all / subset / one),
shared by `SignalScope` and consensus (`cluster_propose` / `group_propose`). *Renamed 2026-07-10:*
`System` → `Cluster` (wire-compatible; `system_propose` kept as a `#[deprecated]` alias, gateway
still accepts `"system"`); `system_stats()` is unrelated — node-local runtime state, not a scope.

## The wiki workflow (non-negotiable)

1. **Query first.** Start any task by reading [`docs/wiki/wiki.md`](docs/wiki/wiki.md) →
   the relevant section/pages. Don't re-derive what the wiki already states.
2. **Ingest on completion.** When finished work produces durable knowledge (new invariant,
   root-caused bug family, shipped workstream, revised position): update the page(s),
   refresh folder-notes, add one dated file to the section's `.log/`.
3. **Schema:** [`docs/wiki/AGENTS.md`](docs/wiki/AGENTS.md). **Code is canon** — the wiki
   cites `src/` rather than paraphrasing it; on conflict trust the code, then fix the page.
4. **Lint periodically** via the `/wiki-lint` skill — doc-vs-code verification first (the
   check that catches drifted claims like the Run-28 lock-table finding).

Private memory (`~/.claude/.../memory/`) holds user preferences and session state only —
promote durable project knowledge to the wiki.

## Where to read what

| For | Read |
|---|---|
| Public API + KV-namespace ownership | `src/lib.rs` crate doc |
| Wire format + version policy | `mycelium-core/src/framing.rs` (top) |
| HLC design + limits | `mycelium-core/src/hlc.rs` module doc |
| Capability model | `src/capability.rs` |
| Purpose / roadmap | `docs/philosophy.md` · `ROADMAP.md` |
| Docs map (guide, operations, design, plans, publications, analysis) | `docs/README.md` |
| Architecture, concurrency, testing lore, security, companions, history | the wiki: [`docs/wiki/`](docs/wiki/wiki.md) |

## Build & test gates (run before pushing)

**`make check`** is the one-command pre-push gate — clippy across the feature matrix CI enforces
(feature-matrix + `--no-default-features` + core), ~3 min, no wasmtime. The `--no-default-features`
clippy is the catcher for the *feature-gated dead-code trap* (an item live only under
`gateway`/`metrics` is dead in a minimal build — CI's Gateway-free + WASM-host jobs). `make
check-full` adds the test suites + wasm-host clippy. The underlying set:

```bash
cargo test --lib --features tls,metrics,a2a,llm
cargo clippy --lib --tests -- -D warnings                 # default features — catches tls-only dead code
cargo clippy --lib --tests --features tls,metrics,a2a,llm -- -D warnings
cargo test --lib --features compliance
cargo test --lib --no-default-features --features gateway
cargo clippy -p mycelium-core --lib --tests -- -D warnings
cargo clippy --lib --no-default-features -- -D warnings   # catches the feature-gated dead-code trap
```

Companion crates: `cargo test -p mycelium-tuple-space --features gateway`, same for
`mycelium-blackboard` (+ clippy `--all-targets`); `cargo test -p mycelium-effects --features tuple-space`
(+ clippy with and without the feature; `rusqlite` bundled, quarantined to that crate); `cargo test -p mycelium-commitment`
(+ clippy `--all-targets` + `cargo run -p mycelium-commitment --example redistribution_cn`, the CN1 gate). CI also gates `tsc --noEmit`, the AFN +
coop smokes, fuzz (non-PR), and `cargo audit`. **Never trust a memorised test count** — run
the suite. Scale suites: `make test-scale` (100 nodes), `test-scale-resilience`,
`test-scale-entries` — read the wiki's
[scale-tests page](docs/wiki/dev/testing/scale-tests.md) before interpreting failures
(Docker-bridge iptables ceiling, VM fatigue).

## Hot invariants (the ones that ship regressions when forgotten)

- **One lock per function**, flat acquisitions only — the [lock-order
  table](docs/wiki/dev/concurrency/lock-order.md) claims completeness: adding any
  `Mutex`/`RwLock` field means adding a row.
- **papaya:** `compute` closures retry-safe; never act on a stale read — the whole
  recurring race family, rules + reference impls in
  [lock-free-and-atomics](docs/wiki/dev/concurrency/lock-free-and-atomics.md).
- **Individual-scope forwarding is unconditional** (flood fallback); only *admission* is
  scoped. Do not "optimize" it away. The one carve-out is not precedent: a frame addressed
  to *this* node terminates here (routing at the terminal, #162) —
  [runtime-invariants](docs/wiki/dev/architecture/runtime-invariants.md).
- **Detection, not prevention:** never teach Layer I a higher-layer law (no prefix write
  guards in `apply_and_notify`) — tripwires + counters instead.
- **Consensus listeners register synchronously**; multi-node consensus tests need a
  listener on every node + a structural peer-ready poll (never fixed sleeps) —
  [testing](docs/wiki/dev/testing/testing.md).
- **KV writes are size-gated** (`framing::MAX_KV_WRITE_BYTES`); anti-entropy is chunked;
  a `FrameTooLarge` frame is dropped without tearing down the connection.
- **Apply to the store, then hand the record to the WAL** — never the reverse — and a WAL ack is
  a durability claim (`Err` when the writer is gone; `append_sync` fsyncs in every `SyncMode`).
  The snapshot merges the WAL tail before truncating; replay is LWW, not a watermark —
  [runtime-invariants](docs/wiki/dev/architecture/runtime-invariants.md) §Persistence.
- **An ack is a receipt that names its rung and nothing above it** — local application · local sync
  · replica sync · destination commit; a timeout is `DeliveryUnknown`, never a negative; same
  `operation_id` + different content is a `Conflict`. Today's `bool`s and `persisted` are pinned by the
  regression floor (`floor_*` tests + the golden on-disk fixtures under `tests/fixtures/persistence/`,
  replayed in CI); a PR changes an ack's meaning by changing a pin, in the open —
  [contracts-receipts ADR](docs/design/contracts-receipts.md).
- **No direct clock, RNG or filesystem access in production logic** — every one goes through
  `mycelium_core::sim_seam` (`wall_now_ms`, `mono_instant`, the timer seam every periodic loop ticks
  through), or a recording cannot replay deterministically. `scripts/check-sim-seams.sh` (in `make check`)
  holds a baseline of the permitted call sites in `scripts/sim-seams-baseline.txt`; a
  new one fails the gate until it is routed or the baseline is updated in the open. Off in every
  shipped build (feature `sim`) — [`mycelium-core/src/sim_seam.rs`](mycelium-core/src/sim_seam.rs).
- Ports via `test_util::alloc_port`; env-var tests hold `config::tests::env_test_lock()`.

## Active work

All engineering plans shipped as of 2026-06-21 (`docs/plans/README.md`); **Legible Emergence**
completed 2026-07-03 (phases 0–5); the **artifact library** completed 2026-07-07 (durable
library + librarian + kind/runtime generalization + resource-aware eligibility + honest demos —
`docs/design/artifact-library.md`; only its crate-naming question stays open).
Research-track: the three-arm work-distribution experiment (Paper 1) and the monetary-
ecology article revision ([wiki](docs/wiki/domain/publications.md)). **Transparency-Platform
council-wiki substrate (2026-08-15):** five mechanism phases built + gated at MEETING scale
(`GitStore`, group-per-council, write gate, claim-check ingest, work distribution — exactly-once
gate); a same-day critique found six gaps before council scale (format codec, failover sync,
branch contention, batch commits, read plane, batch gate) — hardening plan
`docs/plans/council-substrate-hardening.md` (**Phase 6 complete M-side 2026-08-16**, incl. the
measured ten-council contention run; remaining items are FTT-side); design record
`docs/design/transparency-council-substrate.md`. Scale runs double as Paper 1.s case study. **mycelium-reason 0.6.x (2026-09-04 → 06):** 0.6.0 the NVIDIA-PAIR imports — router local reservations, the
OpenAI-compatible façade `/gateway/reason/v1/*`, the `llm_meta` vocabulary + `ollama` collector (position: PAIR =
GPU plane, Mycelium = agent plane, stackable — `docs/plans/mycelium-reason.md` addendum); 0.6.1 rank-and-reserve
under one lock; 0.6.2 the façade omits the unknown token split instead of reporting `0`. **The contracts axis
(2026-09-05 →):** an external review's five defects fixed and released (v2.4.2 / v2.4.3, SDK 0.2.4 / 0.1.1); its
six enhancement plans reconciled into **one plan of record** — `docs/plans/v3-contracts-axis.md`, **rev 1.12**
(posture · dependency graph · phases A–E with exit gates · decision register D1–D39 · §12 delivery surfaces:
examples, dev/ops docs, decks, philosophy · §13 the composition hypothesis, *not* a v3 deliverable · §6.7 the RA
resource-accounting slice); externals vendored under `docs/plans/external/`. **State on 2026-09-18 (unreleased on
`main`, all additive on 2.7.0):** item 7 (gateway caller identity) · item 8 (threat model rev 2) · AE-T (the
evaluator seam, evidence journal) · **item 1 complete** (PRs 1–7: receipts, the effects companion `mycelium-effects`,
the tuple-space consumer, gateway/SDK parity) · **item 2** PRs 1–7 (federated domains; *the transport is not built*)
· **item 3 complete** (the knowledge layer, adapters) · **item 4 complete** (the adaptive-stability ADR, the control
contract, the rights ledger on the node-local journal, the membership/tuning/opacity governors and the provisioner
through the contract, admission control at the companions' queues, the §7 profile ladder through every governor with
its rollout runbook) · **item 5** PRs 1–5 + D4 (scoped mandates; no second fence) · **item 6 complete** (PRs 1–7: the
inventory, the `mycelium-sim` kernel, the seams — every periodic loop ticks through the timer seam — scenarios A/B/C,
the checked-in replay corpus; **the scheduler seam landed in v2.9.0**, so the last unrouted row is closed) ·
**CN1–CN3 complete** (the commitment companion `mycelium-commitment`; CN2's replay half closed with the seam).
The plan's Phase E "shadow-mode rollout" is `docs/operations/control-profiles.md`. **Both public gaps are shut:**
the federation transport shipped in v2.8.0 (item 2's release gate met, in-process then over a two-mesh Docker
suite) and the scheduler seam in v2.9.0. **§12.2's seven how-to chapters shipped 2026-09-19** (#304/#305/#306/#311 — guide 18 contracts & receipts ·
19 replay & simulation · 20 authorising actions · 21 mandates · 22 stability & control · 23 knowledge ·
24 commitments, plus the axis vocabulary in `00-concepts.md`, `ReceiptError` in the error taxonomy, and chapter
14's three-coordination-model comparison). **Nothing in CI enforces doc-vs-code accuracy** — `/doc-coverage` and
`/wiki-lint` are operator-run, and that pass found six pre-existing drift defects. **Shipped since (2026-09-19/20):** §12.2's
chapter 17 restructure (public discovery vs federated domains) · §12.3's federation runbook + §12.3 rows across
nine ops pages · §12.4 both decks · §12.5 (found complete) · **§12.6 complete**: the front door + companion
onboarding checklist, the **Phase-C adversarial self-audit** over items 1+2+7 (v2.9.1, four defects, PR #323 —
plus three post-release fixes #325–#327), **nine trust-edge fuzz targets** (#329/#331, which found four more
defects: the wire `DomainId` validation bypass, replay-bundle field corruption, a torn journal tail counted as a
record, an unbounded journal allocation — #330), and **migration notes per deprecation**, now an adopter-facing
page `docs/guide/deprecations.md` (#333). **The delivery remnants closed 2026-09-23** (PR #362): **item 2's row 11 is
complete** — the SDK verbs (`with_federation_clients` + five `/gateway/federation/*` routes behind
`federation:read`/`federation:invoke`; the credential names the *local caller*, never the node or the client's
configured principal, and never anything in the request body), a hostile network (integrity 09-22, TLS pinned on
an SPKI in the `TrustBundle` 09-23 — this design holds no X.509 material; `src/federation/pinning.rs`), and
**more than two domains** (`three_domains_compose_without_trust_composing`: trust does not compose, a grant you
hold is not re-exported, non-merger pairwise, slots per partner); and **§12.2 is complete** — the SDK receipt
narrative in both READMEs + guide 15 + the checkpointer README (whose `put()` is a **rung-1 receipt**, which
is why the flagship waits by *reading from node B*), the two missing wiki pages
(`dev/architecture/contracts.md`, `dev/testing/replay.md`) and `AGENTS.md` routing rules. That PR also
carries a **security fix**: a deployment configuring only `gateway_named_tokens` ran an **open gateway**
(the auth layer's token-model predicate did not count the named table), affecting **2.10.0–2.12.0** —
found because a scope test returned 504 where 403 was expected. Still open publicly: the §6.6 **removal ledger** is complete and its non-warnings are
**deliberate, each with a stated reason** — an earlier version of this line called them *"four unmarked
deprecations"*, which was wrong twice: the ledger has **9 entries, 4 warning** (2 `#[deprecated]`, 2
`#[non_exhaustive]` from 2.10.0), and *"unmarked"* implied an oversight. Entry 4 is the one that could
carry a compiler warning and does not: a struct-variant field **can** be `#[deprecated]` (checked), but
the field is still produced in ~50 places here, so marking it would mean suppressing a warning about
ourselves in all of them under `-D warnings`. Costed on the page, not open work; `mycelium-commitment`'s unsigned `Offer`/`Award` (provenance, a design question);
the **consumer-side-only per-partner budget** (the edge has no slot accounting — the one audit finding still
needing a decision rather than a patch; **the composition finding is closed**, 2026-09-21, by recording the
receipt's rung and the origin domain onto the execution record — `docs/design/composed-effect.md`, and note what
it claims: the composed sentence is *reconstructable*, not *enforced*); and **V1, the nightly scale runner, whose self-hosted box is offline so the job queues
silently and the criterion has never been met**. **Phase state (2026-09-23, after v2.12.0 — the axis is delivered).** Phases **B, C, D and E are
closed**: D's private half (RA4, AE2, AE3) landed 2026-09-20/21 and E's (RA5, RA6, AE4's contract
half) on 2026-09-21, alongside the public side in 2.8.0/2.9.0. **Phase A has one gate still open and
it is not a code gate:** V1, above. The **only other outstanding gate in the whole axis** is AE4's
**four joint AWS/GCP runs** — §6.8 says neither a fixture nor one cloud can close it, so it needs the
counterparty rather than a commit; AE4's operator and SDK examples travel with it. Phase E's
*combined-feedback scenario green* is met **in the bounded sense scenario C supports** (with the
breakers on every swept schedule settles, with them off at least one does not); the ADR's sharper
sentence — loops oscillating together while each is stable alone — is **not shown**, because the
three loops share one state variable and neither available isolation method can pose the question
(measured 2026-09-22). So a session looking for public work should look at the open list above or
§13's recorded questions, not at a phase gate. The plan's §10 is now a state of play rather than a
queue (**rev 1.15**). Delivery ledger:
[dev/history](docs/wiki/dev/history.md). Self-audit series: `docs/analysis/ratings.md`
(run via `/mycelium-analysis`).
