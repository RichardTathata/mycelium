# Changelog

All notable changes to this project will be documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Changed

- **Tutorial 7 and the `one-hour` job, after a walk-through by a newcomer** (an agent that had not seen Mycelium, reading
  only the page; 121 s on macOS against v2.38.1, every step's checks passing — though the documented macOS checksum
  fallback failed as written). One `PLATFORM` variable instead of three tarball
  names to edit; the macOS checksum command with `--ignore-missing` (without it `shasum -c` fails on the tarballs not
  downloaded); `mkdir -p ~/.local/bin` and the `PATH` before a non-root install; the virtual environment kept active for
  steps 2–6; ports 7950 and 8300; steps 4 and 5 send stderr to a file, the fleet's own node logs; step 4 says its
  `[persistence]` and `[tls]` paths are replaced by the run's own directory (by design — the module doc), and names all
  five of the grant class's checks; steps 4 and 5 print the last lines of their stderr when the class does not pass;
  the page states the job's measured times (12 s on v2.38.0, 13 s on v2.38.1) and that a person's time is not measured.
- **`mycelium doctor` names the enforced guarantees** on an `enforced:` line, beside the count, so a reader sees which
  are met, not only how many.
- **`mycelium test --faults` says, once in its report, where its fleet lives and what its nodes' log lines are**, of
  what the run had: the run's own temporary directory (the configuration's `base_path` and `auto_cert_dir` replaced),
  and stderr from the in-process fleet — the dev profile, a CA minted for the run when it has TLS, the class's
  one-second timers, a cut node's listener errors when a class cut one — not the verdict. The fleet now runs under
  `dev` selected, so its nodes no longer tell a `dev` user to set `profile = "dev"` for the CA key the run mints; and
  the `recovery` class attaches each node's evidence journal before its evaluator, so it no longer logs that decisions
  will not be recorded.

### Added

- **A federation edge serves a credential once** (#695's review, finding 1; `docs/operations/federation.md` § A
  credential is one call): `FederationEdge::claim`, run on `/a2a` after the credential is authorised and before a slot
  is taken, keeps each partner's served credentials — keyed by the digest of the signed bytes — until they expire, and
  refuses one presented again as `CallRefusal::Replayed` (JSON-RPC -32003, `federation_replays_refused` on `GET /stats`).
  Before, a captured credential was served as often as it was sent inside its lifetime, repeating its effect; a body
  binding did not stop it, because the copy carries the same body. Bounded at 65 536 a partner
  (`with_replay_window_per_partner`); full, it refuses what it can no longer rule out as `ReplayUndecidable` (-32004,
  `federation_replays_undecidable`), never serving a replay. `FederationClient` mints each credential strictly later
  than its last, so an honest second call is never one. **Not built:** a set shared across a domain's gateways (a
  credential is served once at each), and one that survives a restart (it is in memory; a captured credential can be
  served once more after a restart, inside its lifetime). Upgrade note: `deprecations.md` §67.
- **A record a span names, read by its hash on the node that wrote it** (`docs/plans/adopter-surface.md` Y4's last
  promise; `docs/operations/audit.md` §3a): `GET /gateway/records/decision/{sha256}` and
  `GET /gateway/records/evidence/{sha256}`, scope `audit:read`, feature `compliance`. The answer carries the record's
  exact bytes (`record_b64`), which hash to the span's `mycelium.record_sha256` — the holder recomputes it — and, for an
  evidence record, the reference record in the node's signed audit stream that cites the hash (a verifying one
  preferred) and whether it verifies; a decision record is said `not_chained`. Evidence is read from the journal; a
  decision from the sinks registered with the route — the agent's own and any given to
  `GossipAgent::with_decision_record_source` (new) — through each sink's own tap, so a read never takes a sink's lock
  or makes a decision point drop a record, then from a trace the embedder declares
  (`GossipAgent::with_decision_trace_file`, new). *Not found* (`404`) is `never_existed` (evidence only, for the
  journal as opened), `not_written`, or `undetermined` with the meanings the node cannot rule out — always so for a
  decision, which may sit in a sink the route was never given (`not_searched`). New in `mycelium-core`: a tap per
  reader (`TapSlot`; `tap`, the collector's channel; `tap_ring`, a `TapRing` that keeps the newest copies, the oldest
  let go; `tap_dropped_for`, `last_seq`); in
  `mycelium`: `closure::find_in_trace`. No SDK verb.
- **The two-mesh federation suite's mesh nodes run under `secure-multi-domain`** (adopter-surface Y7's last promise,
  the first PR of the Y8 track). Every mesh node of both domains now starts under the profile: a one-shot issuer per
  domain mints the CA in a volume no node mounts and issues each node's certificate off-node into that node's own
  volume; the gateways serve HTTPS on the node identity, closed by a token, and the consumer pins each gateway's SPKI;
  every node carries identity proofs, an egress allow-list naming the partner's endpoint hosts, persistence
  (`sync_mode = flush`), `consensus_require_electorate`, an action evaluator at the gateway and at every provider with
  its evidence journal, and an execution authority with durable epochs; the edges bind bodies and meter each partner.
  The runner reads each node's own `GET /gateway/guarantees` for the profile met, finds only each node's own key on its
  filesystem, and shows the partner side cannot reach the test-only admin surface, now on the mesh address only (leg
  0); shows the evaluator and the body binding each refusing a call the catalogue granted (leg 1) and an exclusive
  outcome refused outside an electorate (leg 2); and starts a node with its CA key on-node, refused naming
  `id.ca_key_off_node` and nothing else (leg 10). The earlier legs pass under the profile unchanged.
  `examples/federation_node.rs` gains the `pki` role and `FED_PROFILE`; the admission plant's rogue runs `dev`. **The
  consumer side is not under the profile:** the probe is a bare `FederationClient`, so `fed.pinned`, `fed.caller_named`
  and `fed.partner_egress` as a node's facts are shown by the in-process tests only — carried to Y8.

- **`secure-multi-domain` — the profile, and the federation guarantee family it needs** (`docs/plans/adopter-surface.md`
  Y7, D25; `docs/features/multi-domain-profile.md`): ten `fed.*` guarantees in the catalogue, resolved by the same
  report as the rest — on this node `fed.domain_enforced` (the enforced domain profile: TLS, SWIM off), and on a node
  with a federation edge or clients `fed.pinned`, `fed.body_bound`, `fed.partner_slots`, `fed.caller_named`,
  `fed.catalogue_signed`, `fed.partner_egress`; and `fed.partner_posture`, `fed.non_merger`, `fed.clock_agreement`,
  which only the partner could show, listed `not_verifiable_here` and never counted. `profile = "secure-multi-domain"`
  rev 1 requires `secure-single-domain` rev 5's twenty and the seven node facts (`fed.domain_enforced` the seventh, agreed with the maintainer); `start()` refuses naming each one
  unmet, shown for each of the seven, and `doctor` prints the family. A node with neither an edge nor a client reads
  the side-specific ones `not_applicable`. `FederationClient` gained `tls_pinned` and `verifies_catalogue`.
  **Changed:** `GuaranteeReport::unresolved()` leaves out an external prerequisite that does not apply;
  a federation edge or clients attached after `start()` are ignored under `secure-multi-domain` (only), and the edge,
  the clients and `with_cohort_budget` count as late attachments under their own names; a node facing no partner is
  said so in one line (`deprecations.md` §66). **Open:** the
  two-mesh Docker suite under the profile, to be closed by the Y8-track precursor PR (the two-mesh suite under the profile). **Not built:** `doctor` reporting an edge or clients
  attached in code.
- **Trace export as spans** (`docs/plans/adopter-surface.md` Y4, D17): `mycelium::span_export` turns a decision record
  or an evidence record into a span carrying its identity and outcome — `correlation`, `parent`, the rule and its
  revision, the outcome, a decision's typed reason and the record's content hash — and never its inputs, target,
  effect, subject, resource, arguments, prose reason, mandate or the audit chain's links (the type has no field for
  them; schema `mycelium.span/1`, the attribute set pinned by a golden). `SpanCollector` reads the decision sink
  through `DecisionSink::tap` — a bounded channel the sink feeds after releasing its lock, so a collector never makes
  a decision point drop a record — and the evidence journal by cursor, draining neither, and reports the decisions a
  full sink dropped; `span_export::otlp` (feature `otel`) sends through a batch processor on a thread of its own, so
  an unreachable collector drops spans and never stalls the caller, the trace id from the correlation so one run is
  one trace. The
  guarantee report gains `trace.spans_not_evidence`: a span is not an evidence path — read the record by its hash on
  the node (`mycelium explain --closure`). Not built: an export loop in the `mycelium` binary; the route serving a
  record by hash under `audit:read` follows in its own PR (decided 2026-10-26).
- **Context selection** (`docs/plans/adopter-surface.md` Y1, decision D14): `mycelium::knowledge::selection` returns
  knowledge records for a run without blending them — each with its issuer, its attribution, its standing and the
  reader's verdict for its subject (computed over the admitted records whose standing is current), in one `VerdictClass` — ranked by the
  adopter's closure, verified records only by default, and read through a `ReadScope` built from a mandate that
  enumerates `knowledge.read`, used only inside its window: a record is readable when its subject lies under the
  mandate's scope by path segment and every record it was derived from is held and readable, so a record derived from
  one the caller cannot read is withheld. A condensation is written back (`Selector::write_condensed`) only as a claim
  signed by its own issuer under a key not revoked and dated no later than the drift bound, linked by `DerivedFrom` to
  every source, filed under the one subject all its sources are about, its sources in one class re-derived at write
  time — `InsufficientEvidence` and `Rejected` apart, a `BasisWithdrawn` record never folded into a live one, a
  retracted or expired source refused; a record derived from an expired one is expired. A condensation across subjects
  is not built. Feature page `docs/features/context-selection.md`. **Not built:** the wiki side (pages carry no issuer or
  verdict); a read mandate's epoch and revocation standing; re-verifying a selected record against the reader's current
  key view; scoping `KnowledgeStore`'s own methods (no route or SDK verb reads knowledge records).

## [2.38.1] — 2026-10-26

A security PATCH cut from the v2.38.0 tag. Wire **v12** unchanged (`PREV = 11`); no API change.

### Security

- **`zstd` removed from the tree: RUSTSEC-2026-0391, -0398, -0403, -0410** (dated 2026-10-16, in the advisory database since 2026-10-26; unfixed upstream
  as of wasmtime 50.0.0, whose `wasmtime-internal-cache` still requires `zstd ^0.13`). `zstd` reached the workspace
  only through wasmtime's on-disk compilation cache, a default feature of `wasmtime`; `mycelium-wasm-host` never
  configures that cache (no `cache_config` or `Config::cache` call), so the vulnerable code was compiled in and never
  run. `mycelium-wasm-host` now depends on `wasmtime` with every default feature but `cache`, which takes `zstd`,
  `zstd-safe`, `zstd-sys`, `wasmtime-internal-cache` and ten crates beneath them out of `Cargo.lock`; nothing else
  changes. It clears the ten `zstd-safe` *unsound* warnings beside them too (RUSTSEC-2026-0395 to -0397, -0399 to -0402
  and -0404 to -0406). **Check:** an application that turns wasmtime's `cache` feature on itself still carries `zstd` until
  wasmtime requires `zstd` 0.14.3 (`zstd-safe` 8.2.0, `zstd-sys` 2.1.2); one that depends on wasmtime with
  `default-features = false` and calls its `Config::cache*` API compiled only because this crate's `cache` was unified
  into its build, and must now enable `cache` itself; `cargo audit -f Cargo.lock` names it.

## [2.38.0] — 2026-10-26

Wire **v12** unchanged (`PREV = 11`). Upgrade notes: `docs/guide/deprecations.md` §58–§65.

### Added

- **The adopter surface's documentation and examples pass** (`docs/plans/adopter-surface.md` X11, the closing row):
  the front doors route to the new pages — the README's *Build a fleet* line and a new compass row (*Adopt it
  without a toolchain*), the docs index's Developer route starting at the published node and a scaffold, the capability map's rows
  naming their feature pages with a new *Start from the published node* row; the glossary anchors every term the plan
  introduced (`correlation`, `parent`, `closure`, `effect-class`, `reconciliation-probe`, `effective-configuration`),
  each linked from the feature page that owns it; **tutorial 7, the first hour** (`docs/guide/tutorials/07-the-first-hour.md`)
  — the `one-hour` job in prose, step for step, its fenced blocks diffed against the workflow's steps by
  `tests/first_hour_tutorial.rs`; guide 20 gains the grant, guide 18 the effect classes, the probe and selective
  recovery (and its ledger is a journal, not SQLite), guide 19 the correlation and closure reader; the readiness
  checklist says per section what `doctor` answers; the SDK READMEs start from the published node and the scaffold.
  **Stated:** the five steps stay the toolchain path (the no-toolchain path is tutorial 7, routed beside them); the
  `authority_grant` and `selective_recovery` examples the plan named were delivered as the `grant` and `recovery`
  fault classes against the scaffolds, not example binaries.
- **Grants, the fourth PR — the `grant` fault class and the one-hour job's step 4** (`docs/plans/adopter-surface.md`
  X3; `docs/features/grants.md`, `docs/features/test-faults.md`): `mycelium test --faults grant` drives a declaration's
  first `[[rule]]` with **`requires_grant = true`** on its own fleet — the tool's agent node with an evaluator built
  from every declared rule, a journal and an execution authority: a plain transition refused, silence refused within
  the window, a principal no rule names refused the issue by the evaluator, the declared issuer's grant admitting the
  call once, the request and the grant **replayed from the WAL across a restart** with the agent re-waiting and not
  asking twice, other arguments asking again, and the decisions on the record. It needs `[tls]`, `[persistence]` and a
  `compliance` build, and says which it lacks. The issue and revoke routes' decisions are callable functions now
  (`decide_grant`, `revoke_grant`), which the class uses — the fleet has no gateway port. Both scaffolds declare a
  grant-required tool and its issuer; the `Scaffold` CI job and the **one-hour job's step 4** run the class, so the
  job measures all six steps from the first release carrying it. `wire-check` warns `unissuable grant`; a `[[rule]]`
  or `[[mandate]]` with an unknown key is refused by name (`deprecations.md` §65). **Still not built:** retention of
  `grant/` records, a per-issuer entitlement, a per-call class for the rendering, the gateway and provider doors on the
  declaration's fleet (the door tests' job).
- **Grants, the third PR** (`docs/plans/adopter-surface.md` X3; `docs/features/grants.md`): the surfaces. `GET
  /gateway/grants/stream` (`grant:read`) — the open requests as they come, `grant_request` once each (the standing ones
  first) and `grant_answered` when one is issued, refused or revoked; the SDKs' `grants_stream` / `grantsStream`. A
  streamed A2A task whose serving agent is waiting for a grant reports **`input-required`** with the request's id and
  nothing else, then `working` once it is answered — the row's design question, decided. The request's arguments are
  rendered **under the data-class policy**: `AgentPolicy::grant_arguments_class` names their class (row R's
  `DataClass`), `grant_renderable_classes` the classes a request may render, and a class not listed is withheld — the
  record says so, with the class and the size — while the grant binds the digest (`deprecations.md` §64). From the
  PR's review: the 1.0 state name is `TASK_STATE_INPUT_REQUIRED` (it was built with the hyphen), the streamed A2A
  dispatch's bound is the unary path's 120 s, the stream says `grant_expired` once for a reported request that lapses
  and its task ends when the client goes, the adapter contract's A2A grants cell is carried. **Still not built:** the
  one-hour job's step 4 on the published binary (a `grant` fault class, the fourth PR), retention of `grant/` records,
  a per-issuer entitlement, a per-call class for the rendering.
- **Grants, the second PR** (`docs/plans/adopter-surface.md` X3; `docs/features/grants.md`): the enforcement
  remainder of the row. **One call at each door** — the gateway's preflight and the provider's each admit a grant once
  (`ExecutionAuthority::claim_grant_use`, atomic across concurrent presentations; the marker `…/used/{door}` on the
  WAL) and deny a second presentation `GrantUsed` naming the door. **Revocation** — `POST
  /gateway/grants/{agent}/{request}/revoke` (`grant:issue`; `grant.revoke` decided by the evaluator, `409
  not_the_issuer` elsewhere) publishes the next checkpoint for the grant's scope, offers it to the issuing node's
  authority — whose sweep now judges work under a grant by that standing and the grant's window and stops it within
  the drain bound — and writes it beside the grant (`mycelium_grants_revoked_total`). **Across nodes** — a supervisor at
  any gateway issues for an agent on a member node the gateway holds the key of; the grant's checkpoints travel in
  the KV and every node's authority is fed them (`feed_grant_checkpoint`, a watcher on `grant/`); the agent's node
  decides whether a member may issue (`EntitlementTable::entitle_grants_from_fleet`); a principal no member's is `409
  not_a_member`. **A restart re-waits** on the pending request from the WAL and does not ask twice; an expired one asks
  again. The refusals are reconstructable: a used or mis-bound grant's is a denial sealed into the audit chain, which
  `prove_denials` reads; a revoked grant's is *not established*, sealed as such. **Fixed on the way:** the authority's
  sweep cancelled work running under a grant at its first tick, revoked or not (the resource gate's scope rule);
  `deprecations.md` §63; and, from the PR's review, a grant's checkpoint exempt from the revocation view's start rule
  (a grant issued before a restart was refused after it), the single-use claim moved out of the history loop, an unknown
  standing waited out at the agent, the `grant/` watcher narrowed, the revoke route reading the signed grant, and the
  SDKs' `grant_revoke` / `grantRevoke`. **Still not built:** an SSE stream of requests, the A2A `input-required` state,
  the data-class policy over the argument rendering, the one-hour job's step 4, retention of `grant/` records — the
  row's third PR.
- **Selective recovery — the adopter-surface plan's decisive demonstration** (`docs/plans/adopter-surface.md` X9;
  `docs/features/selective-recovery.md`): the `recovery` class of `mycelium test --faults`. Three nodes of the
  declaration's fleet, each with a reconciler, an evidence journal and the evaluator, one mock destination in the
  process that loses answers once per key; one effect's acknowledgement lost, dependent effects on the other two nodes
  carrying the parent (one two steps down), independent effects beside them; the losing node cut and restarted over
  its WAL, a second node dark. The fleet **probes before it retries** (the restarted node finds its record unknown in
  its own ledger, a retry without a probe is refused, the destination holds one effect); **reads the closure** over
  the journals it can reach — *INCOMPLETE* while a node is dark, and a hole hides what hangs below it; every dependent
  and no independent once the node is back; **recovers each member by its class as a new record** — an idempotent
  dependent probed and resumed by one retry (of two concurrent retries one refused before anything is sent), a
  compensatable one probed and never retried, a committed one whose premise did not happen compensated under the
  evaluator's permit, an irreversible one *left* (unresolved, escalated); leaves independent work untouched; and
  **explains** every decision with `mycelium explain --closure` (kept beside a `--keep` run). Ten checks, the ids
  exercised (`persist.configured`, `persist.sync_mode`, `ae.preflight`). **The seamed form** (`sim`): the two
  recovery phases recorded into the replay corpus (`tests/replay-corpus/x9-selective-recovery/`) and replayed in CI
  against a fresh fleet with the destination reached **zero times in those phases** (the sends that set the scene are
  live in both forms), every check made again on the replay. The
  `Scaffold` job runs the class against the scaffold's declaration; the one-hour job runs it on the published binary
  as its step 5 from the first release carrying it. **The side-effect contract moved into the substrate** to make this
  possible (`mycelium::effects`; `mycelium-effects` re-exports every path unchanged — `deprecations.md` §62): the
  reconciler's ledger is the node-local journal now, not a SQLite file (`rusqlite` stays quarantined to the
  companion), one owner at a time, a frame it cannot read refusing the open; `Reconciler` gained `apply_caused`
  (X4's fields on the record), `leave`, and claims under the ledger's lock for the retry, the probe, the compensation
  and the leave (two moves racing on one record, one refused before anything is sent); a failed compensation's link
  comes off so another may be issued, and a composed compensation is refused rather than sent unchecked;
  `ExternalRefusal` gained `LeaveOfObservable`, `ExternalRefusalSnapshot` a field; `Class` gained `Recovery`. The philosophy's
  *Rollback → compensation* bullet now reads the closure reader as built. **Not built, stated:** a real destination,
  more than one, a closure read at runtime, a policy over the closure's size.
- **The adapter contract, and a unit's `runtime`** (`docs/plans/adopter-surface.md` X7 and X8;
  `docs/features/adapter-contract.md`): `docs/reference/adapter-contract.md` — one row per runtime adapter (the native
  Rust embedding, `mycelium-py`, `mycelium-ts`, `langgraph-checkpoint-mycelium`, `strands-session-mycelium`,
  `mycelium-reason`, the A2A door, the MCP door) and one column per property (session persistence, cancellation,
  tracing, identity propagation, grants, provider-side enforcement, receipts); a cell is *carried* and names the test
  CI runs that shows it, *not carried* with the reason, or *not applicable*. Generated from
  `mycelium::adapter_contract::ADAPTERS` (regenerate with `UPDATE_ADAPTER_CONTRACT=1 cargo test --lib
  adapter_contract`) and gated: `scripts/check-feature-pages.py` requires a row per adapter, a test per carried cell
  in the file it links (a Rust `fn`, a pytest `def`, a jest title), and in CI's `--executed` mode every such test
  executed in the run. Every companion README carries its limits under one name, **What this does not establish**
  (`check-materials.py` requires it); the two that had one under another name are renamed, five have one that had
  none (the Strands README's *Claims* section stays, and gains a limits section before it). The unit file gains an
  optional `runtime` key (`python` · `typescript` · `strands` · `langgraph`); `wire-check` prints, per unit, what
  that runtime carries and does not (the native embedding for a unit without one), an unknown value is the finding
  `unknown runtime`, a runtime unit with a stem section the finding `runtime with a stem section` (the declare
  route refuses those 422), and the declaration document's units carry `runtime`, `adapter`, `carries` and
  `not_carried` as **optional** fields of schema `mycelium.design/declaration/1` — a field added keeps the id, a
  pinned consumer never required it; a field removed, renamed or made required bumps it (the policy, now stated in
  `wire_check.rs` and `capability-lifecycle.md`). The Python scaffold's unit declares `runtime = "python"`.
  **Upgrade note:** a reader older than this release refuses a unit carrying `runtime` (`deprecations.md` §61).
  **Three cells the first table showed as gaps are closed in the adapters** (#676's cells): the LangGraph
  checkpointer reads the receipt every write returns (`WriteReceipt` on `last_receipt` and `on_receipt`;
  `require_on_disk` raises `WriteNotOnDisk` for a write the gateway node did not establish; `langgraph-checkpoint-mycelium`
  0.3.4); the Strands store's bearer is shown to ride every request it makes (`test_the_bearer_rides_every_request_the_store_makes`);
  both SDKs gain the supervisor's grant verbs — `grants_pending()` / `grantsPending()` over
  `GET /gateway/grants/pending` and `grant_decide()` / `grantDecide()` over `POST /gateway/grants/{agent}/{request}`
  — and the A2A task verbs `get_task` / `getTask` and `cancel` (`tasks/get`, `tasks/cancel`); the grants cell was
  mis-cut, since a grant is a mandate and already rides in `_meta.mandate`. **Cancellation is carried by no adapter**,
  and the table now says so: `tasks/cancel` ends a task's record and stops no work (a task runs inside its request),
  `tools/call` has no cancel, a dropped future tells no provider; running work stops only when its authority lapses
  (`authority_drain`). The A2A door's session cell reads *not carried, by design*: a task lives in the node that
  accepted it, and the work it tracks is not durable either — durable work is the tuple space's and the commitment
  net's. **Not built, stated:** the table records; it does not make an adapter carry anything, and no cell is
  measured at runtime.
- **`mycelium test --faults`** (`docs/plans/adopter-surface.md` X6; `docs/features/test-faults.md`): a fleet of
  in-process nodes built from a declaration directory and a configuration — one node per unit file, three at least,
  each the configuration's with loopback addresses, free ports, persistence and TLS material under a temporary
  directory, no gateway port, every node bootstrapping to every other — and the fault classes the repository already
  proves run against it: `revoke` (a node rotates its identity on compromise; a write signed by the old key, the old
  certificate at a peer's handshake and an identity record chained through the old key are each refused; the
  declaration's first `[[mandate]]` is established at an execution authority and not established once a checkpoint
  revokes its term), `restart` (a write requiring sync on every node acknowledged at rung 2, replayed after every node
  restarts over its own directory; a WAL record that does not decode refused at start by name, or moved aside under
  `quarantine`), `partition` (a node's runtime dropped: evicted from every survivor, its offers gone after their
  evaporation window, each declared `[[group]]` governed to a band of one re-filled by an eligible survivor, the node
  back under its identity), `replay` (each unit's node recorded alone under the kernel and replayed with the clock paused, the
  decisions compared, the first divergence reported — the `sim` build only, `not_in_build` naming `sim` elsewhere;
  the schedule between nodes is outside the seams, so a fleet recorded together diverges at its first cross-node
  channel choice, stated) and
  `policy-diff` (the declaration's call set decided under its `[[rule]]`s and recorded at the command's own
  enforcement point, each record re-decided under `--against`'s `[[rule]]`s, every call the proposed revision permits
  and the declared one refused listed; a record not joinable to its inputs *unattributable*). The report lists per
  class the guarantee and rule ids exercised, each with its resolution on the fleet's node, and the ones it could not
  exercise and why, in the guarantee report's vocabulary; a class that cannot run fails the run only when asked for by
  name; a declaration `wire-check` refuses is refused with the same findings; **no activation is ever run** (D23).
  The declared profile is not applied to the test fleet (it needs a gateway and code attachments the fleet has not)
  and is reported as declared; `--keep` leaves the run's directory; `--decisions <evidence.log>` hands `policy-diff`
  records written elsewhere, re-decided from what they carry. Library: `mycelium::faults::{run, Class, Options,
  Report}`. `mycelium test` logs at WARN to stderr so the report is the output. **Not built, stated:** a two-sided partition (the Docker suites'), a load class, the verifiers an
  in-process fleet cannot reach from outside the node (consensus, caller envelopes, knowledge records), a
  provider-side run of the call set.
- **The external side-effect contract** (`docs/plans/adopter-surface.md` X5; `docs/features/side-effects.md`):
  `mycelium-effects::external` — an `ExternalEffect` carries its destination's **declared class** (`EffectClass`:
  idempotent under the key · compensatable by a named effect · irreversible) and a `ProbeBudget`, recorded on the
  prepared record (`prepared_record_hash`, on the reconciler's own SQLite ledger before the destination is reached);
  the `Reconciler` applies it through an `ExternalDestination` (`send` and `observe`, the reconciliation probe),
  maps a lost answer to `DeliveryUnknown` and the record to `Unknown`, requires a probe before any retry, makes at
  most one retry and only against a destination declared idempotent, never probes or retries an irreversible
  effect, marks a record past its budget `Unresolved` and escalated (`Execution::Unresolved`), gates the effect and
  every probe on the egress allow-list, enforces the composed legs on the external path, and commits a compensation
  only under a `CompensationPermit` (the evaluator's, `envelope` feature; bound to the compensation it is for —
  operation, destination, parent, validity, principal) as a new effect whose evidence record's
  `parent` is the compensated record's hash (a lost compensation keeps its link and is completed by the probe
  that settles it); a found record binds what may be presented under its operation (other content a `Conflict`,
  another class or scope a `DeclarationMismatch`), and a record found `Prepared` is unknown, never re-sent — every forbidden move refused by name (`ExternalRefusal`) and counted
  (`RefusalSnapshot::external`, `mycelium_effects_refusals_total{kind="external"}`). The apply and the probe are the
  first production use of the replay kernel's `Input` seam (`mycelium::sim_seam::input`): a recorded run that
  reached the destination replays reaching it zero times (`tests/external_replay.rs`, under the crate's new `sim`
  feature). Reference destinations: the SQLite one (local, its probe the dedup row) and `HttpDestination`, a
  mock-grade HTTP shape whose test harness loses responses. **Not built, stated on the page:** a real provider,
  credential binding on the reference HTTP destination, a policy-authorised probe, the exposure policy over the
  unresolved age, an operator route over the ledger (`deprecations.md` §60).

- **Grants — approval that is default-deny** (`docs/plans/adopter-surface.md` X3, first PR; `docs/features/grants.md`):
  `AgentPolicy::require_grant_for` and `grant_window`; `AgentStateMachine::transition_invoking(tool, arguments)`
  writes the request under `grant/{agent}/{request}` (tool, arguments digest and a bounded rendering, policy revision,
  expiry), waits on the wall clock for `…/issued`, verifies the grant with the node's execution authority under a
  possession proof signed by the node's identity, commits `Invoking` and returns the grant for the call's
  `_meta.mandate`; silence is `GrantNotPresented`, a refusal `GrantRefused`, and a plain `transition` to a
  grant-required tool is refused. A grant is a scoped mandate in its own scope (`grant:{request}`,
  `Mandate::grant_operations`, `EntitlementTable::entitle_grants`) with its binding in its signed operations; the
  authority refuses other arguments (`MandateRefusal::BoundToOtherArguments`) and the preflight a decision under
  another policy revision (`BoundToOtherPolicy`); the evidence record carries the grant's hash (`mandate_digest`).
  The supervisor's door: `GET /gateway/grants/pending` (`grant:read`), `POST /gateway/grants/{agent}/{request}`
  (`grant:issue`) — issuing is an action the evaluator decides with the issuer as the actor (`grant.issue`), a
  gateway with no evaluator issues nothing, and the answer's write returns its receipt's rung. `Rule::requiring_grant()`.
  `mycelium-guardrails` 1.1.0 labels the grant clause and says the veto clause is *veto, not grant*. A grant's
  standing is its issuer's checkpoint, not the gate's freshness (`RevocationView::grant_standing`); the request a
  grant answered is consumed by the commit it admitted; refusing is evaluated like issuing (`grant.refuse`); a grant
  is issued only for an agent on the issuing node (`409 not_this_node`). **Not built in this PR, stated on the page:**
  single use at the doors, the SDK verbs and an SSE stream, the restart re-wait, grants across nodes, a revocation
  route, the A2A `input-required` state, the data-class policy over the rendering, the bypass-matrix row and the drain
  bound (`deprecations.md` §59).

- **One id, and one parent, through the records** (`docs/plans/adopter-surface.md` X4, first PR; `docs/features/causal-records.md`):
  a request's `params._meta.correlation` (one key segment, the run's id) and `params._meta.parent` (the content hash
  of the record the work depends on — the hash `journal_sha256` cites) enter the `ActionEnvelope` at every door the
  preflight runs and ride on every evidence record it produces, decision and execution, at the gateway and at the
  provider (`/mcp` forwards the two keys inside the `tools/call` payload; the provider reads them there); the
  `DecisionRecord` (`correlation`, `parent_hash`) and the `WriteReceipt` (`with_causal`; `POST /gateway/kv` reads
  `correlation`/`parent` and echoes them) carry them where their producer has them. **The placement is the
  application payload and only there:** a header (`x-mycelium-correlation`, `x-mycelium-parent`) or a field in the
  caller envelope is never read and is counted once per request by door (`GET /stats` `causal_fields_misplaced`, a
  map); a malformed value is refused before policy (`PreflightRefusal::Malformed`, `request_malformed`, `-32602`),
  never corrected, with or without an evaluator attached. `AeEvidence` gained `node`, the enforcing node.
  `mycelium explain --correlation <id> <files…>` renders one run as one chain across evidence journals and decision
  traces, each line *captured*, *reconstructed* (joined by `operation_id`) or *unattributable*;
  `mycelium explain --closure <hash> <files…> [--fleet <n>]` the dependency closure of one record — authoritative
  over the journal, best-effort over the trace, every hole named with the sinks' drop counts and never filled, a
  federation edge a boundary, fewer journals than nodes said first. The trace comparison ignores the fields. A fuzz
  target (`closure_corpus`) asserts the reader terminates, prints its root and visits each record once whatever a
  hostile file puts in `parent`. **The second PR:** the provider's `parent` is the gateway's decision — `/mcp`
  forwards its own decision record's hash as the `parent` of the `tools/call` it sends (`Preflight::Proceed` carries
  `journal_sha256`), so the provider's record cites the gateway's and `tests/causal_closure.rs` reads a closure across
  three nodes' journals, every dependent record and no independent one, with a saturated sink's hole named; the
  **door registry** `scripts/causal-doors.txt` and its gate `scripts/check-causal-doors.py` (in `make check` and CI,
  a planted door caught by name), every door's row naming how the fields travel and its test or its gap; a member's
  direct call, `/a2a` on both generations and the LLM door shown; `mycelium-py` 0.2.13 and `mycelium-ts` 0.2.7 carry
  the fields on the A2A `send` verbs, the prompt-skill and federation `call`s and `set`, and echo them on
  `KvReceipt`; the forwarded `parent` falls back to the caller's where the gateway established no record (stated); `mycelium-reason` 0.11.0 puts a traced
  run's `run_id` on the route's records as `correlation`. **Not built, stated on the feature page:** an execution
  record at the provider, the stem activation's parent, the tuple-space and blackboard doors (`deprecations.md` §58).

- **The `one-hour` job** (`docs/plans/adopter-surface.md` § 1.1): `.github/workflows/one-hour.yml` runs when a Release
  workflow run for a tag push completes successfully (not on `release: published`, which a Release created with the
  workflow's own token never fires), and by hand against any tag, in `python:3.12-slim-bookworm` with no Rust
  toolchain, against the published artefacts only: it downloads and verifies the node, scaffolds the Python template
  and installs the SDK from the same tag (its version checked against the tag's `pyproject.toml`), runs the declared
  unit, the permitted write and the refused write, and runs `doctor`, timing each step and the total from the first
  `curl` to the last `doctor` into the job summary (the container's own setup on its own row, outside the clock), and
  fails past 3 600 s. A red run prints no number. Steps 4 and 5 of the claim, and the records half of step 3, are
  printed as *not yet measured* until X3, X4 and X9 extend it. The number the documentation prints is this job's
  measured time on the most recent release, rounded up to the next ten minutes.

### Fixed

- **A governed group's member that went dark no longer holds the floor** (found by `mycelium test --faults
  partition` against the co-op fixture, adopter-surface X6). A member's `grp/{group}/{node}` entry is written once at
  join and withdrawn only by a graceful shutdown, so after a crash every survivor's governor still counted it and
  none rolled to fill the floor. `converge` now counts a member only if it is this node or in its peer table
  (`membership.governed` revision 2 in the rule catalogue); with no peer at all it reads the roster as written, so a
  node restarting over persisted entries or isolated is not a fleet of one. A convergence target, never a guarantee:
  a peer evicted here and alive elsewhere is re-counted when it is heard again. The stale entry itself stays in the
  KV until the node returns: the fleet snapshot's group size, `/gateway/mesh/group` and the group-conflict diagnosis
  still count it, and may report an over-ceiling conflict for the state the governor is producing — stated on the
  feature page.

- **The Release workflow reads the annotated tag's message, not its commit's.** `actions/checkout` resolves a tag ref
  to the commit it points at, so the first real run (v2.37.0) titled the Release with the squash commit's subject and
  used its message as the body; repaired by hand from the local tag. The publish job now fetches the tag object and
  refuses a tag that is not annotated (on a dry run, a version with no tag yet is reported, not failed).
  `RELEASING.md` § 8 records that the `ghcr.io` package, created by the workflow's `GITHUB_TOKEN`, inherited the
  repository's public visibility (the source label links it; it does not set visibility), so the "open it once"
  step is a check, not an action; § 7 says how to recover from a lightweight tag.

## [2.37.0] — 2026-10-23

### Added

- **`mycelium declare`** (`docs/plans/adopter-surface.md`, row X10): `mycelium declare [project-dir] [--gateway <url>]
  [--token <bearer>] [--watch]` declares a project's `units/*.toml` to a gateway through `POST /gateway/units/declare`
  and writes its `skills/<ns>/<name>.md` as prompt templates through `PUT /gateway/prompts/{ns}/{name}` — once, or
  under `--watch` again whenever a file changes, polled by modification time and size. Each tick runs the same
  `wire_check::check` the subcommand runs first; with an error the previous declaration stands and the tick says so.
  A unit's previous handle is retracted before it is re-declared, so a renamed capability does not linger (a good
  edit briefly shows no provider, stated on the page); a unit the gateway refuses after the check passed has its
  previous text put back and is rejected once; a unit or skill file removed is retracted or deleted; under `--watch`
  every handle is heartbeated within its lease on its own timer, a handle the node lost (a restart) is declared again
  at the next tick, and Ctrl-C retracts; once, no lease is sent and the declaration lives until the node stops. Library:
  `mycelium::declare::{Watch, WatchConfig, Tick, run}` (feature `gateway`). The Rust template's skill moved to
  `skills/demo/greeting.md`, the layout the watch reads. **Not built, stated:** the plan's *Input*-class seam for a
  filesystem event source — a poll needs none, and a watched session is not something this tool records.

- **`mycelium init`** (`docs/plans/adopter-surface.md`, row X2): `mycelium init <dir> [--template <name>]` writes a
  scaffold from a template the binary embeds at compile time from `templates/`, pinned to the binary's release tag,
  into a new or empty directory only. Two templates: `governed-call` (Rust — two in-process nodes; a member's direct
  call to a provider's tool, permitted and refused by an `ActionEvaluator` enforced at the provider and recorded in an
  evidence journal; a decision trace; a prompt skill whose text is `skills/greeting.md`, answered by the echo
  backend) and `governed-call-py` (Python — the published node under a named-token table; an SDK agent declares a
  unit and writes a key, a second agent is refused `403 insufficient scope` by the gateway's own rule; its mandate
  and rule come back `not_enforced`, since the published node attaches no evaluator). `templates/` is the source of
  truth and CI's `Scaffold` job builds and runs both as projects, the Rust one against the checkout by rewriting its
  dependency to a path (a `[patch]` would still fetch the pinned tag); a test holds the directory and the embedded
  list equal. `init ""` and `init .` are refused: a scaffold never writes into the current directory. **Not built, stated:** a TypeScript template; a
  policy a node attaches from configuration, so the Python template's per-call policy waits on that.

- **`mycelium doctor`** (`docs/plans/adopter-surface.md`, row X1): the readiness report without a node — what `start()`
  would print and whether it would refuse, from a configuration file and the `GOSSIP_*` environment alone, by running
  `start()`'s own preflight chain and profile check (extracted as `preflight()` and `profile_check()` in
  `src/agent/lifecycle.rs`, which `start()` now calls) and the guarantee and confinement reports over a node that is
  built and never started: no port, no directory, no attachment. Plus the effective configuration, every field with
  its source (`default` · `file` · `env`), secrets as `set`/`unset`; `--format json`; `--units <dir>` then runs
  `wire-check`. Exit 1 when `start()` would refuse. Library: `mycelium::doctor::{run, Doctor, effective_fields}`.
  **What it cannot see:** an attachment made in code, and unreadable persisted state (R7's refusal stays with
  `start()`). Feature page `docs/features/doctor.md`.

- **The node is published** (`docs/plans/adopter-surface.md`, row X0): a `Release` workflow on every `v*` tag builds
  `mycelium`, `mycelium-stem` and `mycelium-artifact` for Linux x86_64, Linux aarch64 and macOS aarch64, smokes each
  Linux tarball in a clean bookworm container with no Rust toolchain, attaches the tarballs and `SHA256SUMS` to the
  tag's GitHub Release (creating it from the annotated tag's message if step 8 of the runbook has not), and pushes a
  multi-arch image to `ghcr.io/richardtathata/mycelium`, tagged by version and `latest`. The published node carries
  `cli,tls,compliance,a2a,llm,metrics`; the stem `stem,gateway,llm,object_store`. `mycelium --version`,
  `mycelium-stem --version` and `mycelium-artifact --version` print one line each. The dry run — the same build and
  smoke, the bundle as a workflow artifact, no Release and no image — runs on every pull request that touches the
  workflow, the Dockerfile or the three binaries' sources, and by `workflow_dispatch` once merged. **Not built, stated:** no
  signing of the binaries (the sums are integrity, not provenance), no Windows build, no package-manager formula.
  Wire v12 unchanged; no API change.

### Changed

- **`GossipConfig::validate()` creates nothing.** Its early check of `persistence.base_path` probed by `create_dir_all`,
  so loading a configuration created the data directory; it now reads the nearest existing ancestor and warns, and
  `start()` creates the node's directory as before (`deprecations.md` §57). Found by `doctor`'s own test.

### Security

- **`rustls-webpki` 0.103.16, `webpki-roots` 1.0.10, `webpki-root-certs` 1.0.10** for three advisories published
  2026-10-19/20: RUSTSEC-2026-0382 (`rustls-webpki`: certificate verification may accept distrusted intermediates
  when the configured anchors allow such a path), RUSTSEC-2026-0381 and -0383 (the bundled root stores allowed trust
  in distrusted emSign intermediates). **Where it matters:** `rustls-webpki` verifies the mesh's mTLS in both
  directions, against the operator's CA alone (`RootCertStore::empty()` plus the fleet anchor), so an emSign
  intermediate cannot chain there; gateway TLS is server-only and verifies nothing; the federation client verifies by
  SPKI pin. The node's `reqwest` 0.12 clients (egress, OIDC, LLM, A2A, MCP) use native-tls and the OS trust store,
  the object-store client the platform verifier — on Linux that is `rustls-webpki` over the OS store, the one
  shipped path where 0382 applies against public anchors — so **neither bundled root store is consulted by a shipped build**
  (`webpki-root-certs` is compiled only on wasm32) — 0381 and 0383 are defence in depth here. All three tracked
  lockfiles updated (`Cargo.lock`, `fuzz/Cargo.lock`, `examples/conway-gpu/Cargo.lock`); `cargo audit -f` reports none
  of the three. Wire v12 unchanged; no API change.

### Fixed

- **The `test-coverage` CI jobs no longer fail on the API's lag, or on a job that never ran**
  (`scripts/ci-test-coverage.py --fetch`). Read seconds after a needed job finishes, the jobs listing could still say
  `in_progress` although the job is in the coverage job's `needs` and had completed — two CI failures read that way
  (19 s and 23 s after completion, 2026-10-20 and 2026-10-23, one of them blocking a merge until a whole-workflow
  re-run). A job listed as unfinished is now re-listed, and a log that is not there yet re-fetched, each within one
  bounded wait (`CI_TEST_COVERAGE_WAIT_S`, 120 s; polled every `CI_TEST_COVERAGE_POLL_S`, 10 s; a value that is not
  a number of seconds is refused by name), and only then is it the same failure by name. A third failure
  (2026-10-20) was a macOS job cancelled in the runner queue, outside `needs`, whose log never exists: a job that
  completed `cancelled` or `skipped` is no longer fetched, and `wasm-host-macos` is in `needs`. Five suite cases
  drive the fetch through stub `gh` and `curl`: the listing lag, the log 404, the cancelled job and the bad setting
  each fail the unchanged script and pass the fixed one; a job that never completes still fails within the wait,
  polled rather than spun.

- **A departed node's tally entry that outlives its collected heartbeat is collected** (post-360 row K2). The
  residue collector dated a non-peer's `sys/ae-tally/` keys by its heartbeat, and skipped a node with none. An entry
  that reached the collector after the heartbeat had been collected — by anti-entropy from a peer that had not
  collected yet, or a gossiped entry its heartbeat overtook — was therefore never collected: it stayed in the store and
  in the scope roster, and every later collection pass and decision listed its node again. The collector now takes a
  non-peer's keys **each by its own times**: an entry once every time is past its window plus the staleness and
  clock-drift bounds, or at once if its only times are forged (below); one that cannot be read with the heartbeat (a
  count that cannot be read is not zero — an older collector must not erase a newer node's entries during a format
  change); the heartbeat **last** — once
  none of the node's entries it holds is live and its `as_of` is past the longest window any counter may have
  (`MAX_FLEET_WINDOW_MS`, 24 hours) plus both bounds, because a collector cannot see the windows of entries held only
  elsewhere (#656's second review, item 3: dated by the entries it held, a collector lacking a member's long-window
  entry removed the heartbeat early, and the nodes holding that entry then read it without one). A departed node's
  heartbeat therefore stays up to 24 hours plus the bounds — one key per node, and about one extra KV read per decision
  for each node id that departed in that time. Found as an intermittent CI failure of `residue_is_collected_and_a_decision_does_not_read_it`
  (the node's own collector tick took a planted heartbeat before its entry was planted); fail-first
  `an_entry_outliving_its_collected_heartbeat_is_collected`.
- **A member's tally entries without its heartbeat are unknown coverage, not zero** (post-360 row K2, #656's
  adversarial review, finding 1). A reader skipped a non-peer it held entries for but no heartbeat, neither counting it
  nor calling it unknown, so a reader that received a crashed member's entries before its heartbeat — restarted without
  persistence, catching up by anti-entropy — read its permits as zero and could admit past the limit. Such a verified
  member's entry is now dated by its own times: inside the window plus the staleness and drift bounds, or unreadable,
  the tally is not established and a `FleetTally` rule decides `Indeterminate`, naming the node; the collector removes
  the entry on the same bound (the same when the writer's window is the counter's, as `publish_permit` writes it). A
  fabricated id is still ignored. **Upgrade note:** a tally rule can decide
  `Indeterminate` where it admitted before, for at most that window (`deprecations.md` §56). Fail-first:
  `a_members_entries_without_its_heartbeat_are_unknown_coverage` and the extended
  `without_tls_a_non_peers_absence_is_not_established`.
- **A forged tally time dates nothing** (post-360 row K2, #656's second review, item 1). Tally keys are unsigned, and a
  time past this node's clock plus `max_clock_drift_ms` — `t: [u64::MAX]` — never aged: it counted as a permit for
  ever, its owner's `publish_permit` kept it, and under a node with no heartbeat it held a counter `Indeterminate` and
  the node's scope-roster row for ever. Now a reader reads an entry holding such a time as one it cannot read (unknown
  coverage, naming the node); the owner, under its tally lock, rewrites its own entries without such times on every
  withdrawal tick — withdrawing one with nothing honest left — drops them when it publishes, and does not wait for
  them when it withdraws; and the collector dates an entry by its honest times only, so one holding nothing else is
  collected at once and the counter is decidable again. **The bound:** a forged time written into a live member's
  entry holds that counter `Indeterminate` for one heartbeat tick (a third of `control_max_staleness_ms`) plus its
  propagation, not its window (#656's second review, round 2). **Not closed:** honest-looking forged times — inside
  the window — still inflate a count; tally entries are unsigned (the stated K2 limit). A heartbeat dated in the future is no heartbeat: a non-peer's is dated
  by its entries, and the collector removes it once none of them is live. **Upgrade note:** a peer's forged time now
  makes a tally rule `Indeterminate` until the entry is rewritten or withdrawn, where it was counted
  (`deprecations.md` §56). Fail-first: `a_forged_future_time_dates_nothing`,
  `a_collector_without_a_members_entry_keeps_its_heartbeat`, `a_forged_time_in_a_live_members_entry_is_scrubbed_by_its_owner`.
- **A fleet history limit is bounded: at most 1 024 seats, a window of at most 24 hours** (post-360 row K2, from the
  review of the private Cedar adapter). For a `HistoryScope::FleetConsensus` rule the enforcement point read every
  seat, a slot digest and a KV read each, on every request the rule covered, synchronously under the actor's history
  gate on an async worker, so a limit of `u32::MAX` pinned a worker for minutes per request; and a fleet window had no
  horizon. A fleet-scoped limit past `MAX_FLEET_SEATS` (1 024), or over a window of zero or past `MAX_FLEET_WINDOW_MS`
  (24 hours, the history horizon), is now **not established**, never clamped: the reference evaluator decides
  `Indeterminate` naming the bound, the enforcement point reads none of the counter's seats or tallies and claims no
  seat, and `preflight_with_history` turns any evaluator's permit relying on one into *not established*. New:
  `FleetBoundsError`, `check_fleet_bounds`, `FleetCounter::try_new` and `FleetCounter::check_bounds`, so an adapter can
  refuse such a policy at load; three contract cases (`fleet_limit_past_the_seat_cap`, `fleet_window_past_the_horizon`,
  `fleet_limit_at_the_bounds`). **Upgrade note:** a policy whose fleet limit is outside the bounds still builds, and
  every request its rule covers now decides `Indeterminate` (`deprecations.md` §55). The contract suite counts an
  adapter's refusal of such a policy at load as a pass on those two cases (`ContractCase::accepts_refusal_at_load`,
  `Report::refused_at_load` — both structs gained a field; only for an adapter that passed `fleet_limit_at_the_bounds` by building), and a stored tally entry's window is clamped to the horizon
  when it is aged or collected, so an oversized one no longer lingers.

### Changed

- **Tests:** the activation egress proxy's flood test was a test race, not a cap violation (CI run 39392016584,
  macOS: *"A holds no more than its share: 8"*). `a_flood_on_one_proxy_leaves_every_other_proxy_its_share` read the
  whole budget's count straight after both commands were answered, when a socket the proxy has answered but not yet
  closed — B's connection, or an upstream socket whose relay task has not ended — is still a descriptor held, so the
  count can be 8 for a moment with A at 6 of its 8. The test now samples A's own count for the whole run (never past
  its share), holds the whole budget to 7 at a moment with nothing in flight (the flood settled), and waits out the
  closing sockets before the final check. Seen failing first with CI's exact message by holding an answered socket
  open 300 ms before close; the new test passes with that delay and fails with A's accept limit raised by one.

## [2.36.1] — 2026-10-20

A **security PATCH**, cut from the v2.36.0 tag: `wasmtime` 49.0.4 for RUSTSEC-2026-0378 and -0379. Wire v12
unchanged; no API change (`PREV = 11`; a rolling upgrade from 2.36.0 holds). Nothing else from `main` is in this release.

### Security

- **Check whether you are affected:** run `uname -m` on each node. A node is exposed to RUSTSEC-2026-0379 only if it
  reports `aarch64` or `arm64` and it hosts WASM components — it runs `mycelium-stem`, or a binary that links
  `mycelium-wasm-host`; on such a node a non-zero `mycelium_artifact_installs_completed_total` means it has installed
  components. After upgrading, `grep -A1 '^name = "wasmtime"$' Cargo.lock` in the build shows `49.0.4`, and
  `cargo audit -f Cargo.lock` no longer reports RUSTSEC-2026-0378 or -0379. A build that depends on `mycelium` by git tag resolves
  wasmtime in its own `Cargo.lock`: run `cargo update -p wasmtime -p wasmtime-wasi -p wiggle` there. Moving the tag
  alone leaves 49.0.3 locked, and that update alone fixes a v2.36.0 build too.
  **Dependency: `wasmtime` / `wasmtime-wasi` 49.0.3 → 49.0.4**, with the cranelift family 0.136.3 → 0.136.4
  (RUSTSEC-2026-0378, -0379, published 2026-10-19). **RUSTSEC-2026-0379** (critical, CVSS 9.9): on **aarch64**,
  Cranelift could compile a large Wasm `br_table` to a jump to the wrong target, so a guest component's control flow
  is not the one it was validated for. The WASM host compiles with Cranelift (wasmtime's default; it selects no other
  strategy), so a node that hosts WASM components on aarch64 — Apple silicon, Graviton, Ampere — is exposed; on
  x86_64 it is not. **RUSTSEC-2026-0378** (medium, 5.7): a host panic lowering a host-created component-model stream
  into a guest that expects a different payload type; the host's WIT (`wit/host.wit`) declares no stream or
  future and it links WASI p2 only, so no host-created stream reaches a guest. **Affected releases:** every release that ships the WASM host — v2.0.0 through v2.36.0 for -0379 (wasmtime
  45.0.3 to 49.0.3; v2.34.0–v2.36.0 on 49.0.3), and v2.4.0 through v2.36.0 for -0378 (wasmtime 46.0.2 onwards; 45.x is
  unaffected). A node built without `mycelium-wasm-host` does not link wasmtime and is not exposed. A lockfile change
  only (`Cargo.lock`; the two other tracked lockfiles hold no wasmtime), no API change; the minimum Rust version
  wasmtime 49 requires (1.96.0) is unchanged.

## [2.36.0] — 2026-10-19

**Two old defects fixed, two security fixes, and the post-360 plan's rows after the comparison.** A request-reply call to
a provider on the calling node could lose its reply and wait out its whole timeout although the handler ran: the
caller registered for the reply only after emitting the request. That shared path serves `/mcp`, `/a2a`, the raw RPC
routes, `scatter` and `bulk_call`. A graceful shutdown could also stop before the tombstones of its soft-state sweep
were on the wire, so a peer kept routing to the stopped node until the key's TTL. Both predate this release; see the
entries under Fixed. Under Security: `GET /gateway/rpc/serve/{kind}` let a `mesh:serve` client stream `llm.invoke`,
`mcp.invoke` or `skill.invoke` and answer such requests first, and a confinement profile could be widened for good by a symlink planted
while it resolved.

The post-360 rows delivered since 2.35.0:
- **Inference routing as policy (R):** the action evaluator sees where a routed model call would go (the
  destination and its declared locality) and the request's data class, per candidate, before the router commits.
  *Confidential only in-domain* is expressible, and `mycelium-reason` 0.10.0 is the first routing door.
- **History limits across the fleet (K2):** a history-limited rule may count exactly, by consensus seats, or
  approximately, by a gossiped tally, the policy choosing per rule.
- **An egress proxy for activation commands (L and M):** a confined `[[activation]]` reaches hosts by name through
  a proxy its stem runs, with a list that changes live (`SIGHUP`) and credentials attached at the node's gate.
  Every confined command now starts with a cleared environment.
- **A confined re-probe re-resolves a replaced path (M's follow-up)** instead of withdrawing the install.

Wire **v12** unchanged (`WIRE_VERSION` 12, `PREV` 11). Companions on their own tags: `mycelium-reason` 0.10.0,
`mycelium-py` 0.2.12, `mycelium-ts` 0.2.6. Unchanged are `langgraph-checkpoint-mycelium`,
`strands-session-mycelium` 0.1.0 and `mycelium-guardrails`.

**Check before upgrading** (`docs/guide/deprecations.md` §50–§54):
- **A client serving `llm.invoke`, `mcp.invoke`, `skill.invoke`, `rpc.result` or `bulk.result` over
  `GET /gateway/rpc/serve/{kind}`** is refused `403 protected_kind`. A kind the node serves in process is refused
  `403 served_in_process`, and a client stream is closed (an `evicted` event) when an in-process receiver takes its
  kind. The SDKs raise `ProtectedKindError` / `ServeEvictedError` (`mycelium-py` 0.2.12, `mycelium-ts` 0.2.6).
- **A gateway client writing an `llm-meta` ad** (`capability/advertise`, `units/declare`) is refused
  `403 reserved_namespace`, with or without `locality`. Serve a model with `mycelium_reason::serve_model` on the node.
- **A classifying router refuses every request while no action evaluator is attached;** attach it, and the journal,
  before `start()`. The façade's `403` now means every candidate was refused.
- **A confined activation command starts with a cleared environment.** Name a variable it needs in
  `[activation.confine] env`. Two `[[activation]]`s for one capability are refused.
- **A `HistoryScope::FleetTally` rule fails closed** until every member runs this release built with `gateway` and
  `tls`. Deploy the release fleet-wide before a policy names it.
- **A probe that replaces one of its own declared paths on every run** now fails as `confinement_changed`; trace
  consumers read `prov.probe`'s three new refusal reasons (rule revision 3).
- **An embedder building a `mycelium_core::CoreCtx` or driving the writers directly** meets the counted outbound
  types (`GossipTxs`, `WriterTx`, `CountedRx`). Code that only uses `GossipAgent` is unaffected.
- **Struct literals and matches:** `ConfineDecl`, `Rule`, `HistoryLimit`, `HistoryView`, `HistoryInput`,
  `mycelium-core`'s `KvStore` and `mycelium`'s `SystemStats` gained fields; `activation_sandbox::Refusal` gained a variant; in
  `mycelium-reason`, `RouteError` gained `Unclassifiable` and `Routed` two fields. An exhaustive literal or `match`
  breaks; `..Default::default()` and a `_` arm do not. A wrapping evaluator forwards `fleet_counters` too.
- **Catalogue revisions:** `ae.preflight` 4, `prov.probe` 3, `prov.activation` 3, `prov.eligible` 2;
  `stem.activation_confined` revision 2.

### Security

- **A client serve stream could receive, and answer first, every model or tool call its node was sent**
  (`GET /gateway/rpc/serve/{kind}`; every release from 1.0.0 through 2.35.0 for `mcp.invoke` and `skill.invoke`, from
  1.1.0 for `llm.invoke`; reachable with `mesh:serve` since 2.15.0 (and `mesh:read`/`mesh:write` through 2.18.1),
  `mesh:read` from 1.2.0, the single gateway token in 1.1.0, no credential in 1.0.0, and by any caller wherever the
  gateway had no credential model). The route registered a receiver for whatever kind it was given. `llm.invoke`, `mcp.invoke`
  and `skill.invoke` fan out to every receiver on a node, so a client stream for one received each such request, the
  routed prompts included, beside the node's own provider, and could answer before it. It now refuses those kinds and
  the reply kinds `rpc.result` and `bulk.result` with `403 protected_kind`, refuses any kind the node serves in
  process (`403 served_in_process`), and closes a client stream with a terminal `evicted` event when an in-process
  receiver registers for its kind later (counted, `signal_client_evictions`). An operator-listed protected kind
  (`protected_rpc_kinds`) stays servable and is checked by provider enforcement. `mycelium-py` 0.2.12 and
  `mycelium-ts` 0.2.6 raise `ProtectedKindError` for either refusal and `ServeEvictedError` on eviction
  (`deprecations.md` §52). Found by #643's review. **Check after upgrading:** `curl -s --max-time 3 -H "Authorization:
  Bearer $SERVE_TOKEN" http://<gateway>/gateway/rpc/serve/llm.invoke` must answer `403` with `"error":"protected_kind"`
  — an SSE stream that stays open means the node is not upgraded; refusals are logged. The old route recorded no
  refusal, so past use cannot be read from the node: list which tokens could open the route on your version (above).

- **A symlink planted while a confinement profile resolved could widen it for good** (post-360 row M; #641's review,
  finding 1, present since 2.35.0). There was a window between `prepare`'s symlink check and its `canonicalize`, and
  another before the Linux pin, which opened without `O_NOFOLLOW`. A confined command still running in that window
  could swap a declared path, or any component of it below a writable root, for a link to a directory the profile
  never named. Two further routes existed: an operator symlink pointing into a root's interior, and a placement
  directory inside a write root. Either way the profile would name and pin the link's target, and every later run
  would reuse it. Now every path is resolved from `/`, one component at a time from its parent's descriptor. A
  symlink is followed only before the resolution enters a writable root, recognised by identity rather than spelling.
  The identity comes from what was opened, Landlock's rule from that descriptor, and every currency check walks the
  path again. A profile runs only while it is current. On Linux, a symlink on a filesystem that holds a write path and
  is mounted more than once, such as by a bind mount, is not followed (`unit-file.md` § Mounts). The rule's limit:
  views of a write path through another filesystem are not detected. These are FUSE `bindfs`, overlayfs, and NFS or
  SMB loopback.

### Fixed

- **A call to a provider on the same node no longer loses its reply.** `rpc_call` emitted the request and only then
  registered for the reply. A provider on the calling node receives the request inside the emit, and on a
  multi-threaded runtime its serve task can run on another worker at once; when it answered before the caller had
  registered, the reply was dropped and the caller waited out its whole timeout although the handler ran. Every
  request-reply call shares the path: `ServiceHandle::rpc_call` and its variants, `scatter`, `bulk_call`, and the
  gateway's dispatch from `/mcp`, `/a2a`, `/gateway/rpc/call`, the overlay and the LLM doors. The caller now registers
  before it emits, and the registration is removed when the call ends, including a cancelled one, which used to leave
  its entry behind. This was the `/mcp` door-test flake recorded open under #642 (#647). No API change.
  Beside it: a registration removes only its own entry (a colliding nonce is re-drawn, never overwritten); a reply
  sent through `MeshHandle::emit_async` (`emit_signal_async`) to a caller on the same node is now claimed — that
  emitter skipped the reply interceptor, so such a reply went to the fan-out and the caller timed out; and a SWIM probe
  dropped mid-flight (its task aborted or its runtime shut down) no longer leaves its entry pending.

- **A graceful shutdown waits until the frames it queued are on the wire.** The shutdown sweeps the node's soft
  state (`cap/`, `req/`, `svc/`, `tools/`, `gcap/`, `sys/load/`) and then waited until the gossip and writer queues
  were empty before signalling the tasks to stop. A frame a shard worker or a writer had already dequeued was in no
  queue, so the wait could end while the sweep's last tombstone was still in a writer's write or connect, or between
  a shard worker and a writer, and the signal stopped it there. The peer kept the swept key, and kept routing to
  the stopped node, until the key's TTL. The node now counts every outbound frame from the moment it is enqueued
  until a writer has flushed it to its socket or dropped it, and the shutdown waits (bounded, 250 ms) for that
  count to reach zero, so the sweep's tombstones are written before `shutdown()` returns, and a process that exits
  straight after it no longer cuts them off. A frame dropped on its way to a dead or unreachable peer releases its
  place at once, so it does not hold the shutdown; a frame released twice would saturate the count at zero and is
  counted (`gossip_outbound_count_underflows_total`), so it cannot hold every later shutdown either. This was `a_fast_reassert_does_not_undo_the_shutdown_sweep`'s
  failure on `main` at 98a311fc (about 1 run in 100 under parallel load, on v2.35.0 as well: it predates this
  release).
  **Upgrade note:** `mycelium_core`'s transport types changed shape: `CoreCtx::gossip_txs` is a
  `mycelium_core::outbound::GossipTxs`, `WriterEntry::tx` a `WriterTx`, and `writer::get_or_spawn_writer` and
  `writer::request_state` take the node's `Unsent` count; `framing::dispatch_gossip_send`,
  `dispatch_gossip_try_send` and `gossip_shard_fill` take `&GossipTxs`. Code that only uses `GossipAgent` is
  unaffected (`deprecations.md` §54).

- **A confined activation's re-probe no longer withdraws an install whose declared path was replaced legitimately**
  (post-360 row M's follow-up, #641). A write directory rotated or a read file replaced by an atomic rename failed
  the next re-probe, so the install was withdrawn and reinstalled, a flap if the path churned. The re-probe now
  re-resolves the profile, repeating every check (symlinks, the canonical comparison, existence), and runs the probe
  under the fresh profile. The install is withdrawn only when re-resolution refuses, and the trace names the refusal
  (`prov.probe`'s `confinement_failed` / `confinement_unavailable`, rule revision 3). A path that changes while the
  probe runs makes the verdict untrusted: the probe is re-resolved and run once more, and a change under both runs is
  `confinement_changed`. A path replaced by a symlink is refused, never followed.
  - A rotation by flipping an ancestor symlink (`current` → `v2`) is now seen and re-resolved. Before, the old profile
    stayed current and wedged the install.
  - The activation's initial probe takes the same check.
  - A re-probe round runs its entries concurrently, at most eight at once, so one churning entry delays no other.

  **Upgrade note:** a probe that replaces one of its own declared paths on every run now fails as
  `confinement_changed` (`deprecations.md` §50).

- **A fleet-consensus seat claim that needed a second ballot is no longer refused "decided too late"** (row K2,
  found before the cut; not in any release). A seat's lease is stamped before the claim's first ballot, and the call is admitted only if a whole
  window of the lease remains. That makes the lease margin the latency a claim is allowed. The margin was 2 s, exactly
  one phase timeout, so a claim that lost one ballot to its timeout was always refused `Indeterminate`. Under load that
  happened about 1 run in 50. The margin is now derived from the claim's bounds: 19 225 ms, the longest a claim is
  allowed to take. That covers four ballots each waiting out both phases, the convergence poll, and a 2 s allowance for
  local work, which is an allowance, not a timeout. The admission check is also made on the monotonic clock, so a
  backward wall-clock step cannot admit a slow claim. A decision whose lease had already ended when it was read is
  refused "too late" at once, not reported as unobserved after a second of polling.

  So that the longer lease does not throttle short windows, the holder now releases a seat one window after its permit
  is admitted, timed on its monotonic clock. The lease ends the seat only if the holder crashes or shuts down first.
  Before, a used seat stayed held for its whole lease; at a one-second window that admitted about a third of the limit
  with the old margin, and about 5 % with the new one. Safety is unchanged: one seat admits at most once per window,
  to within the nodes' wall-clock skew. The release path needs no clock agreement, because the holder writes its marker
  at least a window after admission. A node that instead reads the lease lapse on its own wall clock is covered for
  skew of up to about the margin (`deprecations.md` §53).

### Added

- **An egress proxy for activation commands** (post-360 rows L and M — the three items those rows left *not
  built*). An `[activation.confine]` profile may name `[activation.confine.egress]`: `hosts` (`https://host[:port]`
  or `http://host[:port]`) and `credentials` (the node's `[[egress_credentials]]` bindings the command may name);
  variables the command keeps are named in `[activation.confine] env`. The stem starts a forward proxy for that
  activation on `127.0.0.1` when it starts, confines the command and probe to the proxy's one port, and starts them
  with the proxy's address (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, lower-case forms, `MYCELIUM_EGRESS_PROXY`) and
  a per-proxy token. The proxy carries requests only to the listed hosts, **by name**, under the node's egress
  allow-list: an absolute-form `http://` request is sent on over the entry's scheme (TLS for `https://`) through
  the node's egress gate, which attaches a placeholder's value only where its binding authorises; an `https://` URL
  goes through `CONNECT`, a tunnel that carries no credential — the proxy terminates no TLS and mints no CA.
  Redirects go back to the command. The list **changes live**: `Stem::set_activation_egress_hosts`, and
  `mycelium-stem` re-reads its unit file on `SIGHUP` (a reload narrows a proxy whose activation the file no longer
  names to nothing; a file that does not load leaves every list standing). A proxy that cannot start refuses its
  activation by name (`prov.activation`'s `egress_proxy_unavailable`, the rule's revision 3); a credential that is
  not a binding on the node, or a host the node's allow-list denies, refuses the stem's start. New guarantee
  `stem.activation_egress` (`enforced` under Seatbelt; `not_configured` under Landlock, which limits the proxy's
  port and not its address, and for a `localhost:<port>` `connect` entry). New `EgressCredentials::send_once` (the
  gate's first hop, no redirect followed). `docs/reference/unit-file.md` § `[activation.confine.egress]`.
  - **A value cannot reach the command.** Every confined command starts with a cleared environment (`PATH`, `HOME`,
    `USER`, `LOGNAME`, `LANG`, `LC_ALL`, `LC_CTYPE`, `TZ`, `TMPDIR` and `[activation.confine] env`). A stem refuses
    to start when a binding's value could reach a confined command: an `env` entry naming a binding's `value_env`,
    an `env` value containing a binding's value, or a `value_file` the command could read (judged through a
    symlink, and refused with a second hard link). A value echoed by an upstream is masked in every relayed header
    and body (`mycelium::Redactor`, longest first, across chunk boundaries); an encoded answer is refused where a
    value could hide in it (an answer with no body is relayed whatever its `Content-Encoding`); a tunnel to a
    granted binding's host is refused.
  - **Only this user's processes reach the proxy.** It closes at accept every connection another user's process
    opened, or whose owner the kernel cannot name: on Linux by a `sock_diag` query for the connection's exact
    addresses (`/proc/net/tcp{,6}` only where netlink is unavailable), on macOS from `net.inet.tcp.pcblist_n`, read
    once per batch of waiting connections and bounded at 2 s (a read past it refuses its batch, fail closed). While
    64 connections wait to be named the proxy accepts nothing more, so newer ones wait in the kernel's listen
    backlog. A dual-stack client and a half-closed one are named.
  - **What one proxy holds.** 64 connections that presented the token (a tunnel for its life; one past it is
    refused 503); 16 that have not (each closed after 2 s or on its 407, the oldest closed to admit a newer one); at
    most 256 sockets; header reads bounded at 10 s, idle tunnels at 5 min, buffered bodies at 4. Every proxy of the
    stem shares one descriptor budget, a quarter of the soft `RLIMIT_NOFILE` (not raised), each drawing on its share
    (at least 8 — an egress activation that would push it lower is refused at start) with a quarter kept for
    upstream sockets; a proxy at its share stops accepting, so a flood on one leaves the others theirs. No idle
    upstream connection is kept.
  - **Not built:** a credential inside a `CONNECT` tunnel; limits by path; addresses rather than names; on Linux, a
    limit on the proxy's port by address; `credentials` and `env` changing live; a proxy for an unconfined activation.
- **A history-limited rule may count across the fleet, the policy choosing per rule** (post-360 plan row K2, decided
  2026-10-17; upgrade note `docs/guide/deprecations.md` §53). Row K counted at one enforcement point, so an actor that
  spread its calls over N gateways could be admitted up to N times a limit. `Rule::at_most(n, w).scoped(scope,
  counter_id)` now names one of two fleet scopes and an opaque counter id; the per-point count stays the default.
  - **`HistoryScope::FleetConsensus` — exact.** A counter of `n` seats, each a consensus slot (`ae-seat/…`) leased
    for the window plus a margin, the longest a claim is allowed to take (`SEAT_LEASE_MARGIN_MS`, 19 225 ms), and
    released by its holder a window after its permit is admitted. A permit claims a free seat by an exclusive consensus write *before* the work is
    admitted — after the per-actor history gate is released, so a claim waiting on a quorum holds up no other call —
    and a seat whose remaining lease no longer covers a whole window is refused, so one seat cannot admit twice in a
    window. The fleet admits at most `n` in any window, whichever gateways the calls reach — to within the nodes'
    wall-clock skew. Every seat held: `Deny`. No quorum, the electorate refusing or a claim too slow: `Indeterminate`.
    Decided in the exclusive verbs' electorate (the declared `exclusive_default` group, the cluster otherwise). The
    reference evaluator does not establish it over a wildcard actor: a seat's records are kept for good.
  - **`HistoryScope::FleetTally` — approximate.** Every node built with `gateway` and `tls` publishes a heartbeat
    (`sys/ae-tally/{node}/hb`, saying whether it enforces) from `start()`, and at a graceful shutdown a last one
    marked departed, its entries kept to age out; an enforcement point
    publishes, per counter and actor, its recent permit times (`sys/ae-tally/{node}/c/…`, self-owned under the `sys/`
    tripwire, withdrawn once they age out). A limit reads the sum over this node and every enforcing peer.
    Available without a quorum; **detection, not prevention** — every decision says so and names its coverage. A node
    whose own view fails the governors' confidence bound, or a peer with no heartbeat, a stale one or one from the
    future, gives *not established*, never an empty tally.
  - `ActionEvaluator::fleet_counters` (default: none) names the counter a decision would rely on; the enforcement
    point reads each into `HistoryView::fleet` before the evaluator decides. The evidence record names the fleet scope
    and carries each counter's state and coverage, and an admitted call's seat (`history.fleet`, `history.seats`).
  - At a provider under enforcement, a check that must claim a seat — decided from the envelope the check builds —
    settles the claim beside the serve loop (`rpc_rx`, the MCP tool loop, the bridged MCP loop and the `rpc/serve`
    stream), at most 64 at a time, so it does not stall the requests behind it; every other check stays inline. A
    panicking claim, or one still waiting when its loop ends, is answered `authority_not_established` and stopped:
    nothing more is claimed or recorded for it, and its provisional permit is taken back.
  - A node that is no longer a peer — departed, or declared dead — still counts until its permits age out, if it is a
    verified member (a trusted identity, not removed); a fabricated node id counts for nothing. Past the horizon (window
    + staleness + drift) its keys are not read, and any node collects them with tombstones stamped at the observed
    records' own timestamps, which lose to anything the owner wrote later; a returning node re-asserts its own keys
    before its connections come up. A decision lists nodes from the store's per-prefix scope roster, never every
    entry nor another namespace's scopes. Without `[tls]` a non-peer inside the horizon is unknown coverage
    (`Indeterminate`); with `require_identity_proofs = false` a fabricated id can count again. Not built: tally entries
    are unsigned, so forgery under a dead member's namespace is not detected.
  - `mycelium-core`: `ops::kv_delete_held(ctx, key, observed)` — a tombstone at the stamp of the record held, refused
    unless that record is the observed one; `ops::kv_write_local` (`#[doc(hidden)]`, for this crate's start-up
    re-assert); `store::scopes_under_visiting`; and `KvStore` gained `scope_roster` (an exhaustive struct literal
    breaks). A `/mcp` dispatch timeout logs its diagnostics and tells the caller only that it timed out.
  - An admitted decision's permit is no longer taken back once its record has been handed to the journal: an abort
    there is delivery unknown, and the permit stays counted. The `rpc/serve` stream's loop pulls one request per event
    the client asks for, and answers every request it took when the client goes, on eviction or at shutdown.
  - **Upgrade order:** tally rules fail closed until every member runs this release built with `gateway` and `tls`
    (`docs/guide/deprecations.md` §53).
  - The AE contract suite gains `PolicyClause::AllowAtMostAcross` and six `fleet_*` cases; `ae.preflight` is catalogue
    revision 4.
  - Tests: `the_consensus_scope_admits_at_most_n_across_three_gateways_under_concurrent_calls`,
    `the_consensus_scope_admits_exactly_n_spread_over_gateways`, `without_a_quorum_the_consensus_scope_is_not_established`,
    `a_claim_waiting_on_a_quorum_holds_up_no_other_call`, `a_provider_claim_does_not_stall_its_tool_loop`,
    `a_seat_decided_too_late_for_its_window_is_not_admitted`, `a_seat_is_reclaimed_after_its_window`,
    `under_a_required_electorate_the_seats_are_decided_in_it`, `the_gateway_and_the_provider_count_apart`,
    `a_wrapper_that_hides_the_fleet_counters_is_not_established`,
    `the_tally_scope_converges_and_refuses_after_convergence`, `a_restarted_node_keeps_its_heartbeat_fresh_without_permits`,
    `an_aged_tally_entry_is_withdrawn`, `an_isolated_node_does_not_establish_a_tally`,
    `a_stale_publisher_makes_the_tally_not_established`, `a_peer_with_no_heartbeat_makes_the_tally_not_established`,
    `a_heartbeat_from_the_future_makes_the_tally_not_established`, and the evaluator's unit tests.
  - **Not built:** a seat released before its window has passed (a seat is held for its whole window, even if the
    call's evidence is not recorded or its dispatch fails); collection of a seat's records (each counter, kind of point and actor that ever
    claims keeps up to `count` slots of three to five keys on every member, and `count` shrunk acceptor entries on
    every voter); a tally entry signed by its publisher; a seat's release marker checked against its owner (any mesh
    member can write one, as for a lock: consensus here tolerates crashes, not lies, row A). As at one node, the gateway's doors count together and the
    provider check apart.

- **Inference routing as policy (post-360 plan row R).** The action evaluator can decide where data may go. A routing
  door hands the gateway's preflight what it established — `GossipAgent::gateway_preflight_routed` with `RouteFacts`:
  the `Destination` it is deciding on and its `Locality` (`in_domain` · `external` · `unknown`), the request's
  `DataClass` and who declared it (`ClassSource::{Principal, Declared, Default}`), and the destinations already refused
  for the call. `ActionEnvelope` gained `destination` and `data_class`; `AeEvidence` gained `route` (absent from a
  door that does not route, so a record without it serialises as before) and fills `destination`, which no door could
  state until now. The reference evaluator's `Rule::for_class` and `Rule::at_locality` express *confidential may only
  reach in-domain models*; a class or locality a door did not establish is **not established**, never in-domain — so
  `/gateway/llm/call` and `/gateway/llm/stream`, which classify nothing, cannot pass a class rule. The contract suite
  gains `PolicyClause::ForbidClassAt` and five `route_*` cases; `ae.preflight` is catalogue revision 3 with R (4 after
  K2). `mycelium-reason` 0.10.0 is the first routing door: it passes each candidate's declared locality and the
  request's class, and dispatches to the first candidate the policy permits; a classifying router
  (`reason_router_classified`) refuses every request `403 authority_not_established` while no action evaluator is
  attached, and holds a caller with no bound floor at the default class. A gateway client cannot set a model's
  locality: `/gateway/capability/advertise` and `/gateway/units/declare` refuse any `llm-meta` ad from a client, with
  or without `locality` (`403 reserved_namespace`), and a catalogue entry providing one is never installed
  (`prov.eligible` revision 2). `GET /gateway/rpc/serve/{kind}` refuses the kinds a node serves itself (under
  Security). Also: `mycelium-core`'s `SignalHandlers` gained `register_client`, `register_transient` and
  `client_evictions`, and `SystemStats` gained `signal_client_evictions` (also on `/metrics`; an exhaustive struct
  literal breaks); `GatewayPreflight::is_inert` is undeprecated (§38 deprecated it as unreferenced; it now has its
  caller). **Not built:** content inspection (the class is declared, never detected), observing where a provider's
  backend sends a prompt (the locality is its declaration), any class or destination at the provider's own re-check,
  and binding an ad's key to the member that wrote it (a fleet member can write another node's `llm-meta` ad). The
  private Cedar adapter does not yet enforce class or locality (its next re-pin). Upgrade notes: `deprecations.md`
  §52.

- **`mycelium-py` 0.2.12** and **`mycelium-ts` 0.2.6** (their own tags): `rpc_serve` / `rpcServe` raise
  `ProtectedKindError`, naming the kind, when the node refuses the stream (`403 protected_kind` or
  `403 served_in_process`) — before, an SSE content-type error (#643's round-2 review, E) — and the new
  `ServeEvictedError` (a `ProtectedKindError`) when the node closes the stream because a receiver of its own took the
  kind, rather than ending the worker's loop silently (round 3).

### Changed

- **Behaviour:** a confined `[[activation]]` command no longer inherits the stem's environment (2.35.0 passed it
  whole); `stem.activation_confined` revision 2. A unit with two `[[activation]]`s for one capability is refused.
- `mycelium::ConfineDecl` gained `egress` and `env`, `activation_sandbox::Profile` gained `env` and
  `activation_sandbox::Refusal` gained `EgressUnavailable` (`deprecations.md` §51). A `connect` entry naming a
  hostname is still refused; its message now points at `[activation.confine.egress]`.
- **Tests:** `mycelium-stem`'s SIGHUP reload is tested through the real binary (`mycelium-wasm-host/tests/stem_sighup.rs`:
  a host added to a confined activation's egress list is served by the running proxy after `kill -HUP`, and a unit
  file that does not load leaves every list standing), on Linux and macOS; the stem logs when its SIGHUP handler is
  installed. CI's macOS wasm-host step now runs the `peer_uid` tests, which its name filter had skipped.
- **Behaviour:** `mycelium-stem` installs its SIGHUP handler before it starts anything and for every unit file, so a
  SIGHUP no longer terminates a stem whose unit file names no `[activation.confine.egress]` (it logs that there is
  nothing to apply); a table added after start is refused by name (a restart starts its proxy).
  `docs/reference/unit-file.md` says to replace the unit file atomically (write, then `rename(2)`).
- **Tests:** two K2 test flakes were test races, not defects (#651). `an_abort_after_the_record_keeps_the_permit_counted`
  now loses one prepare on every run, pinning the lease margin, and asserts the record is a permit before the abort;
  `an_aged_tally_entry_is_withdrawn` checks the window's exact boundary from the entry's own instants, reading no
  clock. New: `the_heartbeat_tick_withdraws_an_aged_entry`.

## [2.35.0] — 2026-10-17

**Two security fixes, and the post-360 plan's comparison rows.** A revoked key's certificate is now refused at the mTLS
handshake, and the mesh no longer resumes TLS sessions: since closure plan C5 (2.15.0) a member removed after one
handshake could reconnect on its cached session, past the removal check (row Q's follow-up). A gateway caller envelope
is now served once, at the provider it was signed for: inside its freshness window a captured envelope could be
re-emitted and served again as the client's call (row E's nonce cache). See the entries under Security.

Six rows from comparing Strands Box, NVIDIA OpenShell and Strands Agents (rows K–P):
- **Policy over history (K):** an action evaluator can decide on what an actor has already done, counted at the
  enforcement point; "at most three posts in ten minutes" denies the fourth.
- **Credentials attached at egress (L):** a component or a served skill holds a placeholder, and the egress gate
  attaches the secret only on a request to the bound host and port.
- **Activation commands confined (M):** an `[[activation]]` can name a filesystem and TCP-port allowance, enforced
  by Seatbelt on macOS and Landlock with seccomp on Linux, and refused by name where it cannot be applied.
- **A2A at the current specification (N):** an unmodified `a2a-sdk` 1.2.2 client reaches a node's skills through
  `/a2a`, checked in CI.
- **A Strands session store (O):** `strands-session-mycelium` 0.1.0, a new Python companion; every write is claimed
  fleet-wide by a consensus write first, so nothing is written over what a node has not seen.
- **A Strands fleet, demonstrated (P):** three nodes, one plan, a mandated dispatch, and a resume after a node stops.

Found on the way: the `mycelium` binary never started a consensus listener, so a fleet of binaries could decide
nothing by consensus. And the concepts chapter now defines every term the README and tutorials use, starting with
the stem.

Wire **v12** unchanged (`WIRE_VERSION` 12, `PREV` 11). Companions: `strands-session-mycelium` 0.1.0 is new; unchanged
are `mycelium-reason` 0.9.0, `mycelium-py` 0.2.11, `mycelium-ts` 0.2.5, `langgraph-checkpoint-mycelium` and
`mycelium-guardrails`.

**Check before upgrading** (`docs/guide/deprecations.md` §44–§49):
- **Upgrade every gateway node, then watch the providers.** A 2.34 gateway's caller envelopes carry no id; they are
  accepted for this release and counted. Watch `caller_envelopes_without_id` (`GET /stats`) stop rising on every
  provider: the next release refuses an envelope without an id. An embedder reading raw signals calls
  `receive_gateway_caller` once per signal. A replayed or misaddressed envelope is dropped unanswered and counted
  (`caller_envelopes_replayed`).
- **A reconnect is a full TLS handshake,** and a certificate whose key its node revoked is refused. A direct caller of
  `mycelium_core::tls::load_or_generate`, `generate_rotation` or `generate_rotation_after` passes the key standing.
- **A unit file with an unknown key is refused,** at the top level or in an `[[activation]]`, where it used to be
  dropped silently: `units/declare` answers 400, and `wire-check` and a stem refuse it.
- **The old A2A paths are served for one more release:** move clients to `/.well-known/agent-card.json` and
  `SendMessage` / `message/send`, the skill in `metadata.skillId`.
- **The `mycelium` binary votes:** built with `consensus` (the default), it starts a consensus listener after `start()`;
  build without `consensus` to keep a binary out of quorums.
- **An evaluator that wraps another** forwards `history_request` and `evaluate_with_history`, or its history-limited
  rules decide `Indeterminate`.
- **Struct literals:** `Rule`, `ContractCase`, `NodeStanding`, `GossipConfig`, `OpenAiBackend`, `ServeDecl` and
  `ActivationDecl` gained fields; an exhaustive literal breaks, `..Default::default()` does
  not. A `ServeDecl`'s `Debug` redacts `api_key`.

### Security

- **A revoked key's certificate is refused at the mTLS handshake, and the mesh no longer resumes TLS sessions**
  (post-360 plan row Q's follow-up; upgrade note `docs/guide/deprecations.md` §44). Row Q (2.33.0) made a revoked
  key stop verifying at every signature check, but the handshake judged only the CA chain and the removal ledger,
  so whoever held a revoked key and its certificate — CA-valid until it expires — stayed a connected peer, its
  signed frames refused one at a time. Now both sides of a mesh handshake refuse a certificate whose key its owner revoked by a
  revocation this node **counted** — judged while the key was anchored for the owner (a CA-issued certificate record
  naming the node), and kept for good after (across a restart, through the journal under `[persistence]`, 64 per node), whatever the anchored set's cap or a saturated node later drops
  (`NodeStanding::counted`). A certificate naming its node (`mycelium://{node}`) is judged against that node; one naming
  no node (issued before row Q, valid to 2099) against whichever node counted a revocation of its key — a revocation
  counts only against a key anchored for the revoker, and a key is anchored only for the node its certificate names,
  so one member's revocations can never refuse another's handshakes. A *provisional* revocation (of
  a key whose certificate record has not reached this node yet) refuses nothing, so a node that has just rotated still
  connects; and the handshake checks *revoked*, not the wider *distrusted*, for the same reason. **Found on the way, pre-existing since closure plan C5 (2.15.0):** the
  mesh's rustls configs kept their defaults, a client session cache and a server issuing TLS 1.3 tickets, and a
  resumed session re-runs no certificate verifier — so a member that completed one handshake before its **removal**
  reconnected on the cached session and was admitted at the transport. Resumption is now off on both sides. Tests:
  `a_revoked_keys_certificate_is_refused_at_the_handshake_both_ways` (seen failing first without the check, with
  resumption on, and — its provisional plant — judging every revocation), `a_revoked_key_is_refused_behind_a_certificate_that_names_no_node`
  (seen failing first with a URI-less certificate unjudged; the review's finding 1), `a_removed_member_cannot_reconnect_on_a_resumed_session` (seen failing first with resumption
  on), `a_revoked_keys_certificate_is_refused_at_a_peers_handshake` (end to end after
  `rotate_identity_on_compromise`; seen failing first without the check),
  `a_key_revoked_long_ago_stays_refused_at_the_handshake_past_the_anchor_cap` (sixteen rotations later; seen failing
  first with the rule reading the anchored set — the re-review's finding). **Not closed:** a connection already up
  when the revocation is counted stays up (its signed frames are refused as before); a key that never had a certificate record naming its node
  (its signatures are refused, its handshake is not).
  `mycelium_core::tls::load_or_generate`, `generate_rotation` and `generate_rotation_after` take the key standing
  (`Arc<KeyStanding>`) after the removed set, and `NodeStanding` gained `counted` (an exhaustive struct literal
  breaks).
- **A gateway caller envelope is served once, at the provider it was signed for** (post-360 plan row E's nonce cache;
  upgrade note `docs/guide/deprecations.md` §45). Row E (2.33.0) bounded an envelope's age to 300 s plus the drift
  bound, but inside that window a captured envelope and its payload — re-emitted under a fresh RPC nonce by a member
  that relayed it — were served again as the client's call, and at any provider serving the same kind. Each envelope
  now carries an id (`n`, the gateway's HLC at framing) and its target (`o`), and a second Ed25519 signature covers them
  with the rest (`mycelium:gateway-caller:v2`); the v1 signature is still produced, so a 2.34 provider verifies a new
  gateway's envelope. A provider refuses an envelope addressed to another node, and admits each
  `(gateway, issue time, id)` once at the six places a request is received — `rpc_rx`, the MCP tool loop, the MCP
  bridge, the SDK serve stream, `llm.invoke` and the explain responder (`gateway_caller::verify_received`; `verify`
  keeps no state, so a handler re-reading its request is not a replay). An embedder reading raw signals itself calls
  the new `GossipAgent::receive_gateway_caller`, once per signal. Every `tls` node on this release re-asserts
  `sys/caller-context-id/{self}` every minute — its signed, dated statement that its envelopes carry ids — and while a
  provider holds one signed by a key it trusts for that node and issued within the freshness window plus two refresh
  periods (12 minutes at the default drift bound), an id-less signed envelope in that node's name is a replay with
  the id stripped. A member cannot plant the marker for another node, and it is never tombstoned: a gateway rolled back
  to 2.34 stops re-asserting it and its id-less envelopes are served again once it lapses. A replay in any of these shapes is **dropped unanswered** — a refusal sent back under the original's
  correlation nonce could reach the gateway before the real answer (`CallerError::unanswered`) — and counted
  (`GET /stats` `caller_envelopes_replayed`). The window is per gateway, signed envelopes only, pruned at the freshness
  bound (600 s at the default drift bound) and capped at 65 536 ids: past the cap (about 109 calls a second, sustained,
  from one gateway to one provider) the oldest is evicted and becomes a floor at or below which an envelope is refused,
  so the bound fails closed; windows left empty are swept. **Also fixed:** the MCP bridge (`connect_mcp_server`)
  forwarded a call whose caller context failed verification unless provider enforcement was on; it now refuses it
  (`-32022`) as the native tool loop does. **The window, one release:** an id-less envelope from a gateway without the
  marker (2.34 or earlier) verifies in the v1 form, is served and, if signed, counted (`caller_envelopes_without_id`); once a
  gateway is upgraded and its marker has reached a provider, stripping is refused there. The next release refuses every
  envelope without an id. **Not covered:** an unsigned mesh (no `tls` identity) keeps no window; a provider restart
  forgets its window; a member that tombstones a gateway's id marker at a provider reopens the id-less path for that
  gateway there until the next re-assertion, at most a minute (the `sys/` tripwire counts the foreign write); a gateway
  rolled back to 2.34 has its calls dropped unanswered until its marker lapses, up to 12 minutes; after an eviction
  floor, a gateway whose clock moves back across a restart has its calls dropped until its clock passes the floor; a
  provider whose own clock steps back may serve an envelope it had already pruned, once. Tests, each seen failing first:
  `a_replayed_envelope_is_refused_at_the_receive_boundary` (with a plant: two calls identical in content and issue time
  are both served), `an_envelope_id_and_target_are_signed_and_stripping_them_is_refused`,
  `a_captured_envelope_is_refused_at_another_provider_and_stripped`, `only_a_gateways_own_fresh_id_marker_is_honoured`,
  `rpc_rx_drops_a_replay_unanswered`,
  `the_mcp_tool_loop_drops_a_replay_unanswered`, `the_mcp_bridge_refuses_a_replayed_or_failed_context`,
  `every_receive_boundary_drops_a_replay_unanswered` (the serve stream, `llm.invoke`, the explain responder, and the two
  counters on `GET /stats`); and `the_replay_window_is_bounded_and_fails_closed`, `an_empty_replay_window_is_swept`.

- **Credentials attached at egress: a secret need not enter a component's environment** (post-360 plan row L). A
  node declares **credential bindings** — `[[egress_credentials]]` (or `GOSSIP_EGRESS_CREDENTIALS`), each a host, an
  optional path prefix, a form (`bearer`, `header` with its name, `basic` with a username) and where the value comes from
  (`value_env` or `value_file`, read once at `start()`) — and a caller holds only an opaque placeholder,
  `mycelium-credential:<name>` (`mycelium::credential_placeholder`), where the secret would go. The egress gate
  (`mycelium::EgressCredentials`, `GossipAgent::egress_credentials()`) replaces the header that carries it with the
  value **only** on a request to the bound host (exactly, under the prefix, over https or to loopback unless
  `allow_plaintext`), refuses the request before a byte is sent anywhere else, follows redirects itself through the
  allow-list's per-hop verdict and **drops** the credential on a hop that leaves the binding, and counts each binding's
  attachments, refusals and drops (`uses()`; `mycelium_egress_credential_total{binding, event}` under `metrics`). The
  value is held as a sensitive header value inside the gate: no `Debug`, `Display`, error, log line or trace field
  carries it, and a refusal names a binding by index and name, never a field's contents (so a value pasted into the
  wrong field is not logged). A binding may name a `port`, and one to `localhost` (any spelling) or an IP literal must; only 301, 302, 303, 307 and 308
  are followed; two bindings that would set one header are refused. Moved behind a binding: `[[serve]]`'s new `credential = "<binding>"` (a stem refuses to start if the node
  lacks it) and `OpenAiBackend::with_credentials` (a placeholder `api_key` without a gate is refused, never sent).
  A `[[serve]]` whose endpoint its binding does not authorise refuses the stem's start. A placeholder request goes out
  under the node's egress policy as the gate captured it at `start()`, not a backend's own `with_egress`. Guarantee
  `egress.credentials`. **Not built:** the row's activation-command half — an `[[activation]]` command is a separate
  process with no path to the gate and inherits the stem's environment. Every other place a secret enters a node is listed in
  `docs/operations/crown-jewel.md` § Credentials attached at egress and stated unchanged, with the reason. Found on the
  way: a `{:?}` of a unit file printed a `[[serve]]`'s literal `api_key` (the derived `Debug`) — now redacted;
  `OpenAiBackend` gained a `Debug` that redacts a literal key. Tests: `egress_credentials_tests.rs` (seven seen failing
  first with the gate's attachment stubbed out — the placeholder went out as written, to every host, through every
  redirect), `a_serve_names_one_key_form_and_its_debug_prints_no_literal_key` (seen failing first with the derived
  `Debug`), `egress_credentials_come_from_the_environment` (seen failing first without the override),
  `a_serve_naming_a_binding_the_node_lacks_refuses_the_start` (seen failing first without the check),
  `a_served_skill_sends_its_binding_value_which_the_stem_never_held` (seen failing first without the gate handed to the
  backend), `a_credential_binding_refuses_the_start_and_reads_not_in_build` (a gateway-free build; seen failing first
  without the refusal), and the core binding tests; the review's findings each with a test seen failing first
  (`a_300_is_not_followed`, `two_bindings_that_would_set_one_header_are_refused`,
  `a_redirect_to_another_port_on_the_bound_host_drops_the_credential`, `a_secret_in_a_source_field_is_not_echoed_by_a_refusal`,
  `a_refusal_never_echoes_a_field_that_could_hold_the_value`, `a_serve_whose_endpoint_its_binding_does_not_authorise_refuses_the_start`).
  Additive: no binding configured, nothing changes. **Upgrade notes** (`docs/guide/deprecations.md` §48): `GossipConfig`
  gained `egress_credentials`, `OpenAiBackend` gained `credentials` and `ServeDecl` gained `credential` (an exhaustive
  struct literal breaks; `..Default::default()` is unaffected); a `ServeDecl`'s `Debug` redacts `api_key`; zero-gaps
  Z2's refusal for an unset `api_key_env` variable names the `[[serve]]` section and the field, no longer the variable.
  **Not covered:** a placeholder in a query string or a body; a proxy the gate's client honours (`HTTP(S)_PROXY`) sees
  a plaintext or loopback request's header; zeroising the value; a `value_env` variable stays in the process environment, which an
  `[[activation]]` command inherits (`value_file` keeps it out of every environment; confining the command is row M).

### Added

- **A Strands fleet, demonstrated** (post-360 plan row P; `examples/strands_fleet/`, CI job `strands-fleet`). Strands
  agents on three nodes plan tonight's food redistribution: a `Swarm` (collector → planner) asks the bakery's and the
  kitchen's skills through one gateway's `/a2a`; the planner's attempt to dispatch a van, a mandated role it was not
  appointed to, is refused (`-32031`) and read back from the hub's evidence journal, as is the planner presenting the
  courier's grant; the courier dispatches with its own grant and possession proof; the hub is stopped and the courier
  resumes on the depot from its stored session and dispatches the second van. The node binary is
  `examples/strands_fleet_node.rs` (an operator and three nodes, `--features tls,a2a,compliance`). The README says
  what Strands provides, what Mycelium adds, and what the run does not show (the models are scripted).
- **An evaluator can decide on what an actor has already done** (post-360 plan row K; upgrade note
  `docs/guide/deprecations.md` §47). `ActionEvaluator` gains `history_request` and `evaluate_with_history` (defaults keep
  every evaluator as it was), and a gateway with an evidence journal gives an evaluator that asks the actor's recent
  decisions from that journal — **counted at this enforcement point** (`HistoryScope::EnforcementPoint`, decided in the
  plan's §5: node-local, exact here, no quorum; an actor spreading calls over N gateways can be admitted up to N times a
  limit, and the scope is named on every history decision). The reference evaluator's `Rule::at_most(3, 600_000)` is
  *at most three posts in ten minutes*: the fourth is denied. A history that cannot be read is `Unavailable`, and a
  limited rule decides `Indeterminate` on it, never as an empty history; a count below the limit on an incomplete view
  is not established. One actor's history-limited decisions are made one after another and a permit is counted as it
  is decided, so ten concurrent calls under a limit of three admit three. The history is read from the journal once an
  evaluator that asks for it is attached (a restart keeps it), bounded (24 hours, 1 024 records per actor), and each decision's evidence carries the
  history it read, by attempt id. The AE contract suite gains the clause `AllowAtMost` and five cases, passed by the
  reference evaluator and by the suite's independent replacement.
- **A Strands Agents session store on the fleet: `strands-session-mycelium` 0.1.0** (post-360 plan row O). A new Python
  companion beside `langgraph-checkpoint-mycelium`. `MyceliumSessionManager` is a Strands `SessionManager` over the
  Python SDK's KV verbs, so an agent's conversation and state, and a `Swarm`'s or `Graph`'s shared state, are rows in
  the fleet's gossiped KV, and another node resumes a session once its rows have arrived. Every write returns the
  SDK's `KvReceipt` (`last_receipt`, `on_receipt`): local application on the gateway node and that node's disk state,
  **not replication**; `require_on_disk=True` raises on any write the node did not report `on_disk`. A session read
  while some of its rows are still gossiping raises `IncompleteSession` naming the missing keys rather than returning
  the rows that are there (a per-agent count row covers the tail); an undecodable or malformed row or a gateway error
  raises `SessionException`. A failed message write is held and written, in order, before any later one, so it leaves
  no hole. A row is at most about 1.5 MiB (the gateway's request limit), checked before sending. **Nothing is written
  over what a node has not seen:** every message index, and every version of an agent's, swarm's or graph's record,
  is claimed once, fleet-wide, by a consensus write before it is written (`claims=True`, the default), so a node that
  is behind, or a second writer of one agent or swarm, is refused (`IncompleteSession`, or `StaleWriter`, which
  fences that writer) rather than overwriting. A claim names its write (the row goes under `pending/` first), so a
  writer that claims and dies blocks no one: the next reader takes the row from it. A session's claim fixes its
  generation; a deleted session's id is refused, not reused; a sliding window of 0 resumes without writing over its
  history. Claims cost a consensus round per message and per update, and a quorum for every write; voters should
  persist; `claims=False` is last-writer-wins, with its limits stated. Pinned to `strands-agents` 1.61.1 with the
  interface recorded in a test. Its live suite runs in CI on three voting `mycelium` nodes
  (`MYCELIUM_STRANDS_LIVE_REQUIRED`).
- **An unmodified A2A client reaches a node's skills through `/a2a`** (post-360 plan row N; upgrade note
  `docs/guide/deprecations.md` §46). The 2026-10-12 probe found the current `a2a-sdk` (1.2.2, the library Strands
  Agents uses) could not: the card was served only at `/.well-known/agent.json`, the methods were the pre-0.2
  `tasks/*` set, and the skill was chosen by a Mycelium-only `skillId` that a standard client never sends (the card's
  gossip-port `url` was fixed in 2.34.0, #621). Now the card is also at `/.well-known/agent-card.json` and names
  `/a2a` once per protocol version served (`supportedInterfaces`: JSON-RPC 1.0 and 0.3), with how a message reaches a
  skill in its `description`; `/a2a` serves A2A 1.0's `SendMessage`, `SendStreamingMessage`, `GetTask` and `CancelTask`
  and 0.3's `message/send` and `message/stream` beside the old methods, each answering in its generation's shapes
  (`tasks/get` and `tasks/cancel`, which pre-0.2 and 0.3 share by name, answer a task in the generation that created
  it). A 0.3 / 1.0 message names its skill in `metadata.skillId` — the message's or the request's — or reaches the
  default skill an embedder declares (`GossipAgent::with_a2a_default_skill`), or is refused `-32602`; a presented
  mandate and an operation id ride in the request's `metadata._meta`. Every send, whatever its generation, runs the one
  path `tasks/send` ran — the federation edge (unary sends only; a federated stream, get or cancel is refused as
  before), the task reservation and its owner rule, the action evaluator, the mandate to the provider, the evidence
  record — and the C7 bypass matrix now tries all six send methods, plant and revocation. Each message is its own task:
  one naming a `taskId` is refused `-32004`. Tests: `the_current_a2a_methods_reach_the_skill_and_answer_in_their_generations_shapes`,
  `a_current_send_names_its_skill_in_metadata_or_reaches_the_default`,
  `the_card_names_a2a_once_per_protocol_version_and_how_a_message_reaches_a_skill`, the federation and C7 tests
  extended — the HTTP, federation and C7 ones seen failing first with the new names unrouted (`-32601`) and the card
  path unmounted; and the probe as a CI job, **`a2a-sdk`**: the real `a2a-sdk` 1.2.2 client — its card resolver, client
  factory, 1.0 transport unary and streamed, and its 0.3 compatibility transport — against
  `examples/a2a_sdk_node.rs` (`tests/a2a-sdk/`). **Not built:** the SDKs' own `A2aClient`s (`mycelium-py`,
  `mycelium-ts`) still send `tasks/send` and read `/.well-known/agent.json`, and move before those are removed; push
  notifications, `ListTasks`, `SubscribeToTask` and the HTTP+JSON and gRPC bindings are not served.

- **An activation command can run confined** (post-360 plan row M). An `[[activation]]` ran its `command` and
  `probe` with the stem's privileges — "not a sandbox: there is no allowlist and no confinement". A unit may now name
  an `[activation.confine]` profile: beyond a fixed system read set, the command may read the placement directory and
  `read`, read and write `write`, and open TCP connections only to `connect` (`localhost:<port>` or `*:<port>`; empty:
  no network). On macOS the stem applies it with Seatbelt (`sandbox-exec`, a generated profile, paths passed as
  parameters); on Linux with Landlock (filesystem at ABI 3, TCP ports at ABI 4) and a seccomp filter (no socket but an
  IP stream over TCP, and none at all without `connect`; no `io_uring`), applied to a dedicated thread that spawns
  the command. **No silent fallback:** a profile this node cannot apply — the backend absent, a kernel without
  Landlock or below the ABI it needs, `localhost:<port>` under Landlock (which limits ports, not addresses), a declared
  path that does not exist — refuses the activation by name and runs nothing (`confinement_unavailable`,
  `confinement_failed` in the decision trace; `prov.activation` revision 2). The probe and every re-probe run under
  the same profile, resolved once per activation. A profile naming a hostname or an address, a relative path, a
  placeholder other than `{dir}`/`{path}`, a `..` segment or an unknown key is refused at load. **The default stays
  unconfined**, and the stem says which applies — per activation at start (`Stem::activation_confinement`) and node-wide
  in the guarantee report: `mycelium-stem` registers `stem.activation_confined` (enforced · not configured, naming
  *unconfined* or *refused* · not applicable). Tests: `an_activation_confinement_profile_parses_and_a_bad_one_is_refused_by_name`
  (seen failing to compile), `a_profile_the_platform_cannot_apply_refuses_the_activation_by_name` (forces the backend
  this platform lacks, so it runs on both; seen failing first — the profile was ignored and the command ran),
  `a_confined_activation_reads_and_writes_only_what_its_profile_names` and
  `a_confined_activation_reaches_only_its_declared_tcp_ports` (seen failing first, unconfined; run on macOS with
  Seatbelt and on Linux with Landlock — CI's wasm-host job sets `MYCELIUM_CONFINE_LIVE_REQUIRED`, so a runner that
  cannot confine fails them rather than skipping green), `a_confined_activation_runs_in_a_stem_and_the_stem_says_which_applies`
  and `the_guarantee_report_says_confined_unconfined_or_refused` (planted). **Not built:** a network allow-list that
  changes live (every limit holds until the stem restarts), limits by hostname or address (neither backend has
  them; an egress proxy would), an OpenShell (container) backend, resource limits; on Linux below Landlock ABI 6,
  signals to the stem user's processes are not limited. From the PR's independent review: on Linux no TCP Fast
  Open (`MSG_FASTOPEN` sends, the `TCP_FASTOPEN*` options), no `listen`, and on x86_64 no x32 syscall; the
  always-readable set is exactly the executables, libraries and the few `/etc` files a loader, resolver and TLS read
  (not `/etc`, `/private/etc` or `/usr/local` whole, so not the node's key); an `[[activation]]` refuses unknown keys,
  and so does the unit file's top level, so neither `[activation.confined]` nor `[activations.confine]` can load as no
  profile; a profile is resolved once and kept (on Linux held open), re-resolved when a kept path no longer names
  the same file, and a declared path through a symlink below the placement root or a write path — compared
  canonically — is refused; a macOS CI job
  runs the Seatbelt tests. `landlock` 0.4.7 and `seccompiler` 0.5 are new, Linux-only
  dependencies of `mycelium-wasm-host`. **Upgrade notes:** `ActivationDecl` gained `confine` (an exhaustive struct
  literal breaks); a unit file with an unknown key — at the top level, in an `[[activation]]` or in its
  `[activation.confine]` — is now refused at load (`docs/guide/deprecations.md` §49; every unit in the repository
  loads unchanged).

### Fixed

- **A fleet of `mycelium` binaries could not decide anything by consensus.** The binary never started a consensus
  listener, so its nodes never voted: every consistent write, lock and election its own gateway serves
  (`/gateway/overlay/{consistent/set,lock/acquire,elect}`) timed out on a mesh of two or more — only an embedder that
  called `start_consensus_listener` itself had a quorum. The binary now starts one after `start()` (built with
  `consensus`, a default feature). Found while building row O's claims: a probe on three binaries answered
  `consensus timed out after 3 ballot(s)` for every write, and commits in milliseconds with the listener; the Strands
  session store's live suite runs on such a fleet in CI.
- **A timed-out activation was recorded as `command_failed`.** The hook matched `"timed out"` against the runner's
  error, which says `did not finish within`, so the rule catalogue's `command_timed_out` was never recorded. Found
  while enumerating row M's doors; `a_hung_activation_is_recorded_as_a_timeout`, seen failing first.

### Documentation
- **Every concept on the newcomer path is defined where it is met.** `docs/guide/00-concepts.md` gains § Fleet growth
  (stem, catalogue · library · librarian, unit file, presence band, provisioner vs governor, self-election, runtime
  kind, activation and its confinement, `[[serve]]`, shadow → accept, `wire-check`), the enforcement point and
  evidence journal, history scope, credential binding, and disambiguation rows for *claim*, *scope*, *journal*,
  *catalogue* and *admission*; tutorials 1–5 link the terms they use. `docs/positioning.md`'s stem definition is
  taken from `stem.rs` (a node that declares from its unit file; only one with `[hosts]` installs), and an evaluator
  without a journal is stated to still enforce.

## [2.34.0] — 2026-10-16

**Security fixes in the dependencies, and the post-360 plan's last identity rows.** Wasmtime 49.0.3 closes
thirteen advisories against the WASM host, where third-party components run, and rustls 0.23.46 closes four;
see the entries under Security. To check a build, run `cargo tree -i wasmtime` and `cargo tree -i rustls`: they
should show 49.0.3 and 0.23.46.

Two rows complete the identity hardening. **A TLS node now refuses an unsigned KV frame and a bare KV signature
unconditionally** (row F2): row F's one-release window closes and `kv_require_signed_frames` is retired to a no-op.
**Identity proofs are required by default** (row S): a peer's keys are merged only from its sealed record, so a
member can no longer write its own key into another node's identity. A node that holds the fleet CA key says so
loudly at start (row T).

Shutdown now withdraws a node's soft state at its peers. Persistent service adverts, MCP tools and group
projections are swept with the rest, and a gate stops any re-assert from overtaking the sweep. A capability name
that cannot be a key segment is refused at every door. CI audits every tracked lockfile.

Wire **v12** unchanged (`WIRE_VERSION` 12, `PREV` 11). Companions unchanged: `mycelium-reason` 0.9.0,
`mycelium-py` 0.2.11, `mycelium-ts` 0.2.5, `langgraph-checkpoint-mycelium` and `mycelium-guardrails`.

**Check before upgrading** (`docs/guide/deprecations.md` §40–§43):
- **Upgrade from 2.33.x, not 2.32.** A 2.32 node's KV writes are refused at a 2.34 node and reach it only by
  anti-entropy. Before upgrading, confirm `kv_unsigned_frames_accepted` and `kv_untagged_signatures` (`GET /stats`)
  have stopped rising on every 2.33 node.
- **Remove `kv_require_signed_frames`.** It does nothing now, and setting it is warned at `start()`.
- **Identity proofs are on by default.** A 2.3.0–2.31.x peer is already unlearnable from gossip since 2.33.0. Only a
  node from before 2.3.0 is newly cut off, and `require_identity_proofs = false` re-admits it.
  `GOSSIP_REQUIRE_IDENTITY_PROOFS` is now a strict boolean, so a typo refuses the configuration.
- **A capability name is one key segment.** An `ns` or `name` that is empty, holds a `/`, or is `locality`/`self`
  answers 400 at `capability/advertise` and refuses the unit file.
- **`CoreCtx` gained `soft_state_gate`.** This breaks only an exhaustive struct literal.

### Security

- **Dependency: `wasmtime` / `wasmtime-wasi` 49.0.2 → 49.0.3** (RUSTSEC-2026-0352..0364, published 2026-10-14 —
  thirteen advisories). Nine are host panics or a host stack overflow a guest component can trigger (a start
  function that yields, pending calls after a trap, an async-lifted function via `call`, a cancelled `call_async`,
  a stream or future viewed as the wrong type, an erroneous realloc call-to-host, zero-length out-of-bounds stream
  writes, overlapping string transcode, a long chain of recursive types) and one an unenforced async-entrypoint
  requirement; RUSTSEC-2026-0363 caps component handle tables per instance only, charged to no limit; -0364 can
  misalign host data structures. The one rated high, **-0352** (a filesystem sandbox escape renaming symlinks with
  trailing slashes), needs a preopened directory, and the WASM host preopens none (`mycelium-wasm-host/src/host.rs`,
  `WasiCtxBuilder::new().build()`), so it is not reachable through it. The WASM host is where third-party components
  run. A lockfile change only, no API change.
- **Dependency: `rustls` 0.23.45 → 0.23.46** (RUSTSEC-2026-0346, -0347, -0348, -0349, published 2026-10-14): `Debug`
  impls for client session values exposed TLS secrets; Brotli certificate decompression could allocate up to 1 GiB
  before authentication; an unauthenticated ServerHello could downgrade later connections to classical key
  exchange; buffered application data could reach the outer server after an ECH rejection. Reached through the
  `tls` transport, the gateway's TLS, the pinned federation client and the object-store clients (the
  other `reqwest` clients use the platform's native TLS). A lockfile change only, no API change.
- **A TLS node refuses an unsigned KV frame and a bare KV signature unconditionally** (post-360 plan row F2; upgrade
  note `docs/guide/deprecations.md` §40). Row F (2.33.0) made the refusal opt-in for one release; now, whatever
  `kv_require_signed_frames` says, a TLS node drops an unsigned `Data` frame (`GET /stats` `kv_unsigned_frames_refused`)
  and a `SignedData` frame signed in the bare pre-2.33 form (new: `kv_untagged_signatures_refused`,
  `mycelium_kv_untagged_refused_total`, and a rate-limited warning naming the peer). The two self-authenticating
  records — a sealed identity record and a CA-verified certificate record — stay exempt. `kv_require_signed_frames` is a
  **retired no-op** (`#[deprecated(since = "2.34.0")]`, still parsed, warned at `start()` when `true`; an explicit
  `false` is not an opt-out). Row F's window counters `kv_unsigned_frames_accepted` and `kv_untagged_signatures` stay on
  `GET /stats` at zero and their metrics are no longer emitted. `mesh.signed_writes` is rev 2 and resolves `enforced`
  on `[tls]` alone; `secure-single-domain` is **rev 5** and requires it (met wherever `mesh.tls` is, so it admits every
  node rev 4 admitted). Wire v12 unchanged. **Upgrade note:** upgrade from 2.33.x, not from 2.32 — a 2.32 node's KV
  writes are refused at gossip by an upgraded node and reach it only by anti-entropy. **Not closed:** an anti-entropy
  `StateResponse` entry carries no author and no signature and is still applied by LWW (a wire change; restated in the
  threat model and `what-is-proven.md`). The node's locality advertisement now follows the TLS install, so it leaves
  signed (#624's review; no deterministic test — the start-time frame precedes every connection). Seen failing first
  on 4251d50a:
  `an_unsigned_data_frame_is_refused_whatever_the_setting_says`, `a_bare_2_32_kv_signature_is_refused`,
  `only_a_self_authenticating_sealed_identity_is_exempt_from_the_refusal` (at the default),
  `mesh_signed_writes_is_enforced_under_tls_by_default`, `kv_require_signed_frames_is_inert_wherever_it_is_set`.

### Changed

- **CI audits every tracked lockfile** (#626). The `Dependency audit (RUSTSEC)` job ran `cargo audit` on the root
  `Cargo.lock` only, so `examples/conway-gpu/Cargo.lock` held five advisories no job saw (#625's review). It now
  audits each lockfile `git ls-files` finds, NUL-separated, names the one that failed, and fails closed when the
  root lockfile is not among them.
- **Identity proofs are required by default** (post-360 plan row S; `docs/guide/deprecations.md` §41).
  `require_identity_proofs` defaults to `true`: a TLS node merges a peer's keys only from its sealed record
  (`sys/identity-signed/`, written by every TLS node since 2.14.0), so a bare `sys/identity/{n}` key list without a
  proof is refused and counted (`identity_anchor_conflicts`). This closes the identity-poisoning residual rows F and
  Q stated: a member garbling another node's sealed record and proof and writing a bare key list naming its own key —
  a valid frame under its own signature — was merged at every receiver, reaching the verify-only history set (forged
  audit history) and, for a node on a legacy certificate, its live trusted set (full impersonation). **The 2.14.0
  attempt, re-examined:** it was reverted after the Docker suite's `S12 leader election … Nodes disagree on leader`,
  which it could not have caused — the flag is inert without TLS and those nodes run none; S12 was an election with
  a quorum of one, closed by the electorate refusal (#377). The ordering window the revert named (identity and proof
  as two gossip writes) was real and is closed by the sealed record. Two defects the flip exposed: the field's
  field-level `#[serde(default)]` resolved a TOML file that never named it to `bool`'s `false`, whatever
  `GossipConfig::default()` said (it now has its own serde default), and `GOSSIP_REQUIRE_IDENTITY_PROOFS` read any
  value but `1`/`true`/`TRUE`/`yes` as `false`, so a typo turned the check off (now a strict boolean that refuses
  anything else by name). The guarantee `id.proofs_required` resolves `enforced` on a TLS node that says nothing
  (catalogue regenerated). **Rotation no longer launders a planted key** (the PR's review): `rotate_identity` built the
  new history from the raw `sys/identity/{self}` value — any member can write it — and signed it with the trusted
  prior key into the sealed record, so with proofs on a key a member planted there still entered the node's history
  at every peer; it now builds it from what the node can show it held (`revocation::own_history`, as the start path
  does) — `rotation_does_not_launder_a_poisoned_bare_identity_into_the_history`, seen failing on the PR's first head.
  A rotation also re-signs the sealed record with the new key after the cut-over, so a legacy-certificate node that
  rotates twice keeps its original key in its published history
  (`a_double_rotation_keeps_the_original_key_in_the_published_history`). **Upgrade note:** row S cuts off nobody 2.33.0 did not already cut off except a node from
  before 2.3.0, which publishes no proof at all: since 2.33.0 only a tagged proof is accepted in either posture, so a
  2.3.0–2.31.x peer was already not learned from gossip, with or without this flag. `require_identity_proofs = false`
  (or `GOSSIP_REQUIRE_IDENTITY_PROOFS=0`) re-admits only the pre-2.3.0 node, and is inert on a TLS-less fleet. The
  environment variable now refuses `True`/`False`, `on`/`off` and an empty value by name. **Not closed:** first sighting of a node never
  seen is trust on first use; anti-entropy entries carry no author. Tests (each seen failing first on 4251d50a):
  `the_default_requires_identity_proofs`, `a_config_file_that_never_names_identity_proofs_requires_them`,
  `the_identity_proofs_setting_parses_strictly_from_the_environment`,
  `a_default_configuration_rejects_an_unsigned_identity_entry`,
  `by_default_a_poisoned_identity_key_does_not_reach_the_history_set`,
  `by_default_a_poisoned_identity_key_does_not_sign_for_a_legacy_certificate_node`,
  `id_proofs_required_resolves_enforced_by_default`; and `an_explicit_false_still_opts_out_of_identity_proofs` (a
  guard that passes on both).

- **A node holding the fleet CA key says so loudly at start** (post-360 plan row T). In the development posture
  (`TlsConfig::default()`, `ca-key.pem` in `auto_cert_dir`) one compromised host can mint a certificate for every node
  and key revocation cannot recover a stolen identity; until now that was one unmet `id.ca_key_off_node` line in the
  start-time report, worded for member removal alone. Now `start()` logs a warning naming the consequence and the
  remedy (`mycelium tls issue` on a separate CA host, then `[tls] cert_pem` + `key_pem`), unless the node runs
  `profile = "dev"`; the guarantee's promise names identity compromise and revocation (catalogue regenerated); the
  security guide and production-readiness page say which posture each snippet sets up. No behaviour change beyond the
  log line. Test: `a_node_holding_the_ca_key_is_warned_unless_it_chose_dev`.

### Fixed

- **A capability name that cannot be a key segment is refused at every door** (#613's review, finding 6, recorded in
  post-360 plan §3.6 under E; upgrade note `docs/guide/deprecations.md` §43). A capability's `ns` and `name` are two
  segments of `cap/{node}/{ns}/{name}` (and of `req/…`, `gcap/{group}/{ns}/{name}/{node}`): an empty one or one holding
  a `/` built a key no reader parses, and `ns = "locality", name = "self"` overwrote the node's own locality entry with a
  `Capability`. `POST /gateway/capability/advertise` answered 200 to each. Now `capability::capability_name_problem`
  (and `capability_group_name_problem`) decide it once, and every door refuses: the route answers 400
  `invalid_capability_name`; the unit-file loader refuses the file (so `units/declare` answers 400 and a node config
  fails to load), a group name or a group's `provides` included, and a `[[serve]]` named `locality`/`self`; in process
  `advertise_capability`, `declare_requirement` and `define_capability_group` warn and return a handle that writes
  nothing, and `register_prompt_skill` returns `InvalidName` for the locality pair; and the emergent-group projector
  skips a `cap-group/` definition (a 2.33 peer's, or one persisted before the upgrade) whose names fail the check,
  rather than projecting `gcap/`/`grp/` keys no reader parses. Tests:
  `a_capability_name_that_is_not_a_key_segment_is_refused_at_every_door` (failing first at the route: 200),
  `the_group_projector_skips_a_definition_whose_names_are_not_key_segments`, `a_prompt_skill_named_like_the_locality_entry_is_refused`,
  and the `[[serve]]` load test — each seen failing first.
- **A fast soft-state re-assert no longer undoes the shutdown sweep** (#627's review, finding 1; upgrade note
  `docs/guide/deprecations.md` §42). The soft-state tasks ran until the shutdown signal, which comes after the sweep
  and the outbound flush, so a re-assert tick in between wrote a swept key live again under a newer HLC and the
  flush delivered that: the peer kept routing to the stopped node (20/20 shutdowns at a 5 ms refresh in the review's
  probe; 0/10 at 50 ms and above). Shutdown now closes `CoreCtx::soft_state_gate` before it sweeps: every re-assert
  of a swept family passes through it (`run_kv_persist_task`, the emergent-group projection and requirement
  opacity, `declare_requirement`'s opacity, the load-opacity task), and a write already inside finishes first.
  Lock-free (`SeqCst` flag + in-flight count); only the apply and the send run inside it — a caller's payload closure
  runs before — and the in-flight count is restored on unwind, so a panicking payload cannot wedge `shutdown`. Tests:
  `a_fast_reassert_does_not_undo_the_shutdown_sweep` (five shutdowns at a 5 ms refresh; with `close()` toggled off
  every round kept both the `svc/` and the `cap/` key at the peer), `a_panicking_soft_state_payload_does_not_wedge_shutdown`
  (hung past 60 s before), `write_opacity_key_writes_nothing_once_the_gate_is_closed`, `kv_persist::gate_tests`.
  `CoreCtx` gained a field.
- **Soft state with a live handle is retracted at the peers when its node shuts down** (post-360 plan §3.6, a follow-up
  recorded under row F; pre-existing). Graceful shutdown tombstoned the node's own `cap/` and `req/` entries before
  the outbound flush, but three other families a node keeps alive under its own id were tombstoned only by their own
  tasks when the shutdown signal woke them — after the flush, with the writers stopping — so the tombstone never
  left and peers kept routing to the stopped node until the entry's TTL: a persistent service advert
  (`svc/{kind}/{self}`, `mesh().advertise_persistent`), an MCP tool (`tools/{name}/{self}`, including bridged
  servers' tools) and an emergent group's projection (`gcap/{group}/{ns}/{name}/{self}` — not in the recorded
  follow-up; found by enumerating every soft-state task). `shutdown_with_timeout` now sweeps all three. Test:
  `soft_state_with_a_live_handle_at_shutdown_is_retracted_at_the_peers`, seen failing first on all three families.

- **The A2A agent card names the gateway's `/a2a` endpoint, not the gossip port** (plan row N, its standalone defect). `/.well-known/agent.json`'s `url` was built from the node's gossip address with no path, so an A2A client that follows the card — the current `a2a-sdk`, which Strands Agents uses — posted to the gossip listener and failed with a network error (the 2026-10-12 probe, `docs/plans/external/2026-10-12-a2a-sdk-probe/`). It is now the gateway's scheme (`https` under `gateway_tls`), host (`http_addr`, or the node's own address when the gateway binds every interface), port and `/a2a`. Test: `the_agent_card_names_the_gateways_a2a_endpoint_not_the_gossip_port`. The rest of row N (the current card path and A2A 1.0 methods) is still open.

## [2.33.0] — 2026-10-13

**The post-360 hardening plan's second wave: revocation at every verifier, wire and crypto with a window, and an
electorate that can change.** **A revoked key stops verifying at every verifier** — the KV `SignedData` arm, identity
chaining, the unsigned identity exemption, consensus (its raw-`sys/identity/` fallback gone), caller envelopes,
knowledge keys and `mycelium-agentfacts` — and **only a newer certificate of the node can revoke its keys**:
certificates name their node (`mycelium://<node>`) and their issuance, are published at `sys/identity-cert/`, and a
revocation counts only when signed by an anchored key issued after the one it revokes; counted revocations are
journaled, so one hidden while a node was down still counts (row Q, `docs/design/revocation-authority.md` rev 3).
**Wire and crypto, one MINOR with a window** (row F): KV `SignedData` is signed under its own domain
(`mycelium.kv/update/1`), the bare 2.32 form accepted and counted for one release; under `[tls]` every local KV write
is signed and `kv_require_signed_frames` lets a node refuse an unsigned `Data` frame (off by default this release —
the unconditional refusal is row F2); a signed frame's signer is bound to its sender; a peer certificate's key is read
from its `SubjectPublicKeyInfo`, not by scanning the DER; and 2.32.0's §23 window closes — a bare identity proof or
consensus signature is refused. **An electorate's members change by a migration that terminates** — declare the
successor, retire the group into it, complete the carry — so a retired group can be decommissioned (P2's follow-up),
and an electorate member that restarts without its promises is detected: `[persistence]` is required, an empty store
is warned, a forgotten promise withholds its slot. The gateway remainder (row E): timer periods bounded to
`1..=3600` s, the mailbox and log-group streams record their position only after handing an entry on, prompt keys
cannot alias, a caller envelope is fresh for 300 s plus the drift bound. The companions' doors (row H): a commitment
award is decided on the declarer's signed close, `award_of_verified` checks the declarer the caller pins, and
`mycelium-reason` **0.9.0** bounds its blob store and validates a run id. `src/agent/http.rs` is split by route family
with behaviour unchanged (P5); hygiene (J) fixes the sim-seam gate's test skip, lints every feature and bench, and
deprecates nineteen unreferenced items without removing them; the philosophy states "a protocol, not a service" in
full (Φ). `quinn-proto` 0.11.18 (RUSTSEC-2026-0338..0342). Companions on their own lines: `mycelium-reason` **0.9.0**,
`mycelium-py` **0.2.11**, `mycelium-ts` **0.2.5**; `langgraph-checkpoint-mycelium` and `mycelium-guardrails`
unchanged. Wire **v12** unchanged (`WIRE_VERSION` 12, `PREV` 11): no frame changes shape; what a node accepts and
what a KV signature is over do, and four consensus variants are appended for the migration. **Check before
upgrading:** upgrade from 2.32, not 2.31 — a 2.31 node and an upgraded one verify neither each other's identity
proofs nor their consensus messages; an upgraded node's KV writes reach a 2.32 peer only by anti-entropy until that
peer is upgraded, so upgrade promptly; set `kv_require_signed_frames` only once `kv_unsigned_frames_accepted` and
`kv_untagged_signatures` (`GET /stats`) stop rising fleet-wide; **re-issue every node's certificate** so it carries
`URI:mycelium://<node>` (`openssl x509 -noout -ext subjectAltName -in <cert>`) — until then revocations of its keys
do not count at upgraded nodes; an electorate member without `[persistence]` is refused; a timer period outside
`1..=3600` answers 400 `timer_out_of_range`; remove alerts on `identity_untagged_proofs` and
`consensus_untagged_signatures`, which are gone. **Not claimed:** protection against a holder of the fleet CA key
(every node in the development posture holds it); identity proofs on by default (plan row S); the unconditional
unsigned-frame refusal (F2); authored anti-entropy entries; the mTLS handshake refusing a revoked key. Upgrade notes
in `docs/guide/deprecations.md` §34–§39.

### Added

- **Companions: the commitment deadline decided by the declarer's close, an award checked before it is reported,
  offers read only for their own requirement, a bounded blob store, a validated run id** (post-360 hardening row H,
  #609). `mycelium-commitment`: an offer's deadline was judged on `offered_at_ms`, a field the **offerer** writes —
  so the offerer chose whether its offer was late. The first attempt (this PR's first head) judged on the HLC in the
  offer's log key instead, and #609's review showed that is offerer-written too: any member can
  `kv().set("log/cn/{r}/offers/0000000000010000/…")` and win an award a minute after the deadline; the "drift bound"
  residual it stated was not a bound (the clamp applies to `Hlc::observe`, not to stored keys). Now no timestamp an offerer can write decides lateness: the **declarer closes** the requirement (`close(requirement, key, now_ms)`, at or after `deadline_ms` on its own clock), appending a `Close` to `cn/{r}/closes` that names every offer its node holds by log position and SHA-256; an award counts only the offers the close names and is refused `NotClosed` before one exists.
  Closes go on a stream (`cn/{r}/closes`), so neither a forged close written first nor an overwrite stops the declarer
  (round 2): the checked path takes the earliest close verifying under the pinned declarer — earliest by the signed
  `closed_at_ms`, then digest, never by stream key (round 3) — and `close` refuses `AlreadyClosed` only for one
  verifying under its own key; recovery from an overwrite **re-asserts** the declarer's original close byte for byte
  (`close` again on the same handle, `reassert_close(&kept)` after a restart) and never re-decides (round 3: a re-close
  that recomputed admitted a late offer — `a_reclose_after_an_overwrite_never_admits_an_offer_the_first_close_did_not`
  and `a_second_signed_close_at_a_backdated_key_does_not_change_the_award` seen failing on d90c2d51); a close binds its announcement (deadline, digest), an award
  records its close (`close_sha256`), and a close too large for one KV write is `CloseTooLarge` before writing. The
  residual, stated: the declarer decides which offers arrived in time — a signed close shows which it counted, not
  when it closed (`closed_at_ms` is its claim); and a close names offers by log position and hash, and the award re-reads them from the stream — so a member who deletes or rewrites another participant's named offer removes it from the candidates, and the award does not show it (the close and `close_sha256` are untouched). **Follow-up, not built:** carry each named offer's signed bytes inside the close and award from that copy. `offers()`/`offers_verified()`/`reports()`/
  `assessments()` read only records naming their own requirement (a signed offer copied from another requirement's
  stream verified there). `award_of` reports only a record naming its requirement and operation;
  `award_of_verified(requirement, declarer, resolve)` only one the declarer **the caller pins** signed (a check
  against the writable head fell in two writes); `plan_award_verified` is the checked path — a close the pinned
  declarer signed, verified named offers, and only a verified award as `AlreadyAwarded` — while the unverified path is
  documented as letting any member block an award; a linearizable slot holding another requirement's record is
  `SlotHoldsAnotherRecord`, not `Encoding("…did not decode")`, and on the checked path one holding an unproven award is
  `UnprovenAwardHolds`. New refusals `NotClosed`, `CloseUnproven`, `NotTheDeclarer`, `BeforeDeadline`,
  `AlreadyClosed`, `CloseTooLarge`, `CloseNotWritten` (the enum is `#[non_exhaustive]`); `Award` and `Close` gained
  fields. Seen failing first on 9d4da849 (round 2): `a_forged_close_written_first_does_not_stop_the_declarers_signed_close`,
  `an_overwritten_close_is_recovered_by_the_declarer_closing_again`,
  `on_the_unverified_path_a_close_dated_before_the_deadline_or_naming_another_declarer_is_refused`,
  `an_oversize_close_is_refused_by_name_not_reported_as_written`, and `mycelium-reason`'s `a_tiny_blob_costs_a_block`.
  Seen failing first (on 461da69f, round 1, the review's PoCs):
  `a_backdated_log_key_does_not_make_a_late_offer_a_candidate` (`driver-a`, the backdated offer, awarded),
  `an_offer_report_or_assessment_naming_another_requirement_is_not_read_as_this_ones`,
  `award_of_verified_checks_the_declarer_the_caller_pins_not_the_writable_head`,
  `a_slot_holding_another_requirements_award_says_so` (`Encoding`), `an_award_before_the_requirement_is_closed_is_refused`;
  and on 2933073f `award_of_reports_only_a_record_that_names_this_requirement_and_its_operation`.
  `mycelium-reason` **0.9.0** (its own changelog): the blob store bounded **per digest** (`DEFAULT_BLOB_QUOTA_BYTES`
  1 GiB, `open_with_quota`, 507 `quota_exceeded`; racing puts of one blob count it once — the first head's per-put
  reservation grew N×len, review finding 2 — and each file costs at least one 4 KiB block) and a caller-chosen `run_id` validated as one bounded segment at every
  door (`validate_run_id`, `TraceRecorder::new` → `Result`, 400 `bad_run_id`) — a run id with `/` wrote into another
  run's trace, which `anchor` then sealed. Run ids are validated; runs are **not owned**. `docs/guide/deprecations.md`
  §36.
- **An electorate member that restarts without its promises is detected: persistence required, an empty store
  warned, a forgotten promise withholds the slot** (`src/agent/acceptor_guard.rs`; the code half of #604's
  hazard — detection by name, never a guard on a Layer I write). A node that would adopt, declare, propose or
  answer a genesis for an electorate it is a member of with no `[persistence]` refuses by name:
  `ElectorateError::PersistenceRequired`, **409 `persistence_required`** at `/gateway/govern/electorate`, a door
  refusal, no acceptor answer (so a genesis naming such a member is never decided); counted on
  `electorate_persistence_refused`. A member of a committed genesis whose acceptor memory holds nothing for the
  genesis slot — every member accepted it, and the collector keeps the promise — is **warned once** at `start()`
  and when the record first arrives by gossip (`acceptor_memory_empty_for_electorate`); warned, not refused, since
  the record is read unverified and a brand-new member holds nothing until genesis. And a peer's copy of this
  node's **own** acceptor record arriving with a promise or acceptance above its memory — an answer it gave and
  forgot — **withholds that slot until the next start**: no prepare answered, no vote cast, no proposal made
  (`acceptor_promises_forgotten`; `Timeout` with reason `acceptor_withheld` for a proposer). The copy reaches the
  agent through a new core seam, `CoreCtx::inbound_observer` (`InboundObserver { prefixes, on_inbound }`), which
  `connection::apply_inbound` calls for every inbound update under a named prefix after the apply, whatever LWW
  made of it — core reports, the agent decides. Five tests seen failing first (the declaration decided; 200 at the
  gateway; epoch 1 adopted; nothing counted; **v2 committed at ballot 2 with B's memory moved from `(1, v1)` to
  `(2, v2)`** — two decided values); the electorate test harness now configures persistence, as a member must.
  **The limit, stated** (design §8.5, `what-is-proven.md`): the withholding needs the copy to arrive, and before
  the node answers a higher ballot for the slot — between a restart and the repair the hazard is open; a withheld
  slot never resumes without a restart — but the disk is repaired at detection: the merged record (the higher
  promise, the higher acceptance) is written to the node's own key under `acceptor_records`, so the next start holds
  the forgotten answers even where the node's own empty-handed write had beaten the copy by LWW, and
  `persist_acceptor` writes nothing for a withheld slot (#605's review); memory is re-read under the lock and merged
  again before the write, and an acceptance the copy carries at a ballot at or above memory's promise is forgotten
  too — a vote cast at `b` the node would cast again for another value — unless the promise is the collector's floor
  (the confirming review). The withheld check is re-run per ballot and
  before the proposer's own vote; a cross-group timeout reports its real quorum; the once-only notices are one
  bounded set per kind, and the adoption refusal is counted on every refused view. Counters on `GET /gateway/stats`;
  `mycelium_acceptor_guard_total{event}`.
  The module, its counters and the seam's installer exist only under `consensus` (a gateway-only build is untouched).
  **Upgrade notes:** an electorate member without `[persistence]` is now refused — configure it (it was never
  safe without); `ElectorateError` gained `PersistenceRequired` (`#[non_exhaustive]`, a `_` arm fails closed);
  `CoreCtx` gained `inbound_observer` (an exhaustive literal breaks); `TaskCtx` gained two sets.
- **An electorate's members change by a migration that terminates — close, enumerate, carry, complete — so a
  retired group can be decommissioned** (post-360 plan row P2's follow-up; `docs/design/consensus-electorate.md`
  §8.3, `docs/guide/deprecations.md` §34). 2.32.0 shipped P2 with membership fixed at genesis and no way to change
  it; the parked lazy carry (a successor reading a majority of its predecessor at its first touch of every slot,
  for ever) was replaced, by the maintainer's decision of 2026-10-11, with one that ends. Three governance acts,
  each a consensus decision with a certificate: **declare the successor first**, naming its predecessors
  (`declare_successor_electorate`, `{"predecessors": […]}`; a set — a merge when two or more) — it decides nothing,
  at the proposer's door and at every acceptor, until each has retired to it; **retire** the group into it
  (`retire_electorate`, `{"retire_to": …}`; `Retirement`, a strict majority of the retired group) — accepted only
  into a successor that names the retiree and only while no carry into the retiree is incomplete, passing the
  fleet's exclusive default exactly when held (`passes_default`, checked by every acceptor), and **closing every
  member at its promise, durably**, so no value of the retired group is chosen after it; **complete**
  (`complete_migration`, `{"complete": true}`, any member of the successor, idempotent; `src/agent/migration.rs`)
  — a **manifest** of every slot each member holds acceptor state for in the group, from a strict majority of it
  (the acceptor memory is tagged with the electorate group a claim passed through, `AcceptorSlot::groups`, in the
  durable record, tag `0x04`; manifests paged, `CarryManifestRequest`/`CarryManifestPage`), the union carried slot
  by slot with one ballot of the successor proposing the acceptance's decision envelope whole (`propose_carry`,
  `CarryPrepare`/`CarryPropose` — a lock keeps its holder's window, a released lock stays released, a committed
  value whose record did not propagate is carried), and the **completion** `electorate/{successor}/carried/{p}`
  (`Completion`, certified), before which the successor decides nothing outside the carry set and after which the
  predecessor is never consulted — its members may be removed — and a chain reads only its predecessor (a group
  retires only once every predecessor is complete or **dropped** — `drop_predecessor`, `{"drop_predecessor": …}`,
  admitted only for a predecessor whose retirement elsewhere is visible, or under the operator's `force` assertion
  that its majority is gone for good (audited; outside the guarantee — a false assertion ends as a detected
  **conflict** on the carry, counted on `commit_conflicts` and named in the completion, never prevented) — and every
  acceptor checks this at the retirement's prepare,
  before it closes; a verified completion stands as proof of the retirement, and the predecessor's records are
  fsynced on each successor member at the completion). The safety argument is six sentences in §8.3, each a multi-node test
  seen failing with its mechanism disabled, with the review's interleavings (D1–D3, the report-versus-fence race, a
  committed-but-unpropagated value) and the acceptance case: after the completion, every predecessor node shut
  down and the successor restarted from disk, the successor decides fresh slots and reads every carried slot.
  `GET /gateway/govern` shows `predecessors`, `retired_to` and each predecessor's `migration` phase (`waiting` ·
  `closed` · `carrying` *n* of |S| · `complete`); `ElectorateError::Retired` (409 `retired`); counters
  `electorate_waiting_refusals`, `electorate_retirements_undecided`, `migration_slots_carried`,
  `migration_completions`, `migration_manifest_pages`. Wire **v12**, four variants appended; a genesis without
  predecessors and the record of a slot never claimed on an electorate group are byte-identical to 2.32.0's.
  **Upgrade notes:** `ElectorateDecl` gained `predecessors` (an exhaustive literal breaks); `ElectorateError`
  gained `Retired`; every member of a retired group and of its successor must run this release to migrate. **Not
  built:** disaster recovery of a group that lost its majority (it cannot migrate either); the carry is one ballot
  per slot, sequential; an acceptance a restarted predecessor member holds only by digest, with no decision record
  anywhere, stops the carry at that slot, named.

### Changed

- **Documentation: the philosophy's "A protocol, not a service" paragraph states the post-360 plan's line in full**
  (row Φ, #607; no code change): consensus is a protocol every member of a group runs, never a service a named
  node provides; no shape — a profile, a configuration default, a unit file, a governance route, a document — may
  make a named node set the place where agreement happens; an electorate is a group whose members are fixed at
  genesis and change only by a migration that terminates; and what 2.32.0 enforces of it (the governed electorate,
  leased leadership, collected acceptor state) with the migration after it. The wiki's runtime-invariants page
  § Layer III cites it.

- **Hygiene, post-360 plan row J** (#608). The sim-seam gate's test-item skip now covers exactly the item a
  `#[cfg(test)]` / `#[cfg(all(test, …))]` attribute applies to, at any indentation (the brace-counting skip
  `check-kv-namespaces.sh` took from #591's review); until now it ran to the next column-0 `}`, and the `#[cfg(test)]`
  on `ConsensusEngine::shrink_acceptor_for_test` (#600) had hidden the five timer sites after it in
  `src/consensus.rs` — the gate counted 0 against a baseline of 5 and reported only *progress*, so a new direct clock
  read in that `impl` would have passed since 2.32.0. Planted shapes in `scripts/test-check-sim-seams.sh`: three fail
  on the old gate; `skip_more_attrs` passes on every version (a guard) and `skip_whole_module` is the control. Its first cut here ended a skip only at a `;` or a body, so a `#[cfg(test)]` on a comma-terminated element ran into the next item — `mycelium-core/src/signal.rs`'s `#[cfg(test)] trim_hook` field (#602) hid all of production `impl SignalLog`, three `Instant::now` sites (`:671`, `:673`, `:693`) that the old gate also lost at #602 (its count fell 11 → 8 with no site removed); found by the PR's adversarial review. The skip now also ends at a trailing `,` or an unbalanced `}`, and strips block comments and raw strings before counting braces; the same fix and plants are in `check-kv-namespaces.sh`, where the hole was latent. What still fails open (a multi-line comment or string holding an unbalanced brace inside a test item; a raw string containing `"#`) is stated in both headers. The baseline: `signal.rs` stays 11, `capability_ops.rs` 2→0 and `intent.rs` 1→0 (below), `http.rs` 15→14
  (stale on `main`). Every Cargo feature and both
  bench targets are now linted by a CI step on their own (`otel`, `--benches`; `metrics` on tuple-space, blackboard,
  effects and wiki; guardrails `gateway,metrics-export`; reason `compliance`; the coop examples' `metrics`, a
  `cargo check` in the coop-smoke job) — before, only the test-universe job's
  `--all-features -- --list` compile touched them (and the wiki's `metrics` lint found `mycelium` built with `metrics` and without `gateway` failing `-D warnings` — `mod emergent`'s allowance (`not(any(gateway, metrics))`, `src/agent/mod.rs:106`) lifts under `metrics` alone and the six explain items only `/gateway/explain` reads were dead; each now carries `cfg_attr(not(gateway), allow(dead_code))` and `cargo clippy --lib --no-default-features --features metrics` is a named line (embedded job, `make check`)); the `effects` and `commitment` jobs are pinned to `1.96.0` like
  every other (they floated on `stable`). Two duplicates that disagreed are one definition: `hex_line` in
  `mycelium-wasm-host` (the gateway's copy trimmed, the catalogue's did not — one `pub(crate)` decoder, trimming,
  pinned by a test) and `now_ms` (`capability_ops.rs`'s two copies and `intent.rs`'s read `SystemTime::now()`
  directly while `federation/edge.rs` read the seam — every copy in `src/`, `mycelium-tuple-space` and
  `mycelium-agentfacts` now reads `sim_seam::wall_now_ms`, so a recording replays an intent's stamp and a
  capability's freshness; the baseline rows for the two `src/` files drop to zero, which is the structural pin, and
  `every_now_ms_is_the_seams_wall_clock` checks they read the seam's clock — it cannot tell the old copies from the
  new outside `sim`. The tuple-space and agentfacts copies sit outside the gate's scan and are pinned by no gate.
  **Under `sim`, each of these reads is now a recorded kernel choice**, so a recording made before this change that
  touches these paths would diverge on replay; the checked-in corpus is core-only and unaffected).

- **`src/agent/http.rs` split by route family — behaviour-preserving (post-360 plan row P5, #610).** The
  9,889-line gateway file is now the directory `src/agent/http/`: `mod.rs` (the handler state, listener, TLS,
  router and public surface), **`authz.rs` — the one shared authorisation-and-evidence layer every family goes
  through** (`gateway_auth`, `required_scope`, the AE preflight and its evidence records, the protected-kind /
  protected-key / owned-stream refusals), and one file per family — `node.rs`, `mcp_bridge.rs`, `signals.rs`,
  `caps.rs`, `mesh.rs`, `kv.rs`, `overlay.rs`, `govern.rs`, `compliance.rs`, `llm_bridge.rs`,
  `federation_routes.rs` — with the three test modules in `http_tests.rs` (still module `tests`),
  `gateway_caller_tests.rs` and `ae_seam_tests.rs`. No route added, removed or renamed, no scope changed, no
  handler logic changed: every moved line is byte-identical to the original (checked by a script over the line
  map), the only token added inside moved code being `pub(super)` on 153 column-0 items so the families and the
  tests can still name one another; the public surface (`GatewayPreflight`, `GatewayRefusal`, the `pub(crate)`
  preflight API `a2a.rs` and `provider_enforcement.rs` use) is re-exported from `mod.rs` unchanged, so no caller,
  companion, example or test changed. Every gate that greps the file by path was re-pointed — the sim-seams
  baseline (one row became four, 14 sites as before), the wiki-lint skill's router grep, the lock-order table's
  three `HttpCtx` rows, the KV non-key notes — and the test that enumerates the governance routes from the
  router now reads `mod.rs`.

- **The gateway remainder (post-360 plan row E, #613).** Six promises, each with a test seen failing on the code
  before it (`src/agent/http/remainder_tests.rs`, `gateway_caller::tests::a_stale_or_future_envelope_is_refused`,
  `mycelium-core/src/log_cursor.rs`; `docs/guide/deprecations.md` §38). **Client timer periods are bounded:**
  `capability/advertise` and `units/declare` take `interval_secs` and `lease_secs` as whole seconds in `1..=3600`
  (`MAX_TIMER_PERIOD_SECS`) and answer 400 `timer_out_of_range` naming the field otherwise — any `u64` was a period, and
  `0.5` or `"30"` silently became 30 s; a unit file's `ttl_secs` is held to the same range by `validate()` (`0` was a
  1 ms re-assert loop); a gateway advert gossips a `CapEntry` with its own `refresh_interval_ms` instead of a bare
  `Capability` every reader timed at the legacy 60 s; both SDKs send `interval_secs` in whole seconds, rounded up
  (`mycelium-py` 0.2.11, `mycelium-ts` 0.2.5). **The mailbox stream deletes an event as it hands it to the HTTP
  connection** (`mailbox::open_mailbox_acked`, an in-flight set so nothing is queued twice), not as it enters the
  256-slot channel — a dropped stream had already deleted every queued event (#595's review); the in-process
  `open_mailbox` is unchanged. **The log-group subscribe keeps the core's `(hlc, key)` order** — of two same-HLC entries
  the gateway and `KvHandle::subscribe_log_group` delivered one and skipped the other for ever — through a position
  kept as `(hlc, key)` at `clog/{stream}/{group}/cursor` beside the legacy HLC-only `…/offset` (still written, one below the entry's HLC so a reader that has only it re-delivers rather than skips; read when
  ahead of the cursor, so an older node and a newer one agree), and **records it after the entry is handed on**: at
  the gateway as the stream hands it to the connection (it was saved before queueing, so up to 64 queued entries were
  skipped by the next holder — found by row G), in process after the channel accepts it (it was saved before a send
  to a receiver already gone); `log/scan` and `log/subscribe` order same-HLC entries by key too. **Prompt keys cannot
  alias:** an empty `ns`/`name` or one with a `/` is refused at the three prompt routes and `llm/call` (400
  `invalid_prompt_name`), `llm/stream` (an error event) and in process (`PromptSkillError::InvalidName`) —
  `prompts/{ns}/{name}` made `(a/b, c)` and `(a, b/c)` one key. **The caller envelope binds freshness:**
  `gateway_caller::verify` refuses an envelope issued more than `CALLER_CONTEXT_MAX_AGE_MS` (300 s) plus
  `max_clock_drift_ms` ago, or more than `max_clock_drift_ms` ahead (`CallerError::Stale`, code `stale`); the issue
  time was signed and never read, so a captured envelope verified for ever. Also: `mod.rs` cites the public-surface
  test by its real name. **Upgrade notes:** `PromptSkillError` gained `InvalidName` (an exhaustive `match` breaks);
  the 400s above; a provider reading a request from `rpc_rx` more than 300 s plus the drift bound after it was sent
  refuses it `stale`. **What a drop still loses:** a mailbox event or log-group entry the HTTP connection had accepted
  — the gateway has no resume point (guide 10, *not built*). **Review round** (#613's adversarial review): `max_clock_drift_ms
  = 0` means no drift bound for the envelope too (the age bound is 300 s alone, and at most 300 s ahead —
  `a_zero_drift_bound_bounds_only_the_age`, seen failing first: `left: Err("stale") right: Ok(true)`); two mailbox
  streams of one kind share one in-flight set, so neither hands out what the other holds, and a dropped stream's
  un-acknowledged events go to the other (`two_mailbox_streams_for_one_kind_never_hand_out_one_event_twice`, seen failing
  first: B was handed `AQ==`); the legacy offset is written one below the entry's HLC, so a peer that receives it before
  the cursor re-delivers rather than skips (`an_offset_that_arrives_before_its_cursor_never_skips_a_sibling`, seen failing
  first: `ThroughHlc(512)`); a `[[serve]]` name with a `/` is refused at load
  (`a_serve_name_that_cannot_be_a_prompt_skill_is_refused_at_load`); every `prompts/` key builder (including `a2a.rs` and
  the LLM provider) goes through one function; both SDKs refuse an `interval_secs`/`lease_secs` of 0 or past 3600 locally (seen
  failing first: 4 py, 4 ts); the 300 s window is stated as the gateway's (an in-process caller behind a backlog is
  refused at dequeue); the bound's "never clamped" narrowed to the two routes it covers.

- **Documentation: a lost electorate voter is restored with its state or retired — never reissued empty.**
  2.32.0's pages said an electorate member could be replaced "under the same identity"; that is unsafe as
  stated. A member's promises and acceptances are durable on its own disk
  (`sys/consensus-accepted/{node}/{slot}` — the promise in the record since 2.30.0, fsynced before the answer
  leaves since 2.32.0, under `[persistence]` only; read once at start by `prewarm_accepted`, a gossiped copy
  reaching the store but not the running acceptor), and nothing refuses a start that finds none, so an
  electorate member must configure persistence, and an acceptor reissued its `NodeId` over an empty or older
  store can promise below
  what it promised and forget what it accepted — two decided values. The operator fork is now written on every
  page an operator reads (guide 04, the design record §8.3/§8.5/§8.6, threat model §7, `what-is-proven.md`,
  the FAQ, `cert-rotation.md`, `deployment.md`'s restore table, `production-readiness.md`,
  `confined-fleet.md`, the wiki): **restore** — same `NodeId`, same or off-node-reissued certificate, and
  `{persistence.base_path}/{node_id}/kv/` at its latest state; or **retire** — onto the removal ledger, which
  forecloses the restore. A lost majority's repair (an operator-forced genesis from survivors' durable state)
  is named as not built in `what-is-proven.md`'s third table. No code change.

- **`mycelium-py` 0.2.11** (its own tag `mycelium-py-v0.2.11`; post-360 row E, #613): `advertise_capability`
  and `declare_units` send `interval_secs` in whole seconds, a fraction rounded up, as `lease_secs` already was,
  and refuse an `interval_secs` or `lease_secs` of 0 or past 3600 (`MAX_TIMER_PERIOD_SECS`) with `ValueError`
  before any request — the node's new bound. README § *Timer periods (0.2.11)*.

- **`mycelium-ts` 0.2.5** (its own tag `mycelium-ts-v0.2.5`; post-360 row E, #613): `advertiseCapability` and
  `declareUnits` send `intervalSecs` in whole seconds, a fraction rounded up, as `leaseSecs` already was, and throw
  on an `intervalSecs` or `leaseSecs` of 0 or past 3600 (`MAX_TIMER_PERIOD_SECS`, exported) before any request.
  README § *Timer periods (0.2.5)*.

- **Documentation: the post-360 plan's later rows and state of play** (#612, #614, #615, #618; no code change). Rows
  K, L, M from the Strands Box and NVIDIA OpenShell comparison (policy over history, credentials at egress, confined
  activations), N, O, P (Strands agents on Mycelium, with the `a2a-sdk` probe's evidence under
  `docs/plans/external/2026-10-12-a2a-sdk-probe/`), S (identity proofs on by default) and T (a loud warning when the
  CA key is on the node); §3.6, the state of play, with the shutdown drain's limit stated.

### Deprecated

- **Nineteen public items (eighteen names; `with_max_tokens` is two methods) nobody references, deprecated and not removed** (#608; `docs/guide/deprecations.md`
  §37, the sweep and its greps in the PR). `#[deprecated(since = "2.33.0")]` on
  `mycelium_core::MeshHandle::group_quorum_prehashed` · `mycelium_core::receipt::…::with_replica_sync` ·
  `mycelium_core::kv_ns::{MANDATE, CONSUMER_LOG, LOCK}` · `mycelium_effects::Counting::with_counts` ·
  `mycelium_sim::bundle::…::path_for` · `mycelium_wasm_host::MeshArtifactSource::cached_len` ·
  `Provisioner::disable_resource_policy` · `Provisioner::decision_trace` · `mycelium_wiki::…::with_semantic_lint` ·
  `mycelium_wiki::lint::…::with_max_tokens` and `reconcile::…::with_max_tokens` · `RequestPrincipal::is_client` · `AgentStateMachine::set_task_id` ·
  `mycelium::federation::{verify_descriptor_rotating, verify_policy}` (`tls`) ·
  `FederationClient::with_credential_lifetime` · `mycelium::knowledge::store::…::issuers_about`. Nothing is removed on the 2.x line; an issue naming a use undeprecates it. The sweep's twentieth item,
  `AeEvidence::audit_outcome`, is called by the private companion and is not deprecated; `Preflight::is_inert`
  (`http.rs`) waited for rows P5/E.

- **`GatewayPreflight::is_inert`** (`src/agent/http/authz.rs`), `#[deprecated(since = "2.33.0")]` with §37's note
  (post-360 row E): referenced nowhere in the repository, the SDKs or the private companion (`grep -rn is_inert`
  over both trees).

### Security

- **Dependency: `quinn-proto` 0.11.15 → 0.11.18** (RUSTSEC-2026-0338, -0339, -0340, -0341, -0342: two remote panics on crafted STREAM/ACK_FREQUENCY frames, a stream-reassembly memory guard and a zero-length DATAGRAM buffer bypass, unbounded connection-ID retirements). Reached through `reqwest`'s HTTP/3 client (`async-openai`, `object_store`, `jsonschema`, `reqwest-eventsource`); a lockfile change only, no API change.
- **A revoked key stops verifying at every verifier, and only a newer certificate of the node can revoke it**
  (post-360 plan row Q, #616; decision record `docs/design/revocation-authority.md` rev 3). After
  `rotate_identity_on_compromise` a peer listed the old key as revoked yet still applied a `SignedData` signed by it
  (#611's review): the KV arm, identity chaining and the unsigned identity exemption read `peer_keys` raw. Now every
  verifier of a node's key reads one answer, `CoreCtx::trusted_keys` — the KV arm (`kv_revoked_key_refused`), the
  exemption, chaining, consensus (whose fallback to the raw `sys/identity/` value is gone), caller envelopes,
  `known_verifying_keys`, knowledge keys and `mycelium-agentfacts` (which verified against the raw value). **Who may
  revoke** is decided by **certificate records**: a node publishes `sys/identity-cert/{node}/{key}` (its certificate,
  which now names the node by a `mycelium://<node>` URI and carries its issuance time in the serial); a node with one
  trusts exactly its anchored keys it has not revoked, and a revocation of key P counts only when signed by an anchored
  key issued after P's (or P has no standing). So a poisoned, first-sighted or legacy key neither signs nor revokes for
  an anchored node, and a thief holding a retired key cannot revoke its successor — #616's first fix let any held key
  revoke any other, for ever, and its review showed that silenced a node from a poisoned key, from a first sighting
  after a restart, or from any leaked retired key, and let a restart bypass the chaining fix. Revocations live at
  `sys/revocation/{node}/{P}/{S}` (one per pair, so a later write cannot replace the owner's), as the v1 bytes plus a
  trailer carrying the certificates they rest on (a 2.32 node still reads the prefix); counted ones are journaled
  (`{persistence}/identity/revocations.journal`, bounded, failing closed when full), so a revocation hidden while a
  node was down still counts after its restart; the owner republishes its records, and the `sys/` tripwire counts a
  peer's write to them; a certificate issued past the clock plus the skew bound is ignored and counted
  (`identity_cert_future`). New `/stats`: `identity_cert_future`, `identity_records_over_cap`,
  `revocation_journal_refused`; new guarantee `id.revocation_authority`. Seen failing first on e5c2c27e (#616's first
  head): `a_poisoned_identity_key_neither_revokes_nor_signs_for_an_anchored_member`,
  `a_first_sighted_key_after_a_restart_cannot_revoke_an_anchored_member`,
  `a_fork_from_a_revoked_key_is_not_trusted_after_the_verifier_restarts`,
  `a_retired_key_cannot_revoke_its_successor_and_rotation_continues`,
  `a_revocation_hidden_while_the_verifier_was_down_still_counts`, `an_owner_republishes_its_overwritten_certificate_record`,
  agentfacts' `a_field_signed_by_a_key_written_into_the_raw_identity_does_not_verify`; and on 7c967ba3 (before #616),
  `a_kv_write_signed_by_a_revoked_key_is_refused_once_the_revocation_is_seen` and the other six #616 tests. **Stated,
  not built:** no protection against a holder of the fleet CA key (every node in the development posture); in the secure
  posture rotation cannot run and recovery is re-issue only; a retired key verifies until revoked; unsigned `Data` and
  anti-entropy are not covered; a node with a pre-row-Q certificate has no standing until re-issued, and keys it revoked
  before the upgrade verify again at upgraded nodes until then; the mTLS handshake does not refuse a revoked key. **The
  final review's round** (each seen failing first on 38da0472): history verifies under a separate verify-only set —
  every key held minus revoked ones — so a node's audit chain sealed under a key without a certificate record keeps
  verifying and its genesis is never re-sealed (`an_audit_chain_sealed_under_a_key_without_a_certificate_record_survives_restarts`);
  a key ranks by its first issuance and a counted revocation stays counted, and a node starting with a key its own
  records show revoked refuses to start (`a_revoked_key_restored_at_its_node_does_not_regain_trust_at_a_peer`,
  `a_node_whose_key_pem_names_a_retired_key_refuses_to_start`); a revocation record carries at most two certificates,
  judged certificates are cached, and the caps apply before any check (`junk_revocation_records_keep_a_fold_bounded`,
  3.6 s → under 250 ms); a rotation issues strictly after the node's newest issuance
  (`a_rotation_after_the_clock_steps_back_still_outranks_its_predecessor`); a node adopts only revocations its own
  current or anchored keys signed (`an_owner_does_not_adopt_a_revocation_planted_through_its_raw_identity`); revoking
  one key twice keeps one record (`revoking_one_key_twice_writes_one_record`). **The confirming review** (each seen
  failing on 39cfb571): a revocation of a key with no anchor yet is provisional — re-judged every fold, never journaled
  — and a signer the node counted as revoked signs nothing, so a thief's revocation judged before the successor's
  certificate does not lock in (`a_thiefs_revocation_judged_before_the_successors_certificate_does_not_lock_in`); at
  start the node's own certificate anchors first (`a_planted_record_does_not_refuse_a_start_whose_own_certificate_record_is_missing`);
  the history a node republishes at start comes from its own records, never the raw identity value
  (`a_raw_identity_value_a_member_writes_does_not_enter_the_owners_history`). **The N1/N2 check** (each seen failing on
  4908ecd3): audit recovery never seals over an existing record — records that do not verify are kept, reported
  (`audit_unverified_records` on `/stats`, `audit_unverified` in the guarantee report) and numbering resumes after them,
  so a node upgraded after an old-version rotation keeps its genesis and reports its pre-upgrade prefix unverified
  (`an_audit_genesis_survives_an_upgrade_after_an_old_version_rotation`); the revoked-signer rule is dropped, so two
  compromise rotations learned out of order leave the oldest key revoked
  (`two_compromise_rotations_learned_out_of_order_leave_the_oldest_key_revoked`). **The safety net's review** (each seen
  failing on 81b70a2e): only `{seq:016x}` keys whose record names that seq are read into a stream
  (`an_audit_record_whose_key_and_body_disagree_is_ignored`); a far-away unverified record is reported, not followed,
  and a seal skips an occupied slot and never wraps (`a_forged_high_seq_record_neither_pushes_nor_wraps_the_numbering`,
  `a_far_away_unverified_record_is_reported_not_followed`); `sys/audit/` and `sys/audit-checkpoint/` trip the `sys/`
  tripwire (`a_peers_write_into_a_nodes_audit_stream_trips_the_tripwire`); a stream reads segment by segment
  (`audit_segments`, `GET /gateway/audit` `segments`) and `/stats` counts unverified records
  (`one_planted_record_does_not_leave_the_rest_of_the_stream_unverifiable`); pruning keeps unverified records. **The
  H1/M1 review:** a public `verify_linked` follows the hash links, the contiguous verifiers unchanged and pointing at it
  (`a_stream_with_a_skipped_slot_verifies_under_verify_linked`); a planted run of any length leaves the node sealing — the free slot found outside the audit lock, slot by slot, the run reported (`a_planted_run_of_any_length_leaves_the_node_sealing_and_is_reported`, `a_seal_over_one_planted_record_costs_the_run_not_the_stream`); a pruned stream whose boundary record was removed anchors at the
  newest checkpoint below it. Upgrade notes in `deprecations.md` §39.
  Wire **v12** unchanged.
- **Wire and crypto, one MINOR with a window (post-360 plan row F).** Wire **v12** unchanged — no frame changes
  shape; what changes is what a node accepts and what a signature is over. Three changes and one closure:
  - **KV `SignedData` is domain-tagged.** A KV write is signed over `mycelium.kv/update/1 ‖ u32 len ‖ canonical`
    (`framing::kv_signing_message`), beside 2.32.0's identity and consensus tags. The one signer
    (`make_kv_wire_msg`) and the one verifier (`connection::handle_connection`) were enumerated by
    `grep -rn "canonical_sign_bytes(" src mycelium-core/src mycelium-*/src fuzz`. **Window, one release:** the bare
    2.32 form is accepted and counted (`GET /stats` `kv_untagged_signatures`, `mycelium_kv_untagged_signatures_total`).
    Seen failing first: `a_kv_signature_is_over_the_tagged_message_not_the_bare_bytes`,
    `a_tagged_kv_signature_is_accepted_at_the_connection`, `a_signature_under_another_tag_is_not_a_kv_signature`.
  - **A TLS node can refuse an unsigned `Data` frame.** Under `[tls]` the `Data` arm applied a frame whatever the
    node's posture, so a member could write under another member's `sender` hash by not signing. Enumerating the
    writers (`grep -rn "WireMessage::Data(" src mycelium-core/src mycelium-*/src`) found **twenty-three agent write
    sites that never signed** — membership, mailboxes, state machines, opacity, the gateway's log/KV/offset routes,
    MCP tombstones, the soft-state advertiser; all go through `framing::local_kv_wire_msg` now, and
    `scripts/check-signed-dispatch.sh` (make check, CI, with a planted self-test) refuses a new bare dispatch.
    `kv_require_signed_frames` (`GOSSIP_KV_REQUIRE_SIGNED_FRAMES`) drops the frame; **window, one release:** it
    defaults to `false`, and an unsigned frame is applied and counted (`kv_unsigned_frames_accepted` /
    `kv_unsigned_frames_refused`, `mycelium_kv_unsigned_frames_total{outcome}`). Seen failing first:
    `a_tls_node_requiring_signed_frames_refuses_an_unsigned_data_frame`,
    `an_unsigned_data_frame_is_applied_and_counted_during_the_window`, and
    `an_agent_write_reaches_a_peer_that_requires_signed_frames` (with the refusal built and the sites not yet
    rerouted). **Not covered:** anti-entropy — a `StateResponse` entry carries no author or signature.
  - **A peer certificate's key is read structurally.** `tls::ed25519_key_from_cert_der` scanned the DER for the
    Ed25519 SPKI prefix and returned the first match — in a serial number, a name or an extension the issuer
    chooses — so a node holding the CA key could mint a certificate that anchors as another member
    (`GossipStream::peer_ed25519_key`) or passes a removal of its own key (`refuse_if_removed`). It now reads the
    `SubjectPublicKeyInfo` with `rustls-webpki` (already in the tree) and compares it with RFC 8410's one Ed25519
    encoding. Local, no window. Seen failing first:
    `a_planted_key_pattern_in_the_serial_does_not_fool_the_structural_read`,
    `a_removed_member_hiding_its_key_behind_a_planted_pattern_is_still_refused`,
    `a_non_ed25519_certificate_has_no_ed25519_key_whatever_its_serial_carries`.
  - **2.32.0's §23 window closes.** A bare identity proof is refused in both `require_identity_proofs` postures, a
    bare consensus signature does not verify, an acceptor answers only a tagged request (`SignatureForm` and
    `sign_payload_as` removed), and the two counters `identity_untagged_proofs` / `consensus_untagged_signatures`
    (and their metrics) are removed. Seen failing first against the allowance:
    `a_bare_identity_proof_is_refused_now_the_window_is_closed`,
    `a_bare_proof_signed_by_the_victim_is_refused_and_flagged`,
    `a_consensus_signature_verifies_only_in_the_tagged_form`, `an_acceptor_answers_only_a_tagged_request`.

  - **#611's review, fixed before merge.** A `SignedData` frame's **signer is bound to its sender**: the arm verified
    against `signer`'s key and applied the update as `update.sender`'s, so a member's own valid signature carried a
    write attributed to anyone, `sys/` keys included — now dropped and counted (`kv_signer_mismatch`,
    `mycelium_kv_signer_mismatch_total`). The connection handler's **quorum-evidence write** was a twenty-fourth bare
    dispatch, in a file the first gate exempted whole; it is signed, and the gate now exempts only marked receive
    paths, catches any spelling (a `use`-imported `Data(` too) and plants three dispatches in its self-test. The
    removal check **fails closed** on a certificate with no Ed25519 key. The window counters are **per node**
    (`TransportBounds`) and on `GET /stats` with `kv_signer_mismatch`; the guarantee report gains
    `mesh.signed_writes` (not required by `secure-single-domain` this release); `start()` warns when
    `kv_require_signed_frames` is set without `[tls]`. **The second review** found the first identity exemption (any
    identity frame whose `sender` named its key) exploitable — `sender` is the writer's choice: impersonation with proofs
    off and silencing in both postures, reproduced on three nodes. Now the only exempt unsigned frame is a sealed identity
    record that authenticates itself (proof under its own key, chained to the keys held; counted as
    `kv_identity_exempt_frames`); a node publishes it first, unsigned, then its legacy pair signed; and an identity
    tombstone removes no keys (it silenced a member, signed or not). The dispatch gate skips only an inline
    `#[cfg(test)] mod x { … }` (it stopped at an out-of-line `mod x;`), and both per-frame warnings are rate-limited.
    **The third review (N4):** a node's genuine pre-rotation sealed record, replayed unsigned with a fresh timestamp,
    still proved itself and replaced the current one; an exempt record must now contain every key the receiver holds
    or has stored for the node, and a stale one is refused whatever the flag says (`kv_identity_stale_refused`). Seen
    failing first on 884f310f: `a_replayed_pre_rotation_sealed_record_does_not_replace_the_current_one`. First sighting
    remains trust on first use. The gate's brace counter blanks string and char literals (L1, a fifth plant). Plan row
    Q records a pre-existing, high finding the review reproduced on `main`: revocation does not reach the KV verifier,
    identity chaining or the exemption; `revoke_key`'s doc now says what holds.
  - **CI's co-op demo 04 (provisioning) found a shutdown race row F made reliable.** A capability's soft-state task
    tombstones its key when the handle drops; a provider dropped and shut down at once raced the shutdown signal, and
    the signed tombstone — slower to build in a debug build than the unsigned frame it replaced — lost every time, so
    peers kept the dead provider's capability until its TTL and the worker's RPCs timed out. Graceful shutdown now
    waits (bounded, 250 ms) for queued gossip and writer frames to drain before signalling (`flush_outbound_before_shutdown`),
    and the soft-state and MCP tombstones are signed before their local apply. Seen failing first:
    `a_capability_retracted_at_shutdown_reaches_the_peers` ("B still resolves the capability A retracted as it shut
    down"); the 6081c9e6 export also fails demo 04 locally, origin/main passes it — CI's one green run there was timing.
    Seen failing first on 59d86368: `an_unsigned_identity_forgery_cannot_make_a_member_sign_as_another`,
    `an_identity_tombstone_does_not_erase_a_members_keys` (proofs off and on),
    `only_a_self_authenticating_sealed_identity_is_exempt_from_the_refusal`, and the gate's fourth plant. Seen failing first:
    `a_signed_frame_whose_signer_is_not_its_sender_is_refused`, `quorum_evidence_reaches_a_peer_that_requires_signed_frames`,
    `a_certificate_without_an_ed25519_key_is_refused_by_the_removal_check`, and
    `a_gateway_kv_delete_reaches_a_peer_that_requires_signed_frames` with that site toggled bare. **The opt-in refusal is
    bypassable through anti-entropy** (a `StateResponse` entry carries no author), and the unconditional refusal is
    plan row F2.

  **Upgrade notes** (`docs/guide/deprecations.md` §35, the 2.30.0 shape): **upgrade from 2.32, not 2.31** — a 2.31
  node and an upgraded node verify neither each other's identity proofs nor their consensus messages. **An upgraded
  node's KV writes reach a 2.32 peer only by anti-entropy until that peer is upgraded** (a 2.32 verifier drops the
  tagged `SignedData` as a bad signature, and does not forward it). **An upgraded node with
  `kv_require_signed_frames` drops a 2.32 peer's agent writes until every peer is upgraded** — set it once
  `kv_unsigned_frames_accepted` and `kv_untagged_signatures` stop rising fleet-wide. **Upgrade promptly:** until a 2.32
  peer is upgraded it receives upgraded nodes' writes only by anti-entropy and logs a warning per dropped frame. A 2.31
  node upgraded straight past 2.32 is worse off than one with no proof (its bare proof is refused, so its writes are
  dropped as from an unknown signer). `GossipConfig` gained a field
  (an exhaustive literal breaks). Remove alerts on the two removed metrics. The next MINOR refuses unsigned frames
  and bare KV signatures unconditionally.

## [2.32.0] — 2026-10-11

**The 360 review's must-do list, and the post-360 hardening plan's first wave.** A gateway bound off loopback with no
credential refuses to start unless `gateway_allow_unauthenticated` says so, a blank token is refused in every form,
and `secure-single-domain` is **rev 3** (`gw.exposed_closed`, row P1). **A consensus electorate is a declared,
governed group** (row P2): an electorate group's members are pinned by identity and **fixed at genesis**
(`declare_electorate`, `POST /gateway/govern/electorate`); with `consensus_require_electorate` a lock, an election, a
consistent write or any safety-sensitive proposal outside one is refused `ElectorateNotGoverned`; the cluster-scoped
exclusive verbs decide in the one group marked `exclusive_default` (`exclusive_propose`); and the secure profile is
**rev 4** (`cons.safety_profile`) — both revisions in this one release. **Changing an electorate's members is not built
in this release** (the maintainer's decision, 2026-10-11; the migration is the follow-up). **A lock, a lease and a leadership are one
decision record** that ends where every node can see it: a released lock stays released, a reopened lease does not
adopt an expired holder, a late COMMIT does not revive a lock; `elect_leader` is **leased (30 s) by default** with
`release_leadership` (C1), and finished acceptor state is collected (C2) — row A, wire v12 with five appended consensus
variants. **The transport is bounded** (row B): handshake, idle and read-progress bounds with a 1 KiB/s floor on the
accept path, connect and write-stall bounds on the writer, one anti-entropy reply per peer, no subscriber veto, a
bounded signal log, a stopped agent freed — with the residual stated: a peer that keeps talking can hold the
connection slots it obtained, mitigated by mTLS and the opt-in `max_connections_per_source`. The gateway's doors answer
only what they were asked by whom: the signal SSE streams refuse protected kinds, `rpc/respond` answers only a request
its own serve stream delivered, an A2A task belongs to its creator, the tuple overview sits behind the bearer, the LLM
doors and `mycelium-reason`'s façade (**0.8.0**) run the action preflight as the HTTP client, the OIDC key refresh is
single-flight and timed out. Identity proofs and consensus signatures are domain-tagged (the bare form accepted for
one release), a proposer must be in the group's roster (`NotAMember`), the acceptor's promise reaches the WAL before
it answers, and a forged decided floor is counted and no longer overflows the ballot draw. Hosted WASM components are
confined (`mesh.emit`), bounded in memory and in time — a call has a deadline and runs off the async workers, a trap
does not recompile (row D) — and a `[hosts]` table must name its publishers. Durability: the audit chain's head
survives a restart, a failed WAL append poisons the writer until a snapshot repairs the file, and the companion WALs
compact crash-durably and refuse a corrupt record (row C). `#![deny(unsafe_code)]` across the library crates (row P3);
the KV namespace table is a CI gate. Companions on their own lines: `mycelium-py` **0.2.10**, `mycelium-ts`
**0.2.4**, `langgraph-checkpoint-mycelium` **0.3.3** (TLS, the checkpointer's bearer, whole-second timeouts, escaped
path segments — row G), `mycelium-reason` **0.8.0**. Wire **v12** unchanged in version; consensus gains appended
variants. **Check before upgrading:** a node whose gateway binds `0.0.0.0` (or any non-loopback address) with no
credential, or holds a blank token anywhere, now refuses to start — an unauthenticated `GET /gateway/kv/keys` from
another host answering 200 today means that node is exposed; under `profile = "secure-single-domain"` (now **rev 4**)
a node must set `consensus_require_electorate = true` or it will not start, and then a lock or an election on the whole
cluster or an undeclared group is refused — declare the electorates, sized for their whole life (their members cannot
be changed in this release), and mark one `exclusive_default` only once every node runs 2.32.0; a `lock/`,
`consistent/`, `capauthz/` or commitment-award slot is refused on any group but that default, and `leader/{g}` on any
group but `g`; **`elect_leader` now lapses after 30 s** unless called
again (or asked for `LeaderTerm::Permanent`), and an upgraded proposer times out until a quorum of its group is
upgraded — upgrade every node; `validate()` refuses `inbound_idle_timeout_secs` at or below a non-zero
`writer_idle_timeout_secs`; the outbound write-stall bound defaults to **19 minutes** (1 140 s) and the read floor to
1 KiB/s (set `peer_min_rate_bytes_per_sec = 0` for a joiner on a thin shared link); a `secure-single-domain` node
running `sync_mode = "os"` refuses to start; a unit file with `[hosts]` and no `trusted_publishers` fails to load.
Upgrade notes in `docs/guide/deprecations.md` §23–§33.

### Changed
- **Documentation: the front doors route by intent — build, run, see it work, check the evidence** (2026-10-10; no
  code change). Under the hero and the sentence, the root README adds one plain-language line and an intent router
  of four routes — *Build a fleet* (the developer guide, its tutorials and the five steps), *Run a fleet* (the
  operator journey: readiness and observability), *See it work* (four examples: `conway` for the mesh,
  `provisioning_viz` for self-provisioning, `procurement_authority` for governed action, `diagnostics` for
  operational insight), *Check the evidence* (`what-is-proven.md`) — then a six-row capability compass into
  `docs/capabilities.md`, then the five steps under *Build a fleet — five steps*. `examples/README.md` leads with a
  *What do you want to see?* chooser of the same four, then the five steps as its *Learning path*, then the matrix.
  The operations door keeps its own operator journey; `docs/positioning.md` no longer lists it as carrying the five
  steps, which it never did. `scripts/check-positioning.sh` now runs `scripts/check-front-doors.py`, which checks
  structure and link targets (anchors included), not prose; `scripts/test-check-front-doors.py` shows ten planted
  breaks fail it (CI's documentation job and `make check`).
- **Documentation: discovery is not an electorate — the consensus electorate decision recorded**
  (`docs/design/consensus-electorate.md`, post-360 plan D1). Discovery membership and the consensus electorate are
  separate capabilities by design; the supported profile for safety-sensitive agreement is a fixed electorate per
  decision; consensus is a Layer III protocol, never a service. The record cites what is enforced today
  (`ElectorateUnavailable` and the intent floor, `NotAMember`, the gateway's `governed_group` refusal) and states what
  is not: an embedded `join_group` or `grp/` write still moves a governed group, and nothing requires a governed group
  for a safety-sensitive proposal (row P2, not built); leased leadership and acceptor collection (C1, C2) and
  versioned electorates are not built. Pointed at from the philosophy's corrected litmus, threat model §7, guide 04
  (§ *Discovery is not an electorate*), the FAQ, `what-is-proven.md` and the wiki; three engineering-deck sentences
  qualified to a fixed electorate. 2.30.0's *Not claimed* line stands. No code change.
- **Documentation: the post-360 hardening plan, doc-coverage run 23 and the wiki lint** (#593, #589, #588; 2026-10-10).
  `docs/plans/post-360-hardening.md` records the batch after the 360 review — R0 (this release), Φ, the external
  review's P1/P2/P3/P5 and rows A–J — and decision D1, *consensus is a Layer III protocol, never a service*. Doc-coverage
  run 23 lands stopping a node for operators (`deployment.md` § Stopping a node, building-on §4) and corrects four
  pages the code contradicted; the documented `MyceliumConsensusStalled` rule (`diagnostics.md`) no longer pages on
  `reason="contended"` — **an operator who copied the rule** should re-copy it. The wiki lint corrects lock-order row
  17 and adds four governor keys to the KV table. Since this release, `diagnostics.md` also names the two new timeout
  reasons, `unrecorded` and `ballot_exhausted`.
- **`#![deny(unsafe_code)]` in `mycelium-core` and every companion library** (post-360 P3). The root crate
  and `mycelium-tuple-space` had it; the substrate did not, and it held the workspace's one production `unsafe`:
  `SubjectKeyRegistry::destroy`'s hand-rolled `write_volatile` wipe of a DEK (`erasure.rs`). That is now
  `zeroize::Zeroize` (already in the tree under `tls` through `ed25519-dalek` and `rustls`; a direct optional
  dependency of core under `tls`, one lockfile line), with `destroy_wipes_the_key_it_removes` pinning that the key
  `destroy` removes is cleared. `install_key` over an existing subject now wipes the key it replaces (it was
  dropped un-wiped; seen failing first: `install_key_wipes_the_key_it_replaces`, `left: []`). The copies no wipe
  reaches — `decrypt_for`'s and `encrypt_for`'s stack copies, ring's key schedule, the registry on drop — are
  listed in the module doc and `docs/operations/data-erasure.md`, with `Zeroizing<[u8; 32]>` storage as the path
  to closing them. What remains is test code, each under one scoped `#[allow(unsafe_code)]` with its `SAFETY:` comment:
  core's env-var mutation in one helper (`config.rs` `set_test_env`, replacing four allows; it takes the
  `env_test_lock()` guard as a parameter) and one `set_var` in a wasm-host test (edition 2024 makes both unsafe;
  the comments name the residual hazard, a libc `getenv` on another test thread). `examples/conway.rs` still uses
  `unsafe` and is outside the library crates the attribute covers. Added to `mycelium-sim`, `-blackboard`, `-wiki`, `-wasm-host`
  (its `bindgen!` output compiles under it), `-agentfacts`, `-reason`, `-guardrails`, `-effects`, `-commitment`
  and the coop examples library; not to `loom-spike` or `mycelium-gateway-free-tests`, whose libraries are
  empty. Seen failing first: the attribute alone failed core's build, `error: usage of an \`unsafe\` block
  --> mycelium-core/src/erasure.rs:118:21`. No behaviour change, no API change.
- **The composed-effect record: what it can carry versus what is written.** `AeEvidence` *can* carry the receipt's
  rung and the origin domain (`with_effect_durability`, `with_origin_domain`, 2.11.0) and `states_a_composed_effect()`
  reads them; **no shipped path calls either builder** — the gateway's evidence path writes neither field, so on every
  record it produces both are `None` and the predicate is false (the only callers are the module's own tests). The
  fields are available to an enforcement point that has the receipt and the credential in hand; 2.11.0's *now carries*
  described the record's shape, not a writer. `docs/design/composed-effect.md` §9 and `what-is-proven.md` now say so.
  No code change (the 360 review, 2026-10-09).
- **Documentation: the records a 360 review found contradicting the code, corrected** (2026-10-09; no code change
  beyond doc-comments). Guide 10 told operators to grant `kv:*` / `mesh:*` / `wiki:*` / `tuple:*`, which
  `validate()` has refused by name since 2.16.0 — the scopes are now listed by name; guide 12 showed a
  `schema-seed --fail-on-conflict` binary that does not exist — a labelled sketch against `seed_schemas_from_dir`
  replaces it; the contracts-axis plan's §10.2 and `what-is-proven.md` gave the two-writes window as the reason
  `require_identity_proofs` is default-off — both now cite the live residual (trust on first sighting, closed by
  anchors), as `dev/security.md` already did; CLAUDE.md's sim-seam and lock-order invariants state their scope (the
  three scanned crates; the library crates, not `examples/`), with the two off-seam wasm-host loops named on the
  proof page; `philosophy.md`'s forwarding row reads *regardless of admission*, and `runtime-invariants.md` names the
  removed-member drop (C5) as the second, origin-keyed carve-out; the TLS-without-CA-key row moves to the proof
  page's CI table; README and `positioning.md` say the default configuration is a development posture. Smaller
  drift: the serve window's closing release in `http.rs` (2.18.2, not 2.19.0); `deprecations.md`'s summary table
  extended to entry 22; `converge.py`'s path; `cluster_propose` as the canonical verb in the crate docs; the
  `test-overlay` help comment on its own target; the deck's rolling-upgrade note restated per 2.30.0.

### Added
- **`mycelium::OwnershipLock`** — the core's exclusive-ownership lock on `<file>.lock` (held by the KV WAL and the
  node-local journals), re-exported so a companion's own log refuses a second owner the same way. The tuple space
  and the blackboard take it on their WALs (row C, above). Additive.
- **A consensus electorate is a governed group, pinned by identity and fixed at genesis** (post-360 plan row P2;
  `docs/design/consensus-electorate.md` §8, revised through five rounds of #601's adversarial review; **the
  maintainer's decisions, 2026-10-11: freeze membership, and ship P2 in 2.32.0 without migration**). An **electorate
  group**'s electorate is an `ElectorateDecl { group, epoch, members, exclusive_default, supersedes }` decided by
  consensus on `electorate/{group}/{epoch}`: a **genesis** names the roster and is decided by every member it names,
  each checking it against the roster it sees. Each commit carries a certificate — the decided envelope and the
  deciding votes (`consensus/electorate-cert/`, signed under `[tls]`) — and a node adopts a record only when it
  verifies: one written straight into the store is not adopted and is counted (`mycelium::electorate_records_refused`).
  `GossipAgent::declare_electorate(group, exclusive_default)` / `POST /gateway/govern/electorate` (`govern:write`,
  audited only when something is decided) decides a genesis, and on a group with an electorate returns it unchanged or
  refuses `ElectorateError::MembershipFrozen` (`409 membership_frozen`). **Changing an electorate's members is not built
  in this release**: a dead member lowers the group's fault tolerance, a lost majority stops it; a migration to another
  group is the follow-up (branch `fix/p2-migration`). A genesis whose members no longer all answer is **superseded** by a
  declaration of the live roster from a member that holds it — the next epoch, keeping a strict majority of the pending
  genesis (`mycelium::Superseded`; §8.4 argues why at most one attempt is ever in force, and states what adoption rests
  on after a restart). `declare_electorate` retries an undecided attempt with a bounded backoff (eight tries, 250 ms
  doubling to 4 s); a precondition no retry changes — a door refusal, a second default — is `ElectorateError::Refused`
  (`409 refused`), returned at once. A proposal to an electorate group is refused `ElectorateMismatch` unless the roster
  equals the members **by identity** (counted: `mycelium::electorate_roster_mismatches`); promises and votes count only
  from the members; the quorum is a strict majority whatever `quorum_size` or opacity say; and every Prepare/Propose
  names the electorate's epoch and digest (`ConsensusMsg::PrepareIn`/`ProposeIn`/`StaleElectorate`, appended — wire v12;
  row A's envelope messages naming an electorate): an acceptor answers only its own electorate. Every genesis claim and
  adoption on the group passes one `compute` on its gate key (`claim_gated`), which only decides — the claim runs after
  it, and a refused claim raises nothing. The governor and the emergent watcher leave an electorate group alone;
  `/gateway/mesh/group` refuses it `403 governed_group`. **`consensus_require_electorate`** (env
  `GOSSIP_CONSENSUS_REQUIRE_ELECTORATE`, default off) refuses a **safety-sensitive** proposal —
  `ConsensusConfig::safety_sensitive`, or `lock/`, `leader/`, `consistent/`, `capauthz/`, `electorate/`, a commitment
  award — outside an electorate group, **`ConsensusResult::ElectorateNotGoverned`** (`403 electorate_not_governed`);
  off, it runs as before, counted. The cluster-scoped exclusive verbs — `distributed_lock`, `LockService`,
  `consistent_set`, the lock, consistent and log-claim routes, `mycelium-commitment`'s linearizable award,
  `set_capability_authz_via_consensus` — decide in the group marked `exclusive_default` (a fleet record), via the new
  `ConsensusHandle::exclusive_propose[_receipt]`. **The default is set once, at genesis**: a second `exclusive_default`
  genesis is refused by name while one is held, at the door and at every acceptor, and the default cannot move in this
  release. `consensus_electorate` only restates the default, and a disagreeing setting is refused; while the default
  cannot be named, every acceptor refuses a cluster-scoped exclusive slot, tagged or not. A fleet-exclusive slot is
  decided only in the fleet's electorate, and a slot that names a group (`leader/{g}`, `electorate/{g}/…`) only under
  exactly `Group(g)` — at the door and at every acceptor, whatever the default state or `consensus_require_electorate`,
  never across the cluster or by a cross-group proposal; a tagged proposal is answered only under a group's scope. `cons.safety_profile` (rev 2) is node-enforced and **`secure-single-domain` rev 4** requires it
  (rev 3 is P1's `gw.exposed_closed`). **Not claimed:** changing an electorate's members, or moving the default (the
  follow-up); a lost majority is unrecoverable; two default declarations made at once within gossip lag stop the
  exclusive verbs with no escape in this release (group-scoped work outside the exclusive families keeps running); a
  `safety_sensitive` slot outside the pinned families can also be decided by an untagged cluster-scoped proposal; genesis assumes its members agree on the roster; without `[tls]` the
  certificates are unauthenticated; the marking of the fleet's default is not itself a cluster-scoped decision (§8.6
  says why the profile closes that window instead); live reconfiguration is a later plan. **Removed before release:**
  rounds 2–3 built live one-member steps with a drain before each step, withdrawn in round 4; rounds 4–5 built a
  migration (retirement, a successor carrying lazily), moved to `fix/p2-migration` by the 2.32.0 decision. Seen failing
  first in round 5, each on a toggle of its mechanism: `a_tagged_cluster_proposal_for_an_exclusive_slot_fails_closed`
  (the `named.is_none()` condition restored: `a cluster-scoped lock/x is refused, named Some((1, …))`) and
  `a_second_default_is_refused_by_name` (the default checks removed: `a second default is refused by name:
  Ok(ElectorateDecl { group: "new", …, exclusive_default: true, … })`). Seen failing first in round 4, against
  546a5cbd: `a_refused_claim_through_the_gate_lowers_the_fence_again` (`a refused claim left a fence behind; left: None,
  right: Some(7)` — the test now pins the genesis gate, the one raising kind that ships),
  `an_undecided_genesis_is_superseded_by_one_naming_the_live_roster` (`NotDecided("Timeout { slot:
  \"electorate/council/1\", … votes_last_ballot: 1, quorum_required: 2 }")`) and
  `a_declaration_on_a_sparse_mesh_retries_until_decided` (`NotDecided("Timeout { slot: \"electorate/council/1\",
  ballots_tried: 3, votes_last_ballot: 0, quorum_required: 3 }")`). Seen failing first in earlier rounds, each on a
  toggle of the mechanism it pins: `a_swap_without_a_migration_is_refused` (count rule: `Committed` on {A,B,D}),
  `votes_from_outside_the_electorate_are_not_counted` (filter unwired, and separately the quorum floor removed:
  `Committed` on forged votes), `a_cross_group_proposal_does_not_decide_for_an_electorate_group`,
  `a_cross_group_proposal_does_not_decide_an_exclusive_slot` (`Committed { slot: "lock/x" }`),
  `a_forged_electorate_record_is_not_adopted` and `an_electorate_certificate_verifies_under_tls` (certificate unchecked,
  or votes counted without their signer: the forged record adopted),
  `a_local_electorate_setting_that_disagrees_with_the_fleet_is_refused` (the lock decided locally); and against the
  unfixed code `a_safety_sensitive_proposal_is_decided_only_by_an_electorate_group`, `the_governor_and_the_watcher_
  leave_an_electorate_group_alone`, `an_exclusive_outcome_at_the_gateway_needs_an_electorate_group` and the guarantee
  and profile pins. **Upgrade notes:** `ConsensusResult` (`#[non_exhaustive]`), `CommitError` and `ConsistencyError`
  gain `ElectorateNotGoverned`, `ElectorateStale` and `ElectorateMismatch` — a `_` arm must fail closed;
  `ConsensusConfig` and `GossipConfig` gained fields (an exhaustive literal breaks); a node under `profile =
  "secure-single-domain"` must set `consensus_require_electorate = true` or it will not start, and then a lock or an
  election on the whole cluster or an undeclared group is refused — declare the electorates and mark one
  `exclusive_default`; a `mycelium-commitment` award and a capability-authz policy now decide where the locks do; **an
  electorate's members cannot be changed in this release** — size each for its lifetime's failures; **rolling upgrade:**
  an electorate whose members are not all on this release times out rather than commits (the new messages are ignored
  by older nodes), and a pre-P2 node still proposes the exclusive verbs to the whole cluster — mark an
  `exclusive_default` group only once every node runs this release; **a group-scoped proposal for a `lock/`,
  `consistent/`, `capauthz/` or commitment-award slot is refused on any group but the fleet's exclusive electorate, and
  `leader/{g}` on any group but `g`** (`ElectorateMismatch`).
- **The KV namespace table is a gate now, not a lint item.** `scripts/check-kv-namespaces.sh` enumerates every
  KV prefix production code uses — slash-bearing `const`/`static` `&str` literals whatever their name, `format!`
  key heads, and the first literal of a KV call (`set*`, `get`, `delete*`, `scan_*`, `subscribe*`, `kv_*`,
  `make_gossip_update*`, `publish_*`, `strip_prefix`/`starts_with`; the set is named in the script's header) in
  every library crate — and fails on one whose namespace (top segment; under `sys/`, the second too) has no row in
  `src/lib.rs` § KV namespace ownership, unless `scripts/kv-namespaces-nonkeys.txt` declares it a non-key with a
  reason (36 entries today: seam and journal streams, slot and ring names, schema tags, knowledge subjects, an
  operation id, a prompt id, git paths, relative segments). A literal is matched against the table's **row patterns**
  (`consensus/decided/`, `sys/govern/timing`), not its top segment, so a new key under a namespace with several rows
  needs its own row; entries of constant `&str` arrays and slices are enumerated; and the test-item skip covers
  exactly the item a `#[cfg(test)]` attribute applies to (the adversarial review of #591: the column-0-`}` skip hid
  `mycelium-reason/src/route.rs:372-573`). A stale or reasonless entry fails too, and `--self-test` (in `make check` and CI) plants rowless
  prefixes in a scratch copy and requires the gate to name each. The wiki lint had missed live prefixes five
  times; the table gains the four it missed last (`sys/config/{param}`, `sys/govern/timing`, `sys/govern/fleet`,
  `sys/govern/membership/{group}`, as PR #588 adds them). Seen failing first: the gate on the unfixed table names
  `sys/config/` (`cluster_tuner.rs:26`) and `sys/govern/` (`membership_governor.rs:32`, `timing_governor.rs:38`,
  `tuning_governor.rs:31`). **Not seen:** a prefix assembled from pieces, or a literal two lines below its call.
- **A companion's gateway door can act as the HTTP client, under the evaluator** — the public API
  `mycelium-reason`'s façade needed and nothing had: `ResolvedPrincipal` (the auth layer's resolved principal, read
  from axum's `Extension` on a route merged with `with_http_routes`; fields crate-private, so it is never
  constructed outside the auth layer), `ServiceHandle::rpc_call_as` (the gateway dispatch, carrying the client's
  caller context), `GossipAgent::gateway_preflight` / `gateway_record_execution` (the preflight `/mcp`, `/a2a` and
  `/gateway/llm/call` run, opaque `GatewayPreflight` / `GatewayRefusal`, inert without `tls`) and
  `GatewayDispatchError` (`#[non_exhaustive]`). All under `gateway`; additive.
- **Tripwires on two consensus records a peer should not write.** The `sys/` namespace tripwire
  (`sys_namespace_violations`) now also covers `sys/consensus-accepted/{node}/…` — the acceptor's durable promise and
  acceptance, which only that node's `persist_acceptor` writes and `prewarm_accepted` restores, so a peer writing
  ours is forging a promise we never made — and the identity record's other two forms, `sys/identity-signed/{node}`
  and `sys/identity-proof/{node}`, written only by the node itself. **And it counts only a write that changed
  what this node holds and that this node did not write** — a gossip frame from another originator, or an
  anti-entropy entry that replaced a value it held with a different one — for all eight prefixes: anti-entropy
  handing a node its own records back after a restart used to count each one (the adversarial review of #591
  measured 62 over 30 commits; the pre-existing prefixes counted 1). Seen failing first:
  `anti_entropy_repair_after_a_restart_is_not_a_namespace_violation` (C counted 5) and
  `connection::tests::anti_entropy_repair_of_my_own_record_is_not_counted`; and, for the prefixes:
  `test_sys_namespace_tripwire_flags_foreign_self_owned_write` (B did not flag A's write to
  `sys/consensus-accepted/{B}/slot-x`) and `connection::tests::flags_remote_write_to_each_self_owned_prefix`.
  And `consensus/decided/{slot}`, which any committer writes and so is not self-owned, gets a Layer III tripwire:
  a floor — or a ballot key, `consensus/ballot/{slot}`, read for a draw — more than `2^32` above every ballot this
  node has itself observed for the slot (its own promise, and the ballots of verified COMMITs it processed; not the
  shared ballot key, which any member writes — the adversarial review of #591 showed forging both keys passed
  uncounted, and the ballot key alone exhausted the slot with no tripwire) — ballots are drawn one attempt at a
  time, so no history of a slot gets there, while a member writing `u64::MAX` makes every later prepare on the slot
  refuse — is counted once per slot, at most 4096 slots remembered
  (`SystemStats::consensus_decided_floor_anomalies`, `/stats` `consensus_decided_floor_anomalies`,
  `mycelium_consensus_decided_floor_anomalies_total`) with one `warn!`. Detection only: the floor is still obeyed
  and the refusal is unchanged. Seen failing first: `a_forged_decided_floor_is_counted_and_still_obeyed` (the
  count stayed 0) and `forged_ballot_keys_are_counted_as_well_as_forged_floors` (the both-keys forgery: 0, expected
  1). **Upgrade note:** `SystemStats` gained two fields (an exhaustive struct literal breaks).
- **A slot whose decided floor is `u64::MAX` no longer overflows the proposer.** Both proposers (group/cluster
  and cross-group) drew every ballot as `max(ballot key, floor) + 1` — eight sites — which overflows at the
  ceiling: a panic in any build with overflow checks, and the release profile is `panic = "abort"`, so such a
  build lost the node; without overflow checks a wrap to ballot 0, refused below the floor. Every draw now goes
  through `next_ballot` (`checked_add`), and a slot with no ballot left ends the proposal as a `Timeout` named
  `ballot_exhausted` on `mycelium_consensus_timeouts_total`, counted per node on `SystemStats` and `/stats`
  `consensus_ballot_space_exhausted`, with one `warn!`. A floor one below the ceiling still commits at
  `u64::MAX`. The acceptor side only compares ballots and had no arithmetic to fix. Seen failing first:
  `a_floor_at_the_ballot_ceiling_is_refused_by_name_not_overflowed` (`attempt to add with overflow` at
  `src/consensus.rs:1146`).

### Fixed
- **A hosted WASM guest call is bounded in time, runs off the async workers, and a trap no longer recompiles**
  (`mycelium-wasm-host`, plan row D). Before: the wasmtime engine had no epoch interruption, so a stem that
  declared no fuel ran every guest unmetered, and the serve loop called the guest *on its async task* — one
  looping call pinned a runtime worker for ever (on a current-thread runtime, the whole node); and a trapped
  instance was replaced by **recompiling the component from its bytes**, so a payload that made a guest trap
  bought a full Cranelift compile per request. Now every engine has epoch interruption and every call — and
  every instantiation — gets a wall-clock deadline, `DEFAULT_CALL_DEADLINE` (5 s), metered or not and whoever
  signed the entry (`WasmHost::with_call_deadline`, `[hosts].call_deadline_ms`; `0` refused by name), ending
  in `WasmHostError::DeadlineExceeded { deadline_ms }` and `InvocationOutcome::DeadlineExceeded` (counter
  outcome `deadline_exceeded`) — start-up past it too; `Duration::ZERO` stops the guest at its first check and a
  deadline too large to count in 10 ms ticks is no deadline (it was truncated: `Duration::MAX` meant one tick). An
  uninstalled install's in-flight call that then traps does not re-instantiate. The serve loop runs each call, and install runs compile, instantiation and
  `describe`, on `spawn_blocking`; and a trapped instance is replaced from the install's compiled component
  (`WasmHost::compiles()`). The epoch is advanced by one thread per host every 10 ms — wall time, outside the
  replay seams, recorded in the nondeterminism inventory. Seen failing first:
  `a_guest_call_past_its_deadline_is_stopped_by_name` (`the call returned within its bound: Timeout`),
  `a_long_guest_call_does_not_block_another_task_on_a_current_thread_runtime` (`the heartbeat stalled for
  564.882459ms during a 565.106292ms guest call`), `a_trapping_guest_called_repeatedly_compiles_once`
  (`left: 4, right: 1`). **Upgrade notes:** `WasmHostError` and `InvocationOutcome` each gain a variant
  (neither is `#[non_exhaustive]`, so an exhaustive `match` needs an arm); `HostsDecl` gains
  `call_deadline_ms` (an exhaustive struct literal breaks; `..Default::default()` is unaffected); a guest call
  that legitimately runs longer than 5 s is now stopped — raise `call_deadline_ms` or call
  `with_call_deadline(None)`; `Instance::invoke` is still synchronous, so an embedder that calls it from
  async code should move it to `spawn_blocking` as the serve loop now does — the co-op `catalog`,
  `catalog_viz` and `mcp_toolgrowth` demos now do, instantiation included.
- **Core resource bounds: what one socket, peer or kind can hold** (post-360 hardening row B, 2026-10-10; narrowed
  by the maintainer's decision 2026-10-11 after five review rounds on #602). Each bound with a test seen failing on
  the code before it:
  - **The accept path had no deadline.** A `max_connections` permit was taken before the TLS handshake and the
    read loop waited on the first frame forever, so `max_connections` sockets that connect and say nothing closed
    the node to every peer. The TLS accept, then the first frame's first byte, must each arrive within
    `handshake_timeout_ms` (default 10 s), and an established connection silent for `inbound_idle_timeout_secs`
    (default 300 s, must exceed `writer_idle_timeout_secs`; `0` = never) is closed. Seen failing first:
    `idle_sockets_that_never_speak_are_closed_and_a_new_peer_connects` — *a socket that never spoke must be
    closed after handshake_timeout_ms; got Err(Elapsed(()))*.
  - **Progress, not frames.** A frame being received must move a byte every `peer_read_stall_timeout_ms` (60 s)
    and keep **1 KiB/s** after it (`peer_min_rate_bytes_per_sec`, reads only, per connection — a joiner needs
    senders × floor of link); a frame being sent, a byte every `peer_write_stall_timeout_ms` (1 140 s).
    The write bound must outlast a `flush` receiver applying a chunk entry by entry, one fsync each, while it does
    not read — measured **760 s** for the worst chunk a sender can build (~145 000 tombstones; 2.5 s under `async`; 2026-10-11, debug build, `worst_case_chunk_apply_with_an_fsync_wal`) — the default is 1.5 × that. A whole-frame bound, the first version, cut a 10 MB frame on a slow
    but healthy link on every attempt. Mid-frame stalls are counted: `inbound_frames_stalled`, `outbound_stalls`.
    A header no longer reserves the 10 MB it claims; the buffer grows as bytes arrive and is given back after a
    large frame. Seen failing first: `a_large_first_frame_trickling_steadily_is_read_past_the_handshake_bound` —
    *a frame making steady progress must be read even when it takes longer than handshake_timeout_ms*;
    `a_slow_reader_that_keeps_reading_receives_a_large_frame` — *left: 939745, right: 4194309*;
    `a_header_claiming_a_large_frame_does_not_reserve_it_upfront` — *reserved 10485759 bytes for 15 received*.
  - **The outbound writer could be parked by a peer that accepts and never reads.** Connect + TLS are bounded by
    `handshake_timeout_ms`, the writes and the flush by the progress bound, and shutdown and eviction now
    interrupt a blocked write. Seen failing first: `a_peer_that_never_reads_does_not_block_eviction` /
    `…_shutdown` — *an evicted writer blocked in a write to a peer that never reads must still exit*.
  - **Anti-entropy replies piled up behind a stalled peer**, one whole-store dump per reconnect. A peer now has
    at most one reply in flight; a request while it is answered is skipped and counted. Seen failing first:
    `a_peer_that_never_reads_gets_one_anti_entropy_reply_at_a_time` — *skipped = 0*.
  - **One stalled subscriber vetoed its kind node-wide.** Admission rolled against the *most* full subscriber;
    it now rolls against the *least* full subscriber that bears work — SSE routes register as taps and never
    count, so a fast observer cannot hide a saturated worker either. A full subscriber drops its own copies,
    counted. Seen failing first: `a_stalled_subscriber_does_not_stop_admission_for_another` — *the reading
    subscriber got 228 of 600*; `a_fast_tap_does_not_hide_a_saturated_worker`.
  - **The signal log grew with every sender-chosen kind.** At most 4096 kinds and 1024 senders per kind, the
    least recently seen evicted — never a kind with a local worker or one queried within the window (pins lapse);
    eviction, trim and recording act on a kind under its log lock. Seen failing first:
    `the_signal_log_stays_bounded_under_random_kinds` — *12288 kinds tracked after 12288 random kinds*;
    `a_kind_first_seen_after_a_flood_is_still_tracked`; `a_tap_does_not_exempt_its_kind_from_eviction`;
    `a_query_pin_expires`; `eviction_scans_are_amortised_when_most_kinds_are_exempt`.
  - **`sys/quorum/` evidence** — the signal log's twin, written to Layer I — is written only for a kind with a
    local worker or a live query pin, for at most 1024 senders per kind at a time (a slot frees once its last
    write is older than the signal window) and 4096 writes a second; past either the write is skipped and
    counted. That bounds a **rate** — about 1024 new keys per kind per signal window — not a total: the keys
    are not collected. Seen failing first:
    `a_random_kind_flood_writes_no_evidence`; `one_kinds_evidence_is_bounded_by_the_sender_cap` — *3072 evidence
    keys for one kind*; `a_sender_slot_frees_once_its_evidence_ages_out` — *past the cap a new sender is skipped*.
  - **A stopped agent leaked its whole context.** `RpcRequestRx::recv` now ends at shutdown, as its doc said; the
    A2A cleanup ticker stops too. Seen failing first: `a_stopped_agent_frees_its_task_context` — *rpc_rx must
    return None once the agent has shut down: Elapsed(())*.
  - `check-sim-seams.sh` reads every timer-alias spelling from whole `use` statements, with planted-file checks
    (`scripts/test-check-sim-seams.sh`, in `make check` and CI).

  **The residual, plainly:** the accept path is bounded against silent and slow connections; **an attacker that
  keeps talking can hold the connection slots it obtained from free permits**, up to `max_connections`. The
  mitigations are mTLS — a non-member fails the handshake within `handshake_timeout_ms` — and the opt-in
  `max_connections_per_source` (default off; exact IPs, so an IPv6 /64 is not covered; refusals counted in
  `inbound_per_source_refused`). On the write side there is no rate floor, deliberately (a sender cannot tell a
  slow link from an applying receiver): a peer that reads one byte per `peer_write_stall_timeout_ms` window parks
  its own writer and its reply — the whole divergent dump queued for it — indefinitely, so up to `max_peers` ×
  the divergent store can be held; on a plaintext mesh a forged `Ping` introduces a peer. Connection preemption and
  WAL group commit were built during review and removed (rounds 3–5). The inbound `Data` and anti-entropy paths
  apply a record before its WAL append, as before (stated, unchanged). Counters, on `SystemStats` and as metrics:
  `inbound_connections_timed_out`, `inbound_frames_stalled`, `inbound_per_source_refused`, `outbound_stalls`,
  `anti_entropy_replies_skipped`,
  `signal_handler_drops`, `signal_log_kinds_evicted`, `signal_log_kinds_refused`.

  **Upgrade notes:**
  - `GossipConfig` gained `handshake_timeout_ms`, `inbound_idle_timeout_secs`, `peer_read_stall_timeout_ms`,
    `peer_write_stall_timeout_ms`, `peer_min_rate_bytes_per_sec` and `max_connections_per_source`, and
    `SystemStats` eight fields, and `CoreCtx` gained the public field `transport_bounds` (an exhaustive struct
    literal of any of them breaks; `..Default::default()` is unaffected).
  - `validate()` refuses `inbound_idle_timeout_secs` at or below a non-zero `writer_idle_timeout_secs`; a fleet
    whose writers never idle-close (`writer_idle_timeout_secs = 0`) should set `inbound_idle_timeout_secs = 0`.
  - **The read floor defaults to 1 KiB/s** after `peer_read_stall_timeout_ms`: a joiner bootstrapping from many
    peers over a thin shared link needs senders × floor — set `peer_min_rate_bytes_per_sec = 0` there.
  - **The write stall bound defaults to 1 140 s**: a peer that accepts and never reads parks only its
    own writer and its one anti-entropy reply (the whole divergent dump) that long; lower it for a fleet with
    no `flush` nodes.
  - **`sys/quorum/` evidence is written only by nodes with a worker on the kind or a recent query of it**
    (`deprecations.md` §30): an observer outside a group reading `quorum_persistent` sees evidence only from nodes
    that subscribe to or ask about the kind, itself included from its first query; a poller slower than its
    window sees gaps.
  - `SignalHandlers::fill_ratio` is the least-full work-bearing subscriber, so opacity hints and local emit
    shedding read the same.
  - `mycelium_core::writer::{run_peer_writer, get_or_spawn_writer, request_state}` take a `WriterTiming` (by
    reference for the last two; no longer `Copy`) where they took the idle `Duration`;
    `ConnContext::writer_idle_timeout` is `writer_timing`; `mycelium_core::stall::StallGuard` wraps the gossip
    stream on both sides; the `bytes` requirement is `1.9` (`Bytes::from_owner`).
- **One record per decision: a lock, a lease and a leadership end where every node can see it; `elect_leader` is
  leased; acceptor state decays** (row A of `post-360-hardening.md`, with C1 and C2; design record
  `docs/design/lock-lifecycle.md`, adopted after three review rounds and a design review on #600). A decision lived
  in three keys merged by last-writer-wins on HLC — `committed`, `lease`, `decided` — so a node could hold any
  combination of their versions, and every reading rule over the pair (value, window) had one it read wrong: a
  released lock re-committed to its old holder once its tombstones were collected (K1), a reopened lease adopting
  the expired value before the floor arrived (K2), a late COMMIT reviving a released lock fleet-wide (K3), and, in
  the rounds' own fixes, a newer holder ended by an older window, a lock wedged for ever by adoption under clock
  skew, and two leaders from a release that named a ballot it read from `decided`. Now **what Paxos decides is a
  decision envelope** — the value with its window (permanent, or a lease ending at an `expires_at_ms` fixed on the
  original proposer's wall clock at the first proposal), lineage, original proposer and fencing token
  (`src/consensus_life.rs`) — so every acceptance and adoption carries the lifecycle, and an adopter re-commits the
  holder's window, never its own. It is written under `consensus/life/{escaped slot}/{ballot}`, the same bytes from
  every writer, read by the **highest ballot**; it ends by a release marker (`…/end/{lineage}-{proposer}`, a key no
  record write can remove) or when its lease is past on the reader's **wall clock**. The committer writes the
  record, fsynced, before the COMMIT; the COMMIT (`CommitTerm`, an appended variant) carries the whole envelope,
  and an upgraded learner ignores the legacy `Commit`. `consensus/decided/` is a refusal floor only. **C1:**
  `elect_leader` / `elect_leader_receipt` lease 30 s by default (`DEFAULT_LEADER_LEASE` — the anti-entropy
  interval, and the lock and log-claim default); calling again renews, keeping the lineage; `release_leadership`
  (async, durable) ends the lineage, a renewal in flight included; `elect_leader_with(group,
  LeaderTerm::Permanent)` is the opt-in; `POST /gateway/overlay/elect` takes `ttl_secs` / `permanent` and
  `DELETE /gateway/overlay/elect/{group}` steps down (`200`, `404 not_leader`, `500 release_unrecorded`;
  `consensus:write` is node-wide there). `LockGuard` gains `expires_at_ms()`, `deadline()` and `is_expired()`, and is
  never issued past its deadline. **C2:** a collector task beside each listener shrinks the acceptor state of a slot
  whose decision ended at `e` — promising nothing above it, re-checked in the compare-and-set — to a node-owned,
  fsynced floor `{e}` that refuses `e` and below, after fsyncing the record it relies on; lower records are
  tombstoned (a per-slot sentinel keeps the slot off the legacy reading), lease markers go after their expiry plus
  the drift bound, permanent markers never — steady state the sentinel, the top and its marker (plus a large
  value's `v/` key and one marker per released permanent lineage); 64 slots a pass, sorted then
  seam-rotated. **The implementation review's findings, fixed here:** a renewal keeps its own envelope only when the
  highest reported acceptance is its lineage (D1: it adopted a lower value over its own higher decided acceptance;
  seen failing first: `d1_a_renewal_never_adopts_a_lower_value_over_its_own` — "basis: Observed"); an upgraded
  proposer never sets aside by `decided` (D2, `d2_an_upgraded_proposer_never_sets_aside_by_decided` — "a second
  holder beside B: Ok(LockGuard …)"); a slot's records are read through a per-slot scope index in Layer I's index
  step (D3: 136.7 ms per read → 1.35 ms at 5,000 slots × 100 records, debug build); a lock's lease is its TTL rounded
  up (D4, `d4_a_guard_never_outlives_its_lease` — "the lease ends 1000 ms after the start, before the 1.9 s
  deadline"); a `CommitTerm` above a live decision of another identity is written and counted (D5,
  `d5_a_commit_ahead_of_its_release_marker_is_written` — "L still reads the released holder"); `GET
  /gateway/overlay/consistent/get` and `GET /consensus/{slot}` read through the decision record (D6,
  `d6_the_gateway_consistent_get_reads_the_decision` — the raw key's bytes); uppercase-hex record keys are not
  records (`d7_an_uppercase_ballot_is_not_a_record`), learner records reach the WAL, the deadline refusal answers
  409 `expired_before_grant`; 1,000 acquisitions of one lock leave at most three keys (`growth_a_thousand_acquisitions_leave_constant_keys`
  — "1001 keys remain"). The interleaving tests withhold real frames with a test-only frame filter on the receive
  path (`mycelium-core` feature `test-frame-filter`, enabled by `mycelium`'s `test-frame-filter` for lib test runs
  only — never by `test-support`, so examples and demo images do not compile it; anti-entropy entries carry the
  answering peer, so a partition withholds them too). **And the review of `ee3d9e4b`:** `decided` sets aside only
  acceptances reported raw (an older proposer's), and only when the legacy reading says that decision is over —
  never an envelope; an adopted legacy value is wrapped in a lease, never as permanent (finding 1: a 2.31 permanent
  leadership released the legacy way was re-elected permanently by the next upgraded proposal, and nobody could
  release it; seen failing first: `f1_a_legacy_released_permanent_is_not_resurrected` — "the released 2.31 leader
  was resurrected: Ok(Leadership { leader: … 127.0.0.1:1 …, basis: Observed })"); the renewal rule is pinned in
  both directions (`a_renewal_keeps_its_lineage_only_when_it_is_the_top_report`,
  `f2_a_renewal_adopts_a_higher_foreign_acceptance`) and "a learner never overwrites a tombstone" by
  `f7_a_learner_never_overwrites_a_tombstone` — each seen failing against its mutant; collection fsyncs the top,
  its marker and the sentinel before tombstoning below them; `GET /consensus/{slot}`'s `lease_expired` reads the
  record (a live top whose large value has not arrived, or an `Unknown` slot, is not expired). The scope index is
  generic: `KvState::with_scoped_prefixes`, the agent registers `consensus/life/` before any key is applied or
  replayed, and core names no prefix. Wire **v12**: five appended `ConsensusMsg` variants (`PrepareTerm`, `PrepareAckTerm`,
  `ProposeTerm`, `PromiseTerm`, `CommitTerm`); the acceptor record gains tag `0x03`. **Seen failing first** on the
  branch before the record (black-box, `src/agent/lock_lifecycle_tests.rs`): `row17_a_skewed_holders_decision_does_not_wedge_the_lock`
  ("the lock is wedged for ever after B's window"), `row20_an_ended_decision_does_not_revive_on_a_higher_floor`
  ("an ended leadership read live again"), `row22_an_adoption_keeps_the_holders_lease` ("D acquired inside B's 60 s
  lease"), `row23_a_late_commit_does_not_displace_a_permanent_decision` ("a late COMMIT displaced the permanent
  decision"), `row26_hlc_drift_does_not_end_a_lease_early` ("HLC drift ended a 60 s lease early"),
  `row27_collection_keeps_a_node_owned_floor` ("a finished ballot was accepted after collection"); the earlier rounds'
  tests (K1–K3, round 1–2 rows) failed first on their own commits; the record-level rows are written against the new
  API (`src/agent/lock_lifecycle_api_tests.rs`). **Upgrade notes:** (1) **`elect_leader` is no longer permanent** —
  renew by calling again (every ~10 s) or ask for `LeaderTerm::Permanent`; a slot committed permanently before the
  upgrade has no decision record and is read, and released, the legacy way. (2) **Upgrade every node.** An upgraded
  proposer times out rather than commits until a quorum of its acceptors is upgraded (they ignore `ProposeTerm`), as
  with 2.30.0's prepare phase. Older nodes still receive the legacy keys and `Commit`, and still read them the old way
  — so for an older *reader* the K3 re-stamp and rounds 1–2's pairings remain until it is upgraded; an adoption of a
  value first proposed by an older node takes the adopter's window. The legacy keys stop being written at a later,
  governed MINOR. (3) `LockGuard`'s fields changed (private), `ConsensusListenerHandle` gained a private field,
  `TaskCtx` gained `acceptor_records`; a lock's token is now the decision's token (still monotonic across holders
  when the prior decision's record reached the new proposer). **Not built:** the SDKs expose no
  `release_leadership` verb (row G); a node that proposes without a listener does not collect; a permanent decision
  whose original proposer is gone for good cannot be released by anyone else (design §5).
- **The SDKs reach a gateway over TLS** (`mycelium-py` **0.2.9**, `mycelium-ts` **0.2.3**,
  `langgraph-checkpoint-mycelium` **0.3.2**). Every client built its base URL as `http://{host}:{port}` — eight
  Python handles, five TypeScript clients and the checkpointer; only `A2aClient` took a full URL — so a gateway
  serving HTTPS (`gateway_tls`, v2.3.0, or a TLS-terminating proxy) was unreachable from any SDK, and a bearer
  sent to a node off loopback travelled in cleartext. Each now takes `scheme="https"` / `{ scheme: "https" }`
  (default `http`, unchanged), built in one place per language (`mycelium.base_url`, `mycelium-ts` `baseUrl`,
  the checkpointer's mirror of the Python rule). Verification stays on with no off switch; Python and the
  checkpointer pin a private fleet CA with `ca_file=` (httpx `verify=`, read at construction, a missing file
  refused there), riding the pooled clients and the dedicated SSE/stream clients; the TypeScript SDK documents
  `NODE_EXTRA_CA_CERTS` rather than add `undici`. Seen failing first: `mycelium-py/tests/test_scheme.py`,
  `mycelium-ts/tests/scheme.test.ts`, `langgraph-checkpoint-mycelium/tests/test_connection.py` (the option
  was not a parameter). Guide: `docs/guide/10-language-bridges.md` § Reaching a gateway over TLS. The adversarial
  review of #583 then held both helpers to one rule: the scheme case-insensitive on both sides (the TypeScript helper
  compared strictly, so a JS caller's `"HTTPS"` threw) and `None`/a non-string the documented `ValueError` (it was an
  `AttributeError`); an empty `ca_file` is `None` (`ssl.create_default_context(cafile="")` loaded nothing silently
  where the docs promised a refusal); an IPv6 literal host is bracketed once (`"::1"` built `https://::1:8300`, which
  httpx rejects). Each seen failing first in the same three test files.
- **The LangGraph checkpointer sends a bearer** (`langgraph-checkpoint-mycelium` **0.3.2**). `MyceliumCheckpointSaver`
  opened both httpx clients with no headers and took no token, so a token-protected gateway answered 401 to every
  checkpoint — while the saver's own `"unauthorized"` reason and README sent the operator to the token. It now takes
  `token=`, resolved as the Python SDK resolves it (the argument, then `MYCELIUM_GATEWAY_TOKEN`; empty means none)
  and sent as `Authorization: Bearer` on the sync and async clients — header only, never in a URL, the `repr` or an
  error. Seen failing first: `langgraph-checkpoint-mycelium/tests/test_connection.py` (with only the environment
  variable set, the stub gateway saw `None` where `Bearer env-token` was expected). The review also corrected the
  saver's scope list: every `put()` writes blobs through `PUT /gateway/reason/blob`, which is `llm:write`, so the
  saver needs `kv:read`, `kv:write`, `llm:read` **and `llm:write`** (the README named three).
- **The SDKs' edges with the gateway** (`mycelium-py` **0.2.10**, `mycelium-ts` **0.2.4**,
  `langgraph-checkpoint-mycelium` **0.3.3**; post-360 hardening row G). **Whole-second timeouts:** the gateway reads
  `timeout_secs` as an integer — `as_u64().unwrap_or(30|10)` on `rpc/call` and `scatter`, a `u64` field on
  `overlay/emit_reliable`, `wiki/ingest` and the tuple `take` routes — so the Python SDK's fractional
  `rpc_call`/`scatter_gather` timeout was silently replaced by the route's default while the client gave up at
  `t + 5`, `emit_reliable` and `Wiki.ingest` were refused 422, and `TupleSpace.take`/`take_by_key` sent
  `int(t)`, a zero-second park for `0 < t < 1`; `mycelium-ts`'s tuple `take`s and `Wiki.ingest` were refused 422
  for a fraction. Every one now rounds up (never below 1; `take` keeps `0`, its poll) — one rule per language,
  `mycelium._pool.whole_seconds` and `mycelium-ts` `wire.ts`. **Path segments escaped:** a prompt's
  `ns`/`name`, a signal/serve/mailbox kind, a shard name, a run or blob id, a federation domain, a capability
  handle or lock guard id was interpolated into the path raw in Python (and the prompt routes and handle/guard
  ids in TypeScript), so `/`, `?` or `#` in a value changed the route; the checkpointer took its blob id from a
  peer-writable row, and a row naming `../../kv/keys?prefix=` sent the saver's bearer to `/gateway/kv/keys`. Each
  is now percent-encoded as one segment, and `.`/`..` — which a URL parser resolves even encoded (WHATWG reads
  `%2e%2e` as `..`) — is refused before any request (`ValueError` / `Error`; the checkpointer reads such an id as
  `corrupt` and never fetches it). Enumerated with `grep -rn 'f"/gateway\|f"{self._base' mycelium-py/src
  langgraph-checkpoint-mycelium/src` and ``grep -rn '`/gateway[^`]*\${\|/gateway/.*\${' mycelium-ts/src``.
  **`SupersededError` in TypeScript:** a lost `consistentSet`/`crossGroupPropose`/`distributedLock`/`electLeader`
  (409 `{"error": "superseded"}`) threw a plain `Error`; it now throws `SupersededError` (`status` 409), as the
  Python SDK has since 0.2.8 — a 409 `topology_unsatisfied` and a 504 stay what they were. **SSE reconnect —
  not built: the gateway has no resume point** (no SSE route sends an event `id:` or reads `Last-Event-ID`);
  both READMEs and guide 10 now say what each stream loses when it drops and how to re-open it. Seen failing
  first: `mycelium-py/tests/test_sdk_edges.py` (`assert (<class 'float'> is int)`; `assert (<class 'int'> is int
  and 0 == 1)` for `take(0.3)`; `'/gateway/fed...n/catalog/a/b' == '/gateway/fed...catalog/a%2Fb'`),
  `langgraph-checkpoint-mycelium/tests/test_blob_path.py` (`b'/gateway/kv/keys?prefix=' !=
  b'/gateway/reason/blob/..%2F..%2Fkv%2Fkeys%3Fprefix%3D'`), `mycelium-ts/tests/sdk_edges.test.ts` (38 failed:
  the tuple and wiki timeouts, the prompt and handle/guard segments, every dot segment, `SupersededError`, the
  `scatterGather` default).
- **`mycelium-ts` 0.2.4: `scatterGather` has the gateway's defaults.** With no `minOk` it waited for **every**
  target and gave up after 5 s, where the gateway, the Python SDK and the route's docs default to **one** reply and
  10 s. It now sends `min_ok: 1` and 10 s; both READMEs document the default. **Upgrade note:** a TypeScript
  caller that relied on hearing from every target must pass `{ minOk: targets.length }` — with the new default
  the call returns at the first reply and the gateway cancels the rest. And `scatterGather([])` with no `minOk`
  now asks for one reply from no targets, so the gateway answers 504 at once (a `TimeoutError`) where it resolved `[]` — pass
  `{ minOk: 0 }` or skip the call for an empty target list. Seen failing first:
  `scatterGather waits for one reply by default` (`min_ok` was `2` for two targets).
- **The adversarial review of #595, on the same SDK versions.** `lease_secs` and `ttl_secs` get the same
  whole-second rounding in both SDKs — the gateway reads `lease_secs` with `as_u64()` (`capability/advertise`,
  `units/declare`: a fractional lease left the advertisement **unleased**) and `ttl_secs` as `Option<u64>`
  (`overlay/lock/acquire`: 422). Both helpers refuse a negative, non-finite or past-`u64` duration by name
  (`ValueError` / `Error` naming `timeout_secs`, `lease_secs`, `ttl_secs` or the TS option) instead of mapping it
  to a default; `take`'s `0` stays the poll. A path segment that is empty or holds a lone surrogate is refused
  like `.`/`..`, with the same documented error ("cannot travel as a URL path segment" — the surrogate raised
  `UnicodeEncodeError` / `URIError`); the checkpointer reads an empty, surrogate or **non-string** `blob` in a row
  as `corrupt` (`IncompleteCheckpoint`, not retriable) where a non-string raised `TypeError`. Docs: a mailbox
  stream is **at-most-once across a drop** — the gateway tombstones each event as it enters the stream's
  256-slot queue (`src/agent/mailbox.rs`), so queued events are lost when the stream drops (guide 10 and both
  READMEs said at-least-once); the whole-seconds sentence is scoped (`/gateway/kv/quorum` reads a float). Seen
  failing first: `test_a_timeout_the_gateway_cannot_read_is_refused_by_name` (`OverflowError: cannot convert
  float infinity to integer`, `DID NOT RAISE`), `test_a_lease_or_ttl_is_sent_in_whole_seconds` (`assert (<class
  'float'> is int)`), `test_a_segment_that_cannot_travel_is_refused_before_any_request` (`'utf-8' codec can't
  encode character '\ud800'`), `test_a_non_string_blob_in_a_row_reads_as_corrupt` (`TypeError:
  quote_from_bytes() expected bytes`), and 76 in `mycelium-ts/tests/sdk_edges.test.ts`. **Not claimed:** the
  gateway's mailbox ordering (tombstone before the client reads) and the prompt route's aliasing once `/` is
  encoded (`ns="a/b", name="c"` and `ns="a", name="b/c"` name one KV key) are gateway work, post-360 row E; the
  TypeScript `rpcCall`/`scatterGather`/`emitReliable` abort on the agent-wide timeout rather than `timeout + 5`,
  and `rpcCall` defaults to 5 s where Python and the gateway use 30 s — recorded as a follow-up.
- **The audit chain's head survives a restart.** `AuditChainState::new()` is genesis and nothing read the persisted
  `sys/audit/{self}/` stream back at `start()`, so a restarted node with `[persistence]` sealed seq 0 again with a
  zero `prev_hash` and LWW overwrote its own genesis record — the tamper-evident chain erased its history at every
  restart. `start()` now folds the node's own persisted stream (`audit::restore_chain_head`, after the identity is
  loaded and before anything can seal), **verified** from genesis or the newest signed checkpoint under the retained
  key set: the head resumes after the longest verified prefix, and a record that does not verify is logged at `error`
  and sealed over rather than seeding the chain. Without persistence the stream still restarts at seq 0 (nothing to
  recover from; peers' copies are not consulted). Seen failing first:
  `the_audit_chain_head_is_recovered_from_persistence_at_restart` (the restarted node's stream held three records and
  seq 0 was not the original), `a_forged_record_in_the_persisted_audit_stream_does_not_seed_the_chain_head`.
- **A failed WAL append poisons the writer until a snapshot repairs the file.** A short write followed by an error
  (`ENOSPC`, `EIO`) leaves a torn frame at the end of `wal.bin`; the writer forwarded the error to that one caller
  and carried on, so the next record landed behind the torn frame — the next restart read `Corrupt` with data after
  it (a refusal to start by default, or both files quarantined), and until then every snapshot aborted on the same
  `Corrupt`. Now every later append is answered `Err` (`LocalDurability::Failed` on the receipt path) or counted in
  `dropped_appends()` on every path, a `sync()` establishes nothing, the failure is logged once at
  `error`, and the writer tries a repairing snapshot at once and on every snapshot after — the torn frame is the last
  thing in the file, so a successful snapshot carries the complete records and truncates it — un-poisoning only when
  one succeeds. A refused append has no WAL record, but the store already holds its value (apply first, then the
  WAL), so the repairing snapshot's store scan carries it into `snapshot.bin`; what its caller was told is that
  durability was **not established** by the ack. `/health` carries a `persistence` block (`wal_refusing_appends`,
  `reason`, `dropped_appends`) and the counters `gossip_wal_append_failures_total` /
  `gossip_wal_appends_refused_total` say so too. The node-local journal has done the same since 2.23.0; this is the WAL's mirror. On-disk format
  unchanged. Seen failing first: `a_failed_append_poisons_the_writer_until_a_snapshot_truncates_the_torn_tail` (the
  append after the injected failure was acknowledged `Ok`).
- **A failed snapshot no longer latches the node self-opaque, and the timer keeps snapshotting.** `do_snapshot` raised
  `sys/load/{node}/persistence` at step 1 and lowered it only at step 5; every `?` between them returned past the
  lowering, the writer's timer branch defers while `is_self_opaque` reads that mark — which had no age check — and the
  three callers discarded the error unlogged. One failed snapshot therefore stopped every timer snapshot after it
  (the WAL grew until a restart) and the node abstained from every proposal while the fleet had stopped counting it
  within seconds. The mark is now lowered on every exit, a failed snapshot is logged at `warn` with its reason (a
  repeat of the same failure at `debug`, recovery at `info`), and `is_self_opaque` counts a mark only within the same
  freshness bound the consensus counters use (`opaque_freshness_ms`: two health-check intervals). Seen failing first:
  `a_failed_snapshot_lowers_the_self_opacity_mark_and_the_next_timer_snapshot_runs`,
  `a_stale_self_opacity_mark_does_not_read_as_opaque`. The bound ages **only the `persistence` mark**, read by the
  snapshot-deferral hook (`defer_snapshot_on_self_opacity`); a requirement's or the governor's mark is a transition,
  lowered by whoever raised it, and `is_self_opaque` reads those unaged as before. The bound reads the static
  `health_check_interval_secs`, not the hot-tuned value. The failing exit lowers the mark with `Hlc::tick_logical()`
  — one logical step, no wall-clock read — because a failing exit must consume no choice a recording did not make:
  the storage fault sweep (`a_fault_at_any_storage_effect_leaves_the_acknowledged_record_recoverable`) injects a
  fault into a clean recording, and the first cut's `tick()` there made the replay diverge at the lowering.
- **The `sim` build's storage seams report a failed effect when no kernel is installed.** `kernel_fs` answered
  `Ok(())` before looking at the real outcome, so under `--features sim` without a kernel a failed write, rename or
  sync — `EISDIR` on `snapshot.tmp`, a missing file — came back as success and a snapshot "succeeded" over it; the
  error's kind now survives (`NotFound` on a tail read stays an empty tail). Off in every shipped build; CI's
  `-p mycelium-core --features sim` step is where it showed. Seen failing first:
  `sim_seam::no_kernel_tests::without_a_kernel_a_failed_effect_is_still_a_failure` (the write to a directory returned
  `Ok`).
- **`persist.sync_mode` reads `not_configured` under `sync_mode = "os"`.** The writer syncs only when
  `force_sync || sync_mode == Flush`, so `os` is buffered exactly like `async` — but the guarantee resolved
  `enforced` for it, and `secure-single-domain` admitted a node whose acks were `buffered`. It now resolves
  `not_configured — sync_mode (os: the OS buffers; an ack is `buffered`)`, and the profile refuses the start by
  name; the catalogue golden is regenerated. **Check before upgrading:** a `secure-single-domain` node running
  `sync_mode = "os"` now fails at start — set `flush`. Seen failing first:
  `the_secure_profile_names_sync_mode_os_as_not_configured`.
- **A signature a node made for any other purpose could be presented as its identity proof.** The proof
  (`sys/identity-proof/`, and inside the sealed `sys/identity-signed/` record) was a bare signature over the key
  history, and a history is `32 × N` bytes and nothing more; consensus signed its serialized payload bare too, and a
  `PrepareAck` or `Promise` carries the **proposer's** bytes back (`accepted_value`). So a member could propose a
  value embedding its own key at a 32-byte boundary, have a victim accept it and answer a higher-ballot prepare with
  it — signed as the victim — then publish that signed answer as the victim's identity record; the verifier chained
  it through the victim's trusted key and merged every 32-byte chunk, the member's key among them. The proof is now a
  signature over `mycelium.identity/proof/1 ‖ len ‖ history` (`helpers::identity_proof_message`) and a consensus
  payload over `mycelium.consensus/msg/1 ‖ len ‖ bytes` (`consensus::consensus_signing_message`), so neither
  verifies as the other; `GossipAgent::sign_with_identity` now states that its caller owns the message's domain.
  Seen failing first: `a_signed_consensus_answer_is_not_an_identity_proof` (the member's key entered `peer_keys`).
  Wire **v12** unchanged — the frames carry the same bytes; only the signed message gains a prefix.
  **Mixed fleet, one release, direction by direction** (`docs/guide/deprecations.md` §23): a 2.32 node accepts
  the bare (2.31) form of both signatures and counts each acceptance (`mycelium_identity_untagged_proofs_total`,
  `mycelium_consensus_untagged_signatures_total`; `GET /stats` `identity_untagged_proofs` /
  `consensus_untagged_signatures` — validations, not peers: the identity watcher re-validates every record on
  every `sys/identity*` change), the identity one **only while `require_identity_proofs` is off**, since that flag
  is the posture under which every proof must be one. A 2.32 **acceptor answers in the form of the request it
  verified** — a bare `Prepare`/`Propose` gets a bare `PrepareAck`/`Promise`/`Nack`/vote — so a **2.31 proposer's
  rounds complete against 2.32 acceptors**, lease renewals included (found by the adversarial review of #585:
  the first cut answered everything tagged, which a 2.31 proposer drops). A **2.32 proposer** is dropped at 2.31
  acceptors and learners (*bad signature*), so it times out until a quorum of its group is upgraded, as with
  2.30.0's `Prepare`; a 2.31 learner takes the committed value from anti-entropy instead. On the identity side a
  **2.31 verifier rejects a 2.32 peer's tagged proof** as *present but not verifying*: the keys are not merged,
  `identity_anchor_conflicts` increments, and the watcher re-raises it on every `sys/identity*` event for every
  2.32 peer — so an un-upgraded node's `identity_anchor_conflicts` **climbs during the window** and is not a
  poisoning signal there; a 2.32 peer's handshake key still enters that node's `peer_keys` through the TLS anchor
  (`record_peer_anchor`), its **rotated** keys do not until the node upgrades. The allowance closes in the next
  MINOR. **Not claimed:** while the allowance is open and the flag is off, a bare signature over caller-shaped
  bytes still reads as a proof there (now with the anchor tripwire the unsigned path always had), no narrower
  than the flag-off posture already was; and **the tagging covers the identity proof and the consensus payload
  only** — KV `SignedData` (`canonical_update_bytes`), role claims and audit records are still signed over
  untagged canonical bytes, the residual for the next MINOR (`docs/operations/what-is-proven.md`). The proof side
  is closed regardless: the verifier accepts only a message that *begins* with the 25-byte identity tag, and none
  of those messages can — a KV update's bytes 8–16 are the signer's own id hash, a role claim's or audit record's
  first word is a small length prefix with zero high bytes.
- **An acceptor's promise and acceptance, and a slot's decided floor, never reached the WAL.** The record
  `sys/consensus-accepted/{node}/{slot}` — what `prewarm_accepted` restores at start so a restarted node keeps its
  promise and cannot vote twice at one ballot — was applied to the store and gossiped, never handed to the WAL; so
  was `consensus/decided/{slot}`, the floor acceptors refuse below. Only the committed slot and its lease were. A
  node that crashed after promising or voting restarted with no memory of either (an orderly shutdown hid it: the
  final snapshot is taken from the store), and the prepare path's doc said "handed to the WAL" when it was not.
  Both now reach stable storage through the commit's own `append_sync` (a forced `fdatasync`) **before** the
  `PrepareAck`, the vote or the proposal leaves; a record that does not is not answered — the acceptor stays
  silent, a proposer's own attempt ends as `Timeout` with the new reason `unrecorded` — and a commit's
  `persisted` now folds the floor in with the slot and the lease. The permanent commit's lease tombstone reaches
  the WAL the same way; the shared ballot key (`consensus/ballot/`) is appended fire-and-forget (`append_try`)
  from the proposer's `raise_ballot` and the voter's write alike, a liveness aid a promise already refuses past.
  An acceptor that withholds an answer counts it (`GET /stats` `consensus_acceptor_unrecorded`,
  `mycelium_consensus_unrecorded_total{role}`). **Unmeasured:** the per-answer `append_sync` on the voter loop is
  an fsync per promise and per vote, serialised on the listener; its cost is not measured here —
  `examples/authority_drain` is the shape such a measurement takes. **Upgrade note:**
  `mycelium_consensus_timeouts_total{reason}` gains `unrecorded` (here) and `ballot_exhausted` (above, under Added)
  beside 2.31.0's set — an alert keyed on `reason` should know both (`deprecations.md` §28) — and every acceptor
  answer is now one forced `fdatasync` on the listener.
  Seen failing first: `an_acceptors_record_survives_a_crash_without_a_snapshot`
  (the crash copy replayed `consensus/committed/leader/solo` and not the acceptor's record). **Not claimed:** the
  learner's re-stamp of `consensus/committed/` stays on the gossip path (replicated state anti-entropy
  re-supplies; its absence after a restart reads as *not yet arrived*, which the restored floor keeps safe), and
  a `LockGuard`'s release tombstones stay there too (a restored lock is bounded by its lease, which is on disk).
- **A proposer counted its own promise and vote without being in the group's roster.** `propose` inserted this
  node into its promisers and voters unconditionally while the quorum it needed came from the `grp/{group}/`
  roster; the trust-slice filter applied only to what arrived, and neither `group_propose` nor the gateway's
  `POST /gateway/overlay/elect` — which proposes `leader/{group}` for any group a client names — asked whether this
  node was a member. A stranger to a one-member group therefore had a quorum of one and satisfied it alone,
  choosing the group's leader with no member voting; on a larger group two strangers with disjoint acceptors each
  reach quorum, since a stranger's self-vote is one the roster does not contain and the quorums need not
  intersect in a member. The engine now refuses at its one door, before anything leaves:
  **`ConsensusResult::NotAMember { slot, group }`**, with `CommitError::NotAMember` and
  `ConsistencyError::NotAMember` beside it and **403 `not_a_member`** (naming the group) from the gateway, on
  every route that reaches a group proposal. `cluster_propose` has no roster and is unaffected;
  `cross_group_propose` never counted itself. Seen failing first:
  `a_non_member_cannot_propose_to_a_group_on_either_surface` (B committed `leader/members-only` = B).
  **Upgrade note:** three `#[non_exhaustive]` enums gain a variant — a `_` arm must fail closed, as the enums'
  docs already require (`docs/guide/deprecations.md` §24).
- **`mycelium-reason`'s façade dispatched protected `llm.invoke` as the node.** `POST /gateway/reason/route` and
  `/gateway/reason/v1/chat/completions` never read the principal the gateway's auth layer resolved and called
  `rpc_call` — the node's own action — so under `gateway_caller_profile = secure` the provider saw `node:{gateway}`
  as the caller, not the HTTP client (`gw.caller_profile`'s promise held on `/gateway/llm/call` and not here), and
  no action evaluator was consulted. Both routes now carry the resolved principal into the router's dispatch
  (`InferenceRouter::call_as` → `rpc_call_as`) and run the gateway's preflight per attempt under
  `gateway:reason/route` / `gateway:reason/v1/chat/completions`; a denial is `403` (`{"error": "policy", …}` on
  `/route`, `permission_error`/`policy` with a `mycelium.data` block on the OpenAI envelope) and the provider is never
  reached. `mycelium-reason` **0.8.0** (`RouteError` gained `Refused`; `call_as` is new; `call` is unchanged and
  remains the node's own in-process action). Seen failing first: `the_facade_dispatches_as_the_http_client_not_the_node`
  (the provider saw `node:127.0.0.1:…`) and `the_facade_runs_the_action_preflight` (both doors answered 200 and the
  provider ran). **Upgrade note:** under the secure profile a provider whose `sys/caller-context` marker has not
  reached the gateway is now refused for a façade call (`provider_without_caller_context`, failed over) instead of
  being called as the node — the same rule `/gateway/llm/call` has; an exhaustive `match` on `RouteError` needs the
  new arm.
- **Public `/a2a` `tasks/get` and `tasks/cancel` had no owner check.** Task ids are caller-chosen on `tasks/send`,
  so they are enumerable, and both methods looked an id up with no caller: any client — any bearer, or none — could
  read another caller's completed artifact or cancel its task by naming the id. The Phase-C audit (2026-09-20)
  closed this for federated credentials only. A task now records the principal that created it (`A2aTask::owner`)
  and `tasks/get` / `tasks/cancel` answer **only that identity**; another principal, or an anonymous caller (no
  identity to match), is refused `-32004` naming the rule, and a `tasks/send` or `tasks/sendSubscribe` under an id
  another identity owns is refused before any dispatch. Every reader of the task map enumerated
  (`grep -n "\.tasks" src/agent/a2a.rs`: the two inserts, `get`, `cancel`, the eviction sweep). Seen failing first:
  `a_task_is_readable_and_cancellable_only_by_the_identity_that_created_it` (the second principal read the first's
  task). **Upgrade note:** an A2A client that polls `tasks/get` must send it under the same bearer as its
  `tasks/send`; an anonymous client gets its result on the `tasks/send` response or the `sendSubscribe` stream, which
  already carry it (`deprecations.md` §25).
- **`mycelium-tuple-space`'s `GET /api/tuple` answered without a bearer.** The gateway's auth boundary is a path
  prefix, and the cluster-wide tuple overview (every node's role, WAL bytes, per-stage depth and pressure) was
  mounted outside it. It is now `GET /gateway/tuple/overview` behind `tuple:read`; `/api/tuple` answers 404 (the
  router no longer mounts it — an unauthenticated redirect would have nowhere honest to point). `rbac.md`'s claim
  that *the routing code asserts* the public list is now true: `http::PUBLIC_PATHS` holds it and
  `the_public_surface_is_exactly_the_documented_list` probes a running gateway. Seen failing first:
  `the_overview_sits_behind_the_gateway_bearer` (the old path answered 200 with no bearer). **Upgrade note:** a
  dashboard or the integration scenario reading `/api/tuple` must read `/gateway/tuple/overview` with a
  `tuple:read` (or legacy) bearer (`deprecations.md` §25).
- **`/gateway/llm/call` and `/gateway/llm/stream` ran no action preflight.** The raw routes refuse `llm.invoke` and
  name `/gateway/llm/call` as its door, yet neither door consulted the evaluator `/mcp` and `/a2a` consult — a policy
  denying `llm.invoke` was walked around by choosing it. Both now run `ae_preflight` under `gateway:llm/call` /
  `gateway:llm/stream`, carry a presented `_meta.mandate` to the provider with the resource
  (`prompt:{ns}/{name}@{provider}`) and record the outcome; a denial is `403 {"error": "policy", …}` on `/call` and
  the stream's one event `{"type": "error", "error": "policy", …}` on `/stream`. Inert without an evaluator. Seen
  failing first: `the_llm_doors_run_the_action_preflight` (`/call` answered 200 and the provider ran).
- **Bearer comparison is constant-time.** `resolve_token` compared a presented bearer with `==`, whose early exit
  leaks the length of the matching prefix to a timing observer; it now uses `subtle::ConstantTimeEq` (already in
  the tree under `ed25519-dalek`). No behaviour change; the existing auth tests cover it.
- **The signal SSE doors streamed every protected RPC request to a `mesh:read` holder.** `/signals/{kind}` and
  `/gateway/signal/sse/{kind}` registered a receiver for whatever kind the path named — the same handler table
  `rpc/serve` and the native MCP tools register on, fanned to every receiver — so a `mesh:read` token could open
  `/signals/mcp.invoke` and read each tool call's whole frame (`payload_b64`: the caller envelope, the carried mandate
  and possession proof, the correlation nonce) while the raw routes refused to *send* that kind (closure plan C1).
  Both streams now refuse a protected kind with the raw routes' own body, `403 {"error": "protected_kind", …}`,
  whatever the token holds; an ordinary kind still streams. Added to the C7 bypass matrix. Seen failing first:
  `sse_doors_refuse_protected_kinds` (both doors answered 200 for `mcp.invoke`). **Upgrade note:** a dashboard or
  SDK reader subscribed to `mcp.invoke`, `skill.invoke`, `llm.invoke` or a configured `protected_rpc_kinds` entry
  now gets 403 at open; read the decision trace or the evidence journal instead (`deprecations.md` §25).
- **`mesh:serve` bound to neither a kind nor a request: `rpc/respond` could pre-empt any in-flight RPC by nonce.**
  Two defects, one door. (a) The reply interceptor claimed a pending call's oneshot on **nonce match alone**, and
  `await_nonce_reply` then dropped the reply on sender mismatch — so the oneshot was consumed either way, and a peer
  that learned a nonce could make the legitimate reply undeliverable (the caller timed out although the provider
  answered). The interceptor now claims only when the nonce **and** the node the call was sent to match
  (`TaskCtx::rpc_pending` carries the target); a nonce-only match from another sender is counted in
  `SystemStats::rpc_reply_sender_mismatches` (also on `/stats`) and ignored, and the real reply still arrives.
  (b) `POST /gateway/rpc/respond` took `nonce_hex` and `sender` from the body with nothing binding them to a request
  this principal's serve stream was handed, so any `mesh:serve` holder could answer any call it learned a nonce for —
  and release its parked cohort admission. A request streamed on `rpc/serve` is now recorded against the principal
  that opened the stream (`HttpCtx::served_rpcs`, lock-order row 55, 300 s TTL, capped), and `rpc/respond` answers
  only a nonce that principal was handed and has not yet answered: anything else is `403 {"error":
  "unserved_request"}` and nothing is emitted. Both doors added to the C7 bypass matrix. Entry points enumerated by
  `grep -rn "rpc_pending\|release_parked" src/`: `await_nonce_reply` (the one writer, for `rpc_call` and
  `bulk_call`), the interceptor (the one reader), `gw_rpc_respond` (the one `release_parked` caller). Seen failing
  first: `a_forged_reply_from_the_wrong_sender_does_not_consume_the_pending_call` (`Err(Timeout)` where `Ok("real")`
  was expected) and `rpc_respond_answers_only_a_request_this_principal_was_handed` (the other principal's reply
  answered 200 and the caller received it). **Upgrade note:** an SDK agent that answers with a nonce from its own
  serve stream is unaffected; one that answered on another agent's behalf (a different bearer) now gets 403 — open
  the stream and respond under one credential (`deprecations.md` §25). `SystemStats` gained a field (an exhaustive
  literal breaks).
- **The adversarial review's findings on #587** (rule 4), each fixed before merge. (F1) Two serve streams of one
  principal on one kind both receive each request, so the second replica's `rpc/respond` got `403 unserved_request`
  and both SDKs raise it, killing that replica's loop: a repeat answer from the same principal is now `200
  {"ok": true, "duplicate": true}`, dropped and counted (`/stats` `rpc_respond_duplicates`); the first record of a
  request is never reset by a second stream. Seen failing first: `a_replicated_serve_loop_answers_idempotently`
  (`[200, 403]`). (F2) The 300 s answer window assumed every door waits at most 300 s: `/gateway/llm/call` clamps
  `timeout_ms` to it; an in-process `rpc_call`'s bound is stated. Seen failing first (clamp toggled off):
  `the_llm_door_timeout_is_clamped_to_the_gateway_ceiling`. (F3) The `(nonce, sender)` claim closes the accidental
  case, not a forger who writes the target's id — `sender` is emitter-written; said so in `SystemStats`, §25 and
  lock-order row 2. (F4) `mycelium-agentfacts`' two `/.well-known/agent-facts*` routes are listed
  (`COMPANION_PUBLIC_PATHS`, rbac.md); the public-surface test is renamed for what it checks and checks mounting by
  405 (it failed in a build without `metrics`, whose `/metrics` answers 404 by design). (F5) The A2A owner check and
  the insert straddled the dispatch: the id is now reserved atomically before it (a pending entry removed on any
  failure). Seen failing first: `two_principals_racing_one_task_id_dispatch_once` (the skill ran twice, both
  completed). (F6) The reservation runs after the federation authorisation; `-32001`/`-32004` stay distinct, stated.
  (F7) Both SSE doors refuse `rpc.result` and `bulk.result`. Seen failing first: `sse_doors_refuse_reply_kinds`
  (`/signals/rpc.result` answered 200). (F8) rbac.md states what a `mesh:serve` stream sees. (F9) The served-request
  TTL sweep runs every 1 024 inserts or at the cap, not per request. (F10) The tuple-space crate's manifest comment.
  The `gw_llm_stream` label that failed `llm`-without-`tls` clippy is gone (a helper with an early return).
- **The OIDC key refresh is single-flight, bounded and timed out** (`src/agent/oidc.rs`). The verifier fetched the
  JWKS *before* taking its cache's write guard, so a burst of verifies on a cold or stale cache, or on an unknown
  `kid`, each fetched (sixteen concurrent verifies cost the IdP 32 requests) — although lock-order row 17 has said
  since WS4 that the guard was held across the fetch. And since the gateway offers every bearer to OIDC first, an
  **unauthenticated** client sending JWTs with invented `kid`s made the node call the IdP once per request, through
  a client with **no timeout**, so an IdP that stopped answering held gateway handlers indefinitely. Now:
  - one fetch at a time, run in a **detached task** (a verify cancelled mid-fetch does not cancel it or spend the
    cooldown); the cache is a `std::sync::RwLock` written only to swap in the result, so a token whose `kid` is
    cached **never waits** for a fetch — past the TTL it is checked against the cached keys while they refresh;
  - a refresh forced by an unknown `kid`, or a retry after a failed fetch, at most once per `JWKS_REFRESH_COOLDOWN`
    (30 s) — inside it the token is refused without a fetch — or once per `JWKS_EMPTY_RETRY_BACKOFF` (2 s) while no
    keys are held at all;
  - one `JWKS_FETCH_TIMEOUT` (10 s) deadline across discovery and the JWKS request together;
  - a failed fetch still keeps the previous keys; the cache's clock reads go through `sim_seam` (`oidc.rs` leaves
    the forbidden-call baseline).

  **Behaviour note:** after an IdP key rotation, tokens under the new key can be refused for up to 30 s if an
  unknown `kid` was seen just before. Seen failing first: `concurrent_verifies_with_an_unknown_kid_fetch_the_jwks_once`
  (32 fetches, expected 1), `an_unknown_kid_forces_at_most_one_refresh_per_cooldown` (4, expected 1),
  `a_jwks_endpoint_that_never_answers_does_not_hold_verify` (still waiting at 5 s),
  `a_failed_jwks_fetch_is_not_retried_inside_the_cooldown` (4, expected 1); and, from the PR's adversarial review,
  `a_cached_kid_does_not_wait_behind_a_stalled_fetch` (still waiting at 2 s), `a_cancelled_verify_does_not_cancel_its_fetch`
  (2 fetches, expected 1), `discovery_and_jwks_share_one_deadline` (3.8 s against a 2 s deadline),
  `with_no_keys_a_failed_fetch_is_retried_after_the_short_backoff` (still refused after the back-off),
  `keys_past_the_ttl_are_refreshed` (no refresh). `a_rotated_key_is_accepted_after_the_cooldown` and
  `a_failed_refresh_keeps_the_previous_keys` pin behaviour that already held.
- **The companion WALs (tuple space, blackboard): a failed append no longer strands later records, a corrupt record
  refuses the open, one owner per file, and compaction survives a power loss** (post-360 hardening row C). Both
  writers carried on after a failed `write_all`, so the next acknowledged record landed behind the torn frame, and the
  next open read the torn frame's length over it, called the lot a torn tail and **truncated every later record
  silently** — and any undecodable record, torn or not, ended replay the same way. Now a failed append **poisons the
  writer** (every later append refused by name) until a compaction rewrites the log from the live state — the
  maintenance tick asks for one at once — or a reopen truncates the torn tail; at open, a frame the file ends inside
  is still a crash's torn tail and is truncated (now synced, file and directory), while a whole frame that does not
  decode, or bytes that are no record, **with data after it refuses the open** (`InvalidData`, naming the file and
  the byte; the file untouched). Each WAL takes the core's `OwnershipLock` on `<wal>.lock`, so a second store on one
  path is refused with `WouldBlock`. Compaction installs its file in the core snapshot's order — temp file synced,
  renamed, **directory synced**: the tuple space skipped the directory sync, the blackboard both syncs
  (`std::fs::write` + `rename`). And compaction now **folds the log itself** under the WAL lock instead of
  snapshotting memory: a write appends and *then* applies to memory, so a compaction landing between the two
  rewrote a log without a record already acknowledged, and a crash after it lost the record; the log holds every
  acknowledged record and no append interleaves under the lock, so the rewrite loses none (the tuple space's
  compaction also stops taking stage and in-flight locks inside the WAL lock). Not done: a length prefix corrupted to run past the end of the file still reads as
  a torn tail (the format has no checksum), and there is no `on_unreadable = "quarantine"` counterpart — move the
  file aside to start empty. Seen failing first, in both crates: `a_failed_append_never_strands_a_later_acknowledged_put`
  / `_post` (`left: [0]`, `right: [0, 2]` — the put after the failure was acknowledged and gone at reopen),
  `a_corrupt_middle_record_refuses_the_open_and_leaves_the_file` (the open succeeded), `a_second_owner_of_the_wal_is_refused`
  (the second open succeeded), `compaction_syncs_the_directory_after_the_rename` (`no dir.sync in ["tmp.write",
  "tmp.sync", "rename"]`) / `compaction_syncs_the_temp_file_and_the_directory` (`no tmp.sync in ["tmp.write",
  "rename"]`), `a_compaction_between_append_and_apply_keeps_the_acknowledged_put` (`left: None`, `right: Some(0)`) /
  `_post` (`left: []`, `right: [0]`). **Upgrade notes:** a WAL holding a corrupt record before its last now refuses `TupleStore`/`BoardStore`
  construction — so the node does not become primary — where it used to start with the later records gone; a second
  store on one `wal_path` (in one process or two) is refused; a `<wal_path>.lock` file appears beside each WAL (do not
  delete it while a node runs — the lock is the OS's and goes with its holder).
  **The PR's adversarial review (#597) found a regression and six more, all fixed here.** The poison was cleared only
  by compaction, and only the primary ran it — so a **secondary** whose WAL failed once refused every later mirrored
  record, and the replicate paths had already marked each id seen, so no re-delivery applied it: a failover lost
  everything after the error (before the PR, only the failing record). Now WAL maintenance (the periodic sync, and the
  repairing compaction) runs on **every node that holds a WAL**, primary or secondary; a mirrored record whose append
  is refused is forgotten as seen and the mirror drains the primary again (`apply_records`, the blackboard's
  replicate handler and initial sync); a mirror's `remove_queued` no longer counts an ack whose record was refused
  (the blackboard's `discard` already did not; it is now logged). Also: an Auto-mode node whose store refuses to
  open logs the error (file and byte) and **withdraws its candidacy**, where it logged nothing useful and stayed on
  the ring to be elected again; a valid kind byte followed only by zeros (a zero-extended partial append) is a torn
  tail, not corruption — and the limit beside it is stated: a torn frame whose missing body was zero-filled can
  decode into a phantom record; a repair that keeps failing **backs off** (1, 2, 4 … 64 ticks), and a failing sync no
  longer skips it; the compaction temp file is `<wal_path>.compact`, named after the whole file (it was derived from
  the stem, so `x.a` and `x.b` shared one); and compaction keeps the **id high-water mark** — a lone `Ack` for the
  highest id when that id was acked away — so a restart never reissues an acknowledged id (open fenced `next_id` from
  the highest id left). Seen failing first, in both crates: `a_poisoned_mirror_repairs_and_keeps_every_later_record`
  (`left: 0`, `right: 4` / `left: []`, `right: [0, 1, 2, 3]`), `an_auto_node_whose_wal_refuses_withdraws_its_candidacy`
  (still advertised), `a_valid_kind_followed_by_zeros_is_a_torn_tail` (refused as corrupt), `a_failing_repair_backs_off`
  (`20 attempts in 20 ticks`), `the_compaction_temp_file_is_named_after_the_whole_wal_file` (`IsADirectory` at the
  stem's name), `a_restart_never_reuses_an_acked_id` (`left: 0`, `right: 3`); the tuple space's
  `the_repair_runs_even_when_the_sync_fails` and `a_refused_mirror_ack_is_not_counted` (`left: 1`, `right: 0`). Added
  as pins (they held): the repair path end to end, a known kind whose body does not decode, an ack racing a
  compaction, the blackboard's refused `discard`. **Further upgrade notes:** a secondary now syncs its WAL
  periodically and compacts it; a compacted WAL may end in a lone `Ack` (older builds replay it as a no-op); a temp
  file left by a crashed compaction under the old name (`<stem>.compact`, `<stem>.wal.compact`) is not removed.
  **A second review (the re-review of #597) found the re-drain could resurrect, and four more; fixed.** A tuple-space
  re-drain restarts from offset 0 and an applied `Ack` had forgotten its id, so a primary compacting between drain
  chunks (the `Ack` gone, the `Put` already re-delivered) left an acked item live in the mirror; each mirror now keeps
  a bounded memory of the ids it saw acked (100 000, oldest forgotten first) and never re-applies one — the blackboard
  too, where a snapshot racing a live `Ack` did the same. No re-drain or re-sync while the mirror's own WAL refuses
  appends (it re-fetched the primary's whole log every tick). A promotion with a refused record pending replays the
  old primary's log **from the start** (the tuple space; the heartbeat cursor cannot cover a record refused before
  it), or first re-syncs from the primary the mirror last heard from (the blackboard), and says so when that fails.
  A compaction failing **after its rename** poisons the writer — it went on appending to the unlinked old file. Every
  failing compaction backs off, poisoned or not, capped near 10 s (50 ticks of 200 ms; 10 of 1 s on the blackboard,
  which was a 64 s ceiling). An `init_store` racing `shutdown` aborts the maintenance task it starts, which otherwise
  outlived the space and held the WAL's lock. Seen failing first, in both crates unless named:
  `a_redrain_never_resurrects_an_acked_item` (`left: [1, 0]`, `right: [1]`) / the blackboard's
  `an_ack_before_its_post_does_not_resurrect_the_fact` (`left: [0, 1]`, `right: [1]`), `no_redrain_while_the_mirror_wal_refuses`
  / `no_resync_…`, `a_promotion_with_a_pending_gap_replays_from_the_start` (`left: (3, 999)`, `right: (0, 0)`) /
  `…_syncs_from_the_last_primary` (`left: None`), `a_failure_after_the_rename_poisons_the_writer`,
  `a_failing_compaction_backs_off_even_when_not_poisoned` (`20 attempts in 20 ticks`),
  `the_repair_back_off_is_capped_near_ten_seconds` (`65 ticks between repair attempts`),
  `a_store_opened_after_shutdown_does_not_pin_the_wal`. Pins that held: `the_backfill_redrains_a_refused_record` /
  `the_mirror_resyncs_a_refused_fact` (two nodes, the loop re-draining on `mirror_gap`) and
  `the_high_water_mark_survives_two_compactions_and_a_restart`.

### Security
- **An open gateway is refused off loopback** (plan `post-360-hardening.md` row P1). With no credential model the
  gateway serves every route to an anonymous caller, and `gateway_auth_token`'s docs called that *suitable for
  loopback-only deployments* — but nothing checked that the deployment was one, so `http_addr = "0.0.0.0"` with no
  token served the whole gateway to anyone who could reach the port. `start()` now refuses a gateway on a
  **non-loopback** `http_addr` with no token, no token table and no `[oidc]`, as `InvalidField { field: "http_addr" }`,
  naming the credential settings the build honours, unless the new **`gateway_allow_unauthenticated = true`**
  (`GOSSIP_GATEWAY_ALLOW_UNAUTHENTICATED`, a strict boolean — any other value is refused) waives it; the waiver warns
  once at start. Loopback is `127.0.0.0/8`, `::1` and an IPv4-mapped loopback; `0.0.0.0` and `::` are not; a hostname
  never reaches the bind (`validate()` requires an IP literal). The check runs in every gateway build — without
  `compliance` the only credential is `gateway_auth_token`. **One rule for blank secrets** (found by the
  independent review of #598): `validate()` refuses an empty or whitespace-only token in `gateway_auth_token`, in
  every `gateway_scoped_tokens` entry and every `gateway_named_tokens` entry, by name — a TOML table entry with
  `token = ""` and `scopes = ["*"]` used to count as a credential at start and, over HTTP/2 (which `gateway_tls`
  serves, and which does not trim `Bearer `), granted `*` to an empty bearer; and `resolve_token` never matches a
  blank presented bearer, whatever the table holds. One predicate for `gw.not_open` and the refusal,
  `guarantee::gateway_credential_model`. Seen failing first: `a_blank_gateway_token_is_refused_in_every_form` —
  `a blank token in gateway_scoped_tokens must refuse at validate(), got Ok(())`;
  `resolve_token_never_matches_an_empty_presented_bearer` — `"" must not authenticate`, left
  `Some(("token:gw-1/legacy", ["*"]))`. New guarantee **`gw.exposed_closed`** (`not_applicable` on loopback, `not_configured` under the waiver),
  required by **`secure-single-domain` rev 3**, which refuses one node rev 2 admitted — a gateway whose only credential was a blank `gateway_auth_token`, which `gw.not_open` rev 1 read as enforced (and which `validate()` now refuses in every profile) — and otherwise names the waiver when `gateway_allow_unauthenticated` is set (an upgrade note, below). The demo
  nodes that bind `0.0.0.0` with no credential (`three_node_demo` — the Docker and Kubernetes demo image —
  and `federation_node`) set the opt-in, with a comment saying why. Seen failing first:
  `an_exposed_gateway_with_no_credential_refuses_to_start` — `0.0.0.0 with no credential (token None) must refuse to
  start, got Ok(())`. **Upgrade note** ([deprecations.md §26](docs/guide/deprecations.md)): a deployment binding
  `0.0.0.0` (or any non-loopback address) with no credential now refuses to start — set a credential, bind loopback,
  or set the opt-in; a **blank token anywhere** (`gateway_auth_token`, a scoped or named entry, including
  `GOSSIP_GATEWAY_AUTH_TOKEN=""`) now refuses at `validate()`; `GossipConfig` gained a field (an exhaustive struct
  literal breaks).
- **A hosted WASM component's `mesh.emit` is confined.** The host forwarded any kind a component
  named at cluster scope on the node's full `MeshHandle` — only `kv` was confined — so a component could
  emit `mcp.invoke`, `skill.invoke` or `llm.invoke` from inside the node's process and reach every
  `rpc_rx` of that kind on the hosting node **unframed**: a self-originated request is a plain member
  call, with no mandate and no evaluator, past the doors (`/mcp`, `/a2a`, the raw routes' `protected_kind`
  refusal, provider enforcement) that check authority for exactly those kinds. A component may now emit
  only a kind under `comp/{namespace}/…` or one the host listed for it (`HostState::with_emit_kinds`), and
  never a protected kind — the built-ins or the node's `protected_rpc_kinds`, which a listing cannot
  override (`confine_kind`, rule `host.emit_admission`). The WIT signature is unchanged (it carries no
  error channel, and changing it would re-cut every committed fixture component), so a refused emit is
  dropped, logged and counted: `mycelium_wasm_host_emits_refused_total{reason}`. The predicate every
  door asks is now one ungated function, `mycelium::is_protected_kind` (with
  `BUILTIN_PROTECTED_RPC_KINDS`), which the gateway's raw routes and provider enforcement call too. Seen
  failing first: `a_component_cannot_emit_a_protected_kind_but_can_emit_in_its_own_namespace`
  (`mycelium-wasm-host/src/host.rs` — the protected kind arrived at `rpc_rx(mcp.invoke)`). **Upgrade
  note:** `HostState::emit` returns `Result<(), ConfinementError>` and `ConfinementError` gained
  `ProtectedKind` and `ForeignKind` (an exhaustive `match` breaks); a component that emitted a kind
  outside `comp/{namespace}/…` is now refused — list the kind with `with_emit_kinds` if it is not
  protected work. **Granting such a kind is code-only for now:** nothing in the runtime or the unit file
  calls `with_emit_kinds`, so a component installed by a provisioner or a stem emits only under its own
  namespace; a `[hosts]` field naming extra emit kinds is follow-up work. A namespace that is empty or
  contains `/` emits nothing (`ConfinementError::MalformedNamespace`), since its `comp/` family would
  overlap another namespace's — seen failing first: `a_namespace_with_a_slash_or_none_emits_nothing`
  (`a/b` emitted `comp/a/b/x`). The coop examples that build a `HostState` by hand
  (`catalog`, `catalog_viz`, `mcp_toolgrowth`) pass the node's `protected_rpc_kinds` too.
- **A shadow-lane `tool/*` proposal is no longer registered as a live MCP tool.** D20 promises the
  shadow lane *takes no demand*, and the provisioner kept that for capability resolution (a shadow is
  advertised as `{name}.shadow`, never under the incumbent's filter) — but the WASM runtime bridged
  every `tool/*` namespace install as MCP tool `{name}`, shadow or not, so a proposed tool nobody had
  accepted appeared in `tools/{name}.shadow/{node}`, in `tools/list`, and answered `tools/call`. The
  install now carries its lane (`RuntimeCtx::shadow`) and the runtime bridges a tool only for a real
  load; acceptance withdraws the shadow and the reinstall registers it. A `[[serve]]` skill was already
  shadow-safe (`while_live` resolves `{ns}/{name}`, which a shadow never advertises). Seen failing
  first: `a_shadow_lane_tool_proposal_is_not_registered_as_an_mcp_tool_until_accepted`
  (`mycelium-wasm-host/src/provisioner.rs` — the shadow registered `tools/echo.shadow/{node}`).
  **Upgrade note:** `RuntimeCtx` gained `shadow` (an exhaustive literal breaks). **Unchanged, and
  stated because an earlier draft of this entry said otherwise:** a shadow *blob*'s `[[activation]]`
  does **not** run — the entry is renamed `{name}.shadow` before install and the activation hook
  matches declarations on the renamed name, so it finds none (`no_declaration`); the shadow is placed,
  not activated.
- **A `[hosts]` table with no `trusted_publishers` refuses at start, instead of installing anything any
  peer publishes.** `Provisioner::provenance_ok` admitted every entry when the trusted list was empty,
  `validate()` checked only the subset rules, and the stem started without a word (the reference said
  *empty means provenance is not required*) — so a stem whose unit file omitted the key installed and
  served any catalogue line gossiped into its mesh. The 2.18.1 shape: `validate()` now refuses it by
  name (`hosts.trusted_publishers`), and the stem will not start; a host that means to run without
  provenance says so with `[hosts] accept_unsigned = true`, which the stem warns about once at start
  and which is refused beside a non-empty `trusted_publishers`. Every reader of the list was enumerated
  (`grep -rn trusted_publishers mycelium-wasm-host/src src`): the provisioner's predicate, the stem's
  start, the artifact gateway's publish door and `validate()`'s operator-key rules — none changes
  meaning. Every example unit file under `examples/units/` and the co-op fixtures already list a key.
  Seen failing first: `a_hosts_table_without_trusted_publishers_is_refused_unless_it_opts_out`
  (`src/capability_config.rs` — the table validated). **Upgrade note:** a unit file with `[hosts]` and
  no `trusted_publishers` now fails to load; add the publisher keys, or `accept_unsigned = true` to
  keep the old behaviour on purpose.
- **The staging pull is bounded before its first byte.** `DiskStagedSource::stage_artifact` took the
  holder's `artifact.size` reply as the pull's `total` and wrote toward it piece by piece — a peer
  answering `u64::MAX` had the stage filling its disk, with nothing in the catalogue line consulted —
  and a stem re-tried every failed stage on every tick. A holder's size reply past the stage's ceiling
  (`DEFAULT_MAX_STAGE_BYTES`, 64 GiB; `DiskStagedSource::with_max_stage_bytes`, `StemOptions::max_stage_bytes`)
  or past the entry's own `size_bytes` hint (`stage_artifact_bounded`; the hint is outside the
  signature and bounds the pull without being trusted for anything else — the hash still decides what
  is kept) is refused before any range is requested, counted as
  `mycelium_artifact_stage_refused_total{reason="size_past_ceiling"}`; the stem's tick stages each
  entry under its hint and backs a failed stage off — the next attempt `2^min(n,6)` ticks after the n-th
  consecutive failure (2, 4, … 64), while no holder answering yet is not a failure and is retried on
  the next tick (`DiskStagedSource::try_stage` → `StageOutcome::{Staged, NoHolder, Failed}`; seen failing
  first: `a_missing_holder_is_not_a_failed_stage_and_a_failure_backs_off_from_the_failure`, a `NoHolder`
  outcome skipped the next tick). `mycelium-stem`'s librarian-over-store mirror stages each manifest
  entry under its hint and the ceiling as well. Seen failing first:
  `a_size_reply_past_the_ceiling_is_refused_before_any_byte_is_written` (`mycelium-wasm-host/src/http_source.rs`
  — one range was requested against a size of `u64::MAX`). **Upgrade note:** `StemOptions` gained
  `max_stage_bytes` (an exhaustive literal breaks; `..Default::default()` is unaffected). **Not built:** a
  `--max-stage-bytes` flag on `mycelium-stem` (the default applies).
- **A hosted component's linear memory is capped.** The wasmtime store had no resource limiter, so a
  component could `memory.grow` without bound and take the node's memory with it — fuel bounds
  instructions, not bytes. Every store now carries `StoreLimits` with a memory cap:
  `DEFAULT_MEMORY_LIMIT_BYTES` (256 MiB — generous for a capability component, the fixtures run in
  under 2 MiB) or `HostState::with_memory_limit(bytes)` per instance; a grow past it is answered no
  and an initial allocation past it refuses instantiation. Seen failing first:
  `a_component_cannot_grow_its_memory_past_the_cap` (`mycelium-wasm-host/src/host.rs` — the echo
  component instantiated and ran under a 64 KiB cap). **Not built:** a unit-file field for the cap
  (the runtime uses the default for every install).

## [2.31.0] — 2026-10-09

**Stops that stop, and failures that say what they are.** A node, a stem or a demo image now shuts down on SIGTERM
as on SIGINT — including one that arrives during startup — and a second signal exits at once
(`mycelium::shutdown::ShutdownSignal`, the release's one API addition, which makes it a MINOR). A consensus timeout
is labelled by what other acceptors answered (`no_voters`, `promise_short`, `contended`, `blocked`,
`quorum_short`), and a cross-group retry counts its voters afresh. Doc-coverage run 22 and the wiki lint ride along.
Companions: `mycelium-py` 0.2.8 (`SupersededError`) and `langgraph-checkpoint-mycelium` 0.3.1 (`refused`). Wire
**v12** unchanged. **Upgrade note:** alerts keyed on a consensus timeout `reason` (`deprecations.md` §22).

### Added
- **`mycelium::shutdown::ShutdownSignal`** — one stop-signal handler for the node binary, `mycelium-stem` and the
  long-running examples: SIGINT or SIGTERM, installed before the bind, and **a second signal exits at once** with
  `128 + signal` (a hung shutdown used to wait for SIGKILL). Seen failing first: `tests/shutdown_signal.rs` (the
  child process ignored the second SIGTERM).
- **`mycelium-py` 0.2.8: `SupersededError`** — `consistent_set`, `cross_group_propose`, `distributed_lock` and
  `elect_leader` raise it for a 409 `superseded` (the slot was decided for another value); still an
  `httpx.HTTPStatusError`. Python only. Seen failing first:
  `mycelium-py/tests/test_superseded.py`.

### Fixed
- **The demo images stop on `docker stop`.** `federation_node`, `llm_agent`, `three_node_demo` and
  `confined_fleet_node` (each an image's entrypoint) awaited Ctrl-C only or never stopped, so a stop waited out Docker's grace period and was killed;
  `three_node_demo` now also shuts its agent down, flushing a configured persistence directory.
- **`mycelium-stem` shuts down on SIGTERM** (`docker stop`, a Kubernetes pod stop) as it does on SIGINT — writing its
  `--trace-dir` output and running its shutdown. It awaited `ctrl_c()` only, so a stopped container was killed
  (doc-coverage run 22, code gap 2). **The stem and the `mycelium` node also take a stop that arrives during startup**:
  both registered their handlers only once startup was done (the node already handled SIGTERM after that), so a
  SIGTERM during the bind took the default kill (reproduced 2 in 40); the handlers are now installed before the bind. Seen failing first: `mycelium-wasm-host/tests/stem_shutdown.rs`.
- **A consensus timeout is labelled by what other acceptors answered**, not by which phase it ended in
  (`mycelium_consensus_timeouts_total{reason}`): `no_voters` (none — a partition, or mid-upgrade all older than
  2.30.0), `promise_short` (some promised, too few), `contended`, `blocked`, `quorum_short`. Since 2.30.0 a partition
  ends in the prepare phase, and in `propose` the proposer's own vote made it `quorum_short` before that (code gap 3).
  Seen failing first: `a_proposer_nobody_answers_times_out_as_no_voters` (it was labelled `promise_short` by this
  change's first cut). A refusal in the voting phase is `contended`, not `no_voters`.
- **A cross-group proposal's retry counts its voters afresh.** Each group's tally kept the voters it had seen across
  ballots, so a voter from an earlier ballot never counted again and a retry could not reach quorum. Seen failing
  first: `a_cross_group_retry_counts_its_voters_afresh`.
- **`langgraph-checkpoint-mycelium` 0.3.1:** the blob route's own 403 `refused` (every holder refused for good —
  a removed member, a denied action) is reported as `"refused"`, not `"unauthorized"`, which sent an operator to
  the token (code gap 1). Not retriable, like `unauthorized`. Seen failing first:
  `test_a_route_refusal_is_not_a_token_problem`.

## [2.30.0] — 2026-10-09

**Consensus asks before it proposes.** Two concurrent proposers on a stable roster could commit different values for
one slot — a strictly higher ballot was granted and overwrote what an acceptor held, and the only guard was the second
proposer's local view of the commit — and both were told they had won. Consensus now runs a Paxos prepare phase, an
acceptor never forgets a promise, and a commit of another caller's value returns `Superseded`. It held inside the
threat model's supported profile too, and 2.14.0's notes overclaimed its closure. Wire **v12** unchanged (two message
variants appended). A MINOR: behaviour changes with an upgrade note (`deprecations.md` §21).

### Fixed
- **Two concurrent proposers could commit different values for one slot, on a stable roster.** A proposer
  learned what an acceptor had accepted only from a refusal, and a strictly higher ballot is never refused —
  the acceptor granted it and overwrote what it held. So A could commit `v1` at ballot 1 while B, which had
  not yet seen A's COMMIT, committed `v2` at ballot 2 through an acceptor A's quorum shared; both were told
  they had won (`LeadershipBasis::Decided`), and LWW kept whichever was re-stamped last. It held inside the
  threat model's supported profile too; the fence at the resource is what kept a second holder harmless.
  Consensus now runs **a prepare phase** before it proposes (`Prepare` / `PrepareAck`): a proposer carries the
  highest-ballot value its promise quorum reports, an acceptor promises a ballot to one proposer, and the
  promise survives a restart. The acceptor no longer **erases** its memory when a slot commits — that dropped
  promises, so a delayed lower-ballot proposal could commit a second value even with the prepare phase in place —
  and a commit records its ballot (`consensus/decided/{slot}`), below which acceptors refuse and which a reopened
  leased slot uses to set the previous decision aside. Seen failing first
  (`two_proposers_cannot_choose_two_values_for_one_slot`; the erasure, by the independent review's probe).
  A second review found the floor could hide a commit that had not yet arrived (fixed: set aside only once a node
  sees the decision is over), a late refusal aborting a won ballot, a late COMMIT resurrecting a released lock, and
  callers acting on a commit of an adopted value — now `Superseded`, which `consistent_set` and
  `set_capability_authz_via_consensus` already refuse.
  **Correction to 2.14.0's notes:** "a higher ballot overwrote an accepted value" was closed only where an
  acceptor refused. **Upgrade note:** an upgraded proposer times out rather than commits until a quorum of
  its group's acceptors is upgraded (`docs/guide/deprecations.md` §21). **Not claimed:** intersection across
  an electorate change — guide 04 § *Changing an electorate*, threat model §7.

## [2.29.0] — 2026-10-08

**Who changes an electorate.** A governed group's members are its elections' roster and quorum, yet any `mesh:write`
holder could move a node in or out, unaudited — the open question the group route's own doc named. The membership
intent is now the boundary: `/gateway/mesh/group` refuses a governed group **403** `governed_group`, `POST`/`DELETE
/gateway/govern/group` (`govern:write`, audited) moves the node, and `units/declare` no longer redefines a governed
group's eligibility filter. Beside it, the example-matrix check reads far less text that never runs as an example run.
Wire **v12** unchanged. A MINOR: a behaviour change with an upgrade note (`deprecations.md` §20).

### Changed
- **A governed group's membership moves only through a governance route.** A group under a live membership intent
  decides its elections from its roster, so who belongs to it is governance, not a data-plane write — and
  `POST`/`DELETE /gateway/mesh/group` let any `mesh:write` holder move the node in or out, unaudited (the handler named
  it an open question). Now those routes refuse a governed group **403** `governed_group`, and the new
  `POST`/`DELETE /gateway/govern/group` (`govern:write`) moves the node, for any group; every membership change through
  `/govern/group` is audited or counted (`governance_changes` / `governance_unaudited`), a plain group's join is audited,
  and a no-op join or leave records nothing. `POST /gateway/units/declare` refuses a `[[group]]` whose name is governed
  (403 `governed_group`) — its filter decides whom the governor can elect, so `cap:write` does not redefine it. Plain
  groups are unaffected. Test
  `a_governed_groups_membership_moves_only_through_a_governance_route`, seen failing first. **Upgrade note:** a client
  joining a governed group over HTTP uses `/gateway/govern/group` with `govern:write` (`deprecations.md` §20).
- **`scripts/check-example-matrix.py` reads less text that never runs as an example run.** Shapes in a script CI
  reaches that could pass a ✓ row in `examples/README.md`'s CI column nothing executes, now not read as runs, each
  with a self-test case seen failing first: a single-quoted `'$(cargo run --example x)'` (a `$(…)` counts only where
  the shell expands it), including one whose quote spans lines (a quoted string is one word across lines); an unused
  array `CMD=(cargo run --example x)` (an array runs whole only as `"${CMD[@]}"`, its first element as `$CMD`); a
  branch whose single literal condition cannot take it (`if false`, `if ! true`, `while false`, `until true`) and
  every branch after a literal `true` one; the right side of `false &&` / `true ||`; code after an unconditional
  top-level `exit`; an uncalled function body (`{ … }` or `( … )`; called means from a command that counts, or
  first in a `trap` string); a `case` pattern word; and a Python list outside a process-starting call (Python is
  now parsed; a file that does not parse is reported by name). **Still read as run although it may not run**, and
  listed in the check's docstring: any other conditional or loop (nothing is evaluated), every `case` arm, code
  after a call to a function that exits or after a `return`, a function shadowing a command, a quoted `"$RUN"`, and
  Python control flow and scope (`if False:`, an uncalled `def`, the last assignment anywhere). The check finds the
  same sites on this tree as before.

## [2.28.0] — 2026-10-08

**What the gates cannot see, closed; what the flakes were, found.** The three limits 2.27.0 left documented are gone:
`strict_eligibility` now drives the shipped history source end to end in CI; `mycelium wire-check` names overlapping
`[[presence]]` bands, and a rolling-upgrade dip is counted and named; and the coverage gates see what they could not —
a test whose feature gate no CI build satisfies is refused before push (the check models cargo's dev-dependency
unification, and found a test that had never run), script suites and single example runs are observed cases, and the
examples matrix's CI column is held to what CI executes. Then the open issues: a checkpointer test that read a retriable
miss as fatal (#561), a blob fetch that read an RPC refusal as a corrupt copy (#564 — `mycelium-reason` **0.7.1**, with
a `refused` outcome for a refusal that holds), and a tripwire test that assumed a local emit is a delivery (#568). #563
stays open: not reproduced, now instrumented. Wire **v12** unchanged. A MINOR: new public behaviour (a wire-check
finding, two metrics, a 403 `refused` blob outcome, a machine-readable `reason` on the caller-context refusal).

### Added
- **`examples/strict_eligibility.rs` calls the shipped history source** (`--features tls`, run in CI). A food
  co-op's members' assembly publishes signed appointment records and heads for its rotating coordinator role (at
  most two terms in a row, three in all); a member's node verifies them, its reader advances, and
  `mandate::history_source::history_from_appointment_stream` builds the history `eligible_strict` decides on. Each
  act asserts: a rested candidate is eligible, one at the limit is not, a node missing one appointment record
  answers `Unknown` where the lenient check says yes (and cumulative tenure `Unknown` for a candidate whose
  consecutive-terms rule stays decided), a node whose reader holds no checkpoint answers `Unknown` with
  every head presented to it, revoking the assembly's old key (in a hand-built key view, not through the identity path) leaves the history whole for a reader on a
  current-key head and empty for one still on a revoked-key head, and an `Unknown` verdict never makes a successor
  ready. It replaces the earlier `from_chain`-only walk-through, so the run command now needs `--features tls`.
  Guide 21 points at it instead of the source's tests.
- **Presence bands, the two edge cases 2.26.0 documented.** `mycelium wire-check` names two `[[presence]]` bands over
  the same capability, at least one with a ceiling, that are not provably disjoint (`presence bands overlap`, a
  warning) — the capped one sheds against its ceiling while the other's floor or ceiling brings a provider back. Not
  named: bands disjoint by an `eq` on one attribute to different values or by different `schema_id`s, one band however
  it is spelled (the runtime's identity — ranking ignored), and a floor at or below a ceiling over one population. And the rolling-upgrade dip is visible: when a band
  that was above its ceiling falls below it and the providers that left had no `prov-shed` mark (a stem older than
  2.26.0 withdrawing by its own draw, or a crash), a stem counts `mycelium_artifact_presence_unranked_departures_total`
  and warns, naming the band and the providers — remembered across a round at the ceiling (a dip often arrives in two
  steps), each departure once, for four advertise intervals, and excluding a peer whose mark may still be in flight; `capability-lifecycle.md` gives the two remedies. Tests
  `presence_bands_that_overlap_with_ceilings_are_named` and `an_unmarked_provider_leaving_a_band_below_its_ceiling_is_named`, `the_departure_watch_sees_a_dip_that_arrives_in_two_steps`,
  each seen failing first. Also: `capability-lifecycle.md` still said there was no shed metric.

### Changed
- **A test whose gate no CI build satisfies is refused before push.** `scripts/check-test-inventory.py` evaluated
  each test's `cfg` against a step's flags, so a root-crate test gated off `gateway` or `tls` looked covered by a
  `--no-default-features` step — but cargo unifies a crate's features through the dev-dependencies a test build
  compiles, and the root's turn both back on. The check now computes the features each step's test build enables,
  through `[features]`, every path dependency and the selected packages' dev-dependencies. Its first run found one:
  `a_tls_table_this_build_cannot_enforce_refuses_to_start` had never executed since it was written (2026-10-02). It
  now lives in **`mycelium-tls-free-tests`** — `mycelium` with `gateway` and without `tls`, run on its own in CI
  beside `mycelium-gateway-free-tests` — and passes. Mutation suite: 49 cases, the new one seen missed before, plus
  five synthetic-workspace unification cases; it now lists its cases and marks each as it runs, so the test-coverage
  job holds it to the script-suite rule (#557).
- **Diagnostics for #563 — the root cause is not found.** #561's CI failures read `not_found` on node B while node A
  held the blobs; neither explanation (B had not resolved A as a provider; A answered miss) reproduces locally — 30
  two-process starts mirroring CI and 20 in-process starts, both orderings, all saw A's capability on B within 0.2–4.8 s.
  So the next occurrence is made to say which: `MeshBlobStore::fetch` logs each miss at **debug** with its reason,
  whether the local copy was damaged, and each other provider asked with its answer (`node=miss` / `=corrupt` /
  `=unreachable(…)`, or none); `reason_node` installs a `tracing` subscriber (it had none, so every line from the crate was
  dropped) and CI starts it with `RUST_LOG=warn,mycelium_reason=debug`; the Python job keeps the nodes' logs as an
  artifact on failure. A guard, `tests/first_contact_capability.rs`, holds a capability advertised at start — before the
  peer starts, and after — to reach that peer within 15 s, well under one 30 s refresh.
- **Single example demonstrations are cases, and the examples matrix's CI column is checked.** The eighteen
  `cargo run … --example` steps in `ci.yml` were one exit code each, so a step that stopped running was invisible.
  Each now runs through `scripts/example-case.sh`, which prints `@@case@@ examples::<NAME>` and runs the command
  unchanged; its `--list` reads the calls from `ci.yml` (failing on one it cannot read, or on a `cargo run
  --example` it does not front), and the `test-universe` job lists them, so the `test-coverage` job fails on a
  demonstration that did not execute. `scripts/check-example-matrix.py` (in `make check` and CI; self-test
  `scripts/test-check-example-matrix.py`, 12 cases) reads `examples/README.md`'s matrix and fails on a ✓ row no CI
  step executes, and on an example CI executes whose row says · or that has no row — derived from the workflows,
  the scripts their steps run, and the suites' own listings. Its first run found two rows wrong the other way:
  `reason_node` and `reheal_node` run in every CI run (the LangGraph rungs' nodes, and the deploy/reheal rung's) and
  said ·; they now say ✓. A new cell, **✓ᵖ**, marks an example executed only by a path-filtered workflow (the Docker
  suites run on the changes that touch their paths, not on every change): `confined_fleet_node` was ✓ only through
  them and is now ✓ᵖ. The wrapper fronts only `cargo [+toolchain] run`; an example run inside a script or as a
  process is checked statically, not observed as a case of its own.
- **The test-coverage job sees script-style suites** (verification policy rule 3). A script suite now lists its
  cases with `--list` (`@@case-list@@ <suite>::<case>`, nothing run) and prints `@@case@@ <suite>::<case>` as each
  case starts; `scripts/ci-test-coverage.py` reads both as `script <suite>::<case>` keys. `ci.yml`'s
  `test-universe` job lists `scripts/test-check-materials.py`, `scripts/test-ci-test-coverage.py`,
  `scripts/test-ci-retest.sh`, `scripts/wire-check-examples.sh`, the seven `ci_smoke.sh` suites (the co-op's
  thirteen demos each a case) and the LangGraph ladder, so a case dropped from its script, or a step that stops
  running it, fails the job. The Docker suites (`cluster-suites.yml`: integration scenarios, overlay, federation
  legs, confined-fleet phases, stem-examples demos) list theirs before starting Docker and are checked by a new
  `case-coverage` job in that workflow (`ci-test-coverage.py --scripts-only --require <jobs>`). Every listing is
  now named (`@@test-universe@@ begin <source>`) and each expected one must be non-empty, rather than a count of
  blocks; a shell suite's list is derived from its calls by `scripts/list-script-cases.py`, which fails on a call
  it cannot read. No suite tests anything different.

### Fixed
- **The cross-node checkpointer test waited wrongly (#561).** `TestCrossNodeResume` polled node B for the thread head
  and failed on the first `IncompleteCheckpoint`. B could read the row before it could fetch the row's blobs — in both
  CI failures because it did not yet resolve node A as a blob provider until A's first 30 s capability refresh (the
  propagation gap is #563) — and that `not_found` is the documented retriable "not yet"
  (`examples/langgraph/03_cross_node.py` and `06_deploy_reheal.py` already waited through it). The poll is a helper now
  (`langgraph-checkpoint-mycelium/tests/converge.py`) that keeps waiting on a retriable miss and stops on any non-transient reason, with node-free
  tests seen failing first; `TestRowBeforeBlob`'s second poll re-raises a non-retriable one instead of spinning to its
  deadline. Test-only; no package change.
- **A blob fetch tells a refusal from a corrupt copy (#564; `mycelium-reason` 0.7.1).** A provider's RPC layer refusing
  the fetch read as a corrupt copy — non-retriable — so a transient refusal ended every checkpointer retry loop. The
  refusal is now read by its reason: a transient one (an unknown signer, a marker not yet seen, or none) is
  `unavailable` (503, retriable); one that holds (a removed member, a denied action, a bad envelope), from every holder
  that could be asked, is `refused` (403, which the checkpointer reads as `unauthorized`). For that, `rpc_rx`'s
  caller-context refusal is now JSON built with `json!` — an error text with quotes made it invalid JSON, read as
  corrupt — and carries `reason` (`CallerError::code`). See `mycelium-reason/CHANGELOG.md`.
- **The commit-conflict tripwire test assumed a local emit is a delivery (#568).** It emitted one forged COMMIT right
  after a consensus round — when the proposer's own COMMIT still sat in the listener's 256-slot queue (fill 1/256) — and a
  Cluster signal's local delivery is shed with probability equal to that fill, ~0.4%; it failed once under the strict gate. It re-emits until the tripwire
  fires and checks the idempotent COMMIT only once it is seen delivered (that half could pass vacuously). New
  `the_tripwire_holds_under_a_loaded_signal_queue` meets the forged frame's first emit with a 90%-full queue: the
  single-emit version failed it 3/3. `test_signal_group_admitted_when_member`, the same latent shed on an unstarted agent
  (fill 1/1024), re-emits too.
  Test-only; the tripwire itself was right.

## [2.27.0] — 2026-10-07

**The doors say what they own.** 2.26.0 recorded `kv:write` as a data-plane superuser scope over namespaces other
routes own and left it undecided; it is decided — the raw KV routes write application keys only, every owned
namespace answers **403** `protected_key` naming its own route, the log routes keep off streams a component owns,
and the topology override gets its governance route. The same reading of every door found an A2A card offering
plumbing and prompt skills `/a2a` cannot call, a governance route that neither refused a loose body nor audited,
and an audit that failed silently without an identity — each closed, with the SDKs naming the new refusals
(`mycelium-py` **0.2.7**, `mycelium-ts` **0.2.2**). Issue #168: every group's observed size on the fleet snapshot.
And the alignment sweep after it — doc-coverage run 21 and a wiki lint, which found nine instructions that failed
when followed. Every PR under the verification policy, each with an independent review answered in the PR. Wire
**v12** unchanged. A MINOR: behaviour changes with upgrade notes — read *Upgrade notes* in each entry below
(`deprecations.md` §19), and `FleetSnapshot` gained a field.

### Changed
- **The raw KV routes write application keys only** (the `kv:write` decision, 2026-10-07). `POST`/`DELETE
  /gateway/kv`, `POST /gateway/kv/quorum` and `POST /gateway/overlay/consistent/set` refuse **403**
  `protected_key` for a key in any namespace the substrate or a companion owns — 2.26.0 covered `sys/` and
  `consensus/`; now `grp/`, `cap/`, `req/`, `prompts/`, `log/`, `mailbox/`, `installable/`, `agent/`, the companions'
  and the rest of `src/lib.rs` § KV namespace ownership, each refusal naming the route that owns it. Under
  `kv:write` they could rewrite a prompt template (`llm:write`), enrol another node in a group (a node joins
  itself), forge a capability, a requirement or a mailbox sender, or append to a log (`consensus:write`). The
  checkpointer's `ckpt/`/`ckptw/` rows, the mesh manifest (`manifest/`, application-owned), the schema registry
  (`schemas/`, a non-Rust application's only door) and an application's `agent/{node}/provision/…` report stay
  writable. The log routes (`append`, `compact`) refuse a stream under `cn/`, `wiki/` or `reason/` **403**
  `protected_stream` — under `consensus:write` they could forge a reason trace or delete a commitment's offers. A
  test parses the ownership table, so a new namespace must be classified before it ships. **Upgrade note:** a
  client writing an owned namespace through the KV routes uses its route (`deprecations.md` §19).
- **The topology override has a governance route.** `POST /gateway/govern/topology-override {"group", "override"}`
  (`govern:write`, audited in a `compliance` build with `[tls]`) sets or releases `sys/topology-override/{group}`; it was an unaudited `kv:write`
  write, the one owned key 2.26.0 still let through.
- **The test-coverage job knows more tests** (verification policy rule 3). Its universe lists every TypeScript test
  by name — jest's `--json` report, skipped tests included — where it listed only files, and every workspace
  crate's own featureless build, so a test gated `not(feature = …)` outside the root crate is known. A test known
  and never executed fails CI as before.

### Fixed
- **The SDKs name the KV and log refusals** (`mycelium-py` **0.2.7**, `mycelium-ts` **0.2.2**). Since the raw KV
  routes write application keys only (above), `set`, `delete`, `set_with_min_acks`/`setWithMinAcks` and
  `consistent_set`/`consistentSet` raise **`ProtectedKeyError`** (`.key`) for the gateway's `403 protected_key`, and
  `append` and `compact_log`/`compactLog` raise **`ProtectedStreamError`** (`.stream`) for `403 protected_stream` —
  each carrying the gateway's `message`, which names the route to use. They raised a bare HTTP error before, whose
  text did not include it. In Python both are a `PermissionError` (as `ProtectedKindError` is) **and** an
  `httpx.HTTPStatusError`, what these verbs raised before, so an existing `except` of either kind still catches
  them. Every door enumerated against `src/agent/http.rs` (`refuse_protected_key`: four handlers;
  `refuse_owned_stream`: two) and tested by verb, with a plant: an ordinary scope 403 stays the error it was. Two
  siblings the enumeration found: the TypeScript `delete()` threw `DELETE /gateway/kv failed: 403` and dropped the
  body — it now goes through the shared error path and carries it; and the Python `emit_reliable` raised a bare
  HTTP error for `403 protected_kind` where its five sibling routes raised `ProtectedKindError` — it raises it now.
  Docs: both READMEs gain an Errors section; their caller-identity paragraph named `llm_*` / `llm*` verbs that do
  not exist (the call is `PromptSkillClient.call`); the Python Introspection section lists `agent.node_id`; and the
  `langgraph-checkpoint-mycelium` README no longer calls `IncompleteCheckpoint` retriable unconditionally (only
  `not_found` and `unavailable` are) and states the upgrade order (checkpointer 0.3.0 before `mycelium-reason`
  0.7.0). Fail-first: seven Python and seven TypeScript tests failed against the unwired SDKs
  (`mycelium-py/tests/test_protected_key.py`, `mycelium-ts/tests/protected_key.test.ts`), and
  `test_emit_reliable_raises_it_too` against 0.2.6. **Behaviour changes to check:** in Python the new errors are
  `OSError`s (through `PermissionError`), so an `except OSError` retry loop now catches — and could retry — a
  permanent refusal, and `str(e)` is the gateway's message rather than httpx's status-and-URL text (`e.response`
  still has both); `emit_reliable`'s `ProtectedKindError` is a `PermissionError` only, like its siblings', so an
  `except httpx.HTTPStatusError` around it no longer catches a `protected_kind` 403. In TypeScript every DELETE
  (KV delete, capability drop, lock release) now fails through the shared path: the message is
  `DELETE <path with query> failed: <status> <body>` — the key appears in it — and a 504 is a `TimeoutError`.
- **The A2A agent card lists what `/a2a` can call, and `tasks/send` refuses the rest.** `/.well-known/agent.json`
  listed every advertised capability as a skill, so an external agent was offered `prov-shed/…` (2.26.0's shed
  marks), `{ns}/loading`, `{ns}/installable`, `llm-meta/…`, `artifact/librarian`, `reason/blob-cache` and the
  companions' role marks (`{ns}/{x}.primary`/`.secondary`/`.candidate`/`.curator`) — none of which answers
  `skill.invoke` — and **prompt skills** (`mycelium-reason`'s `llm/{model}`, a stem's `[[serve]]`), which answer
  `llm.invoke`, so a call through `/a2a` failed or waited out its 120 s timeout. They are left out of the card,
  and `tasks/send` (and its stream) refuses them `-32001`, naming `POST /gateway/llm/call` for a prompt skill —
  routing one through `/a2a` would reach a model without `llm:invoke`. Before, a guessed id resolved a provider
  and dispatched `skill.invoke` there, where a co-hosted receiver could run under the guessed name. Test
  `agent_card_leaves_out_infrastructure_capabilities`, seen failing first.
- **Every governance write at the gateway is audited, and one that is not is counted.** `POST /gateway/govern/profile` neither
  refused a loose body (a `target` was dropped and the step applied to the node answering) nor recorded the change
  — every other governance route did both; it now answers **400** on a non-object body or an unknown field, and is
  audited. `audit_govern` discarded a failed seal (a `compliance` build without `[tls]` records nothing): a change
  that leaves no audit record — no `compliance`, or a seal that failed, now warned about — is counted, and `/stats`
  carries `governance_changes` and `governance_unaudited` — accepted governance POSTs at this gateway
  (`/gateway/govern/*` and `POST /gateway/identity/revoke`, which is now audited too; a refresh of a standing intent
  or a no-op profile step counts), not changes an embedder makes through the Rust API. A test enumerates every
  `"/govern/…"` route literal in the gateway's source and requires one audit attempt per accepted change and none
  for a refused body; with `compliance` and `[tls]`, each change, the revocation included, is sealed. Fail-first:
  `govern/profile` recorded no attempt and accepted `target`. Found by the 2026-10-07 wiki lint and doc-coverage
  run 21.
- **The rule catalogue's inputs render.** An input whose source carried its own code spans was wrapped in another,
  breaking 25 inputs across the generated `docs/reference/rule-catalogue.md`; such a source now renders as prose.
  Test `an_input_with_its_own_code_spans_renders_unwrapped`, seen failing first.

### Documentation
- **Doc-coverage run 21 and a wiki lint over the window (#554).** Nine instructions failed when followed — guide
  05's prompt-skill and skill-call snippets (`agent.llm()`, `PromptSkillClient`, and `A2aClient` where `rpc_call`
  has answered 403 `protected_kind` since 2.15.0), guide 08's Python, five runbooks' governance `curl`s without
  `Content-Type` (415), `rbac.md` advising a scope for a `protected_key` 403, a 12-character blob id the route
  rejects; the card's "every capability" corrected everywhere; ops landings for presence ceilings and strict
  eligibility; the coop CI job's demo count (13); `CONTRIBUTING.md`'s gate. The wiki-lint skill gains four
  sharpenings from its ledger.

### Added
- **Every group's size on the fleet snapshot (#168).** `fleet_snapshot().group_sizes` and `group_sizes` on
  `GET /gateway/fleet`: one `{group, observed}` per group with a live member, governed or not — an ungoverned
  group (`join_group`, or a capability group with no membership intent) had no operator-visible size. Pull-based,
  with no per-group Prometheus label (the cardinality trap the issue names); a group nobody belongs to is absent,
  never `0`. **Upgrade note:** `FleetSnapshot` gained a field (an exhaustive struct literal breaks).
- **`mycelium_artifact_presence_sheds_total`** — installs a node withdrew because a `[[presence]]` band was above its
  ceiling and it ranked beyond it (`prov.shed`); before, a withdrawal showed only in the decision trace.

## [2.26.0] — 2026-10-07

The **verification policy** release. Doc-coverage run 20 found six code gaps behind green CI and repeated
analysis; the answer was four rules (enumerate every entry point before fixing, plan rows close on evidence,
CI collects tests by discovery, an independent adversarial review after each PR) — with rule 3 *observed* by a
CI job that fails on any known test that did not execute — and the six gaps closed under them: S5's
distinguishable blob reasons (`mycelium-reason` **0.7.0**, `langgraph-checkpoint-mycelium` **0.3.0**), A3's
history source, the timing door and every door to it, cooling-off's order, one `with_egress` shape
(`mycelium-py` **0.2.6**); and #545, a presence ceiling that shed every host at once. Wire **v12** unchanged. A
MINOR: new public API (`mandate::history_source`, `election::rank`, `BlobMiss`/`LocalRead`) and behaviour
changes with upgrade notes — the KV doors refuse `sys/`/`consensus/` keys **403**, the governance routes refuse
loose bodies **400**, the companions' status codes; read *Upgrade notes* in each entry below.

### Added
- **A3's history source, built** (`docs/plans/realignment-repairs.md` A3 — 2.24.0 shipped the evaluator and
  marked the row merged with its source unbuilt; doc-coverage run 20, code gap 3). `mandate::history_source::
  history_from_appointment_stream` (`tls`) builds a `TermHistory` from an appointment stream in the knowledge
  layer, pinned by `(issuer, stream)`: it walks back from this node's reader's checkpoint along each head's `prev`
  digest — the starting head under a current key, older heads under any key the issuer held, since the
  current-key signature fixes them by hash — so a presenter's other heads (another member's stream, a newer
  head, duplicates, any order) change nothing and the read moves no checkpoint. A linked head whose record
  cannot be fetched is a gap, and so is a repeated term; reaching `prev: None` is genesis; a
  `StreamOrigin::Baseline` binds only at the head digest it names; a stream the reader saw fork since it was
  opened vouches for nothing. "Current" means as of this reader's checkpoint (`AppointmentStream::from_reader`,
  `from_parts`). Before, nothing in the substrate built a `TermHistory` — the embedding application did, so
  "the source establishes coverage" rested on the caller. `HandoverJournal` records its scope (`for_scope`),
  and says which terms it covers (`start()`, `endpoint()`, read from its entries; a journal saved before
  loads with no scope). `mycelium-wiki/examples/curator_handover.rs`
  withholds the re-appointment until the successor has read the journal and the history reaches the current
  head, and shows the successor's write refused at the resource meanwhile. Fail-first: with the reader bypassed,
  heads signed by a key the authority does not hold vouched for six terms; the plan's two witnesses (a missing
  appointment record is `Unknown`, its return decides; terms 20–30 decide consecutive terms, not cumulative
  tenure) pass on the real source.
- **The verification policy** (`CLAUDE.md` § Verification policy; `docs/wiki/dev/testing/verification-policy.md`):
  enumerate before fixing, plan rows close on evidence, CI collects tests by discovery, an independent
  adversarial review after each PR — adopted after doc-coverage run 20 found six code gaps behind green CI. Rule
  3 is **observed**: the `test-coverage` CI job reads the run's own logs and fails on any test it knows of
  (logged, or listed by the new `test-universe` job and `jest --listTests`) that did not execute, against a
  reasoned exceptions file; `scripts/check-test-inventory.py` is the fast pre-push approximation (in `make
  check`, with a 48-case mutation suite). What it found running: `mycelium-core`'s `tls` unit tests, five
  wasm-host and binary targets, the root doctests (one no longer compiled) and the Python live and node-free
  files ran in no step; they run now. The PR template, `RELEASING.md` § 5b and `/adversarial-review` carry
  the rules.

### Documentation
- **Doc-coverage run 20** (`docs/analysis/doc-coverage.md`): the realignment repairs' operator and developer
  landings — persistence start refusals with their messages and actions, every `start()` refusal by field, the
  live timing tunables and their route, the object-store endpoint rule, the discovered-JWKS case in SSO, the
  SDK quick starts that subscribed after emitting, a configuration-ownership row; six code gaps recorded.
- **Correction to 2.24.0's Documentation entry:** it said the guarantee, domain and control profiles were told
  apart. What shipped named the guarantee (startup), control and consensus profiles; the **domain profile**
  joins the table in run 20, which now names four things called *profile*.

### Fixed
- **A presence ceiling sheds the surplus, not every host (#545).** Above `max_providers`, each hosting stem
  drew to withdraw at its bring-up probability, so at `self_elect_p = 1.0` every host withdrew at once: the
  count went max+1 → 0 → max+1, round after round — measured on `first_stem_fleet`, where two of four runs
  oscillated for several seconds and one CI run's snapshot caught three providers. The hosts ranked beyond
  `max` by the band's rendezvous order withdraw now (`mycelium::election::rank`, new), so hosts with the same
  view agree that exactly the surplus leaves. Only providers that will act are ranked: a stem advertises
  `prov-shed/{ns}:{name}:{hash}` for each band it supervises with a ceiling from the band's first round (the hash
  over the filter's gossip encoding and the ceiling, so bands on one capability are told apart), so a herd of new
  installs is marked the moment it goes live and lands exactly on the ceiling; it retracts the mark only while it
  provides the band without an install it could withdraw. A provider that does not advertise it — registered in
  code, a group projection, a stem without the band — keeps its place once it has been seen unmarked for two
  advertise intervals (until then it is presumed to shed, since a `cap/` entry and its mark can arrive apart — a
  partition healing — and counting it fixed too early is what over-sheds), and the rest rank against what is
  left, so it cannot hold the band above its ceiling for more than those few seconds — each time the band rises above it. Bound: a view with *k* wrong
  entries (a crashed provider still advertised, a mark not yet arrived) can leave the band at `max − k` for a
  round — zero if *k* ≥ `max`, which the floor then refills — and it never cascades. Overlapping bands (one
  unattributed, one attribute-restricted, both with tight ceilings) can still trade a provider back and forth. A host now withdraws
  whichever install it holds for the band, read from the install, so an entry since removed from the catalogue
  no longer leaves it unable to shed. **Rolling upgrade:** a stem still on the old code advertises no
  `prov-shed` and still sheds by its draw, so while old and new stems share a band it can dip below the
  ceiling until the floor refills; upgrade a band's stems together where that matters. The same probe settles at the ceiling within ~1.3 s, never
  returning to zero. `prov.shed` is catalogue revision 2 (staying within is `NoAction`
  `ranked_within_ceiling`, and the trace records the rank and the unmarked count); the shed no longer draws, so a recording has one
  draw fewer per host per round spent above the ceiling. The example also snapshots only once
  the observer's providers are the hosting stems and unchanged for a declare-and-tick cycle.
- **Strict eligibility: a consecutive-terms run at the limit behind a stale head is `Unknown`, not `Ineligible`.**
  `eligible_strict` decided a visible run at the limit before checking that the history reached the head — but
  a later term by someone else breaks the run, so the candidate may be eligible. Missing *earlier* history only
  lengthens a run, which is why the shortcut holds at the head; cumulative tenure keeps its shortcut (later
  history can only add). Found by the A3 example: the council's history lacked term 2, A's, which broke
  B's run, and B read as `Ineligible`. Test `a_run_at_the_limit_behind_a_stale_head_is_unknown_not_ineligible`,
  seen failing first.
- **The recorded code gaps of doc-coverage run 20, closed under the verification policy** (enumerated entry points,
  tests seen failing first, an independent review):
  - **R9's third door.** `POST /gateway/govern/timing` and `GossipAgent::govern_timing` published any value, and
    every node's reconciler ignored one outside `validate()`'s bounds, so the route answered `published: true`
    for an intent that governed nothing. Both refuse it now (`TimingIntent::check`, at the one publish path they
    share); the route also refuses a field that is not a non-negative integer, which it read as `0`
    ("ungoverned"). Its reviews found more doors to the same invariant: an intent that governs neither setting
    (it *replaced* the previous one while changing nothing) is refused by both; all three `govern/*` routes
    refuse a non-object body, an unknown field, a value of the wrong type, and a `target` that is not a
    node-id string (read as no target — the whole fleet), and membership a missing `min` (read as 0); timing's
    publishes are audited as its siblings' are. And the four doors that take a KV key from the request —
    `POST`/`DELETE /gateway/kv`, `POST /gateway/kv/quorum`, `POST /gateway/overlay/consistent/set` — refuse
    every key under `sys/` and `consensus/` **403** `protected_key`, except the operator's
    `sys/topology-override/{group}`: under `kv:write` or `consensus:write` they could publish a governance
    intent without `govern:write`, plant the `sys/caller-context/` marker a secure gateway checks before
    dispatch, delete an acceptor's durable ballot, forge a committed slot, or rewrite `sys/config/` and
    `sys/capauthz/`. Within `sys/` and `consensus/` it is an allow-list, so a new key there is covered the day it is
    added. **Not claimed:** a peer writing these keys through Layer I (detection, not prevention); and the other
    route-owned namespaces — `prompts/`, `log/`, `grp/`, `cap/`, `mailbox/`, `installable/`, the companions' —
    stay writable under `kv:write`, which `rbac.md` now states as a data-plane superuser scope (a decision
    recorded, not made here); the topology override stays a `kv:write` write, unaudited. **Upgrade notes:** `govern_timing`
    returns `Result<bool, GossipError>`; a client writing `sys/` or `consensus/` keys through the KV routes, or
    sending the governance routes loose bodies, now gets 403 / 400; `with_egress` callers that relied on deref
    coercion or inference spell the type (`deprecations.md` §18).
  - **Cooling-off decides a visible term regardless.** A candidate visibly inside the cooling window read
    `Unknown` behind a stale head, asking for history that could only confirm the "no"; it is `Ineligible`, as the
    other two limits already were.
  - **One `with_egress` for four clients.** `OpenAiBackend::with_egress` took a reference and `OllamaProbe`,
    `HttpLibrarySource` and `FederationClient` a value; all four take `impl Into<EgressPolicy>`
    (`From<&EgressPolicy>` added), so `agent.egress_policy()` works everywhere.
  - **One signal-stream shape.** `GET /signals/{kind}` gains `payload_b64` and `nonce` beside its original
    `payload`, matching `GET /gateway/signal/sse/{kind}`.
  - **`mycelium-py` 0.2.6:** `MyceliumAgent.node_id` (the TypeScript SDK's `nodeId`), read once from `/health`; the README's
    leader-election example used it and raised `AttributeError`. `LockGuard`'s docstring named a method that does
    not exist.
- **Why a checkpoint blob is missing stays distinguishable** (`docs/plans/realignment-repairs.md` S5 — the half
  2.24.0 did not build: *"absence, temporary unavailability, authorization refusal and corrupt content stay
  distinguishable in the error"*; doc-coverage run 20, code gap 2). **`mycelium-reason` 0.7.0:** the blob tier's
  `MeshBlobStore::fetch` returns a `BlobMiss` — `NotFound`, `Unavailable` (a provider unreachable), `Corrupt` —
  and `GET /gateway/reason/blob/{id}` answers 404 / 503 / 502 with the reason in its body; it answered 404 for
  all three. **Corrupt** only when every copy currently on offer fails its content address — the local copy and
  every **advertised** provider's reply — with no advertised provider unreachable and none merely lacking it (a
  holder offline past its 30 s capability lease is not counted, so it cannot hold the answer at retriable), so one faulty or hostile provider
  cannot turn a blob that is still spreading into a permanent failure. **Damage at rest** is corruption, not
  absence: `FsBlobStore::read` returns `LocalRead::Damaged` for a copy that fails its address or cannot be read
  (EIO, EACCES), and the stock blob server answers a damaged copy — even one truncated to nothing — with a marker
  that fails every content address, instead of the empty *miss* (it read as `NotFound`, retriable forever); a
  holder still on 0.6.x answers the miss as before, so in a mixed fleet its damage reads as `not_found` until it
  upgrades. `FsBlobStore::put` of the right bytes **repairs** a damaged copy, where it returned early because a
  file existed; an existing copy is now read and compared on every put (no second hash).
  **`langgraph-checkpoint-mycelium` 0.3.0** (a MINOR — three behaviours change against 0.2.0: a 401/403 on a blob raises a
  non-retriable `IncompleteCheckpoint` where it raised `httpx.HTTPStatusError`; a 5xx, 408 or 429 raises a
  retriable one where it raised `HTTPStatusError`; and a bare 404 — a node without the reason companion — is
  `unsupported`, non-retriable, where 0.2.0 treated it as a missing blob and kept retrying)**:** `IncompleteCheckpoint.reasons` names each missing blob's reason,
  read from the route's **body** as well as its status — a proxy's 502 page is `unavailable`, not `corrupt`;
  a bare 404 (no reason companion on that node) is `unsupported`; 408/429 are `unavailable`; a 401/403 is
  `unauthorized` (it escaped as a raw HTTP error) — a blob fetched twice keeps its most serious reason, and
  `retriable` is false for `corrupt`, `unauthorized` and `unsupported`. The example rungs' polls re-raise a
  non-retriable error. Fail-first: a two-node test with a provider serving the wrong bytes answered `404
  not_found`; a blob damaged on its only holder's disk answered `404`; one bad provider beside an honest one
  answered `502 corrupt` (the PR's own adversarial review found the last two); twelve checkpointer tests failed
  against 0.2.0; a live two-node test makes the row arrive before its blob and asserts the raise.
  **Upgrade order:** upgrade `langgraph-checkpoint-mycelium` to 0.3.0 **before** the reason nodes to 0.7.0 — a 0.2.x
  checkpointer reads only 404 as a missing blob, so a 503 or a 502 from a 0.7.0 node escapes its retry loop as an
  `httpx.HTTPStatusError`. `mycelium-py`'s `ReasonClient.blob_get` returns `None` only for 404 and raises for
  502/503, whose body names the reason.
- **The CI flake tier (`scripts/ci-retest.sh`) passed steps that had not run their tests.** Without
  `--no-fail-fast`, a flake in one test binary stopped cargo before later binaries and the isolated retry ran
  only the flaked test — the step went green with them unexecuted (the test-coverage job's first run:
  `mycelium-reason`'s `tests/ollama.rs`). With it, a binary that crashed printed no failure list and went green
  beside a retried flake; and a failing doctest's name was cut at its first space, so its retry ran nothing and
  passed as a flake. Now: `--no-fail-fast`, a crashed binary fails the step, the whole name is retried, and a
  retry passes only if it ran a test (`scripts/test-ci-retest.sh`, each case seen failing first).
- **`mycelium-ts` 0.2.1 — two verbs that did not match their gateway handlers, found by sweeping every
  SDK verb's request and response against its handler.** `scatterGather` sent the method under `kind`;
  `gw_scatter` reads `method` and answered **every call 400 `missing method`** — since the client was
  written (2026-05-25). The same mistake in `rpcCall` was fixed in 2.15.0 (#409) and this one, the next
  function down, was not; nor was it caught in 0.2.0 (#526), which changed this function's timeout line
  and tested it with a mocked `fetch` that answers any body. `rpcServe`'s `RpcRequest.kind` read a `kind`
  the serve stream's data does not carry, so it was always `undefined`; it now comes from the event name,
  as `onSignal`'s does. And a gateway **504** (`rpcCall`, `scatterGather`'s expired deadline) is now an
  error named `TimeoutError`, as the README promised and the Python SDK raises, where it was a plain
  `Error`; `sseStream`, `SseOverflowError` and `SseOptions` are exported from the package root. Fail-first:
  four contract tests failed against 0.2.0 (`tests/contract.test.ts` § *verbs the review did not reach*);
  the live suite gains the round-trips a mock cannot prove (`scatterGather` to a served kind, `rpcServe`'s
  kind, `onSignal` after subscribing, `subscribeLog` resuming after an HLC, an unserved RPC as
  `TimeoutError`). The Python SDK's requests and responses matched every handler.
- **The 2.24.0 entry overstated what `mycelium-ts` 0.2.0 verified.** *"The TypeScript SDK matches the
  gateway it talks to"* held for the verbs the review named and the live suite exercised, not for every
  verb; the two above were outside both. The README's claim that a slow consumer "gets
  `SseOverflowError` rather than a silently truncated stream" was also wrong: the node drops past 256
  undelivered signals per subscription and the client is not told — corrected in the README, with a
  migration table from 0.1.x.

---

## [2.25.0] — 2026-10-05

The realignment repairs' last two rows (`docs/plans/realignment-repairs.md` R8, R9 — #534, #535), both
found by A2's configuration audit; with them every row of the plan is closed. Wire **v12** unchanged.
A MINOR rather than a PATCH: R9 changes two public signatures and R8 adds a start-time refusal.

### Fixed
- **The live timing setters keep `validate()`'s bounds** (`docs/plans/realignment-repairs.md` R9,
  found by A2's configuration audit). `set_health_check_interval_secs` and `set_reconnect_backoff_secs`
  stored any value — a health interval of a day, a reconnect backoff past five minutes — and pinned
  the node against the fleet's `TimingIntent`, while `validate()` and the timing governor both refused
  them. A value above 3600 or 300 is now `InvalidField` naming the field, and changes nothing: neither
  the live value nor the local pin. `0` still reverts to the static value. Fail-first on `7ae0ea69`:
  `hot_timing_setters_refuse_what_validate_refuses` — the setters stored 3601 and 301.
  **Upgrade note:** both setters now return `Result<(), GossipError>`; a call that ignores it compiles
  with an `unused_must_use` warning, which `-D warnings` turns into an error.
- **A gateway setting in a build without `gateway` refuses the start, and the gateway guarantees say
  why** (`docs/plans/realignment-repairs.md` R8, found by A2's configuration audit). Such a build runs
  no HTTP server, yet it accepted an `http_port` and a `[gateway_tls]` table and ignored them — and
  still advertised `http_port` as its bulk-transfer port, which nothing served — while the guarantee
  report resolved `gw.not_open`, `gw.tls` and `gw.caller_profile` from the configuration, so a node
  with a token reported `gw.not_open: enforced` for a gateway that did not exist. `start()` now refuses
  `http_port` and `gateway_tls` by name, the class of the v2.18.1 refusals, and those guarantees read
  `not_in_build` (feature `gateway`). **No test had ever run against a gateway-free build:**
  `mycelium`'s dev-dependencies enable `gateway`, so its own test build always has it. The new
  test-only crate `mycelium-gateway-free-tests` is that build, run in the CI job named for it and in
  `make check-full`. Fail-first on `7ae0ea69`: the node started with an `http_port` and `gw.not_open`
  read `enforced`. **Check before upgrading:** a gateway-free build given an `http_port` — including
  `skillrunner` built without default features — now fails at start; it was serving nothing on it.

---

## [2.24.0] — 2026-10-05

The realignment repairs' second MINOR (`docs/plans/realignment-repairs.md` pieces 2 and 4, with R7).
Wire **v12** unchanged.

### Added
- **A signal SSE event's data names its kind** (`docs/plans/realignment-repairs.md` §3.2, the S2
  row's deferred gateway half). `GET /gateway/signal/sse/{kind}` and `GET /signals/{kind}` now put
  `"kind"` in each event's JSON data as well as the SSE event name, so a client that reads the body
  alone is not wrong. Additive: the event name and every other field are unchanged, and wire **v12**
  is unaffected.
- **Strict eligibility: unknown history is never eligible** (`docs/plans/realignment-repairs.md` A3,
  decision D4). `mandate::eligibility::eligible_strict` answers each configured incumbency rule as
  `Eligible`, `Ineligible` or `Unknown` (naming what is missing), from a `TermHistory` whose coverage
  its **source** established — chained terms verified at construction, an `Origin` (the role's first
  term or a trusted baseline), the current head — never from a caller's assurance. Consecutive terms,
  cumulative tenure and cooling-off each need their own coverage. `eligibility::ready` gates a
  successor on both the handover journal read and every rule decided in favour. Additive:
  `handover::eligible` keeps its stated contract. `examples/strict_eligibility.rs` shows a candidate
  the lenient check admits on a history one term short, held at `Unknown` by the strict one, then
  refused once the term is back.

### Documentation
- **"No control plane" defined, and the three things called "profile" named apart**
  (`docs/plans/realignment-repairs.md` A1, decision D2, #528): the phrase is kept and defined
  wherever it appears — control decisions live in participating nodes (governors and provisioners act
  locally, subject to configured consensus and resource-side authority checks), and no separate
  control-plane service is required — and the guarantee profile, the domain profile and the control
  profile are told apart across the positioning page, the concepts chapter, the control-profiles
  runbook and guide 22.
- **Who owns each configuration field, and what each cross-layer hook may do** (A2, #530):
  `docs/reference/configuration.md` gives every `GossipConfig` field its component, the feature that
  enforces it, where an invalid value is refused, whether a change needs a restart, and its default and
  environment variable; `docs/wiki/dev/architecture/hooks.md` states each cross-layer hook's contract.
  It is the information a configuration split would need; the split itself stays deferred (decision D3).

### Fixed
- **A persistence directory that cannot be created refuses the start instead of running in memory**
  (`docs/plans/realignment-repairs.md` R7; found by the configuration audit behind A2). `start()`
  logged the failure and carried on with no persistence, so every write was lost on the next restart,
  while the guarantee report — resolved from the configuration alone — still said
  `persist.configured: enforced`, and `secure-single-domain` let the node start. Now it is
  `InvalidField { field: "persistence" }` naming the directory, the class of the v2.20.0 refusals.
  Fail-first on `f67b3f37`: `a_persistence_directory_that_cannot_be_created_refuses_the_start` (the
  directory under a regular file) — the node started. **Check before upgrading:** a node whose
  persistence directory could not be created now fails at start; it was running without persistence.
- **`mycelium-ts` 0.2.0 — the TypeScript SDK matches the gateway it talks to**
  (`docs/plans/realignment-repairs.md` S1–S4; the review's F06–F09). Run against a real node, the
  SDK's own live suite failed 9 of its 19 assertions, and it had never run in CI. Every defect was
  on the SDK side — the Python SDK reads the same routes correctly — so the wire is unchanged:
  **64-bit values are exact** (responses are parsed losslessly, so an HLC above 2⁵³ is no longer
  rounded — two HLCs one tick apart used to compare equal — and a signal nonce or lock token is
  exact; a `bigint` in a request body is written as an exact integer, so `compactLog` stops being
  refused with 422); **the shapes match** (`get` returns `null` for an absent key instead of
  throwing; `resolveCapability` returns the providers array, not the envelope; `emit` returns the
  gateway's `ok` instead of `undefined`; `emitReliable` reads `ack` and throws a refusal instead of
  returning `undefined`; a signal's `kind` comes from the SSE event name); **the log verbs use the
  gateway's names** (`scanLog` sends `from`/`to` — the bounds were ignored — and reads the bare array;
  `subscribeLog` sends `since` — every resume replayed from zero); **timeouts are whole seconds**
  (`emitReliable` refused a fraction with 422; `rpcCall` and `scatterGather` silently replaced one
  with a 30 s / 10 s default); and **an SSE stream has a lifetime** (`sseStream` reads on demand and
  cancels the reader and aborts the request when the loop ends; a consumer more than `maxPending`
  events behind gets `SseOverflowError` instead of an unbounded buffer). Fail-first on `989cf4bd`:
  `tests/contract.test.ts` (node-free, a mocked `fetch` answering each handler's exact shape) failed
  13 of 14; the live suite failed 9 of 19. Both pass now (20 live, including a real `emitReliable`
  timeout), and **the live suite runs in CI** against a `mycelium` node in the `sdk-ts` job. Three
  live expectations were stale and are corrected in the same PR: an unknown target is refused before
  dispatch (412 `provider_without_caller_context`) rather than timing out, and an election over an
  empty group is refused by name (409 `electorate_unavailable`).
- **`langgraph-checkpoint-mycelium` 0.2.0 — an incomplete checkpoint raises instead of reading as
  absent or smaller** (`docs/plans/realignment-repairs.md` S5; the review's F10). A checkpoint's index
  row gossips in before every blob it references is fetchable, and the loaders answered that gap two
  wrong ways: a missing pending-write blob was dropped, so the tuple came back with fewer
  `pending_writes` and LangGraph re-ran a task that had already completed (losing an
  `__error__`/`__interrupt__`/`__resume__` write the same way); a missing skeleton or channel blob
  returned `None`, which LangGraph reads as *no checkpoint* and restarts the thread. `get_tuple`,
  `aget_tuple`, `list` and `alist` now raise `IncompleteCheckpoint` — retriable, naming the missing
  blob ids — and `None` means only that no checkpoint row exists. Fail-first on `989cf4bd`:
  `tests/test_incomplete.py` (node-free, an in-memory gateway behind `httpx.MockTransport`) — a
  missing pending-write blob, a missing channel blob by id and as the latest, and the async loader —
  each did not raise. **Callers that poll across nodes must catch it:** the LangGraph ladder's
  cross-node rungs (03 and the 06 flagship) now treat `IncompleteCheckpoint` as *not converged yet*
  and retry — the first CI run of this change failed the flagship exactly there, with three blobs of
  the gossiped-in checkpoint not yet fetchable on node B, which the old reader would have answered
  with `None` or a shorter tuple. **Also:** `mycelium-py`'s live gateway suite (`test_gateway.py`) now runs in CI
  against a `mycelium` node (S4's Python half); it had never been in the job's list, and passes (19,
  1 expected failure) — the Python SDK reads every route the TypeScript SDK got wrong correctly.

---

## [2.23.0] — 2026-10-05

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.22.0 holds. A **MINOR**, not the PATCH `realignment-repairs.md` named: the repairs add public API — `mycelium::egress_client`, `EgressPolicy::redirect_verdict`, `OpenAiBackend::with_egress`, `OllamaProbe::with_egress`, `persistence::OwnershipLock`, `WalHandle::hold_ownership`, `mycelium_wasm_host::dial_hosts` — and a `WalMsg` variant (`RELEASING.md` §1). **Check before upgrading:** `rust-version` is now
**1.89** (`std::fs::File::try_lock`); two agents sharing one journal path, or one journal opened twice
in a process, now get `WouldBlock` naming the path on the second open — a configuration that was
silently corrupting its journal. **And:** an outbound client now re-checks every redirect
against the egress allow-list, follows at most five, and never downgrades `https` to `http`; a
deployment whose MCP, LLM or OIDC endpoint redirects to a host off its allow-list now gets a named
egress refusal — that request was leaving the allow-list before.

### Security
- **The object-store fetcher gates on the endpoint it dials, not the bucket name**
  (`docs/plans/realignment-repairs.md` R3b; the review's F03, object-store row). The gate checked
  the store URL, and for `s3://bucket/…` that URL's host is the bucket name, which nothing dials: a
  bucket named like an allowed host passed, and `object_store` then connected to
  `s3.<region>.amazonaws.com` or `AWS_ENDPOINT`. `mycelium_wasm_host::dial_hosts` now derives the
  endpoint from the URL and the same option keys `object_store` reads: an explicit S3 endpoint
  (`AWS_ENDPOINT_URL_S3` first), else the regional host with path-style or virtual-hosted
  addressing; `storage.googleapis.com` or `GOOGLE_BASE_URL` for GCS; the URL's host plus any
  endpoint option for `http(s)://`; nothing for a local store. An endpoint the adapter does not model
  — Azure, S3 Express — is refused under a non-empty allow-list rather than guessed at. Fail-first on
  `d9415708`: `a_bucket_named_like_an_allowed_host_does_not_open_a_store_on_another_endpoint`
  opened the store. **Check before upgrading:** a stem or `mycelium-artifact` reading an
  `s3://`/`gs://` library under a non-empty allow-list must list the endpoint host
  (`.amazonaws.com`, `storage.googleapis.com`, or the MinIO host) — listing the bucket name never
  gated anything. **Not covered:** the cloud identity's credential traffic (instance metadata, STS)
  and redirects inside `object_store`'s own client, which this adapter does not build.
- **The egress gate reads a URL's host the way the HTTP client does** (`docs/plans/realignment-
  repairs.md` R4; found while verifying the review's F03, and stronger than it — no redirect is
  needed). `host_of_url` split the authority on `/?#` and took what followed the last `@`; reqwest
  parses with the WHATWG `url` crate, which reads a backslash as `/` in an http(s) authority,
  strips tabs and newlines, and maps IDNA forms. So `http://evil.example\@allowed.example/` was
  allowed as `allowed.example` and dialled as `evil.example`, and `http://evil.example\.allowed.example/`
  passed a `.allowed.example` suffix entry the same way — for every caller of `permits_url` that hands
  the same string to a client. `host_of_url` now parses with `url` (already in the lockfile through
  reqwest); a string with no `://` is read as `http://…`, and an IPv6 literal still comes back
  without brackets. One visible change: `http:///nohost` now yields `nohost`, the host the client
  would dial. The wiki's `GitMirror`, which splits a git remote by hand and hands it to git (and so to
  curl or ssh), now refuses by name a remote whose authority holds a backslash, whitespace or a
  control character. Fail-first on `55eb4edd`: `the_gate_reads_the_host_the_client_dials` (every
  vector compared with the client's own parse) and
  `a_backslash_in_the_authority_does_not_smuggle_a_denied_host_past_the_gate` (the MCP bridge connected
  to a denied listener through `http://127.0.0.1:{port}\@localhost/` with only `localhost` allowed)
  each failed; `a_remote_whose_authority_two_parsers_could_read_differently_is_refused` likewise
  against the unfixed mirror. **Still not claimed:** resolution — the gate compares names, so an
  allowed name that resolves to an address the operator meant to deny is not caught.
- **A redirect is now checked against the egress allow-list like the first URL**
  (`docs/plans/realignment-repairs.md` R3; the external review's F03, P1). The allow-list gated the
  first URL of each outbound call, and every client then followed reqwest's default ten redirects
  wherever they led — so an allowed MCP server received the node's `initialize` and `tools/list`
  and then forwarded them to a denied host, a 307 from an LLM endpoint re-sent the prompt, a
  redirected JWKS fetch made the gateway trust keys served by a denied host, and the federation
  client's `x-mycelium-federation-call` credential (a header reqwest does not strip cross-host)
  travelled to whatever host a partner named. Now one builder, `mycelium::egress_client`, gives
  every client in the crate one of two rules: with the node's policy, every hop goes through
  `EgressPolicy::redirect_verdict` (target host on the list, at most five hops, never `https` →
  `http`); without one, or carrying a credential header, no redirect is followed. The MCP bridge,
  OIDC discovery and JWKS, capability probes and the skillrunner use the first; the federation
  client, the bulk peer fetch and `OpenAiBackend::new` the second, with `OpenAiBackend::with_egress`
  for the first (the stem's `[[serve]]` uses it). The companions use the same verdict:
  `HttpLibrarySource` follows only with a policy and no static header, and `OllamaProbe` gains
  `with_egress` — it was the one outbound client no allow-list reached. The wiki's `GitMirror`
  runs git with `http.followRedirects=false`. An empty allow-list still permits every host, so a
  deployment that never configured egress sees only the hop cap and the downgrade refusal. Fail-
  first on `36ae00b2`: `an_mcp_redirect_to_a_denied_host_reaches_no_connection`,
  `an_llm_backend_without_a_policy_follows_no_redirect`,
  `a_jwks_redirect_to_a_denied_host_is_not_followed` and
  `the_federation_client_follows_no_redirect` each failed — the denied listener's own accept
  counter moved — and each plants an allowed redirect first. **Not in this entry:** the
  object-store fetcher's endpoint is R3b and the host parser R4, both above. **API note:** `OpenAiBackend::new`
  follows no redirect; call `.with_egress(policy)` to follow allowed ones.

### Documentation
- **What the egress allow-list covers, restated** (`docs/plans/realignment-repairs.md` R5). The
  threat model said the list gated "every outbound HTTP path" and, in the same section, that OIDC
  JWKS was not gated — false since 2026-10-03; neither said a redirect was followed unchecked. The
  threat model's Boundary C, the `egress.allow_list` row of the guarantee catalogue (regenerated),
  the crown-jewel coverage table and the confined-fleet runbook now say what 2.23.0 gates — the
  first URL and every redirect hop, with the host read by the client's own parser, and an object
  store on its endpoint — and what it does not: name resolution, a cloud identity's credential
  traffic, and redirects inside `object_store`'s client. The crown-jewel table gains the object
  store, the Ollama probe and the git mirror. `examples/receipt_ladder.rs` gains step 8b: a torn WAL
  tail, a restart over it, an `OnDisk` write, and a replay of the files copied mid-run.

### Fixed
- **The wiki git store's push tripwire no longer fires on a healthy concurrent publish.** Its doc
  called it an *ancestry* tripwire; the code compared the remote head with ours for *equality* right
  after the push. With several clones publishing to one origin, another publisher can push between
  our push and our `ls-remote`, leaving the remote at a descendant of our head — and that counted as
  a divergence and warned. It made `main` red on 2026-10-04 (`ten_councils_contend_without_spurious_
  failures_measured`: `tripwire quiet for c4`, left 1) while every PR was green; found by the
  release runbook's step 2b. The check now fetches the remote head and counts a divergence only when
  it does not contain ours. Fail-first on `386f8d3e`, made deterministic:
  `a_later_publisher_on_top_of_ours_is_not_a_divergence_but_a_rewound_remote_is` — A publishes, B
  publishes on top, then A asks — failed with the equality rule; its second half keeps a remote
  rewound past our head a divergence.
- **The KV WAL's startup repair is pinned, its failure refuses the start, a partial length prefix
  is a torn tail, and the WAL has one owner** (`docs/plans/realignment-repairs.md` R2; the review's
  F01, narrower than reported). `GossipAgent::start` has always run a snapshot before the first
  append, and the snapshot truncates `wal.bin`, so the node binary already repaired a torn tail — but
  nothing pinned that, the snapshot's result was discarded with `let _ =` (a failed startup snapshot
  started the node behind the torn frame, where the next appends were acknowledged and then swallowed
  by its claimed length), `decode_wal_records` reported a trailing 1–3-byte prefix as `Clean`, and two
  agents on one `base_path`/node id shared one `wal.bin`. Now: a failing startup snapshot **refuses
  the start by name** (`InvalidField { field: "persistence" }`, "startup snapshot"), the same class as
  v2.20.0's refusals; under `on_unreadable = "quarantine"` it warns and starts without the repair
  rather than moving readable files aside. A partial prefix is `WalEnd::Torn`. `start()` takes an
  exclusive OS lock on `wal.bin.lock` (`mycelium_core::persistence::OwnershipLock`, the journal's
  lock moved to core so both share it) before replay and hands it to the writer task
  (`WalHandle::hold_ownership`), which holds it until it exits; a second agent on the path refuses
  to start naming the owner. **`shutdown()` now stops the WAL writer** — final snapshot, lock
  released — after every other task has exited; before, the writer ran until the agent was dropped,
  so an agent restarted over the same directory in one process would now find the lock held. A
  direct embedder of `replay` + `spawn_wal_writer` is told, on both rustdocs, to take the lock,
  hand it over and trigger a snapshot first. **API note:** `WalMsg` gained `HoldOwnership` (an
  exhaustive match outside core breaks; none exists in the workspace), and `WalHandle::shutdown`
  now waits for the writer to exit.
  Witnesses: `a_partial_length_prefix_is_a_torn_tail_not_a_clean_end`,
  `a_failing_startup_snapshot_refuses_the_start_instead_of_appending_behind_a_torn_tail`,
  `a_second_agent_on_one_persistence_path_refuses_to_start_naming_the_owner`, each seen failing on
  the unfixed code; `a_torn_wal_tail_is_repaired_at_start_before_the_first_acknowledged_append` pins
  the existing repair by replaying a copy of the files taken mid-run (a crash; a graceful restart is rescued by the shutdown snapshot and cannot show it), and was seen failing with the trigger toggled off — the replay recovered nothing.
- **The node-local journal truncates a torn tail before the first append, owns its file, and stops
  after a failed append** (`docs/plans/realignment-repairs.md` R1; the external review's F02 and F12).
  `Journal::open` counted complete records but never removed a partial final frame, so an append
  after a crash landed *behind* it and the torn frame's claimed length then swallowed the new bytes:
  the append was acknowledged `OnDisk`, no reader ever returned it, and once enough had been
  appended every consumer that folds its journal on open — the evidence journal, the rights ledger,
  the knowledge durable stores and `DurableEpochs`, which carries v2.15.0's *a restart does not
  restore revoked authority* — refused to start. The v2.10.0 fix had covered counting only, and the
  test `a_truncated_tail_does_not_stop_the_node_from_starting` pinned the defect by asserting only
  that the append returned `Ok`. Now: `open` finds the last complete frame (a 1–3-byte partial
  length prefix counts as torn), truncates the file there, syncs it and its directory, and only then
  spawns the writer; each frame is written in one call; an exclusive OS lock on `<journal>.lock` is
  held for the handle's lifetime, so a second owner in this or another process is refused by name;
  and a failed append poisons the writer — every later append answers `Failed` naming the first
  failure — until a reopen repairs the file. Regression tests in `journal_repair_tests.rs` (the
  review's probes inverted: append after a torn tail, partial prefix, same-process and cross-process
  second owner, a failed append, an epoch floor recorded after a torn tail surviving a restart), each
  seen failing on the unfixed code; the pinning test inverted to read its record back.

---

## [2.22.0] — 2026-10-03

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.21.0 holds. **Upgrade notes:** `StemOptions` gained `stage_dir` (an exhaustive literal breaks; `..Default::default()` is unaffected); `mycelium_wasm_host::serve::spawn` takes each `[[serve]]` with its resolved key (`Stem::start` callers unaffected); a mesh-sourced stem now writes under `<placement_root>/stage`; `mycelium-effects` gained feature `metrics`. **Not claimed:** a real S3/GCS bucket (S4 — S3Mock is a mock), the `select!` branch seam (inventory row 142), `require_identity_proofs` on by default, and everything on the proof page's third table that needs a counterparty or hardware.

**Summary.** the **zero-gaps MINOR** (plan `zero-gaps.md`, Z1–Z9 delivered): the nine code-shaped gaps the repository recorded are closed — a stem reads an **object store** (`StemSource::Store`, `--library s3://…`, `--manifest-source`, a store-backed librarian that mirrors, the `catalog_store` suite profile against S3Mock); `[[serve]].api_key_env`, read once at start and **refusing the start by name** when unset; a blob past the frame cap crosses the mesh **in ranges** (`artifact.size` / `artifact.fetch_range`, `MeshRangedFetcher`; the stem's mesh path stages to disk and re-serves); the `llm_agent` tools as **self-describing components** (`describe` → the component's own schema, no tool section in the unit format); the scheduler seam's **second arm** — a tick due at the same paused instant as another waits its recorded turn, so **a live node replays decision for decision** (the v2.21.0 finding closed, `tests/decision_trace_replay.rs` the assertion); `mycelium-effects` `Counting<D>` + `RefusalCounts`; both SDKs' `set()` return `KvReceipt` (py 0.2.5, ts 0.1.2); `gossip_peers_connected`; `provisioning_viz` on :8101. Wire **v12** unchanged.

### Added
- **Zero gaps, the small four** (`docs/plans/zero-gaps.md` Z6–Z9): `mycelium-effects` gains
  `Counting<D>` + `RefusalCounts` — a destination whose refusals are counted by kind and leg, with
  `mycelium_effects_refusals_total{kind, leg}` under the crate's new `metrics` feature; both SDKs'
  `set()` return the receipt `POST /gateway/kv` has answered since v2.16.0 (`KvReceipt`: `operation_id`,
  `local_durability`, `local_durability_error`; py 0.2.5, ts 0.1.2 — additive, a caller that ignored
  `None`/`void` ignores a value); `gossip_peers_connected`, the live-writer gauge beside
  `gossip_store_entries` (`metrics.md`); `provisioning_viz` moves to `:8101` (`:8097` was also
  `guardrail_viz`'s).
- **`[[serve]].api_key_env`** (zero-gaps Z2, D2): a unit file names the environment variable that holds a
  keyed endpoint's bearer; the stem reads it once at start and **refuses to start, naming the variable,
  when it is unset** — never a skill served with no key. `api_key` stays the literal form; both set, or an
  empty name, is refused by the loader. The proof page's "keyed `[[serve]]` without a secret in the file"
  line closes. `mycelium_wasm_host::serve::spawn` takes each section with its resolved key (an API change
  for a caller spawning the serve task by hand; `Stem::start` callers are unaffected).
- **`llm_agent`'s tools as stems** (zero-gaps Z4, D4): the four MCP tools — weather, ping, search,
  calculate — are `capability-component` fixtures with artifact descriptions under
  `examples/units/llm_agent/artifacts/`, hosted by n-0 and n-1 as stems, bridged by the runtime and
  called by the driver in the stem-examples suite; and the bridge now asks a `tool/*` component to
  **describe itself** (kind `describe` → `{description, inputSchema}`), publishing the component's own
  schema instead of the generic one. No tool section in the unit format: the description is the
  declaration. The proof page's "`llm_agent`'s MCP tools as stems" line closes.
- **A stem reads an object store** (zero-gaps Z1, D1): `StemSource::Store { url }` — `mycelium-stem
  --library s3://bucket/prefix` (or `gs://`, `file://`) stages every blob from the store by ranged pull
  under `<placement_root>/stage`, credentials from the environment, the URL gated by the node's egress
  policy, and re-serves what it staged; a librarian over a store (`--library <url> --librarian …
  --manifest-source <url>`) reads the manifest from the store and mirrors what it names. The stem image
  builds with `object_store`; the stem-examples suite gains the `catalog_store` profile (S3Mock, the
  library published through the adapter, the installer's source the store). The proof page's "a stem
  reading an object store" line closes; "models pulled from S3 or GCS" (a real bucket, S4) stays.
- **A blob past the frame cap crosses the mesh in ranges** (zero-gaps Z3, D3): two RPC kinds beside
  `artifact.fetch` — `artifact.size` and `artifact.fetch_range` (4 MiB pieces, each a fraction of a
  frame) — served by every `serve_artifacts` holder (from disk when the source is ranged), and
  `MeshRangedFetcher`, a `RangedBlobFetcher` over the mesh that `DiskStagedSource` stages through. A
  mesh-sourced stem now stages everything it pulls under `<placement_root>/stage` (`--stage-dir`),
  keeps no blob in memory, re-serves what it staged in ranges, and installs the model demos' 19 MB
  weights with no `--library` — the proof page's "large models over the mesh" line closes.
  `StemOptions` gained `stage_dir` (an exhaustive literal breaks; `..Default::default()` is unaffected).
- **A live node's recording replays decision for decision** (zero-gaps Z5, D5 — the scheduler seam's
  second arm): in replay a timer choice that completed but is not the next recorded choice answers *not
  yet* (`Kernel::next_is`, `Seams::timer_if_next`) and waits its turn, bounded, diverging with both
  sides named when nothing moves; the replay `Ticker` keeps the recording's absolute grid and moves its
  state only after a check-in. The 20th-choice divergence v2.21.0 reported is closed and the comparison
  is a checked-in assertion (`tests/decision_trace_replay.rs`, in CI). Inventory row 141 closes; row
  142 (`select!` branch choice) is unchanged and says so.
- **The guarantee catalogue, generated and gated** (plan I2's deferred golden): `docs/reference/
  guarantee-catalogue.{md,json}` — every core guarantee's descriptor and a **state matrix** of its
  resolution on an unstarted node under four reference configurations, generated by
  `the_checked_in_guarantee_catalogue_is_current` under CI's `compliance,a2a` feature set and refused
  when stale (`UPDATE_GUARANTEE_CATALOGUE=1` regenerates). `mycelium --help` names the profiles,
  `GOSSIP_PROFILE`, the report's door and the catalogue (plan Q1, as proposed).

---

## [2.21.0] — 2026-10-03

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.20.0 holds. **Upgrade notes:** a deployment under `profile = "secure-single-domain"` must issue each node's certificate where the CA key lives and copy only `ca-cert.pem` to the node, or it will not start (rev 2); `BlobRuntime::with_entry_activation`'s closure takes a third argument (`&ActivationCtx`); `RuntimeCtx` gained `trace` and `install_token` (an exhaustive literal breaks); `prov.probe` is catalogue rev 2; a `with_data_at_rest_cipher` after `start()` is now ignored and counted rather than silently inert. **Not claimed:** a decision-level replay of a whole agent — it waits on the scheduler seam covering every periodic loop.

### Added
- **The trace, completed** (plan I5/I6): `[[activation]]` and its probe are instrumented under the
  install's token — `RuntimeCtx` carries the trace and the install token into the runtime, the
  activation hook takes an `ActivationCtx`, `prov.activation` and `prov.probe` record their outcomes
  with the trigger `install token N`, and a re-probe records only a change of verdict; `mycelium
  explain` reads install → activation → probe as one chain. The node binary's
  `GOSSIP_RECORD_BUNDLE_DIR` recording attaches `decisions.jsonl`, `coverage.json` and
  `decisions.stats.json` to the bundle. `mycelium-wasm-host` gained a `sim` feature, and the pilot's
  draw-count equivalence is shown under the replay kernel. A recording-versus-replay comparison of a
  live node was **run** (`decision_trace_replay`, feature `sim`): the recording carries its decisions
  into a bundle that reads back and explains — and the replay diverges at the kernel's 20th choice
  (two periodic loops' timer ticks swap order), so a decision-level comparison of a live node waits on
  the scheduler seam covering every periodic loop; the finding is on `what-is-proven.md`. **Upgrade note:**
  `BlobRuntime::with_entry_activation`'s closure takes a third argument, `&ActivationCtx` (a
  two-argument closure no longer compiles); `RuntimeCtx` gained `trace` and `install_token` (an
  exhaustive literal breaks); `prov.probe` is catalogue revision 2 (a new reason, `probe_failed`).
### Changed
- **Documentation coherence gate.** `scripts/check-materials.py` checks every live reader route, every
  local link target (1,472 of them), a named list of retired claims, and the decks' accessibility
  markers; `scripts/test-check-materials.py` proves four mutations fail it. Both run in `make check` and
  a new CI job. `check-positioning.sh` now asks only *developer* funnels to share the five-step path;
  operators have their own journey. The engineering deck's always-available-gossip, never-reordering
  mailbox and no-operator claims are qualified (overclaim ledger 2026-10-03); mobile overflow and a
  missing appendix heading fixed; `installation.md` uses a shell-safe `MYCELIUM_RELEASE_TAG`, which is a
  release anchor. A third party's follow-up to #475, with its two plan-status sentences brought to
  v2.20.0.

- **`secure-single-domain` rev 2** (announced in v2.20.0, plan G12): the profile now also requires
  `id.ca_key_off_node` — a node certificate issued off-node (`mycelium tls issue`, `[tls] cert_pem` +
  `key_pem`) so the fleet CA's private key is on no node — and `persist.unreadable_refused` (the
  default `persistence.on_unreadable = "refuse"`). A node under the profile that still re-signs its own
  certificate with the CA key **refuses to start**, naming the guarantee and the setting; the acceptance
  test issues its certificate off-node and proves no CA key is minted on the node. **Check before
  upgrading** a deployment that sets `profile = "secure-single-domain"`: issue each node's certificate
  where the CA key lives and copy only `ca-cert.pem` to the node, or the node will not start.
- **A data-at-rest cipher attached after `start()` is rejected** (plan G13, `at_rest.cipher`): the WAL
  writer read its cipher at start, so a later attach was accepted and did nothing while the report said
  `enforced`. It is now not set, not reported, and counted as a late attachment. Test seen failing on the
  unfixed code. **The wasm host is inside the replay-seam scan** (`scripts/check-sim-seams.sh`): its 41
  file-system and timing sites are admitted in the baseline and named in the nondeterminism inventory
  (a stem's artifact fetch, placement and activation timing are outside the seams, and a recording says
  so); a new site fails the gate. **Documented:** a router merged with `with_http_routes` that mounts a
  route outside the gated prefixes is public by construction (`docs/operations/rbac.md`). **Decided** on
  the plan: the guarantee report is not gossiped (Q2); the private companions register through the one
  registry before `start()` (Q3).

---

## [2.20.0] — 2026-10-03

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.19.0 holds. **Upgrade notes:** `StemOptions` gained `trace` and `TracePolicy` gained `Partial` (an exhaustive literal or `match` breaks; `..Default::default()` is unaffected); `mycelium::sim_seam` is re-exported in every build; `PersistenceConfig` gained `on_unreadable` (default `refuse`; an exhaustive literal breaks); `ClientError` gained `Egress` (`#[non_exhaustive]`). **Behaviour changes — each a refusal at `start()` where a node used to start degraded:** unreadable persisted state, or the wrong at-rest key (`persistence.on_unreadable = "quarantine"` to move the files aside); a gateway that cannot bind or load its certificate; an `[oidc]` issuer the egress list denies; an audit sink without `[tls]`. **Not claimed:** the node binary's bundle recording attaches no trace; no recording-versus-replay comparison has been run; activation and probe stay catalogue-only, so install → activation → probe is not linked by an operation id.

### Fixed
- **A node can start without the fleet CA's private key** (`id.ca_key_off_node`, the guarantee the
  startup report could report but no running node could meet). `TlsConfig::cert_pem` was declared and
  never read: the node cert was re-signed at every start with the CA key, which therefore had to be on
  every node — and when it was absent, `start()` minted a **new CA, key included**, into the node's
  directory. Now `cert_pem` + `key_pem` name a certificate **issued off-node** — `mycelium tls issue
  --ca-dir <dir> --node <ip:port> --out <dir>` (`mycelium::issue_node_cert`) on the host that holds the CA —
  and such a node starts from `ca-cert.pem` alone: no CA key loaded or looked for, nothing minted, a
  missing CA certificate an error naming the path, `cert_pem` without `key_pem` refused by name.
  Rotation on such a node means re-issuing (`generate_rotation` needs the CA key, by design). The
  guarantee resolves `enforced` only with a pre-issued certificate; `ConfinementReport.ca_key_off_node`
  follows. Test seen failing on the unfixed init. **Announced one release ahead (G12):**
  `secure-single-domain` rev 2 will require `id.ca_key_off_node` in the next MINOR — a deployment meets
  it by configuration (issue its node certificates off-node) before upgrading.

- **An audit sink that would receive nothing refuses the start** (`audit.sink`, plan §8). Audit
  records are sealed with the node identity, so on a node with no `[tls]` nothing is sealed and an
  attached `AuditSink` is accepted and silently receives nothing — the class v2.18.1 closed for the
  token tables. `start()` now refuses `with_audit_sink` without `[tls]` by name. The chain itself
  without `[tls]` stays a reported `not_configured` (a `compliance` build without TLS is legitimate for
  OIDC alone). Test seen failing on the unfixed code.

- **The egress allow-list reaches the federation client and the OIDC provider** (`report.egress`,
  plan §8). `egress.allow_hosts` gated the MCP bridge, LLM calls, probes, the skillrunner and the wasm
  host, and the startup report said so — but a federation client dialled any partner endpoint, and
  the gateway fetched OIDC discovery and JWKS from any host. Now a `FederationClient` is held to a
  policy (`with_egress`; `with_federation_clients` applies the node's policy to clients built
  without one) and refuses a denied endpoint before any byte is sent (`ClientError::Egress`, a new
  variant of a `#[non_exhaustive]` enum); and an `[oidc]` issuer or `jwks_uri` the allow-list does
  not permit **refuses `start()`** by name, with the runtime fetch gated as the belt to that brace.
  The guarantee `egress.allow_list` names what it covers, and that an `HttpLibrarySource` is gated
  only when built `with_egress`. Two tests seen failing on the unfixed code. **Behaviour change:** a
  node with a non-empty allow-list and an `[oidc]` table whose issuer it does not permit no longer
  starts; add the IdP's host.

- **A node no longer starts over persisted state it could not read** (`persist.replay`, plan §8). A
  corrupt or undecryptable `snapshot.bin` was logged and skipped — the node started from the WAL
  alone and the next snapshot **overwrote** it; a corrupt WAL record stopped replay silently, and the
  snapshot merge, stopping at the same place, **truncated** every record after it. Now `replay` tells a
  **torn tail** (the file ends inside the last record: a crash's signature, still tolerated) from
  **corruption** (a whole record that does not decode or decrypt, or a bad length with data after it),
  and corruption is an error naming the file and the byte; `do_snapshot` aborts on one and leaves the
  WAL as it is. `start()` then **refuses** by default (`persistence.on_unreadable = "refuse"`), or with
  `"quarantine"` moves the files aside as `*.unreadable-N` — never deleted, never compacted over — and
  starts from what was readable. New guarantee `persist.unreadable_refused`. Three tests seen failing
  on the unfixed code. **Behaviour change:** a node with unreadable persisted state now fails to start
  where it used to start and then overwrite it — including a node given the **wrong at-rest cipher
  key**, which used to start empty and snapshot over the ciphertext; a torn tail is unaffected. **Upgrade note:**
  `PersistenceConfig` gained `on_unreadable` (an exhaustive literal breaks; TOML and
  `..Default::default()`-style construction are unaffected — the field defaults to `refuse`).
  **Announced one release ahead (G12):** `secure-single-domain` rev 2 will require it, with
  `id.ca_key_off_node`, in the next MINOR.

- **A gateway that cannot come up fails `start()`** (`gw.tls_runtime`, plan §8). The bind and the
  gateway's TLS setup ran inside the spawned HTTP task, so a busy port or an unreadable certificate was
  logged as "HTTP server exited" while `start()` returned Ok and the node reported ready with no
  gateway. Both are prepared inside `start()` now (`http::prepare_gateway`); a failure is `start()`'s
  error, naming the gateway and the address, and the node is not ready. Test seen failing on the
  unfixed code. **Behaviour change:** a node whose gateway port is taken, or whose `[gateway_tls]`
  paths do not load, no longer starts without it.

### Added
- **The rule catalogue** (`docs/plans/guarantees-and-rule-catalogue.md`, increment I1's rule half). A
  *rule* is a decision point the substrate already has; `mycelium_core::rule::RuleDescriptor` describes
  one — a stable id and semantic revision, the responsibilities it answers (propagation · admission ·
  response · authority), its trigger and inputs with their scope and freshness, every outcome with typed
  reasons, its effects, the guards it passes, the rules it may trigger, inhibit or depend on (hypotheses
  until a trace or a test shows them), its symbol, docs and tests, and whether the decision trace
  records it. A descriptor executes nothing; there is no rule engine. Each crate registers its own
  (`mycelium::rules::RULES`, 11 substrate entries; `mycelium_wasm_host::rules::RULES`, the 15 the stem
  provisioning pilot passes through), and `docs/reference/rule-catalogue.{json,md}` is generated from
  them and checked in CI — unique ids, every relation resolving, every named test present in the tree,
  the document current (`UPDATE_RULE_CATALOGUE=1` regenerates). Every entry is catalogue-only until the
  pilot (I5) attaches a sink. Additive.
- **The decision trace core** (plan I4): `mycelium_core::decision` — a `DecisionRecord` (rule and
  revision, build, config digest and profile, node and incarnation, local sequence, target, trigger and
  parent *when known*, bounded inputs with age and provenance, view status, outcome and typed reason,
  effect reference, completeness flags) and a `DecisionSink` that keeps a bounded number of them without
  blocking the decision that produced one: bounded by count and by bytes, the newest dropped and counted
  at a bound, `try_lock` only, no clock or random draw of its own. Off unless a decision point is handed
  a sink; nothing is instrumented yet (I5). A replay bundle will carry it as `decisions.jsonl`, and a
  bundle without it means *trace unavailable*, never *no decisions*. Additive.
- **The stem provisioning pilot of the decision trace** (plan I5). `Provisioner::with_decision_trace(sink)`
  records every decision a provisioning round makes — health pass, promotion, demand response, presence
  floor, shed, eligibility, self-election, rights admission, and the install's outcome from its task —
  from the values the round already produced: guard order, the self-election draw and policy are
  unchanged, nothing is re-evaluated, nothing is recorded under the hosted lock, and the sink never
  waits. `StemOptions::trace` hands a stem one; `mycelium-stem --trace-dir <dir>` writes
  `decisions.jsonl` and its counters (`DecisionSink::stats_json`) there on shutdown. Off by default. Shown by a test that runs the
  same scenario with and without a sink (same decisions, same hosted state; the traced one reads
  eligible → elected → unmet demand → completed, then healthy and already hosted) and one that
  saturates the sink (one record kept, the rest dropped and counted, the install still happens).
  **Upgrade note:** `StemOptions` gained `trace` (an exhaustive literal breaks; `..Default::default()`
  is unaffected). **Not claimed:** the install → activation → probe chain is not linked by an
  operation id yet, and activation and probe stay catalogue-only.
- **Replay and explanation for the decision trace** (plan I6). A replay bundle can carry the trace:
  `Bundle::attachments` with `decisions.jsonl` and `coverage.json` (which rules the trace could have
  recorded) — a bundle written before them still reads, and `has_decision_trace()` says *unavailable*,
  never *empty*. Two traces compare deterministically on the canonical decision tuple (rule and
  revision, trigger, target, outcome, reason — not the sequence or the stamps), and a divergence whose
  records read gossiped or sampled inputs is reported as *unattributable* rather than counted as a
  reproduction failure (`decision::compare`, `canonical`, `attributable`, `coverage_manifest`). Two
  subcommands on the existing CLI: `mycelium rules [--format md|json]` exports the catalogue this
  binary registers, and `mycelium explain <decisions.jsonl> [--catalogue <rule-catalogue.json>]
  [--target <t>]` renders a trace as what each target went through, ending with what the trace cannot
  say. `mycelium-stem --trace-dir` writes `coverage.json` beside the trace. **Not claimed:** the node
  binary's bundle recording does not attach a trace (nothing in that binary is instrumented), and no
  recording-versus-replay comparison has been run.
- **The decision trace on a node** (plan I7): `GossipAgent::with_decision_trace(sink)`, set once before
  `start()`. The membership governor records every decision it makes — spacing, settling, a hold with
  the roll it rested on, held on an unconfident view, join, leave — and signal admission records its
  refusals and sheds (an admitted signal is the hot path and records nothing; the catalogue says so as
  a *partial* policy). From the values each already produced: no decision changes, the draw and the
  clock reads are untouched, nothing is recorded under a lock, the sink never waits. Off unless attached.
  Shown by a governor fleet converging to its floor identically with and without sinks, and by admission
  at the deterministic fills. `TracePolicy` gained `Partial(&str)` (a `match` needs an arm).

### Fixed
- **A failing initial activation probe is an activation error, not a live install.** `[[activation]]`'s
  probe gates the capability (D21), but the hook returned a healthy flag of `false` as a successful
  install: the capability was advertised and counted complete, and only the next round's health pass
  withdrew it. The install now fails at stage `activation`, nothing is advertised, and the next round
  retries. Test `a_failing_initial_probe_is_an_activation_error`, seen failing on the unfixed hook.
- **The provisioner's self-election draw goes through the replay seam.** It was raw `fastrand`, in a crate
  the seam lint does not scan, so under `sim` the draw was not a kernel choice and a recording could not
  reproduce which node installed. It draws from the `select` stream now, and `scripts/check-sim-seams.sh`
  refuses a random draw outside the seam anywhere in `mycelium-wasm-host` (zero baseline) — seen failing
  on the unfixed code. The host's file-system and timing sites stay outside the main scan; that is a
  baseline decision of its own, recorded on the plan's table. With it, **`mycelium::sim_seam` is
  re-exported in every build**, not only under `sim` — the non-`sim` arm is the plain implementation,
  and a seam a companion can reach only in a `sim` build is a seam nobody routes through (additive).

---

## [2.19.0] — 2026-10-03

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.18.2 holds. **Upgrade notes:** `GossipConfig` gained `profile` (an exhaustive literal breaks; `..Default::default()` is unaffected); no node changes behaviour unless it sets a profile; `ConfinementReport` reads `require_identity_proofs` without `[tls]` as `Unset` now (the flag is inert without the identity). **Not claimed:** `id.ca_key_off_node` is reported but not required — today's TLS init needs the fleet CA's private key on every node at start.

### Added
- **The guarantee registry and the startup report** (plan `guarantees-and-rule-catalogue.md`, I1's
  guarantee half + I2). Twenty-three core guarantees — the gateway's, the transport's, identity,
  authority and evidence, audit, egress, persistence, and the three a node cannot see — each with what it
  needs and where it is enforced, resolved against this node *as built and configured*:
  `GossipAgent::guarantee_report()`, `GET /gateway/guarantees` (`fleet:read`), and one log block at
  `start()` after validation and before traffic. Five states: `enforced` · `not_configured` (the setting
  named) · `not_in_build` (the feature named) · `not_applicable` (the role fact named) ·
  `not_verifiable_here` (listed, never counted). A companion registers its own with
  `register_guarantee` before `start()`; duplicates and core-id overrides are refused. An attachment made
  after `start()` is counted (`late_attachments`) and warned about — the at-rest cipher among them, which
  silently left the WAL plaintext before. `ConfinementReport` is a view over the registry now, which
  sharpened one reading: `require_identity_proofs` without `[tls]` is `Unset`, since the flag is inert
  without the identity. The strongest sentence the report says is *node requirements satisfied*.
- **Guarantee profiles (plan I3).** `GossipConfig::profile` / `GOSSIP_PROFILE` selects `dev` (nothing
  required; the startup log says it is not a production profile) or **`secure-single-domain`** (rev 1,
  fifteen node-enforced guarantees: authenticated transport and identity, a closed HTTPS gateway with the
  caller attested, authority checked and recorded at gateway and provider, revocation that survives a
  restart, a sealed audit chain, an egress allow-list, durable persistence). Under it `start()` **refuses**
  unless every required guarantee resolves `enforced` or `not_applicable` on this node, naming each unmet
  one with what is missing and where to read; a required id the registry does not hold refuses too, and
  an unknown profile name is refused by `validate()`. The report carries the profile, its revision and
  its required set. External prerequisites are never in a profile. **Found by the profile's own
  acceptance test:** `id.ca_key_off_node` cannot be required — `tls::load_or_create_ca` loads the fleet
  CA's private key from the node's directory and regenerates a whole CA without it, so every TLS node
  holds the key at start; the report shows it as not enforced, and the readiness checklist says so.
  **Upgrade note:** `GossipConfig`
  gained `profile` (an exhaustive literal breaks; `..Default::default()` is unaffected); no node changes
  behaviour unless it sets a profile.

---

## [2.18.2] — 2026-10-03

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.18.1 holds; no API change. **Upgrade note:** reissue every serving client's token with `mesh:serve` — the gateway log's warning since 2.15.0 named the ones still relying on the window.

### Security
- **wasmtime 49.0.2 (RUSTSEC-2026-0325, -0326, -0327).** Three advisories published 2026-10-02 against the
  49.0.1 the lock held, one of them GC-heap corruption from missing rooting across `try_call` (medium,
  5.7). The WASM host is the boundary where third-party components run, so the lock moves to the fixed
  release (the cranelift family to 0.136.2 with it). No API or wire change; `cargo audit` green again.

### Changed
- **The `mesh:serve` compatibility window is closed.** Closure plan C1 (2.15.0) admitted a `mesh:read` or
  `mesh:write` token on the serve routes "for one release", with a warning; the window stayed open through
  2.18.x (an independent review found the promise three releases overdue). Such a token now answers
  `403 {"required_scope": "mesh:serve"}`. **Upgrade note:** reissue every serving client's token with
  `mesh:serve`; the gateway log's warning since 2.15.0 named the ones still relying on the window.

---

## [2.18.1] — 2026-10-02

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.18.0 holds; no API change. **Check before upgrading:** a node that sets a token table, an `[oidc]` table, `[tls]` or `[gateway_tls]` in a build without the feature that enforces it now fails at start — it was running without that protection; build with `--features compliance` / `--features tls` or remove the table, and confirm an unauthenticated `GET /gateway/kv/keys` answers 401.

### Note, added after the tag
- **v2.18.1 also carries 36 files that its release notes do not mention.** PR #474 was committed with
  `git add -A` from a checkout another session was editing, and swept in that session's uncommitted work:
  the six-tutorial learning path (`docs/guide/tutorials/`), the `first_stem_fleet` example and
  `compare_stem_observations` consumer, `scripts/check-stem-observations.py` and a `wasm-host` CI step that
  runs them, an interim positioning sentence on every door, and smaller guide edits. All of it passed CI
  and none of it changes the library's code or wire. Reviewed after the fact in the PR that adds this note,
  which also replaces the interim sentence.

### Security
- **A token table the build cannot enforce now refuses to start, instead of running an open
  gateway.** `gateway_named_tokens` and `gateway_scoped_tokens` (and `GOSSIP_GATEWAY_NAMED_TOKENS`)
  are honoured only under `compliance`. A build without it, including the default `mycelium`
  binary, parsed the table, ignored it, and with no `gateway_auth_token` admitted every gateway
  request anonymously, with no warning. `start()` now returns `InvalidField` naming the table and
  the feature. **Check any deployment that sets a token table:** an unauthenticated
  `GET /gateway/kv/keys` must answer 401. Found by the doc-coverage audit (run 18); regression test
  `a_token_table_this_build_cannot_enforce_refuses_to_start`, seen failing on the unfixed code.
  **Upgrade note:** a node that set a token table in a non-`compliance` build now fails at start;
  build with `--features compliance`, or use `gateway_auth_token`.
- **Three more settings of the same class refuse at start, found by the plan's own audit**
  (`docs/plans/guarantees-and-rule-catalogue.md` I1, 2026-10-02). An **`[oidc]`** table in a build
  without `compliance` was **dropped at parse time** (the field is compiled out and the config accepts
  unknown keys), so a node the operator believed was behind SSO ran an open gateway; it now parses in
  every build and `start()` refuses it by name. A **`[tls]`** or **`[gateway_tls]`** table in a build
  without `tls` was accepted and ignored — unauthenticated gossip, bearer tokens in cleartext, and
  `domain_profile = "enforced"` passing `validate()` on the strength of a dead field; both now refuse.
  **Check any default-build deployment that sets one of these tables:** it was running without the
  protection the table names. Regression tests `an_oidc_table_this_build_cannot_enforce_refuses_to_start`
  and `a_tls_table_this_build_cannot_enforce_refuses_to_start`, seen failing on the unfixed code.
- **`/a2a`'s exposure is stated correctly, and at start.** The attach-time warning fired only with no
  evaluator *and* no bearer — but a bearer does not gate `/a2a` (an absent one is anonymous, a present
  one's scopes are dropped), so the exact exposed configuration got no warning, and an evaluator attached
  after `with_a2a()` made it spurious. `start()` now warns whenever `/a2a` is mounted with no evaluator.
- **The confinement report no longer says `Set` for a setting the build cannot act on:**
  `identity_proofs_required` and `ca_key_off_node` report `NotInBuild` without `tls`.

### Fixed
- **A verbose `[[activation]]` command no longer stalls into a false timeout.** The runner piped the
  command's stderr and read it only after exit, so a command writing more than a pipe buffer blocked on
  the write and was reported as *did not finish* — a false failure that could stall provisioning or a
  re-heal. Stderr is now drained while the command runs, keeping a bounded tail (4 KiB) for the error.
  Found by an independent review (360 review F3); tests written first and seen failing:
  `a_verbose_command_that_succeeds_is_not_reported_as_a_timeout` and
  `a_failing_command_reports_a_bounded_tail_of_its_stderr` (`mycelium-wasm-host/src/activation.rs`).

---

## [2.18.0] — 2026-10-01

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.17.0 holds. **Upgrade notes:** `NodeCapabilityConfig` gained `activations` and `serves`, and `StemOptions` gained `reprobe_every` (an exhaustive literal breaks; `..Default::default()` is unaffected); `wire_check` counts a `[[serve]]` skill as an offer; `mycelium-wasm-host` gained feature `llm`. **Not claimed:** an activation command is the operator's, run with the stem's privileges — not a sandbox; the GCS path is still proved only against a real bucket, and none has run.

### Added
- **`llm_agent` as stems (plan X2, seventh slice — the row closes).** The stem-examples suite runs the
  browser demo's subject with real artifacts: n-0 a declared unit; n-1 installs a real dataset blob
  on the agent's demand, through the real `data/loading` tier; n-2 deploys a real model into its
  Ollama and serves `llm/inference` by `[[serve]]`; the driver, as the agent, declares its need,
  watches the loading tier and routes a prompt. The demo's MCP tools stay in-process code. The
  `llm_agent` units now describe this recut (the browser demo still reads `examples/node_n*.toml`).
- **SDK agents load their unit files (plan Q2).** `POST /gateway/units/declare` (scope `cap:write`)
  takes a unit file's text; the node parses and validates it with its own loader and declares its
  capabilities, requirements and groups under **one handle** that `DELETE /gateway/capability/{id}`
  retracts and the heartbeat route renews (`lease_secs`). The hosting sections — `[hosts]`,
  `[[presence]]`, `[[activation]]` — are a stem's and are refused (422); lanes, mandates and rules are
  accepted and reported as `not_enforced`. SDK verbs: `declare_units` / `declare_from` and `UnitHandle`
  (`mycelium-py`), `declareUnits` and `UnitHandle` (`mycelium-ts`) — both carry the text, neither
  parses TOML. The lease watchdog the capability route had is now shared by both handle kinds.
- **Declared model serving, and `reheal_deploy` as stems (plan D22, X2 sixth slice).** The unit file
  gains `[[serve]]`: the prompt skill `{ns}/{name}` over an OpenAI-compatible endpoint, registered
  **while** the install named by `while_live` is live on the node and retracted when it goes — the
  `serve_model` bridge as a declaration, on core's `llm` feature (wasm-host feature `llm`; a stem
  without it refuses a `[[serve]]` by name). `validate()` refuses a missing endpoint or model and a
  skill that shares its key with the install it waits for; `wire-check` counts a served skill as an
  offer. Gate: `a_served_skill_follows_the_install_it_waits_for` (seen failing first). The
  stem-examples suite's `reheal_deploy` profile: two hosts each with its own Ollama and a floor of one
  deployment; the driver routes real inference through `InferenceRouter`, starts the survivor, kills
  the origin, and routes again to the survivor that rehealed. With it, every artifact-shaped demo
  runs both ways.

- **`model_deploy` as stems (plan X2, fifth slice).** The stem-examples suite deploys a real model:
  the TinyStories GGUF (~19 MB, downloaded at run time) and a governed profile naming it by content
  address are published to a library; a librarian stem serves them; the model-host stem places both
  and, by its `[[activation]]`, uploads the weights to a pinned Ollama container (`ollama/ollama:0.9.0`)
  and creates the model through the HTTP API; the driver asserts the profile's SYSTEM prompt is what
  Ollama runs and that the model generates real tokens. The model_deploy units and artifact
  descriptions now describe the real thing rather than echo stand-ins.
- **Declared blob activation (plan D21, X2 fourth slice).** The unit file gains `[[activation]]`: per
  capability, an argv `command` run after a blob is placed and an argv `probe` that gates the
  capability, with `{path}` / `{dir}` / `{artifact}` / `{ns}` / `{name}` placeholders and
  `resolve_artifact_refs` (render `artifact:<hex>` references to placed paths as `{rendered}`; one not
  yet placed fails the activation and the next round retries). `validate()` refuses an empty command,
  an unknown placeholder, `{rendered}` without the flag, a zero timeout, and an activation on a unit
  whose `[hosts]` does not name the blob kind. A stem wires it through the new
  `BlobRuntime::with_entry_activation`, whose probe reads only a per-install flag that a background
  task keeps current (lock-order row 51) — a failing probe withdraws the install and the next round
  reinstalls. Gates: `a_declared_activation_runs_after_placement_and_its_probe_gates_the_capability`
  (seen failing first) and `an_activation_resolves_artifact_references_to_placed_paths`.

- **Provisioning's wave 3 as stems (plan X2, third slice).** The stem-examples suite now runs the
  whole provisioning demo both ways: after the self-heal, the driver starts a late provider-c, an
  agent's v2 is stored in the shared library and proposed into the catalogue, the stems load it only
  into the shadow lane while the incumbent serves, a stranger's acceptance changes nothing, the
  reviewer's promotes it, the operator retires v1 from the manifest (the librarian tombstones it),
  and provider-c serves the accepted v2 — "shadow-then-accept complete" both ways. The provisioning
  units trust the agent's key and name the reviewer; a librarian unit of its own replaces provider-a
  doubling as librarian.

### Fixed
- **A test race that turned the v2.17.0 release commit's strict `Test` job red.**
  `test_identity_anchor_recorded_and_conflict_flagged` poisoned the legacy `sys/identity/{A}` pair
  and waited for the anchor-conflict tripwire; since identity and its proof travel as one sealed
  record (2.14.0), the watcher reads `sys/identity-signed/{A}` first, so the legacy poison is
  **masked** whenever A's sealed record has already reached B — the tripwire fired or not depending
  on arrival order (green eight times on `main`, red on the ninth; one in eight locally). The test
  now waits for the sealed record, asserts the masking (the sealed record is authoritative), then
  poisons the record the watcher actually reads — the sealed record tombstoned, the legacy pair
  carrying the foreign key — and the tripwire fires deterministically (ten of ten). The shipped code
  is unchanged; the release stands.

- **Two stem capability gaps closed (plan X2, second slice).** A hosting stem on the mesh path
  **re-serves its verified cache** (`serve_artifacts` over its `MeshArtifactSource`, advertised as
  `artifact/librarian` with `role = "cache"` once it holds something), so a late joiner installs from
  a peer after the librarian is gone — the catalog demo's phase 6 now runs as stems in the
  stem-examples suite. The WASM runtime **bridges a `tool/{name}` component as MCP tool `{name}`**
  (feature `gateway`; the tool disappears with the install), so the tool-growth demo runs as stems
  with its own marker. Gates: `a_hosting_stem_re_serves_its_verified_cache_to_a_late_joiner`,
  `an_installed_tool_component_is_bridged_as_an_mcp_tool` (both seen failing first), and the two
  suite profiles.

---

## [2.17.0] — 2026-09-30

Wire **v12** unchanged (`PREV = 11`); rolling upgrade from 2.16.0 holds. The design-time tooling MINOR — the plan `docs/plans/design-time-tooling.md` (rev 0.9) and the composition's bounded half. **Upgrade notes:** `WasmHostError` gained `FuelExhausted` (a `match` needs an arm); `verify_library` / `verify_store` gained a `reviewers` parameter; `PublishOutcome`, `wire_check::Edge`, `HostsDecl` and `GossipConfig`-shaped literals gained fields (exhaustive literals break); `mesh`-side wire unchanged. **Not claimed:** the GCS path is proved only against a real bucket (none run yet); the declaration as a consumer record waits for the private companion's pin; the stem recuts `provisioning` and `catalog` phases 1–4 and names the rest.

### Added
- **The stem-examples suite (plan X2, first slice).** One image (`docker/Dockerfile.stem`) carries
  `mycelium-stem`, `mycelium-artifact` and the co-op `stem_driver`; `docker/docker-compose.stem-examples.yml`
  runs the `provisioning` demo (providers a and b as stems fed `examples/units/provisioning/`, the
  driver killing the active one for real) and the `catalog` demo's phases 1–4 (librarian and installer
  stems) with the same markers the code run prints; `make examples-both-ways DEMO=…` runs both and
  greps one contract twice; the `stem-examples` job in `cluster-suites.yml` gates it. The hosting
  units' publisher key is the public half of the suite's seed (`make stem-keys`) — the stem refuses a
  placeholder the checker accepted. What is not recut, and why, is in the compose header.
- **The store test runs once per configured store (plan S3).** `MYCELIUM_S3_TEST_URL` (S3Mock in CI,
  as before) and now `MYCELIUM_GCS_TEST_URL` — the same sequence (publish, manifest round trip, ranged
  staging, provenance and the streamed hash) over each; with neither the local `file://` store stands
  in and the test says so. **No GCS emulator runs in CI:** two runs against `fake-gcs-server` found the
  crate's put path fetching a credential regardless of skip-signature (a static bearer answers that)
  and then the emulator accepting only signed-URL uploads, which the crate's plain PUT is not. The
  GCS gate is a real bucket (S4, open), where the test runs unchanged.

- **Proposed → shadow → accept (plan D20 / F2, `mycelium-wasm-host`).** An `InstallableEntry` gains
  `proposed` (inside the publisher's signature, under its own domain — flipping the byte breaks the
  signature) and `acceptance` (a reviewer's Ed25519 signature over the publisher's signed content
  *and* signature); an entry with neither still encodes as **v1, byte for byte**, one with either as
  format v2 (`ENTRY_FORMAT_VERSION_V2`; a v1 decoder does not see it, which is the safe direction).
  `as_proposed()`, `accepted_by(&reviewer)`, `verify_acceptance(&reviewers)`, `is_loadable(&reviewers)`.
  `Provisioner::require_reviewers` names who may promote; a proposal without a listed reviewer's
  acceptance loads only into the **shadow lane** — installed and advertised as `{ns}/{name}.shadow`
  (`shadow_name`), callable by name for comparison, never resolved by the incumbent's filter, so it
  takes no demand and keeps no presence floor; when the same catalogue line arrives accepted the
  shadow is withdrawn and the entry loads for real. `mycelium-artifact accept <library|url>
  <ns/name | artifact-hex> --key <reviewer>` co-signs in place (same KV key, the librarian
  republishes it as an overwrite); `verify --reviewer ed25519:…` names a forged or unlisted
  acceptance; a description's `proposed = true` publishes a proposal. `[hosts].trusted_reviewers`
  wires it in the stem. `mycelium wire-check` reports a proposed artifact as **`would bind after
  acceptance`** (`Provider::Artifact.proposed`, absent from the document unless true — the golden is
  unchanged). Gates: the catalogue codec tests (v1 identity, v2 round trip, forged/moved acceptances),
  the tool test, the provisioner test (shadow → forged → accepted, seen failing first: the proposal
  installed for real on the first round), the checker fixture `artifacts-proposed`, and the co-op
  `provisioning` demo's **wave 3** (the incumbent serves the whole wave while v2 shadows; a stranger's
  acceptance changes nothing; the reviewer's promotes it and the accepted v2 serves wave 3b, visible
  in its replies). API notes: `verify_library` / `verify_store` gained a `reviewers` parameter;
  `PublishOutcome` gained `proposed`.
- **Agent-authored functions, documented (plan F3):** guide 16 § *Agent-authored functions* (U1–U4,
  the five gates, the honest limit), the artifacts runbook's trust section, a doc-coverage row.
- **The examples' declaration directories (plan X1).** `examples/units/<example>/` for every example
  that advertises a capability, declares a requirement, defines a group or names a lane — sixteen
  directories in the `capability_config` unit format, with `artifacts/` beside the ones that
  provision. `scripts/wire-check-examples.sh` runs `mycelium wire-check` over all of them in CI; the
  matrix in `examples/README.md` gains a *Declared* column linking each row to its directory; the
  gate `tests/wire_check_examples.rs` holds both ways — every directory clean, and deleting any
  capability's last provider turns its directory red (seen failing first, twice), which is why each
  example's caller is written down as a requirer.
- **The declaration document's schema (plan W6, public half).** `docs/reference/declaration.schema.json`
  pins `mycelium.design/declaration/1`; `tests/declaration_schema.rs` validates the golden and freshly
  generated reports against it with a real validator (`jsonschema`, a dev-dependency only) and plants
  three refusals. Every `wire_check::Edge` gains `operations` — its two call shapes as
  `{operation, resource}` (`CallShape`; `call_shapes` is public) — so a consumer joining runtime records
  maps them through its own catalogue rather than re-deriving them. Additive: the document's schema
  version is unchanged and no consumer existed before the pin.

- **`POST /gateway/artifacts/publish` (plan A3, `mycelium-wasm-host` feature `gateway`).** A named,
  checked door into the gossiped catalogue: the body is one **already-signed** catalogue line
  (`{"entry_hex": …}`, the hex of a manifest line), verified against the node's trusted publishers
  and written with `publish_installable`; the publisher's key never reaches a gateway and the bytes
  never ride the request. New scope family **`artifact:publish`** (its own, not `kv:write` — a
  narrower power with a check the raw KV route does not make). Refusals by name: 403 `unsigned
  entry` / `untrusted publisher` / `provenance does not verify` / `no trusted publishers configured`,
  409 `librarian-managed signer` (a librarian's manifest would tombstone the line at its next sync),
  400 `malformed entry`. `artifact_router(agent, trusted, librarian_publisher)` is merged with
  `with_http_routes` before `start()`; the stem binary mounts it from `[hosts].trusted_publishers`.
  SDK verbs `artifacts().publish(entry_hex)` (`mycelium-py`, `mycelium-ts`) post the signed line
  and surface a refusal under the gateway's own name (`ArtifactError.kind`). Gate: a two-node test
  in which the signed line appears under `installable/` on the second node with provenance intact,
  every refusal answers by name, and `kv:write` does not open the door.
- **Fuel by publisher (plan D19 / F1, `mycelium-wasm-host`).** `WasmHost::metered()` counts fuel
  without a default budget; `WasmHost::instantiate_with_fuel` / `provision_with_fuel` give each
  instance its own; `Provisioner::set_fuel_policy(FuelPolicy)` decides per entry from its verified
  signer — an operator key's entries run under `operator_budget` (absent = unbounded), every other
  trusted key is an agent principal under `agent_budget`. A call that runs past its budget is stopped
  as `WasmHostError::FuelExhausted { budget }` (a new variant; a match on `WasmHostError` needs an
  arm) and recorded by name (`Provisioner::invocations()`, `InvocationRecord`/`InvocationOutcome`,
  counter `mycelium_artifact_invocations_total{outcome}`); the install stays live. `[hosts]` gains
  `operator_publishers` and `operator_fuel_per_call`; `validate()` refuses a zero budget and an
  operator list without a trusted list or outside it, by name. The stem wires all of it. Gate: an
  agent-signed entry that loops (the committed `spin_component.wasm`) is stopped at the budget and
  the record says so; the same echo bytes under the agent's key are stopped too, and under the
  operator's key run to completion — seen failing first with the budget applied uniformly. The gate
  also found that a trap **poisons** the component instance (wasmtime refuses to re-enter it), so a
  stopped call used to leave a dead install behind a live advertisement; the serve loop now replaces
  a trapped instance from the verified bytes (the guest's in-memory state is lost, its KV subtree is
  not) and stops serving only if that fails, so the probe withdraws and reinstalls.

### Security
- **wasmtime 49.0.1 / wasmtime-wasi 49.0.1** (from 46.0.3): RUSTSEC-2026-0316 (dynamic record lifting
  could allocate beyond the hostcall fuel limit) and RUSTSEC-2026-0314 (a guest could panic the host
  through a filesystem datetime overflow), both dated 2026-09-24, both without a 46.x fix. Confined to
  `mycelium-wasm-host` as before; the substrate's dependency tree is unchanged.

### Changed
- **The composed effect is enforced at the destination** (`mycelium-effects`; `docs/design/composed-effect.md`
  §9). `Composition` — the verified principal, the operation, the presented mandate, the origin domain —
  travels with an effect, and `EffectDestination::apply_composed(&ComposedEffect, &ResourceAuthority,
  now_ms)` commits only when the effect is **attributed** (the principal is the mandate's holder) and
  **authorised** (`ResourceAuthority::check` for that operation at this resource, now). A refusal is
  `EffectRefusal::Unauthorised { leg, reason }` and leaves no business row and no dedup row, so a later
  authorised attempt is `Fresh`. The domain leg is carried, not re-verified (a destination holds no
  trust bundle). **Upgrade note:** `EffectRefusal` is now `#[non_exhaustive]`; a `_` arm must fail
  closed — an unrecognised refusal is *not committed*, never *retry until it lands*. Gate:
  `mycelium-effects/tests/composed.rs`, written before the check existed and observed to commit every
  planted leg. The check and the commit are two steps: the check-then-act window every resource has,
  narrowed and stated. Beside it: `Composition::from_envelope` (feature `envelope`) builds the
  composition from the gateway's `ActionEnvelope` and the caller's `PresentedMandate`, refusing an
  envelope bound to a different appointment than the presented grant, and `Composition::from_caller`
  builds it at a provider from the caller context a tool handler receives (`from_carried` over its
  pieces; `from_call` over the new `McpCall` that `register_mcp_tool_with_call` hands a handler, which
  keeps the carried mandate on a member's own mandated direct call too — before, mapping a self
  envelope to a node principal dropped it), demonstrated end to end by `examples/composed_commit` in CI — permitted at the gateway,
  refused at the resource once it installs a newer epoch, both decisions in the journal; and
  `AeEvidence::for_destination_refusal` records a destination's refusal as a denied decision at
  enforcement point `destination` with nothing run, so a refusal after a gateway permit is as
  reconstructable as a commit.

### Fixed
- **A large artifact from an HTTP store no longer lands in memory before its hash is checked.**
  `HttpLibrarySource::fetch_remote` read the whole body with `bytes()`, capped only by a declared
  `Content-Length` (512 MiB) — a chunked response with no length was unbounded, as the code's own
  comment from the 2026-07-15 audit said. The body is now read in pieces and counted against
  `DEFAULT_MAX_IN_MEMORY_BYTES` (64 MiB, `with_max_bytes`), refused **by name** the moment it
  crosses, length declared or not; `PrefetchingSource` refuses a blob past its bound rather than
  caching it (`with_max_bytes`). Both pinned by tests seen failing first
  (`a_chunked_body_with_no_length_is_bounded_by_name`,
  `the_prefetch_cache_refuses_a_blob_past_its_bound`).

- **The object-store adapter** (`ObjectStoreFetcher`, `mycelium-wasm-host` feature `object_store`;
  S2 of `docs/plans/design-time-tooling.md` §11): one URL selects the store through the
  `object_store` crate — `s3://`, `gs://`, `az://`, `https://`, `file://` — with credentials from
  the environment (the node's cloud identity, never a file this project defines). It implements
  both `BlobFetcher` (whole-object, refused past the in-memory bound by name) and
  `RangedBlobFetcher` (`head` for the size, `get_range` for a piece), so a model stages to disk in
  pieces through `DiskStagedSource`; every request is gated by the egress policy on the store URL
  before a client is built. The manifest lives in the store at `<prefix>/manifest` (D14):
  `LibrarianConfig` gained `manifest_source: Option<Arc<dyn ManifestSource>>` (an exhaustive
  literal breaks — add `manifest_source: None`), and `mycelium-artifact publish|list|verify` take a
  store URL. `Manifest::upsert` is new. Gated by `publish_stage_and_verify_through_the_store`,
  which CI runs against an S3-compatible store (Adobe's S3Mock; MinIO's images are no longer publicly pullable) and which runs over `file://` elsewhere, saying so; and
  by the egress refusal before any client is built. Not yet: a GCS fixture, a real bucket.
- **`mycelium-artifact publish | list | verify`** (`mycelium-wasm-host`, feature `stem`; the
  functions are public): the artifact tool of `docs/plans/design-time-tooling.md` D10 / A1. A
  reviewable TOML description (kind, the capability it provides, the footprint, the path to the
  bytes — the same file `mycelium wire-check --library` reads) is the input; `publish` stores the
  bytes content-addressed, builds and signs the entry with a 64-hex Ed25519 seed from a file or an
  environment variable, and appends the manifest line, idempotently; `list` renders a manifest back
  into the description shape with the content address and signer shown; `verify` exits 1 naming an
  untrusted signer, a missing or corrupt blob, and — with `--descriptions` — a description whose
  bytes changed under an unchanged manifest. Gated by a round-trip test (`list` of a published
  manifest parses back to the description) and a verify test that names each of the three problems.
- **`mycelium-stem`** (`mycelium-wasm-host`, feature `stem`; `Stem::start` as a library): a node
  that declares from its unit file — every `[[capability]]` advertised, every `[[requirement]]`
  declared, every `[[group]]` defined — and, with `[hosts]`, runs a provisioner configured from it
  (kinds → runtimes, install budget, headroom, trusted publisher keys, placement root, fuel) with
  every `[[presence]]` as a supervision policy, refreshing the catalogue from KV each tick and
  prefetching mesh-pulled artifacts. `--library <dir>` reads bytes from a directory, `--librarian
  <manifest> --publisher ed25519:<hex>` takes the librarian role too. R1's runtime half and R2's
  image of `docs/plans/design-time-tooling.md` §13: the file and the runtime vocabulary are one
  thing, and every declaration still travels as its own evaporating entry. Gated by
  `a_stem_declares_from_its_file_and_a_peer_sees_it` and the in-process fleet test
  `the_stem_fleet_fills_a_presence_floor_and_reheals` (three stems, one librarian, a floor of two,
  a kill, a re-heal, and the checker's declaration compared with what bound).
  `Provisioner::refresh_catalog` / `catalog()` are new for it.
- **The authority overlay** (W4, on by default; `--no-authority` off): for every wired edge whose
  requirer declares any authority vocabulary (a `[[mandate]]` or a `[[rule]]`), the checker asks
  whether some declared rule could ever admit the call — as a skill (`skill.invoke` on
  `skill:{ns}/{name}`) or a tool (`tools/call` on `tool:{name}`), actor and operation and resource
  matching — and, where that rule requires a mandate scope, whether a declared mandate held by the
  requirer's principal enumerates the operation in the gateway's own `{operation}:{resource}` form.
  `unauthorisable edge` (error) names the nearest reason; `ungoverned edge` (warning) marks a unit
  that declares neither beside units that do. It does not evaluate policy; it finds edges no
  declaration could ever authorise. Fixture `tests/fixtures/units/unauthorised`; the co-op fixture's
  mandate and rules now use the canonical operation form.
- **`mycelium wire-check --schemas <dir>`** (W3): the checker reads guide 12's schema directory —
  each `.json` file's path without the extension is a schema id — and reports `unknown schema` for a
  `schema_id` any unit or artifact declares that the directory does not define, beside the
  `schema-only mismatch` it already named with both ids. `[[capability]]` gained `schema_id` so a
  declared offer can carry the schema it advertises. Fixture `tests/fixtures/units/schema-window`.
- **`mycelium wire-check`** (`src/wire_check.rs`; W2, A2, L1 and R1's checker halves of
  `docs/plans/design-time-tooling.md`): the offline check over a directory of unit files, and
  optionally a directory of artifact descriptions (`--library`). A pure function over declarations
  that applies the mesh's own `CapFilter::matches` and reports what **could not bind**: `unwired
  requirement`, `schema-only mismatch` (named with both ids), `type-cross constraint`, `empty group`,
  `group requires unmet`, `orphan lane`, `unhostable entry` (an artifact matches but no unit's
  `[hosts]` could take it), `presence unhostable` (a floor no set of deployed and hosting units can
  meet) — each an error, exit 1 — and `would bind by provisioning`, `unranked ranking`, `single
  provider` as warnings (`--strict-deployed` turns the first into an error). `--format json` emits
  the versioned document `mycelium.design/declaration/1` with the directory's git revision, so a
  consumer can join it to runtime observations by `(principal, ns, name, schema_id)`; `--format dot`
  draws it. Exit 2 when a file does not load. It says *would bind under these declarations*, never
  *is bound*. Fixtures: the co-op deployment as six unit files and one artifact description
  (`tests/fixtures/units/coop`, green with its library, one provisioning warning, a golden JSON), and
  an unwired one that exits 1 naming its ghost; CI runs both through the binary.
- **`docs/operations/capability-lifecycle.md`** — the two ways a capability arrives (directly
  deployed, dynamically installed) as one workflow: declare · check · deploy · publish · watch · read
  back what bound; `deployment.md`, `artifacts.md` and guide 02 point at it.
- **The unit file** (`NodeCapabilityConfig`, W1 of `docs/plans/design-time-tooling.md`): the
  `[[capability]]` TOML grows `principal`, `[[requirement]]` (a `CapFilter` — attributes with
  `eq · ne · gt · gte · lt · lte`, a bare value meaning `eq`, `schema_id`, a ranking),
  `[[group]]` (a `CapabilityGroupDef` with its name), `[[lane]]` (name and `produces | consumes`),
  `[[mandate]]` (holder, scope, enumerated operations), `[[rule]]` (the reference evaluator's
  fields), `[hosts]` (kinds, install budget, headroom, trusted publishers, placement root, fuel) and
  `[[presence]]` (a filter with `min_providers` / `max_providers`). A `Version` is written
  `{ version = "2.1.0" }` wherever a value goes; a bare string stays `Text`, so every existing
  file loads unchanged (pinned by `existing_capability_files_load_unchanged`). `load_from_file`
  now validates and refuses **by name**: an unknown operator, a version that is not
  `major.minor.patch`, a ranking order that is neither direction, a mandate that enumerates
  nothing, a presence floor of zero or a ceiling below it, an unknown hosted kind, a headroom
  outside `(0, 1]`. Nothing is declared to the mesh from the file yet (D18 / R1 is next); the
  sections are the vocabulary the offline check (W2) reads.
- **`DiskStagedSource` and `RangedBlobFetcher`** (`mycelium-wasm-host`): the large-artifact path.
  A blob is pulled in HTTP `Range` pieces (`HEAD` for the size; only a `206` is read — a store that
  ignores `Range` has its `200` dropped unread), hashed as it streams, written to a node-local
  staging directory, and served to the blob runtime from disk, so peak memory is one piece whatever
  the size. Complete-or-absent: a `.part-…` file is renamed into place only after the content
  address matches. Gate: `a_large_blob_stages_to_disk_within_a_memory_bound` asserts resident memory
  grows by under 32 MiB while staging 64 MiB (`MYCELIUM_S1_BLOB_MIB` scales it); it measures the
  process, so it is `#[ignore]`d and CI runs it alone. S1 of
  `docs/plans/design-time-tooling.md` §11; the object-store adapter (S2–S4) is still to come.

## [2.16.0] — 2026-09-26

**Settings where there were sentences.** Wire **v12** unchanged (`PREV = 11`); additive on the 2.x
line, with one validation tightening. Three external reviews in two days (a code review, a customer-
materials review, and the project's own doc-coverage and publication lints run against them) found
the same shape repeatedly: a runbook telling an operator to turn something that had no knob, a route
that discarded what it had computed, a capture path the docs implied and the tree lacked. This
release closes those, and carries the documentation those passes produced: the engagement kit, the
example paths, the supported consensus profile in the threat model, the buyer deck said straight.

**Upgrade notes.** `GossipConfig` gained `control_max_staleness_ms` and `control_min_peers_heard`,
which breaks an exhaustive struct literal (the `Default`+assignment pattern is unaffected). A config
that carries a family-wildcard scope such as `llm:*` now **fails `validate()`** — it never granted
anything, so rewrite it as the scopes it meant. `POST /gateway/kv`'s response gained fields beside
the unchanged `ok`.

- **`POST /gateway/kv` returns the write's receipt** — `operation_id`, `local_durability`
  (`on_disk` · `buffered` · `not_configured` · `failed`), `local_durability_error` when failed —
  beside the unchanged `ok`. Until now the route discarded the receipt, so no HTTP or SDK client could
  learn rung 2 for an ordinary set (doc-coverage run 17, code gap).
- **The governors' confidence bound is a setting**: `GossipConfig.control_max_staleness_ms` (30 000)
  and `control_min_peers_heard` (1), env `GOSSIP_CONTROL_MAX_STALENESS_MS` /
  `GOSSIP_CONTROL_MIN_PEERS_HEARD`, `validate()` refuses 0. Every governor now builds its
  `ConfidenceBound` from config (`ConfidenceBound::from_config`); before, all three hardcoded the
  default and the control runbook's "loosen the bound on evidence" had nothing to turn.
- **`GOSSIP_GATEWAY_NAMED_TOKENS`** — the named-token table from the environment
  (`name|token|scope,scope;…`), so the credential model the docs prefer is settable where
  `gateway_auth_token` is; a malformed entry refuses the whole variable at startup.
- **The node binary records a run** when built with `sim` and started with
  `GOSSIP_RECORD_BUNDLE_DIR=<dir>`: current-thread runtime under the replay seams, the bundle written
  at shutdown. Off in every shipped build; CI compiles the path (`cargo check --bin mycelium
  --features cli,sim`).
- `[[example]]` entries with `required-features = ["metrics"]` for `coordination_viz` and
  `control_envelope_viz`, so a bare `cargo run --example` fails loudly instead of running a hollow build.

### Changed
- **`validate()` refuses a family-wildcard scope** (`llm:*` and the like) on scoped tokens, named
  tokens and OIDC group scopes: scopes match exactly or the single `*`, and a token written the other
  way admitted nothing without anyone being told.

**Upgrade note:** `GossipConfig` gained two fields, which breaks an exhaustive struct literal; the
`Default`+assignment pattern is unaffected. A config that already carried a `family:*` scope now
fails `validate()` — it never granted anything, so rewrite it as the scopes it meant.

## [2.15.1] — 2026-09-26

**Two gates that never ran, and the docs saying what the code does.** Wire **v12** unchanged
(`PREV = 11`); no API change; a PATCH.

An independent customer-readiness review (2026-09-26, of `46ef0c9`) reported ten findings across the
substrate and its private companion. The public-side ones are here. Each fix carries a regression test
that was **seen failing before the fix** — the review's bar, and the one both gates below had been
missing: a documented guarantee treated as covered without an executed test of its failure boundary.

**Check before upgrading:** nothing to do. One behaviour change to know about: a stale
`MembershipIntent` used to hold its electorate floor **forever**; it now expires after 30 s, as its
documentation always said. A group that was refusing elections because of an intent nobody had
refreshed will elect after this upgrade.

### Fixed
- **The electorate floor never expired (review F7).** `declared_electorate_min` compared a Unix-ms
  `written_at_ms` against `mono_now_ns() / 1_000_000`. The monotonic seam's origin is a year in
  nanoseconds (~3.15e10 ms), three orders below any Unix timestamp, so the saturating subtraction was
  `0` for every intent and the TTL never fired — while the membership governor, reading the same key
  with wall time, had long since let it evaporate. Shipped in #377 (2.14.0). Now
  `sim_seam::wall_now_ms()`; `electorate_intent_tests` crosses the TTL (#425).
- **`tests/gateway_mandate_external.rs` panicked with no runtime (review F9).** Added in #399 and
  broken the same day by #417 (attaching an authority spawns the C10 sweeper); with a runtime it then
  failed on fixed timestamps that C8's `mark_started` correctly refuses as predating the start. Nothing
  noticed through the 2.15.0 tag because CI ran only the *other* root integration binary — `cargo
  check --all-targets` proves a test compiles, not that it passes. Now `#[tokio::test]` with
  post-attach stamps, **and a `run` line in `ci.yml`** (#425).

### Changed
- **No green-on-retry for the security gate.** `scripts/ci-retest.sh` gains `CI_RETEST_STRICT=1`: the
  isolated rerun still happens and is annotated, but a pass on retry fails the job. Applied to the
  `compliance,a2a` lib suite — the audit chain and both gateway enforcement points (#425).
- **Trust slices say what they do (review F8).** `declare_trust`'s comment, the `consensus.rs` header
  and guide chapter 4 said slices were stored for a future extension; the ballot loop has filtered the
  tally on the declared set under `use_trust_slices` all along. Restated as what it is — a fixed
  *eligible* voter set, quorum size unchanged, slice-based intersection not implemented — and the
  header states the **safety-sensitive profile**: fixed `quorum_size`, `use_trust_slices` on with
  every voter declaring the same set, `count_opaque_as_absent` off, membership changes outside the
  profile. The quorum is derived from what a node *observes*, so intersection across a membership
  change is an assumption, not a consequence of value-bound votes (#425).
- **Guide wording (review F10).** *"signals flow where they're relevant and nowhere else"* — admission
  is scoped, forwarding is not, and a boundary is not a confidentiality control; *"every higher-layer
  feature is stored in the gossip KV"* — the evidence journal (2.6.0) deliberately is not; the on-ramp
  pin example moves to the current tag; the pilot page's *"days old"* dated (#425).
- **Wiki:** two rules in `dev/testing/testing.md` — every `tests/*.rs` binary has a `run` line in
  `ci.yml`, and every documented expiry has a test that crosses it (#425).

### Not claimed
- The review's F8 (automatic quorum sizing is not a stable electorate) is answered by **restricting
  the supported profile and stating it**, not by a versioned electorate or a membership-transition
  protocol. Dynamic membership under safety-sensitive agreement remains roadmap work.

## [2.15.0] — 2026-09-26

**Authority at every door.** Wire **v12** unchanged (`PREV = 11`); additive on the 2.x line throughout.

**CHECK BEFORE YOU UPGRADE.** A client holding `mesh:write` could put `mcp.invoke` or `skill.invoke`
in the body of `POST /gateway/rpc/call` — or `scatter`, `signal/emit`, `mailbox/deliver`,
`shard/emit`, `overlay/emit_reliable` — and reach a provider **directly**, skipping the action
evaluator and every mandate check that `/mcp` and `/a2a` run. Same for `llm.invoke` around
`llm:invoke`. Those kinds now answer **`403 protected_kind`**, naming the door to use. If you have an
evaluator attached and a `mesh:write` token in circulation, that token had a way past it.

**Why this release exists.** v2.14.0 shipped a gateway that decides authority per call, and said so
honestly: it is *a route-level preflight at this gateway*, and **a provider reached another way is not
covered**. That sentence was a boundary, and this release closes it — the Boundary H closure plan,
C1–C12. The question throughout was not *can we enforce* but **where**: an enforcement point that can
be walked around by choosing a different door is not an enforcement point, and the only way to know
which doors exist is to enumerate them and try each one.

- **C1–C4, the doors.** The raw routes refuse protected kinds; a presented mandate **travels to the
  provider** so it can verify for itself rather than trust the gateway's word; a provider can run the
  gateway's own preflight as the enforcement point `provider`, deciding **where the work runs**; and
  cohort budgets are admitted at the provider, keyed by the verified principal.
- **C7, the matrix.** After a revocation, every door the code has runs nothing — `/mcp`, `/a2a` send
  *and* stream, the raw routes, a member's direct call, the SDK serve stream — by the handlers' **own
  counters**, with a plant proving the front doors reach those handlers beforehand.
- **C8–C12, the awkward cases.** A restart no longer restores revoked authority. The check-then-act
  window is answered **at each site**, with the limit stated once rather than denied. Work **already
  running** stops when its authority lapses, and the stop is **measured** against the bound its class
  declares — `examples/authority_drain`, in CI. Every read of `Hlc::current()` is classified and
  guarded, with a CI script that fails on an unclassified new one.
- **C5, the operator's answer.** A signed member removal from a configured authority names the node
  and every key it has held; an accepting node drops its pings, signals and gossip writes and refuses
  its certificate at the handshake. It spreads by gossip under `sys/membership/removed/` and is
  monotonic. `ConfinementReport::ca_key_off_node` reports **unmet** for a node holding the fleet CA's
  private key, because it could mint a removed member a new identity.

**And the clock, which was failing open.** Three sites deciding whether something had **expired** read
`physical_ms(hlc.current())` — a bare atomic load that never consults the wall clock and advances only
on gossip traffic. On a quiet or partitioned node it froze, so a mandate never expired and a checkpoint
never went stale. A partition is exactly when an expiry check must keep counting. `Hlc::decision_now_ms()`
is the fix, and C11 then classified **every remaining read** rather than leaving the next one to be found
the same way.

**Upgrade notes.** **New scope `mesh:serve`** for `rpc/serve` and `rpc/respond`, so a serving agent
needs no power to call — for **one release** a `mesh:read` or `mesh:write` token is still admitted
there, with a warning; move serving agents over now. Protected kinds on the raw routes answer `403`
(above). `GitStoreConfig` gained `authority`, so a struct literal without `..Default::default()`
breaks. `[[bin]] mycelium` now requires the `cli` feature, so `--no-default-features` builds skip it
rather than failing. `mycelium-ts` users on `rpcCall` were failing `400` before this release and are
not after.

**Not claimed.** The check-then-act window is **narrowed and stated, not eliminated**: a process
paused after its check can still write late by the length of the pause, which is why C9's answer is
per-site and why the wiki store counts `late_writes()` rather than promising zero. `clock_sync` in the
confinement report is always `Unverified` — the substrate cannot attest to an operator's time source.
Provider enforcement **fails closed without an evaluator**, which means a provider that attaches none
enforces nothing; and a mandate carried to a provider is *carried, not verified*, until that provider
verifies it.

- **The bypass matrix and the stop, in the cluster** (Boundary H closure plan C7 and C12, deployment variants).
  - The confined-fleet job now builds a node image (`docker/Dockerfile.confined-fleet`, `examples/confined_fleet_node`).
  - Its phase 2 runs that image as a mandate authority, a provider member and the reference gateway.
  - From the agent pod, the checks are:
    - revoked work runs through no door, by the provider's own counters;
    - the raw routes refuse protected kinds;
    - the member plane is unreachable;
    - running work stops within its declared bound plus checkpoint delivery.
  - The reference network policy lets other members reach the gateway's gossip port.
  - `.dockerignore` skips local worktrees and nested build outputs.
- **The stop, measured** (Boundary H closure plan C12). `examples/authority_drain` (run in CI) admits calls under a
  mandate on a live node, revokes the term, and checks T_admit and T_drain from the node's own records against the
  bound the class declares. **`mycelium::arguments_digest`**: the canonical arguments digest, public in Rust as it
  already was in both SDKs, pinned to the same golden vector. `WorkGuard` is re-exported.
- **An operator can remove a member** (Boundary H closure plan C5, ADR `docs/design/member-removal.md`).
  - A `SignedMemberRemoval` from a configured membership authority (`with_membership_authorities`,
    `offer_member_removal`) names the node and every key it has held.
  - A node that accepts it drops the member's pings, signals and gossip writes, and refuses its certificate at the
    TLS handshake.
  - RPC receive refuses the member (`CallerError::Removed`), and its keys read as revoked, so A1 refuses a mandate
    it holds.
  - The removal spreads by gossip under the new reserved namespace `sys/membership/removed/`, verified at every
    node. It is monotonic.
  - `ConfinementReport::ca_key_off_node`: a node holding the fleet CA's private key reports it unmet, since it
    could mint a removed member a new identity.
- **The bypass matrix** (Boundary H closure plan C7). After a revocation, every door the code has runs nothing, by
  the handlers' own counters: `/mcp`, `/a2a` send and stream, `/gateway/rpc/call` and `signal/emit` with protected
  kinds, a member's direct call, and the SDK serve stream. The plant: before the revocation the front doors reach the
  handlers.
- **Every read of `Hlc::current()` classified, and guarded** (Boundary H closure plan C11).
  - The seen-set TTL sites fail closed on a frozen clock; the rest are stamps.
  - `consensus::causal_now_ms` uses `decision_now_ms`.
  - New `scripts/check-hlc-current.sh` (CI) fails on an unclassified new read.
  - `ConfinementReport::clock_sync` (always `Unverified`).
  - `Hlc::force_state_for_tests` (test support).
  - An end-to-end test: a gateway whose HLC is an hour behind still refuses an expired mandate.
- **Work already running stops when its authority lapses** (Boundary H closure plan C10). A provider with enforcement
  on registers calls acting under an established mandate with its `ExecutionAuthority` (`begin`, `WorkGuard`), and
  the agent sweeps them every `sweep_interval_ms()`, re-running A1's check. The first failed check cancels the
  work. MCP tool handlers are dropped and answered; `rpc_rx` loops race the new `RpcRequest::authority_lapsed()`.
  Stops are recorded as requested, acknowledged and confirmed (`stop_records()`), for `drain_report`. Lock-order rows
  48 and 49.
- **The check-then-act window, answered at each site** (Boundary H closure plan C9). `GitStore::late_writes()`
  counts commits that landed after the authority lapsed, since the store now asks again once a commit has landed.
  A reference pre-receive hook, `mycelium-wiki/hooks/pre-receive-mandate-window`, refuses a push once the
  appointment on the mandate ref has expired by the remote's clock. It is configured with `mycelium.mandateRef`,
  fails closed, and allows reappointment. The ADR's new §9 states the limit once, with each site's answer.
- **A restart no longer restores revoked authority** (Boundary H closure plan C8).
  - `RevocationView::started_at`: a restarted reader refuses, for freshness, checkpoints issued before it started, so
    a replayed pre-revocation checkpoint cannot make a revoked term read as current. It relies on checkpoints being
    cumulative, which is the authority's contract.
  - `DurableEpochs` keeps installed epochs in the node-local journal, journalled before they take effect and
    reloaded as a floor, via `ExecutionAuthority::with_durable_epochs` / `install_epoch_durably` and the wiki's
    `ExecutionGateAuthority` equivalents.
  - `CheckpointOffer::IssuedBeforeStart`. Lock-order row 47.
- **`FsStore` takes the authority-at-execution seam** (Boundary H closure plan C6). `FsStore::with_authority` asks a
  `WriteAuthority` (for example `ExecutionGateAuthority`) before each of its four mutations; a refusal writes nothing.
  `WriteAuthority` moved to the always-compiled store module and is re-exported at `mandate_fence::WriteAuthority`
  and the crate root.
- **Cohort budgets enforced at the provider** (Boundary H closure plan C4, wiring H6). `with_cohort_budget(budget,
  view, external)` admits every protected call a node receives through H6's `CohortBudget`, keyed by the verified
  principal, holding its place while the call is in flight, and refusing a full pool `at_capacity` (`-32004`).
  `offer_cohort_declaration` feeds the provider's view; `cohort_budget_refusals()` counts refusals. Lock-order rows
  45 and 46.
- **Authority decided at the provider** (Boundary H closure plan C3). `GossipAgent::with_provider_enforcement()`
  makes a provider run the gateway's own action preflight, as the enforcement point `provider`, on every protected RPC
  it receives: in the MCP tool loops, in every `rpc_rx` receiver, and in the SDK serve stream. Its own evaluator
  decides, and its own `ExecutionAuthority` verifies any presented mandate. A member calling a provider directly is
  now decided where the work runs, not only at a gateway. Fails closed without an evaluator. The caller envelope
  carries a resource claim, which the provider accepts only for itself and only for what it serves.
  `rpc_call_with_mandate` takes the resource it acts on.
- **A presented mandate travels to the provider** (Boundary H closure plan C2). The caller envelope carries the
  mandate a client presented (`params._meta.mandate` on `/mcp` `tools/call` and `/a2a`), so a provider can verify it
  for itself rather than trust the gateway's word. A member acting under its own grant calls a provider directly with
  the new `ServiceHandle::rpc_call_with_mandate`. Providers read it with `GossipAgent::presented_mandate(&req)`,
  carried, not verified. The envelope field is absent when no mandate is presented, so older providers see no change.
- **The gateway's raw routes refuse protected work** (Boundary H closure plan C1). `rpc/call`, `scatter`,
  `signal/emit`, `mailbox/deliver`, `shard/emit` and `overlay/emit_reliable` take an RPC kind from the body, and a
  client with `mesh:write` could send `mcp.invoke` or `skill.invoke` through them straight to a provider, skipping
  the action evaluator and mandate checks `/mcp` and `/a2a` run (and `llm.invoke` around `llm:invoke`). These
  kinds, and any in the new `protected_rpc_kinds` (`GOSSIP_PROTECTED_RPC_KINDS`), are now refused
  `403 protected_kind`, naming the door to use. **New scope `mesh:serve`** for `rpc/serve` and `rpc/respond`, so a
  serving agent needs no power to call; for one release a `mesh:read` or `mesh:write` token is still admitted
  there, with a warning. **SDKs:** `ProtectedKindError` in `mycelium-py` and `mycelium-ts`.
- **Authority at execution at the wiki's git store** (Boundary H A1; ADR `docs/design/authority-at-execution.md` §7).
  The mandate fence stopped a *superseded* curator inside the git transaction. It could not stop an expired, revoked
  or out-of-touch one. Now:
  - `GitStoreConfig::authority` holds an optional `mandate_fence::WriteAuthority`. It is asked on every commit
    attempt, immediately before the ref transaction, and before every push attempt;
  - `ExecutionGateAuthority` (new feature `execution-authority`) implements it over A1's `ExecutionGate` for a
    mandate enumerating `wiki.write`;
  - a refusal is `WikiError::authority_refused`, and the curator **leaves the proposals queued** rather than
    dropping them.

  `None`, the default, is unchanged behaviour. **Upgrade note:** a `GitStoreConfig` built as a struct literal
  without `..Default::default()` needs the new field. Lock-order row 44.
- **`examples/a2a_skill_authority`** (#406) — the `/a2a` enforcement demonstration, and the gallery's counterpart to
  `mcp_tool_authority`. Seven acts against a real gateway with a real provider behind it: `/mcp` answers 401 without
  a bearer while `/a2a` answers the same caller; a permitted dispatch, a denial on an argument value, a denial by
  remit, an **indeterminate** refusal, a `-32001` that fires *before* the evaluator (so no evidence record claims a
  policy decided it), and a refusal over `tasks/sendSubscribe`. The point it exists to make: an enforcement point
  that can be walked around by choosing a different door is not an enforcement point. `docs/guide/20` gains the
  matching section; CI runs it.

- **Two partners on one edge are gated** (federation, a test-only change). Every per-partner guarantee was
  tested with **one** partner configured, where "the key for the claimed origin" and "the only key in the
  bundle" are the same key. A planted bundle-wide key search left all 24 federation tests green while letting
  one partner mint credentials in another's name. The new gate catches it as a `200` where a `401` belongs.
  Now checked with two partners on one edge: per-partner catalogues, grants that do not cross, a trusted
  partner that cannot speak for another, per-partner revocation, and a common neighbour that is not a bridge.
  It complements `three_domains_compose_without_trust_composing` (a chain) with the star. Written 2026-09-23,
  recovered from an uncommitted worktree on 2026-09-25.

### Fixed

- **Five examples declare the features they use** (`a2a_skill_authority`, `coordination_integrity`,
  `distributed_lock`, `llm_agent`, `three_node_demo`), so `--all-targets --no-default-features` builds skip them
  rather than failing.
- **`--no-default-features` builds no longer fail on the node binary.** `[[bin]] mycelium` now requires `cli` (its
  `tracing-subscriber`), so builds without `cli` skip it. CI's no-default-features clippy covers `--bins --tests` too.
- **`/a2a` `tasks/sendSubscribe` ignored a presented mandate.** The streaming path passed no params to the action
  preflight, so a mandate-requiring rule refused every stream call even though both SDKs offer `mandate=` on
  `stream`. It now passes the request's params, exactly as `tasks/send` does.
- **`mycelium-ts`: `rpcCall` sent `kind` where the gateway reads `method`**, so every call failed `400`. The
  live test that would have shown it only runs against a gateway. Now sends `method`, pinned by a stub test.
- **A1: a revocation now stands against later checkpoints that omit it.** `RevocationView` kept only the newest
  checkpoint per `(authority, scope)`, so a later fresh checkpoint without the term reinstated a revoked appointment.
  The ADR's "revocation is monotonic" held across time, but not across checkpoints. The view now keeps every revoked
  term apart from the newest checkpoint. This affects the gateway's `ExecutionAuthority` and the wiki store alike.
  Gate: `a_later_checkpoint_that_omits_a_revocation_does_not_reinstate_it`.
- **A1: a revocation counts whatever order its checkpoint arrives in.** The view discarded an older
  checkpoint as a replay *before* reading its revocations, so checkpoints delivered out of order could lose
  one. An authentic revocation is now recorded before the replay and future-dating checks, which decide only
  whether a checkpoint refreshes freshness. Gate: `a_revocation_in_a_late_arriving_older_checkpoint_still_revokes`.
- **Docs: two A1 statements corrected.** *F* − 2*s* is the reader-clock threshold, not a real-time partition
  bound (in real time, work stops when the checkpoint is at most *F* old). The wiki store's check comes before
  its git transaction; a paused process can write late by the pause. That is now a stated limit pinned by
  `a_pause_after_the_check_is_not_caught_locally`, not "not a gap". A restart's fail-closed holds only until a
  checkpoint arrives (closure plan C8). All three from an external review.
- **Authorization decisions read a live clock, not a frozen one** (#405). Three sites deciding whether something
  had **expired** read `physical_ms(hlc.current())`. `Hlc::current()` is a bare atomic load: it never consults the
  wall clock and advances only when `tick()` runs, which happens on gossip traffic. On a quiet or partitioned node
  it therefore freezes at whenever the node last heard from anyone — and each site failed **open**: a mandate never
  expired (the AE preflight's `now_ms`), an issuance timestamp stayed fresh (`GatewayCaller::issued_at_ms`), and a
  checkpoint never went stale (`offer_checkpoint`). A partition is exactly when an expiry check must keep counting.
  New `Hlc::decision_now_ms()` = `wall_now_ms().max(physical_ms(current()))` — deliberately **not** `tick()`, so
  taking a reading never mutates the HLC, and floored by the HLC so a peer that has seen the future cannot make this
  node see the past. `wall_now_ms` is the sim seam, so replay determinism is unchanged. Gates:
  `hlc::decision_clock_tests`.
- **`/a2a` warns when it mounts as an open surface** (#403). Unlike `/mcp`, which requires the `mcp:invoke` scope,
  `/a2a` has **no scope floor** — it is public by design, which is what makes it an interoperable A2A endpoint. With
  neither an action evaluator nor any gateway bearer configured, an anonymous caller reaches skill dispatch.
  `with_a2a()` now says so at mount time rather than leaving it to be discovered. An open surface is a deployment
  decision; it must not be an accident.
- **A terminal task state is the last thing an A2A subscriber hears** (#408). `tasks/sendSubscribe` spawns a
  detached task that sleeps 100 ms then emits `working`. It was spawned *before* the outcome was known and nothing
  cancelled it, so an outcome reached inside 100 ms was overtaken by its own progress report: an AE refusal resolves
  in about a millisecond, and the stream read `submitted → failed → working` — a terminal state followed by a
  non-terminal one, which reads as a task recovering from failure. The success path had it too (`completed →
  working` for any dispatch under 100 ms), so this predates the AE seam and is not about refusals. The handle is now
  aborted before every terminal event. Gate: `no_event_follows_a_terminal_task_state_on_the_stream`, which asserts
  the **order** of states rather than forbidding the event — `working` before a terminal state is correct on a slow
  skill. Found by `examples/a2a_skill_authority` on its first CI run.
- **The replay forbidden-call gate never exempted `#[cfg(all(test, …))]` modules** (#408). The test-module skip
  matched the literal `#[cfg(test)]` only, so the nine modules written that way were counted as **production** and
  their clock and filesystem calls sat in `scripts/sim-seams-baseline.txt` as if they were admitted replay debt.
  A gate that fires on correct code teaches people to bump the baseline without reading it, which is the mirror of
  the false pass the first fix addressed. `src/agent/http.rs` 38 → 15, total 181 → 158; **no production site
  changed**. Recorded as §6's third gap of this family, and the first to over-count.

## [2.14.0] — 2026-09-25

**Coordination that says what it means.** Wire **v12** unchanged (`PREV = 11`); additive on the 2.x
line throughout.

The release began with a question in a design note — *is role accumulation constrained anywhere?* —
and the answer was **no**, twice over. Not prevented, which is the design working: *detection, not
prevention* is the law here. But **not detected either**, which is not. Every single-writer ring
elected by *lowest candidate node id wins*, so the same rule over the same candidates put **every
single-writer job on one node**, and the seven existing detectors all watched other axes — P2 churn,
P6 gaps. A concentrated fleet read as **perfectly healthy by every measurement that existed**, until
the node it all depended on went away. **P10** makes it visible; **`mycelium::election`** stops
producing it, with the rule **negotiated from the candidate set** rather than flag-dayed.

**Then a harder question, and it turned over a rock.** An attempt to make
`require_identity_proofs` default-on was reverted a day later, and the failure it was blamed on
turned out not to be its doing — which sent us to look at the election path properly. What was found
there is the substance of this release:

- an election over a roster a node **cannot see** was counted as one member with a quorum of one,
  satisfied by the proposer's own vote, so **every node elected itself**;
- a **vote did not say what it voted for**, and ballots come from a shared key, so two proposers
  could count the same voter and commit **different values at one ballot**;
- a node could **equivocate with itself**, because its proposer and acceptor roles kept separate
  memories;
- a proposer at a higher ballot **overwrote** a value a quorum had already accepted;
- and none of that memory **survived a restart**.

All five are closed. Beside them, a node's identity and its proof now travel as **one record**, so
they cannot arrive apart; a leadership answer **names the rung it reached** and carries a fencing
token; and `POST /gateway/kv` no longer succeeds at writing nothing.

**The honest limit, stated on the tin.** None of this makes leadership an exclusive grant that stays
true — no coordinator-free protocol can promise that. LWW decides which *record* survives; it cannot
undo work two callers each performed after being told they had won. **Exclusivity is enforced at the
resource**, by refusing a stale token, and the substrate's job is to give you a token worth
refusing.

**Upgrade notes.** `ConsensusResult` is now **`#[non_exhaustive]`**; `CommitError` and
`ConsistencyError` gain variants — a `_` arm is required and **must fail closed**. `FleetSnapshot`
gained `role_concentration`. A client that omitted `value_b64` on `POST /gateway/kv` now gets a
**400** where it used to get `{"ok": true}` (every first-party SDK already sends it). Election
behaviour changes only once every candidate advertises `election_rule`. **No identity-proof
action** — the default is unchanged from 2.13.0.

**Rolling upgrade.** While any voter is un-upgraded, group proposals from an upgraded proposer
**time out** rather than commit: fail-closed and visible, instead of committing two values quietly.

**Not claimed:** rendezvous is a **spread, not a bound** (three rings over three nodes still leave
~11% chance one node wins all three), which is why P10 stays. *Proofs required* is **not** *identity
authenticated* — first sighting remains trust-on-first-use, which anchors close, not proofs. And the
agreement repair establishes single-decree safety; it is not a proof of the whole protocol.

- **`with_a2a()` warns when it mounts an open surface.** `/a2a` has **no scope floor** — unlike
  `/mcp`, which requires `mcp:invoke`. Its auth is *optional* by design: a federation credential
  names a partner, a bearer resolves to a principal whose **scopes are deliberately dropped**
  (authority on that route comes from policy, not the scope table), and nothing presented is
  anonymous.

  With an `ActionEvaluator` attached that is a finer instrument than a scope. With **no** evaluator
  the seam is inert, and an anonymous caller reaches **skill dispatch** — established by probing a
  live node rather than by reading the auth branch: an unauthenticated `tasks/send` came back
  `skill not found`, meaning it had passed every gate and failed only on resolution.

  That is fine on a trusted LAN and is not fine on an untrusted network, and nothing said so where
  an operator would look. `with_a2a()` now warns when it mounts with **neither** an evaluator
  **nor** any bearer configured, naming both ways to close it — the same pattern
  `with_action_evaluator` already uses for a missing evidence journal (*enforcement without
  attribution is not governance*).

  One mitigation already existed and is worth knowing: `/a2a` is mounted **only** by an explicit
  `with_a2a()` call, never by the `a2a` feature alone.

- **Seven runnable demonstrations of everything above — and CI *runs* them, it does not merely build
  them.** Building an example proves the API still compiles; running it proves the demonstration
  still demonstrates, which is the thing a gallery is a gate on. CI-run examples went from 6 to 12.

  | Example | The claim it makes checkable |
  |---|---|
  | `coordination_integrity` | absence is not authority · an answer names its rung · exclusivity is fenced **at the resource** |
  | `coordinator_by_accretion` | one rule over one candidate set has one winner, every ring; rendezvous spreads; a spread is **not a bound** |
  | `coordination_viz` (`:8100`) | the same, watchable — a real three-node fleet and a live concentration gauge |
  | `federation_trust_is_not_transitive` | a trust bundle does not compose · a grant you hold is not re-exportable · budgets are pairwise |
  | `identity_one_record` | two entries are two messages; one entry cannot arrive in two parts |
  | `auditor_questions` | who asked · whether the record stands · whether one person can leave |
  | `mcp_tool_authority` | which **tools** an agent may actually call, per call, including the undecided case |

  `receipt_ladder` also gains the rung production actually hits: a **lost acknowledgement**, where
  `prepare_write`/`commit_prepared` lets a retry lose LWW instead of undoing a newer value.

  The Ops Console gains an **Integrity** tab. The coordination and identity properties are
  *refusals*, and you cannot watch a thing not happen — so it shows the **preconditions** (can this
  fleet elect? is it safe to require proofs yet? is one node holding everything?), composed entirely
  from endpoints the console already read.

- **Mandates established at the gateway** (Boundary H A1, gateway wiring; ADR
  `docs/design/authority-at-execution.md` §6). The gateway always bound `mandate: None`, so a policy rule
  requiring a mandate could never be satisfied there. `GossipAgent::with_execution_authority` (plus
  `offer_revocation_checkpoint`) makes `ae_preflight` assess a grant presented in `params._meta.mandate`
  for `/mcp`, `/a2a` and federation calls:
  - its holder must be the **authenticated caller**;
  - P2 checks it, with possession bound to this call's operation, resource and arguments digest;
  - A1's gate checks it at dispatch;
  - the result is bound as `Established`, `Refused` or `Unknown`, and the envelope window is clamped to
    the mandate.

  No authority attached means unchanged behaviour. **SDKs:** `mycelium-py` and `mycelium-ts` gain
  `mandate=` on `A2aClient.send`/`stream`, and `arguments_digest`/`argumentsDigest` and
  `mandate_request_bytes`/`mandateRequestBytes`, with golden vectors shared across Rust, Python and
  TypeScript. Lock-order row 43. `ExecutionAuthority`, `PresentedMandate`, `possession_request`,
  `mandate_operation` and `resource_key` are re-exported at the crate root, and an external-crate test
  (`tests/gateway_mandate_external.rs`) keeps them constructible from outside.

- **Authority at execution** (Boundary H plan, item A1; ADR `docs/design/authority-at-execution.md`).
  Letting a mandate expire stopped new admissions, but not work already admitted. `mandate::authority`:
  - `ExecutionGate::check` re-establishes authority at admission, dequeue, retry and each
    re-authorisation checkpoint, re-running `ResourceAuthority::check`. `strict` refuses advisory
    resources, and parameters that break *F* > 4*s* or *I* + *D* ≤ *F* − 4*s*.
  - `AuthorizedWork` clamps work to its mandate's window, and `delegate` never extends it.
  - Authority-signed `RevocationCheckpoint`s, issued even when empty, feed a `RevocationView` under the exact
    freshness predicate. Replays are refused, silence and stale views deny, and a revocation stands once seen.
  - `Continuation` and `StopContract::drain_bound` give a bound or `Unbounded`; `drain_report` keeps
    T_admit and T_drain apart, and counts unconfirmed stops as unconfirmed.
  - `ReferenceEvaluator::allowances_without_mandate` lists policy holes.

  **Not claimed:** resources must call the gate (none is wired yet); the revocation view is in memory;
  timing is not measured in a deployment.

- **Signed mandate grants (P2) and capability advertisements bound to authority (H3)** (Boundary H;
  ADR `docs/design/knowledge-issuer-binding.md` §5–6).
  - **P2.** `mandate::grant`: `Mandate::canonical_bytes`/`digest`, `SignedMandateGrant`,
    `EntitlementTable`, `GrantVerifier::check`, which checks issuance (through issuer binding), **configured**
    entitlement, the window, currency (highest epoch retained per scope; conflicting equal epochs back
    nothing), and **possession** bound to the specific request. Verdicts are `GrantVerdict`.
  - **H3.** `mandate::protected::filter_protected` keeps a capability in a `ProtectedNamespaces` entry only if
    its advertiser presents a valid grant (`mycelium.grant`) that names it as holder and permits
    `serve:{ns}/{name}`, with a possession proof bound to that advertisement (`mycelium.grant.possession`).
    Everything else is filtered and reported as an `UnbackedCapability`. `attach_grant` is for advertisers.

  **Not claimed:** entitlement beyond configuration (the consensus gate); colluders using their own
  policy; metric or audit wiring of the report.

- **Cohort budgets** (Boundary H plan, item H6; ADR `docs/design/knowledge-cohorts.md` §6).
  `knowledge::cohort_budget::CohortBudget::admit(caller, view, now)` gives an RAII `CohortSlot` or
  `CohortRefusal::AtCapacity`. It caps a **declared population's** calls in flight at a provider, where
  per-caller caps let a colluding fleet overwhelm it together.
  - Membership comes from the provider's own trusted `CohortView`, never from the caller.
  - A caller in several cohorts must fit every cap, and a refusal takes nothing.
  - Undeclared callers share one pool.
  - Lock-order row 42.

  **Stated limits:** per provider instance, concurrency not rate, only calls admitted through it.

- **The confined-fleet profile** (Boundary H plan, item H7; ADR `docs/design/confined-fleet.md`; runbook
  `docs/operations/confined-fleet.md`). For admitting untrusted agents as members:
  - **reference manifests** (`deploy/confined-fleet/`): agents and gateways in separate pods, the member key
    only in the gateway pod, and agent egress restricted to the gateway's API port and DNS;
  - **a node self-report**, `GossipAgent::confinement_report()` → `ConfinementReport`: each node setting
    (`egress.allow_hosts`, `require_identity_proofs`, audit sink, evaluator, evidence journal) as `Set`,
    `Unset` or `NotInBuild`, and network confinement always `Unverified`;
  - **a deployment test**, `make test-confined-fleet` and a CI job: kind with Calico, the manifests
    unchanged, and a positive control for every blocked path.

  **Not claimed:** confinement on a CNI that does not enforce NetworkPolicy; instance metadata (kind has
  none, so the check is reported N/A); recording through the gateway end to end (the test uses a stand-in
  gateway).

- **Challenge admission** (Boundary H plan, milestone M2, item H1; ADR `docs/design/knowledge-cohorts.md` §4).
  One challenger of any group could jam a verdict, and a population could hold every honest release in
  `Rejected`/`Conflicted`.
  - `ReaderPolicy::min_challenge_groups` (default 1, today's behaviour) counts challenges by the **same
    components as support**.
  - Two routes past the threshold, kept distinct: the provider **disowning** its own release
    (`RejectionReason::DisownedByProvider`, mechanically verified), and a **policy-decisive** source
    (`decisive_sources`, `RejectionReason::DecisiveSource`).
  - `Classification` gains `challenges: ChallengeReport` (records, groups, admitted, `evidenced_unadmitted`,
    a bounded sample), `excluded_total`, and `budget_exhausted`.
  - `max_examined` caps a resolution's reads (past it the verdict is `InsufficientEvidence`), and
    `max_reported` caps the reported detail.

  **Behaviour note:** `Rejected` lists one reason per challenger rather than per record, capped at
  `max_reported`.

- **Knowledge cohorts, declared at admission** (Boundary H plan, milestone M2, item H5; ADR
  `docs/design/knowledge-cohorts.md`). An operator the reader trusts signs a `CohortDeclaration` naming a
  fleet, and `ReaderPolicy::cohorts` (a `CohortView`) makes its members **one control group**.
  - Grouping is now **connected components** over configured groups, declared cohorts and each record's
    placement, so overlapping groups merge and the result never depends on order.
  - **Dependence is sticky:** expiry and partition mark a declaration stale and never remove a member.
    Only a superseding declaration does, and only for later evidence.
  - Records are grouped by membership at issue time **and** now.
  - New `UndeclaredRule` (`OwnGroup` default, `OneGroup`, `Excluded`) and `StaleRule` (`Retain` default,
    `Exclude`), with new exclusions `Undeclared` and `StaleCohortOnly`.

  **Not claimed:** lineage, which is a different relation, and durability (`CohortView` is in memory).

- **Durable knowledge stores** (Boundary H plan, milestone M2, item K3a; ADR
  `docs/design/knowledge-validity.md` §4). `knowledge::durable::DurableKnowledgeStore` and
  `DurableHeadCheckpoints` sit on the node-local journal (fsynced, never gossiped), which adds **no new
  filesystem site** and no lock.
  - Every verified record and every checkpoint advance is **fsynced before it is reported**, and memory
    is unchanged if the journal refuses (`PutRefusal::NotPersisted`, `HeadVerdict::CheckpointNotPersisted`).
  - Reopening replays the journal, restores records with their attributions, and still refuses a
    rolled-back head.
  - **An unreadable journal refuses to open**, rather than starting empty and resetting rollback
    protection.
  - The in-memory cores gain two-phase methods: `KnowledgeStore::verify_signed`, and `evaluate`/`apply`
    on `HeadCheckpoints`.

  **Not claimed:** the journal is never compacted, forks are in memory only, and nothing moves between
  nodes yet (K3b).

- **Signed stream heads with verified ancestry** (Boundary H plan, milestone M2, item K2; ADR
  `docs/design/knowledge-validity.md` §3). Heads were unsigned, and `advance_head` took any higher `seq`,
  so head 12 from a branch that diverged at 9 passed as "newer". Now:
  - `Head` gains `prev` (the previous head's digest), its own tagged `canonical_bytes`, and `digest`;
  - `knowledge::heads::HeadCheckpoints::offer` authenticates a `SignedHead` through issuer binding, then
    advances the reader's per-stream checkpoint **only** through a chain of signed heads back to it;
  - it reports `ContinuityUnavailable` (the checkpoint stays), `ForkedStream` (both heads kept),
    `StaleHead`, `AlreadyHeld` or `SignedUnderRevokedKey`;
  - checkpoints load through a `CheckpointStore` when opened (unreadable state **refuses to open**) and are
    persisted **before** an advance is reported;
  - `issuer::verify_signed_by` exposes P1's two paths for any signed bytes.

  **Not claimed:** the only store shipped, `MemoryCheckpointStore`, is **not durable**. A file-backed one
  goes through the filesystem seam with K3. **Upgrade note:** `Head` gained a public field, so an
  exhaustive struct literal needs `prev`.

- **Present eligibility at resolution** (Boundary H plan, milestone M2, item K1b; ADR
  `docs/design/knowledge-validity.md` §2). `resolution::classify_eligible(…, members, external)` returns
  `Classification { verdict, excluded }`. It re-derives eligibility on every call, re-verifies each
  verified record's retained signature against the reader's **current** key view, and reports every
  excluded record with an `Exclusion`:
  - authenticity: `Unchecked`, `NotAttributableNow`;
  - present authority: `KeyRevoked`;
  - currency: `Retracted`, `SupersededByIssuer`, `BasisWithdrawn`, `Expired`.

  `ReaderPolicy::unchecked` (`UncheckedRule::Count`, the default, or `Exclude`) decides whether records
  stored without verification count. `DependencyIndex::build_filtered` builds the currency index over
  authentic records only, so an unverified "retraction" cannot suppress verified support.

- **Knowledge store verifies on storage** (Boundary H plan, milestone M2, item K1; ADR
  `docs/design/knowledge-validity.md`). `KnowledgeStore::put_signed(SignedRecord, members, external)`
  checks two things before storing:
  - **integrity:** the id is the content's digest, and the record satisfies `KnowledgeRecord::new`'s rules.
    A deserialised record never went through `new`, so it could previously carry a stale id or a smuggled
    foreign retraction.
  - **attribution:** through issuer binding's two admissible paths.

  Each record is stored with an `Attribution` snapshot (`Verified { path, key, revoked_at_storage }`) and
  its signature is retained. Refusals (`PutRefusal`) are counted by label (`refusal_counts`), never
  silent. A record signed under a since-revoked key is stored as history and flagged.

  **Additive:** `put` is kept, and its records are marked `Attribution::Unchecked`. Whether unchecked
  records count is K1b's resolver policy, which is not yet built.

- **Knowledge issuer binding** (Boundary H plan, milestone M2, item P1; ADR
  `docs/design/knowledge-issuer-binding.md`). An `IssuerId` was an opaque string and
  `KnowledgeRecord::verify` took the key from its caller, so **one member could be many issuers**.
  `knowledge::issuer::verify_issuer` now attributes a record through exactly two admissible paths:
  - **member:** `IssuerId::for_node`, `node:{node_id}`, checked against the *reader's* retained identity
    keys for that node;
  - **configured external:** `TrustedExternalIssuers`, which refuses the reserved `node:` namespace.

  It reports **`Current` / `Revoked` / `Unverifiable`**. A record signed under a since-revoked key is
  still attributable as history, and carries no present standing. `GossipAgent::knowledge_member_keys`
  supplies the reader's view, and `GossipAgent::sign_knowledge_record` signs **only as this node**
  (both `compliance`).

  **Not claimed:** the member path's strength rests on `require_identity_proofs`, which is
  **default-off**. The store does not yet verify on `put` (K1).

- **P10 reaches the operator** — the half of it that was missing. The detector, the `/stats` field
  and the event ring shipped first; the Prometheus gauge, the `diagnose_fleet` rule and the runbook
  did not, which made it **a mechanism nothing wires** — the exact failure this project keeps
  finding in its own gates, committed one level up. `/gateway/diagnose`, the *"why is the fleet in
  this state"* answer, would have stayed silent about the pathology that had just been added.

  Now: `mycelium_emergent_role_concentration_pct` beside its sibling gauges, a `diagnose_fleet`
  finding that **names who, how much, and what to do**, a `narrate` gloss so `explain` reads it in
  plain language, a `diagnostics.md` recipe, a `metrics.md` row and a Prometheus alert — `for: 30m`
  and `warning`, deliberately: this is a standing structural condition, not an incident, and it
  should be noticed at the next review rather than at 3am.

  The finding says the unusual part out loud — *"nothing is failing: this reads as healthy on every
  other detector, which is why it is easy to miss"* — because an operator who sees a warning with
  no symptom will otherwise assume a false positive. And the first action it gives is **check the
  fleet finished upgrading**: one node too old to advertise `election_rule` pins its whole ring to
  `lowest id wins`, which concentrates by construction, so the commonest cause is a half-done
  rollout rather than anything an operator must fix by hand.

- **P10 — "a coordinator by accretion", and the election rule that stops producing one.** A
  discussion note asked whether role accumulation is constrained anywhere. It is not — and the
  answer turned out to have two halves, neither of which was what the note expected.

  **It was not detected either.** Missing *prevention* is fine here; "detection, not prevention" is
  the substrate's law, so a cap was never the expectation. A **tripwire** was, and none of the seven
  pathologies could see this one: **P2** watches role *churn* (a node calmly holding everything
  produces none) and **P6** watches coverage *gaps* (here every capability has a provider — merely
  the same one). Both are the orthogonal axis, so a concentrated fleet read as **perfectly healthy
  by every measurement that existed**, until the node it all depended on went away.

  **And it was the default, not an edge case.** The tuple space, the blackboard and the wiki each
  elected by *lowest candidate node id wins* — the same rule over the same candidates returns the
  same winner, so a fleet where every node runs every companion put **every single-writer job on one
  node**, deterministically, on first election and again after every restart. Losing that node moved
  every role at once, **as a block**, to the next-lowest id.

  Both halves are closed:

  - **`detect_role_concentration` (P10)** — the share of live single-writer roles (`.primary` /
    `.curator`) held by one node, hysteresis-confirmed like P1 and P6, exposed on `/stats` as
    `role_concentration_pct` and recorded in the event ring so `explain` can narrate it. The gauge
    carries the share **whether or not it trips**, because an operator watching it climb from 40 to
    55 has a warning a boolean would withhold until it was already true. It has a **partition
    guard**: a node that has lost sight of its peers sees only its own roles — the pathology's exact
    shape — so the reading is withheld below two visible holders.
  - **`mycelium::election`** — rings order candidates by `hash(ring, node)` (rendezvous), so
    different rings pick different winners and a failover spreads rather than relocating in one
    block. `LowestId` is kept, because a mixed fleet must still agree.

  **The rollout is negotiated, not flag-dayed**, and this is the part that made the change
  non-trivial. The companions' safety rests on every node computing the *same* answer — the wiki's
  split-brain sentinel says so in as many words. Deploy a new rule node-by-node and an old node and
  a new one would each believe they should hold the role and **neither would resign**: a stable
  two-holder state lasting as long as the rollout. So each candidate advertises the rules it can
  compute (`election_rule`, a capability attribute — no new gossip), and every elector uses the
  **minimum across live candidates**. One old candidate pins the whole ring to the old rule; the
  ring flips by itself when the last one leaves. What that does *not* remove is a
  **convergence-length** window where two nodes briefly disagree about the candidate set — bounded
  by seconds rather than by the rollout, and the same transient the sentinel already exists to
  resolve.

  **Not claimed:** rendezvous is a spread, **not a bound**. Three rings over three nodes still leave
  ~11% chance one node wins all three and ~78% chance somebody holds two. That is precisely why P10
  stays — mitigate the cause, *and* keep the ability to see the residue.

  Gated by `two_rings_over_the_same_candidates_do_not_elect_the_same_node` — three live nodes, two
  rings chosen **at runtime** from what the rule says this port allocation splits, asserting each
  landed where the rule names. Reverting the blackboard to lowest-id makes it fail. Plus four pure
  detector tests (including the partition guard and evaporation) and four election tests (including
  that **one old candidate pins the whole ring**).

  **One internal fix found on the way:** `cap/` values must be decoded with the *fallback* that
  accepts the older bare-`Capability` encoding. The first draft of P10 used `CapEntry::decode`
  alone and would have **silently skipped** legacy entries — reading as *that role is not held*
  rather than *this reader is too new to parse it*. The fallback is now one named function
  (`decode_cap_entry`) instead of three inline copies.

- **The sealed identity record (`sys/identity-signed/{node}`, identity-auth Phase 3b)** — key
  history *and* its proof in **one** KV entry: `version(1) ‖ history ‖ proof(96)`.

  **What it removes.** A node's identity and its proof were two KV entries, hence two gossip
  messages with no ordering between them; a peer requiring proofs could learn the identity first,
  reject it, and hold **no key** for that node until the proof landed. The key recovers on its own —
  a late proof re-validates its entry — so the window is transient, but a one-shot decision taken
  inside it would not be.

  **Stated precisely: this is a hazard nobody has observed firing, not a diagnosed bug.** An earlier
  version of this entry said the window split a four-node cluster. It did not: that failure was an
  intermittent leader election in a test fleet whose nodes run **without TLS**, and every identity
  writer and reader lives inside the `config.tls` block — the flag is inert there. The correlation
  was the flip being the only change in its commit; the mechanism was never checked against the
  test. The window is real and worth removing on its own merits; the cluster split was not its
  doing, and that failure remains unexplained.

  One entry cannot arrive in two parts, so the window is closed **by construction rather than by
  timing**, which is the only kind of fix worth having for a race.

  **Compatibility — nothing to do.** Every TLS node writes the sealed record *and* the legacy pair,
  so a node predating this release still learns keys exactly as before. Readers prefer the sealed
  record; with the flag **off** (still the default) the legacy pair is honoured as it always was.
  With the flag **on**, only the sealed record is accepted — the pair is refused not because it is
  invalid but because it is two messages, and a peer publishing only the pair is one that predates
  the mechanism, which is what the flag has always refused.

  **The remaining precondition is a rollout, not a defect:** turn the flag on once every node runs a
  release that writes the sealed record (`cert-rotation.md` has the one-line check). The *default*
  should flip a release after that, deliberately — by editing a test that explains why it is there.

  Rotation writes the sealed record too. It has to: readers **prefer** it, so a rotation that
  updated only the pair would leave every proof-requiring peer reading the pre-rotation history and
  never learning the new key.

### Changed

- **Control groups resolve as connected components, not first match** (Boundary H H5).
  `ReaderPolicy::group_of` took the first configured group containing an issuer, so with overlapping
  groups independence depended on listing order. Overlapping groups now merge, which can only lower
  counted independence. **Upgrade note (v2.8.0's class):** `ReaderPolicy` gained `cohorts`, `undeclared`
  and `stale`, so an exhaustive struct literal needs `..Default::default()`.

- **`resolution::classify` honours retraction, same-issuer supersession and withdrawn bases**
  (Boundary H K1b). It never consulted them, so a withdrawn assessment kept supporting a release. It also
  now excludes records stored under an already-revoked key, and unchecked records under
  `UncheckedRule::Exclude`. A change that can only make acceptance **harder**. `classify` does not
  re-verify signatures; `classify_eligible` does.

  **Upgrade note (v2.8.0's class):** `ReaderPolicy` gained the field `unchecked`, so an exhaustive struct
  literal needs `..Default::default()`.

- **Knowledge resolution counts support per issuer, not per record** (Boundary H plan, milestone M1,
  item H2). `resolution::classify` counted every supporting *record* toward `min_supporting`, so one
  issuer filing five supporting assessments met a threshold of two by repeating itself. Independence
  was already counted by control group and was unaffected; support now matches it. A change that can
  only make acceptance **harder**, never easier.

  **Upgrade notes**, both in v2.8.0's class:
  - `Verdict` and `RejectionReason` are now `#[non_exhaustive]`, ahead of challenge admission (H1). A
    `match` outside the crate needs a `_` arm, and it **must fail safe**: an unrecognised verdict is not
    an acceptance, which is what `Verdict::is_accepted` already returns.
  - **Same type, new meaning:** `Verdict::Accepted.supporting`, `Conflicted.supporting` and
    `InsufficientEvidence.have`/`need` now count **distinct issuers**. A reader whose issuers each file
    one assessment sees no difference. `Conflicted.challenging` still counts records; H1 changes how
    challenges are counted.

- **`require_identity_proofs` stays `false` — the flip to `true` was made and reverted before any
  release carried it.** **No released version's behaviour changes**; nothing to do on upgrade. What
  ships here is the reason, pinned where the next person to consider the flip will hit it.

  **Correction to the first version of this entry:** the revert was triggered by an intermittent
  `S12 leader election … Nodes disagree on leader`, and that failure was attributed to the flip. It
  cannot have been caused by it — the flag is inert without TLS and the suite's nodes configure
  none. The revert stands on the plainer ground that the flip was never demonstrated safe. The
  leader-election intermittency is **unexplained and open**.

  **The argument for flipping was sound.** Every TLS node has written `sys/identity-proof/{self}`
  unconditionally since Phase 2 (**v2.3.0**, 2026-07-24), so within the one-release window a rolling
  upgrade is supported across, no honest node writes an unsigned identity. The rollout precondition
  the old guidance prescribed had been met ten releases earlier.

  **What it missed.** A node's identity and its proof are **two separate `kv_set` calls**, hence two
  gossip messages with no ordering between them. Requiring proofs means a peer can learn an identity
  *before* its proof, reject it, and hold no key for that node. The key recovers on its own — the
  identity watcher subscribes to the broader `sys/identity` prefix precisely so a late proof
  re-validates its entry — so the window is transient. **A decision taken inside it is not.** A
  leader election is one-shot: a node that could not verify a peer's signature during the window
  does not re-run the election when the key lands. The Docker suite failed `S12 leader election …
  Nodes disagree on leader` intermittently, with the federation two-mesh suite alongside it, after
  twelve consecutive greens.

  **Flipping it broke no unit test**, because every test that exercises the behaviour sets the flag
  explicitly and the in-process suites have no cross-process ordering window to lose a race in. The
  gate that could see it runs after the merge. That is the transferable part: *a config default
  whose only failure mode is a race between processes is not testable by the suite that gates the
  PR.*

  **A future flip's precondition is not "every node writes a proof anyway"** — that is true and it
  is not the failing condition. It is an **atomic** identity+proof record, or a bounded *pending its
  proof* state that defers rather than rejects. Until then the flag is an operator opt-in: fine
  where nodes do not elect during bring-up, not fine where they do. `cert-rotation.md` now says so
  in those terms instead of prescribing a rollout that is long finished.

  **What the episode leaves behind**, all of it worth keeping: the value is pinned *with its reason*
  (`config::tests::the_default_requires_identity_proofs` — flipping it back means editing a test
  that explains itself); the end-to-end join is pinned beside it, renamed
  `a_default_configuration_rejects_an_unsigned_identity_entry` →
  **`an_opted_in_configuration_rejects_an_unsigned_identity_entry`** (the one substantive test
  change here); and the boundary the flag never closed is still stated as a test rather than a
  caveat — *proofs required* is **not** *identity authenticated*, first sighting of an unseen node
  remains **trust on first use**, anchors are what close that, and
  `requiring_proofs_does_not_close_trust_on_first_use` is written to **fail if the window is ever
  closed** so the claim cannot rot. Full account:
  [`docs/wiki/dev/security.md`](docs/wiki/dev/security.md).

### Fixed

- **Nodes sharing a certificate directory could each generate their own cluster CA** (`mycelium-core`
  TLS). `load_or_generate` checked whether a CA existed and generated one if not. So nodes starting
  together on one `auto_cert_dir` (one volume, several containers, as the federation compose does)
  could each generate a CA, trust different roots, and **never peer**. That is the root cause of the
  federation suite's intermittent "b1 sees its one peer" failures. A node could also read a half-written
  CA file and fail to start. Now an exclusive-create lock (`ca.lock`) gives exactly one process the right
  to generate. It re-checks under the lock and publishes each file by rename; every other process waits
  (bounded, 30 s) and loads that CA. A lock left by a crash is an **error naming the lock**, never a
  second CA. The regression test starts 8 nodes on one empty directory at the same instant, 20 times; it
  fails on the old logic and passes on the new.

- **`auto_election_is_deterministic` was asserting the rule we replaced** — *"lowest candidate id
  wins"*, four days after the election became rendezvous (`mycelium::election`). It did not start
  failing; it started being a **coin flip**, because the test's ports are kernel-assigned and
  `hash(ring, node)` has no reason to favour the lower one. It passed twice on `main` — which is how
  the election change and its follow-up both merged green — and failed on the next branch that ran
  it, one with nothing to do with elections.

  Fixed by asserting the property that is actually deterministic rather than pinning the other
  answer: exactly one primary (which never depended on the rule), and the winner is the node
  `mycelium::election::winner` names, computed in the test from the same ring name and candidate ids
  the node uses. A future rule change now fails it honestly instead of re-rolling.

  Two stale comments went with it (`mycelium-wiki`'s curator sentinel said *"lowest id wins"*
  fifteen lines above its `elect` call). Both were only comments, because the wiki's assertions are
  **rule-agnostic** — which is precisely why they survived the rule change, and is the shape worth
  copying.

- **A supported way to join a group over HTTP** — `GET`/`POST`/`DELETE /gateway/mesh/group`
  (`mesh:read` / `mesh:write`).

  Its absence was a finding in its own right. The gateway offered `POST /gateway/overlay/elect`
  over a group while providing no route that could *populate* one: `grp_prefix` was read in
  `http.rs` and written nowhere in the surface, and `/gateway/govern/membership` sets a
  `MembershipIntent { min, max }` — it governs a roster's permitted population, it does not add a
  member. **The only roster an HTTP caller could reach was the empty one**, which was exactly the
  state that used to confer solo authority. An election surface without a membership surface is a
  surface that can only be used wrongly.

  **A node joins itself.** The route takes no node id and there is no verb for enrolling another
  node — the same shape the substrate commits to everywhere else: an agent promises only its own
  behaviour. `GET` returns the roster this node can see *and* the group's `declared_min`, which is
  the read an operator wants **before** an election rather than after a refusal.

  **What it does not settle**, and is tracked rather than implied: *who* may join, *who* may change
  an electorate, and *which* membership version an election is decided against. This is the
  operation; governing it belongs with the agreement repair
  (`.log/2026-09-24-consensus-vote-binding.md`).

### Fixed

- **`POST /gateway/kv` no longer succeeds at writing nothing.** It reads `value_b64`; when that
  field was **absent** it wrote an empty value and answered `{"ok": true}` — so a misspelled or
  assumed field name (`value`) **erased a key and reported success**.

  This was not hypothetical. The overlay test helper had been posting `{"key": …, "value": …}`
  since it was written, so every readiness sentinel it ever wrote was empty — and nothing noticed,
  because the check consuming them only counted keys under a prefix. Two failure-looks-like-success
  mechanisms stacked, each hiding the other.

  **The contract now**, one route test per row (`gateway_kv_write_requires_an_explicit_value`,
  verified to fail on the old code):

  | Request | Result |
  |---|---|
  | missing `value_b64` | **400**, no mutation |
  | `value_b64` not a string | **400**, no mutation |
  | invalid base64 | **400**, no mutation |
  | explicit `"value_b64": ""` | a valid **zero-length** value |
  | valid encoded value | exactly those bytes |
  | `DELETE /gateway/kv?key=` | tombstone, unchanged |

  An explicitly empty value is legitimate; an **omitted** one must not silently become one, and
  `DELETE` already owns tombstoning — so silent-empty had no legitimate caller. The "no mutation"
  half is the load-bearing part: a 400 that had already clobbered the key would be the same data
  loss with a better status code.

  **Upgrade note.** A client that omitted `value_b64` now gets a 400 where it used to get `{"ok":
  true}`. Every first-party SDK (`mycelium-py`, `mycelium-ts`, `langgraph-checkpoint-mycelium`),
  the AFN scenario and the benches already send `value_b64` and are unaffected; the one caller that
  did not was the test helper fixed alongside this.

### Fixed

- **An election needs an electorate. Absence is not authority.** A group proposal whose roster this
  node cannot see — an unknown or unjoined group — is now **refused**
  (`ConsensusResult::ElectorateUnavailable`, `409 electorate_unavailable` at the gateway) instead of
  being decided alone.

  **What it was.** `members.len().max(1)` — an empty roster counted as **one** member, giving a
  quorum of **one**, which the proposer's **own self-vote** satisfied before it began listening. So
  every node committed its own candidate unopposed: *N* singleton elections wearing the shape of
  one, reconciled afterwards by LWW if at all. The arithmetic was not the defect. The defect was
  that *"I cannot see members"* silently meant *"I have authority to decide alone."*

  **Partial views too.** A node that sees fewer members than the group declares through a fresh
  `MembershipIntent { min }` is also refused. A partial view yields a *smaller* quorum — the same
  defect wearing a plausible number.

  **An explicit singleton still elects.** What is refused is inferring authority from absence, never
  solo authority somebody actually established. A one-member roster decides normally, and a
  declaration that has evaporated stops binding, so a stale intent cannot wedge a group shut.

  Both surfaces enforce it — `ConsensusHandle::group_propose` and `POST /gateway/overlay/elect` —
  because a caller must not get a different answer for reaching the same election through a socket.

  **Upgrade notes.** `ConsensusResult` is now **`#[non_exhaustive]`** and gains
  `ElectorateUnavailable`; `CommitError` gains `ElectorateUnavailable` (it is already
  non-exhaustive); `ConsistencyError` gains `ElectorateUnavailable`. A `_` arm is required and
  **must fail closed** — a refusal read as success is the exact class of bug this variant ends.
  Same discipline as `CallRefusal`/`CommitmentRefusal` in v2.13.0. A deployment that relied on an
  unjoined group electing a leader was relying on every node electing itself.

  **This is one of three findings** in `.log/2026-09-24-consensus-vote-binding.md`. The other two —
  votes not bound to the proposal they voted for, and no supported HTTP route by which a node joins
  a group — are **not** fixed here and are tracked there.

### Fixed

- **A vote now says what it voted for.** `ConsensusMsg::VoteForValue` carries a SHA-256
  `value_digest`, and a proposer counts **only** votes bound to the value it proposed.

  **The defect, stated exactly.** `Vote` and `VoteWithLocality` name a `(slot, ballot)` and a voter
  and **nothing else**. The collector matched on `(slot, ballot)` alone and then committed **its
  own** value. Three facts turn that into a safety violation rather than a curiosity:

  1. ballots come from a **shared KV key** (`read_ballot + 1`), so concurrent proposers choose the
     same ballot *by construction*;
  2. votes are emitted to the **group scope** — a broadcast, not a unicast to the proposer — so
     every proposer receives every vote;
  3. the quorum test is a headcount over voters at that `(slot, ballot)`.

  So: A proposes `v_A` at ballot 1, B proposes `v_B` at ballot 1, C votes once for whichever it saw
  first, **both A and B count C's vote**, both reach quorum, and **two different values commit at
  one ballot**.

  **Why the existing defences did not cover it.** `may_cast_vote` (audit 2026-07-15) stops a *voter*
  accepting two values at one ballot — and that is not the same thing: C's single vote never said
  *which* value it accepted, so it could not stop the **counting**. The NACK C sends the second
  proposer does not help either: a proposer acts only on `seen_ballot > ballot`, and that NACK
  carries an **equal** ballot, so it is ignored while C's broadcast vote is still counted.

  **Compatibility, and the trade made deliberately.** `VoteForValue` is appended *after*
  `VoteWithLocality`, so a proposer predating it decodes an unknown variant as `None` and drops it —
  the same rolling-upgrade shape `VoteWithLocality` itself used. **Wire v12 is unchanged.** Voters
  emit the bound vote **and** the legacy form, so an old proposer keeps working exactly as before.
  An upgraded proposer counts **only** bound votes.

  **Rolling-upgrade note:** while any voter is un-upgraded, group proposals from an upgraded
  proposer **time out** instead of committing — fail-closed and visible, rather than committing two
  values quietly. Upgrade voters before relying on elections.

  Pinned by `a_vote_for_one_value_cannot_authorise_another` (a vote for A must not contribute to B's
  quorum at the same slot and ballot), `the_value_digest_separates_distinct_proposals`, and
  `a_bound_vote_still_requires_its_signer` — binding and signer authorisation compose; neither
  replaces the other.

- **A node casts one vote per ballot, whichever role it is playing.** Without this the binding above
  is decorative. A node kept **two separate memories**: as an acceptor, `run_consensus_listener`'s
  `seen_ballot`/`voted_value` were task-local; as a proposer, the ballot loop **self-voted
  unconditionally** without consulting them. So a node could accept `v_X` at ballot 1 as a voter and
  propose-and-self-vote `v_Y` at ballot 1 as a proposer — equivocating with itself through the one
  voter that never has to send a message to vote. Both roles now pass through `claim_vote` against
  shared state on `TaskCtx` (`papaya`, so no lock-order row; the `compute` closure is a
  compare-and-set and therefore retry-safe). **Proposing a value is accepting it.**

- **A proposer at a higher ballot adopts what was already accepted.** `ConsensusMsg::Promise`
  answers a refusal with the acceptor's highest `(accepted_ballot, accepted_value)`, and the retry
  carries **that** value rather than the proposer's own.

  A bare `Nack` carried a ballot number and nothing else, while the retry was
  `ballot = …max(ballot) + 1` with the proposer's original value — which never changed across
  attempts. Acceptors permit that, because `may_cast_vote` returns `true` for any strictly greater
  ballot, so a value a quorum had already accepted at ballot *N* could be replaced at *N+1*. The
  commit record guards the *committed* case, but only once it has propagated; inside that window the
  overwrite stands. Preservation is what makes that guard unnecessary rather than
  usually-sufficient. Applied in both proposer paths (`propose` and `cross_propose`).

  `Promise` is appended last, so older proposers drop it and fall back to the `Nack` still sent
  alongside. Pinned by `a_higher_ballot_adopts_the_accepted_value` — including that adoption follows
  the **highest accepted ballot**, not the latest message.

  **Still not the whole repair**, and tracked in `.log/2026-09-24-consensus-vote-binding.md`:
  **restart recovery** (acceptor memory is in-process and does not survive a restart) and replacing
  the **"wait one second, then read the local slot"** success semantics.

### Fixed

- **The acceptor's memory now survives a restart.** `sys/consensus-accepted/{node}/{slot}` holds
  `ballot ‖ value_digest`, written **before** the vote leaves at both roles, recovered at startup
  before any listener can vote, and deleted when the slot commits.

  Until now *at most one value per ballot* held only for a **process lifetime**: a node that
  restarted mid-ballot forgot what it accepted and could vote again, for a different value, at the
  same ballot. Every acceptor-side guarantee in classical consensus depends on that memory being
  durable, and ours was not.

  **The cost is one extra small write beside one the acceptor already made** (`consensus/ballot/`),
  not a new write on a previously write-free path — which is the answer to the obvious objection,
  arrived at by reading the path rather than arguing about it.

  **A recovered acceptance refuses but cannot report.** The record holds a digest, so a restarted
  node can still refuse a conflicting vote — the safety property — but cannot supply a value in a
  `Promise`, which is only a liveness aid to a proposer. Safety survives the restart; that
  assistance does not.

  Pinned by `acceptor_memory_survives_a_restart`, `a_recovered_acceptance_reports_no_value` and
  `a_malformed_acceptor_record_is_no_record`.

- **`elect_leader_receipt` — a leadership answer that names the rung it reached.** Returns
  `Leadership { leader, epoch, basis }` where `basis` is `Decided` or `Observed`.

  `elect_leader` returns a bare `NodeId`, which cannot distinguish *"a quorum chose me"* from
  *"this is what my replica currently says"* — and callers read the bare id as an **exclusive
  grant**, because nothing in the type said otherwise. Those are different answers, and the
  difference decides whether two callers can act as leader at once. This substrate's own rule is
  that *a receipt names its rung and nothing above it*; leadership was the surface still naming a
  rung it had not reached.

  - **`Decided`** — this node's proposal committed at a quorum: a quorum voted for this value at
    this ballot, **bound to it by digest**, and no other value can have been committed at that
    ballot.
  - **`Observed`** — read from the converged slot. Sound for *following* a leader; **not** evidence
    the cluster agrees right now.

  **Neither is an exclusive grant that stays true**, and no coordinator-free protocol can offer one.
  The honest instrument is to **fence on `Leadership::epoch`** — the commit's HLC, monotonic across
  successive holders, the same token `LockGuard` already uses. The ballot is unusable for this: it
  regresses under gossip lag (#164).

  `elect_leader` is unchanged and still returns the `NodeId`; it now delegates, so both agree about
  *who*. What it cannot tell you is *how*.

### Changed

- **Election convergence is now observed, not assumed.** The fixed `sleep(1s)` before reading the
  committed slot — which the replay inventory already called *"the one whose duration is a
  correctness assumption"* — is replaced by a **bounded poll** on the same budget. A fast cluster
  answers in milliseconds; a slow one still gets its full second; and the answer is a value that was
  observed to be there rather than whatever happened to be present when a timer fired. Still routed
  through the timer seam (`elect/converge`), so replay can explore it.

  Lengthening a sleep only ever changed how often the difference was visible.

## [2.13.0] — 2026-09-23

**The axis' last open questions, and a gateway that was not closed.** Wire **v12** unchanged
(`PREV = 11`); every change additive on the 2.x line.

Three things run through this release. Item 2's **row 11 is complete** — federation reaches Python
and TypeScript, survives a hostile network, and has been shown to work with more than two domains,
which is the first time several of its claims could even be *stated*. The contracts axis' **two open
design decisions are decided and built**, each with the limit it does not close written down beside
it. And a **security fix**: a deployment whose only credential model was `gateway_named_tokens` — the
configuration `GossipConfig`'s own documentation tells operators to prefer — has been running an
**open gateway** since 2.10.0.

**Check first, upgrade second.** If you run 2.10.0–2.12.0 with named tokens and no
`gateway_auth_token` or `gateway_scoped_tokens`, an unauthenticated `GET /gateway/kv/keys` answers
**200 today and 401 after this release**. That is the whole test.

### Security

- **A deployment whose only credential model was `gateway_named_tokens` had an open gateway.**
  `gateway_auth`'s "is a token model configured?" test counted `gateway_auth_token` and
  `gateway_scoped_tokens` — and not the named table, which 2.10.0 added and `resolve_token` has
  honoured ever since. Such a node therefore took the *open gateway* branch: **no bearer
  required**, with `open_gateway_scopes` handing each request exactly the scope its route asks
  for, which turns deny-by-default into admit-by-default for every gateway route on the port.

  It hid because the tokens kept working. Presenting one was admitted (`resolve_token` knows
  them), so nothing looked wrong; the only way to see the hole was to present **nothing**. The
  existing identity test configures both tables at once and so never could — the same shape as the
  fuzz seeds in 2.11.1, where a gate that appeared covered was never reaching its invariant.
  `GossipConfig`'s own documentation says to *prefer* named tokens, so the recommended
  configuration was the affected one.

  **Affected: 2.10.0 – 2.12.0**, and only deployments configuring `gateway_named_tokens` with no
  `gateway_auth_token` and no `gateway_scoped_tokens`. Check yours: an unauthenticated
  `GET /gateway/kv/keys` must answer 401. Found while writing the federation verbs' scope test,
  which returned 504 where 403 was expected — the scope layer had not run at all. Pinned by
  `named_tokens_alone_still_close_the_gateway`, which asserts both halves (no bearer is 401, a
  named token still resolves to its own principal and is still bounded by its scopes) and fails
  when the fix is reverted.

- **The federation edge meters calls per partner** — the Phase-C audit's last open finding, closed
  as a decision with a mechanism. `GatewayPool` has metered slots per partner since PR 5 on the
  *consumer* side: it bounds what we send a partner. Nothing bounded what a partner sends us, and
  the realistic threat there is not an anonymous flood (the credential gate stops that) but a
  **compromised or buggy partner**, whose credentials are by construction minted by them.

  `CallPolicy::max_in_flight_per_partner` (**0 = unlimited**, so no deployment acquires a cap by
  upgrading) and `FederationEdge::admit`, which returns a `PartnerSlot` **RAII guard** — the release
  is a `Drop` because a release a handler has to remember is one it misses on an early return or an
  unwind, and a leaked slot is a partner permanently short of capacity with nothing saying so. Over
  the cap is `CallRefusal::AtCapacity { partner, limit }` and JSON-RPC **-32004**, deliberately not
  -32003: *not permitted* is a standing answer about authority that a retry will not change, this is
  a transient one about load that a retry probably will, and merging them tells a partner to go and
  ask their operator about a grant they already have. **Capacity is asked after authority**, so an
  unauthorised partner cannot occupy a slot belonging to an authorised one.

  **The alternative, and why not.** Applying `mycelium-core`'s M7 pattern — shared observation,
  local decision — to partner domains would catch a partner fanning out across several gateways, and
  would put **foreign domain names into `sys/`**, which is what D7 exists to prevent, and tell every
  node in the mesh which partners exist and how hard each calls. It is also unbuildable without a
  local counter to clamp: M7's own design is a per-peer limit first and aggregate observation
  second. So this is the prerequisite for that, not a competitor to it.

  **What it does not do**, stated because the asymmetry is now smaller rather than gone: the count is
  **one gateway's** (N gateways ⇒ N × cap in aggregate), and it bounds **concurrency, not rate**.

  Gated by `the_edge_meters_calls_per_partner_and_refuses_rather_than_queues` and
  `the_per_partner_cap_refuses_a_concurrent_call_at_the_live_gateway`, which drives two *concurrent*
  calls through a real gateway against a provider that blocks until released — with an instant
  provider the first call finishes before the second arrives and the cap is never consulted. Removing
  the `admit` call from the `/a2a` path makes it fail.

  **Upgrade note:** `CallPolicy` gained a field (an exhaustive struct literal needs
  `max_in_flight_per_partner: 0`; the `..Default::default()` pattern is unaffected), and
  `CallRefusal` gained a variant, so an exhaustive `match` on it needs another arm.

- **`mycelium-commitment`: the crate's first rule acquires a mechanism** — the open provenance
  decision, resolved. *"No component assigns another participant's obligation"* has been the
  companion's headline rule since CN1 and had **nothing enforcing it**: an offer names its
  participant in a *field*, so any member able to append to the stream could post an offer naming
  somebody else, `AwardRule::LowestParticipant` would award that participant work they never offered,
  and — this is the part that makes it more than an annoyance — **every reader checking the award
  against the offers would agree the award was correct.** A pure rule over forgeable inputs checks
  that the rule was applied, not that the inputs were real.

  `Offer` and `Award` gain a signature, exactly as `Assessment` already had one:
  `ContractNet::offer_signed`, `Award::signed`, `verify_offer`, `verify_award`. The teeth are
  `offers_verified(requirement, resolve)` — the candidate set whose signature verifies under the key
  the caller's directory gives for the participant **the offer names**, so an offer signed by its
  forger fails, which is the realistic attack rather than the lazy one — and `plan_award_from`,
  which takes a candidate set the caller chose. That seam is deliberate: **provenance is the
  declarer's decision, not this crate's policy.** `offers()` + `plan_award` behave exactly as before.

  **Unsigned stays legal and keeps meaning *unproven*.** A single-tenant mesh whose members are
  trusted equally has nothing to prove to itself. What is not legal is reading an unsigned record as
  proof.

  **What a verifying signature does not establish**, stated because the axis is about not
  overclaiming: it proves the holder of that key made the record. Whether the key belongs to the
  participant named is `sys/identity/{node}`'s question, only as strong as `require_identity_proofs`
  — **default-off**, without which an admitted node can append its own key to another's identity
  entry. `SelfImposedPrevention` in the guardrails tier vocabulary, no higher, and nothing here stops
  a forged offer being *written* — it stops it being *awarded*.

  Gated by `a_forged_offer_is_not_a_candidate_when_the_declarer_checks_provenance`, which asserts
  **both halves** (unchecked, the forgery wins; checked, it is not a candidate) because a test that
  showed only the fix would leave a reader unable to tell what the fix is for. The gallery example
  `redistribution_cn` carries the same scene end to end.

  **Upgrade notes**, both small and both in the same class as v2.8.0's: `Offer` and `Award` gained a
  field, so an exhaustive struct literal needs `signature: Vec::new()` (`..Default::default()` is
  unaffected); and `CommitmentRefusal::AlreadyAwarded` now carries a `Box<Award>`, because an
  unboxed one made every `Result` in the crate as large as its rarest outcome — `existing.participant`
  is unchanged, but moving the award out of the variant needs a `*`.

- **Federation's consumer side reaches local clients — the SDK verbs** (item 2 row 11). Until now
  a federated call could only be made from Rust: `FederationClient` was the sole consumer, and
  `examples/federation_node.rs` had to hand-roll its own control API to drive one. The Python and
  TypeScript SDKs had no way to reach a partner at all.

  They do not get one by re-implementing the protocol. The credential is signed with the domain's
  key and the catalogue is checked against the partner's; neither the key nor the trust bundle
  belongs in an SDK process, and putting credential minting into two more languages would put the
  trust story in three places. Instead `GossipAgent::with_federation_clients` attaches one client
  per partner to the node, and the gateway grows five routes behind two new scopes —
  `GET /gateway/federation/{domain,partners,catalog/{domain}}` under `federation:read`,
  `POST /gateway/federation/{connect,call}` under `federation:invoke`. `agent.federation()` in
  both SDKs drives them.

  **The principal on the wire is the local caller's, and the request body cannot say otherwise.**
  That is item 7's rule at one more boundary: a gateway holds the domain's key and the partner's
  trust while the client calling it has neither, so a gateway minting every credential under its
  own configured principal would record a service account in the partner's evidence for work it
  never asked for — the confused deputy, one domain wider. `FederationClient::call_as` takes the
  principal from the authenticated caller; `call` is now that, with this client's own principal.
  On a gateway with no token model it is `anonymous`, which is an honest statement and the reason
  the runbook says to configure tokens before connecting two domains.

  **A refusal answers two questions.** `sent` — did any byte reach the partner — and `delivery`:
  `none` · `refused` · `completed` · `unknown`. Both SDKs raise a **distinct type** for `unknown`
  (`DeliveryUnknown`, `DeliveryUnknownError`), so the one outcome that must not be retried blindly
  cannot be caught by accident alongside ordinary failures. An unreadable refusal body (an HTML
  page from a proxy) fails closed to `unknown` rather than to success.

  The outbound call is also an **AE enforcement point** (`gateway:federation/call`), named
  separately from `gateway:a2a` because the two directions are different: an evaluator that
  guards `/mcp` and `/a2a` and not this one is a remit with a third door left open.

  Gated by `the_gateway_verbs_carry_the_local_caller_across_the_boundary` (a real two-domain
  run: the partner's provider reports the *caller's* principal, and a body that names another one
  changes nothing) and `a_read_scoped_token_cannot_invoke_a_partner`.

- **§12.2's last two lines — the SDK receipt narrative, and a wiki page per mechanism.** The SDKs
  had *carried* the receipt vocabulary since 2.8.0 (`CommitResult.local_durability` /
  `localDurability`) and never explained it: a field an SDK user has to read the Rust source to
  interpret is carried, not delivered. Both READMEs now state the four rungs, what each
  `LocalDurability` state actually means operationally, and the rule that **a timeout is not a
  negative** — with the federation verbs' `DeliveryUnknown` named as the same rule at a domain
  boundary.

  The LangGraph chapter and the checkpointer's README gained the part that was missing and is not
  flattering: `MyceliumCheckpointSaver.put()` writes with a plain `POST /gateway/kv`, so it returns
  a **rung-1 receipt** — applied to the store of the node you are talking to, saying nothing about
  that node's disk and nothing about any peer. A checkpoint `put()` acknowledged is therefore not
  yet one that survives losing that node, and the flagship demo already knew it: it waits for
  replication by *reading from node B in a bounded poll* before killing A, rather than trusting the
  write. That is a client-side observation of rung 3, and now it is documented as one.

  Two wiki pages: `dev/architecture/contracts.md` (the rungs, `LocalDurability`'s four states and
  the lesson in `Buffered`, and the regression floor — *a PR changes an ack's meaning by changing a
  pin, in the open*) and `dev/testing/replay.md` (the nondeterminism inventory, the seams, scenarios
  A/B/C, the corpus — moved out of `testing.md`, which had grown five replay sections). `AGENTS.md`
  gains a routing table so a fact about a v3 mechanism has a home rather than a choice, and
  `dev/dev.md`'s AE heading stopped saying *"not implemented"*, which it had said for nine releases
  while the paragraphs under it were kept current.

- **More than two domains — item 2's row 11 is closed.** Every federation test until now ran with
  exactly two domains, where three separate properties are indistinguishable: a catalogue *filtered
  for the asker* looks like the export list, a grant to one partner looks like a grant, and *trust
  is not transitive* cannot be stated at all for want of a third party to fail to reach.

  The new gate runs a chain — alpha grants `demo/whoami` to beta, beta grants `demo/relay` to
  gamma, alpha and gamma strangers — so **beta is a provider and a consumer at once**, which is
  what a hub deployment is made of and what two domains cannot produce. It pins: trust does not
  compose (gamma's link to alpha is refused `UnknownDomain` at the auth layer, before any catalogue
  is computed, so it cannot even learn what alpha exports); a grant you *hold* is not re-exported by
  holding it (beta's catalogue to gamma names beta's own export only); a catalogue is filtered per
  asker with three askers to tell apart, and a planted credential for an exported-but-ungranted
  skill is refused at the edge without reaching a provider; **non-merger holds pairwise over all
  three meshes**, since checking only the pair that exchanged bytes would miss a domain joined
  through a third; and a hub's gateway slots are **per partner**, so one busy neighbour is not a
  denial of service on every other partner — a distinction that does not exist with two domains.

- **TLS on the federation edge, anchored on a pin rather than a CA** (item 2 row 11). v2.12.0 stopped
  an on-path attacker *altering* a federated call; it left them able to *read* one, and said so:
  "TLS on the edge needs a trust anchor that does not exist". This is that anchor.

  Partner trust here is one Ed25519 key per partner, chosen bilaterally — there is no X.509 material
  in the design. Rather than install the public Web PKI underneath a bilateral relationship (a root
  far larger than the trust it would carry) or exchange private CAs (more machinery, and a CA signs
  *any* name), the anchor is the decision the operator already made: `PartnerTrust::tls_spki_sha256`
  pins the sha256 of the endpoint's `SubjectPublicKeyInfo`, and `FederationClient::with_tls_pins`
  dials only an endpoint holding one of those keys. A **list**, because one pin makes a TLS key
  change a flag day — `TrustBundle::pin_tls`/`unpin_tls` open and close the window.

  Two refusals come with it, and both mean **nothing was sent**: `Tls(PinMismatch)` for an endpoint
  presenting an unpinned key, and `Tls(PlaintextEndpoint)` for pins configured against an `http://`
  URL, which is refused rather than quietly downgraded. Neither is a `DeliveryUnknown` — that says
  *it may have run*, and bars an at-most-once caller from retrying, whereas a failed handshake
  delivered nothing.

  The SPKI is read from its **structural position** by the X.509 parser rustls already uses, never by
  scanning the DER for a key-shaped byte pattern. The existing `ed25519_key_from_cert_der` does scan,
  and is sound where it is used because its input is already CA-validated — a premise absent here. A
  test plants the victim's key bytes in an attacker-signed certificate's serial number and asserts
  the scan is fooled while the structural read is not.

  **Not claimed:** this is confidentiality and endpoint authentication, not caller authentication —
  that stays the credential, deliberately, rather than adding a second auth model at one edge. The
  verifier checks no name, chain or expiry, because there is no authority here to check them against.

  **Upgrade notes.** `PartnerTrust` gained a field, so an exhaustive struct literal needs it
  (`TrustBundle::trusting` is unaffected). `ClientError` gained a `Tls` variant **and is now
  `#[non_exhaustive]`**, so an exhaustive `match` needs a `_` arm — and that arm must fail closed: an
  unrecognised refusal is *the call did not happen for a reason this code does not know*, never *the
  call is fine*. Pinning is off unless configured, and off means no confidentiality guarantee.

### Changed

- **`CallRefusal` and `CommitmentRefusal` are `#[non_exhaustive]`.** Both gained a variant this
  cycle (`AtCapacity`; and `AlreadyAwarded` changed shape), and both are vocabularies of *why
  something was refused* that have now grown twice — so this is the §6.6 ledger's own reasoning
  applied before the next addition rather than after it: **one announced break instead of a series
  of unannounced ones**, the same move 2.10.0 made for `CatalogRefusal`.

  An exhaustive `match` downstream needs a `_` arm, and **that arm must fail closed**: an
  unrecognised `CallRefusal` is *this call was not authorised for a reason this code does not know*,
  never *it is fine*; an unrecognised `CommitmentRefusal` is *no award was made*, never *one was*.

## [2.12.0] — 2026-09-23

**Authenticating the caller is not authenticating the call.** Wire **v12** unchanged.

A federated credential's signature covered the origin domain, the principal, the export and the
validity window — and **nothing about the payload**. An attacker on the path between two domains
could rewrite a call's body, leave the header untouched, and the receiving gateway would accept the
altered call as authentic, then run the authorisation preflight and record an evidence decision
about *the attacker's* text. The credential said *this principal may call this export* and stayed
true while the call became a different call.

That is the release. The second change is the same shape one layer out: `preflight` was reachable
only inside this crate, so an enforcement point that is **not** this gateway had no public path but
`ActionEvaluator::evaluate` — which performs none of the seam's five checks. In every one of those
cases a *correct* evaluator returns a permit and the seam refuses; it was never asked that
question. Both defects are compositions where each part behaved exactly as documented.

**Upgrade notes.** `FederationEdge::authorize` takes the request body — §6.6 removal-ledger entry
10, and it will not compile until you pass it. `FederatedCaller` gained `body_sha256`, so an
exhaustive struct literal needs it. `POST /a2a` answers a malformed request as JSON-RPC `-32700`
rather than a bare 400. `CallPolicy::require_body_binding` defaults to `false` — a rolling-upgrade
window that gives **no integrity guarantee against an active attacker**; turn it on once every
partner has upgraded.

- **`mycelium::preflight` is public**, so an enforcement point that is not this gateway can run the
  same checks this gateway runs. It was reachable only inside the crate, which meant a resource
  enforcing at its own boundary — AE2's shape — had no way to call anything but
  `ActionEvaluator::evaluate`, and **an evaluator called directly performs none of the seam's five
  checks**: expiry, stale policy revision, a `Permit` carrying evaluation errors, an unmapped or
  ambiguous operation, and a panicking adapter.

  In every one of those cases a *correct* evaluator returns a permit and the seam refuses — it is
  not being overridden, it was never asked that question. So the failure mode was silent and looked
  like success, which is the *every leg correct, the composition wrong* shape one layer out from
  where this axis first found it.

  Gated as a **difference**: the test asserts that `evaluate` permits *and* `preflight` refuses, for
  four of the five. Asserting only the refusal would pass even if the evaluator refused too, and
  would then prove nothing about which layer carries the guarantee. Guide chapter 20 gains the
  worked example.

### Security

- **A federated credential now binds the request body** (item 2 row 11). Its signature covered the
  origin domain, the principal, the export and the validity window — and **nothing about the
  payload**. An attacker on the path between two domains could rewrite a call's body, leave the
  credential header untouched, and the receiving gateway would accept the altered call as
  authentic; the AE preflight would then decide and record against the attacker's text. The
  credential said *this principal may call this export* and stayed true while the call became a
  different call.

  `FederatedCaller` gains `body_sha256`, covered by the signature. The same reasoning as
  `ActionEnvelope::arguments_digest` one layer up: that binds arguments to a **decision**, this
  binds the body to the **caller**.

  Three details that make it hold rather than merely exist:

  - **Over the bytes as received.** The client serialises once and signs those bytes; the gateway
    takes the body as `Bytes` and parses afterwards. Digesting a re-serialisation would compare our
    encoder against theirs — false refusals for honest partners, and a pass for an attacker who
    matched our encoder. Same rule as the exporter's *verify the received bytes, then parse*.
  - **Stripping the binding is a forgery, not a downgrade.** Whether a credential binds is inside
    the signed bytes, so removing it fails as `BadSignature`.
  - **`CallPolicy::require_body_binding`**, default `false`. That default is a rolling-upgrade
    window, not a recommendation: a partner predating the binding sends no digest, and absence is
    indistinguishable from tampering, so `false` gives **no integrity guarantee against an active
    attacker**. Turn it on once every partner has upgraded. Two refusals, deliberately distinct —
    `BodyNotBound` (upgrade the partner) and `BodyMismatch` (someone is on the path).

  **Upgrade notes.** `FederationEdge::authorize` takes the request body; `FederatedCaller` gained a
  field, so an exhaustive struct literal needs it. `POST /a2a` answers a malformed request as
  JSON-RPC `-32700` rather than a bare 400 — a JSON-RPC endpoint replying in a different protocol
  when it dislikes the input was its own small dishonesty.

  What this does **not** do: it is integrity, not confidentiality. An on-path observer still reads
  every federated call. TLS on the edge is the other half of row 11 and needs a trust-anchor
  decision (public PKI, or pinning the partner's certificate in the `TrustBundle` beside its
  Ed25519 key) that has not been made.

## [2.11.1] — 2026-09-22

**Four defects, and the gate that had never run.** Wire **v12** unchanged; no API change.

Every defect here was found by §12.6's own fuzz targets — and the release is really about *why
they were still there to find*. The fuzz job runs its twelve targets **sequentially and stops at
the first crash**. `presented_call` is fifth. It began failing in the very commit that **added**
the trust-edge targets on 2026-09-20, so targets six through twelve never executed at all. `main`
was red for **22 consecutive runs**, through every AE commit and through v2.11.0's tag, and each
fix simply let the queue advance to the next defect waiting behind it.

So §12.6's "nine trust-edge fuzz targets" was a weaker claim than it read as, in two independent
ways. Three of them never reached their own invariants, because random bytes essentially never
form a parseable credential — measured: **0 of 7** assertion-bearing targets were reached by a
20,000-input noise pass. And most of them were never *run*. Both are fixed: every target now has a
valid seed asserted to parse, and a **reachability registry** claims completeness the way the
lock-order table does.

`RELEASING.md` gains **step 2b — check CI on the branch you are releasing *from***. A green
`make check-full` and a green PR do not imply a green `main`; v2.11.0 was tagged on exactly that
gap. This release was cut from a commit whose CI run was checked by id, fuzz job included.

### Fixed

- **A replay bundle's object codec loses nothing: two defects, found by searching instead of
  sampling.** An exhaustive fidelity sweep over the characters that break flat text formats — every
  key/value pair up to length two — found **7,092 of 17,556 pairs did not survive a write and a
  read**. Then, with the first defect fixed, **1,729 still did not**.

  The first: `parse_object` trimmed the value *inside* its quotes. The separator it split on had
  already consumed the value's opening quote, so `.trim()` was stripping characters that belonged
  to the value — `" v"` read back as `v`, and `"\t"` as empty. The line's own indentation was
  already gone, which is the only thing the trim was ever for.

  The second could not be patched. `split_once("\": \"")` searched for four characters that occur
  *inside* an escaped key: a key of `"` is written `"\""`, whose tail is `"` `"`, so the search
  found its separator one character early and the key came back as `\`. **A literal-substring
  split cannot tell a real delimiter from one appearing inside a quoted string**; it is replaced by
  a scanner that knows where a string ends.

  The stake is what the bundle carries: `Build.commit`, the field a replay uses to decide it is
  replaying against the same binary, and `witness.assertion`, which is free text.

  **Why example-based tests missed it.** Two fidelity tests for this codec already existed and
  passed throughout — they used ordinary strings, and ordinary strings round-tripped fine. The
  sweep is now a permanent test, and both defects are planted against it (14,364 and 10,032 pairs
  fail respectively when reintroduced).

  Same family as the `\n` / `\r` escapes fixed in 2.10.0 — and the part of that fix which did not
  go far enough.

- **A replay trace does not silently lose a carriage return.** `str::lines()` strips a `\r` only
  when it precedes `\n`. A trace line ending in a *bare* carriage return therefore parsed with the
  `\r` kept in its last field; `to_text` appended `\n`, making it `\r\n`; and re-parsing stripped
  it. The trace that came back was not the trace that went in.

  That matters more than a stray byte sounds. A trace is what a whole-node recording replays from,
  so **what it decodes to *is* the run** — a silently shortened field replays a different run while
  looking like the same one, which is the exact failure `Trace::parse` refuses holes to prevent.
  `Trace::parse` now refuses a line containing `\r`, so what it accepts is what `to_text` can
  re-emit. Our recorder emits none, so nothing legitimate is refused.

  Found by §12.6's `replay_trace` fuzz target, by the round-trip assertion.

- **`main` had been red for 21 consecutive CI runs, and the release was cut on it.** The fuzz job
  runs its twelve targets **sequentially and stops at the first crash**. `presented_call` is fifth.
  It began failing in the very commit that added the trust-edge targets (2026-09-20), so targets
  six through twelve **never executed at all** — for two days, through every AE commit and through
  v2.11.0's tag.

  So §12.6's "nine trust-edge fuzz targets" was a weaker claim than it read as, in a second way:
  not only were three of them never reaching their own invariants (above), most of them were never
  being *run*. Both defects fixed in this release were found the moment the queue started moving.
  `RELEASING.md` gains step 2b — check CI on the branch you are releasing *from*; a green
  `check-full` and a green PR do not imply it.

  **Not yet known to be the end of the queue:** `caller_envelope`, `replay_bundle` and
  `fixint_decode` sit after `replay_trace` and have still never run in CI.

- **A federation credential is ASCII in its *content*, not merely its encoding.** The `/a2a`
  credential header gate checked `raw.is_ascii()` on the header text. JSON spells any character in
  pure ASCII, so `"\u0809"` is an ASCII header carrying a non-ASCII field — it passed, and
  `PresentedCall::to_header_value` then re-emitted that character **unescaped**, producing a header
  this same parser rejects.

  Two consequences, and the second is why it is a defect rather than a nicety. The `principal` is
  an identity string, and one that may hold arbitrary Unicode admits **confusables** — a principal
  that renders like another one — which the ASCII rule existed to prevent and did not. And the
  accept-set had come apart from the emit-set: a credential this node accepted, it could not itself
  re-parse, and a peer forwarding it would refuse what we had just allowed. The method's own doc
  said *"a principal that is not [ASCII] is refused at parse time"*; nothing delivered that.

  `principal`, `export` and `signature` are now checked after parsing. `origin` needed no change —
  `DomainId` is restricted to `[a-z0-9.-]` at construction and its `Deserialize` routes through it.

  **Found by §12.6's own `presented_call` fuzz target**, on `main`, by the round-trip assertion
  rather than the parse — and *after* v2.11.0 was tagged, which is the honest sequence.

- **Every trust-edge fuzz target now reaches the invariant it asserts.** Auditing the above found
  the general case, and it is worse than the specific one: a 20,000-input noise pass reached the
  invariant of **0 of the 7** assertion-bearing targets. Random bytes essentially never form a
  parseable credential, policy, bundle or reply — so those targets proved *"does not panic on
  garbage"*, which is worth having, and **nothing they claim**, while looking fully covered.

  `presented_call_parse` had asserted round-trip stability from the day it was written and had
  never once executed that assertion. `trust_bundle_parse` and `catalog_reply_parse` had no valid
  seed at all. The in-suite mini-fuzz now seeds all of them — each **asserted to parse**, so the
  invariant is provably reached — with mutations and truncations of each seed, plus the escaped
  credential as a literal regression.

  And a **reachability registry** closes the class: one block naming every target that asserts
  anything, with the seed that reaches it. It claims completeness in the same sense the lock-order
  table does — adding a trust-edge target means adding a row — and it fails loudly if a seed ever
  stops reaching its parser, which is the failure that matters, because the target goes on passing
  either way. Same tell as the caller-envelope seed in 2.10.0; this is the systematic version.

## [2.11.0] — 2026-09-21

**The AE slice's evidence and contract halves.** Wire **v12** unchanged.

Two things this release is actually about. First, **a composed claim finally has an artefact that
carries it**: the axis' headline sentence — *a durable, attributed, cross-domain effect* — was
proved leg by leg and nowhere as a whole, and the execution record now carries the receipt's rung
and the origin domain so the sentence is reconstructable from one record instead of believed across
four. Note precisely what that does and does not establish: it is *reconstructable*, not *enforced*.

Second, **the AE evaluator seam stopped being replaceable in principle only**. AE0 §9's negative
cases existed and passed, but each was written against the reference evaluator's own rule types, so
nobody else's evaluator could run one of them — tests of an implementation wearing the name of a
contract. `mycelium::ae_contract` states them once, evaluator-neutrally, and is public because
fixtures an adopter cannot see hold nobody to anything.

That change immediately paid for itself. Putting a real adapter through the new suite found the
suite's own last assumption: it handed every evaluator a revision *string* and expected it back,
which an adapter whose revision is a policy **digest** can no more do than a file can be told its
own hash. Two evaluators had passed without noticing.

**Upgrade notes**, both one class — an exhaustive `match` needs a `_` arm: `RecordKind` and
`Execution` are now `#[non_exhaustive]`. For `Execution` that arm **must fail safe**: an execution a
reader does not recognise is *we did not look*, never *nothing ran*.

- **Contract fixtures a replacement evaluator can actually run (AE4).** New public module
  `mycelium::ae_contract`: AE0 §9's negative cases stated **once**, in evaluator-neutral terms, so
  the same set holds the shipped reference evaluator and anybody else's.

  They existed before and they passed — but each was written inline against `ReferenceEvaluator`,
  constructing that evaluator's own `Rule` type. **An adopter with a different evaluator could not
  run a single one of them.** They were tests of the reference implementation wearing the name of a
  contract, and the gap was invisible precisely because they all passed.

  A `ContractCase` states an intent, an envelope and the outcome the seam must reach; each
  evaluator translates the intent into whatever it holds internally, and the suite judges only what
  came out of the seam. It is **public on purpose** — fixtures an adopter cannot see hold nobody to
  anything, and *"the evaluator is replaceable"* is a claim about someone else's code.

  Three rules keep it a contract rather than a description. It checks **identifiers, not wording**,
  so a conforming evaluator may say it however it likes. **Declining is not passing** — an
  evaluator that cannot express a clause must declare that in advance, and `Report::conformant`
  requires the declined set to equal the declared set *exactly* (an undeclared decline is a silent
  gap; a stale declaration claims a weakness that is not there). And the suite carries its own
  adversarial tests, because a suite that passes everything is worth nothing.

  **What running a real adapter found.** The first version put a revision *string* in `PolicyIntent`
  and assumed every evaluator would report it back. An adapter whose revision is a policy **digest**
  — derived so it cannot drift from the artifact actually loaded — cannot be told to report a label,
  any more than a file can be told its own hash. `EvaluatorUnderTest::revision_for` and
  `RevisionBinding` fix it: the suite now *asks* what the evaluator will report instead of assuming
  it was told. Nothing about the contract changed; the suite had one assumption left in it, and only
  a third evaluator could surface it.

  Coverage is stated rather than implied: **9 of AE0 §9's 11 rows** are gated (`ae_contract::COVERAGE`
  says which and why). *Shared identity* and *unobserved route* are properties of the evidence record
  and of route coverage, not of a decision — no fixture about decisions can speak to them — and
  *substitution* is partial, because the seam holds no decision cache for a bound decision to be
  substituted into. Guide chapter 20 gains the worked example under *Replacing the evaluator*.

- **The composed guarantee has a gate** — *a durable, attributed, cross-domain effect*. The
  Phase-C audit found it **proved leg by leg and nowhere as a whole**: the receipt tests and the
  federation tests had zero overlap, and no single artefact ever held all four properties, so the
  sentence could not be checked — only believed. Record:
  [`docs/design/composed-effect.md`](docs/design/composed-effect.md).

  The fix is a **join between things that already existed**, not a new mechanism. A receipt knows
  how durable a write was and is then *returned and gone*; an evidence record knows who asked and
  *survives*. So the execution record now carries item 1's own `LocalDurability` — carried, not
  restated as a string, because a second spelling of a rung is a second vocabulary — and the
  **origin domain** as a fact rather than an inference from the principal's spelling.
  `AeEvidence::states_a_composed_effect()` checks all four legs from one record.

  **`OnDisk` only**: `Buffered` survives a process crash and is lost to a power failure, and item 1
  added it precisely to stop a receipt claiming a durability the node never established — a composed
  claim resting on it would re-make that mistake one level up. **Completion only**: `Attempted` is
  the honest answer when a dispatcher did not watch, and it is not an effect that happened.

  Ordering is unchanged: the receipt rides the `Execution` record, which was always *post*-effect,
  while AE0 §5's pre-effect barrier is on the `Decided` record. Nothing is added in front of an
  effect.

  **What it establishes, exactly:** the sentence is now *reconstructable from one artefact* rather
  than believed across four. It does **not** make the composition enforced — nothing here stops a
  durable effect being attributed to the wrong principal; item 7 decides that at its own strength
  and this record carries its verdict.

- **An evidence record can correct another (AE3).** `AeEvidence` gains `supersedes` — the content
  hash of the record this one corrects — with `correcting()`, `is_correction()` and
  `corrects_the_same_action_as()`. AE0 §5 requires correction/supersession references and nothing
  carried them.

  **A correction supersedes; it never edits.** The reason is the one that makes evidence
  append-only at all: a record that could be revised in place could be revised *after someone read
  it*. The corrected record is retained exactly as it was, so a reader sees both and sees which
  replaced which.

  **The hash, not a name** — a correction pointing at "the last execution record for this attempt"
  would be ambiguous the moment there were two, which is precisely when a correction exists. It
  cites the same hash `AeReference::journal_sha256` does, so the two agree by construction.

  **And a correction must describe the same action.** Correcting an *observation* means the outcome
  was misread, not that the action was something else. If the subject, operation, resource or
  identities move, it is a different action wearing a citation, and accepting it would let one
  attempt's evidence be replaced by another's. Checked rather than assumed, because the citation is
  the only thing binding the two records and a citation can be wrong.

- **`LocalDurability` is serialisable** (`serde::Serialize` / `Deserialize`, snake_case), so the rung
  can be recorded into the evidence journal rather than restated there. Additive.

### Changed — API

- **`RecordKind` and `Execution` are now `#[non_exhaustive]`.** A downstream exhaustive `match` on
  either needs a `_` arm — and for `Execution` that arm **must fail safe**: an execution a reader
  does not recognise is *we did not look*, never *nothing ran*. Omitting the field is the stronger
  claim and belongs only to `None`.

  The variants each ships are not the whole list: AE0 §5 names six records, and two exist because
  two are what AE-T needed. The rest wait on the evidence consumer's contract 1.2
  (`not_dispatched`, handover seam 2) — minting them first would put a distinction in our records
  that nothing downstream could read.

  So the attribute lands **before** the variants rather than with them: one announced break instead
  of a series of unannounced ones, which is the §6.6 ledger's own reasoning for the config structs.
  It is a ledger row, with a migration on
  [`docs/guide/deprecations.md`](docs/guide/deprecations.md).

## [2.10.0] — 2026-09-20

**The axis auditing itself, and §12.6 closed.** Wire **v12** unchanged.

Three more Phase-C audit findings closed — v2.9.1 named them as *known and not fixed* — and **five
more defects found by the gates built to prevent them**, which is the release's actual shape: every
new defect here was found by §12.6's own work rather than reported from outside. The trust-edge fuzz
targets found two (a wire `DomainId` bypassing its own validating constructor; a replay bundle
silently corrupting fields). Auditing the coverage of those targets found two more in the journal
reader. And writing AE1's private half against the public seam found the fifth, which no in-crate
test could have seen.

§12.6 is complete: the front door, the companion onboarding checklist, the Phase-C adversarial
self-audit, **nine trust-edge fuzz targets**, and migration notes per deprecation — now an
adopter-facing page, [`docs/guide/deprecations.md`](docs/guide/deprecations.md).

**The upgrade notes.** One is a compile change; two are things to read before you upgrade:

- **`CatalogRefusal`** gains `StaleRevision { seen, offered }` and is now `#[non_exhaustive]`, so an exhaustive `match` needs a `_` arm. Nothing else changes.
- Two **deprecations announced late and now written down**: `system_propose` (deprecated in code
  since 2.1.0, never in this changelog until now) and **reading isolation into `cluster_name`**,
  which never provided any. If you relied on `cluster_name` to keep deployments apart, **you do not
  have that separation** — two nodes with different names that can reach each other and pass
  admission will gossip. The mechanism that separates is a federated domain. Neither is removed;
  both are on the `3.0.0` ledger with a migration on the new page.

### Fixed

- **The replica-sync rung stopped claiming what a dropped append broke.** The rung rested on *the
  store holds it, therefore its record was appended*. The inbound path applies a received update and
  **then** appends, and in `Async`/`Os` that append is a `try_send` which drops on a full queue and
  still answers `Ok` — with both call sites discarding the verdict. So a peer under write-ahead-log
  backpressure held a value with **no log record**, answered `Persisted`, and the origin's receipt
  named it as holding the record across a crash. `WalHandle` now counts skipped appends
  (`dropped_appends()`), and a node that has skipped any declines the rung rather than claiming it —
  it cannot tell whether *this* record was the one it dropped, and per-entry tracking is what the
  contracts record declined (§1a). **A node that never drops is unaffected.**
- **A key rotation's overlap now reaches the call path.** `TrustBundle::rotate` is deliberately
  overlapping so a partner can move to the new key at its own pace, but both verifiers consulted
  `key_for` — the *current* key only — and `acceptable_keys` was called from nowhere in production.
  A partner still signing with the retiring key was refused as **`BadSignature`**, the refusal this
  module reserves for *someone is forging*. A partner doing exactly what the rotation design tells it
  to do was reported to the operator as an attacker.
- **A catalogue revision never goes backwards.** `DomainPolicy::revision` documents itself as
  monotonic — *a consumer that has seen a higher revision must not accept a lower one* — and the rule
  was implemented nowhere. A signed reply carries no expiry and no nonce, so one captured while an
  export was granted still verified after the grant was withdrawn, and the withdrawn export
  reappeared in the client's view. Bounded: the catalogue confers **visibility, not authority**, and
  the provider re-authorises at call time, so a replay ended in a refusal rather than an
  unauthorised call.
- **A `DomainId` from the wire now obeys its own constructor.** `DomainId::new` refuses anything
  outside `[a-z0-9.-]` and anything over 253 bytes, and the type says why: two ids differing only in
  case are *one domain to a human and two to a `HashMap`, and the place that difference would
  surface is a trust decision*. A derived `Deserialize` on a newtype writes the inner field and
  validates nothing — so that rule held only for ids this process **constructed**, while most
  `DomainId`s are **parsed**: the origin on a presented credential, both domains on a catalogue
  reply, a descriptor's subject, a policy's grants. Each is read from partner-controlled bytes
  before anything about them has been verified. Bounded, and worth stating exactly: this **could not
  forge authority** — an id no trust bundle holds a key for is refused whatever its spelling. What it
  admitted was `Depot` and `depot` as two entries an operator reads as one; an id carrying `/`, which
  makes `federation:{domain}/{principal}` ambiguous about where the domain ends; an id carrying a
  newline, which reaches a log line before it is authenticated; and an id of unbounded length, which
  reaches all three. `PrincipalId` and `TermId` had the same gap against their own non-empty rule and
  are fixed with it.

- **A torn journal tail is no longer counted as a record.** `count_records` decided a record was
  complete by seeking past it and checking only that the seek returned `Ok` — but **seeking past the
  end of a file is legal and succeeds**, so a tail whose length prefix outran the file counted as a
  record. It drives the next append's sequence number while `read_journal_from` stops *at* the torn
  record, so the two readers disagreed: the next append took a seq no reader would ever hand out,
  and a cursor-based exporter would see that seq go missing. Under both the evidence journal and the
  rights ledger. The landing position is now checked rather than the seek's `Ok`.
- **A journal record is not allocated for before the file is known to hold it.** The reader took a
  `u32` length prefix straight into `vec![0u8; want]`, and the `max_bytes` bound is checked *after*
  the read and by design never for the first record — so a single flipped bit in a length prefix
  asked for up to 4 GiB before anything looked at whether the file could supply it. The journal is
  node-local, so this is corruption and disk error rather than an attacker; a corrupt journal should
  be reported by the reader that finds it, not resolved by the allocator. **Behaviour is unchanged**
  — such a record already read as a torn tail, just after the allocation.

- **A replay bundle's fields survive being written and read back.** `mycelium-sim`'s flat object
  form is written by `quote` and read by `unquote`, and the pair disagreed in two ways. `unquote`
  used `trim_matches('"')`, which strips **every** trailing quote rather than the one delimiter — so
  a value ending in an escaped quote came back with the quote gone and its backslash left behind
  (`he said "hi"` read back as `he said "hi\`). And `quote` never escaped newlines although the
  reader is line-oriented (`parse_object` iterates `text.lines()`), so a value containing a newline
  was **truncated at it and the rest dropped without a word**. `witness.assertion` is free text, so
  this was reachable: a replay would check a shorter, weaker assertion than the one recorded and
  then report success — a silent divergence, in the crate whose whole purpose is to make divergence
  loud. Both are fixed, and the fidelity claim is now pinned by a test and asserted by the fuzz
  target. **Bundles already on disk read back unchanged** — the escape is new on the write side and
  the reader is strictly more faithful than before.

- **Fuzz targets for the parsers the axis put on a trust edge** (§12.6) — nine of them, across the
  three surfaces §12.6 names: the caller-context frame **and the envelope behind it**, the presented
  federation credential, the catalogue reply, the descriptor/policy pair, the trust bundle, the
  replay trace, and the replay bundle's other readers (the recorded outcome codec and the
  hand-rolled `build.json`/`config.json` parser). Each asserts **the invariant the parser is relied
  on for** rather than merely surviving — byte conservation for the frame (no input byte lost or
  invented, whichever way it is classified), and round-trip stability for the parsed objects, whose
  signatures cover bytes rebuilt from the parsed fields. They run in the nightly `cargo-fuzz` job
  and, as mutation passes over valid seeds, in the in-suite mini-fuzz on every PR — which is how
  the `DomainId` gap above was found.
- **A fuzz target for `serde_fixint`**, the hand-rolled byte layer under the rights ledger's signed
  documents, over the two types that reach it from outside the process — an unsigned on-disk
  `LedgerEvent` and a peer-writable `PublishedRightsHead`. Kept although a 20,000-case mutation sweep
  found nothing, because that sweep is far weaker than coverage-guided fuzzing and because a
  hand-rolled binary decoder reading bytes off disk is the shape behind the unbounded-allocation
  decode DoS that sat uncaught through M2 Run-20.

- **The action envelope binds a scoped mandate (AE1).** `ActionEnvelope` gains
  `mandate: Option<MandateBinding>` — *which* appointment (`holder`, `term`, `scope`, `epoch`) and
  what the enforcement point established about it (`Established` · `Refused(MandateRefusal)` ·
  `Unknown`). Before this, a rule naming `mandate` could only be declared as a fact the evaluator
  **cannot** establish, so every mandate clause answered `Indeterminate` by construction.
  `Rule::requiring_mandate(scope)` now decides it.

  **A refused mandate is a fence, not a policy input:** it denies before any rule is consulted,
  carrying item 5's own refusal by name. Evaluating it after the allow-list would let a matching
  allowance turn a refusal into a permit — for `Superseded`, exactly the laundering item 5 refuses
  to allow (which is why it is not a `Conflict`). Three answers stay apart because an operator told
  the wrong one fixes the wrong thing: *claims none* and *could not check* are `Indeterminate`
  (AE0 §9 — absence is never a denial), while *checked and refused* is `Deny`.

  Additive: `ActionEnvelope` has been `#[non_exhaustive]` from birth. The gateway passes `None` —
  it is a route-level preflight with no fence to consult, so a mandated rule reads it as authority
  not established. Resource-side enforcement is AE2.

### Deprecated

**The `3.0.0` removal ledger now has an adopter-facing page:**
[`docs/guide/deprecations.md`](docs/guide/deprecations.md) — what is on the way out, what replaces
it, and, per entry, whether the compiler will tell you. **Nothing listed there is removed on 2.x.**

§12.6 of the plan requires that every ledger entry ship a migration note *the day it is deprecated,
not the day it is removed*. An audit found three entries that had never been announced in this file
at all, two of them for months. They are announced here rather than quietly backdated:

- **`system_propose` is deprecated in favour of `cluster_propose`** — `#[deprecated(since = "2.1.0")]`
  in code since 2026-07-15, and never mentioned in this changelog until now. Identical behaviour, and
  the gateway still accepts `"system"` on the wire; the rename makes the consensus verb match its
  scope (`SignalScope::Cluster`). Unrelated despite the name: `system_stats()` is node-local runtime
  state, not a scope.
- **Reading isolation into `cluster_name` is deprecated.** It never provided any. A cluster is
  emergent from network reachability — peer exchange plus CA admission — so two nodes with different
  `cluster_name`s that can reach each other *will* gossip. Deployments that must not merge need a
  **federated domain** (`DomainId`, a trust bundle, per-partner authorisation). `cluster_name` keeps
  working as a display label; what is deprecated is treating it as a boundary.
- **`ConsensusResult::Committed { persisted: bool }` is superseded by the receipt verbs**
  (`cluster_propose_receipt` / `group_propose_receipt`). A `bool` cannot separate *"the bytes are on
  disk"* from *"the write was queued and nothing is promised"* — `false` means durability was **not
  established**, never "absent from the WAL". The field keeps working; a receipt names its rung and
  nothing above it.

**Known and not fixed:** only two of the seven ledger entries carry a `#[deprecated]` attribute, so
for the rest the page and this section are the whole notice. `docs/guide/building-on-mycelium.md`
claimed the old surface is always `#[deprecated]`; that claim is corrected to what the code actually
does. Adding the missing attributes is an API-surface change with its own blast radius (internal call
sites would need `#[allow(deprecated)]` under `-D warnings`) and is left to a separate change rather
than folded in here.

### Changed — API

- **`CatalogRefusal` gains `StaleRevision { seen, offered }` and is now `#[non_exhaustive]`.** A
  downstream exhaustive `match` needs a `_` arm. Reusing the existing transport error would have
  avoided the change and would have been the refusal-conflation this module forbids: a stale view is
  not a broken connection, and an operator told the wrong one looks in the wrong place.

### Measured, not fixed — the remaining trust-edge parsers

The coverage sweep behind the fuzz targets named four further parsers that read externally-supplied
bytes before anything is verified. The journal's framing was the serious one and is **fixed above**.
The other three were then **measured rather than left as a list of worries**, and each turns out to
be weaker than the list implied — recorded here because an unqualified list of "unfuzzed parsers"
reads as a set of vulnerabilities, and these are not:

- `src/control/ledger.rs:333` — `PublishedRightsHead::decode` returns `Option` and cannot panic, and
  its only callers today are inside `#[test]` functions. It is a **reserved surface**: a trust edge
  when a production reader exists, not one now.
- `src/control/ledger.rs:362` — `RightsLedger::open` is live and decodes unsigned on-disk
  `LedgerEvent`s through the hand-rolled `serde_fixint`. A 20,000-case mutation sweep over a valid
  encoding produced **3,808 successful decodes and no panic**: the decoder checks its remaining
  length before every read, so the unchecked subtraction in `remaining()` is not reachable from
  these types. It now has a fuzz target regardless — see below.
- `mycelium-commitment/src/lib.rs:482` — offers and awards are decoded from the gossip log and
  `award()` picks a winner from them without either type being signed. `serde_json` underneath, so
  the exposure is **semantic, not memory safety**: a peer can publish an offer, which is what an open
  contract net is for, and what is missing is provenance rather than a decoder bound.

### Still open from the audit — neither is a patch

- **The per-partner budget is enforced consumer-side only.** `GatewayPool` lives in the *client*; the
  edge has no slot accounting, so a partner that does not run the honest client can consume another
  partner's share of a provider's gateway. A design gap.
- **The composed guarantee still has no gate.** *A durable, attributed, cross-domain effect* is
  proved leg by leg and nowhere as a whole — the receipt tests and federation tests have zero
  overlap. Underneath: a receipt carries **no principal**, the evidence journal carries **no
  durability rung**, and a receipt is **never persisted**. Closing it means deciding whether a receipt
  should be *recorded* rather than merely returned.

  > **Closed in [Unreleased] (2026-09-21).** The answer was *yes, recorded* — the execution record
  > carries the receipt's rung and the origin domain, so the claim is reconstructable from one
  > artefact. See [`docs/design/composed-effect.md`](docs/design/composed-effect.md). The finding is
  > left in place as the historical record of what was open at 2.10.0.

## [2.9.1] — 2026-09-19

**A security PATCH from the axis' own Phase-C adversarial self-audit.** §12.6 asks for a five-pass
adversarial audit at Phase C exit over items **1 + 2 + 7 together**, because those three compose into
the first *composed* guarantee this plan makes. It found four defects in shipped code. All four are
fixed; three carry a new gate. Wire **v12** (`PREV = 11`) **unchanged**; on-disk format unchanged;
no API change. Upgrade in place.

### Fixed

- **A durability receipt reported `on_disk` for a write that had failed.** `tokio::fs::File::write_all`
  copies into an in-process buffer and queues the real syscall, returning `Ok` **before it runs**;
  `File::sync_data` then completes that in-flight write and **discards its error** (stashed to
  surface on the *next* write) before syncing the descriptor, which succeeds. So the pair answered
  `Ok` for a record that failed with `ENOSPC`, the receipt claimed the strongest rung in the
  vocabulary, and the error went on to blame the following record. `set_requiring_sync` was worst
  affected: its contract is *durable or nothing*, and it returned `OnDisk`, then applied **and**
  gossiped. A completed write is now the unit of the write seam, so a failure is reported by the
  record that caused it. (`do_snapshot` already flushed for exactly this reason before reading the
  tail back; the receipt path did not.)
- **A federated partner could read or cancel any caller's tasks.** A credential binds **one export**,
  and only `tasks/send` authorised against it. `tasks/get` and `tasks/cancel` took no caller and no
  export, looking up any task by id — and ids are caller-supplied on `tasks/send`, so they are
  enumerable. A partner granted a single export could read any task's completed artifact, **including
  a native in-domain caller's**, or destroy it. Both are now refused for federated callers, as
  `tasks/sendSubscribe` already was.
- **A client could supply its own caller-context frame.** The raw-emission routes
  (`/gateway/signal/emit`, `/gateway/shard/emit`) carry a client's bytes verbatim with *this node* as
  the sender, so a frame the client prepended verified as the gateway's own envelope — `via` matched
  by construction. The existing guard caught only **bare** bytes. On a node with no `tls` identity the
  signature branch does not run at all, so nothing else stood between a client and an arbitrary
  principal. Both routes now refuse a payload carrying a frame where one would be read.
- **A remote panic in item 7's own refusal path.** `Bytes::slice` asserts, so an under-length payload
  panicked where an RPC reply echoes the 8-byte correlation nonce — reachable because a short signal
  on a served kind fails verification and the refusal branch answers by calling into that slice,
  killing the provider's serve task permanently. A nonce never received cannot be echoed, so the
  reply is dropped with a warning.

- **A replay test was passing because of the durability defect.** `a_replayed_write_does_not_touch_the_disk`
  asserted that a replayed write performs no I/O, and opened a read-only file to prove it. The replay
  path *does* re-perform a recorded success — the module's own "the effect happens in both modes";
  only a recorded **failure** skips it, which is what makes a fault sweep honest. The write was
  failing with `EBADF` every run and the swallowed error hid it. Corrected and renamed
  (`a_replayed_write_reperforms_the_effect_it_recorded`); found only because fixing the swallow made
  it fail in CI.

### Operator notes

- **No action required to adopt the fixes** — no configuration, no API change, no wire change.
- **A node running `SyncMode::Flush` or using `set_requiring_sync` may now see failures it did not
  see before.** That is the point: those writes were failing already and being reported as durable.
  A rising `mycelium_kv_receipts_total{local_durability="failed"}` after upgrading is the fault
  becoming visible, not a new fault.
- **A federated partner calling `tasks/get` or `tasks/cancel` now receives `-32004`.** No in-tree
  client does this; the federation client only issues `tasks/send`.

### Known and not fixed in this patch

The audit's other findings are recorded rather than closed, and none is a privilege boundary:
signed catalogue replies carry no expiry or nonce, so a reply from a withdrawn grant can be replayed
into a stale *view* (the provider re-authorises at call time, so it ends in a refusal, not an
invocation); key **rotation** is unreachable on the call path, so a partner mid-rotation is refused
as `BadSignature`; the per-partner budget is enforced consumer-side only; and `persisted_by`'s "this exact content" is
stronger than the primitive beneath it. Each is tracked for the next release.

> **Post-release correction (2026-09-20).** This section originally also listed `Buffered`'s
> *"survives a process crash"* as an open over-claim. It is **not** open: the same flush that fixed
> the `on_disk` defect closes it. `poll_flush` awaits the in-flight operation and returns the write's
> real result, so once `fs_write_all` returns `Ok` the syscall has run and the bytes are in the OS
> page cache — which is exactly what `Buffered` claims. The release notes under-sold their own fix;
> the tag message carries the original wording and is left as the historical record.


## [2.9.0] — 2026-09-19

**The axis proves itself.** A MINOR whose headline is not new capability but new *evidence*: a whole
node now replays deterministically, every item of the v3 axis has a runnable demonstration, and two
of those demonstrations found real defects on the way in. Wire **v12** (`PREV = 11`) **unchanged**;
on-disk format unchanged; a backwards-compatible rolling upgrade.

- **The scheduler seam's first arm** (item 6) — the last unbuilt piece of the public axis. A
  replayed wait is taken on tokio's paused clock rather than collapsed to one yield, so a task's
  wait orders it against every other waiting task. With it a whole-node recording of a linearizable
  award **replays without divergence**, and the commitment companion's CN2 rung is complete. The
  design note predicted the mechanism and missed the blocker: `Record` wrote its trace entry *after*
  a wait while `Replay` checked *before* one, so a recording's order was the order waits **completed**
  and a replay's the order tasks **entered** them. Both now check in at the same point.
- **§12.1's gallery is complete** — one decisive demonstration per item, each ending by naming what
  it does *not* establish: the receipt ladder (1), federated domains (2), the knowledge layer (3),
  the control envelope (4), the curator handover (5), replay a bundle (6). CI **runs** the four CLI
  ones rather than only building them: building proves the API compiles, running proves the
  demonstration still demonstrates.
- **Two defects the demonstrations found, both fixed and pinned with plants:** a revoked wiki curator
  was told to *"re-read and retry"* — advice that refuses forever — and is now refused by name; and a
  federated call against a blackholed gateway hung instead of returning `DeliveryUnknown` within a
  bound.
- **A recorded doubt resolved by measuring it:** scenario C had said hysteresis was not load-bearing
  while the release spacing was on. It is — the earlier reading was schedule coverage. The sweep
  gained a per-breaker plant, and the measured *bound* (hysteresis damps a hover, it does not settle
  one) is recorded with it.

**The upgrade note**, and there is one. A `mycelium-wiki` write refused by the **mandate fence** now
returns `WikiError::Io` carrying `MandateRevoked` instead of `WikiError::Conflict`. Retry logic keyed
on `Conflict` will no longer retry that case — which is the point, since a replaced curator retrying
refuses forever — but it *is* a change in what a caller sees. A genuine lost compare-and-swap race
still returns `Conflict`. Stores with no mandate fence configured are unaffected, byte for byte.

- `mycelium_core::sim_seam::{pause_clock_for_replay, resume_clock_after_replay}` and `mono_instant`
  (feature `sim`, off in every shipped build; `sim` now also enables `tokio/test-util`).
- `mycelium_wiki::{MandateRevoked, WikiError::mandate_revoked, WikiError::as_mandate_revoked}`.
- Examples: `receipt_ladder`, `control_envelope_viz` (+ `.html`), `mycelium-sim`'s
  `replay_a_bundle`, `mycelium-wiki`'s `curator_handover`.

### Changed — hysteresis is load-bearing after all; the sweep now has a per-breaker plant (v3 item 4)

- Scenario C had recorded that **hysteresis was not load-bearing** while the release spacing was on. Measuring
  it showed that was an artefact of *schedule coverage*: every schedule ends by dropping inbound before the
  quiet tail, so a persistent hover never coexists with the window where *at rest* is judged, and in the
  schedules that do cross the band the per-tick fill step (0.575) is nearly three times the band (0.200), so
  the fill steps over it. Hold the hover — a lone member at 400‰ — and the shipped breakers give **one**
  boundary transition where removing hysteresis *alone* gives **twelve**.
- **`Breakers::without_hysteresis()`** — a per-breaker plant. `Breakers::off()` removes every breaker at once
  and so can never name which one did the work.
- Three new tests: the comparative claim, the plant, and one asserting the release spacing does **not** fire on
  these schedules — because if it did, the two breakers would overlap and the reasoning would need revisiting.
- **The bound, also measured and now recorded:** hysteresis *damps* a hover, it does not *settle* one. At 350‰
  it takes 15 transitions to 11. A first draft of the gate asserted rest, failed, and that is where the bound
  came from. The ADR's §5 note is corrected rather than quietly edited.

### Added — the control envelope, item 4's decisive demonstration (v3 §12.1 — the axis' gallery is complete)

- **`examples/control_envelope_viz.rs`** (+ `.html`, port `:8096`, `--features metrics`) — the same
  proposal stream run past `control::decide` under **all four profiles at once**, in four live
  columns: `legacy` proceeds (the predicate is never consulted), `observe` counts a would-hold *and
  proceeds anyway*, `enforce-local` holds with the refusal naming what was missing
  (`TooFewHeard { heard, required }`), and `enforce-allocated` additionally refuses over the
  allocated budget — with the rejection **recorded** in the `RightsLedger`, never silent. The ladder
  is steppable from the page, which is the operator judgement the shadow-mode runbook describes.
  Every branch is a shipped function (`decide`, `spacing_allows`, `may_propose`, `RightsLedger`)
  driven by a load generator rather than a re-implementation. Follows the UI-example contract
  (gateway + metrics, Ops Console link, concepts box); like every browser showcase it runs
  continuously and is built, not run, in CI.
- **This completes §12.1's gallery**: item 1's receipt ladder, item 3's knowledge layer, item 2's
  federated domains, item 5's curator handover, item 6's replay-a-bundle, and item 4's control
  envelope.

### Fixed — a revoked curator was told to retry (v3 item 5, `mycelium-wiki`)

- **The mandate fence now names its own refusal.** A write whose appointment had moved failed the
  git ref transaction and was reported as `WikiError::Conflict` — *"compare-and-swap version
  conflict (re-read and retry)"* — which is the opposite of the right advice: a replaced curator
  who retries will refuse forever. `WikiError::mandate_revoked` / `as_mandate_revoked` carry
  `MandateRevoked { refname, expected, found }` inside `WikiError::Io`, the same additive shape as
  `GateRefusal`, so no downstream match breaks. A genuine lost CAS race still reports `Conflict`,
  which still *is* a retry signal.
- Found by building item 5's decisive demonstration — the first end-to-end exercise of the fence,
  whose existing tests check the transaction *text*. Pinned by
  `a_moved_appointment_is_refused_as_a_revocation_and_writes_nothing` and its plant
  `a_stale_version_under_a_valid_appointment_is_still_a_conflict`.

### Added — the curator handover, item 5's decisive demonstration (v3 §12.1)

- **`mycelium-wiki/examples/curator_handover.rs`** (feature `git-store`, run in CI) — a council's
  curator appointed as a git ref, writing under it, the council re-appointing mid-stream, the
  replaced curator's next write **refused as a revocation with nothing written** (content and
  `HEAD` shown unchanged either side), the new curator writing, and the git log showing both names:
  authority moved, history did not.

### Added — replay a bundle, item 6's decisive demonstration (v3 §12.1)

- **`mycelium-sim/examples/replay_a_bundle.rs`** — a depot's surplus-food sweep recorded through the
  kernel, written as a bundle, read back and replayed; then the same bundle replayed against a
  deliberately broken version, which **diverges at the exact effect that changed**, and against the
  fixed one, which replays green. It ends by naming the *other* shape of failure — a bug whose
  effects never reach a seam, which replays perfectly and is caught by the bundle's witness
  assertion instead — and by saying why the real WAL/snapshot race is replayed by the corpus gate
  rather than by a binary: reproducing it means removing the WAL-tail merge, and that switch is
  `cfg(test)` on purpose, because a binary that could disable a durability fix would be a data-loss
  switch. Run in CI.

### Added — the receipt ladder, item 1's decisive demonstration (v3 §12.1)

- **`examples/receipt_ladder.rs`** — the same write made four ways, so the rungs are visible rather
  than described: `set` (a bool that names no rung) · `set_with_receipt` with no persistence
  (`NotConfigured`) · `set_requiring_sync` on that node (**refused**, carrying whether persistence
  was configured at all) · `set_with_receipt` on a buffered node (`Buffered`) ·
  `set_requiring_sync` there (`OnDisk`) · `set_with_replica_sync` (rung 3). Then it shuts the peer
  down — the peer that cannot answer is **unknown**, never failed — and reopens the persisted
  node's data directory to show what replayed. It closes by naming what it does *not* establish,
  including that its clean shutdown stages neither of the failures `Buffered` declines to survive.
- **CI now runs the decisive demonstrations** (`receipt_ladder`, `knowledge_layer`,
  `federated_domains`) instead of only building them: building proves the API still compiles,
  running proves the demonstration still demonstrates. Gallery rows added for all three.

### Added — the scheduler seam's first arm; a whole node now replays (v3 item 6)

- **`mycelium_core::sim_seam::pause_clock_for_replay`** (with `resume_clock_after_replay`) — under `sim`,
  arms the replay arms to take each recorded wait on tokio's **paused clock** instead of collapsing it to one
  `yield_now`. A wait becomes ordering information again: no wall time is spent, but a task's wait still orders
  it against every other waiting task. Requires a `current_thread` runtime; the `sim` feature now pulls in
  `tokio/test-util` for the clock control, and is off in every shipped build.
- **A whole-node recording now replays.** `mycelium-commitment`'s day-old pin on the gap is the claim:
  `a_whole_node_recording_of_a_linearizable_award_replays_under_the_scheduler_seam`, with the unarmed replay of
  the same trace kept beside it as the plant.
- **Fixed on the way, and it was the real blocker:** `Record` wrote its trace entry *after* a wait while
  `Replay` checked its request *before* one, so a recording's order was the order waits **completed** and a
  replay's was the order tasks **entered** them. Those differ whenever two waits overlap — the only case the
  arm exists for. Both modes now check in at the same point. Pausing the clock alone did **not** flip the pin.

## [2.8.0] — 2026-09-18

**The v3 contracts axis.** The largest MINOR since 2.0 — eight numbered items and three new
companion crates, all of it additive. Wire **v12** (`PREV = 11`) **unchanged**; the on-disk format
is unchanged; a backwards-compatible rolling upgrade.

What shipped, one line each:

- **Contracts (item 1).** An acknowledgement names its rung and nothing above it: typed durability
  receipts, stable operation identity, a required local sync, the exact-identity ack, the
  persisted-by-peer protocol, and one complete external-effect adapter with a destination-commit
  receipt.
- **Federated domains (item 2).** A domain is one independently admitted mesh; federation carries
  *exported services* across an authenticated edge and never joins the transports. **Its release
  gate is met:** two meshes in separate processes, the link cut at the network layer, non-merger
  proved from membership tables, consensus state and connection tables.
- **Knowledge layer (item 3).** Claim, observation, assessment and acceptance as attributable
  records, with reader-specific acceptance and a semantic gate that pins what evidence *cannot* do.
- **Adaptive stability (item 4).** One admission contract for every governor, strict budgets as
  allocated rights, admission control reported at the companions' queues, and a shadow-first
  profile ladder an operator can watch before enforcing.
- **Scoped mandates (item 5).** Authority checked by the protected resource, never inferred from a
  role advertisement; the fence sits inside the store's own transaction.
- **Deterministic replay (item 6).** Production decision logic under controlled time, RNG, channels
  and storage; a checked-in corpus replayed in CI; a static check that keeps new nondeterminism
  outside the seams.
- **Gateway caller identity (item 7)** — a provider sees the client principal, never the gateway
  node — and **threat model revision 2 (item 8)**.
- New crates: **`mycelium-sim`** (the replay kernel), **`mycelium-effects`** (the external-effect
  adapter), **`mycelium-commitment`** (the contract net as five records).

**The upgrade notes.** Every one is the same class: a struct gained a field, which breaks an
*exhaustive struct literal* and nothing else. Reading and matching on fields is unaffected, and the
documented `..Default::default()` pattern is unaffected.

- `GossipConfig` gained `domain_profile`.
- `GovernorSnapshot` gained `profile`, `held_by_spacing`, `held_by_settling` and `settled_unknown`;
  `ParamSnapshot` gained `pending`.
- `OpacityHint` gained `release_spacing_ms`.
- `BoardConfig` gained `high_watermark`; `BoardStats` gained `rejected`.

**One behavioural change worth stating on its own.** A federated call against a gateway whose
network is *blackholed* — interface gone, default route still present — now returns
`DeliveryUnknown` within a bound (5 s to connect, 30 s in total, `FederationClient::with_timeouts`)
instead of waiting indefinitely. A silent gateway is contractually an unknown delivery, and a
client that never returns cannot deliver that verdict.

### Fixed — a blackholed federation gateway hung instead of reading as unknown (v3 item 2 PR 10b)

- **`FederationClient` now bounds every HTTP attempt** (5 s to connect, 30 s in total;
  `with_timeouts` to change them). A gateway that *refuses* fails fast, but one that is
  **blackholed** — interface gone, default route still present, which is what a real network
  severance looks like — sends nothing, and the unbounded client waited forever. Since a silent
  gateway is contractually `DeliveryUnknown`, a client that never returns cannot deliver that
  verdict. Found by the new two-mesh Docker suite; pinned by
  `a_blackholed_gateway_is_unknown_within_a_bound_rather_than_hanging` (RFC 5737 TEST-NET-1).
- The federation node config no longer sets `health_check_interval_secs` equal to
  `reconnect_backoff_secs`, which `validate()` warns about on every node start.

### Added — the two-mesh Docker suite; item 2's release gate met without a caveat (v3 item 2 PR 10b)

- **`make test-federation`** (CI job `federation`) — item 2's release-gate choreography with **process
  isolation** (one container per node) and a **real network severance** (`docker network disconnect`), the two
  things the in-process test could not claim. `make test-federation-clean` tears down; `make federation-keys`
  derives the suite's public keys from its fixed seeds.
- **`examples/federation_node.rs`** — one binary, four roles (`member`, `gateway`, `probe`, `keys`) driven by
  environment variables, plus a test-only `/fed-admin` surface (tables, propose, committed, kv, policy, revoke)
  that every assertion reads instead of a log line. Requires `gateway`, `tls`, `a2a`, `cli`.
- `docker/docker-compose.federation.yml`, `docker/Dockerfile.federation`,
  `tests/integration/run_federation.sh`.

### Added — the signed catalogue reply (v3 item 2 PR 10a)

- **`CatalogReply`** now carries `for_partner` (whom the list was filtered for), `issued_at_ms` and an optional
  `signature` — Ed25519 over a tagged canonical form (`TAG_CATALOG`) that covers the domain, the partner, the
  revision, the time and the exports, so a reply cannot be replayed to another partner or answered by a gateway
  without the domain's key. `CatalogReply::{canonical_bytes, signed, verify}`; `CatalogRefusal::{Unsigned,
  BadSignature, WrongDomain, NotForUs}`.
- **`FederationEdge::with_signing_key(key)`** signs every catalogue reply; `signs_catalogue()` reports it.
- **`FederationClient::with_partner_key(key)`** requires a verified catalogue: unsigned, forged, wrong-domain or
  misaddressed → `ClientError::Catalogue(refusal)` and the link stays `Down`. Without a key the catalogue is
  relied on only as an observation attributed to the gateway asked (PR 3), as before. Freshness is unchanged:
  the resolver's window, not the signature.
- The JSON on `GET /federation/catalog` gained three fields; a PR 8 client reading it still works
  (`signature` is optional on the wire).

### Added — the release gate's choreography over the transport (v3 item 2 PR 9)

- **`GossipAgent::connected_peers()`** — the peers this node holds a *connection* to (the transport's own
  record), beside `peers()` (what membership believes). Exists for the federation gate's *traces* leg.
- **`GatewayPool::retire(id)`** / **`FederationClient::retire_gateway(id)`** — a replaced or dead gateway is
  removed from the pool and never admitted again. The pool keeps no health memory by design (a silent gateway
  is a per-call outcome), so without this an at-most-once call pays a dead gateway once per call.
- **The two-mesh harness** (`lib_tests.rs`): `assert_never_merged` now scans `consensus/` too, over keys *and*
  values, and adds a connection-table leg over `connected_peers()`; its non-vacuity control checks the new leg.
- **Test:** `lib_tests::federation_transport::the_release_gates_choreography_over_the_transport` — every node
  under the enforced domain profile (TLS, SWIM off), two meshes under two auto-generated CAs: discover, invoke,
  consensus in each mesh, lose the only gateway (explicit `DeliveryUnknown`s, discovery cannot refresh, link
  `Down`), keep working locally (gossip and consensus on both sides), change the grant mid-partition, bring up a
  replacement gateway, reconnect (refused until discovery refreshes; the changed grant is what it sees), retire
  the dead gateway, honour authority issued before the partition to its expiry and not past it, and assert
  non-merger from the tables, the consensus namespace and the connection tables — before, during and after.
  Plus a node holding B's CA that cannot join A. The gate's *in-process* form is met; the Docker two-mesh suite
  and a signed catalogue reply remain (record §13).

### Added — the federation transport, first arm (v3 item 2 PR 8)

- **`mycelium::federation::edge`** (`tls`): `PresentedCall` — the `FederatedCaller` and its signature as one
  JSON header (`x-mycelium-federation-call`) on an ordinary A2A `tasks/send`; `FederationEdge` — this domain's
  identity, exports, policy and trust bundle, with `authenticate` (signature, lifetime, expiry, skew; binds
  nothing), `authorize` (the export binding and the policy grant) and `catalog_for` (the *filtered* list under
  the reserved export `federation.catalog`); `set_policy` / `revoke` take effect on the next request.
  `GossipAgent::with_federation_edge(edge)` (`gateway` + `tls`, before `start()`) serves
  `GET /federation/catalog` and admits federated credentials on `/a2a`.
- **`mycelium::federation::client`** (`gateway` + `tls`): `FederationClient` — `connect` fetches the catalogue
  and brings the `PartnerLink` to `Ready`; `call(export, text, repeatability)` runs link → resolver → pool →
  HTTP and returns the task's text, with every refusal one the contract already names (`ClientError::{Link,
  Resolve, Outcome, Refused, Transport}`); a silent gateway is `DeliveryUnknown` for at-most-once and a failover
  for repeatable; `disconnect` / `revoke` on this side.
- **`verify_federated_credential`** (`federation::call`): the identity-only half of `verify_federated_call`,
  for a verifier that learns *who* before it learns *what*.
- **`federation_principal(origin, principal)`** → `federation:{origin}/{principal}`, what a provider's
  `authorized_callers` sees for a federated caller.
- **Gateway:** a presented federation credential that fails is refused (400 malformed, 401 untrusted / expired /
  no edge attached), never anonymised; a bearer and a credential on one request is a 400;
  `tasks/sendSubscribe` under a credential is `-32003` (federated calls are unary); an authenticated-but-refused
  export is `-32003` with the refusal's reason. The anonymous and bearer paths on `/a2a` are unchanged.
- **Test:** `lib_tests::federation_transport` — two meshes, a call that crosses, eight plants refused with no
  dispatch, revocation mid-session, then the PR 1 never-merged assertions *after* bytes crossed. The release
  gate's traces leg and partition choreography are not claimed (record §13).
- **`mycelium_core::sim_seam::mono_instant()`** — an `Instant` to *store* (a public field whose age is read
  through `mono_elapsed`), constructed by the seam so the static seams check has no `Instant::now` to count
  outside it. Identical in both arms by design.

### Changed — the control profile reaches every governor; the shadow-mode rollout runbook (item 4 §7)

- **`Legacy` is again exactly the pre-4b behaviour.** The tuning gate's spacing and settling and the opacity
  boundary's release spacing act only from `EnforceLocal` up; under `Observe` they are evaluated and **counted**
  (`held_by_spacing` / `held_by_settling` / `opacity_releases_spaced` then mean *would have held*;
  `GovernorSnapshot.profile` says which reading) and nothing is held; under `Legacy` the contract is not
  consulted. `TuningGovernor::set_control_profile` (fanned out by `GossipAgent::set_control_profile`).
- **The provisioner's rights refusal is Tier C:** an install the ledger says the node holds no unit for is refused
  only under `EnforceAllocated`; every other profile admits it, counts it in the new
  `Provisioner::rights_would_refuse()`, and still records `admission.rejected` in the ledger.
- **Scenario C** pins the ladder: the "settles" sweep runs under the enforcing profiles; under `Legacy` the same
  breakers do nothing and the schedules flap; under `Observe` they flap while counting every hold not made.
- **Gateway:** `GET /gateway/govern` gains `control` — the node's profile and its tripwires (`would_hold`,
  `opacity_releases_spaced`, the tuning counters with the profile they were taken under) — and each param's
  `pending`; **`POST /gateway/govern/profile`** (`govern:write`) steps the ladder by name (`legacy` · `observe` ·
  `enforce-local` · `enforce-allocated`, `Profile::{name, parse}`, pinned); an unknown name is `400` and changes
  nothing. Per node, like every govern route.
- **Runbook:** `docs/operations/control-profiles.md` — the ladder per governor, the tripwires to watch before
  stepping up, rollback, the routes. **Upgrade note:** `GovernorSnapshot` gained `profile`;
  `opacity::spaced_transition` (crate-private) now takes the profile and returns `Spaced`.

### Added — the award under the acceptor's mandate (v3 §6.9, CN3)

- **`ContractNet::commit_award_under_mandate(award, acceptor, authority, now_ms)`** — the mandate must be
  the acceptor's own (`MandateNotTheAcceptors { holder, acceptor }` by name otherwise), and the requirement's
  `ResourceAuthority` must authorize `accept` for it now: a stale holder's award — minted under an epoch the
  resource has moved past — is `CommitmentRefusal::Mandate(MandateRefusal::Superseded { installed, presented })`,
  refused **before any write**; a passing check commits linearizably. Item 5 is present, so the negative case
  runs in CI rather than being skipped; a deployment without mandates uses `commit_award_linearizable`.

### Added — the linearizable award and the double-award witness (v3 §6.9, CN2 — partial)

- **`ContractNet::{plan_award, commit_award, commit_award_linearizable, award_linearizable}`** — the plain
  award split into plan and commit, and a **linearizable** commit through a consensus round on the slot
  `cn/{requirement}/award` (the plan's "a `group_propose` round where the award must be linearizable"):
  of two declarers racing exactly one commits, the other is `AlreadyAwarded` **with the committed award**; a
  round with no commit is `AwardUnknown { ballots_tried }` — the award may have committed elsewhere, a retry
  resolves it. `award_of` reads the consensus slot before the KV head. `AwardedLinearizable { award, commit }`
  carries item 1's `CommitReceipt`.
- **The witness, as an explicit interleaving** (the kernel has no scheduler seam): two declarers plan, then
  commit — the plain path lets both commit and LWW keeps one silently; the linearizable path refuses the
  second. **Not landed, pinned as a gap:** the replay half. A whole-node recording of a linearizable award
  diverges at the interleaving of the membership governor's jitter draw and the round's defer timer; the
  test `a_whole_node_recording_of_a_linearizable_award_diverges_without_a_scheduler_seam` asserts the
  divergence and names it, so the scheduler seam's arrival flips it into the claim.
- **`mycelium::sim_seam`** re-exported under the `sim` feature, so a companion on the public API can record
  and replay its own runs. `mycelium-commitment` now enables `mycelium/consensus`.

### Added — admission control at the companions' queues, reported (item 4 PR 5)

- **`mycelium-tuple-space`:** each stage counts the `put`s its watermark refused; **`TupleSpace::admission(stage)`**
  reports `admitted` · `rejected` · `taken` · `high_watermark` per stage — `Some` at the primary, `None` on a
  secondary (*ask the primary*, not *nothing was refused*: the depth RPC's fixed encoding cannot carry it) — and
  the metrics writer publishes `…/stage/{stage}/rejected_total` beside the other counters. The bound itself is
  unchanged; what changes is that a rejection is now a visible outcome beside completions (adaptive-stability §1).
- **`mycelium-blackboard`:** its first admission bound — **`BoardConfig.high_watermark: Option<u64>`** (`None` =
  unbounded, the behaviour before) — refusing `post` with **`BlackboardError::Backpressure { available,
  high_watermark }`**, counted in **`BoardStats.rejected`**, crossing the RPC as itself (status code 2; a pre-2.8
  client sees a generic `Rpc` error, never a false success). Replication and WAL replay never refuse. Two-step
  and self-imposed: a burst can briefly overshoot; a Tier B bound, not a hard one.
- Example `mycelium-tuple-space/examples/admission.rs`; runbook `docs/operations/admission-control.md`.
  **Upgrade notes:** `BoardConfig` and `BoardStats` gained a field each — exhaustive struct literals break
  (`..Default::default()` / `stats()` are unaffected); §6.6 of the plan names `BoardConfig`.

### Added — the commitment companion (v3 §6.9, CN1)

- **`mycelium-commitment`** — the contract net as five records on the public API, a composition and not a
  subsystem: `ContractNet::{announce, offer, award, report, assess}` over `kv().set` / `append` / `scan_log` /
  `set_with_receipt`. The award is chosen by `AwardRule::LowestParticipant` (the tuple-space election's rule,
  pure) and written **with a receipt** under `cn/{requirement}/award` (`Awarded { award, receipt }`); a second
  award is refused (`AlreadyAwarded`), never overwritten; `NoOffers` and a late offer are visible states;
  assessments are Ed25519-signed with a caller's key and `verify_assessment` is `false` for an unsigned one.
  `kv_ns::CN` (`cn/`) registered in the namespace table. Gallery: `examples/redistribution_cn.rs` — the
  redistribution workload as a contract net — in CI (`commitment` job). Not yet: CN2 (replay, double-award
  witness), CN3 (mandate epoch at award).

### Added — replay scenario C, the interacting governors (item 6 PR 6)

- **`src/control/scenario_c.rs`** (test-only): the combined-feedback harness of `adaptive-stability.md` §5 (D19)
  — a schedule sweep (48 × 2 profiles) over the three governors' **shipped** pure decisions with their real
  spacing, settling and hysteresis, coupled through a small plant. With the loop-breakers on every run settles;
  with them off, schedules flap and chatter; a stale view holds routine scale-down under `EnforceLocal` and not
  under `Legacy`. The objectives are numbers on *releases* (the shed is never held), knob changes, rest after the
  last disturbance, and the decisive rule. **What it does not prove:** that hysteresis is load-bearing while the
  release spacing is on (a plant removing it was not caught — the spacing alone bounds the release rate), and the
  ADR's "oscillate together while each is stable alone". No shipped code changed; three `agent` submodules and the
  opacity decision functions became crate-visible for the harness.

### Added — the replay corpus and its gate (item 6 PR 7)

- **`mycelium-core/tests/replay-corpus/scenario-a-wal-snapshot/`** — the first checked-in failure bundle
  (scenario A, thirteen effects), recorded by `record_scenario_a_into_the_corpus` under
  `MYCELIUM_RECORD_CORPUS=1` and gated by
  `the_checked_in_scenario_a_bundle_replays_here_and_matches_a_fresh_recording` in every `--features sim` run:
  the committed schedule replays on the running machine without divergence, the witness toggle is still the one
  this build knows, and a fresh recording asks for the same effects in the same order. A change in the snapshot
  path's effects fails the suite until the corpus is re-recorded on purpose.
- **`mycelium_sim::bundle::Build::current(features)`** — the build identity as honestly known at compile time
  (crate version, `GITHUB_SHA` under CI, arch-os; `unknown` for the compiler).
- Not built: the minimiser and a replay binary.

### Changed — every periodic loop ticks through the timer seam (item 6 follow-on)

- The seven tickers #276 left unrouted now go through `sim_seam::interval_ms`: `kv-persist/{key}`,
  `mesh/emit/{kind}`, `mesh/stale/{kind}`, `persistence/snapshot`, `rate/decide` (the one `Delay` loop),
  `swim/probe` (its 1 ms floor kept) and `membership/tick`. Live behaviour is the same `tokio::time::interval`
  with the same missed-tick setting; under `sim` each tick is a recorded decision. Seams baseline 185 → 178.
- **`Ticker::reset_after_ms`** (both arms): the snapshot loop defers 30 s while the node is opaque for another
  reason, and a deferral is a decision — under `sim` the next recorded tick carries the deferral as its
  nominal wait (`tick(30000ms)`), so a trace shows where the loop chose to wait instead of a period that
  never elapsed. Pinned by `a_deferred_tick_records_the_deferral_then_resumes_the_period`.

### Added — the provisioner against the rights ledger (item 4 PR 4c)

- **`Provisioner::with_install_rights(ledger, holder, resource, signing_key)`** (`mycelium-wasm-host`, on
  the public API only): the ledger's first live user. Before an `Installing` reservation the round asks
  `may_admit(holder, resource, reserved + live + 1)` — a unit in flight is as consumed as one serving — and
  refuses otherwise: counted (`rights_refusals`, metric
  `mycelium_artifact_installs_refused_by_rights_total`) and **recorded** in the ledger as
  `admission.rejected`, off the synchronous admission path. A busy ledger is a refusal, not a wait.
- **`rights/head/{holder}`** is published on attach and after every refusal as
  **`control::ledger::PublishedRightsHead { head, signature }`** (new, `serde_fixint`, `encode`/`decode`/
  `is_signed`), signed with an operator-supplied Ed25519 key; `mycelium_wasm_host::verify_published_head`
  says `false` for an unsigned head — unsigned means unproven.
- The provisioner never allocates to itself: an unallocated node is refused every install, visibly. Without
  `with_install_rights` nothing changes. Lock-order table row 37.

### Fixed — the seams check sees timer calls through a module alias (item 6 follow-on)

- **`scripts/check-sim-seams.sh`** now counts `time::sleep|interval|timeout|Instant` in any file that imports
  `tokio::time` as a module (`use tokio::time;`, grouped `use tokio::{…, time, …}`, `time::{self, …}`,
  `time as <alias>`) — the `fs as <alias>` gap over again, for the timer, found while routing nine tickers
  (#276) and recorded in the script's header as open. The regenerated baseline admits **19 pre-existing
  sites in eight files (166 → 185)**; nothing in the tree changed, only what the check can see. Verified by
  planting: a `time::sleep` line fails the check at exactly +1 while a `std::time::Instant` type mention beside
  it adds nothing (the left guard, not `\b`), and commenting one alias site out reports as progress.

### Added — the tuning governor and the opacity gate through the contract (item 4 PR 4b)

- **Tuning governor:** `TuningGovernor::gate_at` / `acted_at` (pure in time) beside `gate` / `acted`
  (through the replay clock seam), and `set_control_timing`. The **reconcile step is the knob's
  readback**: an action is pending until a later `gate` sees the current value at what was applied
  (consumed) or the settle timeout passes (settled as *unknown* — counted and warned); a recommendation
  equal to the current value is not an action; a change inside `spacing_ms` of the last action is held
  (`None`, like a disabled gate). `acted` is separate from `gate` on purpose — a value a `ConfigPolicy`
  rejects runs no clock. `start_cluster_tuner` sets both timings to two ticks; both are `0` by default,
  which is the old gate exactly. **Upgrade note:** `GovernorSnapshot` gained `held_by_spacing`,
  `held_by_settling`, `settled_unknown` and `ParamSnapshot` gained `pending` — an exhaustive struct
  literal of either breaks; construct through `snapshot()`.
- **Opacity gate:** `OpacityHint.release_spacing_ms` (default `1_000`) — the minimum interval between
  the previous boundary transition and a **release** (`BOUNDARY_TRANSPARENT`). Only the release is
  spaced: going opaque is protective shedding, which the ADR's decisive rule says is never held, and the
  full-channel override stands. No settle state: the loop's own input is the effect channel. Tripwire
  `GossipAgent::opacity_releases_spaced`. **Upgrade note:** `OpacityHint` gained a field — an exhaustive
  literal breaks; the documented `..Default::default()` pattern is unaffected.
- ADR `docs/design/adaptive-stability.md` §9 row 4b marks the decisions.

### Added — the membership governor through the contract (item 4 PR 4a)

- **`GossipAgent::set_control_profile` / `control_profile` / `control_would_hold_count`** — the node's profile
  (`control::Profile`) as an atomic on the task context: no lock, no config-struct change. **Default `Legacy`:
  production behaviour is unchanged until an operator opts in.** `Observe` records what an enforcing profile
  would have held and holds nothing — the count is the number to watch before enforcing.
- **The membership governor consults the confidence predicate per pass**, on the real `ViewConfidence`, through
  a pure and tested `classify`: a join into an empty group is a rescue; a join below `min` is a **deficit
  fill** — a fifth action class, added by dated amendment to the ADR because the governor's own primary action
  fitted none of the four and by *cost* is a rescue, not speculation; a leave over `max` is routine scale-down,
  the one class here uncertainty holds (the view may be a partition, and leaving on a partition makes it worse);
  a drain is an operator's instruction and no class at all.
- **Settling is observed, not timed:** a join or leave is pending until the group's membership reflects it, and
  no new proposal is made on that group until then or until the settle timeout (two health-check intervals),
  after which it is settled as `unknown`. The cooldown keeps its meaning exactly, as `ControlSpec.spacing_ms`.
- The governor's own nondeterminism now goes through the seams — the roll from the `govern` stream, the jitter
  from `jitter` and the timer seam, the cooldown on the monotonic seam — so a replay elects the same. Its seams
  baseline drops **6 → 1** (`tokio::time::interval`, which has no seam arm yet).
- Three tests (the class mapping, the join/leave asymmetry, the profile's stored form round-trip); the live
  `test_membership_governor_converges_to_min` passes unchanged under the default.

### Added — gateway/SDK receipt parity (item 1 PR 7)

- **`"local_durability"` beside `"persisted"`** in the JSON of the two consensus-backed gateway
  verbs (`overlay/consistent/set`, `cross_group_propose`): the commit receipt's `LocalDurability`,
  named `on_disk` · `buffered` · `not_configured` · `failed` (`LocalDurability::tag`, pinned by a
  test — a rename is a wire change), with `"local_durability_error"` present only for `failed`.
  `"persisted"` is unchanged: it is read off the result *before* it becomes a receipt, so its
  regression-floor pin does not move. Both handlers now go through the same `receipt_from` as
  `cluster_propose_receipt`, so HTTP cannot drift from Rust. Additive: a pre-2.8 client reads
  `persisted` and ignores the rest.
- **SDKs:** `mycelium-py` `CommitResult.local_durability` / `.local_durability_error` / `.on_disk`;
  `mycelium-ts` `CommitResult.localDurability` / `.localDurabilityError` and the `LocalDurability`
  type. Absent fields (a pre-2.8 gateway) read as `None` / `null`; a stub-gateway test in each SDK
  pins the four names and that `persisted` is untouched.
- **CI:** `mycelium-py/tests/test_commit_result.py` joins the Python job's list — it has been
  node-free since 2026-09-05 and never ran.

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

### Added — the tuple-space consumer with effect recovery (item 1 PR 6)

- **`mycelium-effects::tuple_consumer`** (feature `tuple-space`, optional so the crate's core couples to neither
  companion): a worker that `take`s an item, **commits its effect at the destination first**, and only then
  `ack`s or `complete`s. The operation id is derived from the item — `tuple/{namespace}/{stage}/{id}` — and the
  tuple id survives lease expiry and a WAL restart, so a re-delivered item is a *new attempt of the same
  operation* and its apply comes back `Replayed`. That is the effect recovery the receipts record's §7 promised
  for PR 5/6: `complete` is the pipeline's receipt and never stands in for the destination's.
- **A refused effect is not acknowledged.** The item stays in flight for re-delivery; `DeliveryUnknown` and
  `Failed` resolve on retry, and `Conflict` is surfaced as the poison pill it is — dead-lettering is the caller's
  policy, because acknowledging it would make a conflicting effect vanish.
- Three live tests (a primary over a WAL on a bind-verified port): a worker that committed and died before
  acknowledging is re-delivered on restart and replays with one row; a refused effect's item comes back after
  restart with its id intact — the test that pins the ordering, since the replay test alone would pass for a
  consumer that acked first; and the ordinary fresh path. The ordering inversion is verified by planting it.

### Added — the effects companion: the destination-commit receipt as a reference destination (item 1 PR 5)

- **`mycelium-effects`**, a new companion crate on the public API. The substrate provides three of item 1's four
  receipts and never the fourth — *destination commit* — because only the caller's resource can say whether an
  effect happened there. This crate is that last rung made generic and inspectable: an `EffectDestination` whose
  **dedup row (keyed by `operation_id`) and business change commit in one transaction**, answering with
  `DestinationCommit { destination, dedup: Fresh | Replayed }`.
- **`SqliteDestination`**, the reference destination (`rusqlite`, bundled and quarantined to this crate; core carries
  no database). An `IMMEDIATE` transaction takes the write lock at `BEGIN`, so appliers racing on one operation
  serialise at the database and exactly one is `Fresh`. The business change is the caller's handler, run *inside*
  the transaction; if it fails, whatever it wrote goes with the rollback and **no dedup row is left**, so a retry
  starts clean rather than finding a false `Replayed`.
- **The vocabulary, as refusals that say what is true afterwards:** same `operation_id` with different content is
  `Conflict` (the first version stands — a retry that changed its mind is not a retry); a failed transaction is
  `Failed` (nothing committed); a deadline overrun is **`DeliveryUnknown`, never "nothing happened"** — the apply
  is not cancelled and may commit after the caller stopped waiting, which is exactly the case the retry resolves as
  `Replayed`. `apply_within` maps a timeout to it.
- The effect's `content_hash` is the receipt vocabulary's own `content_hash(operation_id, payload, false)` — one
  specified, golden-pinned hash for the axis, not a second one.
- Seven tests; the dedup lookup and the rollback-on-failure each verified by planting their absence. The in-process
  half — the local fiber runtime of the plan's §13 — is beyond v3 (D34) and not here.

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

- **Per-peer gossip rate-limiting** — `GossipConfig::max_inbound_frames_per_sec` (also `GOSSIP_MAX_INBOUND_FRAMES_PER_SEC` env var). When set to a non-zero value, frames received faster than this rate from a single peer are dropped with a warning log. Prevents a malicious or misbehaving peer from flooding the inbound processing pipeline. Default `0` = unlimited (existing behaviour preserved).
- **`bulk_serve` handler concurrency cap** — `GossipConfig::max_concurrent_bulk_handlers` (also `GOSSIP_MAX_CONCURRENT_BULK_HANDLERS` env var). Limits the number of concurrent per-request background tasks spawned by `bulk_serve` via a `tokio::sync::Semaphore`. When the cap is reached, new bulk signals are dropped with a warning. Default `64`; set to `0` for unlimited.

### Changed

- **`GossipError::Config(String)` replaced by three structured variants** — `InvalidField { field: &'static str, reason: String }`, `FieldConflict { field_a, field_b, reason }`, `NodeIdMismatch { node_id, bind_addr }`. Callers can now match specific configuration failures without parsing error strings. All `validate()` and `apply_env_overrides()` error paths updated.
- **`GossipError::Network(String)` replaced by two structured variants** — `FrameTooLarge { size: usize, limit: usize }` and `UnsupportedWireVersion { received: u8, current: u8, prev: u8, hint: &'static str }`. Framing errors are now fully typed; callers can distinguish oversized frames from version mismatches.

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
