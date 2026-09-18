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
